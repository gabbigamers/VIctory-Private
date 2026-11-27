//! The synthetic traversal programs (`workload::walk`) as the software
//! path, on any host: each is translated as any program is, and the
//! function its module holds is run one thread at a time
//! (`bitcode`'s `Flow`) over trees `victory-rt` builds, the node
//! instruction answered by what the translation emulates it with. What a
//! thread stores must equal, word for word, a reference of the walk written
//! here from `victory-rt`'s node test, and for the probe's walk
//! `victory-rt`'s own traversal.
//!
//! Not covered, and so not claimed: that a driver compiles or runs these
//! modules (`scripts/validate-dxil.sh` puts them before Microsoft's
//! validator; nothing ran on a GPU), two threads of a group at once (each
//! thread runs alone here; that its stack stays in its own part of the
//! group's memory is checked by address), and any program of a title.

use std::collections::HashMap;

use victory_rt::console::{BuildOptions, SLOT, SwScene};
use victory_rt::generate::{self, Class};
use victory_rt::node::{self, NodeRay};
use victory_rt::rng::Rng;
use victory_rt::scene::{BottomLevel, BuildFlags, Geometry, Indices, Instance, Scene, Transform};
use victory_rt::traverse::{self, Boxes, Quotient};
use victory_rt::{Hit, Ray};

use super::lower::{self, Lowered, Parts};
use super::{ROOT_SRV_VIEW, ROOT_UAV_VIEW, ROOT_VIEW_OFFSET, UserData};
use crate::native::RootType;
use crate::workload::walk::{
    self, CAP, GROUP_THREADS, HIT_BYTES, RAW_TREE, RAY_BYTES, ROOT_COUNT, ROOT_POINTER, ROOT_TABLE,
    ROOT_TREE, STACK_ENTRIES, Walk,
};
use crate::{io, ir, resource, structure};

/// The walks that walk a tree with a stack and are translated.
fn stack_walks() -> impl Iterator<Item = Walk> {
    Walk::ALL
        .into_iter()
        .filter(|w| w.stack() && w.translates())
}

fn lowered(walk: Walk) -> Lowered {
    let native = walk.native();
    let p = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
    let st = structure::structure(&p).unwrap();
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
    let io = io::raise(&p, &native).unwrap();
    let l = lower::lower(&Parts {
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: vec![],
        outputs: vec![],
        prims: vec![],
        mesh_stride: 0,
        view_marks: false,
    })
    .unwrap();
    l.flow.check().unwrap();
    l
}

/// Where the tree lies in title memory, and the heap's slots: the table's
/// first descriptor (the rays; the hits follow) and the view of the memory
/// that holds the tree.
const TITLE: u64 = RAW_TREE;
const TABLE_SLOT: u32 = 40;
const TREE_VIEW: u32 = 7;
const NULL_VIEW: u32 = 3;

/// A handle made from a binding (the heap's are their slot).
const BOUND: u64 = 1 << 40;

