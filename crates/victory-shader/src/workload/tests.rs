//! The recogniser on the synthetic programs of [`super::walk`]: what each
//! is recognised as, a program for every refusal and for every condition
//! behind one, the region analyses on the same loops, and the recogniser's
//! work as counts. Every claim here is about these programs; no title loop
//! was read.

use std::collections::{BTreeSet, HashSet};

use super::census::Census;
use super::describe::{
    Arg, Compare, Description, Kind, MissWord, Order, Refusal, Root, StructureSource,
};
use super::recognise::{MAX_ATOMS, Parts, Report, Work, recognise, recognise_loop};
use super::region::{Memory, data_reads, touches};
use super::walk::{ROOT_POINTER, ROOT_TREE, Variation, Walk};
use crate::ir::{self, Reg};
use crate::native::NativeProgram;
use crate::{io, resource, structure};

/// A walk through the stages before emission, each kept if it did not
/// refuse.
struct Staged {
    native: NativeProgram,
    program: ir::Program,
    lanes: Option<structure::Structured>,
    resources: Option<resource::Resources>,
    io: Option<io::Io>,
    lowered: bool,
}

impl Staged {
    fn of(walk: Walk) -> Staged {
        Staged::from(walk.native())
    }

    fn from(native: NativeProgram) -> Staged {
        let program = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
        let lanes = structure::structure(&program).ok();
        let resources =
            resource::track(&program, &native.layout, native.stage, lanes.as_ref()).ok();
        let io = io::raise(&program, &native).ok();
        let lowered = super::walk::translate_native(native.clone()).is_ok();
        Staged {
            native,
            program,
            lanes,
            resources,
            io,
            lowered,
        }
    }

    fn parts(&self) -> Parts<'_> {
        Parts {
            native: &self.native,
            p: &self.program,
            st: self.lanes.as_ref(),
            res: self.resources.as_ref(),
            io: self.io.as_ref(),
            lowered: self.lowered,
        }
    }

    fn report(&self) -> Report {
        recognise(&self.parts())
    }
}

fn described(walk: Walk) -> Description {
    let report = Staged::of(walk).report();
    let [finding] = &report.loops[..] else {
        panic!("{walk:?}: {} loops", report.loops.len());
    };
    finding
        .result
        .clone()
        .unwrap_or_else(|why| panic!("{walk:?}: {}", why.reason()))
}

/// What each program's loops with a node instruction come to, in the
/// order of their headers: the order field of a description, or a refusal.
fn expected(walk: Walk) -> Vec<Result<Order, Refusal>> {
    use Refusal::*;
    use Walk::*;
    match walk {
        Nearest | NearestInclusive | NearestUnordered | NearestNode => vec![Ok(Order::TieByVisit)],
        Any => vec![Ok(Order::None)],
        TieByKey => vec![Ok(Order::TieByKey)],
        // The loop around the walk, then the walk.
        TwoPasses => vec![Err(NestedLoop), Ok(Order::TieByVisit)],
        NoNode => vec![],
        NearestIds => vec![Err(NodeLoad)],
        LastHit => vec![Err(VisitOrder)],
        Capped => vec![Err(IterationCap)],
        BitsCompare => vec![Err(CompareForm)],
        NoExtent => vec![Err(ExtentCompare)],
        LoadedInverse => vec![Err(InverseNotProven)],
        MovingRay => vec![Err(RayChanges)],
        Counting => vec![Err(LiveOut)],
        WaveWalk => vec![Err(WholeWave)],
        TwoTests => vec![Err(SeveralNodeTests)],
        PackedDirections => vec![Err(NodeForm)],
        LaneNumber => vec![Err(NoPerLaneForm)],
        RawBase => vec![Err(StructureSource)],
        SkipsChild => vec![Err(StackShape)],
        StoresInLoop => vec![Err(Stores)],
        LoadsInLoop => vec![Err(Loads)],
        EarlyExit => vec![Err(ExitForm)],
        AcceptEither => vec![Err(AcceptForm)],
    }
}

