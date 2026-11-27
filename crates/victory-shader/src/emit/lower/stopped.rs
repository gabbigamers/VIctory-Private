//! What a lane does that [`LOOP_LIMIT`] or [`LOOP_TOTAL_LIMIT`] stopped,
//! stage by stage, and the tests of it. `limit` says which lanes are
//! stopped and where; nothing here moves that. A stop is a defect to look
//! into (`docs/progress.md`, C7), never a way to render: what follows only
//! makes the stopped lane's end defined, bounded and without effect.
//!
//! A lane is stopped at a latch: the block whose branch would have taken
//! it round its loop once more. Two kinds of latch differ in what can
//! follow.
//!
//! * A latch whose branch also leaves its loop (a conditional back edge:
//!   `do { } while (c)`, a per-lane `s_cbranch_execnz`, a conditional
//!   `continue`). The stopped lane takes that branch's other side, an edge
//!   of the program that lanes take of themselves, so every value read
//!   after it is one the function computes. "Its loop" is the loop the
//!   back edge counts for (`limit`, "Loops"), and the other side need not
//!   be outside every loop: from a loop inside another it is the outer
//!   loop's body, and from a conditional `continue`, which is counted as
//!   a loop of its own at the loop's header, it is the rest of that
//!   loop's body ("Pixel" below says what a lane does there).
//! * A latch that only goes back (an unconditional back edge: the end of
//!   a `while (c) { }` whose test is at its head, a `continue` that ends
//!   an arm of a branch). No edge of the program leaves the loop from
//!   there. Another way out would be one the program does not have: a
//!   branch to a block after the loop, whose joins have no value for it,
//!   or one more round to reach a test that leaves, which the limits
//!   forbid and which would put the stop into the condition of an exit
//!   that is not the latch (the shape drivers hang on, `limit`, "Shape").
//!   A lane stopped there ends at once, in every stage, in the one block
//!   [`Lower::abort_block`] makes.
//!
//! # Pixel
//!
//! How the lowering already treats a lane that must write nothing but
//! keep running: the title's own discard is a host `discard` where the
//! console takes the lane out of its live mask, and the lane runs on
//! (`structure`, the module's documentation; `Lower::run`). The host
//! keeps a discarded pixel as a helper lane of its quad. Derivatives and
//! quad reads see what it computes, wave operations leave it out, none
//! of its output writes counts, before the discard or after, and from the
//! discard on its operations on unordered access views change no memory
//! (Direct3D 11.3 functional specification, 16.13 and 22.5.1; the DXIL
//! `isHelperLane` is true in it, HLSL shader model 6.6). There is no
//! deferred kill mask: the host holds the lane's state. A stopped lane is
//! treated the same way.
//!
//! **A stop at a latch that leaves its loop.** The latch keeps its one
//! branch, and the block on its edge out of the loop, which sent the
//! stopped lanes to the end of the program, now holds, for the lanes that
//! would have gone round again (the latch's condition `cond` against the
//! latch's own sense, as before):
//!
//! * `discard(stopped)`, and
//! * a `select` on `cond` between `LOOP_TOTAL_LIMIT - 1`, for the stopped
//!   lanes, and `total`, for the others, as the lane's count of its back
//!   edges on that edge, where a back edge can still follow,
//!
//! and then goes on to where the loop's exit goes ([`Lower::leave_stopped`]).
//! The block the exit goes to takes that count in a phi, also where it has
//! one way in (`Lower::count_loops`). That is all "continues as a helper"
//! means in the emitted code: one conditional discard and one select on
//! the loop's own exit, no branch, and no flag carried through the
//! program, because the discard is the flag and the host carries it. The
//! loop keeps the shape it had, and no edge leaves it for a block shared
//! by the whole program.
//!
//! Where the back edge is the side on which `cond` holds, `stopped` is
//! `cond`. Where it is the other side, `stopped` is its negation, made in
//! that block and read by the discard alone ([`Lower::negated`]). For a
//! `cond` that is a compare it is the compare of the same operands with
//! the opposite predicate, and not an `xor` of the compare: a driver hung
//! on a loop exit that tested one (`limit`, "Shape"), and this block is
//! next to a loop, and inside the loop around it where the latch is an
//! inner loop's. For any other `cond` (a join of lane masks, compares
//! joined by `and` or `or`, a compare of 64 bits, which is made of two, a
//! float compare whose opposite is not emitted) it is `xor cond, true`,
//! as a guarded export's discard is of its guard.
//!
//! **Where the lane goes on.** The exit of the latch's loop is wherever
//! the program has the other side of that branch. After a loop that
//! stands alone it is the code after the loop. From an inner loop it is
//! the outer loop's body, and the lane runs that to the outer loop's
//! latch, which refuses it in turn. At a conditional `continue` with more
//! of the body after it (`if (c) continue;`, the same graph as an inner
//! loop that shares the header, so the two cannot be told apart or
//! treated differently) it is the rest of that body. The lane refused
//! there is discarded and then runs the code the program's `continue`
//! was skipping for it, stores and exports included, as far as the loop's
//! own latch, which lets it out or, where that one only goes back, ends
//! it. Those stores come after its discard and the host drops them like
//! every other. They are still calls the program makes for no lane in
//! that state: the lane is in the part of the body that its condition
//! says to skip.
//!
//! What this gives, and what it does not:
//!
//! * *Derivatives.* The stopped lane runs the code on the latch's other
//!   side, so the other lanes of its quad take their derivatives against
//!   a lane that computes, as against one the title discarded. Every
//!   value it reads is one the function computes: none is missing on its
//!   path. What those values are worth is less. Of the values the loop
//!   writes it holds what the last round the limits allowed left, which
//!   is not what the console's loop would have ended with; nothing can
//!   supply that. What a read through an unordered access view returns to
//!   a pixel after its discard is undefined on the host (16.13: such an
//!   operation changes no memory, and "if [it] return[s] a value to the
//!   shader, the value returned is undefined"): a load from a buffer or a
//!   texture bound for unordered access, the old value an atomic returns.
//!   That is so for a lane the title discarded as well. The lane is also
//!   in a state the program never produces (past the loop with the loop's
//!   condition still holding), so the masks structurisation derives from
//!   that condition say of it what they would say of a lane inside the
//!   loop, and a join that proved no lane on that edge reads a value
//!   gives it 0. So the stopped lane holds what the console's lane would
//!   only of values that depend on none of these: not on the loop, and
//!   not on such a read after the stop. An index it computes from the
//!   others is clamped to the heap as any index is (`emit`, "Heap
//!   indices"); what a wrong index inside the heap reads is the hazard of
//!   every lane that computes from data with no host copy, and a stopped
//!   lane that goes on is one more such lane.
//! * *No output.* The discard drops the lane's colour, depth, stencil and
//!   coverage, whether exported before the stop or after.
//! * *No further effect.* Every store, atomic operation and mark a pixel
//!   program makes is an operation on an unordered access view (title
//!   memory, buffer and texture views, the bitmaps of `emit`'s "Write
//!   marks" and "Read marks"), and the host performs none of them for the
//!   lane after its discard. The calls are still in the function and the
//!   lane still reaches them: it is the host that drops them, exactly as
//!   for the title's discards, and no mask of this lowering's own stands
//!   behind that. What the lane stored before the stop stands: it was
//!   made by rounds the limits allowed.
//! * *Bounded.* The lane leaves with its count of back edges at the most
//!   [`LOOP_TOTAL_LIMIT`] lets a lane reach, so every latch after the stop
//!   refuses it by the test it already makes: it takes no further back
//!   edge, whatever its values are. From the stop to the end it enters no
//!   block twice, and its work after the stop is at most one pass over
//!   the program. In a loop that follows, or that it is still inside, it
//!   runs to the latch and leaves there (a latch that only goes back ends
//!   it). The other choice within the limits, to let it go round later
//!   loops while its count allows, would keep derivatives right inside
//!   those loops, but a lane the loop's limit stopped inside another loop
//!   would then run the inner loop to its limit again on each outer
//!   round, up to four times the work a stop cost before this change, in
//!   exactly the programs that reach the limit (a loop reading a view
//!   with no host copy). A stop must not cost more than it did.
//!   Derivatives taken inside a later loop, from its second round on, are
//!   therefore taken against a lane that has left it.
//! * *No loop to hang in.* It takes no back edge and waits for no other
//!   lane: nothing in a pixel program waits, the host's wave operations
//!   leave helper lanes out, and a loop that would wait for a value only
//!   another lane's wave read can give (`helper_reads_own`) is a loop it
//!   leaves at its first latch. The host is free to stop running it after
//!   the last derivative that needs it. Whether a draw whose lanes ran to
//!   the limits is slow enough for the system to reset the device is what
//!   the limits' values decide, as before; the helper adds one pass.
//!
//! A helper lane that is refused again at a later latch is discarded
//! again there, which changes nothing.
//!
//! **A stop at a latch that only goes back.** The lane is discarded and
//! returns, as every stopped pixel lane did before: `discard(true)`, then
//! `ret`. Its quad's later derivatives are taken against a lane that no
//! longer runs. The same block ends a lane the counted outer back edge
//! refuses where a latch falls from an inner loop into the header of the
//! loop around it.
//!
//! # Vertex
//!
//! A stopped vertex lane ends at once, and in the block it ends in it
//! writes every output component the program writes anywhere: a quiet NaN
//! to each written component of the position, 0 to the rest
//! ([`Lower::end_stopped`]). Written last, these replace what the lane
//! exported before the stop. The rasteriser discards a primitive that has
//! a vertex whose position is NaN after clipping and the perspective
//! divide (Direct3D 11.3 functional specification, 15.4 and 15.16.1), and
//! NaN in all of x, y, z and w stays NaN through both. So every primitive
//! the vertex is part of is dropped, whatever the stop left of its other
//! outputs, and no pixel takes the zeros for a result: geometry that is
//! missing cannot be mistaken for geometry that is right, which a position
//! of 0, or one exported before the stop with parameters of 0, could be.
//! The components written are the ones the program's own exports write, so
//! the signature and its masks are what they were. A program that exports
//! no position has no such mark; its stopped lane writes zeros. Stores a
//! vertex lane made before the stop stand, and it makes none after.
//!
//! # Mesh
//!
//! A mesh lane is a thread of its group and writes the vertex and the
//! primitive of its own index. A stopped lane ends at once, and where it
//! ends it writes its vertex as a stopped vertex lane writes its outputs
//! (NaN to the position, 0 to the rest), culls its primitive (all three
//! indices 0, as every lane does before its code, `mesh_prologue`) and
//! writes 0 to the per-primitive outputs the program writes. A write to an
//! output that was written before replaces it. Its own primitive is
//! therefore not drawn, and any other lane's primitive that uses its
//! vertex is discarded by the rasteriser for the NaN. A lane whose index
//! is past the declared vertex count writes no vertex, and one past the
//! primitive count no primitive (it has none): each of the two is behind
//! the test of its own count. What the lane stored in the group's shared
//! memory before the stop stands, so other lanes may read a partial
//! result from it; after the stop it stores nothing.
//!
//! # Compute
//!
//! A stopped compute lane returns at once, as before, and the function is
//! written exactly as before. A compute program has no outputs. What the
//! lane stored before the stop stands, and it performs no store, atomic
//! operation or mark after it. A thread that reads what the stopped one
//! would have written, in a buffer or in the group's shared memory, reads
//! what was there. A barrier after the loop is one the stopped thread
//! does not reach; that was so before, and what a host does with the rest
//! of the group then has not been seen on a card.
//!
//! Two forms that only a compute program has meet that block; no other
//! stage has either, so no thread comes to a vertex's or a mesh lane's
//! end that a loop limit did not stop. With thread limits (`emit`,
//! "Thread limits") a thread past its limit leaves at entry through it
//! (`Lower::limit_threads`): the block only returns, which is the end of
//! both. In an emulated wave (`emulated`) every thread takes the wave's
//! branches and counts the same back edges, so a limit stops the whole
//! wave at one latch and no thread is left at a meeting the others never
//! reach (`emit`'s `emulated_tests`).
//!
//! # Other stages
//!
//! Only vertex, pixel, compute and mesh programs are lowered (`emit`'s
//! `translate`). The rectangle geometry shader (`rect`) is written by hand
//! and has no loop.
//!
//! # What the tests show, and what they cannot
//!
//! The tests run one lane at a time through `bitcode`'s interpreter, which
//! sees every call a lane makes in order. Lanes that leave a loop at
//! different times are separate runs with different inputs. That shows
//! each lane's own path, calls and end. It does not show a quad: no
//! derivative is taken, no helper lane exists there, and that the host
//! drops a discarded lane's writes is the host's rule, which the tests
//! apply to the calls they see (a store after a true discard is counted
//! as dropped) and cannot observe. All but one of the programs the tests
//! run compare integers at their latches. The one latch that compares
//! floats is lowered and its two compares looked at, the float
//! predicates' opposites are checked as a table (`bitcode`'s tests), and
//! its lanes are run with bounds whose bits are small positive floats and
//! one that is no number, on this host's `f32` as the interpreter has it:
//! that is not a driver's float compare. Microsoft's validator
//! (`scripts/validate-dxil.sh`) says the modules are well-formed. That a
//! driver takes the shapes, drops the writes, and discards a primitive
//! for a NaN position is for a run on a card. The shapes to look for
//! there: on the exit of a loop, and inside the loop around it where the
//! loop is an inner one, a block that holds a compare or an `xor` feeding
//! a `discard`, and a `select` on the latch's condition; a phi with one
//! entry where that exit goes; the blocks vertex and mesh lanes end in.
//!
//! [`LOOP_LIMIT`]: super::LOOP_LIMIT
//! [`LOOP_TOTAL_LIMIT`]: super::LOOP_TOTAL_LIMIT

