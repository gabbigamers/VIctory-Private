//! The recogniser: from a loop of a program to its workload description,
//! or to the reason it has none.
//!
//! **What it accepts** is one shape, written against the synthetic programs
//! of [`super::walk`] and nothing else; no title loop has been seen:
//!
//! * a loop with one way in, one block that goes round again and one way
//!   out, no loop inside it, and one `image_bvh_intersect_ray` with a
//!   32-bit pointer and a register for each operand;
//! * the structure's descriptor built before the loop around a root
//!   descriptor's address; origin and direction set before the loop, the
//!   inverse `v_rcp_f32` of each direction component;
//! * the pointer popped from a stack in group-shared memory: thread `n` of
//!   a one-dimensional group has the bytes from `n * stride`, the root is a
//!   constant every lane pushes before the loop, a round pops one pointer,
//!   and the loop goes round while the stack holds one and for no other
//!   reason;
//! * a pointer whose type (`pointer & 7`) is below 4 is a candidate, with
//!   `t`, `u`, `v` the node test's numerators times `v_rcp_f32` of its
//!   denominator; for any other pointer every result word that is not
//!   invalid is pushed, each once, and nothing else is ever stored;
//! * a candidate is accepted when `minimum < t` and `t < extent`, each in
//!   one of the forms of [`Compare`], or also at an equal `t` with the
//!   smaller pointer; on acceptance every value the loop keeps is replaced
//!   at once, and each is the distance, a weight, a constant, one of two
//!   constants by the denominator's sign, or the pointer;
//! * nothing else leaves the loop: no other value, no lane mask, no memory.
//!
//! A candidate is not a hit: the node test answers a triangle the ray
//! misses with a distance of +inf, which `t <= extent` passes at an
//! infinite extent and a compare written as a negation at a NaN one. Which
//! of the two the accept can pass is part of the description
//! ([`MissWord`]), never assumed away.
//!
//! The lane's compares are taken as independent yes-or-no cases and the
//! loop's values are followed through every combination of them
//! ([`Cases`]), so "accepted when" and "pushed when" are checked as truth
//! tables, not matched as instruction sequences. What the checks do not
//! establish is refused; the order of the checks decides which reason a
//! loop with several is given.

use std::collections::{HashMap, HashSet};

use super::describe::{
    Arg, Class, Compare, Description, Extent, Filter, Flags, InstanceCode, Inverse, Kept, Kind,
    Mask, MissWord, Order, RaySource, Refusal, Root, Stack,
};
use super::region::{Memory, addresses, data_reads, touches};
use super::terms::{T, Term, Terms};
use crate::gfx10::{Fields, Operand};
use crate::io::{Input, Io};
use crate::ir::{self, Inst, Loop, Program, Reg, Value};
use crate::native::{NativeProgram, RootType, StageIo};
use crate::resource::{self, Resources, Site, Source};
use crate::structure::{Atom, Bool, LaneExit, Structured};

/// A program with what the stages of its translation made of it; `None`
/// for a stage that refused it.
#[derive(Clone, Copy)]
pub struct Parts<'a> {
    pub native: &'a NativeProgram,
    pub p: &'a Program,
    pub st: Option<&'a Structured>,
    pub res: Option<&'a Resources>,
    pub io: Option<&'a Io>,
    /// Whether the program was lowered per lane (`emit::emit_with` made its
    /// container without an emulated wave). The per-lane structure of a
    /// program the lowering refuses is not known to be what the program
    /// means (a lane mask it reads as a number is refused only there), so
    /// nothing is recognised in one; and such a program has no software
    /// path to fall back to or to be compared with.
    pub lowered: bool,
}

/// What became of one loop that holds a node instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub header: usize,
    /// The loop's place among those of its header ([`ir::Loop::place`]).
    pub place: usize,
    pub result: Result<Description, Refusal>,
}

/// What the recogniser found in one program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Node instructions in the program.
    pub node_instructions: usize,
    /// Of those, the ones in no loop.
    pub outside_loops: usize,
    /// Every loop that holds a node instruction, a loop around another
    /// included, in [`ir::natural_loops`]' order.
    pub loops: Vec<Finding>,
    /// Instructions looked at, each as often as a pass looked at it: at
    /// most a fixed multiple of the program for each loop examined. The
    /// smaller part of the work where a loop gets as far as its tables.
    pub visited: usize,
    /// The cases the loops' tables were made over: for each loop that got
    /// that far, 2 to the number of its free compares (at most
    /// [`MAX_ATOMS`] of them).
    pub cases: usize,
    /// Conditions evaluated in those cases (calls of `Terms::holds`): what
    /// the tables cost. The cases double with each free compare of a loop,
    /// and a case costs more the more picks and pushes the loop has: the
    /// limit on free compares bounds this, the instruction count does not.
    pub evaluated: usize,
}

/// The recogniser's work on one loop, counted ([`Report`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Work {
    pub visited: usize,
    pub cases: usize,
}