#[test]
fn every_walk_is_recognised_or_refused_as_meant() {
    let mut refusals = BTreeSet::new();
    for walk in Walk::ALL {
        let staged = Staged::of(walk);
        let report = staged.report();
        let got: Vec<Result<Order, Refusal>> = report
            .loops
            .iter()
            .map(|f| f.result.clone().map(|d| d.order))
            .collect();
        assert_eq!(got, expected(walk), "{walk:?}");
        assert_eq!(
            report.node_instructions,
            usize::from(walk != Walk::NoNode) + usize::from(walk == Walk::TwoTests)
        );
        assert_eq!(report.outside_loops, 0, "{walk:?}");
        refusals.extend(got.into_iter().filter_map(Result::err));
        // Every walk has a per-lane structure; the ones with no per-lane
        // translation are refused by the lowering.
        assert!(
            staged.lanes.is_some() && staged.resources.is_some() && staged.io.is_some(),
            "{walk:?}"
        );
        assert_eq!(staged.lowered, walk.translates(), "{walk:?}");
    }
    // The loop without a node instruction is in no report; asked for by
    // itself it is refused as such.
    let staged = Staged::of(Walk::NoNode);
    let loops = ir::natural_loops(&staged.program);
    let [l] = &loops[..] else { panic!("{loops:?}") };
    let mut terms = staged
        .lanes
        .as_ref()
        .map(|st| super::terms::Terms::new(&staged.program, st));
    let refused = recognise_loop(
        &staged.parts(),
        terms.as_mut(),
        &loops,
        l,
        &mut Work::default(),
    );
    assert_eq!(refused, Err(Refusal::NoNodeInstruction));
    refusals.insert(Refusal::NoNodeInstruction);
    // The limit on compares has its program among the variations.
    let many = Staged::from(Variation::ManyCompares.native()).report();
    refusals.extend(many.loops.into_iter().filter_map(|f| f.result.err()));
    // A program for every reason there is.
    assert_eq!(refusals, Refusal::ALL.into_iter().collect());
    let reasons: HashSet<&str> = Refusal::ALL.iter().map(|r| r.reason()).collect();
    assert_eq!(reasons.len(), Refusal::ALL.len());
}

/// The reason each variation's walk is refused for: the condition it
/// breaks is checked, by the check that names it.
fn refused(v: Variation) -> Refusal {
    use Refusal::*;
    use Variation::*;
    match v {
        GuardedRoot | BranchedRoot | RootAboveBase | DeepStart | Grid | OtherAxis | NextNode
        | ClearsOnMinimum | WrongChild | PushesAbove | PushWithoutRaise | TriangleRaises => {
            StackShape
        }
        LoadAtPass | LoadWithCarry => NodeLoad,
        WeightOnFront | KeyIsFacing => AcceptForm,
        LastPointer | MaskedStore | FacingInclusive => LiveOut,
        ManyCompares => TooManyCompares,
        MinimumMoves => CompareForm,
        ExtentIsWeight => RayChanges,
        TestsEarlySp | SecondExit => ExitForm,
        DescriptorInLoop => StructureSource,
    }
}

/// One refusal reason stands for several conditions: each has a program
/// that breaks it alone, every one of them translated by the normal
/// lowering (so none is refused for having no per-lane form), and each is
/// refused for its reason. The walk a variation changes is recognised.
#[test]
fn a_walk_that_breaks_one_condition_is_refused_for_it() {
    for v in Variation::ALL {
        let staged = Staged::from(v.native());
        assert!(
            staged.lanes.is_some() && staged.resources.is_some() && staged.io.is_some(),
            "{v:?}"
        );
        assert!(staged.lowered, "{v:?}");
        let got: Vec<Result<Order, Refusal>> = (staged.report().loops.into_iter())
            .map(|f| f.result.map(|d| d.order))
            .collect();
        // The walk's own loop is the last (a loop around it comes first).
        let around = usize::from(v.of() == Walk::TwoPasses);
        assert_eq!(got.len(), 1 + around, "{v:?}");
        assert_eq!(got[around], Err(refused(v)), "{v:?}");
        // Changed back, the same walk is recognised.
        let unchanged = expected(v.of());
        assert!(unchanged[around].is_ok(), "{v:?}");
        assert!(v.native() != v.of().native(), "{v:?}");
    }
}