/// What thread `n` of a dispatch of `l` over `tree` (a region of 64-byte
/// slots, its root box node in slot 0) and `rays` stores: its hit record,
/// if it stored one. A word is stored once, unless `rewrites` (the walk
/// that stores inside its loop), where the last store stays.
fn thread(
    l: &Lowered,
    tree: &[u32],
    rays: &[u32],
    count: u32,
    n: u32,
    rewrites: bool,
) -> Option<[u32; 8]> {
    let mut user = [0u32; 16];
    for &UserData {
        index,
        slot,
        root,
        dword,
    } in &l.user_data
    {
        user[index as usize] = match (slot, root, dword) {
            (ROOT_TREE, RootType::Descriptor, 0) => TITLE as u32,
            (ROOT_TREE, RootType::Descriptor, 1) => (TITLE >> 32) as u32,
            (ROOT_TABLE, RootType::Table, 0) => 32 * TABLE_SLOT,
            (ROOT_COUNT, RootType::Constants, 0) => count,
            other => panic!("user data {other:?}"),
        };
        assert!(
            ![ROOT_SRV_VIEW, ROOT_UAV_VIEW, ROOT_VIEW_OFFSET].contains(&dword) || slot != ROOT_TREE
        );
    }
    let pages =
        (TITLE >> 16 & 0xFFF) as u32..=((TITLE + 4 * tree.len() as u64 - 1) >> 16 & 0xFFF) as u32;
    let mut bound = 0u64;
    let mut out = [None::<u32>; 8];
    let mut call = |name: &str, args: &[u64]| -> Vec<u64> {
        let word = |words: &[u32], byte: u64| -> u64 {
            assert!(byte.is_multiple_of(4), "{name} at {byte:#x}");
            u64::from(words.get((byte / 4) as usize).copied().unwrap_or(0))
        };
        match name {
            "dx.op.cbufferLoadLegacy.i32" => {
                let row = args[2] as usize * 4;
                user[row..row + 4].iter().map(|&x| u64::from(x)).collect()
            }
            "dx.op.groupId.i32" => {
                vec![u64::from(if args[1] == 0 { n / GROUP_THREADS } else { 0 })]
            }
            "dx.op.threadIdInGroup.i32" => {
                vec![u64::from(if args[1] == 0 { n % GROUP_THREADS } else { 0 })]
            }
            "dx.op.createHandleFromBinding" => {
                bound += 1;
                vec![BOUND + bound]
            }
            "dx.op.createHandleFromHeap" | "dx.op.annotateHandle" => vec![args[1]],
            "dx.op.rawBufferLoad.f32" | "dx.op.rawBufferLoad.i32" => {
                let (view, at) = (args[1], args[2] & 0xFFFF_FFFF);
                (0..4)
                    .map(|k| {
                        let at = at + 4 * k;
                        match view {
                            // The page table: level 1, then block 0 (no
                            // copy) and block 1, two dwords an entry.
                            v if v > BOUND => {
                                let dword = at / 4;
                                if dword < 4096 {
                                    return u64::from(dword == TITLE >> 28);
                                }
                                let (entry, second) = ((dword - 4096) / 2, (dword - 4096) % 2);
                                let held =
                                    entry / 4096 == 1 && pages.contains(&((entry % 4096) as u32));
                                u64::from(match (held, second) {
                                    (true, 0) => TREE_VIEW,
                                    (true, _) => TITLE as u32,
                                    (false, 0) => NULL_VIEW,
                                    (false, _) => 0,
                                })
                            }
                            v if v == u64::from(TABLE_SLOT) => word(rays, at),
                            v if v == u64::from(TREE_VIEW) => word(tree, at),
                            v => panic!("a load through view {v}"),
                        }
                    })
                    .collect()
            }
            "dx.op.rawBufferStore.i32" => {
                assert_eq!(
                    args[1],
                    u64::from(TABLE_SLOT + 1),
                    "a store through another view"
                );
                let at = (args[2] & 0xFFFF_FFFF) - u64::from(HIT_BYTES * n);
                for k in 0..4 {
                    if args[8] >> k & 1 != 0 {
                        let slot = &mut out[(at / 4) as usize + k];
                        assert!(
                            slot.replace(args[4 + k] as u32).is_none() || rewrites,
                            "two stores of a word"
                        );
                    }
                }
                Vec::new()
            }
            // A float as it is, with denormals read as zeros or not: the
            // node test's and the walk's numbers are far from those.
            "dx.op.legacyF16ToF32" => vec![u64::from(node::half_to_f32(args[1] as u32).to_bits())],
            "dx.op.atomicBinOp.i32" => panic!("thread {n} missed a page of title memory"),
            other => panic!("a call these tests do not answer: {other}"),
        }
    };
    let mut stack = HashMap::new();
    l.flow
        .run_shared(1 << 26, &mut stack, &mut call)
        .unwrap_or_else(|e| panic!("thread {n}: {e}"));
    // The thread's stack is its own part of the group's memory.
    let own = u64::from(n % GROUP_THREADS * STACK_ENTRIES);
    for at in stack.keys().map(|a| a & 0xFFFF_FFFF) {
        assert!(
            (own..own + u64::from(STACK_ENTRIES)).contains(&at),
            "thread {n} at dword {at}"
        );
    }
    if out.iter().all(Option::is_none) {
        return None;
    }
    Some(out.map(|w| w.expect("a hit record stored whole")))
}

