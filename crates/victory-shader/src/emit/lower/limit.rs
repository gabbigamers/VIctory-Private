//! The limits on translated loops, [`LOOP_LIMIT`] for each loop and
//! [`LOOP_TOTAL_LIMIT`] for all of a lane's loops together: which back
//! edges make one loop, the counts, the limits at a conditional back edge,
//! and the tests. Each loop counts the iterations a lane has made since it
//! entered that loop, the lane counts every back edge it takes, and a back
//! edge is taken only below both limits. What a lane either limit stops
//! does then is its stage's to say: the module `stopped`.
//!
//! # Loops
//!
//! The control flow that reaches the lowering is reducible. `ir::build`
//! splits a loop entered at several blocks into copies and refuses what it
//! cannot split, and structurisation refuses irreducible flow again (both
//! with "irreducible control flow"), so no such program is lowered. Every
//! edge to a block at or before its source in reverse postorder is then a
//! back edge: its target, the header, is on every path to its source.
//!
//! The natural loop of a back edge is its header with every block that
//! reaches the edge's source without passing the header. Back edges to
//! different headers belong to different loops. The back edges to one
//! header are placed by how their natural loops lie in one another: a back
//! edge's place is the length of the longest chain of back edges to that
//! header, the natural loop of each properly inside the next's, that ends
//! properly inside its own (0 where none is inside it). The header's loop
//! n is what the natural loops of its back edges of place n and below hold
//! together. The back edges of place n are that loop's own, the ones that
//! count for it; those of lesser place belong to the loops inside it. Each
//! of a header's loops is properly inside the next, and a back edge starts
//! from outside every loop of the header below its own
//! ([`crate::ir::back_edge_places`], which the region analysis shares).
//!
//! Two back edges of which one's natural loop is properly inside the
//! other's are therefore always two loops, whatever else goes back to the
//! header, and two with the same natural loop are one. Two of which each
//! has a block the other lacks are one loop where the chains inside them
//! are equally long, as where nothing is inside either; where they are
//! not, the one with the shorter chain counts for the loop further in.
//!
//! An outer loop whose body begins with its inner loop is two loops with
//! one header: the inner back edge's natural loop is inside the outer
//! one's. A `continue` and the loop's end are one loop where neither
//! branch back is reached from the other without passing the header (the
//! `continue` ends an arm of a branch). In a nest with one header a
//! `continue` from an arm of the inner loop's body has nothing inside its
//! natural loop and counts for the inner loop. The graph does not say
//! whether it is the inner loop's `continue` or the outer one's, and this
//! reading starts no count again; read as the outer loop's, it would
//! start the inner loop's count again.
//!
//! A conditional `continue` that falls through towards the loop's end is
//! the same graph as an inner loop sharing the header, so it cannot be
//! told from one and is counted as one. Either count is then at most what
//! one count for both would be, so that reading stops no lane the other
//! would let run. It does let such a loop run longer: the count of the
//! `continue` starts again each time the loop's end is taken, so what
//! bounds a lane that takes both in turn is [`LOOP_TOTAL_LIMIT`] and not
//! [`LOOP_LIMIT`]. The same holds for a conditional `continue` of an
//! outer loop taken from inside an inner one.
//!
//! # Counts
//!
//! A loop's count is a phi at its header; a header that n loops begin at
//! has n of them. Along a forward edge into the header each takes 0: the
//! lane enters all of those loops. Along a back edge the count of the
//! edge's own loop takes itself plus one, the counts of the loops inside it
//! take 0 (the lane enters them again from outside them), and the counts
//! of the loops around it take themselves. A loop inside another that
//! begins at a block of its own is entered along a forward edge.
//!
//! The lane's count of all its back edges is one value followed through
//! the function. It is 0 at the entry, a block passes on what it received,
//! and a back edge passes on one more. Where the edges into a block bring
//! different values the block has a phi of them: always at a loop header
//! (a back edge brings more than the entry does), and at a join of forward
//! edges where they differ and a back edge can still follow (after the
//! last one nothing reads the count). Blocks are placed in reverse
//! postorder, so a block that is no header knows what each predecessor
//! holds, and what a block holds is 0 or a phi of a block on every path to
//! it: it is computed wherever it is read. [`Lower::count_loops`] makes the
//! phis before any block is lowered. Each forward edge then adds its entry
//! where the edge is made ([`Lower::enter`]) and each back edge where it is
//! counted ([`Lower::count_iteration`]), which is every edge into such a
//! block once, as for the loops' counts.
//!
//! # Shape
//!
//! Host drivers are sensitive to the shape of a loop: the AMD driver hung
//! on a loop exit that tested an `xor` of a compare, and hangs on divergent
//! loops whose exiting block is not the latch (`bitcode`, where the module
//! is laid out). A loop therefore stays as DXC would emit it. A back edge
//! keeps its block, a conditional latch keeps its one two-way branch with
//! the limits folded into its condition and leaves the loop to a block of
//! its own, and what tells the stopped lanes from the others comes there,
//! after the loop is left ([`Lower::limit_exit`]).
//!
//! With `count` the count of the back edge's loop and `total` the lane's,
//! both as the latch received them, a latch branches on:
//!
//! * an unconditional back edge: `count + 1 < LOOP_LIMIT and total + 1 <
//!   LOOP_TOTAL_LIMIT`, round where it holds and to the stop block where
//!   not;
//! * a back edge taken where the latch's condition holds: `condition and
//!   (count + 1 < LOOP_LIMIT and total + 1 < LOOP_TOTAL_LIMIT)`;
//! * a back edge taken where it does not hold: `condition or (count + 1 >=
//!   LOOP_LIMIT or total + 1 >= LOOP_TOTAL_LIMIT)`, out where it holds.
//!
//! These are unsigned compares of an `add` with a constant, joined by
//! `and` or `or`, and no other kind of instruction. A program with a loop
//! has, beyond its own code: at each header one phi per loop that begins
//! there and one for the lane's count; at each join of forward edges that
//! bring different counts of the lane before a later back edge, one phi;
//! at each back edge two adds, two compares and the `and` or `or` of the
//! compares. A program without a loop has none of it.
//!
//! The block on a conditional latch's way out of its loop sends the
//! stopped lanes to the end of the program in every stage but one: in a
//! pixel program it discards them and lets them go on with the others,
//! without a branch (`stopped`, which says what such a program has there
//! beyond the list above: a select, and where the back edge is the side
//! not taken a negation of the latch's condition, which is no `xor` where
//! the condition is a compare). The rest of this section is about the
//! block that branches.
//!
//! In two respects the shape is no closer to DXC's than loops were before
//! this test existed. The block the stopped lanes leave through is one for
//! the whole program: every latch and every such test branches to it, so
//! from a loop inside another the inner loop's test (as before it an inner
//! latch with an unconditional back edge) is a way out of the outer loop
//! that is not its latch. And the test branches on the latch's condition
//! as it was lowered, as a `break` out of a loop does. Only a negation at
//! the top of the condition is turned into a branch the other way round;
//! the value itself can be an `xor` (a lane compare of 64 bits for
//! inequality is one), which the latch only ever tested folded with the
//! limits. None of this has run on a GPU from this file's tests on a host
//! without one: the Windows runs below are what shows a driver takes it,
//! the second compare and the second phi at a header included.
//!
//! The DXIL validator runs on Windows only. The tests here lower small
//! hand-assembled programs and check, walk and run the function the module
//! writes (`bitcode`'s `Flow`) on any host; on Windows they also validate
//! each module and run the compute programs on the hardware adapter and on
//! WARP.
//!
//! [`LOOP_LIMIT`]: super::LOOP_LIMIT
//! [`LOOP_TOTAL_LIMIT`]: super::LOOP_TOTAL_LIMIT

use std::collections::HashMap;

use super::Lower;
use crate::emit::bitcode::{Term, V};

/// A count: its phi and the phi's incoming (value, DXIL block) pairs.
pub(super) struct Counter {
    pub(super) phi: V,
    pub(super) incoming: Vec<(V, usize)>,
}

/// The loops that begin at one header.
pub(super) struct Header {
    /// Which loop each back edge to the header belongs to, by the IR block
    /// the edge comes from: an index into `counts`.
    pub(super) of: HashMap<usize, usize>,
    /// Each loop's iteration count, the innermost loop's first.
    pub(super) counts: Vec<Counter>,
}