#[test]
fn the_nearest_hit_walk_is_described_field_by_field() {
    let staged = Staged::of(Walk::Nearest);
    let d = described(Walk::Nearest);
    let p = &staged.program;
    // The region: the loop's five blocks, one way in, one way out.
    assert_eq!(
        (d.region.header, &d.region.blocks[..]),
        (2, &[2, 3, 4, 5, 6][..])
    );
    assert_eq!(
        (&d.region.entries[..], &d.region.exits[..]),
        (&[1][..], &[(6, 7)][..])
    );
    assert_eq!(
        p.blocks[d.node.0].insts[d.node.1].name(),
        "image_bvh_intersect_ray"
    );
    // The ray: what the two loads before the loop read.
    let loaded = |k: usize| {
        let load = &p.blocks[1].insts[1 + k / 4];
        assert_eq!(load.name(), "buffer_load_dwordx4");
        load.writes[k % 4].1
    };
    assert_eq!(d.ray.origin, [loaded(0), loaded(1), loaded(2)]);
    assert_eq!(d.ray.direction, [loaded(4), loaded(5), loaded(6)]);
    assert_eq!(d.ray.minimum, (Arg::Value(loaded(3)), Compare::LESS));
    assert_eq!(
        (d.ray.extent.compare, d.ray.extent.shrinks),
        (Compare::LESS, true)
    );
    // `+inf < extent` holds for no extent: only a hit is accepted.
    assert_eq!(d.ray.extent.miss_word, MissWord::NEVER);
    // The extent starts as a copy of the ray's last word.
    let Arg::Value(start) = d.ray.extent.start else {
        panic!()
    };
    let copy = p.blocks[1]
        .insts
        .iter()
        .find(|i| i.writes.iter().any(|w| w.1 == start))
        .unwrap();
    assert_eq!(
        (copy.name(), copy.read(Reg::V(11))),
        ("v_mov_b32", Some(loaded(7)))
    );
    assert_eq!(
        d.structure,
        StructureSource::RootParameter(usize::from(ROOT_TREE))
    );
    assert_eq!(d.root, Root::Constant(ROOT_POINTER));
    assert_eq!((d.stack.stride, d.stack.checked), (256, false));
    assert_eq!(
        (
            d.flags.accept_first_hit,
            d.flags.cull_front,
            d.flags.cull_back
        ),
        (false, false, false)
    );
    // Kept: the distance (a miss leaves the ray's own extent), two weights
    // and the facing (a miss leaves 0 in each).
    let mut kept: Vec<(Kind, Arg)> = d.kept.iter().map(|k| (k.kind, k.miss)).collect();
    kept.sort_by_key(|k| format!("{:?}", k.0));
    assert_eq!(
        kept,
        [
            (Kind::Barycentric(0), Arg::Const(0)),
            (Kind::Barycentric(1), Arg::Const(0)),
            (Kind::Distance, Arg::Value(start)),
            (Kind::Facing { front: 1, back: 2 }, Arg::Const(0)),
        ]
    );
    // Each is read after the loop.
    for k in &d.kept {
        assert!(d.region.live_out.iter().any(|o| o.1 == k.value), "{k:?}");
    }
    assert_eq!(
        d.forms(),
        "lanes=per-lane; inverse=reciprocal; minimum=strict; extent=strict,shrinks; miss-word=never; structure=root-parameter; \
         root=constant; first-hit=0; cull=00; mask=untested; instance-code=none; filter=none; \
         kept=barycentric-0+barycentric-1+distance-by-reciprocal+facing-by-sign; order=tie-by-visit; \
         iteration-cap=none; stack=unchecked"
    );
    assert_eq!(d.class, Description::class_of(&d.forms()));
}