/// The walk's specification, in `victory-rt`'s node test: what `walk`
/// stores for `ray` (its twelve words) over `tree`; nothing where the
/// translation's loop limit would stop it (never, in these tests).
fn reference(walk: Walk, tree: &[u32], ray: &[u32; 12]) -> [u32; 8] {
    use Walk::*;
    let f = |k: usize| f32::from_bits(ray[k]);
    let (mut origin, tmin, dir, tmax) = ([f(0), f(1), f(2)], f(3), [f(4), f(5), f(6)], f(7));
    let inv = match walk {
        LoadedInverse => [f(8), f(9), f(10)],
        _ => dir.map(|d| 1.0 / d),
    };
    let nearest = !matches!(walk, Any | LastHit | NoExtent);
    let (mut best, mut u, mut v, mut kind, mut kept, mut count) =
        (tmax, 0f32, 0f32, 0u32, 0u32, 0u32);
    // The second pass of two starts from what the first found.
    for _ in 0..if walk == TwoPasses { 2 } else { 1 } {
        let mut stack = vec![ROOT_POINTER];
        while let Some(pointer) = stack.pop() {
            let at = (pointer >> 3) as usize * SLOT;
            let extent = if nearest { best } else { tmax };
            let ray = NodeRay {
                extent,
                origin,
                dir,
                inv,
            };
            let r = node::intersect(&tree[at..], pointer, &ray);
            if walk == Capped {
                count += 1;
            }
            if pointer & 7 < 4 {
                let [tn, d, i, j] = r.map(f32::from_bits);
                let rcp = 1.0 / d;
                let (t, cu, cv) = (tn * rcp, i * rcp, j * rcp);
                if walk == Counting {
                    count += 1;
                }
                let within = match walk {
                    NoExtent => true,
                    NearestInclusive => t <= extent,
                    // "Not greater or equal": less, or one is a NaN.
                    NearestUnordered => t < extent || t.is_nan() || extent.is_nan(),
                    BitsCompare => (t.to_bits() as i32) < (extent.to_bits() as i32),
                    TieByKey => t < best || (t == best && pointer < kept),
                    _ => t < extent,
                };
                let accepted = if walk == AcceptEither {
                    tmin < t || within
                } else {
                    tmin < t && within
                };
                if accepted {
                    if walk == Any {
                        kind = 1;
                        stack.clear();
                    } else {
                        (best, u, v, kind) = (t, cu, cv, if d > 0.0 { 1 } else { 2 });
                        kept = pointer;
                    }
                }
            } else {
                let pushed = if walk == SkipsChild { 3 } else { 4 };
                stack.extend(
                    r[..pushed]
                        .iter()
                        .rev()
                        .filter(|&&child| child != node::INVALID),
                );
                if walk == MovingRay {
                    origin[0] += dir[0];
                }
            }
            if (walk == Capped && count >= CAP) || (walk == EarlyExit && kind != 0) {
                break;
            }
        }
    }
    let t = if kind != 0 && walk != Any { best } else { 0.0 };
    let mut out = [t.to_bits(), u.to_bits(), v.to_bits(), kind, 0, 0, 0, 0];
    match walk {
        NearestIds if kind != 0 => {
            let at = (kept >> 3) as usize * SLOT;
            (out[6], out[7]) = (tree[at + 12] & 0x7FFF_FFFF, tree[at + 13]);
        }
        NearestNode | TieByKey => out[7] = if kind != 0 { kept } else { 0 },
        Counting => out[7] = count,
        _ => {}
    }
    out
}