/// Runs the recogniser over every loop of a program that holds a node
/// instruction. A program without one costs one scan of its instructions.
pub fn recognise(x: &Parts<'_>) -> Report {
    let p = x.p;
    let node = |b: usize| {
        p.blocks[b]
            .insts
            .iter()
            .filter(|i| i.name().starts_with("image_bvh"))
            .count()
    };
    let mut report = Report {
        node_instructions: (0..p.blocks.len()).map(node).sum(),
        visited: p.blocks.iter().map(|b| b.insts.len()).sum(),
        ..Report::default()
    };
    if report.node_instructions == 0 {
        return report;
    }
    let loops = ir::natural_loops(p);
    let looped: HashSet<usize> = loops
        .iter()
        .flat_map(|l| l.blocks.iter().copied())
        .collect();
    report.outside_loops = (0..p.blocks.len())
        .filter(|b| !looped.contains(b))
        .map(node)
        .sum();
    let mut terms = x.st.map(|st| Terms::new(p, st));
    let mut work = Work::default();
    for l in loops
        .iter()
        .filter(|l| l.blocks.iter().any(|&b| node(b) != 0))
    {
        let result = recognise_loop(x, terms.as_mut(), &loops, l, &mut work);
        report.loops.push(Finding {
            header: l.header,
            place: l.place,
            result,
        });
    }
    report.visited += work.visited + terms.as_ref().map_or(0, |t| t.visited);
    report.cases = work.cases;
    report.evaluated = terms.map_or(0, |t| t.evaluated.get());
    report
}

/// What one of a lane's compares is to the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// The pointer is a triangle's.
    Triangle,
    /// `minimum < t`; whether the minimum is the compare's second operand.
    Minimum(Compare, bool),
    /// `t < extent`.
    Extent(Compare),
    /// `t == extent`.
    Equal,
    /// The pointer is below the one this carried value holds.
    Key(Value),
    /// Result word `k` of the node test is a valid pointer.
    Valid(u8),
    /// The denominator is positive.
    Front,
    /// The stack holds a pointer.
    NotEmpty,
    /// A constant against a count of rounds.
    Cap,
    Other,
}

/// The lane's compares as independent cases: every atom the loop's
/// conditions depend on that the loop's entry does not decide is free, and
/// a case gives each a value (bit `i` of the case for atom `i`).
struct Cases {
    free: Vec<Atom>,
    index: HashMap<Atom, usize>,
    fixed: HashMap<Atom, bool>,
    roles: Vec<Role>,
}

impl Cases {
    fn count(&self) -> u32 {
        1 << self.free.len()
    }

    fn truth(&self, case: u32) -> impl Fn(Atom) -> bool + '_ {
        move |a| match self.index.get(&a) {
            Some(&i) => case >> i & 1 != 0,
            None => self.fixed.get(&a).copied().unwrap_or(false),
        }
    }

    /// `f` in every case.
    fn table(&self, f: impl Fn(u32) -> bool) -> Vec<bool> {
        (0..self.count()).map(f).collect()
    }

    /// The atoms a table depends on.
    fn depends(&self, table: &[bool]) -> Vec<usize> {
        let on =
            |i: usize| (0..self.count()).any(|c| table[c as usize] != table[(c ^ 1 << i) as usize]);
        (0..self.free.len()).filter(|&i| on(i)).collect()
    }

    /// The one atom of a role among `atoms`, if there is exactly one.
    fn one(&self, atoms: &[usize], role: impl Fn(Role) -> bool) -> Result<Option<usize>, ()> {
        let mut found = atoms.iter().copied().filter(|&i| role(self.roles[i]));
        let first = found.next();
        if found.next().is_some() {
            Err(())
        } else {
            Ok(first)
        }
    }
}

/// A value the loop carries from one round to the next.
#[derive(Clone, Copy)]
struct Carried {
    /// What it is on the way in, and after a round: the value and its term.
    init: Value,
    start: T,
    last: Value,
    next: T,
}

/// The most free atoms a loop may have (2^14 cases); a loop with more is
/// refused as such ([`Refusal::TooManyCompares`]).
pub const MAX_ATOMS: usize = 14;