impl Lower<'_> {
    /// Finds the loops and makes the phis of their counts and of the
    /// lane's count of all its back edges (the module's documentation says
    /// where and why). `order` is the reachable IR blocks in reverse
    /// postorder, each with its DXIL block. A program without a back edge
    /// gets nothing.
    pub(super) fn count_loops(&mut self, order: &[usize]) {
        let p = self.x.p;
        let ty = self.s.t.i32;
        // A block's reachable predecessors, and of those the sources of
        // back edges to it.
        let preds = |l: &Self, b: usize| -> Vec<usize> {
            let preds = p.blocks[b].preds.iter().copied();
            preds.filter(|&q| l.block[q].is_some()).collect()
        };
        // The rule for what a back edge and a loop are is `ir`'s, which the
        // region analysis shares.
        let latches = |l: &Self, b: usize| crate::ir::latches(&p.blocks, &l.rpo, b);
        // The blocks a back edge can still follow: only there is the
        // lane's count read.
        let mut read = vec![false; p.blocks.len()];
        let mut stack: Vec<usize> = order.iter().flat_map(|&b| latches(self, b)).collect();
        while let Some(b) = stack.pop() {
            if !std::mem::replace(&mut read[b], true) {
                stack.extend(preds(self, b));
            }
        }
        for &b in order {
            let at = self.block[b].expect("reachable");
            let from = latches(self, b);
            let phi = if !from.is_empty() {
                let of = self.header_loops(b, &from);
                let loops = of.values().max().map_or(0, |n| n + 1);
                let counts = (0..loops).map(|_| Counter {
                    phi: self.s.m.phi(at, ty),
                    incoming: Vec::new(),
                });
                let counts = counts.collect();
                self.counters.insert(b, Header { of, counts });
                true
            } else if read[b] {
                // Forward edges only, from blocks placed before this one.
                let from = preds(self, b);
                let held: Vec<Option<V>> = from.iter().map(|&q| self.total[q]).collect();
                let first = held.first().copied().flatten();
                self.total[b] = first;
                // An edge on which a pixel's latch leaves its loop brings
                // a count of its own (`Lower::leave_stopped`).
                held.iter().any(|&other| other != first)
                    || from.iter().any(|&q| self.leaves_stopped(q, b))
            } else {
                false
            };
            if phi {
                let phi = self.s.m.phi(at, ty);
                self.total[b] = Some(phi);
                let incoming = Vec::new();
                self.totals.insert(b, Counter { phi, incoming });
            }
        }
    }

    /// The loops that begin at header `h`, whose back edges come from the
    /// IR blocks `from`: for each of those blocks the place of its back
    /// edge, which is the loop it counts for, the innermost loop 0. The
    /// rule, and why it is the rule, is at [`crate::ir::back_edge_places`].
    fn header_loops(&self, h: usize, from: &[usize]) -> HashMap<usize, usize> {
        crate::ir::back_edge_places(&self.x.p.blocks, &self.rpo, h, from)
    }

    /// The block latch `b`'s conditional branch goes to in place of `via`,
    /// the block of its edge out of the loop to IR block `to`. The latch
    /// leaves the loop both for the lanes whose own condition says so
    /// (`cond` is `own` in them) and for the lanes a limit stopped (`cond`
    /// is the opposite: they would have gone round again). This block tells
    /// them apart once the loop is left: the stopped lanes go to the abort
    /// block, the others on to `via`. A lane whose own condition ends the
    /// loop on the last iteration the limits allow is one of the others.
    /// (A pixel program's stopped lanes are discarded in this block and go
    /// on to `via` too: [`Lower::leave_stopped`].)
    ///
    /// It tests the latch's own condition and nothing new, as the branch
    /// of an `if` after the loop would (whatever instruction `cond` is: the
    /// module's documentation says what that can be); the loop keeps its
    /// latch, its one conditional branch and an exit of its own, and `via`,
    /// where it is an edge block, stays the next loop's preheader. Where
    /// `via` is `to`'s own block, this block is what `to`'s phis see the
    /// edge come from.
    ///
    /// A latch whose other side is a back edge too (a loop laid out before
    /// the header of the loop around it, which it falls into) leaves one
    /// loop to go round the other: that back edge is counted in a block of
    /// its own, which is then the outer loop's latch for this edge.
    pub(super) fn limit_exit(
        &mut self,
        b: usize,
        to: usize,
        mut via: usize,
        cond: V,
        own: bool,
    ) -> usize {
        // A pixel's stopped lanes go on along the edge as helpers.
        if self.s.pixel {
            return self.leave_stopped(b, to, via, cond, own);
        }
        let abort = self.abort_block();
        if self.counters.contains_key(&to) && self.rpo[to] <= self.rpo[b] {
            let here = std::mem::replace(&mut self.cur, self.s.m.new_block());
            let ok = self.count_iteration(b, to, true);
            self.s.m.terminate(self.cur, Term::CondBr(ok, via, abort));
            via = std::mem::replace(&mut self.cur, here);
            self.edges.insert((b, to), via);
        }
        let test = self.s.m.new_block();
        let term = if own {
            Term::CondBr(cond, via, abort)
        } else {
            Term::CondBr(cond, abort, via)
        };
        self.s.m.terminate(test, term);
        if Some(via) == self.block[to] {
            self.edges.insert((b, to), test);
        }
        test
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::tests::{lower_valu, lower_valu_limited};
    use super::super::{LOOP_LIMIT, LOOP_TOTAL_LIMIT, Limits, Lowered};
    use crate::emit::bitcode::{Arg, Bin, Flow, FlowOp, FlowTerm, pred};

    /// The limits a program is lowered with where a test gives it none.
    /// The inputs of those tests are chosen against the default ones, so
    /// with the `diagnostics` feature they need `VICTORY_SHADER_LOOP_LIMIT`
    /// and `VICTORY_SHADER_LOOP_TOTAL_LIMIT` out of the environment.
    pub(in crate::emit::lower) fn defaults() -> Limits {
        let set = Limits {
            each: crate::emit::loop_limit(),
            total: crate::emit::loop_total_limit(),
        };
        let default = Limits {
            each: LOOP_LIMIT,
            total: LOOP_TOTAL_LIMIT,
        };
        assert_eq!(
            set, default,
            "these tests need the default loop limits: unset VICTORY_SHADER_LOOP_LIMIT and \
             VICTORY_SHADER_LOOP_TOTAL_LIMIT"
        );
        set
    }

    /// The steps within which every lane of `l`, lowered with `limits`,
    /// ends, whatever its inputs: the interpreter counts one step a block
    /// entered and one an instruction run. A lane takes fewer than
    /// `limits.total` back edges, and from one to the next (as from the
    /// entry to the first and from the last to the end) it enters no block
    /// twice, since every cycle of the function goes through a back edge.
    /// Every run of these tests has this budget and no more, so each of
    /// them shows its lane ending within it.
    pub(in crate::emit::lower) fn budget(l: &Lowered, limits: Limits) -> u64 {
        let size: usize = l.flow.blocks.iter().map(|b| 1 + b.insts.len()).sum();
        u64::from(limits.total) * size as u64
    }

    /// One lane of a program [`lower_valu`] lowered with `limits`, run on
    /// this host with `constants` in s0..s3: the dword it stores from v0,
    /// or `None` where it ends without storing.
    fn stored(l: &Lowered, limits: Limits, constants: [u32; 4]) -> Option<u32> {
        let mut stored = None;
        let mut call = |name: &str, args: &[u64]| match name {
            // The root constants are the user data's first register.
            "dx.op.cbufferLoadLegacy.i32" if args[2] == 0 => constants.map(u64::from).to_vec(),
            "dx.op.cbufferLoadLegacy.i32" => vec![0; 4],
            "dx.op.rawBufferStore.i32" => {
                assert_eq!(stored.replace(args[4] as u32), None, "one store");
                Vec::new()
            }
            _ => vec![0],
        };
        let ran = l.flow.run(budget(l, limits), &mut call);
        assert_eq!(ran, Ok(()), "{constants:x?} with {limits:?}");
        stored
    }

    /// Runs one lane of `body`, which [`lower_valu`] lowered to `l` with
    /// the default limits, for each case: its root constants and what it
    /// stores ([`stored`]). On Windows the same lanes then run on the
    /// hardware adapter and on WARP, where the buffer is zeroed before
    /// each: a lane that stores nothing leaves 0 there.
    fn runs(l: &Lowered, body: &[u32], cases: &[([u32; 4], Option<u32>)]) {
        for &(constants, want) in cases {
            assert_eq!(stored(l, defaults(), constants), want, "{constants:x?}");
        }
        #[cfg(windows)]
        {
            use crate::emit::valu_tests::{build_valu_cs, run_cases};
            let constants: Vec<[u32; 4]> = cases.iter().map(|c| c.0).collect();
            run_cases(
                &build_valu_cs(body),
                &constants,
                |host, n, constants, got| {
                    assert_eq!(got, cases[n].1.unwrap_or(0), "{host}: {constants:x?}");
                },
            );
        }
        #[cfg(not(windows))]
        let _ = body;
    }

    /// How a latch's branch folds the limits into its condition.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Folded {
        /// Round again where the condition holds and the counts are below
        /// their limits.
        And,
        /// Out where the condition holds or a count is at its limit.
        Or,
    }

    /// Whether `v`, read by the branch of block `at`, is what a back edge
    /// is counted with, and if so the phis of the two counts: the loop's
    /// count plus one compared with `limits.each` by `p`, and the lane's
    /// count plus one compared with `limits.total` by `p`, joined by an
    /// `and` (`p` is below) or an `or` (`p` is at or above), all of it
    /// computed in `at`.
    fn counted(f: &Flow, limits: Limits, at: usize, v: Arg, p: u8) -> Option<(u32, u32)> {
        let here = |v: Arg| match v {
            Arg::Inst(id) => f.inst(id).filter(|x| x.0 == at).map(|x| &x.2.op),
            _ => None,
        };
        // A count plus one compared with `limit`: the count's phi.
        let count = |v: Arg, limit: u32| {
            let &FlowOp::Cmp(q, 32, next, Arg::Int(x)) = here(v)? else {
                return None;
            };
            let &FlowOp::Bin(Bin::Add, Arg::Inst(phi), Arg::Int(1)) = here(next)? else {
                return None;
            };
            let is_phi = matches!(f.inst(phi)?.2.op, FlowOp::Phi(_));
            (q == p && x == u64::from(limit) && is_phi).then_some(phi)
        };
        let &FlowOp::Bin(join, own, all) = here(v)? else {
            return None;
        };
        let joins = if p == pred::IULT { Bin::And } else { Bin::Or };
        let counts = (count(own, limits.each)?, count(all, limits.total)?);
        (join == joins && matches!(p, pred::IULT | pred::IUGE)).then_some(counts)
    }

    /// A back edge of a written function.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(in crate::emit::lower) struct Back {
        /// The block whose branch takes it, and the header it goes to.
        pub from: usize,
        pub to: usize,
        /// The phi of its loop's count and the phi the lane's count is
        /// read from there.
        pub count: u32,
        pub total: u32,
    }

    /// The back edges of the function `l` has, by the branches that count
    /// one ([`counted`]). The phi of each one's loop is in its header, and
    /// the count the edge brings there is the phi's plus one.
    pub(in crate::emit::lower) fn back_edges(l: &Lowered, limits: Limits) -> Vec<Back> {
        let f = &l.flow;
        let mut found = Vec::new();
        for from in 0..f.blocks.len() {
            let FlowTerm::CondBr(cond, yes, no) = f.blocks[from].term else {
                continue;
            };
            let folded = match cond {
                Arg::Inst(id) => f.inst(id).map(|x| &x.2.op),
                _ => None,
            };
            let below = |v| counted(f, limits, from, v, pred::IULT);
            let (to, counts) = if let Some(counts) = below(cond) {
                (yes, counts)
            } else if let Some(&FlowOp::Bin(Bin::And, _, ok)) = folded
                && let Some(counts) = below(ok)
            {
                (yes, counts)
            } else if let Some(&FlowOp::Bin(Bin::Or, _, stop)) = folded
                && let Some(counts) = counted(f, limits, from, stop, pred::IUGE)
            {
                (no, counts)
            } else {
                continue;
            };
            let (count, total) = counts;
            let (header, _, phi) = f.inst(count).unwrap();
            assert_eq!(header, to, "block {from}: the count is its header's");
            let FlowOp::Phi(incoming) = &phi.op else {
                unreachable!()
            };
            let brought = incoming.iter().find(|x| x.1 == from).unwrap().0;
            let Arg::Inst(next) = brought else {
                panic!("block {from} brings {brought:?} to its loop's count");
            };
            assert!(
                matches!(f.inst(next).unwrap().2.op,
                    FlowOp::Bin(Bin::Add, Arg::Inst(x), Arg::Int(1)) if x == count),
                "block {from}: its loop's count goes up by one"
            );
            found.push(Back {
                from,
                to,
                count,
                total,
            });
        }
        found
    }

    /// Checks the lowered function ([`Flow::check`]; on Windows the DXIL
    /// validator, where `dxil.dll` is there, checks the module too) and
    /// every way into the block a lane a limit stopped leaves through.
    /// Nothing follows that block. A latch with an unconditional back edge
    /// reaches it directly, on the two counts ([`counted`]). A latch with a
    /// conditional one keeps its two-way branch on the condition and the
    /// counts folded together, and leaves the loop to a block of its own
    /// that tests the condition alone: the lanes that would have gone round
    /// again go to the stop, the others on to where `after` (the block of
    /// the program's store) is still reached. Returns each such latch's
    /// block and how its branch is folded.
    fn limit_stops(l: &Lowered, limits: Limits, after: usize) -> Vec<(usize, Folded)> {
        let f = &l.flow;
        f.check().unwrap();
        #[cfg(windows)]
        if let Ok(mut v) = crate::emit::validator::Validator::load(
            crate::emit::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut l.dxil.clone())
                .expect("DXIL validation failed");
        }
        let stop = l.limit_stop.expect("a loop that can stop a lane");
        assert_eq!(f.blocks[stop].term, FlowTerm::Ret);
        assert!(
            f.calls(stop).iter().all(|&c| c == "dx.op.discard"),
            "{:?}",
            f.calls(stop)
        );
        let mut latches = Vec::new();
        let mut into = f.preds(stop);
        into.dedup();
        for t in into {
            let FlowTerm::CondBr(cond, yes, no) = f.blocks[t].term else {
                panic!("block {t} goes to the stop unconditionally");
            };
            // A counted unconditional back edge: on below the limits.
            if counted(f, limits, t, cond, pred::IULT).is_some() {
                assert_eq!(no, stop);
                assert!(f.reaches(yes, t, &[]), "block {t} is no latch");
                continue;
            }
            let [latch] = f.preds(t)[..] else {
                panic!(
                    "block {t}, a loop's exit, has predecessors {:?}",
                    f.preds(t)
                );
            };
            let FlowTerm::CondBr(Arg::Inst(both), round, out) = f.blocks[latch].term else {
                panic!("latch {latch} does not branch on a computed condition");
            };
            let (at, _, both) = f.inst(both).unwrap();
            assert_eq!(at, latch, "the folded condition is the latch's");
            let counts = |v, p| counted(f, limits, latch, v, p).is_some();
            let (folded, on) = match both.op {
                FlowOp::Bin(Bin::And, c, ok) if c == cond && counts(ok, pred::IULT) => {
                    assert_eq!((out, yes), (t, stop), "latch {latch}");
                    assert!(
                        f.reaches(round, latch, &[t]),
                        "latch {latch} has no back edge"
                    );
                    (Folded::And, no)
                }
                FlowOp::Bin(Bin::Or, c, at_limit) if c == cond && counts(at_limit, pred::IUGE) => {
                    assert_eq!((round, no), (t, stop), "latch {latch}");
                    assert!(
                        f.reaches(out, latch, &[t]),
                        "latch {latch} has no back edge"
                    );
                    (Folded::Or, yes)
                }
                ref op => panic!("latch {latch} branches on {op:?}"),
            };
            assert_ne!(on, stop);
            assert!(
                f.reaches(on, after, &[stop]),
                "latch {latch}: the loop's own exit"
            );
            latches.push((latch, folded));
        }
        latches
    }

    /// The block of the store [`lower_valu`] ends its programs with.
    fn store_block(l: &Lowered) -> usize {
        let blocks = 0..l.flow.blocks.len();
        let mut stores = blocks.filter(|&b| l.flow.calls(b).contains(&"dx.op.rawBufferStore.i32"));
        let store = stores.next().expect("a store");
        assert_eq!(stores.next(), None, "one store");
        store
    }

    /// Audit finding A-C4. `do { s12 += 1 } while (s12 != s0)`, then the
    /// store of s12, as `body` lays it out. A lane whose own condition ends
    /// the loop stores its count, also where that is on the very iteration
    /// the limit allows last; a lane that would go round once more is
    /// stopped and stores nothing (before, it left through the loop's exit
    /// and stored the count the limit cut short).
    fn counts_to_s0(body: &[u32], folded: Folded) {
        let l = lower_valu(body).unwrap();
        let store = store_block(&l);
        let [(latch, how)] = limit_stops(&l, defaults(), store)[..] else {
            panic!("one latch: {:?}", limit_stops(&l, defaults(), store));
        };
        assert_eq!(how, folded);
        // The loop's one way out is through the test of the stopped lanes.
        let stop = l.limit_stop.unwrap();
        let [test] = l.flow.preds(stop)[..] else {
            panic!("into the stop: {:?}", l.flow.preds(stop));
        };
        assert!(
            !l.flow.reaches(latch, store, &[test]),
            "a way round block {test}"
        );
        // One loop: one count, and the lane's beside it in the header.
        let [back] = back_edges(&l, defaults())[..] else {
            panic!("one back edge: {:?}", back_edges(&l, defaults()));
        };
        assert_eq!(
            (back.from, l.flow.inst(back.total).unwrap().0),
            (latch, back.to)
        );
        let limit = defaults().each;
        let cases = [
            ([1, 0, 0, 0], Some(1)),
            ([5, 0, 0, 0], Some(5)),
            // The last iteration the limit allows, and one more.
            ([limit, 0, 0, 0], Some(limit)),
            ([limit + 1, 0, 0, 0], None),
            // A count from a view that reads zeros: four billion iterations.
            ([0, 0, 0, 0], None),
        ];
        runs(&l, body, &cases);
    }

    /// The loop of [`counts_to_s0`] with the back edge the taken side of
    /// the latch's branch.
    fn back_edge_taken(cmp: u32, branch: u32) -> [u32; 5] {
        [
            0xBE8C_0380,     // s_mov_b32 s12, 0
            0x810C_810C,     // loop: s_add_i32 s12, s12, 1
            cmp | 0x000C,    // s_cmp_{lg,eq}_u32 s12, s0
            branch | 0xFFFD, // s_cbranch_scc{1,0} loop
            0x7E00_020C,     // v_mov_b32 v0, s12
        ]
    }

    /// The loop of [`counts_to_s0`] with the latch before its header: it
    /// falls through into the loop and branches out of it.
    fn back_edge_fallen_into(cmp: u32, branch: u32) -> [u32; 7] {
        [
            0xBE8C_0380,     // s_mov_b32 s12, 0
            0xBF82_0002,     // s_branch header
            cmp | 0x000C,    // latch: s_cmp_{eq,lg}_u32 s12, s0
            branch | 0x0002, // s_cbranch_scc{1,0} out
            0x810C_810C,     // header: s_add_i32 s12, s12, 1
            0xBF82_FFFC,     // s_branch latch
            0x7E00_020C,     // out: v_mov_b32 v0, s12
        ]
    }

    const S_CMP_EQ_U32: u32 = 0xBF06_0000;
    const S_CMP_LG_U32: u32 = 0xBF07_0000;
    const S_CBRANCH_SCC0: u32 = 0xBF84_0000;
    const S_CBRANCH_SCC1: u32 = 0xBF85_0000;

    #[test]
    fn a_loop_limit_stop_on_a_back_edge_taken_while_scc() {
        counts_to_s0(&back_edge_taken(S_CMP_LG_U32, S_CBRANCH_SCC1), Folded::And);
    }

    #[test]
    fn a_loop_limit_stop_on_a_back_edge_taken_while_not_scc() {
        counts_to_s0(&back_edge_taken(S_CMP_EQ_U32, S_CBRANCH_SCC0), Folded::Or);
    }

    #[test]
    fn a_loop_limit_stop_on_a_back_edge_fallen_into_unless_scc() {
        counts_to_s0(
            &back_edge_fallen_into(S_CMP_EQ_U32, S_CBRANCH_SCC1),
            Folded::Or,
        );
    }

    #[test]
    fn a_loop_limit_stop_on_a_back_edge_fallen_into_unless_not_scc() {
        counts_to_s0(
            &back_edge_fallen_into(S_CMP_LG_U32, S_CBRANCH_SCC0),
            Folded::And,
        );
    }

    /// The loop's exit joins a branch round the loop: the test of the
    /// stopped lanes comes before the block that splits the edge, which
    /// the join's phi names. With the latch's branch on either sense.
    #[test]
    fn a_loop_limit_stop_before_a_join() {
        for (cmp, branch, folded) in [
            (S_CMP_LG_U32, S_CBRANCH_SCC1, Folded::And),
            (S_CMP_EQ_U32, S_CBRANCH_SCC0, Folded::Or),
        ] {
            let body = [
                0xBE8C_0380,     // s_mov_b32 s12, 0
                0xBF06_8001,     // s_cmp_eq_u32 s1, 0
                0xBF85_0003,     // s_cbranch_scc1 out
                0x810C_810C,     // loop: s_add_i32 s12, s12, 1
                cmp | 0x000C,    // s_cmp_{lg,eq}_u32 s12, s0
                branch | 0xFFFD, // s_cbranch_scc{1,0} loop
                0x7E00_020C,     // out: v_mov_b32 v0, s12
            ];
            let l = lower_valu(&body).unwrap();
            let store = store_block(&l);
            let latches = limit_stops(&l, defaults(), store);
            assert_eq!(latches.iter().map(|x| x.1).collect::<Vec<_>>(), [folded]);
            // The join: from the branch round the loop and from the block
            // splitting the loop's exit, which the test alone leads to.
            let f = &l.flow;
            let [test] = f.preds(l.limit_stop.unwrap())[..] else {
                panic!("into the stop: {:?}", f.preds(l.limit_stop.unwrap()));
            };
            let into = f.preds(store);
            assert_eq!(into.len(), 2, "{into:?}");
            assert!(!into.contains(&test) && into.iter().any(|&b| f.preds(b) == [test]));
            let cases = [
                ([0, 0, 0, 0], Some(0)),
                ([7, 1, 0, 0], Some(7)),
                ([0, 1, 0, 0], None),
            ];
            runs(&l, &body, &cases);
        }
    }

    /// One lane as the limits are documented, for the models of the
    /// programs below: what each stores, written out from its loops with
    /// no part of the lowering.
    struct Lane {
        limits: Limits,
        /// The back edges it has taken, the one being asked about included.
        taken: u32,
    }

    impl Lane {
        fn new(limits: Limits) -> Self {
            Lane { limits, taken: 0 }
        }

        /// Whether the lane goes round the loop whose count is `count` once
        /// more: that count and the lane's go up, and both must stay below
        /// their limits. A lane that may not is stopped.
        fn round(&mut self, count: &mut u32) -> bool {
            *count += 1;
            self.taken += 1;
            *count < self.limits.each && self.taken < self.limits.total
        }
    }

    /// What a program's lane stores, by its model: `None` where a limit
    /// stops it.
    pub(in crate::emit::lower) type Model = fn(Limits, [u32; 4]) -> Option<u32>;

    /// `do { s12 += 1 } while (s12 != s0); do { s13 += 1 } while (s13 !=
    /// s1)`, then the store of s12 + s13: the first loop's exit is the
    /// second loop's preheader.
    pub(in crate::emit::lower) const TWO_LOOPS: [u32; 10] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBE8D_0380, // s_mov_b32 s13, 0
        0x810C_810C, // first: s_add_i32 s12, s12, 1
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFD, // s_cbranch_scc1 first
        0x810D_810D, // second: s_add_i32 s13, s13, 1
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFFD, // s_cbranch_scc1 second
        0x810C_0D0C, // s_add_i32 s12, s12, s13
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    fn two_loops(limits: Limits, [s0, s1, ..]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s12, mut s13) = (0u32, 0u32);
        let (mut first, mut second) = (0, 0);
        loop {
            s12 = s12.wrapping_add(1);
            if s12 == s0 {
                break;
            }
            lane.round(&mut first).then_some(())?;
        }
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                break;
            }
            lane.round(&mut second).then_some(())?;
        }
        Some(s12.wrapping_add(s13))
    }

    /// The loop's exit is the next loop's header: the test of the stopped
    /// lanes comes before that loop's preheader, where its count starts
    /// again at 0 and the lane's goes on from what the first loop left.
    #[test]
    fn a_loop_limit_stop_before_the_next_loop() {
        let l = lower_valu(&TWO_LOOPS).unwrap();
        assert_eq!(limit_stops(&l, defaults(), store_block(&l)).len(), 2);
        // Each loop has its count; the lane's is a phi in each header, and
        // the second takes the first's along the forward edge.
        let [first, second] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        assert_ne!(first.count, second.count);
        let FlowOp::Phi(incoming) = &l.flow.inst(second.total).unwrap().2.op else {
            unreachable!()
        };
        assert!(incoming.iter().any(|x| x.0 == Arg::Inst(first.total)));
        let limit = defaults().each;
        let cases = [
            ([3, 4, 0, 0], Some(7)),
            // Each loop has its count.
            ([limit, limit, 0, 0], Some(2 * limit)),
            // The first loop stops the lane; the second one.
            ([0, 4, 0, 0], None),
            ([3, 0, 0, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(two_loops(defaults(), constants), want, "{constants:x?}");
        }
        runs(&l, &TWO_LOOPS, &cases);
    }

    /// `do { s12 = 0; do { s12 += 1 } while (s12 != s0); s13 += 1 } while
    /// (s13 != s1)`, then the store of s12 + s13: a loop in a loop, each
    /// with a header of its own.
    pub(in crate::emit::lower) const NEST: [u32; 10] = [
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBE8C_0380, // outer: s_mov_b32 s12, 0
        0x810C_810C, // inner: s_add_i32 s12, s12, 1
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFD, // s_cbranch_scc1 inner
        0x810D_810D, // s_add_i32 s13, s13, 1
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFF9, // s_cbranch_scc1 outer
        0x810C_0D0C, // s_add_i32 s12, s12, s13
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    fn nest(limits: Limits, [s0, s1, ..]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s13, mut outer) = (0u32, 0);
        let mut s12: u32;
        loop {
            s12 = 0;
            let mut inner = 0;
            loop {
                s12 = s12.wrapping_add(1);
                if s12 == s0 {
                    break;
                }
                lane.round(&mut inner).then_some(())?;
            }
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                break;
            }
            lane.round(&mut outer).then_some(())?;
        }
        Some(s12.wrapping_add(s13))
    }

    /// A loop in a loop with headers of their own: each has its count. A
    /// lane the inner loop's limit stops leaves the outer loop with it, the
    /// inner count starts again each time the outer loop enters it, and
    /// each loop runs to the last iteration its own limit allows.
    #[test]
    fn a_loop_limit_stop_in_a_nested_loop() {
        let l = lower_valu(&NEST).unwrap();
        assert_eq!(limit_stops(&l, defaults(), store_block(&l)).len(), 2);
        let [inner, outer] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        assert!(inner.to != outer.to && inner.count != outer.count);
        // The inner count starts at 0 wherever the lane enters: from the
        // outer header, which is all the outer loop's back edge leads to.
        let f = &l.flow;
        let FlowOp::Phi(incoming) = &f.inst(inner.count).unwrap().2.op else {
            unreachable!()
        };
        let entries: Vec<_> = incoming.iter().filter(|x| x.1 != inner.from).collect();
        assert_eq!(entries, [&(Arg::Int(0), outer.to)]);
        let limit = defaults().each;
        let cases = [
            ([3, 4, 0, 0], Some(7)),
            // The inner count starts again with each entry.
            ([limit, 2, 0, 0], Some(limit + 2)),
            // The inner loop stops the lane; the outer one.
            ([0, 4, 0, 0], None),
            ([1, 0, 0, 0], None),
            // The outer loop's last iteration.
            ([1, limit, 0, 0], Some(limit + 1)),
        ];
        for (constants, want) in cases {
            assert_eq!(nest(defaults(), constants), want, "{constants:x?}");
        }
        runs(&l, &NEST, &cases);
    }

    /// `do { s12 += 1; if (s12 < s1) continue; } while (s12 != s0)`, then
    /// the store of s12, with the `continue` a conditional branch back in
    /// the header's own block: the graph of a loop of one block inside a
    /// loop that begins with it.
    const CONTINUE: [u32; 7] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0x810C_810C, // loop: s_add_i32 s12, s12, 1
        0xBF0A_010C, // s_cmp_lt_u32 s12, s1
        0xBF85_FFFD, // s_cbranch_scc1 loop
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFB, // s_cbranch_scc1 loop
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    fn continues(limits: Limits, [s0, s1, ..]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s12, mut inner, mut outer) = (0u32, 0, 0);
        loop {
            s12 = s12.wrapping_add(1);
            if s12 < s1 {
                lane.round(&mut inner).then_some(())?;
                continue;
            }
            if s12 == s0 {
                break;
            }
            lane.round(&mut outer).then_some(())?;
            inner = 0;
        }
        Some(s12)
    }

    /// Two conditional back edges to one header, the first of which falls
    /// through towards the second: each latch has its own test, and the
    /// first one's other side stays in the loop. By the graph this is a
    /// loop of one block in a loop that begins with it, so each back edge
    /// has its own count and the first one's starts again when the second
    /// is taken. (With one count for the header both counted the same
    /// iterations, and `[limit + 8, 9]` below was stopped.)
    #[test]
    fn a_loop_limit_counts_a_continue_that_falls_through_as_a_loop() {
        let l = lower_valu(&CONTINUE).unwrap();
        assert_eq!(limit_stops(&l, defaults(), store_block(&l)).len(), 2);
        let [first, second] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        assert!(first.to == second.to && first.count != second.count);
        let limit = defaults().each;
        let cases = [
            ([6, 3, 0, 0], Some(6)),
            // The last iteration the second count allows, after 8 of the
            // first; and one more.
            ([limit + 8, 9, 0, 0], Some(limit + 8)),
            ([limit + 9, 9, 0, 0], None),
            // The same for the first count, and the second goes on after.
            ([limit + 5, limit, 0, 0], Some(limit + 5)),
            ([limit + 5, limit + 1, 0, 0], None),
            ([0, u32::MAX, 0, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(continues(defaults(), constants), want, "{constants:x?}");
        }
        runs(&l, &CONTINUE, &cases);
    }

    /// `do { s12 += 1; if (s12 < s1) { if (s12 != s2) continue; break; } }
    /// while (s12 != s0)`, then the store of s12. The `continue` and the
    /// loop's end are in different arms of a branch: neither back edge is
    /// reached from the other without passing the header.
    const TWO_LATCHES: [u32; 10] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0x810C_810C, // loop: s_add_i32 s12, s12, 1
        0xBF0A_010C, // s_cmp_lt_u32 s12, s1
        0xBF85_0003, // s_cbranch_scc1 early
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFB, // s_cbranch_scc1 loop
        0xBF82_0002, // s_branch out
        0xBF07_020C, // early: s_cmp_lg_u32 s12, s2
        0xBF85_FFF8, // s_cbranch_scc1 loop
        0x7E00_020C, // out: v_mov_b32 v0, s12
    ];

    fn two_latches(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s12, mut count) = (0u32, 0);
        loop {
            s12 = s12.wrapping_add(1);
            if s12 == if s12 < s1 { s2 } else { s0 } {
                break;
            }
            lane.round(&mut count).then_some(())?;
        }
        Some(s12)
    }

    /// Two back edges of one loop share its count: the lane is stopped
    /// after the iterations the limit allows whichever of the two it took
    /// for each, here about half of them by either.
    #[test]
    fn a_loop_limit_counts_two_back_edges_of_one_loop_together() {
        let l = lower_valu(&TWO_LATCHES).unwrap();
        assert_eq!(limit_stops(&l, defaults(), store_block(&l)).len(), 2);
        let [first, second] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        assert_ne!(first.from, second.from);
        assert_eq!((first.to, first.count), (second.to, second.count));
        let limit = defaults().each;
        let cases = [
            ([6, 3, 0, 0], Some(6)),
            // Left from the arm of the `continue`.
            ([6, 5, 4, 0], Some(4)),
            // The last iteration the limit allows, and one more.
            ([limit, limit / 2, 0, 0], Some(limit)),
            ([limit + 1, limit / 2, 0, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(two_latches(defaults(), constants), want, "{constants:x?}");
        }
        runs(&l, &TWO_LATCHES, &cases);
    }

    /// `for (;;) { s13 += 1; s12 = 0; if (s13 == s1) break; while (s12 !=
    /// s0) s12 += 1; }`, then the store of s13, laid out with the inner
    /// loop's test before the outer loop's code.
    pub(in crate::emit::lower) const LATCH_IS_INNER_HEADER: [u32; 10] = [
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBF82_0003, // s_branch outer
        0x810C_810C, // body: s_add_i32 s12, s12, 1
        0xBF07_000C, // inner: s_cmp_lg_u32 s12, s0
        0xBF85_FFFD, // s_cbranch_scc1 body
        0x810D_810D, // outer: s_add_i32 s13, s13, 1
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFFA, // s_cbranch_scc1 inner
        0x7E00_020D, // v_mov_b32 v0, s13
    ];

    fn latch_is_inner_header(limits: Limits, [s0, s1, ..]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s13, mut outer) = (0u32, 0);
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                break;
            }
            let (mut s12, mut inner) = (0u32, 0);
            while s12 != s0 {
                s12 = s12.wrapping_add(1);
                lane.round(&mut inner).then_some(())?;
            }
            lane.round(&mut outer).then_some(())?;
        }
        Some(s13)
    }

    /// A `while` loop in a loop that ends with it: the inner header is the
    /// outer loop's latch, whose other side goes on into the inner loop's
    /// body. The stopped lanes are told from the others on that edge, and
    /// the inner back edge, an unconditional one, stops its own.
    #[test]
    fn a_loop_limit_stop_where_the_latch_is_an_inner_header() {
        let body = LATCH_IS_INNER_HEADER;
        let l = lower_valu(&body).unwrap();
        let latches = limit_stops(&l, defaults(), store_block(&l));
        assert_eq!(
            latches.iter().map(|x| x.1).collect::<Vec<_>>(),
            [Folded::Or]
        );
        assert_eq!(
            l.flow.preds(l.limit_stop.unwrap()).len(),
            2,
            "both latches stop lanes"
        );
        assert_eq!(back_edges(&l, defaults()).len(), 2);
        let limit = defaults().each;
        let cases = [
            ([3, 4, 0, 0], Some(4)),
            // The inner count starts again with each entry.
            ([limit - 1, 2, 0, 0], Some(2)),
            // The inner loop stops the lane; the outer one.
            ([limit, 2, 0, 0], None),
            ([0, 0, 0, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(
                latch_is_inner_header(defaults(), constants),
                want,
                "{constants:x?}"
            );
        }
        runs(&l, &body, &cases);
    }

    /// `for (;;) { s13 += 1; s12 = 0; if (s13 == s1) break; do { s12 += 1 }
    /// while (s12 != s0); }`, then the store of s13, laid out with the
    /// inner loop before the outer loop's code.
    pub(in crate::emit::lower) const BOTH_SIDES_BACK: [u32; 10] = [
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBF82_0003, // s_branch outer
        0x810C_810C, // inner: s_add_i32 s12, s12, 1
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFD, // s_cbranch_scc1 inner
        0x810D_810D, // outer: s_add_i32 s13, s13, 1
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFF9, // s_cbranch_scc1 inner
        0x7E00_020D, // v_mov_b32 v0, s13
    ];

    fn both_sides_back(limits: Limits, [s0, s1, ..]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s13, mut outer) = (0u32, 0);
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                break;
            }
            let (mut s12, mut inner) = (0u32, 0);
            loop {
                s12 = s12.wrapping_add(1);
                if s12 == s0 {
                    break;
                }
                lane.round(&mut inner).then_some(())?;
            }
            lane.round(&mut outer).then_some(())?;
        }
        Some(s13)
    }

    /// An inner loop laid out before the header of the loop around it: its
    /// latch branches back to itself and falls into the outer header, a
    /// back edge on both sides. The inner back edge is the latch's; the
    /// outer one is counted in a block of its own after the test of the
    /// lanes the inner limit stopped.
    #[test]
    fn a_loop_limit_stop_where_both_sides_are_back_edges() {
        let body = BOTH_SIDES_BACK;
        let l = lower_valu(&body).unwrap();
        let store = store_block(&l);
        let [(inner, Folded::And)] = limit_stops(&l, defaults(), store)[..] else {
            panic!(
                "the inner loop's latch: {:?}",
                limit_stops(&l, defaults(), store)
            );
        };
        let f = &l.flow;
        assert!(
            f.succs(inner).contains(&inner),
            "the inner loop is its latch"
        );
        // Into the stop: the inner latch's test, then the outer loop's
        // counted latch, which that test alone leads to.
        let stop = l.limit_stop.unwrap();
        let into = f.preds(stop);
        let [test] = f
            .succs(inner)
            .into_iter()
            .filter(|&s| s != inner)
            .collect::<Vec<_>>()[..]
        else {
            panic!("out of the inner latch: {:?}", f.succs(inner));
        };
        let [counted] = into
            .iter()
            .copied()
            .filter(|&b| b != test)
            .collect::<Vec<_>>()[..]
        else {
            panic!("into the stop: {into:?}");
        };
        assert!(into.contains(&test));
        assert_eq!(f.preds(counted), [test]);
        assert!(!f.succs(inner).contains(&counted) && !f.reaches(inner, store, &[counted]));
        // The block of its own counts the outer loop's back edge, with the
        // lane's count as the inner loop left it.
        let backs = back_edges(&l, defaults());
        let [outer] = backs
            .iter()
            .filter(|x| x.from == counted)
            .collect::<Vec<_>>()[..]
        else {
            panic!("the outer back edge: {backs:?}");
        };
        let [own] = backs.iter().filter(|x| x.from == inner).collect::<Vec<_>>()[..] else {
            panic!("the inner back edge: {backs:?}");
        };
        assert!(outer.to != inner && outer.count != own.count && outer.total == own.total);
        let limit = defaults().each;
        let cases = [
            ([3, 4, 0, 0], Some(4)),
            // The inner count starts again with each entry.
            ([limit, 3, 0, 0], Some(3)),
            // The inner loop stops the lane; the outer one.
            ([0, 4, 0, 0], None),
            ([1, 0, 0, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(
                both_sides_back(defaults(), constants),
                want,
                "{constants:x?}"
            );
        }
        runs(&l, &body, &cases);
    }

    /// `do { v0 += 1 } while (v1 > v0)` with the bound in the lane's v1,
    /// then the store of v0: a back edge each lane takes for itself
    /// (`s_cbranch_execnz` after a `v_cmpx`).
    const PER_LANE: [u32; 6] = [
        0xBE8D_037E, // s_mov_b32 s13, exec_lo
        0x7E00_0280, // v_mov_b32 v0, 0
        0x4A00_0081, // loop: v_add_nc_u32 v0, 1, v0
        0x7DA8_0101, // v_cmpx_gt_u32 exec, v1, v0
        0xBF89_FFFD, // s_cbranch_execnz loop
        0xBEFE_030D, // s_mov_b32 exec_lo, s13
    ];

    fn per_lane(limits: Limits, [s0, ..]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut v0, mut count) = (0u32, 0);
        loop {
            v0 = v0.wrapping_add(1);
            if s0 <= v0 {
                break;
            }
            lane.round(&mut count).then_some(())?;
        }
        Some(v0)
    }

    /// A back edge each lane takes for itself.
    #[test]
    fn a_loop_limit_stop_on_a_per_lane_back_edge() {
        let l = lower_valu(&PER_LANE).unwrap();
        let latches = limit_stops(&l, defaults(), store_block(&l));
        assert_eq!(
            latches.iter().map(|x| x.1).collect::<Vec<_>>(),
            [Folded::And]
        );
        let limit = defaults().each;
        let cases = [
            ([5, 0, 0, 0], Some(5)),
            // The last iteration the limit allows, and one more.
            ([limit, 0, 0, 0], Some(limit)),
            ([limit + 1, 0, 0, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(per_lane(defaults(), constants), want, "{constants:x?}");
        }
        runs(&l, &PER_LANE, &cases);
    }

    /// `do { do { v6 += 1; v10 += 1 } while (v[6:7] != v[8:9]); s14 += 1 }
    /// while (s14 != s1)` with the bound's dwords s0 and s2 in v8 and v9,
    /// then the store of v10: a per-lane loop in a uniform one, its latch a
    /// 64-bit compare for inequality.
    const PER_LANE_64: [u32; 16] = [
        0xBE8D_037E, // s_mov_b32 s13, exec_lo
        0xBE8E_0380, // s_mov_b32 s14, 0
        0x7E0E_0280, // v_mov_b32 v7, 0
        0x7E10_0301, // v_mov_b32 v8, v1
        0x7E12_0303, // v_mov_b32 v9, v3
        0x7E14_0280, // v_mov_b32 v10, 0
        0x7E0C_0280, // outer: v_mov_b32 v6, 0
        0x4A0C_0C81, // inner: v_add_nc_u32 v6, 1, v6
        0x4A14_1481, // v_add_nc_u32 v10, 1, v10
        0x7DEA_1106, // v_cmpx_ne_u64 exec, v[6:7], v[8:9]
        0xBF89_FFFC, // s_cbranch_execnz inner
        0xBEFE_030D, // s_mov_b32 exec_lo, s13
        0x810E_810E, // s_add_i32 s14, s14, 1
        0xBF07_010E, // s_cmp_lg_u32 s14, s1
        0xBF85_FFF7, // s_cbranch_scc1 outer
        0x7E00_030A, // v_mov_b32 v0, v10
    ];

    fn per_lane_64(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut v10, mut s14, mut outer) = (0u32, 0u32, 0);
        loop {
            let (mut v6, mut inner) = (0u32, 0);
            loop {
                v6 = v6.wrapping_add(1);
                v10 = v10.wrapping_add(1);
                if v6 == s0 && s2 == 0 {
                    break;
                }
                lane.round(&mut inner).then_some(())?;
            }
            s14 = s14.wrapping_add(1);
            if s14 == s1 {
                break;
            }
            lane.round(&mut outer).then_some(())?;
        }
        Some(v10)
    }

    /// A per-lane loop in a uniform one, its latch a 64-bit compare for
    /// inequality. That compare lowers to an `xor` (not both dwords equal),
    /// which the test of the stopped lanes branches on as it is, and the
    /// test, inside the outer loop, is a way out of it that is not its
    /// latch. The hardware adapter has to take both.
    #[test]
    fn a_loop_limit_stop_on_a_per_lane_back_edge_of_64_bits_in_a_loop() {
        let body = PER_LANE_64;
        let l = lower_valu(&body).unwrap();
        let store = store_block(&l);
        assert_eq!(
            limit_stops(&l, defaults(), store)
                .iter()
                .map(|x| x.1)
                .collect::<Vec<_>>(),
            [Folded::And, Folded::And]
        );
        // The inner loop's test: the one branch to the stop on an `xor`.
        let f = &l.flow;
        let stop = l.limit_stop.unwrap();
        let on_xor = |&t: &usize| {
            let FlowTerm::CondBr(Arg::Inst(cond), ..) = f.blocks[t].term else {
                return false;
            };
            matches!(f.inst(cond).unwrap().2.op, FlowOp::Bin(Bin::Xor, ..))
        };
        let [test] = f.preds(stop).into_iter().filter(on_xor).collect::<Vec<_>>()[..] else {
            panic!("into the stop: {:?}", f.preds(stop));
        };
        // From it the outer loop goes on, and the test is in that loop.
        let on = f.succs(test).into_iter().find(|&b| b != stop).unwrap();
        assert!(f.reaches(on, test, &[stop]) && f.reaches(on, store, &[test, stop]));
        let limit = defaults().each;
        let cases = [
            ([3, 4, 0, 0], Some(12)),
            // The inner count starts again with each entry.
            ([limit, 2, 0, 0], Some(2 * limit)),
            // The inner loop stops the lane, also where the high dwords
            // never meet; the outer one.
            ([limit + 1, 2, 0, 0], None),
            ([3, 4, 1, 0], None),
            ([1, 0, 0, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(per_lane_64(defaults(), constants), want, "{constants:x?}");
        }
        runs(&l, &body, &cases);
    }

    /// `do { do { s14 += 1 } while (s14 != s0); s14 = 0; s13 += 1 } while
    /// (s13 != s1)`, then the store of s13: the outer loop's body begins
    /// with the inner loop, so both back edges go to the same block.
    pub(in crate::emit::lower) const SHARED_HEADER: [u32; 10] = [
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBE8E_0380, // s_mov_b32 s14, 0
        0x810E_810E, // head: s_add_i32 s14, s14, 1
        0xBF07_000E, // s_cmp_lg_u32 s14, s0
        0xBF85_FFFD, // s_cbranch_scc1 head
        0xBE8E_0380, // s_mov_b32 s14, 0
        0x810D_810D, // s_add_i32 s13, s13, 1
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFF9, // s_cbranch_scc1 head
        0x7E00_020D, // v_mov_b32 v0, s13
    ];

    fn shared_header(limits: Limits, [s0, s1, ..]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s13, mut outer) = (0u32, 0);
        loop {
            let (mut s14, mut inner) = (0u32, 0);
            loop {
                s14 = s14.wrapping_add(1);
                if s14 == s0 {
                    break;
                }
                lane.round(&mut inner).then_some(())?;
            }
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                break;
            }
            lane.round(&mut outer).then_some(())?;
        }
        Some(s13)
    }

    /// Two loops with one header: each has its count there. The inner
    /// loop's starts again at 0 when the outer loop goes round, and the
    /// outer loop's stays while the inner one goes round, so a nest whose
    /// loops are each far below the limit runs to its end though it goes
    /// round more than 2^20 times in all; a lane is stopped where the inner
    /// loop passes its limit, or the outer one. (With one count for the
    /// header, which only the entry from outside both started at 0, the
    /// limit was on the inner iterations of all outer ones together, and
    /// the first of the long cases below was stopped.)
    #[test]
    fn a_loop_limit_counts_loops_sharing_a_header_apart() {
        let body = SHARED_HEADER;
        let l = lower_valu(&body).unwrap();
        let latches = limit_stops(&l, defaults(), store_block(&l));
        let [(inner, Folded::And), (outer, Folded::And)] = latches[..] else {
            panic!("two latches: {latches:?}");
        };
        // Both go round to the one header, the inner loop's only block,
        // which holds a count for each and the lane's.
        let f = &l.flow;
        assert_eq!((f.succs(inner)[0], f.succs(outer)[0]), (inner, inner));
        let [own, around] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        assert_eq!((own.from, around.from), (inner, outer));
        assert!(own.count != around.count && own.total == around.total);
        // Along the outer back edge the inner count takes 0 and the outer
        // one goes up; along the inner one the outer count takes itself.
        let incoming = |phi: u32, from: usize| {
            let FlowOp::Phi(incoming) = &f.inst(phi).unwrap().2.op else {
                unreachable!()
            };
            incoming.iter().find(|x| x.1 == from).unwrap().0
        };
        assert_eq!(incoming(own.count, outer), Arg::Int(0));
        assert_eq!(incoming(around.count, inner), Arg::Inst(around.count));
        let limit = defaults().each;
        assert!(2048 * 1024 > limit);
        let cases = [
            ([3, 4, 0, 0], Some(4)),
            // Twice the limit in all, with either loop far below it.
            ([2048, 1024, 0, 0], Some(1024)),
            // The inner loop's last iteration each time, and one more.
            ([limit, 2, 0, 0], Some(2)),
            ([limit + 1, 2, 0, 0], None),
            // The outer loop's last iteration, and one more.
            ([1, limit, 0, 0], Some(limit)),
            ([1, limit + 1, 0, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(shared_header(defaults(), constants), want, "{constants:x?}");
        }
        runs(&l, &body, &cases);
    }

    /// The value the phi `phi` of `f` takes along the edge from block
    /// `from`.
    fn brought(f: &Flow, phi: u32, from: usize) -> Arg {
        let FlowOp::Phi(incoming) = &f.inst(phi).unwrap().2.op else {
            panic!("instruction {phi} is no phi")
        };
        incoming.iter().find(|x| x.1 == from).unwrap().0
    }

    /// `do { do { s12 += 1; s14 += 1; if (s14 == s2) { s12 += 2; continue; }
    /// } while (s14 != s0); s14 = 0; s13 += 1 } while (s13 != s1)`, then the
    /// store of s12: [`SHARED_HEADER`]'s nest with a second back edge in
    /// the inner loop, from an arm of a branch. That edge's natural loop
    /// (the header and the arm) is neither inside nor around the inner
    /// loop's end's, and neither inside nor around the outer loop's end's.
    const SHARED_HEADER_ARM: [u32; 17] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBE8E_0380, // s_mov_b32 s14, 0
        0x810C_810C, // head: s_add_i32 s12, s12, 1
        0x810E_810E, // s_add_i32 s14, s14, 1
        0xBF06_020E, // s_cmp_eq_u32 s14, s2
        0xBF85_0007, // s_cbranch_scc1 again
        0xBF07_000E, // s_cmp_lg_u32 s14, s0
        0xBF85_FFFA, // s_cbranch_scc1 head
        0xBE8E_0380, // s_mov_b32 s14, 0
        0x810D_810D, // s_add_i32 s13, s13, 1
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFF6, // s_cbranch_scc1 head
        0xBF82_0002, // s_branch out
        0x810C_820C, // again: s_add_i32 s12, s12, 2
        0xBF82_FFF3, // s_branch head
        0x7E00_020C, // out: v_mov_b32 v0, s12
    ];

    fn shared_header_arm(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s12, mut s13, mut outer) = (0u32, 0u32, 0);
        loop {
            let (mut s14, mut inner) = (0u32, 0);
            loop {
                s12 = s12.wrapping_add(1);
                s14 = s14.wrapping_add(1);
                if s14 == s2 {
                    s12 = s12.wrapping_add(2);
                } else if s14 == s0 {
                    break;
                }
                lane.round(&mut inner).then_some(())?;
            }
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                break;
            }
            lane.round(&mut outer).then_some(())?;
        }
        Some(s12)
    }

    /// A nest with one header stays two loops where the inner loop has a
    /// second back edge in an arm. That edge has the inner loop's place
    /// (nothing is inside its natural loop) and so its count, the outer
    /// loop's end has the place above, and the inner count starts again
    /// when the outer loop goes round: the nest runs to its end though it
    /// goes round more than 2^20 times in all. (Grouped by chains of back
    /// edges of which neither is inside the other, the three were one loop
    /// with one count, and the first of the long cases below was stopped.)
    #[test]
    fn a_loop_limit_counts_a_nest_apart_where_an_arm_goes_back_too() {
        let body = SHARED_HEADER_ARM;
        let l = lower_valu(&body).unwrap();
        let store = store_block(&l);
        assert_eq!(limit_stops(&l, defaults(), store).len(), 2);
        let f = &l.flow;
        let stop = l.limit_stop.unwrap();
        let [inner, outer, arm] = back_edges(&l, defaults())[..] else {
            panic!("three back edges: {:?}", back_edges(&l, defaults()));
        };
        // The arm's back edge is the unconditional one, and the outer
        // loop's end is reached from the inner loop's alone.
        assert!(f.succs(arm.from).contains(&stop));
        assert!(f.reaches(inner.from, outer.from, &[inner.to]));
        assert!(!f.reaches(arm.from, outer.from, &[arm.to]));
        assert!(inner.to == outer.to && inner.to == arm.to);
        assert!(inner.count == arm.count && inner.count != outer.count);
        assert!(inner.total == outer.total && inner.total == arm.total);
        // The outer back edge starts the inner count again; neither of
        // the inner loop's moves the outer one.
        assert_eq!(brought(f, inner.count, outer.from), Arg::Int(0));
        for from in [inner.from, arm.from] {
            assert_eq!(brought(f, outer.count, from), Arg::Inst(outer.count));
        }
        let limit = defaults().each;
        assert!(1025 * 1024 > limit);
        let cases = [
            ([3, 4, 0, 0], Some(12)),
            ([3, 4, 2, 0], Some(20)),
            // More than the limit in all, with either loop far below it
            // and the arm taken in each round of the outer loop.
            ([1025, 1024, 7, 0], Some(1025 * 1024 + 2 * 1024)),
            // The inner loop's last iteration, one of them by the arm, and
            // one more.
            ([limit, 1, 7, 0], Some(limit + 2)),
            ([limit + 1, 1, 7, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(
                shared_header_arm(defaults(), constants),
                want,
                "{constants:x?}"
            );
        }
        runs(&l, &body, &cases);
    }

    /// `outer: do { s13 += 1; do { s12 += 1; if (s12 < s2) continue outer;
    /// s14 += 1; if (s12 < s3) continue outer; } while (s14 != s0); s14 = 0
    /// } while (s13 != s1)`, then the store of s12, with each `continue` a
    /// branch from inside the inner loop straight back to the outer loop's
    /// first block. The inner loop has a header of its own. The two
    /// `continue`s have the same natural loop, the outer header with the
    /// whole inner loop, and that is properly inside the natural loop of
    /// the outer loop's end.
    const OUTER_CONTINUES: [u32; 16] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBE8E_0380, // s_mov_b32 s14, 0
        0x810D_810D, // outer: s_add_i32 s13, s13, 1
        0x810C_810C, // inner: s_add_i32 s12, s12, 1
        0xBF0A_020C, // s_cmp_lt_u32 s12, s2
        0xBF85_FFFC, // s_cbranch_scc1 outer
        0x810E_810E, // s_add_i32 s14, s14, 1
        0xBF0A_030C, // s_cmp_lt_u32 s12, s3
        0xBF85_FFF9, // s_cbranch_scc1 outer
        0xBF07_000E, // s_cmp_lg_u32 s14, s0
        0xBF85_FFF8, // s_cbranch_scc1 inner
        0xBE8E_0380, // s_mov_b32 s14, 0
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFF4, // s_cbranch_scc1 outer
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    fn outer_continues(limits: Limits, [s0, s1, s2, s3]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s12, mut s13, mut s14) = (0u32, 0u32, 0u32);
        let (mut continues, mut outer) = (0, 0);
        'outer: loop {
            s13 = s13.wrapping_add(1);
            let mut inner = 0;
            loop {
                s12 = s12.wrapping_add(1);
                if s12 < s2 {
                    lane.round(&mut continues).then_some(())?;
                    continue 'outer;
                }
                s14 = s14.wrapping_add(1);
                if s12 < s3 {
                    lane.round(&mut continues).then_some(())?;
                    continue 'outer;
                }
                if s14 == s0 {
                    break;
                }
                lane.round(&mut inner).then_some(())?;
            }
            s14 = 0;
            if s13 == s1 {
                break;
            }
            lane.round(&mut outer).then_some(())?;
            continues = 0;
        }
        Some(s12)
    }

    /// Two back edges with the same natural loop are one loop and share a
    /// count: the lane is stopped after the iterations the limit allows
    /// whichever of the two `continue`s it took for each, here half by
    /// either. The outer loop's end, whose natural loop holds theirs and
    /// more, is the loop around that one, with the next count: it starts
    /// theirs again, and they do not move its own.
    #[test]
    fn a_loop_limit_counts_two_back_edges_with_one_natural_loop_together() {
        let body = OUTER_CONTINUES;
        let l = lower_valu(&body).unwrap();
        assert_eq!(limit_stops(&l, defaults(), store_block(&l)).len(), 4);
        let f = &l.flow;
        let backs = back_edges(&l, defaults());
        let [first, second, inner, end] = backs[..] else {
            panic!("four back edges: {backs:?}");
        };
        // The inner loop's header is its own, and the outer loop's end
        // comes after the inner loop.
        assert!(first.to == second.to && first.to == end.to && inner.to != end.to);
        assert!(f.reaches(inner.from, end.from, &[inner.to, end.to]));
        assert!(first.count == second.count && first.count != end.count);
        assert_eq!(brought(f, first.count, end.from), Arg::Int(0));
        for from in [first.from, second.from] {
            assert_eq!(brought(f, end.count, from), Arg::Inst(end.count));
        }
        let limit = defaults().each;
        let half = limit / 2;
        let cases = [
            ([3, 4, 0, 0], Some(12)),
            // Two rounds by the first `continue`, three by the second, and
            // two by the outer loop's end.
            ([4, 8, 3, 6], Some(14)),
            // The last iteration the limit allows, and one more.
            ([half + 1, limit, half, limit], Some(limit)),
            ([half + 2, limit + 1, half, limit + 1], None),
        ];
        for (constants, want) in cases {
            assert_eq!(
                outer_continues(defaults(), constants),
                want,
                "{constants:x?}"
            );
        }
        runs(&l, &body, &cases);
    }

    /// `do { do { do { s12 += 1; s13 += 1 } while (s13 != s0); s13 = 0;
    /// s14 += 1 } while (s14 != s1); s14 = 0; s15 += 1 } while (s15 != s2)`,
    /// then the store of s12: three loops, each beginning with the next.
    const THREE_AT_A_HEADER: [u32; 17] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBE8E_0380, // s_mov_b32 s14, 0
        0xBE8F_0380, // s_mov_b32 s15, 0
        0x810C_810C, // head: s_add_i32 s12, s12, 1
        0x810D_810D, // s_add_i32 s13, s13, 1
        0xBF07_000D, // s_cmp_lg_u32 s13, s0
        0xBF85_FFFC, // s_cbranch_scc1 head
        0xBE8D_0380, // s_mov_b32 s13, 0
        0x810E_810E, // s_add_i32 s14, s14, 1
        0xBF07_010E, // s_cmp_lg_u32 s14, s1
        0xBF85_FFF8, // s_cbranch_scc1 head
        0xBE8E_0380, // s_mov_b32 s14, 0
        0x810F_810F, // s_add_i32 s15, s15, 1
        0xBF07_020F, // s_cmp_lg_u32 s15, s2
        0xBF85_FFF4, // s_cbranch_scc1 head
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    fn three_at_a_header(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s12, mut s15, mut outer) = (0u32, 0u32, 0);
        loop {
            let (mut s14, mut middle) = (0u32, 0);
            loop {
                let (mut s13, mut inner) = (0u32, 0);
                loop {
                    s12 = s12.wrapping_add(1);
                    s13 = s13.wrapping_add(1);
                    if s13 == s0 {
                        break;
                    }
                    lane.round(&mut inner).then_some(())?;
                }
                s14 = s14.wrapping_add(1);
                if s14 == s1 {
                    break;
                }
                lane.round(&mut middle).then_some(())?;
            }
            s15 = s15.wrapping_add(1);
            if s15 == s2 {
                break;
            }
            lane.round(&mut outer).then_some(())?;
        }
        Some(s12)
    }

    /// Three loops with one header: three counts there, the innermost
    /// loop's first, then the lane's. A back edge brings its own loop's
    /// count plus one, 0 to the count of every loop inside its own (not
    /// only the next one in) and to the count of every loop around its
    /// own (not only the next one out) that count itself.
    #[test]
    fn a_loop_limit_counts_three_loops_sharing_a_header_apart() {
        let body = THREE_AT_A_HEADER;
        let l = lower_valu(&body).unwrap();
        assert_eq!(limit_stops(&l, defaults(), store_block(&l)).len(), 3);
        let f = &l.flow;
        let backs = back_edges(&l, defaults());
        let [inner, middle, outer] = backs[..] else {
            panic!("three back edges: {backs:?}");
        };
        // The innermost loop is the header alone, and the outer loop's
        // end is reached from the middle one's.
        assert_eq!(inner.from, inner.to);
        assert!(f.reaches(middle.from, outer.from, &[inner.to]));
        assert!(!f.reaches(outer.from, middle.from, &[inner.to]));
        for x in &backs {
            assert_eq!((x.to, x.total), (inner.to, inner.total));
        }
        assert!(inner.count < middle.count && middle.count < outer.count);
        assert!(outer.count < inner.total);
        let mut into = f.preds(inner.to);
        into.retain(|&b| !f.reaches(inner.to, b, &[]));
        let [entry] = into[..] else {
            panic!("into the loops from outside them: {into:?}");
        };
        for (n, of) in backs.iter().enumerate() {
            assert_eq!(brought(f, of.count, entry), Arg::Int(0));
            for (k, by) in backs.iter().enumerate() {
                let value = brought(f, of.count, by.from);
                match k.cmp(&n) {
                    // A back edge of a loop inside this one.
                    std::cmp::Ordering::Less => assert_eq!(value, Arg::Inst(of.count)),
                    // This loop's own: `back_edges` checked it.
                    std::cmp::Ordering::Equal => assert_ne!(value, Arg::Inst(of.count)),
                    // A back edge of a loop around this one.
                    std::cmp::Ordering::Greater => assert_eq!(value, Arg::Int(0)),
                }
            }
        }
        let limit = defaults().each;
        let half = limit / 2;
        let cases = [
            ([3, 4, 5, 0], Some(60)),
            // The outer loop's end starts the innermost count again, two
            // loops in: half the limit before it and half after, more than
            // the limit in all.
            ([half + 1, 1, 2, 0], Some(limit + 2)),
            // The outer loop one past its limit, the innermost loop going
            // round once in each of its iterations.
            ([2, 1, limit + 1, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(
                three_at_a_header(defaults(), constants),
                want,
                "{constants:x?}"
            );
        }
        runs(&l, &body, &cases);
    }

    /// `do { s12 = 0; do { s12 += 1 } while (s12 != s0); s13 += 1 } while
    /// (s13 != s1); do { s14 += 1 } while (s14 != s2)`, then the store of
    /// s12 + s13 + s14: a nest, then a loop.
    pub(in crate::emit::lower) const NEST_THEN_LOOP: [u32; 15] = [
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBE8C_0380, // outer: s_mov_b32 s12, 0
        0x810C_810C, // inner: s_add_i32 s12, s12, 1
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFD, // s_cbranch_scc1 inner
        0x810D_810D, // s_add_i32 s13, s13, 1
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFF9, // s_cbranch_scc1 outer
        0xBE8E_0380, // s_mov_b32 s14, 0
        0x810E_810E, // last: s_add_i32 s14, s14, 1
        0xBF07_020E, // s_cmp_lg_u32 s14, s2
        0xBF85_FFFD, // s_cbranch_scc1 last
        0x810C_0D0C, // s_add_i32 s12, s12, s13
        0x810C_0E0C, // s_add_i32 s12, s12, s14
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    fn nest_then_loop(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s13, mut s14) = (0u32, 0u32);
        let (mut outer, mut last) = (0, 0);
        let mut s12: u32;
        loop {
            s12 = 0;
            let mut inner = 0;
            loop {
                s12 = s12.wrapping_add(1);
                if s12 == s0 {
                    break;
                }
                lane.round(&mut inner).then_some(())?;
            }
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                break;
            }
            lane.round(&mut outer).then_some(())?;
        }
        loop {
            s14 = s14.wrapping_add(1);
            if s14 == s2 {
                break;
            }
            lane.round(&mut last).then_some(())?;
        }
        Some(s12.wrapping_add(s13).wrapping_add(s14))
    }

    /// [`NEST_THEN_LOOP`] lowered with the default limits and checked: the
    /// nest of s0 by s1 iterations takes s0 * s1 - 1 back edges and the
    /// loop after it s2 - 1.
    fn nest_then_loop_runs(cases: &[([u32; 4], Option<u32>)]) {
        let l = lower_valu(&NEST_THEN_LOOP).unwrap();
        assert_eq!(limit_stops(&l, defaults(), store_block(&l)).len(), 3);
        let backs = back_edges(&l, defaults());
        let mut counts: Vec<u32> = backs.iter().map(|x| x.count).collect();
        counts.sort_unstable();
        counts.dedup();
        assert_eq!(counts.len(), 3, "{backs:?}");
        for &(constants, want) in cases {
            assert_eq!(
                nest_then_loop(defaults(), constants),
                want,
                "{constants:x?}"
            );
        }
        runs(&l, &NEST_THEN_LOOP, cases);
    }

    /// The limit on a lane's loops together stops a nest whose loops are
    /// each far below their own limit: 2,048 by 2,048 iterations take the
    /// last back edge that limit allows, and 2,048 by 2,049 take more.
    #[test]
    fn a_total_loop_limit_stop_in_a_nest() {
        let Limits { each, total } = defaults();
        assert!(2048 * 2048 == total && 2049 < each);
        nest_then_loop_runs(&[
            ([2048, 2048, 1, 0], Some(2048 + 2048 + 1)),
            ([2048, 2049, 1, 0], None),
        ]);
    }

    /// A loop after a nest goes on from the nest's count of the lane's
    /// back edges: with the nest 2,049 short of that limit, the loop's
    /// 2,049th iteration is the last it allows.
    #[test]
    fn a_total_loop_limit_stop_in_a_later_loop() {
        let Limits { each, total } = defaults();
        assert!(2048 * 2047 + 2048 == total && 2050 < each);
        nest_then_loop_runs(&[
            ([3, 4, 5, 0], Some(12)),
            ([2048, 2047, 2049, 0], Some(2048 + 2047 + 2049)),
            ([2048, 2047, 2050, 0], None),
        ]);
    }

    /// `do { s12 += 1; if (s12 == s1) break; if (s12 == s2) goto second; }
    /// while (s12 != s0)`, then the store of s12, plus 0x4000_0000 after
    /// the `break` and 0x8000_0000 after the second exit: three ways out of
    /// one loop to three blocks.
    pub(in crate::emit::lower) const THREE_EXITS: [u32; 17] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0x810C_810C, // loop: s_add_i32 s12, s12, 1
        0xBF06_010C, // s_cmp_eq_u32 s12, s1
        0xBF85_0006, // s_cbranch_scc1 broken
        0xBF06_020C, // s_cmp_eq_u32 s12, s2
        0xBF85_0007, // s_cbranch_scc1 second
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFF9, // s_cbranch_scc1 loop
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBF82_0005, // s_branch end
        0xBE8D_03FF, // broken: s_mov_b32 s13, 0x40000000
        0x4000_0000,
        0xBF82_0002, // s_branch end
        0xBE8D_03FF, // second: s_mov_b32 s13, 0x80000000
        0x8000_0000,
        0x810C_0D0C, // end: s_add_i32 s12, s12, s13
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    pub(in crate::emit::lower) const BROKEN: u32 = 0x4000_0000;
    pub(in crate::emit::lower) const SECOND: u32 = 0x8000_0000;

    fn three_exits(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s12, mut count) = (0u32, 0);
        loop {
            s12 = s12.wrapping_add(1);
            if s12 == s1 {
                return Some(s12.wrapping_add(BROKEN));
            }
            if s12 == s2 {
                return Some(s12.wrapping_add(SECOND));
            }
            if s12 == s0 {
                return Some(s12);
            }
            lane.round(&mut count).then_some(())?;
        }
    }

    /// Lanes that leave one loop at different times and by different ways:
    /// by the latch, by a `break` and by a second exit, each on an early
    /// iteration, on the last one a limit allows, and not before the limit
    /// stops the lane (the loop's limit, then the lane's). Only the latch
    /// counts and tests: a lane that would leave by another way on the
    /// iteration after a limit's last never gets there.
    #[test]
    fn a_loop_limit_stop_by_the_latch_a_break_and_a_second_exit() {
        let l = lower_valu(&THREE_EXITS).unwrap();
        let store = store_block(&l);
        assert_eq!(limit_stops(&l, defaults(), store).len(), 1);
        // Three ways from the loop to the store that avoid one another,
        // and one test of the stopped lanes, on the latch's.
        let f = &l.flow;
        let [back] = back_edges(&l, defaults())[..] else {
            panic!("one back edge: {:?}", back_edges(&l, defaults()));
        };
        assert_eq!(f.preds(store).len(), 3, "{:?}", f.preds(store));
        assert_eq!(f.preds(l.limit_stop.unwrap()).len(), 1);
        assert!(f.reaches(back.to, store, &[back.from]));
        let limit = defaults().each;
        let cases = [
            ([5, 0, 0, 0], Some(5)),
            ([9, 4, 0, 0], Some(4 + BROKEN)),
            ([9, 0, 6, 0], Some(6 + SECOND)),
            ([9, 6, 6, 0], Some(6 + BROKEN)),
            // The last iteration the limit allows, and one more.
            ([0, limit, 0, 0], Some(limit + BROKEN)),
            ([0, limit + 1, 0, 0], None),
            ([0, 0, limit, 0], Some(limit + SECOND)),
            ([0, 0, limit + 1, 0], None),
        ];
        for (constants, want) in cases {
            assert_eq!(three_exits(defaults(), constants), want, "{constants:x?}");
        }
        runs(&l, &THREE_EXITS, &cases);
        // The lane's limit, with it below the loop's: by each way out the
        // fifth iteration is the last, and the loop's own limit is not near.
        let small = Limits { each: 9, total: 5 };
        let l = lower_valu_limited(&THREE_EXITS, Some(small)).unwrap();
        assert_eq!(limit_stops(&l, small, store_block(&l)).len(), 1);
        for (constants, want) in [
            ([5, 0, 0, 0], Some(5)),
            ([6, 0, 0, 0], None),
            ([0, 5, 0, 0], Some(5 + BROKEN)),
            ([0, 6, 0, 0], None),
            ([0, 0, 5, 0], Some(5 + SECOND)),
            ([0, 0, 6, 0], None),
        ] {
            assert_eq!(three_exits(small, constants), want, "{constants:x?}");
            assert_eq!(stored(&l, small, constants), want, "{constants:x?}");
        }
    }

    /// `if (s2 != 0) do { s12 += 1 } while (s12 != s0); do { s13 += 1 }
    /// while (s13 != s1)`, then the store of s12 + s13: a loop in one arm of
    /// a branch, a join, and a loop after it.
    pub(in crate::emit::lower) const LOOP_IN_AN_ARM: [u32; 13] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBE8D_0380, // s_mov_b32 s13, 0
        0xBF06_8002, // s_cmp_eq_u32 s2, 0
        0xBF85_0003, // s_cbranch_scc1 join
        0x810C_810C, // arm: s_add_i32 s12, s12, 1
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFD, // s_cbranch_scc1 arm
        0xBE8E_0380, // join: s_mov_b32 s14, 0
        0x810D_810D, // last: s_add_i32 s13, s13, 1
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFFD, // s_cbranch_scc1 last
        0x810C_0D0C, // s_add_i32 s12, s12, s13
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    fn loop_in_an_arm(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> Option<u32> {
        let mut lane = Lane::new(limits);
        let (mut s12, mut s13) = (0u32, 0u32);
        let (mut arm, mut last) = (0, 0);
        if s2 != 0 {
            loop {
                s12 = s12.wrapping_add(1);
                if s12 == s0 {
                    break;
                }
                lane.round(&mut arm).then_some(())?;
            }
        }
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                break;
            }
            lane.round(&mut last).then_some(())?;
        }
        Some(s12.wrapping_add(s13))
    }

    /// The lane's count of its back edges through a join: the block where
    /// the arm with a loop and the branch round it meet has a phi of what
    /// the loop left and 0, and the loop after it goes on from that phi.
    /// With limits of 6 for a loop and 8 for the lane, the second loop
    /// may make four iterations and not five after five in the arm, and
    /// six, its own limit's last, where the lane went round the arm.
    #[test]
    fn a_total_loop_limit_through_a_join_after_a_loop_in_one_arm() {
        let body = LOOP_IN_AN_ARM;
        let l = lower_valu(&body).unwrap();
        assert_eq!(limit_stops(&l, defaults(), store_block(&l)).len(), 2);
        let f = &l.flow;
        let [arm, last] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        // The second loop's header takes the lane's count from a phi of
        // the join, which takes 0 and the first loop's.
        let incoming = |phi: u32| {
            let (at, _, inst) = f.inst(phi).unwrap();
            let FlowOp::Phi(incoming) = &inst.op else {
                panic!("instruction {phi} is no phi")
            };
            (at, incoming.iter().map(|x| x.0).collect::<Vec<_>>())
        };
        let (header, into_last) = incoming(last.total);
        assert_eq!(header, last.to);
        let [joined] = into_last
            .iter()
            .filter_map(|&x| match x {
                Arg::Inst(id) if incoming(last.total).0 != f.inst(id).unwrap().0 => Some(id),
                _ => None,
            })
            .collect::<Vec<_>>()[..]
        else {
            panic!("into the second loop: {into_last:?}");
        };
        let (join, mut into_join) = incoming(joined);
        into_join.sort_by_key(|&x| x != Arg::Int(0));
        assert_eq!(into_join, [Arg::Int(0), Arg::Inst(arm.total)]);
        assert!(join != arm.to && join != last.to && f.preds(join).len() == 2);
        let limit = defaults().each;
        let cases = [
            ([3, 4, 1, 0], Some(7)),
            ([3, 4, 0, 0], Some(4)),
            // The loop in the arm stops the lanes that enter it.
            ([limit + 1, 1, 1, 0], None),
            ([limit + 1, 1, 0, 0], Some(1)),
        ];
        for (constants, want) in cases {
            assert_eq!(
                loop_in_an_arm(defaults(), constants),
                want,
                "{constants:x?}"
            );
        }
        runs(&l, &body, &cases);
        // The lane's limit, with small limits: four back edges in the arm
        // and four after it are one too many; without the arm's they are
        // not, and nor are three after it.
        let small = Limits { each: 6, total: 8 };
        let l = lower_valu_limited(&body, Some(small)).unwrap();
        assert_eq!(limit_stops(&l, small, store_block(&l)).len(), 2);
        for (constants, want) in [
            ([5, 4, 1, 0], Some(9)),
            ([5, 5, 1, 0], None),
            ([5, 5, 0, 0], Some(5)),
            ([5, 6, 0, 0], Some(6)),
            ([5, 7, 0, 0], None),
        ] {
            assert_eq!(loop_in_an_arm(small, constants), want, "{constants:x?}");
            assert_eq!(stored(&l, small, constants), want, "{constants:x?}");
        }
    }

    /// The programs of these tests that store one dword, each with the
    /// model of what its lane stores and how many of s0..s3 it reads.
    pub(in crate::emit::lower) const PROGRAMS: [(&str, &[u32], Model, usize); 15] = [
        ("two loops", &TWO_LOOPS, two_loops, 2),
        ("a nest", &NEST, nest, 2),
        ("a continue", &CONTINUE, continues, 2),
        ("two latches", &TWO_LATCHES, two_latches, 3),
        (
            "the latch is an inner header",
            &LATCH_IS_INNER_HEADER,
            latch_is_inner_header,
            2,
        ),
        ("both sides back", &BOTH_SIDES_BACK, both_sides_back, 2),
        ("per lane", &PER_LANE, per_lane, 1),
        ("per lane, 64 bits", &PER_LANE_64, per_lane_64, 3),
        ("a shared header", &SHARED_HEADER, shared_header, 2),
        (
            "a shared header and an arm",
            &SHARED_HEADER_ARM,
            shared_header_arm,
            3,
        ),
        ("outer continues", &OUTER_CONTINUES, outer_continues, 4),
        (
            "three at a header",
            &THREE_AT_A_HEADER,
            three_at_a_header,
            3,
        ),
        ("a nest, then a loop", &NEST_THEN_LOOP, nest_then_loop, 3),
        ("three exits", &THREE_EXITS, three_exits, 3),
        ("a loop in an arm", &LOOP_IN_AN_ARM, loop_in_an_arm, 3),
    ];

    /// Every program above with small limits, for every input from 0 to
    /// two past a loop's limit in each constant it reads (0 is a bound no
    /// count reaches before it wraps): the lane stores what the
    /// program's model says or is stopped where the model says, so each
    /// limit is hit exactly, missed by one and passed by one in every loop
    /// it can be, and every lane ends within its budget ([`budget`]), the
    /// stopped ones and the ones that would otherwise run four billion
    /// iterations included. The limits are a loop's below the lane's, the
    /// two equal, the lane's below a loop's, the lane's out of reach, and
    /// a lane that may take no back edge at all.
    #[test]
    fn the_loop_limits_stop_exactly_the_lanes_their_models_say() {
        let limits = [(5, 13), (4, 4), (6, 3), (3, 1000), (4, 1)];
        for (each, total) in limits {
            let limits = Limits { each, total };
            for (name, body, model, reads) in PROGRAMS {
                let l = lower_valu_limited(body, Some(limits)).unwrap();
                limit_stops(&l, limits, store_block(&l));
                let values = each + 3;
                let mut stopped = 0;
                for n in 0..values.pow(reads as u32) {
                    let at = |k: u32| {
                        if (k as usize) < reads {
                            n / values.pow(k) % values
                        } else {
                            0
                        }
                    };
                    let constants = [at(0), at(1), at(2), at(3)];
                    let want = model(limits, constants);
                    stopped += u32::from(want.is_none());
                    let got = stored(&l, limits, constants);
                    assert_eq!(got, want, "{name}: {constants:?} with {limits:?}");
                }
                assert!(
                    stopped > 0 && stopped < values.pow(reads as u32),
                    "{name} with {limits:?}: {stopped} lanes stopped"
                );
            }
        }
    }
}