/// A bottom level of `layers` squares one above the other, each two
/// triangles, and beside them a fan of slivers around one vertex.
fn layers(layers: usize) -> BottomLevel {
    let mut geometries: Vec<Geometry> = (0..layers)
        .map(|k| Geometry {
            vertices: vec![
                [0.0, 0.0, k as f32],
                [0.0, 1.0, k as f32],
                [1.0, 0.0, k as f32],
                [1.0, 1.0, k as f32],
            ],
            indices: Indices::U32(vec![0, 1, 2, 2, 1, 3]),
            transform: None,
            opaque: true,
        })
        .collect();
    let mut fan = vec![[3.0, 0.0, 0.5]];
    fan.extend((0..9).map(|k| {
        let a = k as f32 * 0.4;
        [3.0 + a.cos(), a.sin(), 0.5 + 0.125 * k as f32]
    }));
    geometries.push(Geometry {
        vertices: fan,
        indices: Indices::U32((1..9).flat_map(|k| [0, k, k + 1]).collect()),
        transform: None,
        opaque: true,
    });
    BottomLevel {
        geometries,
        flags: BuildFlags::default(),
    }
}

/// The bottom levels the walks are run over: the squares and the fan, and
/// every bottom level of some generated scenes, each geometry taken as
/// opaque.
fn bottoms() -> Vec<BottomLevel> {
    let mut all = vec![layers(1), layers(7)];
    for class in [Class::StaticSoup, Class::Overlapping, Class::Precision] {
        let case = generate::generate(class, generate::DEFAULT_SEED);
        all.extend(
            case.scene
                .bottoms
                .into_iter()
                .filter(|b| b.primitive_count() > 0)
                .take(3),
        );
    }
    for bottom in &mut all {
        for g in &mut bottom.geometries {
            g.opaque = true;
        }
    }
    all
}

/// `bottom` alone under an identity instance of id 0, as `victory-rt`
/// writes it, padded for rays from within `reach` of the origin.
fn image(bottom: &BottomLevel, reach: f32) -> SwScene {
    let scene = Scene {
        bottoms: vec![bottom.clone()],
        instances: vec![Instance {
            transform: Transform::IDENTITY,
            id: 0,
            mask: 0xFF,
            flags: 0,
            blas: 0,
        }],
        flags: BuildFlags::default(),
    };
    SwScene::build(&scene, BuildOptions::reach(reach)).unwrap()
}

/// Rays at `bottom`: each from a point near it towards a point of one of
/// its triangles (met at `t` = 1) or past them, with a range that ends
/// before, at or behind it. No component is a negative zero.
fn rays_at(bottom: &BottomLevel, count: usize, seed: u64) -> (Vec<Ray>, f32) {
    let mut rng = Rng::new(seed);
    let triangles: Vec<[[f32; 3]; 3]> = bottom
        .geometries
        .iter()
        .flat_map(|g| (0..g.primitive_count()).map(move |p| g.triangle(p)))
        .collect();
    let far = triangles
        .iter()
        .flatten()
        .flatten()
        .fold(1.0f32, |m, x| m.max(x.abs()));
    let mut rays = Vec::new();
    while rays.len() < count {
        let tri = triangles[rng.below(triangles.len() as u32) as usize];
        let (a, b) = (rng.unit(), rng.unit());
        let (a, b) = if a + b > 1.0 {
            (1.0 - a, 1.0 - b)
        } else {
            (a, b)
        };
        let spread = if rng.chance(1, 8) { 0.25 * far } else { 0.0 };
        let target: [f32; 3] = std::array::from_fn(|k| {
            tri[0][k]
                + a * (tri[1][k] - tri[0][k])
                + b * (tri[2][k] - tri[0][k])
                + spread * (rng.unit() - 0.5)
        });
        let origin: [f32; 3] = std::array::from_fn(|_| (rng.unit() - 0.5) * 3.0 * far);
        let dir: [f32; 3] = std::array::from_fn(|k| target[k] - origin[k]);
        let tmax = *rng.pick(&[f32::INFINITY, 4.0, 1.5, 1.0, 0.75]);
        let tmin = *rng.pick(&[0.0, 0.0, 0.25]);
        let ray = Ray::new(origin, dir, tmin, tmax);
        let signed_zero = origin
            .iter()
            .chain(&dir)
            .any(|x| x.to_bits() == 0x8000_0000);
        if ray.validate().is_ok() && !signed_zero {
            rays.push(ray);
        }
    }
    (rays, 2.0 * far)
}

