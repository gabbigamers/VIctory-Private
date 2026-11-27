//! Control flow: block exits and edges, loop counters, the paths lanes
//! take, and the phis, with what decides whether a value joined at one
//! is read.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::{Lower, MaskData, Stop, Val, Why, unsupported};
use crate::emit::bitcode::{Attr, Bin, Term, V, pred};
use crate::gfx10::{Fields, NULL, Operand};
use crate::io::Elsewhere;
use crate::ir::{Block, EXEC, Exit, Inst, Phi, Reg, Value};
use crate::resource::Site;
use crate::structure::{Atom, Bool, LaneExit};
#[cfg(doc)]
use super::LOOP_LIMIT;

/// The scalar instruction defining an SSA value in this block and its operands.
fn scalar_def<'a, const N: usize>(
    block: &'a Block,
    value: Value,
    op: &str,
) -> Option<(&'a Inst, [Operand; N])> {
    let inst = block
        .insts
        .iter()
        .find(|i| i.writes.iter().any(|w| w.1 == value))?;
    let Fields::Sop { src, .. } = &inst.fields else {
        return None;
    };
    (inst.name() == op).then_some((inst, src.as_slice().try_into().ok()?))
}

fn sgpr_operand(inst: &Inst, operand: Operand) -> Option<Value> {
    match operand {
        Operand::Scalar {
            code: code @ 0..=101,
            dwords: 1,
        } => inst.read(Reg::S(code)),
        _ => None,
    }
}

fn scalar_constant(operand: Operand) -> Option<u32> {
    match operand {
        Operand::Const(code) => crate::resource::inline(code),
        Operand::Literal(value) => Some(value),
        _ => None,
    }
}

impl<'a> Lower<'a> {
    /// Whether no lane that runs the instruction at `site` has been where
    /// `unless` says: a lane that came along such an edge or ran (skipped)
    /// such an instruction meets its condition, which the instruction's must
    /// contradict. Those lane bits must not change in between, so none of
    /// those blocks is on a cycle. A lane mask joined where such an edge
    /// enters holds, for the lanes that came along it, the edge's mask.
    pub(super) fn avoids(&self, (b, i): Site, unless: &[Elsewhere]) -> bool {
        let st = self.x.st;
        let runs = [
            (st.reach[b], true),
            (st.guards[b][i].unwrap_or(Bool::TRUE), true),
        ];
        let mut atoms = st.bools.support(runs[0].0);
        atoms.extend(st.bools.support(runs[1].0));
        unless.iter().all(|&u| {
            let (mut all, at, mut links) = match u {
                Elsewhere::Edge(q, to) => {
                    let all = self.lane_edge(q, to);
                    let block = &self.x.p.blocks[to];
                    let n = block.preds.iter().position(|&p| p == q).expect("an edge");
                    let links: Vec<(Bool, Bool)> = block
                        .phis
                        .iter()
                        .filter(|ph| atoms.contains(&Atom::Phi(ph.value)))
                        .filter_map(|ph| {
                            Some((*st.masks.get(&ph.value)?, *st.masks.get(&ph.args[n])?))
                        })
                        .collect();
                    (all, to, links)
                }
                Elsewhere::Write((w, k)) | Elsewhere::Skip((w, k)) => {
                    let guard = st.guards[w][k].unwrap_or(Bool::TRUE);
                    let ran = matches!(u, Elsewhere::Write(_));
                    (vec![(st.reach[w], true), (guard, ran)], w, vec![])
                }
            };
            all.extend(runs);
            // Each joined mask equals the incoming one: try both values.
            links.truncate(6);
            let some_lane = (0..1u32 << links.len()).any(|bits| {
                let mut all = all.clone();
                for (k, &(joined, incoming)) in links.iter().enumerate() {
                    let v = bits >> k & 1 != 0;
                    all.extend([(joined, v), (incoming, v)]);
                }
                st.bools.meet(&all)
            });
            !self.on_cycle(at) && !some_lane
        })
    }

    /// What holds in a lane that goes from IR block `q` to `b`.
    fn lane_edge(&self, q: usize, b: usize) -> Vec<(Bool, bool)> {
        let st = self.x.st;
        let mut all = vec![(st.reach[q], true)];
        if let LaneExit::Branch {
            when,
            taken,
            not_taken,
        } = st.exits[q]
            && taken != not_taken
        {
            all.push((when, taken == b));
        }
        all
    }

    /// Whether every lane enters IR block `b` along the edge the console's
    /// wave takes, so that an SGPR joined there holds what that edge
    /// brought: every branch into it is uniform (SCC values). The wave runs
    /// one path; a per-lane branch into the join (an EXEC skip, a loop's
    /// per-lane exit) is where lanes would take edges the wave did not, and
    /// the console compiler's if/else always skips into its join that way.
    /// Lanes that left EXEC inside a uniform loop (the BVH walks) take no
    /// edge into its header; the others take the wave's.
    fn uniform_join(&self, b: usize) -> bool {
        let st = self.x.st;
        let preds = &self.x.p.blocks[b].preds;
        preds.iter().filter(|&&q| self.block[q].is_some()).all(|&q| match st.exits[q] {
            LaneExit::Branch { when, taken, not_taken } if taken != not_taken => {
                st.bools.support(when).iter().all(|a| matches!(a, Atom::Uniform(_)))
            }
            _ => true,
        })
    }

    /// Whether no lane goes from IR block `q` to `b`.
    pub(super) fn never_taken(&self, q: usize, b: usize) -> bool {
        !self.x.st.bools.meet(&self.lane_edge(q, b))
    }

    /// Whether, at a per-lane join `b` of a lane mask whose register holds
    /// data along edge `n`, some edge brings a mask and no lane taking edge
    /// `n` is in any of them (a compare inside a region those lanes skip).
    /// The console wave runs one path: on a mask edge, the lanes of edge `n`
    /// read their bit of that mask, 0. The compiler merges no value for them,
    /// so it holds none for them either where the whole wave skips (the data
    /// there is a reused register): 0 is theirs.
    fn no_mask_lane_on_edge(&self, b: usize, args: &[Value], n: usize) -> bool {
        let st = self.x.st;
        let preds = &self.x.p.blocks[b].preds;
        let on_n = self.lane_edge(preds[n], b);
        let mut some = false;
        for (k, arg) in args.iter().enumerate() {
            let Some(&m) = st.masks.get(arg).filter(|_| k != n) else {
                continue;
            };
            some = true;
            let mut lanes = on_n.clone();
            lanes.push((m, true));
            if st.bools.meet(&lanes) {
                return false;
            }
        }
        some
    }