#[test]
fn the_other_accepted_walks_differ_where_their_programs_do() {
    let nearest = described(Walk::Nearest);
    let form = |strict, unordered| Compare { strict, unordered };
    assert_eq!(
        described(Walk::NearestInclusive).ray.extent.compare,
        form(false, false)
    );
    assert_eq!(
        described(Walk::NearestUnordered).ray.extent.compare,
        form(true, true)
    );
    // What the accept does with the node test's answer for a triangle the
    // ray misses (t = +inf): `t <= extent` passes it at an infinite
    // extent, "not `t >= extent`" at a NaN one, and neither is a hit.
    let passes = |walk: Walk| {
        let d = described(walk);
        let m = d.ray.extent.miss_word;
        assert!(d.forms().contains("; miss-word="), "{walk:?}");
        (m.at_infinite, m.at_nan)
    };
    assert_eq!(passes(Walk::NearestInclusive), (true, false));
    assert_eq!(passes(Walk::NearestUnordered), (false, true));
    assert!(
        described(Walk::NearestInclusive)
            .forms()
            .contains("miss-word=at-infinite-extent;")
    );
    assert!(
        described(Walk::NearestUnordered)
            .forms()
            .contains("miss-word=at-nan-extent;")
    );
    // The tie by key compares `t == extent`, which +inf passes at an
    // infinite extent, but nothing is below the kept pointer's first
    // value, 0.
    for walk in [Walk::Any, Walk::NearestNode, Walk::TieByKey] {
        assert_eq!(passes(walk), (false, false), "{walk:?}");
    }
    // Yes or no: one constant kept, the extent the ray's own throughout,
    // the walk over at the first candidate accepted.
    let any = described(Walk::Any);
    let kinds = |d: &Description| -> Vec<Kind> { d.kept.iter().map(|k| k.kind).collect() };
    assert_eq!(kinds(&any), [Kind::Flag(1)]);
    assert!(any.flags.accept_first_hit && !any.ray.extent.shrinks);
    assert_eq!(any.ray.extent.start, nearest_tmax(Walk::Any));
    // The pointer kept and stored, not loaded through.
    let node = described(Walk::NearestNode);
    assert!(kinds(&node).contains(&Kind::NodePointer) && kinds(&node).len() == 5);
    assert_eq!(kinds(&described(Walk::TieByKey)).len(), 5);
    // The second pass starts from the first's values, not from constants.
    let twice = described_inner(Walk::TwoPasses);
    assert!(twice.kept.iter().all(|k| matches!(k.miss, Arg::Value(_))));
    assert_eq!(twice.root, nearest.root);

    // The class is the forms': the same walk in another program is the
    // same class, and each walk that differs in a form is a class of its own.
    assert_eq!(twice.class, nearest.class);
    let walks = [
        Walk::Nearest,
        Walk::NearestInclusive,
        Walk::NearestUnordered,
        Walk::Any,
        Walk::NearestNode,
        Walk::TieByKey,
    ];
    let classes: HashSet<_> = walks.iter().map(|&w| described(w).class).collect();
    assert_eq!(classes.len(), walks.len());
    for w in walks {
        let forms = described(w).forms();
        assert!(!forms.contains("Value") && !forms.contains("0x"), "{forms}");
    }
}

/// The ray's last word (its extent) as the walk's description names it.
fn nearest_tmax(walk: Walk) -> Arg {
    let staged = Staged::of(walk);
    Arg::Value(staged.program.blocks[1].insts[2].writes[3].1)
}

fn described_inner(walk: Walk) -> Description {
    let report = Staged::of(walk).report();
    report
        .loops
        .into_iter()
        .find_map(|f| f.result.ok())
        .expect("a recognised loop")
}

#[test]
fn compare_forms_are_read_from_the_instruction_names() {
    let form = |strict, unordered| Compare { strict, unordered };
    let table = [
        ("lt_f32", false, form(true, false)),
        ("gt_f32", true, form(true, false)),
        ("le_f32", false, form(false, false)),
        ("ge_f32", true, form(false, false)),
        ("nge_f32", false, form(true, true)),
        ("nle_f32", true, form(true, true)),
        ("ngt_f32", false, form(false, true)),
        ("nlt_f32", true, form(false, true)),
    ];
    for (op, swapped, form) in table {
        assert_eq!(Compare::of(op), Some((swapped, form)), "{op}");
        // Each is an instruction the decoder knows.
        let name = format!("v_cmp_{op}");
        assert!(
            crate::gfx10::opcodes::VOPC.iter().any(|e| e.1 == name),
            "{name}"
        );
    }
    for op in [
        "lt_i32",
        "lt_u32",
        "eq_f32",
        "neq_f32",
        "lg_f32",
        "o_f32",
        "u_f32",
        "class_f32",
    ] {
        assert_eq!(Compare::of(op), None, "{op}");
    }
    // What each form means, and so which of them the miss word's +inf
    // passes: none of a finite extent's, the inclusive ones at +inf, the
    // unordered ones at a NaN.
    let (inf, nan) = (f32::INFINITY, f32::NAN);
    let samples = [-1.0, 0.0, 1.0, 1.5, inf, nan];
    for low in samples {
        for high in samples {
            let unordered = low.is_nan() || high.is_nan();
            assert_eq!(form(true, false).holds(low, high), low < high);
            assert_eq!(form(false, false).holds(low, high), low <= high);
            assert_eq!(form(true, true).holds(low, high), low < high || unordered);
            assert_eq!(form(false, true).holds(low, high), low <= high || unordered);
        }
    }
    for (strict, unordered) in [(true, false), (false, false), (true, true), (false, true)] {
        let f = form(strict, unordered);
        assert!(!f.holds(inf, 1.0e30) && !f.holds(inf, -0.0));
        assert_eq!(f.holds(inf, inf), !strict);
        assert_eq!(f.holds(inf, nan), unordered);
    }
}