use super::Lower;
use crate::emit::bitcode::{Attr, Bin, Term, V, pred};
use crate::native::HwStage;
use crate::structure::LaneExit;

/// The bits of the float a stopped vertex's position is written as: a
/// quiet NaN.
const STOPPED_POSITION: u32 = 0x7FC0_0000;

impl Lower<'_> {
    /// Whether the edge from IR block `q` to `b` is one on which a pixel
    /// program's latch leaves its loop for a block that is no loop's header
    /// behind it: the other side of `q`'s branch is a back edge. The lanes
    /// a limit stopped at `q` go on along it ([`Lower::leave_stopped`]).
    /// `b` can be inside a loop still: the body of the loop around `q`'s,
    /// or, where `q` ends in a conditional `continue`, the rest of the
    /// body of the same loop of the program.
    pub(super) fn leaves_stopped(&self, q: usize, b: usize) -> bool {
        let LaneExit::Branch {
            taken, not_taken, ..
        } = self.x.st.exits[q]
        else {
            return false;
        };
        let other = match (taken == b, not_taken == b) {
            (true, false) => not_taken,
            (false, true) => taken,
            _ => return false,
        };
        let back = |to: usize| self.counters.contains_key(&to) && self.rpo[to] <= self.rpo[q];
        self.s.pixel && back(other) && !back(b)
    }

    /// Pixel: the block latch `b`'s conditional branch goes to in place of
    /// `via`, the block of its edge out of the loop to IR block `to`, as
    /// [`Lower::limit_exit`] makes it for the other stages. The lanes whose
    /// own condition ends the loop (`cond` is `own` in them) pass through
    /// it untouched. The lanes a limit stopped (`cond` is the opposite:
    /// they would have gone round again) are discarded here and go on with
    /// the others as helper lanes; the module's documentation says what
    /// that gives, and where `to` is.
    ///
    /// A stopped lane takes no further back edge: where one can still
    /// follow, the lane's count of its back edges along this edge is, for
    /// the stopped lanes, the most the lane's limit lets a count reach
    /// (one below the limit), so every later latch refuses them by the
    /// test it makes of every lane.
    ///
    /// Where the latch's other side is a back edge too, the block of its
    /// own that counts it comes after this one and counts with that
    /// count: the outer loop takes the lanes that left the inner one of
    /// themselves round, and ends the stopped ones.
    pub(super) fn leave_stopped(
        &mut self,
        b: usize,
        to: usize,
        mut via: usize,
        cond: V,
        own: bool,
    ) -> usize {
        let at = self.s.m.new_block();
        let stopped = if own { self.negated(at, cond) } else { cond };
        let void = self.s.t.void;
        self.s
            .op(at, "discard", Attr::NoUnwind, void, 82, &[stopped]);
        let header = self.counters.contains_key(&to) && self.rpo[to] <= self.rpo[b];
        if header || self.totals.contains_key(&to) {
            let spent = self.s.m.i32(self.limits.total.saturating_sub(1));
            let all = self.total_at(b);
            // Chosen by the latch's condition as it is, whichever sense
            // stops: the negation is the discard's alone.
            let all = if own {
                self.s.m.select(at, cond, all, spent)
            } else {
                self.s.m.select(at, cond, spent, all)
            };
            self.left.insert((b, to), all);
        }
        if header {
            let here = std::mem::replace(&mut self.cur, self.s.m.new_block());
            let ok = self.count_iteration(b, to, true);
            let abort = self.abort_block();
            self.s.m.terminate(self.cur, Term::CondBr(ok, via, abort));
            via = std::mem::replace(&mut self.cur, here);
            self.edges.insert((b, to), via);
        }
        self.s.m.terminate(at, Term::Br(via));
        if Some(via) == self.block[to] {
            self.edges.insert((b, to), at);
        }
        at
    }

    /// In block `at`, the negation of a latch's condition `cond` (whatever
    /// `cond` is computed from is computed before the latch's branch, so
    /// in `at` too). Where `cond` is a compare, this is the compare of the
    /// same operands with the opposite predicate, as DXC writes `!(a == b)`,
    /// and not an `xor` of a compare, which a driver hung on where a loop's
    /// exit tested one (`limit`, "Shape"). Where `cond` is anything else (a
    /// join of lane masks, compares joined by `and` or `or`, a compare of
    /// 64 bits, which is made of two), or a float compare whose opposite
    /// the lowering does not emit, it is `xor cond, true`: what a guarded
    /// export's discard is of its guard (`Lower::export`).
    fn negated(&mut self, at: usize, cond: V) -> V {
        let compare = self.s.m.compare(cond);
        match compare.and_then(|(p, x, y)| Some((pred::opposite(p)?, x, y))) {
            Some((p, x, y)) => self.s.m.cmp(at, p, x, y),
            None => {
                let t = self.s.m.bool(true);
                self.s.m.bin(at, Bin::Xor, cond, t)
            }
        }
    }

    /// Fills the block a stopped lane ends in ([`Lower::abort_block`]) for
    /// the stages with outputs to define there, once every block is
    /// lowered and the components the program's own exports write are
    /// known. A vertex lane writes each of them: a NaN to the position's,
    /// 0 to the others. A mesh lane writes its vertex that way, culls its
    /// primitive and writes 0 to the per-primitive outputs. A pixel's
    /// block discards and a compute lane's only returns, both from the
    /// start.
    pub(super) fn end_stopped(&mut self) {
        let Some(at) = self.abort else { return };
        let vertex = self.x.native.stage == HwStage::Vs;
        if !vertex && self.s.mesh.is_none() {
            return;
        }
        // What is still to come places code by the current block.
        let here = std::mem::replace(&mut self.cur, at);
        match self.s.mesh {
            None => {
                for id in 0..self.s.outputs.len() {
                    self.stopped_output(id, false, None);
                }
            }
            Some(mesh) => {
                let threads = self.s.threads.expect("mesh")[0];
                let flat = self.mesh_thread();
                // Whether the lane has an output of its index among
                // `count`: only tested where the group has more lanes.
                let within = |l: &mut Self, count: u32| {
                    (threads > count).then(|| {
                        let count = l.s.m.i32(count);
                        l.s.m.cmp(l.cur, pred::IULT, flat, count)
                    })
                };
                let guard = within(self, mesh.max_vertices);
                let guarded = self.begin_guarded(guard);
                for id in 0..self.s.outputs.len() {
                    self.stopped_output(id, false, Some(flat));
                }
                self.end_guarded(guarded, &[]);
                let guard = within(self, mesh.max_primitives);
                let guarded = self.begin_guarded(guard);
                let (z, void) = (self.s.m.i32(0), self.s.t.void);
                self.s.op(
                    self.cur,
                    "emitIndices",
                    Attr::NoUnwind,
                    void,
                    169,
                    &[flat, z, z, z],
                );
                for id in 0..self.s.prims.len() {
                    self.stopped_output(id, true, Some(flat));
                }
                self.end_guarded(guarded, &[]);
                self.s.m.terminate(self.cur, Term::Ret);
            }
        }
        self.cur = here;
    }

    /// In the current block, what a stopped lane writes to each component
    /// the program writes of output element `id` (of the per-primitive
    /// outputs if `primitive`): a NaN to a position's, 0 to any other's. A
    /// mesh lane writes its own vertex or primitive, `thread`.
    fn stopped_output(&mut self, id: usize, primitive: bool, thread: Option<V>) {
        let outputs = if primitive {
            &self.s.prims
        } else {
            &self.s.outputs
        };
        let (kind, comp, usage) = (outputs[id].kind, outputs[id].comp, outputs[id].usage);
        let (name, opcode) = match (thread, primitive) {
            (None, _) => ("storeOutput", 5),
            (Some(_), false) => ("storeVertexOutput", 171),
            (Some(_), true) => ("storePrimitiveOutput", 172),
        };
        for col in (0..4u8).filter(|col| usage >> col & 1 != 0) {
            let (suffix, x) = match comp {
                9 if kind == 3 => (".f32", self.s.m.f32(STOPPED_POSITION)),
                9 => (".f32", self.s.m.f32(0)),
                _ => (".i32", self.s.m.i32(0)),
            };
            let mut args = vec![
                self.s.m.i32(id as u32),
                self.s.m.i32(0),
                self.s.m.i8(col),
                x,
            ];
            args.extend(thread);
            let void = self.s.t.void;
            self.s.op(
                self.cur,
                &format!("{name}{suffix}"),
                Attr::NoUnwind,
                void,
                opcode,
                &args,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;

    use super::super::limit::tests::{
        BOTH_SIDES_BACK, BROKEN, Back, LATCH_IS_INNER_HEADER, LOOP_IN_AN_ARM, NEST, NEST_THEN_LOOP,
        PROGRAMS, SECOND, SHARED_HEADER, THREE_EXITS, TWO_LOOPS, back_edges, budget, defaults,
    };
    use super::super::tests::{lower_valu_limited, root};
    use super::super::{Limits, Lowered, Parts, lower_limited};
    use super::STOPPED_POSITION;
    use crate::emit::bitcode::{Arg, Bin, FlowOp, FlowTerm, pred};
    use crate::emit::container::SigElement;
    use crate::emit::{USER_THREAD_LIMIT, UserData};
    use crate::native::{
        ComputeIo, HwStage, MeshIo, NativeProgram, ResourceType, RootParam, RootType, StageIo,
        Usage, UsedResource, VsOutput,
    };
    use crate::{io, ir, resource, structure};

    const S_ENDPGM: u32 = 0xBF81_0000;

    /// The small limits the programs are swept with, as the limits' own
    /// tests take them: a loop's below the lane's, the two equal, the
    /// lane's below a loop's, the lane's out of reach, and a lane that may
    /// take no back edge at all.
    const SMALL: [(u32, u32); 5] = [(5, 13), (4, 4), (6, 3), (3, 1000), (4, 1)];

    thread_local! {
        /// Whether [`lower_stage`] asks for thread limits (`emit`, "Thread
        /// limits"), which a compute program takes and no other stage
        /// does: only inside [`with_thread_limits`].
        static THREAD_LIMITS: Cell<bool> = const { Cell::new(false) };
    }

    /// What `lower` makes when every program it lowers is lowered with
    /// thread limits asked for.
    fn with_thread_limits<T>(lower: impl FnOnce() -> T) -> T {
        THREAD_LIMITS.set(true);
        let made = lower();
        THREAD_LIMITS.set(false);
        made
    }

    /// A four-channel float element of a signature, interpolated as `emit`
    /// declares its kind.
    fn element(name: &str, kind: u8, row: u32) -> SigElement {
        SigElement {
            name: name.into(),
            index: 0,
            kind,
            comp: 9,
            interp: match kind {
                3 => 4,
                0 => 2,
                _ => 0,
            },
            row: Some(row),
            col: 0,
            cols: 4,
            usage: 0,
            half: false,
        }
    }

    /// Lowers the program `words` of `stage` with `limits`: four root
    /// constants in its first user SGPRs, then a descriptor table with one
    /// raw buffer for unordered access. `outputs` and `prims` are its
    /// output signatures.
    fn lower_stage(
        stage: HwStage,
        io: StageIo,
        words: &[u32],
        outputs: Vec<SigElement>,
        prims: Vec<SigElement>,
        limits: Limits,
    ) -> Lowered {
        let constants = RootParam {
            sgprs: 4,
            ..root(RootType::Constants, 0, 0)
        };
        let buffer = UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 1,
        };
        let native = NativeProgram {
            stage,
            wave64: false,
            vgprs: 256,
            layout: vec![constants, root(RootType::Table, 4, 1)],
            used: vec![buffer],
            io,
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = ir::build(&native.code, false, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let parts = Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs,
            prims,
            mesh_stride: 0,
            view_marks: false,
        };
        let lowered = lower_limited(&parts, limits, THREAD_LIMITS.get()).unwrap();
        // Every program these tests lower is a well-formed function, and
        // on Windows a module `dxil.dll` takes, where it is there.
        lowered.flow.check().unwrap();
        #[cfg(windows)]
        if let Ok(mut v) = crate::emit::validator::Validator::load(
            crate::emit::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut lowered.dxil.clone())
                .expect("DXIL validation failed");
        }
        lowered
    }

    /// What every program of a vertex, pixel or compute stage here begins
    /// with: the buffer's descriptor in s[8:11], and the root constants
    /// s0..s3 copied to v1..v4.
    fn prologue() -> Vec<u32> {
        let mut words = vec![
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202, // s_load_dwordx4 s[8:11], s[4:5], 0
            0xFA00_0000,
        ];
        // v_mov_b32 v1..v4, s0..s3
        words.extend((0..4u32).map(|i| 0x7E00_0200 | (i + 1) << 17 | i));
        words
    }

    /// The store every such program makes of what its body left in v0.
    const STORE_V0: [u32; 2] = [
        0xE070_0000, // buffer_store_dword v0, off, s[8:11], 0
        0x8002_0000,
    ];

    /// A pixel program around `body`, which takes the root constants and
    /// leaves its result in v0: it then stores v0 to the buffer and exports
    /// v0 in target 0's first channel and v1, the first root constant,
    /// which no body writes, in the others.
    fn lower_pixel(body: &[u32], limits: Limits) -> Lowered {
        let mut words = prologue();
        words.extend_from_slice(body);
        words.extend_from_slice(&STORE_V0);
        words.extend_from_slice(&[
            0xF800_180F, // exp mrt0, v0, v1, v1, v1 done vm
            0x0101_0100,
            S_ENDPGM,
        ]);
        let io = StageIo::Ps {
            inputs: vec![],
            input_addr: 0,
            db_shader_control: 0,
            patches: vec![],
        };
        let target = element("SV_Target", 16, 0);
        lower_stage(HwStage::Ps, io, &words, vec![target], vec![], limits)
    }

    /// A vertex program around `body`, as [`lower_pixel`]'s: after the
    /// store it exports v0, v1, v1, v1 as parameter 0 (output row 1) and as
    /// the position. With `early`, it exports the parameter before the
    /// body instead, as v1 in every channel.
    fn lower_vertex(body: &[u32], early: bool, limits: Limits) -> Lowered {
        let mut words = prologue();
        if early {
            // exp param0, v1, v1, v1, v1
            words.extend_from_slice(&[0xF800_020F, 0x0101_0101]);
        }
        words.extend_from_slice(body);
        words.extend_from_slice(&STORE_V0);
        if !early {
            // exp param0, v0, v1, v1, v1
            words.extend_from_slice(&[0xF800_020F, 0x0101_0100]);
        }
        words.extend_from_slice(&[
            0xF800_08CF, // exp pos0, v0, v1, v1, v1 done
            0x0101_0100,
            S_ENDPGM,
        ]);
        let io = StageIo::Vs {
            inputs: vec![],
            outputs: vec![VsOutput {
                usage: Usage::Generic,
                row: 1,
                param: 0,
                fp16: false,
                pack2: false,
            }],
        };
        let outputs = vec![element("SV_Position", 3, 0), element("TEXCOORD", 0, 1)];
        lower_stage(HwStage::Vs, io, &words, outputs, vec![], limits)
    }

    /// What a lane did that has an effect, or would have one.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Did {
        /// A `discard` whose condition held.
        Discard,
        /// The dword it stored to the buffer.
        Store(u32),
        /// A vertex or pixel output: element, channel, the value's bits.
        Output(u32, u32, u32),
        /// A mesh vertex output: element, channel, the value's bits, vertex.
        Vertex(u32, u32, u32, u32),
        /// A mesh per-primitive output: the same, with the primitive.
        PerPrimitive(u32, u32, u32, u32),
        /// A mesh primitive: its index and its three vertices.
        Indices([u32; 4]),
    }

    /// One lane's run.
    struct Run {
        /// Each such call in order, with how many blocks the lane had
        /// entered when it made it.
        calls: Vec<(usize, Did)>,
        /// The blocks it entered, in order.
        blocks: Vec<usize>,
    }

    impl Run {
        fn did(&self) -> Vec<Did> {
            self.calls.iter().map(|c| c.1).collect()
        }

        /// The dwords stored, in order.
        fn stores(&self) -> Vec<u32> {
            let stores = self.calls.iter().filter_map(|c| match c.1 {
                Did::Store(x) => Some(x),
                _ => None,
            });
            stores.collect()
        }

        /// What output element `id` holds when the lane ends: the last
        /// value written to each of its four channels, if all were.
        fn output(&self, id: u32) -> Option<[u32; 4]> {
            let mut held = [None; 4];
            for &(_, did) in &self.calls {
                if let Did::Output(e, col, x) | Did::Vertex(e, col, x, _) = did
                    && e == id
                {
                    held[col as usize] = Some(x);
                }
            }
            Some([held[0]?, held[1]?, held[2]?, held[3]?])
        }
    }

    /// Runs one lane of `l`, lowered with `limits`, on this host: thread
    /// `thread` of its group, with `constants` as the root constants. It
    /// has the step budget every lane of the limits' tests has
    /// ([`budget`]: the lane's limit times the function's size) and must
    /// end within it.
    fn run(l: &Lowered, limits: Limits, constants: [u32; 4], thread: u32) -> Run {
        run_with(l, limits, constants, thread, [0; 4])
    }

    /// [`run`], with `next` as the four dwords of user data after the root
    /// constants (the descriptor table's, and after it a compute program's
    /// thread limit, where it has one).
    fn run_with(
        l: &Lowered,
        limits: Limits,
        constants: [u32; 4],
        thread: u32,
        next: [u32; 4],
    ) -> Run {
        let blocks = RefCell::new(Vec::new());
        let mut calls = Vec::new();
        let mut call = |name: &str, args: &[u64]| {
            let arg = |n: usize| args[n] as u32;
            let did = match name {
                // The root constants are the user data's first register.
                "dx.op.cbufferLoadLegacy.i32" if args[2] == 0 => {
                    return constants.map(u64::from).to_vec();
                }
                "dx.op.cbufferLoadLegacy.i32" if args[2] == 1 => {
                    return next.map(u64::from).to_vec();
                }
                "dx.op.cbufferLoadLegacy.i32" => return vec![0; 4],
                "dx.op.flattenedThreadIdInGroup.i32" => return vec![u64::from(thread)],
                "dx.op.discard" if args[1] != 0 => Did::Discard,
                "dx.op.rawBufferStore.i32" => Did::Store(arg(4)),
                "dx.op.storeOutput.f32" | "dx.op.storeOutput.i32" => {
                    Did::Output(arg(1), arg(3), arg(4))
                }
                "dx.op.storeVertexOutput.f32" => Did::Vertex(arg(1), arg(3), arg(4), arg(5)),
                "dx.op.storePrimitiveOutput.f32" => {
                    Did::PerPrimitive(arg(1), arg(3), arg(4), arg(5))
                }
                "dx.op.emitIndices" => Did::Indices([arg(1), arg(2), arg(3), arg(4)]),
                _ => return vec![0],
            };
            calls.push((blocks.borrow().len(), did));
            Vec::new()
        };
        let mut enter = |b: usize| blocks.borrow_mut().push(b);
        let ran = l.flow.walk(budget(l, limits), &mut call, Some(&mut enter));
        assert_eq!(ran, Ok(()), "{constants:x?} with {limits:?}");
        let blocks = blocks.into_inner();
        Run { calls, blocks }
    }

    /// One pixel lane as the host counts what it did (Direct3D's rule for a
    /// discarded pixel, which these tests apply and cannot observe): the
    /// stores before its first discard are performed, the stores after it
    /// are dropped, and so is its export if it was discarded at all.
    #[derive(Debug, PartialEq, Eq)]
    struct Pixel {
        discarded: bool,
        performed: Vec<u32>,
        dropped: Vec<u32>,
        /// The channels of target 0 as the lane exported them, where it
        /// reached its export.
        exported: Option<[u32; 4]>,
    }

    /// Runs one lane of a pixel program ([`run`]). A lane that is discarded
    /// enters no block twice from the block of its first discard on: it
    /// takes no back edge after it.
    fn pixel(l: &Lowered, limits: Limits, constants: [u32; 4]) -> Pixel {
        let run = run(l, limits, constants, 0);
        let first = run.calls.iter().position(|c| c.1 == Did::Discard);
        if let Some(first) = first {
            let from = run.calls[first].0 - 1;
            let mut seen = HashSet::new();
            assert!(
                run.blocks[from..].iter().all(|&b| seen.insert(b)),
                "{constants:x?} with {limits:?}: a block entered again after the discard: {:?}",
                &run.blocks[from..]
            );
        }
        let (mut performed, mut dropped) = (Vec::new(), Vec::new());
        for (n, &(_, did)) in run.calls.iter().enumerate() {
            if let Did::Store(x) = did {
                if first.is_some_and(|first| n > first) {
                    dropped.push(x);
                } else {
                    performed.push(x);
                }
            }
        }
        Pixel {
            discarded: first.is_some(),
            performed,
            dropped,
            exported: run.output(0),
        }
    }

    /// `do { s12 += 1 } while (s12 != s0)`, the result s12, with the back
    /// edge the taken side of the latch's branch: on SCC set
    /// (`s_cmp_lg_u32`, `s_cbranch_scc1`) or on SCC clear.
    fn counts_to_s0(on_set: bool) -> [u32; 5] {
        let (cmp, branch) = if on_set {
            (0xBF07_0000, 0xBF85_0000)
        } else {
            (0xBF06_0000, 0xBF84_0000)
        };
        [
            0xBE8C_0380,     // s_mov_b32 s12, 0
            0x810C_810C,     // loop: s_add_i32 s12, s12, 1
            cmp | 0x000C,    // s_cmp_{lg,eq}_u32 s12, s0
            branch | 0xFFFD, // s_cbranch_scc{1,0} loop
            0x7E00_020C,     // v_mov_b32 v0, s12
        ]
    }

    /// The block latch `latch` leaves its loop to, with the operation its
    /// conditional branch is on.
    fn leaves(l: &Lowered, latch: usize, header: usize) -> (usize, &FlowOp) {
        let f = &l.flow;
        let FlowTerm::CondBr(Arg::Inst(both), yes, no) = f.blocks[latch].term else {
            panic!("latch {latch} does not branch on a computed condition");
        };
        let out = if yes == header { no } else { yes };
        (out, &f.inst(both).unwrap().2.op)
    }

    /// Back edge `back`'s latch: the block it leaves its loop to, its own
    /// condition (what its branch folds with the limits), and whether the
    /// back edge is the side where that condition holds.
    fn way_out(l: &Lowered, back: Back) -> (usize, Arg, bool) {
        match leaves(l, back.from, back.to) {
            (out, &FlowOp::Bin(Bin::And, cond, _)) => (out, cond, true),
            (out, &FlowOp::Bin(Bin::Or, cond, _)) => (out, cond, false),
            (_, op) => panic!("the latch branches on {op:?}"),
        }
    }

    /// Whether instructions `equal` and `not` of `l` compare the same two
    /// values of 32 bits, the first for equality and the second for
    /// inequality.
    fn opposite_compares(l: &Lowered, equal: u32, not: u32) -> bool {
        let of = |id: u32, p: u8| match l.flow.inst(id).unwrap().2.op {
            FlowOp::Cmp(q, 32, x, y) if q == p => Some((x, y)),
            _ => None,
        };
        of(equal, pred::EQ).is_some() && of(equal, pred::EQ) == of(not, pred::NE)
    }

    /// The condition of the one `discard` in block `at`.
    fn discards(l: &Lowered, at: usize) -> Arg {
        let of = l.flow.blocks[at].insts.iter().filter_map(|i| match &i.op {
            FlowOp::Call(name, args) if name == "dx.op.discard" => Some(args[1]),
            _ => None,
        });
        let [of] = of.collect::<Vec<_>>()[..] else {
            panic!("block {at} has not one discard");
        };
        of
    }

    /// The pixel program's one conditional back edge: no block ends the
    /// program for the lanes a limit stops. The latch keeps its branch on
    /// its condition folded with the limits and leaves the loop to a block
    /// that discards the lanes that would have gone round again (the
    /// latch's condition, or, where the back edge is the side not taken,
    /// the compare of the same operands with the opposite predicate, and
    /// no `xor`) and goes on, without a branch, to the store and the
    /// export. A lane whose own condition ends the loop, also on the last
    /// iteration the limit allows, is not discarded: its store is
    /// performed and it exports its count. A lane one past the limit is
    /// discarded, runs on to the end of the program, makes its store and
    /// its export there, which the host drops, and exports in the other
    /// channels what any lane does: the value no loop writes.
    #[test]
    fn a_stopped_pixel_lane_goes_on_as_a_helper() {
        for on_set in [true, false] {
            let body = counts_to_s0(on_set);
            let l = lower_pixel(&body, defaults());
            let f = &l.flow;
            assert_eq!(l.limit_stop, None);
            let [back] = back_edges(&l, defaults())[..] else {
                panic!("one back edge: {:?}", back_edges(&l, defaults()));
            };
            let (out, cond, holds) = way_out(&l, back);
            assert_eq!(holds, on_set);
            // The block on the loop's exit: one discard, of the lanes
            // whose condition says to go round again. Where the back edge
            // is the side not taken, that is the opposite of the latch's
            // condition, computed there first: the latch compares for
            // equality, and this compares the same two for inequality.
            assert_eq!(f.preds(out), [back.from]);
            let stopped = match (&f.blocks[out].insts[..], on_set) {
                ([_], true) => cond,
                ([not, _], false) => {
                    let Arg::Inst(equal) = cond else {
                        panic!("the latch's condition is {cond:?}");
                    };
                    assert!(opposite_compares(&l, equal, not.id), "block {out}: {not:?}");
                    Arg::Inst(not.id)
                }
                (insts, _) => panic!("block {out}: {insts:?}"),
            };
            assert_eq!(discards(&l, out), stopped);
            // From it, without a branch, the rest of the program.
            let FlowTerm::Br(on) = f.blocks[out].term else {
                panic!("block {out} ends in {:?}", f.blocks[out].term);
            };
            assert!(f.calls(on).contains(&"dx.op.rawBufferStore.i32"));
            assert!(f.calls(on).contains(&"dx.op.storeOutput.f32"));
            assert_eq!(f.blocks[on].term, FlowTerm::Ret);
        }
        let limit = defaults().each;
        let lane = |on_set: bool, s0: u32| {
            let l = lower_pixel(&counts_to_s0(on_set), defaults());
            pixel(&l, defaults(), [s0, 0, 0, 0])
        };
        let ends = |s0: u32| Pixel {
            discarded: false,
            performed: vec![s0],
            dropped: vec![],
            exported: Some([s0; 4]),
        };
        assert_eq!(lane(true, 5), ends(5));
        assert_eq!(lane(false, 5), ends(5));
        // The last iteration the limit allows, and one more: the count
        // the limit cut short in the first channel, which is not the
        // console's, and the root constant in the others, which is.
        assert_eq!(lane(true, limit), ends(limit));
        let stopped = Pixel {
            discarded: true,
            performed: vec![],
            dropped: vec![limit],
            exported: Some([limit, limit + 1, limit + 1, limit + 1]),
        };
        assert_eq!(lane(true, limit + 1), stopped);
        assert_eq!(lane(false, limit + 1), stopped);
    }

    /// `do { s12 += 1; store(s12) } while (s12 != s0)`, the result s12: a
    /// store in each round.
    const STORES_IN_THE_LOOP: [u32; 8] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0x810C_810C, // loop: s_add_i32 s12, s12, 1
        0x7E00_020C, // v_mov_b32 v0, s12
        0xE070_0000, // buffer_store_dword v0, off, s[8:11], 0
        0x8002_0000,
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFA, // s_cbranch_scc1 loop
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    /// What a lane stored before it was stopped stands, and it stores
    /// nothing after, in every stage. With a loop limit of 5, a lane whose
    /// loop ends by itself stores once in each round and once after the
    /// loop. A lane the limit stops has stored in the five rounds the limit
    /// allowed. A compute lane then ends in a block that calls nothing. A
    /// vertex lane ends in the block that writes its outputs and makes no
    /// store there. A pixel lane is discarded and goes on to the store
    /// after the loop, which is after its discard: the host drops it.
    #[test]
    fn a_stopped_lane_s_earlier_stores_stand_and_it_makes_none_after() {
        let limits = Limits { each: 5, total: 13 };
        let body = STORES_IN_THE_LOOP;
        let compute = lower_valu_limited(&body, Some(limits)).unwrap();
        let ended = run(&compute, limits, [3, 0, 0, 0], 0);
        assert_eq!(ended.stores(), [1, 2, 3, 3]);
        let stopped = run(&compute, limits, [0, 0, 0, 0], 0);
        assert_eq!(stopped.stores(), [1, 2, 3, 4, 5]);
        assert_eq!(stopped.did().len(), 5, "{:?}", stopped.did());
        let stop = compute.limit_stop.unwrap();
        assert_eq!(stopped.blocks.last(), Some(&stop));
        assert!(compute.flow.blocks[stop].insts.is_empty());
        assert_eq!(compute.flow.blocks[stop].term, FlowTerm::Ret);

        let vertex = lower_vertex(&body, false, limits);
        assert_eq!(run(&vertex, limits, [3, 0, 0, 0], 0).stores(), [1, 2, 3, 3]);
        let stopped = run(&vertex, limits, [0, 0, 0, 0], 0);
        assert_eq!(stopped.stores(), [1, 2, 3, 4, 5]);
        let stop = vertex.limit_stop.unwrap();
        assert_eq!(stopped.blocks.last(), Some(&stop));
        assert!(matches!(stopped.did()[..5], [Did::Store(_), ..]));
        assert!(
            stopped.did()[5..]
                .iter()
                .all(|d| matches!(d, Did::Output(..)))
        );

        let px = lower_pixel(&body, limits);
        let ended = pixel(&px, limits, [3, 0, 0, 0]);
        assert_eq!(
            (ended.discarded, ended.performed),
            (false, vec![1, 2, 3, 3])
        );
        let stopped = pixel(&px, limits, [0, 0, 0, 0]);
        assert!(stopped.discarded && stopped.exported.is_some());
        assert_eq!(stopped.performed, [1, 2, 3, 4, 5]);
        assert_eq!(stopped.dropped, [5]);
    }

    /// A compute program around `body`, as [`lower_pixel`]'s, with thread
    /// groups of eight threads along x: after the body it stores v0 to the
    /// buffer.
    fn lower_compute(body: &[u32], limits: Limits) -> Lowered {
        let mut words = prologue();
        words.extend_from_slice(body);
        words.extend_from_slice(&STORE_V0);
        words.push(S_ENDPGM);
        let io = StageIo::Cs(ComputeIo {
            // Five user SGPRs, no LDS.
            rsrc2: 5 << 1,
            threads: [8, 1, 1],
        });
        lower_stage(HwStage::Cs, io, &words, vec![], vec![], limits)
    }

    fn thread_limit(u: &&UserData) -> bool {
        u.root == RootType::Other(USER_THREAD_LIMIT)
    }

    /// Compute with thread limits (`emit`, "Thread limits"): a thread past
    /// its limit leaves at entry through the block a lane stopped by a loop
    /// limit ends in (`Lower::limit_threads`). In a compute program that
    /// block calls nothing and returns, so it is the right end for both:
    /// it has two ways in, the entry's test and the loop's exit, where the
    /// program without thread limits has one. A thread inside its limit
    /// does what the program without thread limits does, whether its loop
    /// ends of itself or a limit stops it: a stopped one has made the
    /// stores of the rounds the limit allowed and makes none after. A
    /// thread past its limit calls nothing at all.
    #[test]
    fn a_thread_past_its_limit_and_a_stopped_compute_lane_end_in_one_block_that_only_returns() {
        let limits = Limits { each: 5, total: 13 };
        let body = STORES_IN_THE_LOOP;
        let plain = lower_compute(&body, limits);
        let limited = with_thread_limits(|| lower_compute(&body, limits));
        assert_eq!(plain.user_data.iter().filter(thread_limit).count(), 0);
        // One limit, along x, after the root constants' four dwords and
        // the table's one.
        let of: Vec<_> = limited.user_data.iter().filter(thread_limit).collect();
        let [at] = of[..] else {
            panic!("one thread limit: {of:?}");
        };
        assert_eq!((at.index, at.slot, at.dword), (5, 0, 8));

        let f = &limited.flow;
        let stop = limited.limit_stop.unwrap();
        assert!(
            f.blocks[stop].insts.is_empty(),
            "{:?}",
            f.blocks[stop].insts
        );
        assert_eq!(f.blocks[stop].term, FlowTerm::Ret);
        let FlowTerm::CondBr(_, _, left_out) = f.blocks[0].term else {
            panic!("the entry ends in {:?}", f.blocks[0].term);
        };
        assert_eq!(left_out, stop);
        assert_eq!(f.preds(stop).len(), 2, "{:?}", f.preds(stop));
        let plain_stop = plain.limit_stop.unwrap();
        assert_eq!(plain.flow.preds(plain_stop).len(), 1);

        // Thread 0 of group 0, with the host's limit dword `threads`.
        let lane = |threads: u32, s0: u32| {
            run_with(&limited, limits, [s0, 0, 0, 0], 0, [0, threads, 0, 0])
        };
        let ended = lane(1, 3);
        assert_eq!(ended.stores(), [1, 2, 3, 3]);
        assert_eq!(ended.did(), run(&plain, limits, [3, 0, 0, 0], 0).did());
        let stopped = lane(1, 0);
        assert_eq!(stopped.stores(), [1, 2, 3, 4, 5]);
        assert_eq!(stopped.did().len(), 5, "{:?}", stopped.did());
        assert_eq!(stopped.blocks.last(), Some(&stop));
        assert_eq!(stopped.did(), run(&plain, limits, [0; 4], 0).did());
        for s0 in [3, 0] {
            let left_out = lane(0, s0);
            assert!(left_out.calls.is_empty(), "{:?}", left_out.did());
            assert_eq!(left_out.blocks, [0, stop]);
        }
    }

    /// Thread limits are a compute program's. Asked of a pixel, a vertex
    /// or a mesh program they change nothing: the module is byte for byte
    /// the one without them, the host is told of no limit, and the block a
    /// stopped vertex or mesh lane ends in, which writes outputs, has no
    /// way in from the entry. No thread comes to a stage's end for stopped
    /// lanes that a loop limit did not stop.
    #[test]
    fn thread_limits_asked_of_a_stage_with_outputs_change_nothing() {
        let limits = Limits { each: 5, total: 13 };
        let body = STORES_IN_THE_LOOP;
        let same = |plain: Lowered, asked: Lowered| {
            assert_eq!(asked.dxil, plain.dxil);
            assert_eq!(asked.limit_stop, plain.limit_stop);
            assert_eq!(asked.user_data.iter().filter(thread_limit).count(), 0);
            plain
        };
        let pixel = same(
            lower_pixel(&body, limits),
            with_thread_limits(|| lower_pixel(&body, limits)),
        );
        assert_eq!(pixel.limit_stop, None);
        let vertex = same(
            lower_vertex(&body, false, limits),
            with_thread_limits(|| lower_vertex(&body, false, limits)),
        );
        let mesh = same(
            lower_mesh(64, 48, 16, false, limits),
            with_thread_limits(|| lower_mesh(64, 48, 16, false, limits)),
        );
        for l in [vertex, mesh] {
            let stop = l.limit_stop.unwrap();
            assert!(!l.flow.blocks[stop].insts.is_empty());
            assert!(!l.flow.preds(stop).contains(&0), "{:?}", l.flow.preds(stop));
        }
    }

    /// One pixel lane as this module documents it, for the models of the
    /// programs below: the limits' own model of a lane (`Lane` of the
    /// limits' tests), but a lane that is refused a back edge is stopped
    /// and is refused every one after it.
    struct Helper {
        limits: Limits,
        /// The back edges it has taken, the one being asked about included.
        taken: u32,
        stopped: bool,
    }

    impl Helper {
        fn new(limits: Limits) -> Self {
            Helper {
                limits,
                taken: 0,
                stopped: false,
            }
        }

        /// Whether the lane goes round the loop whose count is `count` once
        /// more.
        fn round(&mut self, count: &mut u32) -> bool {
            *count += 1;
            self.taken += 1;
            let on = *count < self.limits.each && self.taken < self.limits.total;
            self.stopped |= !on;
            !self.stopped
        }
    }

    /// What a pixel program's lane does, by its model: whether it is
    /// stopped, and what it has in v0 where it reaches the end of the
    /// program (a stopped lane too, as a helper); `None` where a latch
    /// that only goes back ends it.
    type HelperModel = fn(Limits, [u32; 4]) -> (bool, Option<u32>);

    /// [`TWO_LOOPS`]: a lane the first loop's limit stops leaves it, runs
    /// the second loop's first round and leaves that too.
    fn two_loops(limits: Limits, [s0, s1, ..]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s12, mut s13) = (0u32, 0u32);
        let (mut first, mut second) = (0, 0);
        loop {
            s12 = s12.wrapping_add(1);
            if s12 == s0 || !lane.round(&mut first) {
                break;
            }
        }
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s1 || !lane.round(&mut second) {
                break;
            }
        }
        (lane.stopped, Some(s12.wrapping_add(s13)))
    }

    /// [`NEST`]: a lane the inner loop's limit stops leaves it into the
    /// outer loop's body, and the outer loop's latch lets it go too.
    fn nest(limits: Limits, [s0, s1, ..]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s13, mut outer) = (0u32, 0);
        let mut s12: u32;
        loop {
            s12 = 0;
            let mut inner = 0;
            loop {
                s12 = s12.wrapping_add(1);
                if s12 == s0 || !lane.round(&mut inner) {
                    break;
                }
            }
            s13 = s13.wrapping_add(1);
            if s13 == s1 || !lane.round(&mut outer) {
                break;
            }
        }
        (lane.stopped, Some(s12.wrapping_add(s13)))
    }

    /// [`SHARED_HEADER`]: the same nest with one header for both loops.
    fn shared_header(limits: Limits, [s0, s1, ..]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s13, mut outer) = (0u32, 0);
        loop {
            let (mut s14, mut inner) = (0u32, 0);
            loop {
                s14 = s14.wrapping_add(1);
                if s14 == s0 || !lane.round(&mut inner) {
                    break;
                }
            }
            s13 = s13.wrapping_add(1);
            if s13 == s1 || !lane.round(&mut outer) {
                break;
            }
        }
        (lane.stopped, Some(s13))
    }

    /// [`THREE_EXITS`]: a stopped lane leaves by the latch's way out.
    fn three_exits(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s12, mut count) = (0u32, 0);
        loop {
            s12 = s12.wrapping_add(1);
            if s12 == s1 {
                return (false, Some(s12.wrapping_add(BROKEN)));
            }
            if s12 == s2 {
                return (false, Some(s12.wrapping_add(SECOND)));
            }
            if s12 == s0 || !lane.round(&mut count) {
                return (lane.stopped, Some(s12));
            }
        }
    }

    /// [`LATCH_IS_INNER_HEADER`]: the inner loop's latch only goes back,
    /// so a lane its limit stops ends there. The outer loop's latch is
    /// the inner loop's header, whose other side is the inner loop's
    /// body: a lane the outer limit stops goes on into the body as a
    /// helper and is ended by the inner latch.
    fn latch_is_inner_header(limits: Limits, [s0, s1, ..]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s13, mut outer) = (0u32, 0);
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                return (false, Some(s13));
            }
            let (mut s12, mut inner) = (0u32, 0);
            while s12 != s0 {
                s12 = s12.wrapping_add(1);
                if !lane.round(&mut inner) {
                    return (true, None);
                }
            }
            if !lane.round(&mut outer) {
                return (true, None);
            }
        }
    }

    /// [`BOTH_SIDES_BACK`]: the inner latch's other side is the outer
    /// loop's back edge, counted in a block of its own that only goes
    /// back. A lane the inner limit stops is discarded where it leaves
    /// the inner loop and ended by that block; so is one the outer limit
    /// stops.
    fn both_sides_back(limits: Limits, [s0, s1, ..]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s13, mut outer) = (0u32, 0);
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s1 {
                return (false, Some(s13));
            }
            let (mut s12, mut inner) = (0u32, 0);
            loop {
                s12 = s12.wrapping_add(1);
                if s12 == s0 || !lane.round(&mut inner) {
                    break;
                }
            }
            if !lane.round(&mut outer) {
                return (true, None);
            }
        }
    }

    /// [`NEST_THEN_LOOP`]: a lane stopped in the nest leaves both of its
    /// loops, and the loop after them once it has run its first round.
    fn nest_then_loop(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s13, mut s14) = (0u32, 0u32);
        let (mut outer, mut last) = (0, 0);
        let mut s12: u32;
        loop {
            s12 = 0;
            let mut inner = 0;
            loop {
                s12 = s12.wrapping_add(1);
                if s12 == s0 || !lane.round(&mut inner) {
                    break;
                }
            }
            s13 = s13.wrapping_add(1);
            if s13 == s1 || !lane.round(&mut outer) {
                break;
            }
        }
        loop {
            s14 = s14.wrapping_add(1);
            if s14 == s2 || !lane.round(&mut last) {
                break;
            }
        }
        (lane.stopped, Some(s12.wrapping_add(s13).wrapping_add(s14)))
    }

    /// [`LOOP_IN_AN_ARM`]: a lane stopped in the arm's loop comes to the
    /// join, and the loop after it, with its count used up.
    fn loop_in_an_arm(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s12, mut s13) = (0u32, 0u32);
        let (mut arm, mut last) = (0, 0);
        if s2 != 0 {
            loop {
                s12 = s12.wrapping_add(1);
                if s12 == s0 || !lane.round(&mut arm) {
                    break;
                }
            }
        }
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s1 || !lane.round(&mut last) {
                break;
            }
        }
        (lane.stopped, Some(s12.wrapping_add(s13)))
    }

    /// The programs of the limits' tests that have a model of their pixel
    /// lanes here, by their bodies.
    const HELPERS: [(&[u32], HelperModel); 8] = [
        (&TWO_LOOPS, two_loops),
        (&NEST, nest),
        (&SHARED_HEADER, shared_header),
        (&THREE_EXITS, three_exits),
        (&LATCH_IS_INNER_HEADER, latch_is_inner_header),
        (&BOTH_SIDES_BACK, both_sides_back),
        (&NEST_THEN_LOOP, nest_then_loop),
        (&LOOP_IN_AN_ARM, loop_in_an_arm),
    ];

    /// Every input of a program that reads `reads` of the root constants,
    /// each from 0 to two past a loop's limit `each` (0 is a bound no count
    /// reaches before it wraps).
    fn inputs(each: u32, reads: usize) -> impl Iterator<Item = [u32; 4]> {
        let values = each + 3;
        (0..values.pow(reads as u32)).map(move |n| {
            let at = |k: u32| {
                if (k as usize) < reads {
                    n / values.pow(k) % values
                } else {
                    0
                }
            };
            [at(0), at(1), at(2), at(3)]
        })
    }

    /// Every program of the limits' tests as a pixel program, with small
    /// limits, for every input up to two past a loop's limit: nested
    /// loops with headers of their own and with one header, lanes that
    /// leave a loop at different times and by different ways (each lane a
    /// run of its own), loops one after another and through a join.
    ///
    /// A lane the limits do not stop (by the program's model in the
    /// limits' tests, which is the compute program's) does exactly what it
    /// did before a stopped lane could go on: it is not discarded, its
    /// one store is performed, and it exports what it stored. A lane the
    /// limits stop is discarded before any store or export it still
    /// makes, performs no store, takes no back edge after its discard
    /// ([`pixel`]) and ends within the budget of every lane ([`run`]);
    /// where it reaches its export it exports, in the channels no loop
    /// writes, what every lane does. In a program without a latch that
    /// only goes back every stopped lane reaches its export, and no block
    /// ends the program for stopped lanes. For the programs with a model
    /// here the stopped lanes also compute exactly what the model says.
    #[test]
    fn stopped_pixel_lanes_do_what_their_models_say() {
        for (each, total) in SMALL {
            let limits = Limits { each, total };
            for (name, body, model, reads) in PROGRAMS {
                let l = lower_pixel(body, limits);
                let helper = HELPERS.iter().find(|h| h.0 == body).map(|h| h.1);
                let (mut stopped, mut helpers) = (0, 0);
                for constants in inputs(each, reads) {
                    let says = format!("{name}: {constants:?} with {limits:?}");
                    let got = pixel(&l, limits, constants);
                    let rest = [constants[0]; 3];
                    match model(limits, constants) {
                        Some(v) => {
                            let ended = Pixel {
                                discarded: false,
                                performed: vec![v],
                                dropped: vec![],
                                exported: Some([v, rest[0], rest[1], rest[2]]),
                            };
                            assert_eq!(got, ended, "{says}");
                        }
                        None => {
                            stopped += 1;
                            assert!(got.discarded && got.performed.is_empty(), "{says}: {got:?}");
                            if let Some(exported) = got.exported {
                                helpers += 1;
                                assert_eq!(exported[1..], rest, "{says}");
                                assert_eq!(got.dropped, [exported[0]], "{says}");
                            } else {
                                assert!(got.dropped.is_empty(), "{says}: {got:?}");
                            }
                        }
                    }
                    if let Some(helper) = helper {
                        let got = (got.discarded, got.exported.map(|e| e[0]));
                        assert_eq!(got, helper(limits, constants), "{says}");
                    }
                }
                assert!(stopped > 0, "{name} with {limits:?}: no lane stopped");
                if l.limit_stop.is_none() {
                    assert_eq!(helpers, stopped, "{name} with {limits:?}");
                }
            }
        }
    }

    /// [`lower_pixel`]'s lane around a body that makes no store of its
    /// own, by a model of the body: its one store and its export are made
    /// where it reaches the end of the program, and are dropped if it was
    /// stopped.
    fn modelled(model: HelperModel, limits: Limits, constants: [u32; 4]) -> Pixel {
        let (stopped, v0) = model(limits, constants);
        let stores: Vec<u32> = v0.into_iter().collect();
        let (performed, dropped) = if stopped {
            (Vec::new(), stores)
        } else {
            (stores, Vec::new())
        };
        let s0 = constants[0];
        Pixel {
            discarded: stopped,
            performed,
            dropped,
            exported: v0.map(|v| [v, s0, s0, s0]),
        }
    }

    /// [`EITHER_THEN_LOOP`]: a lane the first loop's limit stops leaves it
    /// with neither bound met, runs the second loop's first round and
    /// leaves that too.
    fn either_then_loop(limits: Limits, [s0, s1, s2, _]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut s12, mut s13) = (0u32, 0u32);
        let (mut first, mut second) = (0, 0);
        loop {
            s12 = s12.wrapping_add(1);
            if s12 == s0 || s12 == s1 || !lane.round(&mut first) {
                break;
            }
        }
        loop {
            s13 = s13.wrapping_add(1);
            if s13 == s2 || !lane.round(&mut second) {
                break;
            }
        }
        (lane.stopped, Some(s12.wrapping_add(s13)))
    }

    /// `do { v0 += 1 } while (!(v1 <= v0))` with the bound in the lane's
    /// v1, the result v0: a loop each lane leaves for itself as a `break`
    /// does it, by a compare of the lanes that are done (`compare`, a
    /// `v_cmp` of v1 with v0 into VCC) taken out of EXEC, and a branch back
    /// while a lane is left. The latch's condition is that compare, and
    /// its back edge the side on which it does not hold.
    fn per_lane_break(compare: u32) -> [u32; 7] {
        [
            0xBE8D_037E, // s_mov_b32 s13, exec_lo
            0x7E00_0280, // v_mov_b32 v0, 0
            0x4A00_0081, // loop: v_add_nc_u32 v0, 1, v0
            compare,     // v_cmp_le_* vcc, v1, v0
            0x8A7E_6A7E, // s_andn2_b32 exec_lo, exec_lo, vcc_lo
            0xBF89_FFFC, // s_cbranch_execnz loop
            0xBEFE_030D, // s_mov_b32 exec_lo, s13
        ]
    }

    const V_CMP_LE_U32: u32 = 0x7D86_0101;
    const V_CMP_LE_F32: u32 = 0x7C06_0101;

    /// [`per_lane_break`] of unsigned numbers: a stopped lane leaves with
    /// the count the limit allowed. It is the model of the float form too
    /// for the bounds the tests give that one: the bits of floats that are
    /// not negative are in the floats' own order (the smallest counts are
    /// the smallest floats above 0), and a bound that is no number, which
    /// no float is at or above, has bits above every count a lane reaches.
    fn per_lane_breaks(limits: Limits, [s0, ..]: [u32; 4]) -> (bool, Option<u32>) {
        let mut lane = Helper::new(limits);
        let (mut v0, mut count) = (0u32, 0);
        loop {
            v0 = v0.wrapping_add(1);
            if s0 <= v0 || !lane.round(&mut count) {
                break;
            }
        }
        (lane.stopped, Some(v0))
    }

    /// `do { s12 += 1; if (s12 != s0) continue; store(s12); s13 += 1 }
    /// while (s13 != s1)`, the result s13: a conditional `continue` with a
    /// store in the part of the body it skips. By the counting rule the
    /// `continue` is a loop of its own at the loop's header (`limit`,
    /// "Loops"), and the other side of its branch is the rest of the body.
    const CONTINUE_PAST_A_STORE: [u32; 12] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBE8D_0380, // s_mov_b32 s13, 0
        0x810C_810C, // loop: s_add_i32 s12, s12, 1
        0xBF07_000C, // s_cmp_lg_u32 s12, s0
        0xBF85_FFFD, // s_cbranch_scc1 loop
        0x7E00_020C, // v_mov_b32 v0, s12
        0xE070_0000, // buffer_store_dword v0, off, s[8:11], 0
        0x8002_0000,
        0x810D_810D, // s_add_i32 s13, s13, 1
        0xBF07_010D, // s_cmp_lg_u32 s13, s1
        0xBF85_FFF7, // s_cbranch_scc1 loop
        0x7E00_020D, // v_mov_b32 v0, s13
    ];

    /// [`CONTINUE_PAST_A_STORE`] as a pixel program. A lane refused at the
    /// `continue` is discarded there and falls into the rest of the body,
    /// which the `continue` was skipping for it: it makes that round's
    /// store, of a count that is not the bound, and then the loop's own
    /// latch lets it out, and it makes the store after the loop. Both are
    /// after its discard. A lane the loop's own latch refuses has made
    /// its stores of the rounds before, which stand, and makes only the
    /// last one after its discard.
    fn continue_past_a_store(limits: Limits, [s0, s1, ..]: [u32; 4]) -> Pixel {
        let mut lane = Helper::new(limits);
        let (mut s12, mut s13) = (0u32, 0u32);
        let (mut skips, mut rounds) = (0, 0);
        let (mut performed, mut dropped) = (Vec::new(), Vec::new());
        let mut store = |lane: &Helper, x: u32| {
            if lane.stopped {
                dropped.push(x);
            } else {
                performed.push(x);
            }
        };
        loop {
            s12 = s12.wrapping_add(1);
            if s12 != s0 && lane.round(&mut skips) {
                continue;
            }
            store(&lane, s12);
            s13 = s13.wrapping_add(1);
            if s13 == s1 || !lane.round(&mut rounds) {
                break;
            }
            // The loop's end enters the `continue`'s loop again.
            skips = 0;
        }
        store(&lane, s13);
        Pixel {
            discarded: lane.stopped,
            performed,
            dropped,
            exported: Some([s13, s0, s0, s0]),
        }
    }

    /// Pixel programs whose latches are of kinds the limits' programs do
    /// not have, each lane against an exact model, with small limits and
    /// every input up to two past a loop's limit:
    ///
    /// * a nest whose inner latch goes back on the side not taken, so the
    ///   block that discards its stopped lanes with the opposite compare
    ///   is inside the outer loop, and two loops of which the first does
    ///   (the models are the ones of the same programs with the latch the
    ///   other way round: the programs do the same);
    /// * a latch whose condition is no compare, discarded on its `xor`;
    /// * a loop each lane leaves by a mask of the lanes that are done, as
    ///   the console's compiler writes a `break`, comparing unsigned
    ///   numbers and comparing floats (the discard then reads the float
    ///   compare's opposite; run on this host's `f32`), and of the float
    ///   form the lane whose bound is no number: it is never done, so the
    ///   limit stops it, and its discard must hold where neither operand
    ///   order does;
    /// * a conditional `continue`: the lane refused there runs the rest
    ///   of the loop's body as a helper, the store in it included, and is
    ///   let out by the loop's own latch.
    #[test]
    fn stopped_pixel_lanes_at_other_latches_do_what_their_models_say() {
        type Lane = fn(Limits, [u32; 4]) -> Pixel;
        let programs: [(&str, Vec<u32>, Lane, usize); 6] = [
            (
                "a nest, the inner latch back on SCC clear",
                back_on_clear(&NEST, 4),
                |limits, constants| modelled(nest, limits, constants),
                2,
            ),
            (
                "two loops, the first back on SCC clear",
                back_on_clear(&TWO_LOOPS, 4),
                |limits, constants| modelled(two_loops, limits, constants),
                2,
            ),
            (
                "either bound, then a loop",
                EITHER_THEN_LOOP.to_vec(),
                |limits, constants| modelled(either_then_loop, limits, constants),
                3,
            ),
            (
                "per lane, by a break mask",
                per_lane_break(V_CMP_LE_U32).to_vec(),
                |limits, constants| modelled(per_lane_breaks, limits, constants),
                1,
            ),
            (
                "per lane, by a break mask of float compares",
                per_lane_break(V_CMP_LE_F32).to_vec(),
                |limits, constants| modelled(per_lane_breaks, limits, constants),
                1,
            ),
            (
                "a continue past a store",
                CONTINUE_PAST_A_STORE.to_vec(),
                continue_past_a_store,
                2,
            ),
        ];
        for (each, total) in SMALL {
            let limits = Limits { each, total };
            for (name, body, model, reads) in &programs {
                let l = lower_pixel(body, limits);
                assert_eq!(l.limit_stop, None, "{name}");
                let (mut stopped, mut ended) = (0, 0);
                for constants in inputs(each, *reads) {
                    let got = pixel(&l, limits, constants);
                    if got.discarded {
                        stopped += 1;
                    } else {
                        ended += 1;
                    }
                    let says = format!("{name}: {constants:?} with {limits:?}");
                    assert_eq!(got, model(limits, constants), "{says}");
                }
                assert!(stopped > 0 && ended > 0, "{name} with {limits:?}");
            }
            // A bound that is no number: the lane's compare never holds,
            // the limit stops it, and the opposite compare, which holds
            // for an unordered pair, discards it.
            let l = lower_pixel(&per_lane_break(V_CMP_LE_F32), limits);
            let unordered = [STOPPED_POSITION, 0, 0, 0];
            let got = pixel(&l, limits, unordered);
            assert!(
                got.discarded && got.performed.is_empty(),
                "{limits:?}: {got:?}"
            );
            assert_eq!(
                got,
                modelled(per_lane_breaks, limits, unordered),
                "{limits:?}"
            );
        }
        // The lane of the module's documentation: refused at its third
        // `continue` by a lane's limit of 3, it stores the 3 the `continue`
        // was skipping the store of, counts one round of the loop and
        // leaves, and stores and exports that 1: all after its discard.
        let limits = Limits {
            each: 1000,
            total: 3,
        };
        let l = lower_pixel(&CONTINUE_PAST_A_STORE, limits);
        let refused = Pixel {
            discarded: true,
            performed: vec![],
            dropped: vec![3, 1],
            exported: Some([1, 0, 0, 0]),
        };
        assert_eq!(pixel(&l, limits, [0, 5, 0, 0]), refused);
        assert_eq!(continue_past_a_store(limits, [0, 5, 0, 0]), refused);
        // One whose count meets the bound in the first round makes that
        // round's store, which stands. Refused at the second `continue`
        // after it, it stores the 3 that one was skipping and the 2 it
        // leaves the loop with, after its discard.
        let later = Pixel {
            discarded: true,
            performed: vec![1],
            dropped: vec![3, 2],
            exported: Some([2, 1, 1, 1]),
        };
        assert_eq!(pixel(&l, limits, [1, 5, 0, 0]), later);
        assert_eq!(continue_past_a_store(limits, [1, 5, 0, 0]), later);
    }

    /// The value the phi `phi` of `l` takes along the edge from block
    /// `from`.
    fn brought(l: &Lowered, phi: u32, from: usize) -> Arg {
        let FlowOp::Phi(incoming) = &l.flow.inst(phi).unwrap().2.op else {
            panic!("instruction {phi} is no phi")
        };
        incoming.iter().find(|x| x.1 == from).unwrap().0
    }

    /// The select in block `at`, on a latch's way out of its loop, that
    /// gives the lanes their count of back edges from there, if the block
    /// has one: its condition, the count where that holds, the count
    /// where it does not, and its number.
    fn used_up(l: &Lowered, at: usize) -> Option<([Arg; 3], u32)> {
        l.flow.blocks[at].insts.iter().find_map(|i| match i.op {
            FlowOp::Select(c, held, not) => Some(([c, held, not], i.id)),
            _ => None,
        })
    }

    /// `do { s12 += 1 } while (s12 != s0 && s12 != s1); do { s13 += 1 }
    /// while (s13 != s2)`, the result s12 + s13. The first loop's latch is
    /// a block of its own that two compares reach, so its condition is a
    /// join of them and no compare, and its back edge is the side not
    /// taken.
    const EITHER_THEN_LOOP: [u32; 12] = [
        0xBE8C_0380, // s_mov_b32 s12, 0
        0xBE8D_0380, // s_mov_b32 s13, 0
        0x810C_810C, // first: s_add_i32 s12, s12, 1
        0xBF06_000C, // s_cmp_eq_u32 s12, s0
        0xBF85_0001, // s_cbranch_scc1 met
        0xBF06_010C, // s_cmp_eq_u32 s12, s1
        0xBF84_FFFB, // met: s_cbranch_scc0 first
        0x810D_810D, // second: s_add_i32 s13, s13, 1
        0xBF07_020D, // s_cmp_lg_u32 s13, s2
        0xBF85_FFFD, // s_cbranch_scc1 second
        0x810C_0D0C, // s_add_i32 s12, s12, s13
        0x7E00_020C, // v_mov_b32 v0, s12
    ];

    /// `body` with the latch that ends at its word `at` (a compare for
    /// inequality, then a branch back on SCC set) turned round: a compare
    /// for equality, then a branch back on SCC clear. The program does
    /// what it did; its back edge is the other side of the latch's branch.
    fn back_on_clear(body: &[u32], at: usize) -> Vec<u32> {
        let mut body = body.to_vec();
        // s_cmp_lg_u32 and s_cbranch_scc1, to s_cmp_eq_u32 and
        // s_cbranch_scc0.
        assert_eq!((body[at - 1] >> 16, body[at] >> 16), (0xBF07, 0xBF85));
        body[at - 1] ^= 0x0001_0000;
        body[at] ^= 0x0001_0000;
        body
    }

    /// Where a loop can still follow, the block that discards the stopped
    /// lanes also gives them the count of back edges of a lane whose
    /// limit is used up, one below [`LOOP_TOTAL_LIMIT`], and the others
    /// the count they have; where none can, it does not. The select is on
    /// the latch's own condition in either sense, so only the discard
    /// reads a negation. With two loops one after the other the second
    /// loop's header takes that count on its way in; with a loop in one
    /// arm of a branch the join after it does, beside the 0 of the lanes
    /// that went round the arm.
    ///
    /// The discard is of the latch's condition where the back edge is the
    /// side on which it holds. Where it is the other side, the discard is
    /// of the opposite compare if the condition is a compare, and of an
    /// `xor` of the condition if it is not (here a join of two compares).
    ///
    /// [`LOOP_TOTAL_LIMIT`]: super::super::LOOP_TOTAL_LIMIT
    #[test]
    fn a_stopped_pixel_lane_s_count_of_back_edges_is_used_up() {
        let spent = Arg::Int(u64::from(defaults().total - 1));
        let l = lower_pixel(&TWO_LOOPS, defaults());
        assert_eq!(l.limit_stop, None);
        let [first, second] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        let (out, cond, holds) = way_out(&l, first);
        let (of, count) = used_up(&l, out).expect("the first loop's exit");
        assert!(holds && discards(&l, out) == cond);
        assert_eq!(of, [cond, spent, Arg::Inst(first.total)]);
        assert_eq!(l.flow.succs(out), [second.to]);
        assert_eq!(brought(&l, second.total, out), Arg::Inst(count));
        let (out, _) = leaves(&l, second.from, second.to);
        assert!(l.flow.calls(out) == ["dx.op.discard"] && used_up(&l, out).is_none());

        // The first loop's back edge the side not taken: the stopped
        // lanes are the ones in which the condition does not hold.
        let l = lower_pixel(&back_on_clear(&TWO_LOOPS, 4), defaults());
        let [first, second] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        let (out, cond, holds) = way_out(&l, first);
        let (of, count) = used_up(&l, out).expect("the first loop's exit");
        assert!(!holds);
        assert_eq!(of, [cond, Arg::Inst(first.total), spent]);
        assert_eq!(brought(&l, second.total, out), Arg::Inst(count));
        let (Arg::Inst(equal), Arg::Inst(not)) = (cond, discards(&l, out)) else {
            panic!("block {out} discards on a constant");
        };
        assert!(opposite_compares(&l, equal, not));
        let no_xor = |b: usize| {
            let mut insts = l.flow.blocks[b].insts.iter();
            !insts.any(|i| matches!(i.op, FlowOp::Bin(Bin::Xor, ..)))
        };
        assert!((0..l.flow.blocks.len()).all(no_xor));

        // A condition that is no compare: the discard is of its `xor`,
        // the select still of the condition itself.
        let l = lower_pixel(&EITHER_THEN_LOOP, defaults());
        let [first, second] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        let (out, cond, holds) = way_out(&l, first);
        let Arg::Inst(joined) = cond else {
            panic!("the latch's condition is {cond:?}");
        };
        assert!(!holds && matches!(l.flow.inst(joined).unwrap().2.op, FlowOp::Phi(_)));
        let (of, count) = used_up(&l, out).expect("the first loop's exit");
        assert_eq!(of, [cond, Arg::Inst(first.total), spent]);
        assert_eq!(brought(&l, second.total, out), Arg::Inst(count));
        let [not, _, _] = &l.flow.blocks[out].insts[..] else {
            panic!("block {out}: {:?}", l.flow.blocks[out].insts);
        };
        assert!(matches!(not.op, FlowOp::Bin(Bin::Xor, c, Arg::Int(1)) if c == cond));
        assert_eq!(discards(&l, out), Arg::Inst(not.id));

        // A float compare: ordered "less or equal" in the latch, and its
        // opposite, unordered "greater", for the discard. (Its lanes are
        // run in `stopped_pixel_lanes_at_other_latches_do_what_their_models_say`.)
        let l = lower_pixel(&per_lane_break(V_CMP_LE_F32), defaults());
        let [back] = back_edges(&l, defaults())[..] else {
            panic!("one back edge: {:?}", back_edges(&l, defaults()));
        };
        let (out, cond, holds) = way_out(&l, back);
        let (Arg::Inst(done), Arg::Inst(not)) = (cond, discards(&l, out)) else {
            panic!("block {out} discards on a constant");
        };
        let of = |id: u32| match l.flow.inst(id).unwrap().2.op {
            FlowOp::Cmp(p, 32, x, y) => (p, x, y),
            ref op => panic!("{op:?}"),
        };
        let ((ole, x, y), (ugt, not_x, not_y)) = (of(done), of(not));
        assert!(!holds && (ole, ugt) == (pred::OLE, pred::UGT));
        assert_eq!((x, y), (not_x, not_y));
        assert_eq!(l.flow.blocks[out].insts.len(), 2);

        let l = lower_pixel(&LOOP_IN_AN_ARM, defaults());
        let [arm, last] = back_edges(&l, defaults())[..] else {
            panic!("two back edges: {:?}", back_edges(&l, defaults()));
        };
        let (out, cond, _) = way_out(&l, arm);
        let (of, count) = used_up(&l, out).expect("the exit of the arm's loop");
        assert_eq!(of, [cond, spent, Arg::Inst(arm.total)]);
        // The join's phi of the lane's count, which the last loop's
        // header takes on its way in.
        let FlowOp::Phi(into_last) = &l.flow.inst(last.total).unwrap().2.op else {
            unreachable!()
        };
        let [joined] = into_last
            .iter()
            .filter_map(|&(x, from)| match x {
                Arg::Inst(id) if from != last.from => Some(id),
                _ => None,
            })
            .collect::<Vec<_>>()[..]
        else {
            panic!("into the last loop: {into_last:?}");
        };
        let FlowOp::Phi(into_join) = &l.flow.inst(joined).unwrap().2.op else {
            panic!("instruction {joined} is no phi")
        };
        let mut values: Vec<Arg> = into_join.iter().map(|x| x.0).collect();
        values.sort_by_key(|&x| x != Arg::Int(0));
        assert_eq!(values, [Arg::Int(0), Arg::Inst(count)]);
    }

    /// A stopped helper lane is ended by the limit on all of a lane's
    /// loops, at the real limits. A lane the first loop's limit stops
    /// finds its count used up at the second loop, which reads a bound of
    /// 0 and would keep it for another 2^20 rounds, and leaves it after
    /// one. A lane the lane's own limit stops in a nest of 2,048 by 2,049
    /// iterations leaves both of its loops and the loop after them the
    /// same way. Both run to the end of the program within the budget
    /// every lane has, and take no back edge after their discard
    /// ([`pixel`]).
    #[test]
    fn a_stopped_pixel_lane_takes_no_further_back_edge() {
        let Limits { each, total } = defaults();
        let l = lower_pixel(&TWO_LOOPS, defaults());
        let stopped = pixel(&l, defaults(), [0, 0, 0, 0]);
        assert!(stopped.discarded && stopped.performed.is_empty());
        assert_eq!(stopped.exported.map(|e| e[0]), Some(each + 1));
        assert_eq!(two_loops(defaults(), [0; 4]), (true, Some(each + 1)));
        // The same lane with the second loop's bound the first round's
        // count leaves that loop of itself, with the same values.
        assert_eq!(
            pixel(&l, defaults(), [0, 1, 0, 0]).exported,
            stopped.exported
        );

        assert!(2048 * 2048 == total && 2049 < each);
        let l = lower_pixel(&NEST_THEN_LOOP, defaults());
        let constants = [2048, 2049, 0, 0];
        let stopped = pixel(&l, defaults(), constants);
        assert!(stopped.discarded && stopped.performed.is_empty());
        // The inner loop's count and the outer loop's where the lane's
        // limit refused the outer back edge, and one round of the last.
        assert_eq!(stopped.exported.map(|e| e[0]), Some(2048 + 2048 + 1));
        assert_eq!(
            nest_then_loop(defaults(), constants),
            (true, Some(2048 + 2048 + 1))
        );
    }

    /// What vertex element `id` of [`lower_vertex`]'s programs holds in a
    /// lane that ends of itself with `v` in v0 and `s0` in the first root
    /// constant, the parameter (element 1) exported `early` or not.
    fn vertex_output(id: u32, v: u32, s0: u32, early: bool) -> [u32; 4] {
        if id == 1 && early {
            [s0; 4]
        } else {
            [v, s0, s0, s0]
        }
    }

    /// Checks the block a stopped vertex lane of `l` ends in: it writes a
    /// quiet NaN to each channel of the position (element 0), then 0 to
    /// each channel of the parameter, and returns.
    fn vertex_stop(l: &Lowered) -> usize {
        let stop = l.limit_stop.expect("a loop that can stop a lane");
        let wrote: Vec<[Arg; 4]> = l.flow.blocks[stop]
            .insts
            .iter()
            .map(|i| match &i.op {
                FlowOp::Call(name, args) if name == "dx.op.storeOutput.f32" => {
                    [args[1], args[2], args[3], args[4]]
                }
                op => panic!("block {stop}: {op:?}"),
            })
            .collect();
        let all = |id: u64, bits: u32| {
            let channel = |col| [id, 0, col, u64::from(bits)].map(Arg::Int);
            (0..4).map(channel).collect::<Vec<_>>()
        };
        assert_eq!(wrote, [all(0, STOPPED_POSITION), all(1, 0)].concat());
        assert_eq!(l.flow.blocks[stop].term, FlowTerm::Ret);
        stop
    }

    /// Every program of the limits' tests as a vertex program, with small
    /// limits and every input up to two past a loop's limit, its parameter
    /// exported after the loops and, in a second program, before them.
    ///
    /// A lane the limits do not stop makes its store and exports what it
    /// did before. A lane the limits stop makes no store and ends in the
    /// one block that writes NaN to its position and 0 to its parameter
    /// ([`vertex_stop`]): those are the last values written, also where
    /// the parameter was exported before the loop that stopped the lane,
    /// so no output is left unwritten and none keeps a value from before
    /// the stop.
    #[test]
    fn a_stopped_vertex_lane_writes_a_position_that_is_no_number() {
        let nan = [STOPPED_POSITION; 4];
        assert!(f32::from_bits(STOPPED_POSITION).is_nan());
        for (each, total) in SMALL {
            let limits = Limits { each, total };
            for (name, body, model, reads) in PROGRAMS {
                for early in [false, true] {
                    let l = lower_vertex(body, early, limits);
                    let stop = vertex_stop(&l);
                    let mut stopped = 0;
                    for constants in inputs(each, reads) {
                        let says = format!("{name}: {constants:?} with {limits:?}");
                        let lane = run(&l, limits, constants, 0);
                        let outputs = [lane.output(0), lane.output(1)];
                        match model(limits, constants) {
                            Some(v) => {
                                let wrote = |id| Some(vertex_output(id, v, constants[0], early));
                                assert_eq!(lane.stores(), [v], "{says}");
                                assert_eq!(outputs, [wrote(0), wrote(1)], "{says}");
                                assert!(!lane.blocks.contains(&stop), "{says}");
                            }
                            None => {
                                stopped += 1;
                                assert!(lane.stores().is_empty(), "{says}");
                                assert_eq!(outputs, [Some(nan), Some([0; 4])], "{says}");
                                assert_eq!(lane.blocks.last(), Some(&stop), "{says}");
                            }
                        }
                    }
                    assert!(stopped > 0, "{name} with {limits:?}: no lane stopped");
                }
            }
        }
        // At the real limit: the last iteration it allows, and one more.
        let l = lower_vertex(&counts_to_s0(true), true, defaults());
        vertex_stop(&l);
        let limit = defaults().each;
        let ended = run(&l, defaults(), [limit, 0, 0, 0], 0);
        assert_eq!(ended.stores(), [limit]);
        assert_eq!(ended.output(0), Some([limit; 4]));
        let stopped = run(&l, defaults(), [limit + 1, 0, 0, 0], 0);
        assert!(stopped.stores().is_empty());
        assert_eq!(
            [stopped.output(0), stopped.output(1)],
            [Some(nan), Some([0; 4])]
        );
        // The parameter it exported before the loop was written, and
        // written over.
        assert!(stopped.did().contains(&Did::Output(1, 0, limit + 1)));
    }

    /// A stopped vertex lane writes the components the program's own
    /// exports write, and no other: 0 as an integer to an integer
    /// element's, 0 to the two channels a float element is exported in
    /// and not to the two it is not, and nothing to an element the
    /// program never exports. The position's are NaN. So the block has a
    /// store for each store the program's exports make, and the masks of
    /// the signature are what the exports alone made them.
    #[test]
    fn a_stopped_vertex_lane_writes_the_components_its_program_writes() {
        let limits = Limits { each: 5, total: 13 };
        let mut words = prologue();
        words.extend_from_slice(&counts_to_s0(true));
        words.extend_from_slice(&STORE_V0);
        words.extend_from_slice(&[
            0xF800_0203, // exp param0, v0, v1, off, off
            0x0000_0100,
            0xF800_0215, // exp param1, v0, off, v1, off
            0x0001_0000,
            0xF800_08CF, // exp pos0, v0, v1, v1, v1 done
            0x0101_0100,
            S_ENDPGM,
        ]);
        let output = |row: u8, param: u8| VsOutput {
            usage: Usage::Generic,
            row,
            param,
            fp16: false,
            pack2: false,
        };
        let io = StageIo::Vs {
            inputs: vec![],
            outputs: vec![output(1, 0), output(2, 1)],
        };
        // An unsigned element of two columns, a float one exported in its
        // first and third channels, and one no export names.
        let mut unsigned = element("TEXCOORD", 0, 1);
        (unsigned.comp, unsigned.cols, unsigned.interp) = (5, 2, 1);
        let outputs = vec![
            element("SV_Position", 3, 0),
            unsigned,
            element("COLOR", 0, 2),
            element("NORMAL", 0, 3),
        ];
        let l = lower_stage(HwStage::Vs, io, &words, outputs, vec![], limits);
        let stop = l.limit_stop.expect("a loop that can stop a lane");
        let wrote: Vec<(&str, [Arg; 4])> = l.flow.blocks[stop]
            .insts
            .iter()
            .map(|i| match &i.op {
                FlowOp::Call(name, args) => (name.as_str(), [args[1], args[2], args[3], args[4]]),
                op => panic!("block {stop}: {op:?}"),
            })
            .collect();
        let store = |name: &'static str, id: u64, col: u64, bits: u32| {
            (name, [id, 0, col, u64::from(bits)].map(Arg::Int))
        };
        let float = "dx.op.storeOutput.f32";
        let want = [
            store(float, 0, 0, STOPPED_POSITION),
            store(float, 0, 1, STOPPED_POSITION),
            store(float, 0, 2, STOPPED_POSITION),
            store(float, 0, 3, STOPPED_POSITION),
            store("dx.op.storeOutput.i32", 1, 0, 0),
            store("dx.op.storeOutput.i32", 1, 1, 0),
            store(float, 2, 0, 0),
            store(float, 2, 2, 0),
        ];
        assert_eq!(wrote, want);
        assert_eq!(l.flow.blocks[stop].term, FlowTerm::Ret);
        // A lane that ends of itself writes those components and no
        // other, each once (its count and the first root constant, both 3
        // here); a stopped lane writes them in that block and nowhere
        // else.
        let written = |lane: &Run| {
            let outputs = lane.did().into_iter().filter_map(|d| match d {
                Did::Output(id, col, x) => Some(((id, col), x)),
                _ => None,
            });
            outputs.collect::<Vec<_>>()
        };
        let ended = written(&run(&l, limits, [3, 0, 0, 0], 0));
        let mut components: Vec<(u32, u32)> = ended.iter().map(|w| w.0).collect();
        components.sort_unstable();
        let stopped = written(&run(&l, limits, [0, 0, 0, 0], 0));
        assert_eq!(stopped.iter().map(|w| w.0).collect::<Vec<_>>(), components);
        let bits = |(id, _): (u32, u32)| if id == 0 { STOPPED_POSITION } else { 0 };
        assert!(stopped.iter().all(|&(at, x)| x == bits(at)));
        assert!(ended.len() == components.len() && ended.iter().all(|w| w.1 == 3));
    }

    /// Triangle (0, 1, 2), each corner's index in its 10-bit field.
    const TRIANGLE: u32 = 1 << 10 | 2 << 20;

    /// A triangle mesh program of `threads` lanes with `vertices` vertices
    /// and `primitives` primitives: `do { s13 += 1 } while (s13 != s8)` (a
    /// mesh program's user data begins at s8: the first root constant),
    /// and the lane's exports, each with s13 in every channel: its
    /// primitive (0, 1, 2), a per-primitive parameter, a vertex parameter
    /// and the position. With `early` the exports come before the loop
    /// (s13 is 0 there).
    fn lower_mesh(
        threads: u32,
        vertices: u32,
        primitives: u32,
        early: bool,
        limits: Limits,
    ) -> Lowered {
        let looped = [
            0x810D_810D, // loop: s_add_i32 s13, s13, 1
            0xBF07_080D, // s_cmp_lg_u32 s13, s8
            0xBF85_FFFD, // s_cbranch_scc1 loop
        ];
        let exports = [
            0x7E08_020D, // v_mov_b32 v4, s13
            0x7E12_02FF, // v_mov_b32 v9, TRIANGLE
            TRIANGLE,
            0xF800_0941, // exp prim, v9, off, off, off done
            0x0000_0009,
            0xF800_021F, // exp param1, v4, v4, v4, v4
            0x0404_0404,
            0xF800_020F, // exp param0, v4, v4, v4, v4
            0x0404_0404,
            0xF800_08CF, // exp pos0, v4, v4, v4, v4 done
            0x0404_0404,
        ];
        let mut words = vec![0xBE8D_0380]; // s_mov_b32 s13, 0
        if early {
            words.extend_from_slice(&exports);
            words.extend_from_slice(&looped);
        } else {
            words.extend_from_slice(&looped);
            words.extend_from_slice(&exports);
        }
        words.push(S_ENDPGM);
        let output = |row: u8, param: u8| VsOutput {
            usage: Usage::Generic,
            row,
            param,
            fp16: false,
            pack2: false,
        };
        let io = StageIo::Ms(MeshIo {
            // Parameter 0 is vertex row 1; parameter 1 is per-primitive
            // row 0.
            outputs: vec![output(1, 0), output(0, 0x81)],
            clip_cntl: 0,
            threads,
            max_vertices: vertices,
            max_primitives: primitives,
            topology: 2,
            lds_bytes: 0,
        });
        let outputs = vec![element("SV_Position", 3, 0), element("TEXCOORD", 0, 1)];
        let mut per_primitive = element("COLOR", 0, 0);
        per_primitive.interp = 1;
        lower_stage(
            HwStage::Ms,
            io,
            &words,
            outputs,
            vec![per_primitive],
            limits,
        )
    }

    /// What a mesh lane leaves of its own outputs when it ends: its
    /// vertex's position and parameter, its primitive's parameter, and
    /// the primitive it emitted last (its index and three vertices).
    fn mesh_lane(lane: &Run) -> ([Option<[u32; 4]>; 3], Option<[u32; 4]>) {
        let mut per_primitive = [None; 4];
        let mut indices = None;
        for &(_, did) in &lane.calls {
            match did {
                Did::PerPrimitive(0, col, x, _) => per_primitive[col as usize] = Some(x),
                Did::Indices(emitted) => indices = Some(emitted),
                _ => {}
            }
        }
        let [Some(x), Some(y), Some(z), Some(w)] = per_primitive else {
            return ([lane.output(0), lane.output(1), None], indices);
        };
        (
            [lane.output(0), lane.output(1), Some([x, y, z, w])],
            indices,
        )
    }

    /// A mesh lane a loop limit stops ends in a block that writes its own
    /// vertex (NaN to the position, 0 to the parameter), emits its own
    /// primitive with all three indices 0 and writes 0 to its
    /// per-primitive parameter: the last values written, also where the
    /// lane exported before the loop. A lane the limit does not stop
    /// writes what its program says. Every write names the lane's own
    /// index. In a group of more lanes than vertices and primitives, a
    /// stopped lane whose index is past them writes nothing. Where the
    /// two counts differ each write is behind the test of its own count:
    /// a lane with a vertex and no primitive writes its vertex only, and
    /// a lane with a primitive and no vertex only culls its primitive and
    /// writes 0 to that one's parameter.
    #[test]
    fn a_stopped_mesh_lane_writes_its_vertex_and_culls_its_primitive() {
        let limits = Limits { each: 5, total: 13 };
        let nan = [STOPPED_POSITION; 4];
        for early in [false, true] {
            let l = lower_mesh(32, 32, 32, early, limits);
            let stop = l.limit_stop.expect("a loop that can stop a lane");
            let f = &l.flow;
            assert_eq!(
                f.calls(stop),
                [
                    ["dx.op.storeVertexOutput.f32"; 8].as_slice(),
                    &["dx.op.emitIndices"],
                    &["dx.op.storePrimitiveOutput.f32"; 4],
                ]
                .concat()
            );
            assert_eq!(f.blocks[stop].term, FlowTerm::Ret);
            for thread in [0, 7, 31] {
                let own = |lane: &Run| {
                    lane.did().iter().all(|&d| match d {
                        Did::Vertex(.., at) | Did::PerPrimitive(.., at) => at == thread,
                        Did::Indices(emitted) => emitted[0] == thread,
                        _ => false,
                    })
                };
                let ended = run(&l, limits, [3, 0, 0, 0], thread);
                let v = if early { 0 } else { 3 };
                let wrote = Some([v; 4]);
                assert_eq!(
                    mesh_lane(&ended),
                    ([wrote; 3], Some([thread, 0, 1, 2])),
                    "thread {thread}"
                );
                assert!(own(&ended) && !ended.blocks.contains(&stop));
                for s8 in [0, 6] {
                    let stopped = run(&l, limits, [s8, 0, 0, 0], thread);
                    assert_eq!(
                        mesh_lane(&stopped),
                        (
                            [Some(nan), Some([0; 4]), Some([0; 4])],
                            Some([thread, 0, 0, 0])
                        ),
                        "thread {thread}"
                    );
                    assert!(own(&stopped));
                    assert_eq!(stopped.blocks.last(), Some(&stop));
                    // Where it exported before the loop, its triangle
                    // was emitted and is culled again.
                    let emitted = Did::Indices([thread, 0, 1, 2]);
                    assert_eq!(stopped.did().contains(&emitted), early);
                }
            }
        }
        // 64 lanes, 32 vertices and primitives: the writes are behind a
        // test of the lane's index.
        let l = lower_mesh(64, 32, 32, false, limits);
        let stop = l.limit_stop.expect("a loop that can stop a lane");
        let FlowTerm::CondBr(Arg::Inst(within), ..) = l.flow.blocks[stop].term else {
            panic!("block {stop} ends in {:?}", l.flow.blocks[stop].term);
        };
        let below = pred::IULT;
        assert!(matches!(
            l.flow.inst(within).unwrap().2.op,
            FlowOp::Cmp(p, 32, _, Arg::Int(32)) if p == below
        ));
        assert!(l.flow.calls(stop).is_empty());
        let stopped = run(&l, limits, [0, 0, 0, 0], 9);
        assert_eq!(
            mesh_lane(&stopped),
            ([Some(nan), Some([0; 4]), Some([0; 4])], Some([9, 0, 0, 0]))
        );
        let past = run(&l, limits, [0, 0, 0, 0], 40);
        assert_eq!(past.did(), []);
        assert!(past.blocks.contains(&stop));
        // The two counts apart, either the greater, with more lanes than
        // both and with more lanes than one of them only.
        for (threads, vertices, primitives) in
            [(64, 48, 16), (64, 16, 48), (32, 32, 16), (32, 16, 32)]
        {
            let l = lower_mesh(threads, vertices, primitives, false, limits);
            let stop = l.limit_stop.expect("a loop that can stop a lane");
            for thread in [5, 15, 16, 20, 47, 48, threads - 1] {
                if thread >= threads {
                    continue;
                }
                let says = format!("thread {thread} of {threads}, {vertices}, {primitives}");
                let stopped = run(&l, limits, [0, 0, 0, 0], thread);
                assert!(stopped.blocks.contains(&stop), "{says}");
                let (vertex, primitive) = (thread < vertices, thread < primitives);
                let wrote = (
                    [
                        vertex.then_some(nan),
                        vertex.then_some([0; 4]),
                        primitive.then_some([0; 4]),
                    ],
                    primitive.then_some([thread, 0, 0, 0]),
                );
                assert_eq!(mesh_lane(&stopped), wrote, "{says}");
                // Nothing but those: eight channels of its vertex; its
                // primitive, culled before its code as every lane's is
                // and again here, and four channels of that.
                let calls = 8 * usize::from(vertex) + 6 * usize::from(primitive);
                assert_eq!(stopped.did().len(), calls, "{says}");
                let own = stopped.did().iter().all(|&d| match d {
                    Did::Vertex(.., at) | Did::PerPrimitive(.., at) => at == thread,
                    Did::Indices(emitted) => emitted[0] == thread,
                    _ => false,
                });
                assert!(own, "{says}");
            }
        }
    }

    /// A program without a loop has nothing of any of this, in any stage:
    /// no block for stopped lanes, no count of anything, and no discard in
    /// a pixel program that has none of its own. (That its container is
    /// byte for byte what it was before stopped lanes had these ends is
    /// shown outside the tests, against the tree before:
    /// `docs/progress.md`.)
    #[test]
    fn a_program_without_a_loop_has_no_end_for_stopped_lanes() {
        let body = [0x7E00_0201]; // v_mov_b32 v0, s1
        let plain = |l: &Lowered| {
            assert_eq!(l.limit_stop, None);
            let insts = l.flow.blocks.iter().flat_map(|b| &b.insts);
            for inst in insts {
                let unwanted = match &inst.op {
                    FlowOp::Call(name, _) => name == "dx.op.discard",
                    FlowOp::Phi(_) => true,
                    _ => false,
                };
                assert!(!unwanted, "{:?}", inst.op);
            }
        };
        plain(&lower_valu_limited(&body, None).unwrap());
        plain(&lower_pixel(&body, defaults()));
        plain(&lower_vertex(&body, false, defaults()));
        // A mesh program without its loop: only the exports.
        let l = lower_mesh(32, 32, 32, true, defaults());
        assert!(l.limit_stop.is_some());
        let mut words = vec![0xBE8D_0380]; // s_mov_b32 s13, 0
        words.extend_from_slice(&[0x7E08_020D, 0xF800_08CF, 0x0404_0404, S_ENDPGM]);
        let io = StageIo::Ms(MeshIo {
            outputs: vec![],
            clip_cntl: 0,
            threads: 32,
            max_vertices: 32,
            max_primitives: 32,
            topology: 2,
            lds_bytes: 0,
        });
        let position = element("SV_Position", 3, 0);
        let l = lower_stage(HwStage::Ms, io, &words, vec![position], vec![], defaults());
        plain(&l);
    }
}
