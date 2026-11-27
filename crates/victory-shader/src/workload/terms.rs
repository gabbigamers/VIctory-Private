//! A program's values as one lane computes them: each SSA value as a term
//! over the instructions' names and operands, with the per-lane form's
//! conditions (`structure::Bool`) where a lane's value depends on its path.
//! Equal terms are one term, so "the same computation" is an equal number.
//!
//! One pass over the program in reverse postorder, an instruction at a
//! time. A term claims only what its rule below says; whatever has no rule
//! is [`Term::Opaque`], which equals nothing but itself.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use super::region::reads_beyond_its_operands;
use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{self, Inst, Program, Reg, Value};
use crate::resource::{self, Site};
use crate::structure::{Atom, Bool, LaneExit, Structured};

/// A term, by number.
pub(crate) type T = u32;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Term {
    Const(u32),
    /// An operand that is no register and no number here (`vccz`, an
    /// attribute), by its code.
    Code(u8),
    /// A register read before any write: what the hardware set up.
    Input(Value),
    /// A join this pass does not see through: a loop header's, or one that
    /// is not the end of an `if`.
    Phi(Value),
    /// Dword `k` of what the load at a site reads, the node test's four
    /// result words among them.
    Load(Site, u8),
    /// Result dword `k` of the instruction of this name on these operands.
    /// Only for an instruction whose result is a function of them: no
    /// modifier, every operand one dword (a scalar instruction's: each
    /// dword it reads, SCC and EXEC included, then its constants), and no
    /// register read that is not among them. The operands of a vector
    /// instruction that commutes are in the order of their numbers.
    Op(&'static str, u8, Vec<T>),
    /// The first where the lane's condition holds, else the second.
    Pick(Bool, T, T),
    /// Anything else, by the value.
    Opaque(Value),
}

/// Instructions whose two operands may be swapped.
const COMMUTE: &[&str] = &[
    "v_add_nc_u32",
    "v_add_f32",
    "v_mul_f32",
    "v_mul_lo_u32",
    "v_and_b32",
    "v_or_b32",
    "v_xor_b32",
    "v_min_f32",
    "v_max_f32",
];

/// A compare: its name after `v_cmp_` or `v_cmpx_`, its two operands, and
/// where it is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Compared {
    pub op: &'static str,
    pub args: [T; 2],
    pub site: Site,
}

pub(crate) struct Terms<'a> {
    pub p: &'a Program,
    pub st: &'a Structured,
    terms: Vec<Term>,
    ids: HashMap<Term, T>,
    /// By value.
    of: Vec<T>,
    /// The block each value is defined in (`usize::MAX`: an input).
    pub block: Vec<usize>,
    /// Compares without modifiers, by the value of the mask they write
    /// (its low dword).
    pub compares: HashMap<Value, Compared>,
    /// Instructions this pass looked at.
    pub visited: usize,
    /// Calls of [`Terms::holds`]: conditions evaluated in a case.
    pub evaluated: Cell<usize>,
}

const NONE: T = T::MAX;

/// How many values `p` has: one more than the largest.
pub(crate) fn values(p: &Program) -> usize {
    let inputs = p.inputs.iter().map(|i| i.1);
    let blocks = p.blocks.iter().flat_map(|b| {
        let phis = b.phis.iter().map(|phi| phi.value);
        phis.chain(b.insts.iter().flat_map(|i| i.writes.iter().map(|w| w.1)))
    });
    inputs
        .chain(blocks)
        .map(|v| v.0 as usize + 1)
        .max()
        .unwrap_or(0)
}

impl<'a> Terms<'a> {
    pub fn new(p: &'a Program, st: &'a Structured) -> Self {
        let count = values(p);
        let mut t = Terms {
            p,
            st,
            terms: Vec::new(),
            ids: HashMap::new(),
            of: vec![NONE; count],
            block: vec![usize::MAX; count],
            compares: HashMap::new(),
            visited: 0,
            evaluated: Cell::new(0),
        };
        for &(_, v) in &p.inputs {
            t.of[v.0 as usize] = t.make(Term::Input(v));
        }
        let order = ir::reverse_postorder(&p.blocks);
        let mut rpo = vec![usize::MAX; p.blocks.len()];
        for (n, &b) in order.iter().enumerate() {
            rpo[b] = n;
        }
        for &b in &order {
            let block = &p.blocks[b];
            let header = !ir::latches(&p.blocks, &rpo, b).is_empty();
            for phi in &block.phis {
                t.block[phi.value.0 as usize] = b;
                let joined = if header { None } else { t.join(b, &phi.args) };
                t.of[phi.value.0 as usize] = joined.unwrap_or_else(|| t.make(Term::Phi(phi.value)));
            }
            for (i, inst) in block.insts.iter().enumerate() {
                t.visited += 1;
                for &(_, w) in &inst.writes {
                    t.block[w.0 as usize] = b;
                }
                t.inst((b, i), inst);
            }
        }
        t
    }