#[test]
fn loops_and_regions_of_the_walks() {
    // The walk: one loop, the rule's (the loop limits count by the same).
    let staged = Staged::of(Walk::Nearest);
    let p = &staged.program;
    let loops = ir::natural_loops(p);
    let [l] = &loops[..] else { panic!("{loops:?}") };
    assert_eq!(
        (l.header, l.place, &l.blocks[..], &l.latches[..]),
        (2, 0, &[2, 3, 4, 5, 6][..], &[6][..])
    );
    let r = ir::region(p, l);
    assert_eq!((&r.entries[..], &r.exits[..]), (&[1][..], &[(6, 7)][..]));
    // Carried: the header's phis. Live in: defined before the loop and
    // read in it, the ray and the thread's base among them.
    assert_eq!(r.carried.len(), p.blocks[2].phis.len());
    let defined_in = |v: ir::Value| {
        let writes = |b: &ir::Block| b.insts.iter().any(|i| i.writes.iter().any(|w| w.1 == v));
        let phis = |b: &ir::Block| b.phis.iter().any(|phi| phi.value == v);
        (0..p.blocks.len()).find(|&b| writes(&p.blocks[b]) || phis(&p.blocks[b]))
    };
    assert!(
        r.live_in
            .iter()
            .all(|&(_, v)| defined_in(v).is_none_or(|b| !l.holds(b)))
    );
    assert!(
        r.live_out
            .iter()
            .all(|&(_, v)| defined_in(v).is_some_and(|b| l.holds(b)))
    );
    for reg in [4u8, 5, 6, 8, 9, 10, 12, 13, 14, 15] {
        assert!(r.live_in.iter().any(|i| i.0 == Reg::V(reg)), "v{reg}");
    }
    // Live out: what the hit record is made of (v18-v21), each read by an
    // instruction after the loop.
    for reg in [18u8, 19, 20, 21] {
        assert!(r.live_out.iter().any(|o| o.0 == Reg::V(reg)), "v{reg}");
    }

    // Two passes: two loops, the walk's inside the other, each a region
    // with one way in; the outer one's exit is not the inner one's.
    let staged = Staged::of(Walk::TwoPasses);
    let loops = ir::natural_loops(&staged.program);
    let [outer, inner] = &loops[..] else {
        panic!("{loops:?}")
    };
    assert!(
        inner.blocks.iter().all(|&b| outer.holds(b)) && outer.blocks.len() > inner.blocks.len()
    );
    assert_ne!(outer.header, inner.header);
    let (ro, ri) = (
        ir::region(&staged.program, outer),
        ir::region(&staged.program, inner),
    );
    assert_eq!(
        (
            ro.entries.len(),
            ri.entries.len(),
            ro.exits.len(),
            ri.exits.len()
        ),
        (1, 1, 1, 1)
    );
    assert_ne!(ro.exits, ri.exits);
    // What the inner loop hands the outer one's next round is live out of
    // the inner and not of the outer, which only the code after reads.
    assert!(ri.live_out.len() >= ro.live_out.len());
}

#[test]
fn the_memory_a_region_touches() {
    let count = |a: &[super::region::Access], m: Memory, store: bool| {
        a.iter()
            .filter(|x| x.memory == m && x.store == store)
            .count()
    };
    let touched = |walk: Walk, inside: bool| {
        let staged = Staged::of(walk);
        let l = &ir::natural_loops(&staged.program)[0];
        let blocks: Vec<usize> = (0..staged.program.blocks.len())
            .filter(|&b| l.holds(b) == inside)
            .collect();
        touches(&staged.program, staged.resources.as_ref().unwrap(), &blocks)
    };
    // The walk's loop: a pop and four pushes in group-shared memory, and
    // the node test through the descriptor around the root parameter.
    let a = touched(Walk::Nearest, true);
    assert_eq!(a.len(), 6);
    assert_eq!(
        (
            count(&a, Memory::Shared, false),
            count(&a, Memory::Shared, true)
        ),
        (1, 4)
    );
    assert_eq!(count(&a, Memory::Node { tracked: true }, false), 1);
    // Outside it: the root's push, two loads of the ray and two stores of
    // the hit through the table's descriptors.
    let a = touched(Walk::Nearest, false);
    assert_eq!(
        (
            count(&a, Memory::Shared, true),
            count(&a, Memory::Shared, false)
        ),
        (1, 0)
    );
    assert_eq!(
        (
            count(&a, Memory::Tracked, false),
            count(&a, Memory::Tracked, true)
        ),
        (4, 2)
    );
    // A descriptor around a constant address: the nodes at a raw address.
    assert_eq!(
        count(
            &touched(Walk::RawBase, true),
            Memory::Node { tracked: false },
            false
        ),
        1
    );
    // Loads and stores through descriptors inside the loop.
    assert_eq!(
        count(&touched(Walk::LoadsInLoop, true), Memory::Tracked, false),
        1
    );
    assert_eq!(
        count(&touched(Walk::StoresInLoop, true), Memory::Tracked, true),
        1
    );
    // The probe's walk loads through the kept pointer after its loop, at
    // the root parameter's address plus what it computed.
    let ids = touched(Walk::NearestIds, false);
    assert_eq!(
        count(&ids, Memory::Tracked, false) - count(&a, Memory::Tracked, false),
        2
    );
    assert_eq!(count(&ids, Memory::Raw, false), 0);
}