/// Every stack walk, over every tree: what each thread stores is what the
/// walk's specification gives, and a thread with no ray stores nothing.
#[test]
fn each_walk_stores_what_its_specification_gives() {
    let bottoms = bottoms();
    for walk in stack_walks() {
        let l = lowered(walk);
        // The container the walk's own entry gives is this module.
        assert_eq!(
            walk::translate(walk).unwrap().shader.dxil,
            l.dxil,
            "{walk:?}"
        );
        let (mut hits, mut total) = (0, 0);
        for (b, bottom) in bottoms.iter().enumerate() {
            // Fewer rays for the walks that differ from the first in one
            // compare: the trees and the stack are the same code.
            let count: u32 = if matches!(walk, Walk::Nearest | Walk::NearestIds | Walk::Any) {
                96
            } else {
                24
            };
            let (rays, reach) = rays_at(bottom, count as usize, 0x5EED + b as u64);
            let sw = image(bottom, reach);
            let tree = &sw.words[sw.bottoms[0].base..][..sw.bottoms[0].words];
            assert!(
                sw.stack <= STACK_ENTRIES,
                "a tree the walk's stack does not hold"
            );
            let mut words: Vec<u32> = rays.iter().flat_map(|r| r.to_words()).collect();
            assert_eq!(words.len() as u32 * 4, RAY_BYTES * count);
            if walk == Walk::LoadedInverse {
                // This walk's record ends with the inverse direction where
                // the others' has the mask and the flags, which none reads.
                for ray in words.as_chunks_mut::<12>().0 {
                    for k in 0..3 {
                        ray[8 + k] = (1.0 / f32::from_bits(ray[4 + k])).to_bits();
                    }
                }
            }
            for n in 0..count {
                let ray: &[u32; 12] = words[12 * n as usize..][..12].try_into().unwrap();
                let got = thread(&l, tree, &words, count, n, walk == Walk::StoresInLoop)
                    .expect("a hit record");
                assert_eq!(
                    got,
                    reference(walk, tree, ray),
                    "{walk:?}, tree {b}, ray {n}: {:?}",
                    rays[n as usize]
                );
                hits += u32::from(got[3] != 0);
                total += 1;
            }
            // Past the ray count, in the same group and in the next.
            for n in [count, count + GROUP_THREADS] {
                assert_eq!(
                    thread(&l, tree, &words, count, n, false),
                    None,
                    "{walk:?}: thread {n}"
                );
            }
        }
        // Rays that hit and rays that miss, of each walk. (The walk whose
        // origin moves a whole direction at every box node loses most of
        // its triangles.)
        eprintln!(
            "{walk:?}: {} trees, {total} rays, {hits} hits",
            bottoms.len()
        );
        let few = if walk == Walk::MovingRay {
            1
        } else {
            total / 8
        };
        assert!(
            hits >= few && hits < total,
            "{walk:?}: {hits} hits of {total} rays"
        );
    }
}