    pub fn make(&mut self, term: Term) -> T {
        if let Some(&t) = self.ids.get(&term) {
            return t;
        }
        self.terms.push(term.clone());
        self.ids.insert(term, self.terms.len() as T - 1);
        self.terms.len() as T - 1
    }

    /// The number of `term` if any value has it.
    pub fn find(&self, term: &Term) -> Option<T> {
        self.ids.get(term).copied()
    }

    pub fn get(&self, t: T) -> &Term {
        &self.terms[t as usize]
    }

    /// The term of `v`; a value nothing reachable defines is opaque.
    pub fn term(&mut self, v: Value) -> T {
        match self.of.get(v.0 as usize) {
            Some(&t) if t != NONE => t,
            _ => self.make(Term::Opaque(v)),
        }
    }

    fn pick(&mut self, when: Bool, then: T, otherwise: T) -> T {
        if then == otherwise || when == Bool::TRUE {
            then
        } else if when == Bool::FALSE {
            otherwise
        } else {
            self.make(Term::Pick(when, then, otherwise))
        }
    }

    /// The join of `args` at block `b` where `b` ends an `if`: its two
    /// predecessors are the block that branches and the one block of its
    /// arm, or the two one-block arms of one branch.
    fn join(&mut self, b: usize, args: &[Value]) -> Option<T> {
        let (p, st) = (self.p, self.st);
        let &[q0, q1] = &p.blocks[b].preds[..] else {
            return None;
        };
        let arm = |q: usize, a: usize| p.blocks[q].preds == [a] && st.exits[q] == LaneExit::Jump(b);
        // The block that branches, and where each predecessor's edge starts
        // from it.
        let (a, from) = if arm(q1, q0) {
            (q0, [b, q1])
        } else if arm(q0, q1) {
            (q1, [q0, b])
        } else if let &[a] = &p.blocks[q0].preds[..]
            && arm(q0, a)
            && arm(q1, a)
        {
            (a, [q0, q1])
        } else {
            return None;
        };
        let LaneExit::Branch {
            when,
            taken,
            not_taken,
        } = st.exits[a]
        else {
            return None;
        };
        let k = if (taken, not_taken) == (from[0], from[1]) {
            0
        } else if (taken, not_taken) == (from[1], from[0]) {
            1
        } else {
            return None;
        };
        let (then, otherwise) = (self.term(args[k]), self.term(args[1 - k]));
        Some(self.pick(when, then, otherwise))
    }

    fn operand(&mut self, inst: &Inst, op: &Operand, k: u8) -> T {
        let read = |reg| inst.read(reg);
        match *op {
            Operand::Scalar { code, .. } if code == NULL => self.make(Term::Const(0)),
            Operand::Scalar { code, .. } => match read(Reg::S(code + k)) {
                Some(v) => self.term(v),
                None => self.make(Term::Code(code)),
            },
            Operand::Vector { reg, .. } => match read(Reg::V(reg + k)) {
                Some(v) => self.term(v),
                None => self.make(Term::Code(0)),
            },
            Operand::Const(c) => match resource::inline(c) {
                Some(x) => self.make(Term::Const(x)),
                None => self.make(Term::Code(c)),
            },
            Operand::Literal(x) => self.make(Term::Const(x)),
            Operand::Attr { .. } => self.make(Term::Code(0)),
        }
    }