#[test]
fn an_unguarded_write_does_not_read_what_it_overwrites() {
    let staged = Staged::of(Walk::Nearest);
    let (p, st) = (&staged.program, staged.lanes.as_ref().unwrap());
    let mut guarded = 0;
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let guard = st.guards[b][i].is_some();
            let reads = data_reads(inst, guard);
            // EXEC is data to no vector instruction here.
            let vector = inst.name().starts_with("v_") || inst.name().starts_with("ds_");
            assert!(
                !vector || reads.iter().all(|r| r.0 != Reg::S(ir::EXEC)),
                "{}",
                inst.name()
            );
            if inst.name() == "v_add_nc_u32" {
                let dst = inst.writes[0].0;
                let named = match &inst.fields {
                    crate::gfx10::Fields::Valu(v) => v.src.iter().any(|s| matches!(*s, crate::gfx10::Operand::Vector { reg, .. } if Reg::V(reg) == dst)),
                    _ => false,
                };
                // The old value is data only where the add is guarded, or
                // is one of its own operands.
                assert_eq!(reads.iter().any(|r| r.0 == dst), guard || named, "{b}.{i}");
                guarded += usize::from(guard);
            }
        }
    }
    // The pushes' address and stack-pointer adds.
    assert_eq!(guarded, 8);
}