    /// Whether IR block `b` can reach itself.
    fn on_cycle(&self, b: usize) -> bool {
        let blocks = &self.x.p.blocks;
        let mut seen = vec![false; blocks.len()];
        let mut stack = blocks[b].exit.successors();
        while let Some(x) = stack.pop() {
            if x == b {
                return true;
            }
            if !std::mem::replace(&mut seen[x], true) {
                stack.extend(blocks[x].exit.successors());
            }
        }
        false
    }

    pub(super) fn exit(&mut self, b: usize) -> Result<(), Stop> {
        let offset = self.x.p.blocks[b]
            .insts
            .last()
            .map_or(self.x.p.blocks[b].offset, |i| i.machine.offset);
        let term = match self.x.st.exits[b] {
            LaneExit::Jump(t) if self.counters.contains_key(&t) && self.rpo[t] <= self.rpo[b] => {
                let ok = self.count_iteration(t, pred::IULT);
                let abort = self.abort_block();
                Term::CondBr(ok, self.block[t].expect("reachable"), abort)
            }
            LaneExit::Jump(t) => {
                self.enter_loop(b, t, self.cur);
                Term::Br(self.block[t].expect("reachable"))
            }
            LaneExit::Branch {
                when,
                taken,
                not_taken,
            } => {
                // A negated condition branches the other way round.
                let (cond, taken, not_taken) = match self.x.st.bools.node(when) {
                    Some((atom, Bool::TRUE, Bool::FALSE)) => {
                        let a = self
                            .atom(atom, self.cur)
                            .map_err(|w| unsupported(offset, w))?;
                        (a, not_taken, taken)
                    }
                    _ => {
                        let c = self
                            .boolean(when, self.cur)
                            .map_err(|w| unsupported(offset, w))?;
                        (c, taken, not_taken)
                    }
                };
                // A back edge is taken only below the iteration limit.
                let back =
                    |l: &Self, to: usize| l.counters.contains_key(&to) && l.rpo[to] <= l.rpo[b];
                // The latch's other side and the value of `cond` in the
                // lanes that take it of themselves.
                let leaves = if back(self, taken) {
                    Some((not_taken, false))
                } else if back(self, not_taken) {
                    Some((taken, true))
                } else {
                    None
                };
                // (No `xor` of a compare: a loop exit testing one hung the
                // AMD driver.)
                let latch = match leaves {
                    Some((_, false)) => {
                        let ok = self.count_iteration(taken, pred::IULT);
                        self.s.m.bin(self.cur, Bin::And, cond, ok)
                    }
                    Some((_, true)) => {
                        let stop = self.count_iteration(not_taken, pred::IUGE);
                        self.s.m.bin(self.cur, Bin::Or, cond, stop)
                    }
                    None => cond,
                };
                // Critical edges get a block of their own, so loops have
                // dedicated exits and preheaders as DXC emits them.
                let (mut t, mut f) = (self.edge(b, taken), self.edge(b, not_taken));
                // Where each side's edge arrives from: an edge block, or
                // here.
                let from = |l: &Self, to: usize, via: usize| {
                    if Some(via) == l.block[to] {
                        l.cur
                    } else {
                        via
                    }
                };
                let mut arrives = [from(self, taken, t), from(self, not_taken, f)];
                // The latch's branch keeps its shape; where it leaves the
                // loop, the lanes the limit stopped are told from the others.
                if let Some((to, own)) = leaves {
                    let (via, arrives) = if own {
                        (&mut t, &mut arrives[0])
                    } else {
                        (&mut f, &mut arrives[1])
                    };
                    let test = self.limit_exit(b, to, *via, cond, own);
                    if *arrives == self.cur {
                        *arrives = test;
                    }
                    *via = test;
                }
                for (to, from) in [(taken, arrives[0]), (not_taken, arrives[1])] {
                    if !back(self, to) {
                        self.enter_loop(b, to, from);
                    }
                }
                Term::CondBr(latch, t, f)
            }
            LaneExit::End => {
                if self.s.pixel && !self.exported[b] && !self.has_done_export(b) {
                    // A lane leaving without its final export was killed.
                    let t = self.s.m.bool(true);
                    self.s
                        .op(self.cur, "discard", Attr::NoUnwind, self.s.t.void, 82, &[t]);
                }
                Term::Ret
            }
            LaneExit::Indirect => return Err(unsupported(offset, "an indirect jump")),
        };
        let _ = Exit::End;
        self.s.m.terminate(self.cur, term);
        Ok(())
    }

    /// A forward edge into loop header `to` (from DXIL block `via`) starts
    /// its count at 0.
    fn enter_loop(&mut self, _from: usize, to: usize, via: usize) {
        if let Some((_, incoming)) = self.counters.get_mut(&to) {
            let z = self.s.m.i32(0);
            incoming.push((z, via));
        }
    }

    /// A back edge to header `to` (a back edge keeps its block, no
    /// critical-edge block): the count goes up; the result compares it with
    /// [`LOOP_LIMIT`] by `p` (below: go on; at or above: stop).
    pub(super) fn count_iteration(&mut self, to: usize, p: u8) -> V {
        let c = self.cur;
        let (phi, _) = self.counters[&to];
        let one = self.s.m.i32(1);
        let next = self.s.m.bin(c, Bin::Add, phi, one);
        let limit = self.s.m.i32(super::loop_limit());
        self.counters
            .get_mut(&to)
            .expect("header")
            .1
            .push((next, c));
        self.s.m.cmp(c, p, next, limit)
    }