/// The description of loop `l` of the program, or why it has none. `loops`
/// are all the program's loops; `terms` the program's terms if it has a
/// per-lane form; `work` counts the instructions looked at and the cases.
pub(crate) fn recognise_loop(
    x: &Parts<'_>,
    terms: Option<&mut Terms<'_>>,
    loops: &[Loop],
    l: &Loop,
    work: &mut Work,
) -> Result<Description, Refusal> {
    use Refusal::*;
    let visited = &mut work.visited;
    let p = x.p;
    let inst_at = |s: Site| &p.blocks[s.0].insts[s.1];

    // The node instruction and its operands.
    let mut nodes = Vec::new();
    for &b in &l.blocks {
        *visited += p.blocks[b].insts.len();
        let insts = p.blocks[b].insts.iter().enumerate();
        nodes.extend(
            insts
                .filter(|(_, i)| i.name().starts_with("image_bvh"))
                .map(|(i, _)| (b, i)),
        );
    }
    let node = match nodes[..] {
        [] => return Err(NoNodeInstruction),
        [n] => n,
        _ => return Err(SeveralNodeTests),
    };
    let test = inst_at(node);
    let Fields::Image {
        addr, a16, rsrc, ..
    } = &test.fields
    else {
        return Err(NodeForm);
    };
    let mut operands = Vec::new();
    for a in addr {
        if let Operand::Vector { reg, dwords } = *a {
            operands.extend((0..dwords).filter_map(|k| test.read(Reg::V(reg + k))));
        }
    }
    if test.name() != "image_bvh_intersect_ray" || *a16 || operands.len() != 11 {
        return Err(NodeForm);
    }
    let (pointer, extent) = (operands[0], operands[1]);
    let origin = [operands[2], operands[3], operands[4]];
    let direction = [operands[5], operands[6], operands[7]];
    let inverse = [operands[8], operands[9], operands[10]];

    // One pointer for the wave: a move from a scalar register or a
    // constant. Read from the IR alone, since such a program need not have
    // a per-lane form.
    *visited += p.blocks.iter().map(|b| b.insts.len()).sum::<usize>();
    let written = |i: &&Inst| i.writes.iter().any(|w| w.1 == pointer);
    let moved = p.blocks.iter().flat_map(|b| &b.insts).find(written);
    if let Some(Inst {
        fields: Fields::Valu(v),
        ..
    }) = moved.filter(|i| i.name() == "v_mov_b32")
        && !matches!(v.src.first(), Some(Operand::Vector { .. }))
    {
        return Err(WholeWave);
    }
    let (Some(t), Some(st), Some(res), Some(io), true) = (terms, x.st, x.res, x.io, x.lowered)
    else {
        return Err(NoPerLaneForm);
    };
    if loops
        .iter()
        .any(|m| m != l && l.holds(m.header) && (m.header != l.header || m.place < l.place))
    {
        return Err(NestedLoop);
    }
    let region = ir::region(p, l);
    let within = |t: &Terms, v: Value| {
        t.block
            .get(v.0 as usize)
            .is_some_and(|&b| b != usize::MAX && l.holds(b))
    };

    // The structure: a descriptor built around a root descriptor's address,
    // before the loop.
    let built = res
        .descriptors
        .iter()
        .find(|d| d.site == node)
        .map(|d| &d.source);
    let root_parameter = match built {
        Some(Source::Built { address, .. })
            if address.offset == 0
                && address.dynamic.is_none()
                && x.native
                    .layout
                    .get(address.param)
                    .is_some_and(|r| r.root == RootType::Descriptor) =>
        {
            address.param
        }
        _ => return Err(StructureSource),
    };
    if let Operand::Scalar { code, dwords } = *rsrc
        && (0..dwords).any(|k| test.read(Reg::S(code + k)).is_none_or(|v| within(t, v)))
    {
        return Err(StructureSource);
    }

    // The ray.
    if origin.iter().chain(&direction).any(|&v| within(t, v)) {
        return Err(RayChanges);
    }
    for k in 0..3 {
        let d = t.term(direction[k]);
        if t.find(&Term::Op("v_rcp_f32", 0, vec![d])) != Some(t.term(inverse[k])) {
            return Err(InverseNotProven);
        }
    }

    // Memory: the node test, one pop, pushes, and nothing else.
    let (mut pushes, mut pop) = (Vec::new(), None);
    let size = |blocks: &[usize]| {
        blocks
            .iter()
            .map(|&b| p.blocks[b].insts.len())
            .sum::<usize>()
    };
    *visited += size(&l.blocks);
    for a in touches(p, res, &l.blocks) {
        match (a.memory, a.store, inst_at(a.site).name()) {
            (Memory::Node { .. }, ..) => {}
            (Memory::Shared, true, "ds_write_b32") => pushes.push(a.site),
            (Memory::Shared, false, "ds_read_b32") if pop.is_none() => pop = Some(a.site),
            (Memory::Shared, ..) => return Err(StackShape),
            (_, true, _) => return Err(Stores),
            (_, false, _) => return Err(Loads),
        }
    }

    // One way in, one way round, one way out.
    let header = &p.blocks[l.header];
    let (&[entry], &[latch], 2) = (&region.entries[..], &l.latches[..], header.preds.len()) else {
        return Err(ExitForm);
    };
    let LaneExit::Branch {
        when,
        taken,
        not_taken,
    } = st.exits[latch]
    else {
        return Err(ExitForm);
    };
    let stays = taken == l.header;
    let out = if stays { not_taken } else { taken };
    if region.exits != [(latch, out)] || (!stays && not_taken != l.header) {
        return Err(ExitForm);
    }
    let back = header.preds.iter().position(|&q| q == latch).unwrap_or(0);
    let mut carried = HashMap::new();
    for phi in &header.phis {
        let (init, last) = (phi.args[1 - back], phi.args[back]);
        let (start, next) = (t.term(init), t.term(last));
        let held = Carried {
            init,
            start,
            last,
            next,
        };
        carried.insert(phi.value, held);
    }

    // The pop: the pointer is what the round's one shared load reads, at
    // the thread's base plus the stack pointer less 4.
    let ds_address = |t: &mut Terms, s: Site| match &inst_at(s).fields {
        Fields::Ds {
            addr: Some(Operand::Vector { reg, .. }),
            offset0: 0,
            offset1: 0,
            ..
        } => inst_at(s).read(Reg::V(*reg)).map(|v| t.term(v)),
        _ => None,
    };
    let ds_data = |t: &mut Terms, s: Site| match &inst_at(s).fields {
        Fields::Ds { data, .. } => match data[..] {
            [Operand::Vector { reg, .. }] => inst_at(s).read(Reg::V(reg)).map(|v| t.term(v)),
            _ => None,
        },
        _ => None,
    };
    let ptr = t.term(pointer);
    let pop = pop
        .filter(|&s| *t.get(ptr) == Term::Load(s, 0))
        .ok_or(StackShape)?;
    let Some(Term::Op("v_add_nc_u32", 0, sum)) = ds_address(t, pop).map(|a| t.get(a).clone())
    else {
        return Err(StackShape);
    };
    let constant = |t: &mut Terms, k: u32| t.make(Term::Const(k));
    let (four, zero, less4) = (
        constant(t, 4),
        constant(t, 0),
        constant(t, 4u32.wrapping_neg()),
    );
    // The stack pointer a term is 4 below, if it is that.
    let below = |t: &Terms, x: T| match t.get(x) {
        Term::Op("v_add_nc_u32", 0, a) if a.contains(&less4) => {
            a.iter().find_map(|&y| match t.get(y) {
                Term::Phi(v) if carried.contains_key(v) => Some(*v),
                _ => None,
            })
        }
        _ => None,
    };
    let &[a, b] = &sum[..] else {
        return Err(StackShape);
    };
    let (sp, sp1, base) = match (below(t, a), below(t, b)) {
        (Some(sp), None) => (sp, a, b),
        (None, Some(sp)) => (sp, b, a),
        _ => return Err(StackShape),
    };
    // The base: the thread's id along x, the only dimension, times a stride.
    let thread = |t: &Terms, y: T| match t.get(y) {
        Term::Input(v) => io.inputs.contains(&(*v, Input::ThreadIdInGroup(0))),
        _ => false,
    };
    let number = |t: &Terms, y: T| match t.get(y) {
        Term::Const(k) => Some(*k),
        _ => None,
    };
    let stride = match t.get(base) {
        Term::Op("v_lshlrev_b32", 0, a) if a.len() == 2 && thread(t, a[1]) => {
            number(t, a[0]).filter(|&k| k < 32).map(|k| 1u32 << k)
        }
        Term::Op("v_mul_lo_u32", 0, a) if a.len() == 2 => {
            let by = |i: usize| number(t, a[i]).filter(|_| thread(t, a[1 - i]));
            by(0).or(by(1))
        }
        _ => None,
    };
    let flat = matches!(x.native.io, StageIo::Cs(cs) if cs.threads[1] == 1 && cs.threads[2] == 1);
    let stride = stride
        .filter(|s| flat && *s >= 4 && s.is_multiple_of(4))
        .ok_or(StackShape)?;

    // What the code outside the loop reads as data, and whether a lane
    // mask of the loop's reaches it.
    let from_loop = |t: &Terms, b: Bool| {
        st.bools.support(b).into_iter().any(|a| match a {
            Atom::Lane(v) | Atom::Uniform(v) | Atom::Phi(v) | Atom::Quad(v) | Atom::Opaque(v) => {
                within(t, v)
            }
            Atom::Constant(_) | Atom::Wave(_) => false,
        })
    };
    let (mut read_outside, mut mask_leaves) = (Vec::new(), false);
    let mut joins = HashMap::new();
    // What the instructions outside the loop read whose result the terms
    // know as nothing but itself (a carry out, a modifier), by that term.
    let mut unruled: HashMap<T, Vec<T>> = HashMap::new();
    let outside: Vec<usize> = (0..p.blocks.len()).filter(|&b| !l.holds(b)).collect();
    for &b in &outside {
        let block = &p.blocks[b];
        *visited += block.insts.len();
        joins.extend(block.phis.iter().map(|phi| (phi.value, &phi.args)));
        for (i, inst) in block.insts.iter().enumerate() {
            let guard = st.guards[b][i];
            mask_leaves |= guard.is_some_and(|g| from_loop(t, g));
            let reads: Vec<T> = data_reads(inst, guard.is_some())
                .into_iter()
                .map(|r| t.term(r.1))
                .collect();
            for &(_, w) in &inst.writes {
                if let Some(x) = t.find(&Term::Opaque(w)) {
                    unruled.insert(x, reads.clone());
                }
            }
            read_outside.extend(reads);
        }
        mask_leaves |= matches!(st.exits[b], LaneExit::Branch { when, .. } if from_loop(t, when));
    }
    read_outside.sort_unstable();
    read_outside.dedup();
    // The terms of what a join outside the loop joins.
    let joined: HashMap<T, Vec<T>> = joins
        .iter()
        .filter_map(|(&v, args)| {
            let x = t.find(&Term::Phi(v))?;
            Some((x, args.iter().map(|&a| t.term(a)).collect()))
        })
        .collect();

    // The pushes: where, and which result word.
    let mut conds = vec![when];
    let mut stores = Vec::new();
    for &s in &pushes {
        conds.push(st.reach[s.0]);
        conds.extend(st.guards[s.0][s.1]);
        let Some(at) = ds_address(t, s) else {
            return Err(StackShape);
        };
        let child = match ds_data(t, s).map(|d| t.get(d).clone()) {
            Some(Term::Load(site, k)) if site == node => k,
            _ => return Err(StackShape),
        };
        stores.push((s, at, child));
    }
    // The carried values that matter: the node test's operands, what the
    // loop's compares read and what is read outside the loop are made of
    // them. And every condition a value or a store of the loop depends on.
    // (A carried value met only on the side of a pick that no lane takes
    // is still counted: that refuses, it never accepts.)
    let mut roots: Vec<T> = operands.iter().map(|&v| t.term(v)).collect();
    roots.extend(t.compared(when));
    let mut need = Vec::new();
    loop {
        let before = (conds.len(), need.len());
        t.walk(&roots, &mut |_, term| match term {
            Term::Pick(c, ..) => {
                conds.push(*c);
                Some(Vec::new())
            }
            Term::Phi(v) => Some(carried.get(v).map_or(Vec::new(), |c| {
                need.push(*v);
                vec![c.next]
            })),
            _ => Some(Vec::new()),
        });
        // From outside: through the joins there, and from what the carried
        // values are on the way in.
        let mut from: Vec<T> = read_outside.clone();
        from.extend(need.iter().map(|v| carried[v].start));
        t.walk(&from, &mut |x, term| match term {
            Term::Phi(v) if carried.contains_key(v) => {
                need.push(*v);
                Some(Vec::new())
            }
            _ => Some(joined.get(&x).cloned().unwrap_or_default()),
        });
        // The stores' addresses: their conditions, and nothing they hold
        // in a lane that does not store.
        let at: Vec<T> = stores.iter().map(|s| s.1).collect();
        t.walk(&at, &mut |_, term| {
            if let Term::Pick(c, ..) = term {
                conds.push(*c);
            }
            Some(Vec::new())
        });
        conds.sort_unstable();
        conds.dedup();
        need.sort_unstable();
        need.dedup();
        for v in &need {
            roots.push(t.find(&Term::Phi(*v)).unwrap_or(zero));
        }
        for &c in &conds {
            roots.extend(t.compared(c));
        }
        if before == (conds.len(), need.len()) {
            break;
        }
    }

    // The cases, and what each free compare is.
    let ext = t.term(extent);
    let hit = |k: u8| Term::Load(node, k);
    let rcp = t
        .find(&hit(1))
        .and_then(|d| t.find(&Term::Op("v_rcp_f32", 0, vec![d])));
    let ratio = |t: &Terms, k: u8| {
        let mut args = vec![t.find(&hit(k))?, rcp?];
        args.sort_unstable();
        t.find(&Term::Op("v_mul_f32", 0, args))
    };
    let (dist, weight) = (ratio(t, 0), [ratio(t, 2), ratio(t, 3)]);
    let kind = {
        let mut args = vec![t.make(Term::Const(7)), ptr];
        args.sort_unstable();
        t.find(&Term::Op("v_and_b32", 0, args))
    };
    let next_sp = carried[&sp].next;
    let invariant = |t: &Terms, x: T| {
        let mut fixed = true;
        t.walk(&[x], &mut |_, term| {
            fixed &= match term {
                Term::Phi(v) | Term::Opaque(v) => !within(t, *v),
                Term::Load(s, _) => !l.holds(s.0),
                Term::Pick(..) => false,
                _ => true,
            };
            Some(Vec::new())
        });
        fixed
    };
    let mut cases = Cases {
        free: Vec::new(),
        index: HashMap::new(),
        fixed: HashMap::new(),
        roles: Vec::new(),
    };
    for atom in conds.iter().flat_map(|&c| st.bools.support(c)) {
        if cases.index.contains_key(&atom) || cases.fixed.contains_key(&atom) {
            continue;
        }
        let compared = match atom {
            Atom::Lane(v) => t.compares.get(&v).copied().filter(|c| l.holds(c.site.0)),
            _ => None,
        };
        // A compare made before the loop is what it was on the way in.
        if compared.is_none()
            && let Some(known) = st.bools.decides(&[(st.reach[l.header], true)], atom)
        {
            cases.fixed.insert(atom, known);
            continue;
        }
        let role = compared.map_or(Role::Other, |c| {
            let [a, b] = c.args;
            let (ka, kb) = (number(t, a), number(t, b));
            // A constant against a count that goes up by one each round.
            let counts = |y: T| {
                let step = |v: &Value| {
                    let c = carried.get(v)?;
                    let Term::Op("v_add_nc_u32", 0, sum) = t.get(c.next) else {
                        return None;
                    };
                    let one = sum.iter().any(|&z| number(t, z) == Some(1));
                    let own = sum.iter().any(|&z| *t.get(z) == Term::Phi(*v));
                    (one && own).then_some(c.next)
                };
                match t.get(y) {
                    Term::Phi(v) => step(v).is_some(),
                    _ => carried.keys().any(|v| step(v) == Some(y)),
                }
            };
            let cap = || match (ka, kb) {
                (Some(_), None) if counts(b) => Role::Cap,
                (None, Some(_)) if counts(a) => Role::Cap,
                _ => Role::Other,
            };
            match c.op {
                "gt_u32" if ka == Some(4) && Some(b) == kind => Role::Triangle,
                "lt_u32" if Some(a) == kind && kb == Some(4) => Role::Triangle,
                "lt_u32" if a == ptr => match t.get(b) {
                    Term::Phi(v) if carried.contains_key(v) => Role::Key(*v),
                    _ => Role::Other,
                },
                "ne_u32" => {
                    let (k, other) = match (ka, kb) {
                        (Some(k), None) => (k, b),
                        (None, Some(k)) => (k, a),
                        _ => return Role::Other,
                    };
                    match (k, t.get(other)) {
                        (0, _) if other == next_sp => Role::NotEmpty,
                        (u32::MAX, &Term::Load(s, word)) if s == node => Role::Valid(word),
                        _ => cap(),
                    }
                }
                "eq_f32" if (Some(a), b) == (dist, ext) || (Some(b), a) == (dist, ext) => {
                    Role::Equal
                }
                op => match Compare::of(op) {
                    Some((swapped, form)) => {
                        let (low, high) = if swapped { (b, a) } else { (a, b) };
                        if Some(low) == dist && high == ext {
                            Role::Extent(form)
                        } else if number(t, low) == Some(0)
                            && t.find(&hit(1)) == Some(high)
                            && form == Compare::LESS
                        {
                            Role::Front
                        } else if Some(high) == dist && invariant(t, low) {
                            Role::Minimum(form, swapped)
                        } else {
                            Role::Other
                        }
                    }
                    None => cap(),
                },
            }
        });
        cases.index.insert(atom, cases.free.len());
        cases.free.push(atom);
        cases.roles.push(role);
    }
    if cases.free.len() > MAX_ATOMS {
        return Err(TooManyCompares);
    }
    work.cases += cases.count() as usize;
    // The loop goes round again exactly while the stack holds a pointer.
    let again = cases.table(|c| t.holds(when, &cases.truth(c)) == stays);
    let on = cases.depends(&again);
    let not_empty = on
        .iter()
        .copied()
        .find(|&i| cases.roles[i] == Role::NotEmpty);
    if on.len() != 1 || not_empty.is_none_or(|i| again != cases.table(|c| c >> i & 1 != 0)) {
        let capped = on.iter().any(|&i| cases.roles[i] == Role::Cap);
        return Err(if capped { IterationCap } else { ExitForm });
    }

    // What each carried value becomes: itself, or one thing in the cases
    // that replace it.
    struct Slot {
        value: Value,
        kind: Option<Kind>,
        replaced: Vec<bool>,
    }
    let front = (0..cases.free.len()).find(|&i| cases.roles[i] == Role::Front);
    let mut slots = Vec::new();
    for &value in need.iter().filter(|&&v| v != sp) {
        let own = t.find(&Term::Phi(value));
        let leaves: Vec<T> = (0..cases.count())
            .map(|c| t.leaf(carried[&value].next, &cases.truth(c)))
            .collect();
        let replaced: Vec<bool> = leaves.iter().map(|&x| Some(x) != own).collect();
        let mut new: Vec<T> = leaves.iter().copied().filter(|&x| Some(x) != own).collect();
        new.sort_unstable();
        new.dedup();
        let kind = match new[..] {
            [x] if Some(x) == dist => Some(Kind::Distance),
            [x] if Some(x) == weight[0] => Some(Kind::Barycentric(0)),
            [x] if Some(x) == weight[1] => Some(Kind::Barycentric(1)),
            [x] if x == ptr => Some(Kind::NodePointer),
            [x] => number(t, x).map(Kind::Flag),
            // One of two constants, by the denominator's sign alone.
            [x, y] => front
                .zip(number(t, x).zip(number(t, y)))
                .and_then(|(f, (kx, ky))| {
                    let by_sign = |a: T, b: T| {
                        (0..cases.count())
                            .filter(|&c| replaced[c as usize])
                            .all(|c| leaves[c as usize] == if c >> f & 1 != 0 { a } else { b })
                    };
                    if by_sign(x, y) {
                        Some(Kind::Facing {
                            front: kx,
                            back: ky,
                        })
                    } else if by_sign(y, x) {
                        Some(Kind::Facing {
                            front: ky,
                            back: kx,
                        })
                    } else {
                        None
                    }
                }),
            _ => None,
        };
        slots.push(Slot {
            value,
            kind,
            replaced,
        });
    }
    let kind_of = |v: Value| slots.iter().find(|s| s.value == v).and_then(|s| s.kind);

    // When a candidate is accepted: the same for everything kept.
    let mut tables = slots
        .iter()
        .filter(|s| s.kind.is_some())
        .map(|s| &s.replaced);
    let Some(accept) = tables.next() else {
        return Err(AcceptForm);
    };
    if tables.any(|other| other != accept) {
        return Err(AcceptForm);
    }
    let on = cases.depends(accept);
    if on.iter().any(|&i| cases.roles[i] == Role::Other) {
        return Err(CompareForm);
    }
    let one = |role: &dyn Fn(Role) -> bool| cases.one(&on, role).map_err(|()| AcceptForm);
    let triangle = one(&|r| r == Role::Triangle)?;
    let minimum = one(&|r| matches!(r, Role::Minimum(..)))?;
    let within_extent = one(&|r| matches!(r, Role::Extent(_)))?;
    let equal = one(&|r| r == Role::Equal)?;
    let key = one(&|r| matches!(r, Role::Key(_)))?;
    let (Some(triangle), Some(minimum)) = (triangle, minimum) else {
        return Err(AcceptForm);
    };
    let known = [Some(triangle), Some(minimum), within_extent, equal, key];
    if on.iter().any(|i| !known.contains(&Some(*i))) {
        return Err(AcceptForm);
    }
    let bit = |c: u32, i: usize| c >> i & 1 != 0;
    let is = |f: &dyn Fn(u32) -> bool| {
        *accept == cases.table(|c| bit(c, triangle) && bit(c, minimum) && f(c))
    };
    let tie = match (within_extent, equal, key) {
        (None, None, None) if is(&|_| true) => return Err(ExtentCompare),
        (Some(e), None, None) if is(&|c| bit(c, e)) => false,
        (Some(e), Some(q), Some(k)) if is(&|c| bit(c, e) || bit(c, q) && bit(c, k)) => true,
        _ => return Err(AcceptForm),
    };
    let (Some(Role::Extent(extent_form)), Role::Minimum(minimum_form, swapped)) =
        (within_extent.map(|i| cases.roles[i]), cases.roles[minimum])
    else {
        return Err(AcceptForm);
    };

    // The extent: the same throughout, or the distance kept so far.
    let arg = |t: &mut Terms, v: Value| {
        let x = t.term(v);
        number(t, x).map_or(Arg::Value(v), Arg::Const)
    };
    let (shrinks, start) = match *t.get(ext) {
        Term::Phi(v) if carried.contains_key(&v) => {
            if kind_of(v) != Some(Kind::Distance) {
                return Err(RayChanges);
            }
            (true, arg(t, carried[&v].init))
        }
        _ if within(t, extent) => return Err(RayChanges),
        _ => (false, arg(t, extent)),
    };

    // The stack, case by case: a triangle's round pushes nothing (or
    // empties the stack exactly when its candidate is accepted); any other
    // round pushes each valid result word once, one above the other.
    let mut children: Vec<u8> = stores.iter().map(|s| s.2).collect();
    children.sort_unstable();
    let mut valid = [0; 4];
    for (k, i) in valid.iter_mut().enumerate() {
        let all: Vec<usize> = (0..cases.free.len()).collect();
        match cases.one(&all, |r| r == Role::Valid(k as u8)) {
            Ok(Some(found)) => *i = found,
            _ => return Err(StackShape),
        }
    }
    if children != [0, 1, 2, 3] {
        return Err(StackShape);
    }
    // How many times 4 was added to what `x` is in a case, and to what.
    let raised = |t: &Terms, mut x: T, case: u32| {
        let mut n = 0;
        loop {
            x = t.leaf(x, &cases.truth(case));
            match t.get(x) {
                Term::Op("v_add_nc_u32", 0, a) if a.len() == 2 && a.contains(&four) => {
                    n += 1;
                    x = if a[0] == four { a[1] } else { a[0] };
                }
                _ => return (n, x),
            }
        }
    };
    let mut emptied = Vec::new();
    for case in 0..cases.count() {
        let truth = cases.truth(case);
        let (n, at) = raised(t, next_sp, case);
        if bit(case, triangle) {
            if n != 0 || (at != sp1 && at != zero) {
                return Err(StackShape);
            }
            emptied.push(at == zero);
        } else {
            let wanted = valid.iter().filter(|&&i| bit(case, i)).count();
            if at != sp1 || n != wanted {
                return Err(StackShape);
            }
            emptied.push(false);
        }
        let mut pushed = 0;
        for &(s, address, child) in &stores {
            let runs = t.holds(st.reach[s.0], &truth)
                && st.guards[s.0][s.1].is_none_or(|g| t.holds(g, &truth));
            if runs != (!bit(case, triangle) && bit(case, valid[usize::from(child)])) {
                return Err(StackShape);
            }
            if !runs {
                continue;
            }
            let slot = match t.get(t.leaf(address, &truth)) {
                Term::Op("v_add_nc_u32", 0, a) if a.len() == 2 && a.contains(&base) => {
                    Some(if a[0] == base { a[1] } else { a[0] })
                }
                _ => None,
            };
            if slot.map(|y| raised(t, y, case)) != Some((pushed, sp1)) {
                return Err(StackShape);
            }
            pushed += 1;
        }
    }
    let first_hit = emptied.contains(&true);
    if first_hit && emptied != *accept {
        return Err(StackShape);
    }
    // The root: a constant stored at the thread's base on the way in, by
    // every lane that comes that way (a lane the store's own guard leaves
    // out would pop what nothing stored), the stack pointer one entry
    // above it, and no other use of shared memory.
    *visited += size(&outside);
    let elsewhere: Vec<_> = touches(p, res, &outside)
        .into_iter()
        .filter(|a| a.memory == Memory::Shared)
        .collect();
    let root = match elsewhere[..] {
        [a] if a.store
            && inst_at(a.site).name() == "ds_write_b32"
            && st.guards[a.site.0][a.site.1].is_none()
            && ds_address(t, a.site) == Some(base) =>
        {
            ds_data(t, a.site).and_then(|d| number(t, d)).filter(|_| {
                // Stored on the one path into the loop: every lane that
                // enters it has been through the store's block.
                let mut b = entry;
                while b != a.site.0 && p.blocks[b].preds.len() == 1 {
                    b = p.blocks[b].preds[0];
                }
                b == a.site.0
            })
        }
        _ => None,
    };
    let start_sp = t.term(carried[&sp].init);
    let (Some(root), true) = (root, start_sp == four) else {
        return Err(StackShape);
    };

    // The order the result depends on.
    let kinds: Vec<Kind> = slots.iter().filter_map(|s| s.kind).collect();
    let flags = kinds.iter().all(|k| matches!(k, Kind::Flag(_)));
    let order = if tie {
        let keyed = matches!(key.map(|i| cases.roles[i]), Some(Role::Key(v)) if kind_of(v) == Some(Kind::NodePointer));
        if !(shrinks && keyed && extent_form == Compare::LESS && !first_hit) {
            return Err(AcceptForm);
        }
        Order::TieByKey
    } else if flags {
        Order::None
    } else if shrinks && !first_hit {
        Order::TieByVisit
    } else {
        return Err(VisitOrder);
    };

    // What leaves: whatever the code outside reads of the loop, or the
    // loop is given back on its next entry, is a kept value as a whole.
    if mask_leaves || slots.iter().any(|s| s.kind.is_none()) {
        return Err(LiveOut);
    }
    let mut from: Vec<T> = read_outside.clone();
    from.extend(need.iter().map(|v| carried[v].start));
    let (mut left, mut stray) = (Vec::new(), false);
    t.walk(&from, &mut |x, term| {
        if let Some(slot) = slots.iter().find(|s| carried[&s.value].next == x) {
            left.push(slot.value);
            return None;
        }
        stray |= match term {
            Term::Phi(v) | Term::Opaque(v) => within(t, *v),
            Term::Load(s, _) => l.holds(s.0),
            Term::Pick(c, ..) => from_loop(t, *c),
            _ => false,
        };
        Some(joined.get(&x).cloned().unwrap_or_default())
    });
    if stray {
        return Err(LiveOut);
    }
    left.sort_unstable();
    left.dedup();
    let mut kept = Vec::new();
    for v in left {
        let Some(kind) = kind_of(v) else {
            return Err(LiveOut);
        };
        let miss = arg(t, carried[&v].init);
        kept.push(Kept {
            value: carried[&v].last,
            kind,
            miss,
        });
    }

    // A kept pointer that memory is reached through outside the loop:
    // straight after it, through a join there (a later pass of a loop
    // around this one), or through an instruction the terms have no rule
    // for (whatever such an instruction reads, its result is made of).
    let pointers: Vec<T> = slots
        .iter()
        .filter(|s| s.kind == Some(Kind::NodePointer))
        .map(|s| carried[&s.value].next)
        .collect();
    if !pointers.is_empty() {
        let mut through = Vec::new();
        *visited += size(&outside);
        for a in touches(p, res, &outside) {
            let inst = inst_at(a.site);
            for op in addresses(inst) {
                let dwords = match op {
                    Operand::Vector { reg, dwords } => (0..dwords)
                        .filter_map(|k| inst.read(Reg::V(reg + k)))
                        .collect(),
                    Operand::Scalar { code, dwords } if code != crate::gfx10::NULL => (0..dwords)
                        .filter_map(|k| inst.read(Reg::S(code + k)))
                        .collect(),
                    _ => Vec::new(),
                };
                through.extend(dwords.into_iter().map(|v| t.term(v)));
            }
        }
        let mut reached = false;
        t.walk(&through, &mut |x, _| {
            reached |= pointers.contains(&x);
            let mut more = joined.get(&x).cloned().unwrap_or_default();
            more.extend(unruled.get(&x).into_iter().flatten());
            Some(more)
        });
        if reached {
            return Err(NodeLoad);
        }
    }

    // The node test's answer for a triangle the ray misses is a candidate
    // at t = +inf (numerator +inf, denominator 1): whether the accept can
    // pass it, by the extent's compare; and in a tie by key, where an
    // infinite extent equals it, unless nothing is below the kept
    // pointer's first value.
    let below_nothing = matches!(
        key.map(|i| cases.roles[i]),
        Some(Role::Key(v)) if number(t, carried[&v].start) == Some(0)
    );
    let ties = tie && !below_nothing;
    let miss_word = MissWord {
        at_infinite: extent_form.holds(f32::INFINITY, f32::INFINITY) || ties,
        at_nan: extent_form.holds(f32::INFINITY, f32::NAN),
    };

    let compared = match cases.free[minimum] {
        Atom::Lane(v) => t.compares.get(&v).copied(),
        _ => None,
    };
    let minimum_from = compared.and_then(|c| {
        let inst = inst_at(c.site);
        let Fields::Valu(v) = &inst.fields else {
            return None;
        };
        Some(match *v.src.get(usize::from(swapped))? {
            Operand::Vector { reg, .. } => arg(t, inst.read(Reg::V(reg))?),
            Operand::Scalar { code, .. } => arg(t, inst.read(Reg::S(code))?),
            Operand::Const(c) => Arg::Const(resource::inline(c)?),
            Operand::Literal(x) => Arg::Const(x),
            Operand::Attr { .. } => return None,
        })
    });
    let Some(minimum_from) = minimum_from else {
        return Err(AcceptForm);
    };
    let mut description = Description {
        region,
        node,
        ray: RaySource {
            origin,
            direction,
            inverse: Inverse::Reciprocal,
            minimum: (minimum_from, minimum_form),
            extent: Extent {
                compare: extent_form,
                start,
                shrinks,
                miss_word,
            },
        },
        // (`StructureSource` alone is the refusal here.)
        structure: super::describe::StructureSource::RootParameter(root_parameter),
        root: Root::Constant(root),
        flags: Flags {
            accept_first_hit: first_hit,
            ..Flags::default()
        },
        mask: Mask::Untested,
        instance: InstanceCode::None,
        filter: Filter::None,
        kept,
        order,
        stack: Stack {
            stride,
            checked: false,
        },
        class: Class(0),
    };
    description.class = Description::class_of(&description.forms());
    Ok(description)
}
