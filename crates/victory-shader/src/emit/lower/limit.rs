//! The iteration limit of translated loops ([`LOOP_LIMIT`]) at a
//! conditional back edge, and the limit's tests. Each loop header counts
//! the iterations a lane has made since it entered the loop, each back edge
//! is taken only below the limit, and a lane the limit stops leaves the
//! program through one block, where a pixel discards.
//!
//! Host drivers are sensitive to the shape of a loop: the AMD driver hung
//! on a loop exit that tested an `xor` of a compare, and hangs on divergent
//! loops whose exiting block is not the latch (`bitcode`, where the module
//! is laid out). A loop therefore stays as DXC would emit it. A back edge
//! keeps its block, a conditional latch keeps its one two-way branch with
//! the limit folded into its condition and leaves the loop to a block of
//! its own, and what tells the stopped lanes from the others comes there,
//! after the loop is left ([`Lower::limit_exit`]).
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
//! limit. Neither has run on a GPU from this file's tests on a host
//! without one: the Windows runs below are what shows a driver takes them.
//!
//! The DXIL validator runs on Windows only. The tests here lower small
//! hand-assembled programs and check, walk and run the function the module
//! writes (`bitcode`'s `Flow`) on any host; on Windows they also validate
//! each module and run the compute programs on the hardware adapter and on
//! WARP.
//!
//! [`LOOP_LIMIT`]: super::LOOP_LIMIT

use super::Lower;
use crate::emit::bitcode::{Term, V, pred};