    fn inst(&mut self, site: Site, inst: &Inst) {
        let name = inst.name();
        let guard = self.st.guards[site.0][site.1];
        // What each written dword is, where a rule says; the rest opaque.
        let mut new: Vec<Option<T>> = vec![None; inst.writes.len()];
        match &inst.fields {
            Fields::Valu(v) => {
                let single = |o: &Operand| match *o {
                    Operand::Scalar { dwords, .. } | Operand::Vector { dwords, .. } => dwords == 1,
                    _ => true,
                };
                let plain = v.neg == 0
                    && v.abs == 0
                    && v.neg_hi == 0
                    && v.opsel == 0
                    && v.opsel_hi == 0
                    && !v.clamp
                    && v.omod == 0
                    && v.sdwa.is_none()
                    && v.dpp.is_none();
                // A function of its operands: `v_cndmask_b32` reads its
                // mask as a mask, whatever its width.
                let pure = plain
                    && !reads_beyond_its_operands(inst)
                    && v.src.iter().take(2).all(single)
                    && (name == "v_cndmask_b32" || v.src.iter().all(single));
                let src: Vec<T> = v.src.iter().map(|o| self.operand(inst, o, 0)).collect();
                let compare = name.strip_prefix("v_cmpx_").or(name.strip_prefix("v_cmp_"));
                if let (Some(op), true, &[a, b], Some(w)) =
                    (compare, pure, &src[..], inst.writes.first())
                {
                    // The lane's bit is the per-lane form's; the operands
                    // are kept for whoever asks what was compared.
                    self.compares.insert(
                        w.1,
                        Compared {
                            op,
                            args: [a, b],
                            site,
                        },
                    );
                } else if compare.is_none()
                    && pure
                    && matches!(v.dst, Some(Operand::Vector { dwords: 1, .. }))
                    && v.carry.is_none()
                    && let Some(first) = new.first_mut()
                {
                    *first = match name {
                        "v_mov_b32" => src.first().copied(),
                        "v_cndmask_b32" => self.select(inst, v.src.get(2), &src),
                        _ => {
                            let mut src = src;
                            if COMMUTE.contains(&name) {
                                src.sort_unstable();
                            }
                            Some(self.make(Term::Op(name, 0, src)))
                        }
                    };
                }
            }
            Fields::Sop { dst, src, imm } => {
                // Every dword it reads, SCC and EXEC among them, then its
                // constants and its immediate.
                let mut args: Vec<T> = inst.reads.iter().map(|r| self.term(r.1)).collect();
                for s in src {
                    if matches!(s, Operand::Const(_) | Operand::Literal(_)) {
                        args.push(self.operand(inst, s, 0));
                    }
                }
                args.extend(imm.map(|i| self.make(Term::Const(u32::from(i)))));
                let copied = match (name, &src[..]) {
                    ("s_mov_b32", [from]) => Some(self.operand(inst, from, 0)),
                    _ => None,
                };
                for (k, &(reg, _)) in inst.writes.iter().enumerate() {
                    if reg == Reg::Scc || dst.is_none() {
                        continue;
                    }
                    new[k] = Some(match copied {
                        Some(from) => from,
                        None => self.make(Term::Op(name, k as u8, args.clone())),
                    });
                }
            }
            // A load's dwords; an atomic's old value is left opaque.
            Fields::Smem { .. }
            | Fields::Ds { .. }
            | Fields::Buffer { .. }
            | Fields::Flat { .. }
            | Fields::Image { .. }
                if !name.contains("atomic") && !name.contains("cmpswap") =>
            {
                for (k, n) in new.iter_mut().enumerate() {
                    *n = Some(self.make(Term::Load(site, k as u8)));
                }
            }
            _ => {}
        }
        for (&(reg, w), new) in inst.writes.iter().zip(new) {
            let mut t = match new {
                Some(t) => t,
                None => self.make(Term::Opaque(w)),
            };
            // A lane the guard leaves out keeps what the register held.
            if let (Some(g), Reg::V(_), Some(old)) = (guard, reg, inst.read(reg)) {
                let old = self.term(old);
                t = self.pick(g, t, old);
            }
            self.of[w.0 as usize] = t;
        }
    }

    /// `v_cndmask_b32`: the second operand where the lane's bit of the mask
    /// is set, else the first.
    fn select(&mut self, inst: &Inst, mask: Option<&Operand>, src: &[T]) -> Option<T> {
        let &Operand::Scalar { code, .. } = mask? else {
            return None;
        };
        let m = inst.read(Reg::S(code))?;
        let when = *self.st.masks.get(&m).or_else(|| self.st.other.get(&m))?;
        Some(self.pick(when, *src.get(1)?, *src.first()?))
    }

    /// Whether the lane's condition `b` holds where each atom has the
    /// value `truth` gives it.
    pub fn holds(&self, mut b: Bool, truth: &dyn Fn(Atom) -> bool) -> bool {
        self.evaluated.set(self.evaluated.get() + 1);
        while let Some((atom, low, high)) = self.st.bools.node(b) {
            b = if truth(atom) { high } else { low };
        }
        b == Bool::TRUE
    }

    /// `t` in a lane whose atoms are as `truth` says: the picks taken.
    pub fn leaf(&self, mut t: T, truth: &dyn Fn(Atom) -> bool) -> T {
        while let Term::Pick(when, then, otherwise) = *self.get(t) {
            t = if self.holds(when, truth) {
                then
            } else {
                otherwise
            };
        }
        t
    }

    /// Every term `roots` are made of, each given to `each` once: its
    /// operands, both sides of a pick and the operands of the compares the
    /// pick depends on follow, with whatever further terms `each` names
    /// (what a loop's phi becomes, say), unless `each` says `None`, which
    /// ends the walk below that term.
    pub fn walk(&self, roots: &[T], each: &mut dyn FnMut(T, &Term) -> Option<Vec<T>>) {
        let mut seen = HashSet::new();
        let mut work = roots.to_vec();
        while let Some(t) = work.pop() {
            if !seen.insert(t) {
                continue;
            }
            let term = self.get(t);
            let Some(more) = each(t, term) else {
                continue;
            };
            work.extend(more);
            match term {
                Term::Op(_, _, args) => work.extend(args),
                Term::Pick(when, then, otherwise) => {
                    work.extend([*then, *otherwise]);
                    work.extend(self.compared(*when));
                }
                _ => {}
            }
        }
    }

    /// The operands of the compares the condition `b` depends on.
    pub fn compared(&self, b: Bool) -> Vec<T> {
        let atoms = self.st.bools.support(b);
        let of = |a: Atom| match a {
            Atom::Lane(v) => self.compares.get(&v).map(|c| c.args),
            _ => None,
        };
        atoms.into_iter().filter_map(of).flatten().collect()
    }
}