    /// The block a lane stopped by [`LOOP_LIMIT`] leaves through.
    pub(super) fn abort_block(&mut self) -> usize {
        if let Some(a) = self.abort {
            return a;
        }
        let a = self.s.m.new_block();
        if self.s.pixel {
            let t = self.s.m.bool(true);
            self.s
                .op(a, "discard", Attr::NoUnwind, self.s.t.void, 82, &[t]);
        }
        self.s.m.terminate(a, Term::Ret);
        self.abort = Some(a);
        a
    }

    /// The DXIL block a branch from IR block `from` to `to` goes to.
    fn edge(&mut self, from: usize, to: usize) -> usize {
        let target = self.block[to].expect("reachable");
        let preds = self.x.p.blocks[to]
            .preds
            .iter()
            .filter(|&&q| self.block[q].is_some())
            .count();
        // Back edges stay as they are: the exiting block is the latch.
        if preds < 2 || self.rpo[to] <= self.rpo[from] {
            return target;
        }
        let e = self.s.m.new_block();
        self.s.m.terminate(e, Term::Br(target));
        self.edges.insert((from, to), e);
        e
    }

    /// Whether no lane entering IR block `b` from its `n`th predecessor
    /// reads what `reg` holds there before something overwrites it. The
    /// lane is followed along its per-lane branches, with the phis it
    /// passes taking that path's values; a branch whose compares have
    /// constant operands on the path is followed one way. Scalar arithmetic
    /// on the value passes it on; any other read of it, or a branch or guard
    /// that depends on it, makes it live. Paths meeting at a block with the
    /// same registers holding it go on with what they agree on. This is the
    /// console compiler's flag after a join (`if (c) { x = load; f = .. }
    /// if (f) use(x)`, with `f` 0 on the other path), where the register
    /// that holds `x` holds a stale value, often a lane mask, on that path.
    fn dead_on_edge(&self, b: usize, n: usize, reg: Reg) -> bool {
        let p = self.x.p;
        let st = self.x.st;
        // Values computed from the stale one on some path followed.
        let mut tainted = HashSet::new();
        let mut work = vec![(b, n, HashMap::new(), BTreeSet::from([reg]))];
        // Per block reached and registers holding tainted values on entry:
        // the phi values known there, over the paths so far.
        let mut at: HashMap<(usize, BTreeSet<Reg>), HashMap<Value, u32>> = HashMap::new();
        let mut visits = 0;
        while let Some((b, n, mut env, mut regs)) = work.pop() {
            let block = &p.blocks[b];
            let known: Vec<_> = block
                .phis
                .iter()
                .map(|phi| (phi.value, self.konst(&env, phi.args[n])))
                .collect();
            for (v, k) in known {
                match k {
                    Some(k) => env.insert(v, k),
                    None => env.remove(&v),
                };
            }
            for phi in &block.phis {
                if regs.contains(&phi.reg) {
                    tainted.insert(phi.value);
                }
            }
            let key = (b, regs.clone());
            if let Some(old) = at.get(&key) {
                let agreed: HashMap<Value, u32> = old
                    .iter()
                    .filter(|&(v, k)| env.get(v) == Some(k))
                    .map(|(&v, &k)| (v, k))
                    .collect();
                if agreed.len() == old.len() {
                    continue;
                }
                env = agreed;
            }
            at.insert(key, env.clone());
            visits += 1;
            if visits > 4 * p.blocks.len() + 64 {
                return false;
            }
            for (i, inst) in block.insts.iter().enumerate() {
                if regs.is_empty() {
                    break;
                }
                let guard = st.guards[b][i];
                if guard.is_some_and(|g| self.touches(g, &env, &tainted)) {
                    return false;
                }
                // The closing branch is the lane's exit, tested below.
                if inst.reads.iter().any(|r| regs.contains(&r.0))
                    && !inst.name().starts_with("s_cbranch")
                {
                    // Scalar arithmetic only passes the value on.
                    let pure = matches!(inst.fields, Fields::Sop { .. })
                        && inst.write(Reg::S(EXEC)).is_none()
                        && !inst.name().starts_with("s_setreg");
                    if !pure {
                        return false;
                    }
                    for &(r, w) in &inst.writes {
                        regs.insert(r);
                        tainted.insert(w);
                    }
                } else {
                    // Only VGPR writes keep their old value under a lane guard;
                    // SGPR and SCC writes (e.g. `v_readfirstlane_b32`, `v_cmp_*`)
                    // always overwrite the register unconditionally.
                    for &(r, _) in &inst.writes {
                        if guard.is_none() || !matches!(r, Reg::V(_)) {
                            regs.remove(&r);
                        }
                    }
                }
            }
            if regs.is_empty() {
                continue;
            }
            let into = |t: usize| p.blocks[t].preds.iter().position(|&q| q == b);
            let mut go = |t: usize, env, regs| match into(t) {
                Some(n) => {
                    work.push((t, n, env, regs));
                    true
                }
                None => false,
            };
            let ok = match st.exits[b] {
                LaneExit::End => true,
                LaneExit::Indirect => false,
                LaneExit::Jump(t) => go(t, env, regs),
                LaneExit::Branch {
                    when,
                    taken,
                    not_taken,
                } => {
                    let (no, yes) = self.can_be(when, &env, &mut HashMap::new());
                    !(no && yes && self.touches(when, &env, &tainted))
                        && (!yes || go(taken, env.clone(), regs.clone()))
                        && (!no || go(not_taken, env, regs))
                }
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// Whether deciding `b`, given the values in `env`, may test an atom
    /// of a `tainted` value.
    fn touches(&self, b: Bool, env: &HashMap<Value, u32>, tainted: &HashSet<Value>) -> bool {
        let mut stack = vec![b];
        let mut seen = HashSet::new();
        while let Some(b) = stack.pop() {
            let Some((atom, lo, hi)) = self.x.st.bools.node(b) else {
                continue;
            };
            if !seen.insert(b) {
                continue;
            }
            let v = match atom {
                Atom::Lane(v)
                | Atom::Uniform(v)
                | Atom::Phi(v)
                | Atom::Quad(v)
                | Atom::Opaque(v) => Some(v),
                Atom::Constant(_) => None,
            };
            if v.is_some_and(|v| tainted.contains(&v)) {
                return true;
            }
            match v.and_then(|v| self.compared(env, v)) {
                Some(true) => stack.push(hi),
                Some(false) => stack.push(lo),
                None => stack.extend([lo, hi]),
            }
        }
        false
    }

    /// Whether `b` can be false, and true, given the values in `env`.
    fn can_be(
        &self,
        b: Bool,
        env: &HashMap<Value, u32>,
        memo: &mut HashMap<Bool, (bool, bool)>,
    ) -> (bool, bool) {
        if b == Bool::TRUE || b == Bool::FALSE {
            return (b == Bool::FALSE, b == Bool::TRUE);
        }
        if let Some(&r) = memo.get(&b) {
            return r;
        }
        let (atom, lo, hi) = self.x.st.bools.node(b).expect("not a constant");
        let v = match atom {
            Atom::Lane(v) | Atom::Uniform(v) => Some(v),
            _ => None,
        };
        let r = match v.and_then(|v| self.compared(env, v)) {
            Some(true) => self.can_be(hi, env, memo),
            Some(false) => self.can_be(lo, env, memo),
            None => {
                let (a, c) = self.can_be(lo, env, memo);
                let (d, e) = self.can_be(hi, env, memo);
                (a || d, c || e)
            }
        };
        memo.insert(b, r);
        r
    }

    /// The block, instruction index, and instruction writing `v`.
    fn def_site(&self, v: Value) -> Option<(usize, usize, &'a Inst)> {
        let b = *self.def_block.get(&v)?;
        let (i, inst) = self.x.p.blocks[b]
            .insts
            .iter()
            .enumerate()
            .find(|(_, inst)| inst.writes.iter().any(|w| w.1 == v))?;
        Some((b, i, inst))
    }

    /// The instruction writing `v`.
    pub(super) fn def_inst(&self, v: Value) -> Option<&'a Inst> {
        self.def_site(v).map(|(_, _, inst)| inst)
    }

    /// The constant `v` holds, `env` giving phi values: unguarded moves of
    /// unmodified constants.
    fn konst(&self, env: &HashMap<Value, u32>, v: Value) -> Option<u32> {
        if let Some(&k) = env.get(&v) {
            return Some(k);
        }
        let (b, i, inst) = self.def_site(v)?;
        if self.x.st.guards[b][i].is_some() {
            return None;
        }
        let op = match &inst.fields {
            Fields::Valu(w)
                if inst.name() == "v_mov_b32"
                    && w.sdwa.is_none()
                    && w.dpp.is_none()
                    && w.abs == 0
                    && w.neg == 0
                    && !w.clamp
                    && w.omod == 0 =>
            {
                w.src.first()?
            }
            Fields::Sop { src, .. } if inst.name() == "s_mov_b32" => src.first()?,
            Fields::Sop { imm, .. } if inst.name() == "s_movk_i32" => {
                return Some(i32::from((*imm)? as i16) as u32);
            }
            _ => return None,
        };
        self.konst_operand(env, inst, op)
    }

    fn konst_operand(&self, env: &HashMap<Value, u32>, inst: &Inst, op: &Operand) -> Option<u32> {
        match *op {
            Operand::Const(c) => crate::resource::inline(c),
            Operand::Literal(x) => Some(x),
            Operand::Scalar { code, .. } if code != NULL => {
                self.konst(env, inst.read(Reg::S(code))?)
            }
            Operand::Vector { reg, .. } => self.konst(env, inst.read(Reg::V(reg))?),
            _ => None,
        }
    }

    /// The lane's bit of the 32-bit integer compare writing `v` (a VALU
    /// result before EXEC masking, or SCC), when its operands are constants.
    fn compared(&self, env: &HashMap<Value, u32>, v: Value) -> Option<bool> {
        let inst = self.def_inst(v)?;
        let name = inst.name();
        let rest = ["v_cmpx_", "v_cmp_", "s_cmp_"]
            .iter()
            .find_map(|p| name.strip_prefix(p))?;
        let (op, ty) = rest.rsplit_once('_')?;
        let (a, b) = match &inst.fields {
            Fields::Valu(w) if w.sdwa.is_none() && w.dpp.is_none() => {
                (w.src.first()?, w.src.get(1)?)
            }
            Fields::Sop { src, .. } => (src.first()?, src.get(1)?),
            _ => return None,
        };
        let (a, b) = (
            self.konst_operand(env, inst, a)?,
            self.konst_operand(env, inst, b)?,
        );
        let (sa, sb) = (a as i32, b as i32);
        Some(match (op, ty) {
            ("f", _) => false,
            ("t", _) => true,
            ("eq", "u32" | "i32") => a == b,
            ("ne" | "lg", "u32" | "i32") => a != b,
            ("lt", "u32") => a < b,
            ("le", "u32") => a <= b,
            ("gt", "u32") => a > b,
            ("ge", "u32") => a >= b,
            ("lt", "i32") => sa < sb,
            ("le", "i32") => sa <= sb,
            ("gt", "i32") => sa > sb,
            ("ge", "i32") => sa >= sb,
            _ => return None,
        })
    }

    /// The incoming edges (phi, predecessor index) over which a value can
    /// reach a read. A phi that only another read phi consumes is live too
    /// (else a value with no DXIL form reaching a read through two joins
    /// would be replaced by zero), but only over edges a lane can take on
    /// its way there: each step back adds the branch a lane took on that
    /// edge, and a contradiction (the lane that skipped an arm is not one
    /// that ran it) leaves the edge dead. A back edge forgets the branches
    /// (the loop redefines their conditions). Fewer conditions are weaker,
    /// so keeping only the last few stays sound.
    fn live_edges(&self) -> HashSet<(Value, usize)> {
        const MAX_KNOWN: usize = 4;
        /// What is known of the lanes along a path: the last few branch
        /// conditions taken, in order (a value, not a heap allocation:
        /// shaders with control flow push thousands of them).
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        struct Known {
            conditions: [(Bool, bool); MAX_KNOWN],
            len: usize,
        }
        impl Known {
            fn get(&self) -> &[(Bool, bool)] {
                &self.conditions[..self.len]
            }
            /// With `c` after the others, the oldest dropped past the limit.
            fn and(mut self, c: (Bool, bool)) -> Known {
                if self.len == MAX_KNOWN {
                    self.conditions.copy_within(1.., 0);
                    self.len -= 1;
                }
                self.conditions[self.len] = c;
                self.len += 1;
                self
            }
        }
        let none = Known {
            conditions: [(Bool::FALSE, false); MAX_KNOWN],
            len: 0,
        };
        let (p, st) = (self.x.p, &self.x.st);
        let mut work: Vec<(Value, Known)> = self.read_phis.iter().map(|&v| (v, none)).collect();
        let mut live = HashSet::new();
        let mut seen = HashMap::<Value, Vec<Known>>::new();
        // Whether some lane meets each set of conditions: paths into a join
        // ask again for the same sets.
        let mut meets = HashMap::<Known, bool>::new();
        while let Some((v, known)) = work.pop() {
            let Some(&(b, i)) = self.phi_at.get(&v) else {
                continue;
            };
            let done = seen.entry(v).or_default();
            // Known of fewer lanes than before: nothing new is live.
            if done
                .iter()
                .any(|d| d.get().iter().all(|c| known.get().contains(c)))
            {
                continue;
            }
            done.push(known);
            for (n, &q) in p.blocks[b].preds.iter().enumerate() {
                if self.tail[q].is_none() {
                    continue;
                }
                let mut k = none;
                // Forward edges out of a block that is no loop's latch (a
                // latch's exit is not narrowed by its condition, which
                // only keeps more edges live).
                if let LaneExit::Branch {
                    when,
                    taken,
                    not_taken,
                } = st.exits[q]
                    && taken != not_taken
                    && self.rpo[taken] > self.rpo[q]
                    && self.rpo[not_taken] > self.rpo[q]
                {
                    k = known.and((when, taken == b));
                    if !*meets.entry(k).or_insert_with(|| st.bools.meet(k.get())) {
                        continue;
                    }
                } else if self.rpo[q] < self.rpo[b] {
                    k = known;
                }
                live.insert((v, n));
                let arg = p.blocks[b].phis[i].args[n];
                if self.phi_at.contains_key(&arg) {
                    work.push((arg, k));
                }
            }
        }
        live
    }

    /// An unguarded indexed write replaces the incoming value before a
    /// consumer can observe it, either on this edge or in a counted loop.
    fn overwritten_movreld_edge(&self, b: usize, phi: &Phi, edge: usize) -> Option<()> {
        let Reg::V(reg) = phi.reg else { return None };
        let p = self.x.p;
        let block = &p.blocks[b];
        if self.sections.contains_key(&b)
            || self.sections.contains_key(&block.preds[edge])
            || p.blocks
                .iter()
                .flat_map(|bb| &bb.phis)
                .any(|ph| ph.args.contains(&phi.value))
        {
            return None;
        }
        let mut uses = p.blocks.iter().enumerate().flat_map(|(bb, block)| {
            block.insts.iter().enumerate().filter_map(move |(i, inst)| {
                inst.reads
                    .iter()
                    .any(|r| r.1 == phi.value)
                    .then_some((bb, i, inst))
            })
        });
        let (bb, i, mov) = uses.next()?;
        if uses.next().is_some()
            || bb != b
            || self.x.st.guards[b][i].is_some()
            || mov.name() != "v_movreld_b32"
            || mov.read(phi.reg) != Some(phi.value)
        {
            return None;
        }
        let Fields::Valu(v) = &mov.fields else {
            return None;
        };
        let (
            Some(Operand::Vector {
                reg: base,
                dwords: 1,
            }),
            [
                Operand::Vector {
                    reg: src,
                    dwords: 1,
                },
            ],
        ) = (v.dst, v.src.as_slice())
        else {
            return None;
        };
        if v.dpp.is_some()
            || v.sdwa.is_some()
            || mov.read(Reg::V(*src))? == phi.value
            || mov.write(phi.reg).is_none()
        {
            return None;
        }
        let index = u32::from(reg.checked_sub(base)?);
        // Follow M0 to a copy of a scalar phi in this block, then the
        // constant written on the corresponding predecessor.
        let (copy, [src]) = scalar_def(block, mov.read(Reg::S(crate::ir::M0))?, "s_mov_b32")?;
        let scalar = sgpr_operand(copy, src)?;
        let index_phi = block.phis.iter().find(|ph| ph.value == scalar)?;
        let (_, [initial]) = scalar_def(
            &p.blocks[block.preds[edge]],
            index_phi.args[edge],
            "s_mov_b32",
        )?;
        let known = scalar_constant(initial)?;
        if index == known {
            return Some(());
        }
        if index < known {
            return None; // Reaching this slot would require the index to wrap.
        }
        self.counted_movreld_index(b, edge, phi, index_phi, mov, index)
    }

    /// An unguarded move fills one more register on every iteration.
    /// Values from before the loop can only survive in slots the index skips.
    fn counted_movreld_index(
        &self,
        b: usize,
        edge: usize,
        phi: &Phi,
        indices: &Phi,
        mov: &Inst,
        slot: u32,
    ) -> Option<()> {
        let p = self.x.p;
        let st = self.x.st;
        let block = &p.blocks[b];
        let LaneExit::Branch {
            when,
            taken: latch,
            not_taken: exit,
        } = st.exits[b]
        else {
            return None;
        };
        let back = block.preds.iter().position(|&q| q == latch)?;
        if block.preds.len() != 2
            || back == edge
            || b == latch
            || b == exit
            || exit == latch
            || self.sections.contains_key(&latch)
            || st.exits[latch] != LaneExit::Jump(b)
            || p.blocks[latch].preds != [b]
            || p.blocks[exit].preds != [b]
            || st.exits[exit] != LaneExit::End
            || self.rpo[b] >= self.rpo[latch]
            || self.rpo[block.preds[edge]] >= self.rpo[b]
        {
            return None;
        }
        // The indexed result may only go around the loop as its own old
        // value or be consumed in the normal exit block.
        let result = mov.write(phi.reg)?;
        if phi.args[back] != result {
            return None;
        }
        for (bb, block) in p.blocks.iter().enumerate() {
            if bb != exit
                && block
                    .insts
                    .iter()
                    .any(|inst| inst.reads.iter().any(|r| r.1 == result))
            {
                return None;
            }
            if block.phis.iter().any(|other| {
                other
                    .args
                    .iter()
                    .enumerate()
                    .any(|(n, &arg)| arg == result && (other.value != phi.value || n != back))
            }) {
                return None;
            }
        }
        // Follow the latch copy to the unit increment, then require that
        // this very increment is compared for equality to the exit bound.
        let (copy, [src]) = scalar_def(&p.blocks[latch], indices.args[back], "s_mov_b32")?;
        let next = sgpr_operand(copy, src)?;
        let (increment, [from, step]) = scalar_def(block, next, "s_add_i32")?;
        if sgpr_operand(increment, from)? != indices.value || scalar_constant(step)? != 1 {
            return None;
        }
        let (Atom::Uniform(scc), Bool::TRUE, Bool::FALSE) = st.bools.node(when)? else {
            return None;
        };
        let branch = block.insts.last()?;
        if branch.name() != "s_cbranch_scc0" || branch.read(Reg::Scc) != Some(scc) {
            return None;
        }
        let (cmp, [tested, bound]) = scalar_def(block, scc, "s_cmp_eq_u32")?;
        if sgpr_operand(cmp, tested)? != next {
            return None;
        }
        let limit = scalar_constant(bound)?;
        // The initial index is below this slot. The loop must pass it before exit.
        (slot < limit).then_some(())
    }

    pub(super) fn fill_phis(&mut self) -> Result<(), Stop> {
        let p = self.x.p;
        let live = self.live_edges();
        for (v, b, i) in std::mem::take(&mut self.data_phis) {
            let phi = &p.blocks[b].phis[i];
            let bit = phi.reg == Reg::Scc;
            let mut incoming = Vec::new();
            for (n, &q) in p.blocks[b].preds.iter().enumerate() {
                let Some(qb) = self.tail[q] else { continue };
                let qb = self.edges.get(&(q, b)).copied().unwrap_or(qb);
                let arg = phi.args[n];
                let read = live.contains(&(phi.value, n));
                let x = match (self.vals.get(&arg).copied(), bit) {
                    (Some(Val::I32(x)), false) | (Some(Val::Bit(x)), true) => x,
                    // No lane from this edge reads it, or no lane comes
                    // along it at all: any value will do.
                    _ if read
                        && (self.never_taken(q, b) || self.dead_on_edge(b, n, phi.reg)) =>
                    {
                        if bit {
                            self.s.m.bool(false)
                        } else {
                            self.s.m.i32(0)
                        }
                    }
                    // An input an unguarded counted `v_movreld` loop overwrites
                    // before anything reads it (PR #457).
                    (Some(Val::Missing("barycentrics read directly")), false)
                        if read && self.overwritten_movreld_edge(b, phi, n).is_some() =>
                    {
                        self.s.m.undef(self.s.t.i32)
                    }
                    (Some(Val::Bit(_)), false) if read => {
                        return Err(Stop::Retry(phi.value, "an SCC joined with SGPR values"));
                    }
                    // A lane mask joined with data at a join every lane
                    // enters along one edge: the register holds the mask's
                    // number there (console lanes, so exact).
                    _ if read
                        && !bit
                        && self.mask_data == MaskData::Console
                        && (self.x.st.masks.contains_key(&arg) || self.x.st.highs.contains_key(&arg))
                        && self.uniform_join(b) =>
                    {
                        self.mask_dword(arg).map_err(|w| Stop::Retry(phi.value, w))?
                    }
                    (Some(Val::Missing(why)), _) if read => {
                        return Err(Stop::Retry(phi.value, why));
                    }
                    _ if read => {
                        return Err(Stop::Retry(phi.value, "a join of mismatched values"));
                    }
                    _ if bit => self.s.m.bool(false),
                    _ => self.s.m.i32(0),
                };
                incoming.push((x, qb));
            }
            self.s.m.set_phi(v, incoming);
        }
        for (phi, incoming) in std::mem::take(&mut self.counters).into_values() {
            self.s.m.set_phi(phi, incoming);
        }
        while let Some((v, b, i)) = self.pending_masks.pop() {
            let phi = &p.blocks[b].phis[i];
            let offset = p.blocks[b].offset;
            let mut incoming = Vec::new();
            for (n, &q) in p.blocks[b].preds.iter().enumerate() {
                let Some(qb) = self.tail[q] else { continue };
                let qb = self.edges.get(&(q, b)).copied().unwrap_or(qb);
                let arg = phi.args[n];
                const UNTRACKED: Why = "a lane-mask join of an untracked mask";
                // Data joined with masks where the wave takes one edge (the BVH
                // walks pop masks from memory): with console lanes the lane's
                // own bit of the number, as `Atom::Opaque`.
                let number = self.console_lanes && !self.x.native.wave64 && self.uniform_join(b);
                let x = match (self.x.st.masks.get(&arg), self.vals.get(&arg)) {
                    (Some(&m), _) => self.boolean(m, qb).map_err(|w| unsupported(offset, w))?,
                    (None, Some(&Val::I32(word))) if number => self.lane_bit(qb, word, None),
                    (None, _) if !self.uniform_join(b) && self.no_mask_lane_on_edge(b, &phi.args, n) => {
                        self.s.m.bool(false)
                    }
                    (None, _) => return Err(unsupported(offset, UNTRACKED)),
                };
                incoming.push((x, qb));
            }
            self.s.m.set_phi(v, incoming);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{S_ENDPGM, lowers};

    #[test]
    fn a_missing_value_stays_missing_through_two_live_joins() {
        let mut words = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0xBF06_8000, // s_cmp_eq_u32 s0, 0 (s0 = group id X)
            0xBF85_0001, // skip the first overwrite
            0xBE86_0380, // s_mov_b32 s6, 0
            0xBF06_8101, // s_cmp_eq_u32 s1, 1 (s1 = group id Y)
            0xBF85_0001, // skip the second overwrite
            0xBE86_0381, // s_mov_b32 s6, 1
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001, // use the result to branch
            0xBE83_0380,
            S_ENDPGM,
        ];
        let group_ids_xy = 3 << 7;
        // Compare to a direct read so future timestamp support works in both cases.
        let refusal = |code: &[u32]| match lowers(code, group_ids_xy) {
            Ok(()) => None,
            Err(crate::ShaderError::Unsupported { what, .. }) => Some(what),
            Err(error) => panic!("{error}"),
        };
        let direct = [&words[..2], &words[8..]].concat();
        assert_eq!(refusal(&words), refusal(&direct));
        words[8] = 0xBF07_8000; // read the group id instead; the timestamp is dead
        lowers(&words, group_ids_xy).expect("dead timestamp");
    }

    #[test]
    fn a_value_no_lane_from_its_edge_reads_needs_no_form() {
        // s6 is a timestamp (no DXIL form) where the load is skipped and 5
        // where it runs; a flag in v1, set only with the load, guards the
        // read after the join (the console compiler's form, where the
        // skipped path leaves a stale lane mask in the register).
        let mut words = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0x7E02_0280, // v_mov_b32 v1, 0
            0xBF06_8000, // s_cmp_eq_u32 s0, 0 (s0 = group id X)
            0xBF85_0002, // s_cbranch_scc1 -> join
            0xBE86_0385, // s_mov_b32 s6, 5
            0x7E02_02C1, // v_mov_b32 v1, -1
            0x7D84_02C1, // join: v_cmp_eq_u32 vcc_lo, -1, v1
            0xBF86_0003, // s_cbranch_vccz -> end
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001, // s_cbranch_scc1 -> end
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        let group_ids_xy = 3 << 7;
        lowers(&words, group_ids_xy).expect("the timestamp is never read");
        // With the flag set on both paths the timestamp is read: as refused
        // (or accepted) as a direct read.
        words[2] = 0x7E02_02C1; // v_mov_b32 v1, -1
        let refusal = |code: &[u32]| match lowers(code, group_ids_xy) {
            Ok(()) => None,
            Err(crate::ShaderError::Unsupported { what, .. }) => Some(what),
            Err(error) => panic!("{error}"),
        };
        let direct = [&words[..2], &words[9..]].concat();
        assert_eq!(refusal(&words), refusal(&direct));
    }

    #[test]
    fn a_mask_joined_with_data_needs_a_uniform_join() {
        // s6: lanes below 24, then shifted (data) on each back edge; read
        // after the loop. Lanes leaving the loop at different iterations
        // (per lane) see different numbers than the console's register:
        // refused. The uniform loop is `synthetic_tests::
        // a_mask_joined_with_data_at_a_uniform_loop_is_its_number`.
        let mut words = [
            0x7D88_0098, // v_cmp_gt_u32 vcc_lo, 24, v0
            0x8706_7E6A, // s_and_b32 s6, vcc_lo, exec_lo
            0xBE87_0380, // s_mov_b32 s7, 0
            0x9006_8406, // header: s_lshr_b32 s6, s6, 4
            0x8007_8107, // s_add_u32 s7, s7, 1
            0x7D88_0007, // v_cmp_gt_u32 vcc_lo, s7, v0
            0x8A7E_6A7E, // s_andn2_b32 exec_lo, exec_lo, vcc_lo
            0xBF89_FFFB, // s_cbranch_execnz header
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001, // s_cbranch_scc1 +1
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        let Err(crate::ShaderError::Unsupported { what, .. }) = lowers(&words, 1 << 7) else {
            panic!("a per-lane loop's mask join translated");
        };
        assert!(what.contains("lane mask"), "{what}");
        // Leaving on a uniform condition instead: the mask's number.
        words[5] = 0xBF0A_8207; // s_cmp_lt_u32 s7, 2
        words[6] = 0xBF80_0000; // s_nop 0
        words[7] = 0xBF85_FFFB; // s_cbranch_scc1 header
        lowers(&words, 1 << 7).expect("a uniform loop's mask join");
    }

    #[test]
    fn a_scalar_result_needs_no_exec() {
        // s6 is 0 where every lane skips and the saved EXEC where some run:
        // no lane mask at the per-lane join. EXEC taken from it guards only
        // a `v_readlane` (written whatever EXEC holds) before it is restored
        // (`CS_IrradianceCache_Shade`).
        let words = [
            0xBE86_0380, // s_mov_b32 s6, 0
            0x7D88_0090, // v_cmp_gt_u32 vcc_lo, 16, v0
            0xBE84_3C6A, // s_and_saveexec_b32 s4, vcc_lo
            0xBF88_0001, // s_cbranch_execz +1
            0xBE86_0304, // s_mov_b32 s6, s4
            0xBEFE_0306, // s_mov_b32 exec_lo, s6
            0xD760_0002, // v_readlane_b32 s2, v0, 3
            0x0001_0700,
            0xBEFE_0304, // s_mov_b32 exec_lo, s4
            S_ENDPGM,
        ];
        lowers(&words, 1 << 7).expect("a lane read under an EXEC with no mask");
    }

    #[test]
    fn guarded_or_modified_v_mov_b32_is_not_folded_as_constant() {
        let group_ids_xy = 3 << 7;
        let refusal = |code: &[u32]| match lowers(code, group_ids_xy) {
            Ok(()) => None,
            Err(crate::ShaderError::Unsupported { what, .. }) => Some(what),
            Err(error) => panic!("{error}"),
        };
        let direct = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001, // s_cbranch_scc1 -> end
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        assert!(refusal(&direct).is_some());

        // Guarded `v_mov_b32 v1, 0`: inactive lanes (v0 != 0) preserve v1 = -1,
        // so they pass the compare after the join and read s6 (`s_memtime`).
        let mut guarded = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0x7E02_02C1, // v_mov_b32 v1, -1
            0x7DA4_0080, // v_cmpx_eq_u32 exec, 0, v0
            0x7E02_0280, // v_mov_b32 v1, 0 (guarded)
            0xBEFE_03C1, // s_mov_b32 exec_lo, -1
            0xBF06_8000, // s_cmp_eq_u32 s0, 0
            0xBF85_0002, // s_cbranch_scc1 -> join
            0xBE86_0385, // s_mov_b32 s6, 5
            0x7E02_02C1, // v_mov_b32 v1, -1
            0x7D84_02C1, // join: v_cmp_eq_u32 vcc_lo, -1, v1
            0xBF86_0003, // s_cbranch_vccz -> end
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001, // s_cbranch_scc1 -> end
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        assert_eq!(refusal(&guarded), refusal(&direct));
        // Without the EXEC guard, `v_mov_b32 v1, 0` writes every lane and s6 is dead.
        guarded[3] = 0xBF80_0000; // s_nop 0
        guarded[5] = 0xBF80_0000; // s_nop 0
        lowers(&guarded, group_ids_xy).expect("unguarded v_mov_b32 still folds");

        // VOP3 `v_mov_b32_e64 v1, -(1.0)`: neg modifier flips 1.0f to -1.0f,
        // matching the compare after the join so s6 is live.
        let mut negated = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0xD581_0001,
            0x2000_00F2, // v_mov_b32_e64 v1, -(1.0)
            0xBF06_8000, // s_cmp_eq_u32 s0, 0
            0xBF85_0002, // s_cbranch_scc1 -> join
            0xBE86_0385, // s_mov_b32 s6, 5
            0x7E02_02F3, // v_mov_b32 v1, -1.0
            0x7D84_02F3, // join: v_cmp_eq_u32 vcc_lo, -1.0, v1
            0xBF86_0003, // s_cbranch_vccz -> end
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001, // s_cbranch_scc1 -> end
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        assert_eq!(refusal(&negated), refusal(&direct));
        // Clearing the neg bit yields unmodified 1.0f (!= -1.0f), so s6 is dead.
        negated[3] = 0x0000_00F2; // v_mov_b32_e64 v1, 1.0
        lowers(&negated, group_ids_xy).expect("unmodified v_mov_b32_e64 still folds");
    }

    #[test]
    fn a_missing_value_on_a_path_no_lane_takes_is_dead() {
        let refusal = |code: &[u32]| match lowers(code, 1 << 7) {
            Ok(()) => None,
            Err(crate::ShaderError::Unsupported { what, .. }) => Some(what),
            Err(error) => panic!("{error}"),
        };
        // The timestamp stays in s6 only where s0 == 0, and those lanes
        // overwrite it before the second join: it never reaches the read.
        let mut joins = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0xBF06_8000, // s_cmp_eq_u32 s0, 0 (s0 = group id X)
            0xBF85_0001, // s_cbranch_scc1: s0 == 0 keeps the timestamp
            0xBE86_0380, // s_mov_b32 s6, 0
            0xBF84_0001, // s_cbranch_scc0: s0 != 0 keeps that 0
            0xBE86_0381, // s_mov_b32 s6, 1
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001,
            0xBE83_0380,
            S_ENDPGM,
        ];
        assert_eq!(refusal(&joins), None);
        // Tested on the other side, the timestamp reaches the read.
        joins[5] = 0xBF85_0001; // s_cbranch_scc1
        assert!(refusal(&joins).is_some());
        // The same through a VGPR whose old value a guarded write keeps
        // (lanes outside it get 0 for a value with no DXIL form, as for any
        // such write): keeping does not make the join live.
        let kept = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0x7E02_0206, // v_mov_b32 v1, s6
            0xBF06_8000, // s_cmp_eq_u32 s0, 0
            0xBF85_0001, // s_cbranch_scc1: s0 == 0 keeps the timestamp
            0x7E02_0280, // v_mov_b32 v1, 0
            0xBF84_0004, // s_cbranch_scc0: s0 != 0 keeps that 0
            0x7DA4_0080, // v_cmpx_eq_u32 exec, 0, v0
            0x7E02_0282, // v_mov_b32 v1, 2 (the other lanes keep v1)
            0xBEFE_03C1, // s_mov_b32 exec_lo, -1
            0x7E02_0283, // v_mov_b32 v1, 3
            0x7D84_0280, // v_cmp_eq_u32 vcc, 0, v1
            0xBF87_0001, // s_cbranch_vccnz
            0xBE83_0380,
            S_ENDPGM,
        ];
        assert_eq!(refusal(&kept), None);
    }

    #[test]
    fn a_guarded_scalar_write_kills_a_stale_sgpr() {
        // s6 holds a missing value (timestamp) where s0 == 0 and 5 where
        // s0 != 0; the flag in v1 skips the first read on the s0 == 0 path.
        // After that, inside a guarded region (EXEC narrowed without a
        // branch), `v_readfirstlane_b32` overwrites s6 unconditionally before
        // s6 is read again.
        let mut words = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0x7E02_0280, // v_mov_b32 v1, 0
            0xBF06_8000, // s_cmp_eq_u32 s0, 0 (s0 = group id X)
            0xBF85_0002, // s_cbranch_scc1 -> join
            0xBE86_0385, // s_mov_b32 s6, 5
            0x7E02_02C1, // v_mov_b32 v1, -1
            0x7D84_02C1, // join: v_cmp_eq_u32 vcc_lo, -1, v1
            0xBF86_0003, // s_cbranch_vccz -> after
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001, // s_cbranch_scc1 -> after
            0xBE84_0380, // s_mov_b32 s4, 0
            0x7DA4_0080, // after: v_cmpx_eq_u32 exec_lo, 0, v0 (narrows EXEC)
            0x7E0C_0500, // v_readfirstlane_b32 s6, v0 (guarded, writes SGPR)
            0x7DA4_0006, // v_cmpx_eq_u32 exec_lo, s6, v0 (reads new s6)
            0xBEFE_03C1, // s_mov_b32 exec_lo, -1
            S_ENDPGM,
        ];
        let group_ids_xy = 3 << 7;
        lowers(&words, group_ids_xy).expect("guarded v_readfirstlane overwrites stale s6");
        // Without the overwrite, the guarded compare reads the stale s6.
        let mut no_kill = words;
        no_kill[13] = 0xBF80_0000; // s_nop 0
        assert!(lowers(&no_kill, group_ids_xy).is_err());
        // With the flag set on both paths, the earlier read sees the stale s6.
        words[2] = 0x7E02_02C1; // v_mov_b32 v1, -1
        assert!(lowers(&words, group_ids_xy).is_err());
    }
}