/// Equal terms are claimed only for equal computations: an instruction
/// that reads SCC is a function of it too, and a guarded write is a pick.
#[test]
fn a_term_is_a_function_of_everything_its_instruction_reads() {
    use super::asm::{Asm, Src};
    use super::terms::{Term, Terms};
    let mut a = Asm::new();
    a.sopc("s_cmp_eq_u32", Src::S(0), Src::S(1));
    a.sop2("s_cselect_b32", 8, Src::S(3), Src::S(4));
    a.sopc("s_cmp_eq_u32", Src::S(0), Src::S(5));
    a.sop2("s_cselect_b32", 9, Src::S(3), Src::S(4));
    a.sop2("s_add_u32", 10, Src::S(3), Src::S(4));
    a.sop2("s_add_u32", 11, Src::S(3), Src::S(4));
    a.sop2("s_add_u32", 12, Src::S(3), Src::K(4));
    a.sop1("s_mov_b32", 13, Src::S(12));
    a.endpgm();
    let code: Vec<u8> = a.finish().iter().flat_map(|w| w.to_le_bytes()).collect();
    let p = ir::build(&code, false, 8).unwrap();
    let st = structure::structure(&p).unwrap();
    let mut t = Terms::new(&p, &st);
    let written = |i: usize| p.blocks[0].insts[i].writes[0].1;
    let (first, second) = (t.term(written(1)), t.term(written(3)));
    // The same operands under two different SCCs: two terms.
    assert_ne!(first, second);
    assert!(matches!(t.get(first), Term::Op("s_cselect_b32", 0, args) if args.len() == 3));
    // The same operands and nothing else read: one term; another constant,
    // another term; a move is what it moves.
    assert_eq!(t.term(written(4)), t.term(written(5)));
    assert_ne!(t.term(written(4)), t.term(written(6)));
    assert_eq!(t.term(written(7)), t.term(written(6)));
    assert_eq!(t.visited, 9);

    // A move relative to M0 reads registers it does not name: the same
    // operand under two values of M0 is two things, each known as nothing
    // but itself.
    let mut a = Asm::new();
    a.sop1("s_mov_b32", u16::from(ir::M0), Src::K(1));
    a.vop1("v_movrels_b32", 4, Src::V(0));
    a.sop1("s_mov_b32", u16::from(ir::M0), Src::K(2));
    a.vop1("v_movrels_b32", 5, Src::V(0));
    a.vop1("v_rcp_f32", 6, Src::V(0));
    a.vop1("v_rcp_f32", 7, Src::V(0));
    a.endpgm();
    let code: Vec<u8> = a.finish().iter().flat_map(|w| w.to_le_bytes()).collect();
    let p = ir::build(&code, false, 8).unwrap();
    let st = structure::structure(&p).unwrap();
    let mut t = Terms::new(&p, &st);
    let moved = |i: usize| {
        let inst = &p.blocks[0].insts[i];
        assert_eq!(inst.name(), "v_movrels_b32");
        assert!(super::region::reads_beyond_its_operands(inst));
        inst.writes[0].1
    };
    let (first, second) = (t.term(moved(1)), t.term(moved(3)));
    assert_ne!(first, second);
    assert_eq!(t.get(first), &Term::Opaque(moved(1)));
    assert_eq!(t.get(second), &Term::Opaque(moved(3)));
    // Where an instruction names all it reads, the same operand is the
    // same term.
    let rcp = |i: usize| p.blocks[0].insts[i].writes[0].1;
    assert_eq!(t.term(rcp(4)), t.term(rcp(5)));

    // In the walk: a push's stack-pointer add is guarded, so its value is
    // the sum where the guard holds and the old stack pointer elsewhere;
    // the inverse is the reciprocal of the direction, as one term.
    let staged = Staged::of(Walk::Nearest);
    let (p, st) = (&staged.program, staged.lanes.as_ref().unwrap());
    let mut t = Terms::new(p, st);
    let (i, add) = (p.blocks[5].insts.iter().enumerate())
        .find(|(i, inst)| {
            inst.name() == "v_add_nc_u32"
                && st.guards[5][*i].is_some()
                && inst.writes[0].0 == Reg::V(16)
        })
        .unwrap();
    let (new, old) = (
        t.term(add.writes[0].1),
        t.term(add.read(Reg::V(16)).unwrap()),
    );
    let Term::Pick(when, then, otherwise) = t.get(new).clone() else {
        panic!("{:?}", t.get(new));
    };
    assert_eq!((Some(when), otherwise), (st.guards[5][i], old));
    assert!(matches!(t.get(then), Term::Op("v_add_nc_u32", 0, sum) if sum.contains(&old)));
    let rcp = &p.blocks[1].insts[3];
    let (inverse, direction) = (
        t.term(rcp.writes[0].1),
        t.term(rcp.read(Reg::V(8)).unwrap()),
    );
    assert_eq!(t.get(inverse), &Term::Op("v_rcp_f32", 0, vec![direction]));
}