impl Lower<'_> {
    /// The block latch `b`'s conditional branch goes to in place of `via`,
    /// the block of its edge out of the loop to IR block `to`. The latch
    /// leaves the loop both for the lanes whose own condition says so
    /// (`cond` is `own` in them) and for the lanes the limit stopped (`cond`
    /// is the opposite: they would have gone round again). This block tells
    /// them apart once the loop is left: the stopped lanes go to the abort
    /// block, the others on to `via`. A lane whose own condition ends the
    /// loop on the last iteration the limit allows is one of the others.
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
        let abort = self.abort_block();
        if self.counters.contains_key(&to) && self.rpo[to] <= self.rpo[b] {
            let here = std::mem::replace(&mut self.cur, self.s.m.new_block());
            let ok = self.count_iteration(to, pred::IULT);
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
mod tests {
    use super::super::tests::{lower_valu, root};
    use super::super::{Lowered, Parts, lower};
    use crate::emit::container::SigElement;
    use crate::native::{HwStage, NativeProgram, RootParam, RootType, StageIo};
    use crate::{io, ir, resource, structure};

    const S_ENDPGM: u32 = 0xBF81_0000;

    /// One lane of a program [`lower_valu`] lowered, run on this host with
    /// `constants` in s0..s3: the dword it stores from v0, or `None` where
    /// it ends without storing.
    fn stored(l: &Lowered, constants: [u32; 4]) -> Option<u32> {
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
        l.flow.run(1 << 28, &mut call).unwrap();
        stored
    }

    /// The limit the programs here are lowered with. The inputs of these
    /// tests are chosen against the default one, so with the `diagnostics`
    /// feature they need `VICTORY_SHADER_LOOP_LIMIT` out of the environment.
    fn limit() -> u32 {
        let limit = crate::emit::loop_limit();
        assert_eq!(
            limit,
            super::super::LOOP_LIMIT,
            "these tests need the default loop limit: unset VICTORY_SHADER_LOOP_LIMIT"
        );
        limit
    }

    /// Runs one lane of `body`, which [`lower_valu`] lowered to `l`, for
    /// each case: its root constants and what it stores ([`stored`]). On
    /// Windows the same lanes then run on the hardware adapter and on WARP,
    /// where the buffer is zeroed before each: a lane that stores nothing
    /// leaves 0 there.
    fn runs(l: &Lowered, body: &[u32], cases: &[([u32; 4], Option<u32>)]) {
        for &(constants, want) in cases {
            assert_eq!(stored(l, constants), want, "{constants:x?}");
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

    /// How a latch's branch folds the iteration limit into its condition.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Folded {
        /// Round again where the condition holds and the count is below
        /// the limit.
        And,
        /// Out where the condition holds or the count is at the limit.
        Or,
    }

    /// Checks the lowered function ([`Flow::check`]; on Windows the DXIL
    /// validator, where `dxil.dll` is there, checks the module too) and
    /// every way into the block a lane the loop limit stopped leaves
    /// through. Nothing follows
    /// that block. A latch with an unconditional back edge reaches it
    /// directly. A latch with a conditional one keeps its two-way branch on
    /// the condition and the limit folded together, and leaves the loop to
    /// a block of its own that tests the condition alone: the lanes that
    /// would have gone round again go to the stop, the others on to where
    /// `after` (the block of the program's store) is still reached. Returns
    /// each such latch's block and how its branch is folded.
    ///
    /// [`Flow::check`]: crate::emit::bitcode::Flow::check
    fn limit_stops(l: &Lowered, after: usize) -> Vec<(usize, Folded)> {
        use crate::emit::bitcode::{Arg, Bin, FlowOp, FlowTerm, pred};
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
        let limit = u64::from(limit());
        // Whether `v` compares an iteration count with the limit by `p`.
        let counts = |v: Arg, p: u8| {
            let Arg::Inst(id) = v else { return false };
            matches!(f.inst(id), Some((_, _, i))
                if matches!(i.op, FlowOp::Cmp(q, 32, _, Arg::Int(x)) if q == p && x == limit))
        };
        let mut latches = Vec::new();
        let mut into = f.preds(stop);
        into.dedup();
        for t in into {
            let FlowTerm::CondBr(cond, yes, no) = f.blocks[t].term else {
                panic!("block {t} goes to the stop unconditionally");
            };
            // A counted unconditional back edge: on below the limit.
            if counts(cond, pred::IULT) {
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
        let [(latch, how)] = limit_stops(&l, store)[..] else {
            panic!("one latch: {:?}", limit_stops(&l, store));
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
        let limit = limit();
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

    /// A pixel program: the lane the limit stops discards its pixel and
    /// exports nothing; the others export and discard nothing.
    #[test]
    fn a_loop_limit_stop_discards_a_pixel() {
        let words = [
            0xBE8C_0380, // s_mov_b32 s12, 0
            0x810C_810C, // loop: s_add_i32 s12, s12, 1
            0xBF07_000C, // s_cmp_lg_u32 s12, s0
            0xBF85_FFFD, // s_cbranch_scc1 loop
            0x7E08_020C, // v_mov_b32 v4, s12
            0xF800_180F, // exp mrt0, v4, v4, v4, v4 done vm
            0x0404_0404,
            S_ENDPGM,
        ];
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 8,
            layout: vec![RootParam {
                sgprs: 4,
                ..root(RootType::Constants, 0, 0)
            }],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = ir::build(&native.code, false, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let target = SigElement {
            name: "SV_Target".into(),
            index: 0,
            kind: 16,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 0xF,
            half: false,
        };
        let l = lower(&Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![target],
            prims: vec![],
            mesh_stride: 0,
            view_marks: false,
        })
        .unwrap();
        let f = &l.flow;
        let blocks = 0..f.blocks.len();
        let mut exports = blocks.filter(|&b| f.calls(b).contains(&"dx.op.storeOutput.f32"));
        let export = exports.next().expect("an export");
        assert_eq!(limit_stops(&l, export).len(), 1);
        assert_eq!(f.calls(l.limit_stop.unwrap()), ["dx.op.discard"]);
        // What a lane with `s0` in the first root constant does.
        let run = |s0: u32| {
            let mut did = Vec::new();
            let mut call = |name: &str, args: &[u64]| match name {
                "dx.op.cbufferLoadLegacy.i32" => vec![u64::from(s0), 0, 0, 0],
                "dx.op.discard" if args[1] != 0 => {
                    did.push(("discard", 0));
                    Vec::new()
                }
                // Opcode, output, row, column, value (the count's bits).
                "dx.op.storeOutput.f32" if args[3] == 0 => {
                    did.push(("export", args[4] as u32));
                    Vec::new()
                }
                _ => vec![0],
            };
            f.run(1 << 28, &mut call).unwrap();
            did
        };
        let limit = limit();
        assert_eq!(run(5), [("export", 5)]);
        assert_eq!(run(limit), [("export", limit)]);
        assert_eq!(run(0), [("discard", 0)]);
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
            let latches = limit_stops(&l, store);
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

    /// The loop's exit is the next loop's header: the test of the stopped
    /// lanes comes before that loop's preheader, where its count starts
    /// again at 0.
    #[test]
    fn a_loop_limit_stop_before_the_next_loop() {
        let body = [
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
        let l = lower_valu(&body).unwrap();
        assert_eq!(limit_stops(&l, store_block(&l)).len(), 2);
        let limit = limit();
        let cases = [
            ([3, 4, 0, 0], Some(7)),
            // Each loop has its count.
            ([limit, limit, 0, 0], Some(2 * limit)),
            // The first loop stops the lane; the second one.
            ([0, 4, 0, 0], None),
            ([3, 0, 0, 0], None),
        ];
        runs(&l, &body, &cases);
    }

    /// A loop in a loop: a lane the inner loop's limit stops leaves the
    /// outer loop with it, and the inner count starts again each time the
    /// outer loop enters it.
    #[test]
    fn a_loop_limit_stop_in_a_nested_loop() {
        let body = [
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
        let l = lower_valu(&body).unwrap();
        assert_eq!(limit_stops(&l, store_block(&l)).len(), 2);
        let limit = limit();
        let cases = [
            ([3, 4, 0, 0], Some(7)),
            // The inner count starts again with each entry.
            ([limit, 2, 0, 0], Some(limit + 2)),
            // The inner loop stops the lane; the outer one.
            ([0, 4, 0, 0], None),
            ([1, 0, 0, 0], None),
        ];
        runs(&l, &body, &cases);
    }

    /// Two conditional back edges to one header (a `continue` and the
    /// loop's end): each latch has its own test, the first one's other side
    /// stays in the loop, and both count the same iterations.
    #[test]
    fn a_loop_limit_stop_with_two_latches() {
        let body = [
            0xBE8C_0380, // s_mov_b32 s12, 0
            0x810C_810C, // loop: s_add_i32 s12, s12, 1
            0xBF0A_010C, // s_cmp_lt_u32 s12, s1
            0xBF85_FFFD, // s_cbranch_scc1 loop
            0xBF07_000C, // s_cmp_lg_u32 s12, s0
            0xBF85_FFFB, // s_cbranch_scc1 loop
            0x7E00_020C, // v_mov_b32 v0, s12
        ];
        let l = lower_valu(&body).unwrap();
        assert_eq!(limit_stops(&l, store_block(&l)).len(), 2);
        let limit = limit();
        let cases = [
            ([6, 3, 0, 0], Some(6)),
            // The last iteration the limit allows.
            ([limit, 9, 0, 0], Some(limit)),
            // The loop's end stops the lane; the `continue`.
            ([limit + 1, 9, 0, 0], None),
            ([0, u32::MAX, 0, 0], None),
        ];
        runs(&l, &body, &cases);
    }

    /// A `while` loop in a loop that ends with it: the inner header is the
    /// outer loop's latch, whose other side goes on into the inner loop's
    /// body. The stopped lanes are told from the others on that edge, and
    /// the inner back edge, an unconditional one, stops its own.
    #[test]
    fn a_loop_limit_stop_where_the_latch_is_an_inner_header() {
        let body = [
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
        let l = lower_valu(&body).unwrap();
        let latches = limit_stops(&l, store_block(&l));
        assert_eq!(
            latches.iter().map(|x| x.1).collect::<Vec<_>>(),
            [Folded::Or]
        );
        assert_eq!(
            l.flow.preds(l.limit_stop.unwrap()).len(),
            2,
            "both latches stop lanes"
        );
        let limit = limit();
        let cases = [
            ([3, 4, 0, 0], Some(4)),
            // The inner count starts again with each entry.
            ([limit - 1, 2, 0, 0], Some(2)),
            // The inner loop stops the lane; the outer one.
            ([limit, 2, 0, 0], None),
            ([0, 0, 0, 0], None),
        ];
        runs(&l, &body, &cases);
    }

    /// An inner loop laid out before the header of the loop around it: its
    /// latch branches back to itself and falls into the outer header, a
    /// back edge on both sides. The inner back edge is the latch's; the
    /// outer one is counted in a block of its own after the test of the
    /// lanes the inner limit stopped.
    #[test]
    fn a_loop_limit_stop_where_both_sides_are_back_edges() {
        let body = [
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
        let l = lower_valu(&body).unwrap();
        let store = store_block(&l);
        let [(inner, Folded::And)] = limit_stops(&l, store)[..] else {
            panic!("the inner loop's latch: {:?}", limit_stops(&l, store));
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
        let limit = limit();
        let cases = [
            ([3, 4, 0, 0], Some(4)),
            // The inner count starts again with each entry.
            ([limit, 3, 0, 0], Some(3)),
            // The inner loop stops the lane; the outer one.
            ([0, 4, 0, 0], None),
            ([1, 0, 0, 0], None),
        ];
        runs(&l, &body, &cases);
    }

    /// A back edge each lane takes for itself (`s_cbranch_execnz` after a
    /// `v_cmpx`): `do { v0 += 1 } while (v1 > v0)` with the bound in the
    /// lane's v1.
    #[test]
    fn a_loop_limit_stop_on_a_per_lane_back_edge() {
        let body = [
            0xBE8D_037E, // s_mov_b32 s13, exec_lo
            0x7E00_0280, // v_mov_b32 v0, 0
            0x4A00_0081, // loop: v_add_nc_u32 v0, 1, v0
            0x7DA8_0101, // v_cmpx_gt_u32 exec, v1, v0
            0xBF89_FFFD, // s_cbranch_execnz loop
            0xBEFE_030D, // s_mov_b32 exec_lo, s13
        ];
        let l = lower_valu(&body).unwrap();
        let latches = limit_stops(&l, store_block(&l));
        assert_eq!(
            latches.iter().map(|x| x.1).collect::<Vec<_>>(),
            [Folded::And]
        );
        let limit = limit();
        let cases = [
            ([5, 0, 0, 0], Some(5)),
            // The last iteration the limit allows, and one more.
            ([limit, 0, 0, 0], Some(limit)),
            ([limit + 1, 0, 0, 0], None),
        ];
        runs(&l, &body, &cases);
    }

    /// A per-lane loop in a uniform one, its latch a 64-bit compare for
    /// inequality: `do { do { v6 += 1; v10 += 1 } while (v[6:7] != v[8:9]);
    /// s14 += 1 } while (s14 != s1)` with the bound's dwords s0 and s2 in
    /// v8 and v9. That compare lowers to an `xor` (not both dwords equal),
    /// which the test of the stopped lanes branches on as it is, and the
    /// test, inside the outer loop, is a way out of it that is not its
    /// latch. The hardware adapter has to take both.
    #[test]
    fn a_loop_limit_stop_on_a_per_lane_back_edge_of_64_bits_in_a_loop() {
        use crate::emit::bitcode::{Arg, Bin, FlowOp, FlowTerm};
        let body = [
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
        let l = lower_valu(&body).unwrap();
        let store = store_block(&l);
        assert_eq!(
            limit_stops(&l, store)
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
        let limit = limit();
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
        runs(&l, &body, &cases);
    }

    /// Two loops with one header: the outer loop's body begins with the
    /// inner loop, so both back edges go to the same block and there is one
    /// count, which only the entry from outside both starts at 0. The limit
    /// is then on the inner iterations of all outer ones together: `do { do
    /// { s14 += 1 } while (s14 != s0); s14 = 0; s13 += 1 } while (s13 !=
    /// s1)` goes round s0 times s1 times in all, and a lane is stopped where
    /// that is more than the limit, though neither loop comes near it.
    #[test]
    fn a_loop_limit_counts_loops_sharing_a_header_together() {
        let body = [
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
        let l = lower_valu(&body).unwrap();
        let latches = limit_stops(&l, store_block(&l));
        let [(inner, Folded::And), (outer, Folded::And)] = latches[..] else {
            panic!("two latches: {latches:?}");
        };
        // Both go round to the one header, the inner loop's only block.
        let f = &l.flow;
        assert_eq!((f.succs(inner)[0], f.succs(outer)[0]), (inner, inner));
        let limit = limit();
        let cases = [
            ([3, 4, 0, 0], Some(4)),
            // In all exactly the iterations the limit allows.
            ([limit / 2, 2, 0, 0], Some(2)),
            ([limit, 1, 0, 0], Some(1)),
            // More in all, with either loop far below the limit.
            ([limit / 2 + 1, 2, 0, 0], None),
            ([limit / 4, 5, 0, 0], None),
        ];
        runs(&l, &body, &cases);
    }
}
