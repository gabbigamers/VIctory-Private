//! Emulated waves (`emit`, "Emulated waves"): a compute program whose
//! thread group is exactly one console wave, lowered so that the group
//! itself is that wave, whatever waves the host makes of its threads.
//!
//! The program is in its wave form ([`crate::structure::waves`]): every
//! thread runs every block the wave runs, a branch on a lane mask is one
//! decision for all of them, and EXEC is a thread's own bit. A thread's
//! lane is its flattened id in the group. What one lane learns of another
//! goes through group-shared memory, past the program's own:
//!
//! * a dword per lane, where a lane puts a value another lane reads
//!   ([`Lower::emu_read_lane`]);
//! * two dwords holding a lane mask, where each lane sets or clears its own
//!   bit ([`Lower::emu_ballot`]);
//! * sixteen dwords, where lane 0 puts what the wave loads once
//!   ([`Lower::emu_shared`]).
//!
//! Between writing and reading the lanes meet: a group barrier that waits
//! for every thread, with the memory fences. They meet again after reading,
//! so the next exchange cannot overwrite what a slower thread has yet to
//! read. Every thread reaches every meeting, because in the wave form the
//! only branches that differ between threads are the ones this lowering
//! makes around a lane's own stores ([`Lower::begin_guarded`]), and an
//! exchange asked for inside one is refused.
//!
//! No instruction of the result depends on how the host divides the group
//! into waves: it has no wave intrinsic and declares no wave size.
//!
//! Two things the console's wave gives by running one instruction in all
//! lanes before the next are kept explicitly:
//!
//! * **Memory order.** On the console an access is done in every lane
//!   before any lane's later one. Here the lanes meet before a memory
//!   access whenever a lane may have written that kind of memory (the
//!   program's shared memory; anything else) since they last met, and
//!   before a write whenever a lane may have read it since ([`meetings`]).
//!   Two lanes that write one address in the same instruction are in no
//!   order here; which of them stays on the console is not modelled.
//! * **One load for the wave.** A scalar load happens once on the console.
//!   Here every thread would load for itself, and two threads reading
//!   memory that something else is writing could go different ways and
//!   never meet again; lane 0's load is the wave's.

use std::collections::HashMap;

use super::{Lower, Parts, Val, Why};
use crate::emit::bitcode::{Attr, Bin, Rmw, V, pred};
use crate::gfx10::Fields;
use crate::ir::{Inst, Program};
use crate::native::StageIo;
use crate::resource::{CODE, MEMORY, Site, USER_DATA};
use crate::structure::Bool;

/// Dwords of a lane mask in the meeting memory.
const MASK: u32 = 2;
/// Dwords of what the wave loads once: the largest scalar load's.
const SHARED: u32 = 16;

/// The meeting memory's size in dwords for a wave of `wave` lanes.
pub(super) const fn dwords(wave: u32) -> u32 {
    wave + MASK + SHARED
}

/// What an emulated wave cannot do, as the translation's refusals name it.
pub(super) const SEVERAL_WAVES: Why = "an emulated wave in a thread group of several waves";
pub(super) const PARTIAL_WAVE: Why = "an emulated wave in a thread group that does not fill it";
pub(super) const NOT_COMPUTE: Why = "an emulated wave outside compute";
pub(super) const SOME_LANES: Why = "lanes meeting where only some of them run";

/// The state of an emulated wave's lowering.
pub(super) struct Emulated {
    /// Lanes of the console wave, which is the thread group.
    pub wave: u32,
    /// The first dword of the meeting memory in group-shared memory.
    base: u32,
    /// The thread's lane, made in the entry block.
    lane: Option<V>,
    /// Ballots taken, by block and the lanes' bit.
    ballots: HashMap<(usize, V), (V, V)>,
    /// Per IR block and instruction: the lanes meet before it
    /// ([`meetings`]).
    meet_before: Vec<Vec<bool>>,
    /// The meetings fence unordered access views too.
    fence_views: bool,
    /// Meetings written.
    pub meetings: u32,
}