/// The recogniser's work, as counts: instructions looked at, bounded by a
/// multiple of the program's size for each loop examined, and conditions
/// evaluated over the cases of the loops' tables, bounded by the limit on
/// a loop's free compares and not by the program's size. Reported here
/// (run with `--nocapture`). Not a time: none was measured.
#[test]
fn the_recognisers_work_is_counted_and_bounded() {
    let mut census = Census::default();
    let mut counted = Vec::new();
    for walk in Walk::ALL {
        let staged = Staged::of(walk);
        let report = staged.report();
        let insts: usize = staged.program.blocks.iter().map(|b| b.insts.len()).sum();
        eprintln!(
            "{walk:?}: {insts} instructions, {} loops with a node instruction, {} visited, \
             {} conditions evaluated in {} cases",
            report.loops.len(),
            report.visited,
            report.evaluated,
            report.cases
        );
        // One scan to find node instructions; with a per-lane form one
        // pass for the terms; for each loop its own blocks at most twice,
        // the whole program once (the pointer's definition) and what is
        // outside it at most three times.
        assert!(
            report.visited <= insts * (2 + 4 * report.loops.len()),
            "{walk:?}"
        );
        assert!(report.visited >= insts);
        // The tables: at most 2^14 cases a loop, a power of two for each
        // loop that got that far, and in a case no more evaluations than
        // one for each pick a value's chain can hold (fewer than the
        // program has values), for each of fewer chains than four times
        // its values.
        let values = super::terms::values(&staged.program);
        assert!(report.cases <= report.loops.len() << MAX_ATOMS, "{walk:?}");
        assert!(
            report.evaluated <= report.cases * 4 * values * values,
            "{walk:?}"
        );
        assert_eq!(report.cases == 0, report.evaluated == 0, "{walk:?}");
        // A loop refused before its tables costs no case; a recognised
        // one was followed through all of its own.
        let recognised = report.loops.iter().filter(|f| f.result.is_ok()).count();
        assert!(recognised == 0 || report.cases >= 2, "{walk:?}");
        if matches!(
            walk,
            Walk::NoNode | Walk::WaveWalk | Walk::LaneNumber | Walk::TwoTests | Walk::RawBase
        ) {
            assert_eq!((report.cases, report.evaluated), (0, 0), "{walk:?}");
        }
        if walk == Walk::NoNode {
            assert_eq!(report.visited, insts);
        }
        counted.push((walk, insts, report.visited, report.cases, report.evaluated));
        census.add(&report);
    }
    // The figures the document gives. Two more free compares (the tie by
    // key's) are four times the cases: the tables, not the instructions,
    // are most of the work.
    let of = |walk: Walk| *counted.iter().find(|c| c.0 == walk).unwrap();
    assert_eq!(
        of(Walk::Nearest),
        (Walk::Nearest, 93, 465, 512, NEAREST_EVALUATED)
    );
    assert_eq!(
        of(Walk::TieByKey),
        (Walk::TieByKey, 99, 539, 2048, TIE_EVALUATED)
    );
    assert_eq!(of(Walk::NoNode), (Walk::NoNode, 12, 12, 0, 0));
    assert_eq!((of(Walk::TwoPasses).1, of(Walk::TwoPasses).2), (97, 639));
    // A loop with more free compares than the limit is refused before any
    // table is made.
    let many = Staged::from(Variation::ManyCompares.native()).report();
    assert_eq!(many.loops[0].result, Err(Refusal::TooManyCompares));
    assert_eq!((many.cases, many.evaluated), (0, 0));

    assert_eq!(census.programs, Walk::ALL.len());
    assert_eq!(census.with_node, Walk::ALL.len() - 1);
    assert_eq!(census.loops, Walk::ALL.len());
    let recognised: usize = census.recognised.values().map(|r| r.0).sum();
    assert_eq!(
        (recognised, census.refused.values().sum::<usize>()),
        (7, 19)
    );
    // Nearest, its inner twin of two passes: one class of two loops.
    assert_eq!(census.recognised.len(), 6);
    assert!(census.recognised.values().any(|r| r.0 == 2));
    // The lines: counts, forms and reasons; merging is adding.
    let lines = census.lines();
    assert_eq!(lines.len(), 2 + 6 + 19);
    assert!(lines[0].starts_with(
        "ray tracing  25 of 26 compute programs examined hold a node instruction \
         (26 instructions, 0 in no loop); 0 more compute programs were not built and not examined; \
         26 loops hold one: 7 recognised, 19 refused"
    ));
    assert_eq!(
        lines[1],
        format!(
            "ray tracing  the recogniser's work: {} instructions visited, {} conditions evaluated in {} cases",
            census.visited, census.evaluated, census.cases
        )
    );
    assert!(census.evaluated > census.visited);
    // Every line is the census's own: nothing of a program is in them.
    for line in &lines {
        eprintln!("{line}");
        assert!(
            line.starts_with("ray tracing  ") || line.starts_with("  "),
            "{line}"
        );
        let own = line.contains("recognised, class ") || line.contains("refused: ");
        assert!(line.starts_with("ray tracing  ") || own, "{line}");
    }
    let mut twice = census.clone();
    twice.merge(census.clone());
    assert_eq!(
        (twice.loops, twice.visited, twice.cases, twice.evaluated),
        (
            2 * census.loops,
            2 * census.visited,
            2 * census.cases,
            2 * census.evaluated
        )
    );
    assert_eq!(twice.recognised.values().map(|r| r.0).sum::<usize>(), 14);
    // A compute program whose IR was not built is counted, not left out.
    let mut census = Census::default();
    census.add(&Staged::of(Walk::Nearest).report());
    census.not_built();
    census.not_built();
    assert_eq!((census.programs, census.unbuilt), (1, 2));
    assert!(
        census.lines()[0].contains("; 2 more compute programs were not built and not examined;")
    );
    let mut merged = census.clone();
    merged.merge(census);
    assert_eq!(merged.unbuilt, 4);
}

/// The conditions [`Walk::Nearest`] and [`Walk::TieByKey`] cost.
const NEAREST_EVALUATED: usize = 9_408;
const TIE_EVALUATED: usize = 40_768;