/// The probe's walk against `victory-rt`'s traversal of the same tree
/// under one identity instance of id 0, its quotients formed as a console
/// program forms them: the same hit record, bit for bit.
#[test]
fn the_probes_walk_equals_the_traversal_model() {
    let l = lowered(Walk::NearestIds);
    assert_eq!(walk::software().unwrap().shader.dxil, l.dxil);
    let (mut hits, mut differ) = (0, 0);
    for (b, bottom) in bottoms().iter().enumerate() {
        let (rays, reach) = rays_at(bottom, 160, 0xA11CE + b as u64);
        let sw = image(bottom, reach);
        let tree = &sw.words[sw.bottoms[0].base..][..sw.bottoms[0].words];
        let words: Vec<u32> = rays.iter().flat_map(|r| r.to_words()).collect();
        for (n, ray) in rays.iter().enumerate() {
            let (want, _) =
                traverse::trace_as(&sw, ray, Boxes::Tested, Quotient::Reciprocal).unwrap();
            let got =
                thread(&l, tree, &words, rays.len() as u32, n as u32, false).expect("a hit record");
            assert_eq!(Hit::from_words(&got), want, "tree {b}, ray {n}: {ray:?}");
            hits += u32::from(!want.is_miss());
            // The model as it is by default divides: the same triangle,
            // numbers that can differ in their last bit.
            let (divided, _) = traverse::trace(&sw, ray, Boxes::Tested).unwrap();
            differ += u32::from(divided != want);
            if divided.ids() == want.ids() && !want.is_miss() {
                let ulps = |a: f32, b: f32| (a.to_bits() as i64 - b.to_bits() as i64).abs();
                assert!(
                    ulps(divided.t, want.t) <= 1,
                    "tree {b}, ray {n}: {divided:?} {want:?}"
                );
            }
        }
    }
    eprintln!(
        "the probe's walk: {} trees, {hits} hits, {differ} rays differ from the dividing model",
        bottoms().len()
    );
    assert!(hits > 300, "{hits}");
    // Why the model has both forms: they are not the same numbers.
    assert!(differ > 0);
}

/// What the recogniser says of a walk's one loop.
fn description(walk: Walk) -> crate::workload::Description {
    let native = walk.native();
    let p = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
    let st = structure::structure(&p).unwrap();
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
    let io = io::raise(&p, &native).unwrap();
    let report = crate::workload::recognise(&crate::workload::Parts {
        native: &native,
        p: &p,
        st: Some(&st),
        res: Some(&res),
        io: Some(&io),
        lowered: walk.translates(),
    });
    let [finding] = &report.loops[..] else {
        panic!("{walk:?}: {} loops", report.loops.len());
    };
    finding.result.clone().expect("a recognised walk")
}