/// How an instruction uses memory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Use {
    /// Each lane reads for itself.
    Read,
    /// Each lane writes (or reads and writes: an atomic operation).
    Write,
    /// A scalar load: one read for the wave, which the lanes share through
    /// an exchange of their own ([`Lower::emu_shared`]) and so have met
    /// after by the time any of them goes on.
    Load,
}

/// The memory an instruction uses, and how: 0 the program's shared memory,
/// 1 any other.
fn access(inst: &Inst) -> Option<(usize, Use)> {
    let n = inst.name();
    let lane = |writes| if writes { Use::Write } else { Use::Read };
    match inst.fields {
        Fields::Ds { .. } if n.starts_with("ds_swizzle") || n.contains("permute") => None,
        Fields::Ds { .. } => Some((0, lane(!n.starts_with("ds_read")))),
        Fields::Smem { .. } => Some((1, Use::Load)),
        Fields::Buffer { .. } | Fields::Image { .. } | Fields::Flat { .. } => {
            Some((1, lane(n.contains("store") || n.contains("atomic"))))
        }
        _ => None,
    }
}

/// What the lanes may have done to one kind of memory since they last met.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
struct Since {
    read: bool,
    written: bool,
}

impl Since {
    fn or(self, other: Since) -> Since {
        Since { read: self.read | other.read, written: self.written | other.written }
    }
}

/// One instruction's part in [`meetings`]: whether the lanes meet before
/// it, given what they did to each kind of memory `since` they last met,
/// which it updates.
///
/// On the console an instruction is done in every lane before the next
/// one starts. Between two meetings the lanes here run in any order, so two
/// instructions on the same kind of memory may share a stretch only if both
/// read: a write must not land before another lane's earlier read (that
/// lane would read what it should not see yet) or earlier write (which
/// would then be the one that stays), and a read must not come before
/// another lane's earlier write.
fn settle(inst: &Inst, since: &mut [Since; 2]) -> bool {
    if inst.name() == "s_barrier" {
        // The program's own barrier is a meeting with the same fences.
        *since = [Since::default(); 2];
        return false;
    }
    let Some((kind, how)) = access(inst) else {
        return false;
    };
    let meet = match how {
        Use::Read | Use::Load => since[kind].written,
        Use::Write => since[kind].written || since[kind].read,
    };
    if meet {
        *since = [Since::default(); 2];
    }
    match how {
        Use::Read => since[kind].read = true,
        Use::Write => since[kind].written = true,
        // The lanes meet again before they go on.
        Use::Load => {}
    }
    meet
}

/// Per block and instruction, whether the lanes meet before it so that
/// memory is as the console's wave would have left it, and each lane reads
/// of it what the console's lane would: before every access to a kind of
/// memory some lane may have written since the lanes last met, and before
/// every write to one some lane may have read since, along any path
/// ([`settle`]). (Exchanges are meetings too and are not counted: a meeting
/// more than needed changes nothing.)
fn meetings(p: &Program) -> Vec<Vec<bool>> {
    let order = crate::ir::reverse_postorder(&p.blocks);
    let mut entry = vec![[Since::default(); 2]; p.blocks.len()];
    loop {
        let mut changed = false;
        for &b in &order {
            let mut since = entry[b];
            for inst in &p.blocks[b].insts {
                settle(inst, &mut since);
            }
            for s in p.blocks[b].exit.successors() {
                let joined = [entry[s][0].or(since[0]), entry[s][1].or(since[1])];
                changed |= joined != entry[s];
                entry[s] = joined;
            }
        }
        if !changed {
            break;
        }
    }
    (0..p.blocks.len())
        .map(|b| {
            let mut since = entry[b];
            p.blocks[b].insts.iter().map(|inst| settle(inst, &mut since)).collect()
        })
        .collect()
}

impl Emulated {
    /// The emulation of `x`'s program with its meeting memory after
    /// `lds_dwords` dwords of the program's own, or why it has none.
    pub(super) fn plan(x: &Parts, lds_dwords: u32) -> Result<Emulated, Why> {
        let StageIo::Cs(cs) = x.native.io else {
            return Err(NOT_COMPUTE);
        };
        let wave: u32 = if x.native.wave64 { 64 } else { 32 };
        // A lane is a thread of the group and every lane must be one: a
        // barrier waits for the whole group, and a second wave of the group
        // would take its own branches.
        let threads = cs.threads.iter().fold(1u64, |count, &n| count.saturating_mul(u64::from(n)));
        if threads > u64::from(wave) {
            return Err(SEVERAL_WAVES);
        }
        if threads < u64::from(wave) {
            return Err(PARTIAL_WAVE);
        }
        let insts = || x.p.blocks.iter().flat_map(|b| &b.insts);
        Ok(Emulated {
            wave,
            base: lds_dwords,
            lane: None,
            ballots: HashMap::new(),
            meet_before: meetings(x.p),
            fence_views: insts().any(|i| access(i) == Some((1, Use::Write))),
            meetings: 0,
        })
    }
}

impl Lower<'_> {
    /// Whether the lanes meet before instruction `i` of IR block `b`.
    pub(super) fn emu_meets_before(&self, b: usize, i: usize) -> bool {
        self.emu.as_ref().is_some_and(|e| e.meet_before[b][i])
    }

    /// The thread's lane of the emulated wave: its place in the group.
    pub(super) fn emu_lane(&mut self) -> V {
        if let Some(lane) = self.emu.as_ref().expect("an emulated wave").lane {
            return lane;
        }
        let (entry, i32t) = (self.s.entry, self.s.t.i32);
        let lane = self.s.op(
            entry,
            "flattenedThreadIdInGroup.i32",
            Attr::ReadNone,
            i32t,
            96,
            &[],
        );
        self.emu.as_mut().expect("an emulated wave").lane = Some(lane);
        lane
    }

    /// Whether every lane runs the code being written at `at`: an exchange
    /// anywhere else would leave the others waiting.
    fn emu_all_lanes(&self, at: usize) -> Result<(), Why> {
        if at == self.cur && self.in_guarded > 0 {
            return Err(SOME_LANES);
        }
        Ok(())
    }

    /// The lanes meet in `at`: a group barrier every thread waits at, with
    /// the fences that make what each wrote visible to the others.
    pub(super) fn emu_meet(&mut self, at: usize) {
        let e = self.emu.as_mut().expect("an emulated wave");
        e.meetings += 1;
        // Thread-group sync and the shared-memory fence; the fence of
        // unordered access views where the program writes any.
        let mode = self.s.m.i32(if e.fence_views { 1 | 2 | 8 } else { 1 | 8 });
        let void = self.s.t.void;
        self.s.op(at, "barrier", Attr::NoDuplicate, void, 80, &[mode]);
    }

    /// The address of dword `index` past `first` of the meeting memory.
    fn emu_cell(&mut self, at: usize, first: u32, index: Option<V>) -> V {
        let base = self.emu.as_ref().expect("an emulated wave").base;
        let k = self.s.m.i32(base + first);
        let index = match index {
            Some(i) => self.s.m.bin(at, Bin::Add, k, i),
            None => k,
        };
        let lds = self.s.lds();
        self.s.m.element(at, lds, index)
    }

    /// This lane sets or clears its bit of the mask in the meeting memory.
    /// Every lane does, so no bit keeps an older value.
    fn emu_put_bit(&mut self, at: usize, bit: V) {
        let wave = self.emu.as_ref().expect("an emulated wave").wave;
        let lane = self.emu_lane();
        let (k31, k5, one, zero, ones) = (
            self.s.m.i32(31),
            self.s.m.i32(5),
            self.s.m.i32(1),
            self.s.m.i32(0),
            self.s.m.i32(u32::MAX),
        );
        let sh = self.s.m.bin(at, Bin::And, lane, k31);
        let own = self.s.m.bin(at, Bin::Shl, one, sh);
        let others = self.s.m.bin(at, Bin::Xor, own, ones);
        let keep = self.s.m.select(at, bit, ones, others);
        let set = self.s.m.select(at, bit, own, zero);
        let half = self.s.m.bin(at, Bin::LShr, lane, k5);
        let cell = self.emu_cell(at, wave, Some(half));
        self.s.m.atomic(at, Rmw::And, cell, keep);
        self.s.m.atomic(at, Rmw::Or, cell, set);
    }

    /// The mask in the meeting memory, once the lanes have met.
    fn emu_get_mask(&mut self, at: usize) -> (V, V) {
        let wave = self.emu.as_ref().expect("an emulated wave").wave;
        let lo = self.emu_cell(at, wave, None);
        let lo = self.s.m.load(at, lo);
        let hi = if wave == 64 {
            let hi = self.emu_cell(at, wave + 1, None);
            self.s.m.load(at, hi)
        } else {
            self.s.m.i32(0)
        };
        (lo, hi)
    }

    /// The lanes of the wave whose `bit` holds in block `at`, as the two
    /// dwords of a lane mask (the high one 0 for a wave of 32).
    pub(super) fn emu_ballot(&mut self, at: usize, bit: V) -> Result<(V, V), Why> {
        let e = self.emu.as_ref().expect("an emulated wave");
        let wave = e.wave;
        if let Some(&mask) = e.ballots.get(&(at, bit)) {
            return Ok(mask);
        }
        if let Some(k) = self.s.m.int_value(bit) {
            // The same in every lane: all of them or none.
            let all = if k != 0 { u32::MAX } else { 0 };
            let high = if wave == 64 { all } else { 0 };
            return Ok((self.s.m.i32(all), self.s.m.i32(high)));
        }
        self.emu_all_lanes(at)?;
        self.emu_put_bit(at, bit);
        self.emu_meet(at);
        let mask = self.emu_get_mask(at);
        self.emu_meet(at);
        self.emu.as_mut().expect("an emulated wave").ballots.insert((at, bit), mask);
        Ok(mask)
    }

    /// `x` as lane `lane` of the wave holds it (the lane number wraps at
    /// the wave size), read in the block being written.
    pub(super) fn emu_read_lane(&mut self, x: V, lane: V) -> Result<V, Why> {
        let at = self.cur;
        self.emu_all_lanes(at)?;
        let wave = self.emu.as_ref().expect("an emulated wave").wave;
        let me = self.emu_lane();
        let mine = self.emu_cell(at, 0, Some(me));
        self.s.m.store(at, mine, x);
        self.emu_meet(at);
        let last = self.s.m.i32(wave - 1);
        let lane = self.s.m.bin(at, Bin::And, lane, last);
        let theirs = self.emu_cell(at, 0, Some(lane));
        let r = self.s.m.load(at, theirs);
        self.emu_meet(at);
        Ok(r)
    }

    /// `v_readfirstlane_b32`: `x` of the lowest lane whose `exec` bit is
    /// set (`None`: every lane's is), and of lane 0 when no lane's is.
    pub(super) fn emu_first_lane(&mut self, x: V, exec: Option<V>) -> Result<V, Why> {
        let Some(exec) = exec.filter(|&e| self.s.m.int_value(e).is_none()) else {
            let zero = self.s.m.i32(0);
            return self.emu_read_lane(x, zero);
        };
        let at = self.cur;
        self.emu_all_lanes(at)?;
        // One exchange for both: each lane gives its value and its bit.
        let me = self.emu_lane();
        let mine = self.emu_cell(at, 0, Some(me));
        self.s.m.store(at, mine, x);
        self.emu_put_bit(at, exec);
        self.emu_meet(at);
        let (lo, hi) = self.emu_get_mask(at);
        // The lowest set bit of the two dwords; lane 0 for none.
        let i32t = self.s.t.i32;
        let (zero, k32) = (self.s.m.i32(0), self.s.m.i32(32));
        let first_lo = self.s.op(at, "unaryBits.i32", Attr::ReadNone, i32t, 32, &[lo]);
        let first_hi = self.s.op(at, "unaryBits.i32", Attr::ReadNone, i32t, 32, &[hi]);
        let first_hi = self.s.m.bin(at, Bin::Add, first_hi, k32);
        let none_hi = self.s.m.cmp(at, pred::EQ, hi, zero);
        let first_hi = self.s.m.select(at, none_hi, zero, first_hi);
        let none_lo = self.s.m.cmp(at, pred::EQ, lo, zero);
        let first = self.s.m.select(at, none_lo, first_hi, first_lo);
        let theirs = self.emu_cell(at, 0, Some(first));
        let r = self.s.m.load(at, theirs);
        self.emu_meet(at);
        Ok(r)
    }

    /// The wave's decision at branch `n` of [`crate::structure::Structured::wave`]:
    /// whether some lane's bit holds, or whether every lane's does.
    pub(super) fn emu_decision(&mut self, n: u32, at: usize) -> Result<V, Why> {
        let Some(wave) = self.emu.as_ref().map(|e| e.wave) else {
            return Err("a wave's branch outside an emulated wave");
        };
        let branch = self.x.st.wave[n as usize];
        // The ballot of a negated bit is the ballot of the bit, read the
        // other way round: often a mask the program has just read as a
        // number, whose ballot is then taken once.
        let (bit, negated) = match self.x.st.bools.node(branch.when) {
            Some((atom, Bool::TRUE, Bool::FALSE)) => (self.atom(atom, at)?, true),
            _ => (self.boolean(branch.when, at)?, false),
        };
        let (lo, hi) = self.emu_ballot(at, bit)?;
        // "Some lane's `when`" is "not every lane's bit" of a negated bit,
        // and "every lane's `when`" is "no lane's bit".
        let some = branch.any != negated;
        let (zero, ones) = (self.s.m.i32(0), self.s.m.i32(u32::MAX));
        let (word, full) = if some {
            (self.s.m.bin(at, Bin::Or, lo, hi), zero)
        } else if wave == 64 {
            (self.s.m.bin(at, Bin::And, lo, hi), ones)
        } else {
            (lo, ones)
        };
        // `some`: a bit is set; else: every bit is. Negated, the opposite.
        let p = if some != negated { pred::NE } else { pred::EQ };
        Ok(self.s.m.cmp(at, p, word, full))
    }

    /// After a scalar load from memory: what it loaded becomes what lane 0
    /// loaded, in every lane (the module's notes, "One load for the wave").
    /// What a thread reads of its root arguments, of a descriptor or of the
    /// program's own code is the same in every thread already.
    pub(super) fn emu_shared(&mut self, inst: &Inst, site: Site) -> Result<(), Why> {
        if self.emu.is_none() {
            return Ok(());
        }
        let from_memory = match self.scalar.get(&site).map(|a| a.param) {
            Some(CODE | USER_DATA) => false,
            Some(MEMORY) => true,
            _ => inst.name().starts_with("s_buffer_load_dword"),
        };
        if !from_memory {
            return Ok(());
        }
        let loaded: Vec<_> = inst
            .writes
            .iter()
            .filter_map(|&(_, w)| match self.vals.get(&w) {
                Some(&Val::I32(x)) if self.s.m.int_value(x).is_none() => Some((w, x)),
                _ => None,
            })
            .collect();
        if loaded.is_empty() {
            return Ok(());
        }
        if loaded.len() > SHARED as usize {
            return Err("a scalar load of more than sixteen dwords in an emulated wave");
        }
        let at = self.cur;
        self.emu_all_lanes(at)?;
        let wave = self.emu.as_ref().expect("an emulated wave").wave;
        let lane = self.emu_lane();
        let zero = self.s.m.i32(0);
        let first = self.s.m.cmp(at, pred::EQ, lane, zero);
        let guarded = self.begin_guarded(Some(first));
        for (k, &(_, x)) in loaded.iter().enumerate() {
            let cell = self.emu_cell(self.cur, wave + MASK + k as u32, None);
            self.s.m.store(self.cur, cell, x);
        }
        self.end_guarded(guarded, &[]);
        let at = self.cur;
        self.emu_meet(at);
        for (k, &(w, _)) in loaded.iter().enumerate() {
            let cell = self.emu_cell(at, wave + MASK + k as u32, None);
            let x = self.s.m.load(at, cell);
            self.vals.insert(w, Val::I32(x));
        }
        self.emu_meet(at);
        Ok(())
    }
}