/// A candidate is not a hit. The node test answers a triangle the ray
/// misses with a distance of +inf, and the walk whose accept is
/// `t <= extent` takes that for a hit while its extent is +inf, as the one
/// whose accept is "not `t >= extent`" does while its extent is a NaN: a
/// ray that hits nothing then stores a hit at +inf with weights of 0 and a
/// front face, if it came to test any triangle. `victory-rt`'s traversal
/// model, which skips a miss, says for each ray what a hit is; the
/// description says of each walk whether its accept passes the miss word
/// (`Extent::miss_word`), and this holds the two to what the translated
/// walks store.
#[test]
fn a_missed_triangle_is_accepted_only_where_the_description_says_so() {
    let miss_word = [f32::INFINITY.to_bits(), 0, 0, 1, 0, 0, 0, 0];
    let (mut rays_run, mut stored) = (0, [[0u32; 2]; 3]);
    let walks = [
        Walk::Nearest,
        Walk::NearestInclusive,
        Walk::NearestUnordered,
    ];
    for (w, walk) in walks.into_iter().enumerate() {
        let l = lowered(walk);
        let says = description(walk).ray.extent.miss_word;
        for (e, extent) in [f32::INFINITY, f32::NAN].into_iter().enumerate() {
            let passes = [says.at_infinite, says.at_nan][e];
            // An ordered compare is false for a NaN extent whatever `t`.
            let ordered_nan = extent.is_nan() && walk != Walk::NearestUnordered;
            for (b, bottom) in bottoms().iter().enumerate() {
                let (mut rays, reach) = rays_at(bottom, 48, 0x1F + b as u64);
                for ray in &mut rays {
                    ray.tmax = f32::INFINITY;
                    assert!(ray.validate().is_ok());
                }
                let sw = image(bottom, reach);
                let tree = &sw.words[sw.bottoms[0].base..][..sw.bottoms[0].words];
                // The walk is given the extent under test; the model the
                // infinite one, which a NaN is to an unordered compare and
                // to the box test.
                let mut words: Vec<u32> = rays.iter().flat_map(|r| r.to_words()).collect();
                for ray in words.as_chunks_mut::<12>().0 {
                    ray[7] = extent.to_bits();
                }
                for (n, ray) in rays.iter().enumerate() {
                    let (want, work) =
                        traverse::trace_as(&sw, ray, Boxes::Tested, Quotient::Reciprocal).unwrap();
                    let got = thread(&l, tree, &words, rays.len() as u32, n as u32, false)
                        .expect("a hit record");
                    let at = format!("{walk:?}, extent {extent}, tree {b}, ray {n}: {ray:?}");
                    rays_run += 1;
                    if ordered_nan {
                        assert_eq!(got, [0; 8], "{at}");
                    } else if !want.is_miss() {
                        // A hit: the model's distance. (Among candidates at
                        // one distance the inclusive walk keeps the last,
                        // the model the first, so only the distance and
                        // that there is a hit are the same for it.)
                        assert_eq!(got[0], want.t.to_bits(), "{at}");
                        assert_ne!(got[3], 0, "{at}");
                        if walk != Walk::NearestInclusive {
                            let model = [want.t, want.u, want.v].map(f32::to_bits);
                            assert_eq!((&got[..3], got[3]), (&model[..], want.kind), "{at}");
                        }
                    } else if passes && work.triangle_tests > 0 {
                        // No hit, and a triangle was tested: the miss word.
                        assert_eq!(got, miss_word, "{at}");
                        stored[w][e] += 1;
                    } else {
                        assert_eq!(got, [0; 8], "{at}");
                    }
                }
            }
        }
    }
    eprintln!("{rays_run} rays; missed triangles stored as hits, by walk and extent: {stored:?}");
    // The strict ordered walk never; the inclusive one at +inf only; the
    // unordered one at a NaN only, and on the same rays.
    assert_eq!(stored[0], [0, 0]);
    assert!(stored[1][0] > 0 && stored[1][1] == 0, "{stored:?}");
    assert_eq!(stored[2], [0, stored[1][0]]);
}

/// The programs that are no software path. The wave's walk has no per-lane
/// form, as the title's wave walks have none, and is given its emulated
/// wave only where that is asked for; the lowering refuses a select by a
/// constant lane mask and packed directions; the loop without a node
/// instruction translates as any loop.
#[test]
fn the_programs_that_walk_no_tree_translate_as_such_programs_do() {
    let refused = |walk: Walk| match walk::translate(walk) {
        Err(crate::ShaderError::Unsupported { offset, what }) => (offset, what),
        other => panic!("{walk:?}: {:?}", other.map(|t| t.shader.dxil.len())),
    };
    assert_eq!(
        refused(Walk::WaveWalk),
        (0xa4, "a lane-mask join of an untracked mask")
    );
    assert_eq!(refused(Walk::LaneNumber).1, "a constant lane mask");
    assert_eq!(
        refused(Walk::PackedDirections).1,
        "a BVH intersection with 16-bit directions"
    );
    assert!(walk::translate(Walk::NoNode).is_ok());
    for walk in Walk::ALL {
        assert_eq!(walk::translate(walk).is_ok(), walk.translates(), "{walk:?}");
    }
}
