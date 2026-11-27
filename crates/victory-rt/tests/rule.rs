//! The proposed rule for functionally equivalent rendering on this host:
//! the measurement of its tolerance constant, and the CPU model of the
//! software path judged under it on every scene class, levels 1 to 3.
//! Nothing here touches a GPU, and no verdict is a hardware result: what
//! these tests show is that the rule's reference and the software model,
//! written apart, agree under the rule, and that the rule sees a defect
//! when one is planted.
//!
//! `cargo test -p victory-rt --test rule -- --nocapture` prints the tables
//! of docs/dxr-validation-reference.md.

use victory_rt::compare::{Verdict, compare};
use victory_rt::console::{BuildOptions, SwScene};
use victory_rt::generate::{
    BIAS_UNIT, BIASES, CLOSED_OUTSIDE, Case, Class, ClassKind, DEFAULT_SEED, generate,
};
use victory_rt::picture;
use victory_rt::record::{Hit, Ray, hit_kind, instance_flag, ray_flag, refusal};
use victory_rt::reference::Reference;
use victory_rt::rule::{
    self, Constants, Evaluation, K, KFit, Level1, Level2, Rule, RuleOutcome, RuleVerdict, kind,
    special,
};
use victory_rt::scene::Scene;
use victory_rt::traverse::{Boxes, trace, trace_all};

const C: Constants = Constants::PROPOSED;

/// The scene of each frame of a class and the software path's records on
/// it: refits where the structures allow, builds where they do not.
fn frames(case: &Case) -> Vec<(Scene, Vec<Hit>)> {
    let options = BuildOptions::for_rays(&case.rays);
    let mut scene = case.scene.clone();
    let mut image = SwScene::build(&scene, options).unwrap();
    let mut out = Vec::new();
    for f in 0..case.frame_count() {
        if f > 0 {
            let actions = case.frames[f - 1].apply(&mut scene).unwrap();
            image.update(&scene, &actions).unwrap();
        }
        out.push((scene.clone(), trace_all(&image, &case.rays).unwrap()));
    }
    out
}

/// Levels 1 and 2 of a class's frames together, under `c`.
fn judged(
    case: &Case,
    all: &[(Scene, Vec<Hit>)],
    c: &Constants,
) -> (Vec<Evaluation>, Level1, Level2) {
    let evals: Vec<Evaluation> = all
        .iter()
        .map(|(scene, hits)| rule::evaluate(scene, &case.rays, hits, c).unwrap())
        .collect();
    let (l1, l2) = rule::sum(&evals.iter().collect::<Vec<_>>());
    (evals, l1, l2)
}

fn by(l1: &Level1, bit: u32) -> u32 {
    let at = kind::NAMES.iter().position(|(b, _)| *b == bit).unwrap();
    l1.by_kind[at]
}

// ---- the tolerance constant ----

fn fit(class: Class, seed: u64) -> KFit {
    let case = generate(class, seed);
    let mut fit = KFit::NONE;
    for scene in case.scenes() {
        fit.merge(&rule::measure_k(&scene, &case.rays, C.graze_cos));
    }
    fit
}

#[test]
fn the_tolerance_constant_is_measured_not_chosen() {
    println!(
        "| class | pairs in range | with an error to measure | largest error, in 2^-23 M | of t | of a weight | twice the largest, rounded up |\n|---|---|---|---|---|---|---|"
    );
    let mut first = KFit::NONE;
    for class in Class::ALL {
        if class.kind() != ClassKind::Random {
            continue;
        }
        let f = fit(class, DEFAULT_SEED);
        println!(
            "| {} | {} | {} | {:.3} | {:.3} | {:.3} | {} |",
            class.name(),
            f.pairs,
            f.compared,
            f.largest,
            f.t_part,
            f.b_part,
            f.k(2.0)
        );
        // Every class has pairs to measure, and the errors are those of
        // single precision: a few spacings.
        assert!(
            f.compared > 0 && f.largest > 0.0 && f.largest < 4.0,
            "{}",
            class.name()
        );
        first.merge(&f);
    }
    println!(
        "all random classes, the documented seed: {} pairs, {} measured, largest error {:.4}; alone it gives k = {}",
        first.pairs,
        first.compared,
        first.largest,
        first.k(2.0)
    );
    // The constant is the measurement over the documented seed and the 200
    // that follow it (`more_seeds_by_hand`). Here: the documented seed and
    // the one of them that carries the sample's largest error. The constant
    // in the crate, in the result file and in the documents is that
    // measurement, and a change to the generator or to the model of the
    // emulated test that moves it fails here.
    assert_eq!(rule::K_SEEDS[0], 0);
    let mut all = first;
    for &seed in &rule::K_SEEDS[1..] {
        let mut carried = KFit::NONE;
        let mut worst = "";
        for class in Class::ALL {
            if class.kind() == ClassKind::Random {
                let f = fit(class, DEFAULT_SEED + seed);
                if f.largest > carried.largest {
                    worst = class.name();
                }
                carried.merge(&f);
            }
        }
        println!(
            "seed + {seed}: {} pairs, largest error {:.4} ({worst}); alone it gives k = {}",
            carried.pairs,
            carried.largest,
            carried.k(2.0)
        );
        all.merge(&carried);
    }
    println!(
        "the seeds measured here: largest error {:.4}; k = {}",
        all.largest,
        all.k(2.0)
    );
    assert_eq!(all.k(2.0), K);
    assert!(
        all.worst_is_numbers,
        "the largest error is one of numbers, both sides hitting"
    );
    // What a single seed gives is less: the documented one alone gives
    // half the constant. The documents say so; if this changes, they are
    // to change with it.
    assert_eq!(first.k(2.0), K / 2.0);
    // The t part carries the constant; the weights' part is well under it.
    assert!(all.t_part == all.largest && all.b_part < 1.0, "{all:?}");
    // The margin holds at other seeds: no error is over half the constant.
    for seed in 1..=3 {
        for class in Class::ALL {
            if class.kind() == ClassKind::Random {
                let f = fit(class, DEFAULT_SEED + seed);
                assert!(
                    2.0 * f.largest <= K,
                    "{} seed + {seed}: {f:?}",
                    class.name()
                );
            }
        }
    }
}

#[test]
fn the_constant_is_a_property_of_the_classes_triangles() {
    // The measurement says what the emulated test does on the triangles it
    // is given. A shape no random class holds, a needle 384 to 512 times as
    // long as it is high, met at a slant and wholly inside the measured
    // range (c at least 2^-6, h c at least 2^-10 M): the error of its t
    // alone, by the first wording's tolerance (x_tol / (L c)), is several
    // times what the classes show. By the tolerance as it stands, with the
    // weights' part of t in it, it is inside the constant.
    for (aspect, offset) in [(384.0f32, 2.0f32), (512.0, 4.0)] {
        let h = 16.0 / aspect;
        let scene = Scene {
            bottoms: vec![victory_rt::scene::BottomLevel {
                geometries: vec![victory_rt::scene::Geometry {
                    vertices: vec![
                        [offset, offset, offset],
                        [offset + 16.0, offset, offset],
                        [offset, offset + h, offset],
                    ],
                    indices: victory_rt::scene::Indices::None,
                    transform: None,
                    opaque: true,
                }],
                flags: victory_rt::scene::BuildFlags::default(),
            }],
            instances: vec![victory_rt::scene::Instance {
                transform: victory_rt::scene::Transform::IDENTITY,
                id: 1,
                mask: 0xFF,
                flags: 0,
                blas: 0,
            }],
            flags: victory_rt::scene::BuildFlags::default(),
        };
        scene.validate().unwrap();
        // Rays through points inside it, leaning along its length.
        let mut rng = victory_rt::rng::Rng::new(0x0BAD_5EED ^ aspect as u64);
        let mut rays = Vec::new();
        for _ in 0..20_000 {
            let (mut u, mut v) = (rng.unit(), rng.unit());
            if u + v > 1.0 {
                (u, v) = (1.0 - u, 1.0 - v);
            }
            let p = [offset + 16.0 * u, offset + h * v, offset];
            let d = [
                0.5 + rng.unit(),
                (2.0 * rng.unit() - 1.0) * 0.125,
                1.0 + rng.unit(),
            ];
            let back = 0.25 + rng.unit();
            let o: [f32; 3] = std::array::from_fn(|a| p[a] - d[a] * back);
            let ray = Ray::new(o, d, 0.0, 100.0);
            ray.validate().unwrap();
            rays.push(ray);
        }
        let f = rule::measure_k(&scene, &rays, C.graze_cos);
        // The t error by the first wording, on the same pairs: the error
        // of t times L c, in units of 2^-23 M.
        let reference = Rule::new(&scene, C);
        let mut exact = Reference::new(&scene);
        let mut first = 0.0f64;
        for ray in &rays {
            let (Some([t, ..]), e) = (reference.emulated(0, 0, ray), exact.exact_pair(0, 0, ray))
            else {
                continue;
            };
            if !e.inside {
                continue;
            }
            let (o, d) = (ray.origin.map(f64::from), ray.dir.map(f64::from));
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let m = o
                .iter()
                .fold(f64::from(offset) + 16.0, |m, x| m.max(x.abs()))
                .max(e.t * l);
            first = first.max((f64::from(t) - e.t).abs() * d[2] / (rule::SPACING * m));
        }
        println!(
            "a needle of aspect {aspect} at {offset}: {} pairs in range, largest error {:.3} (t {:.3}, weights {:.3}); by the first wording's t tolerance {first:.3}",
            f.pairs, f.largest, f.t_part, f.b_part
        );
        assert!(f.pairs > 15_000, "{f:?}");
        assert!(2.0 * f.largest <= K, "{f:?}");
        assert!(2.0 * first > K, "{first}");
    }
}

#[test]
fn the_measured_model_is_the_traversal_s_own_triangle_test() {
    // What `measure_k` calls the emulated test is, bit for bit, what the
    // software traversal computes for the triangle it ends on: the same
    // instance inverse, the same node, the same divisions.
    for class in [
        Class::Instancing,
        Class::Transformed,
        Class::Seam,
        Class::Far,
    ] {
        let case = generate(class, DEFAULT_SEED);
        let image = SwScene::build(&case.scene, BuildOptions::for_rays(&case.rays)).unwrap();
        let rule = Rule::new(&case.scene, C);
        let mut hits = 0;
        for ray in &case.rays {
            let (hit, _) = trace(&image, ray, Boxes::AllVisited).unwrap();
            if hit.is_miss() {
                continue;
            }
            let (i, g) = (hit.instance_index as usize, hit.geometry_index as usize);
            let bottom = &case.scene.bottoms[case.scene.instances[i].blas as usize];
            let before: usize = bottom.geometries[..g]
                .iter()
                .map(|g| g.primitive_count())
                .sum();
            let k = before + hit.primitive_index as usize;
            let got = rule.emulated(i, k, ray).map(|x| x.map(f32::to_bits));
            assert_eq!(
                got,
                Some([hit.t, hit.u, hit.v].map(f32::to_bits)),
                "{}",
                class.name()
            );
            hits += 1;
        }
        assert!(hits > 500, "{} {hits}", class.name());
    }
}

// ---- the software model under the rule ----

#[test]
fn the_software_model_passes_levels_1_and_2_on_every_class() {
    let mut counts = String::from(
        "| class | kind | rays | unambiguous | ambiguous | sliver or grazing | shared edge | closed seam | open seam | boundary edge | extent | tie | filter | degenerate | agree | admissible | not accepted | verdict |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let mut sizes = String::from(
        "| class | unambiguous hits | largest magnitude among them | among rays with a sliver or grazing pair | worst t, of tolerance | worst weight, of tolerance | consumed: equal | within | differ |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    for class in Class::ALL {
        let case = generate(class, DEFAULT_SEED);
        let all = frames(&case);
        let (_, l1, l2) = judged(&case, &all, &C);
        let v = rule::verdict(class.kind(), &l1, &l2, &C);
        // Each constant halved and doubled: the verdict again.
        let mut sensitive = Vec::new();
        for (name, c) in C.variants() {
            // The shares change the verdict's arithmetic alone; k and the
            // two thresholds change which rays are ambiguous.
            let (a, b) = if (c.k, c.graze_cos, c.sliver_b) == (C.k, C.graze_cos, C.sliver_b) {
                (l1, l2)
            } else {
                let (_, a, b) = judged(&case, &all, &c);
                (a, b)
            };
            let other = rule::verdict(class.kind(), &a, &b, &c);
            if other != v {
                sensitive.push((name, other));
            }
        }
        let text = rule::to_text(class.name(), v, &l1, &l2, &sensitive);
        // No record of the software model is one the rule does not accept,
        // at level 1 or level 2, and the exact reference's own answer is
        // admissible on every ray (`evaluate` gives an error where it is
        // not).
        assert_eq!(l1.failures(), 0, "{text}");
        assert_eq!((l2.differ, l2.not_judged), (0, 0), "{text}");
        assert_eq!(l1.agree, l1.unambiguous, "{text}");
        // Its errors are those the constant was measured from: under half
        // a tolerance.
        assert!(l1.t_err_max < 0.5 && l1.b_err_max < 0.5, "{text}");
        match class.kind() {
            ClassKind::Directed => assert_eq!(v, RuleVerdict::Admissible, "{text}"),
            ClassKind::Random => assert_ne!(v, RuleVerdict::Fail, "{text}"),
        }
        let mut verdict = format!("{v:?}");
        if !sensitive.is_empty() {
            let names: Vec<String> = sensitive
                .iter()
                .map(|(n, v)| format!("{n}: {v:?}"))
                .collect();
            verdict += &format!("; sensitive ({})", names.join(", "));
        }
        counts += &format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {verdict} |\n",
            class.name(),
            if class.kind() == ClassKind::Random {
                "random"
            } else {
                "directed"
            },
            l1.rays,
            l1.unambiguous,
            l1.ambiguous,
            l1.special,
            by(&l1, kind::SHARED_EDGE),
            by(&l1, kind::SEAM_CLOSED),
            by(&l1, kind::SEAM_OPEN),
            by(&l1, kind::BOUNDARY_EDGE),
            by(&l1, kind::EXTENT),
            by(&l1, kind::TIE),
            by(&l1, kind::FILTER),
            by(&l1, kind::DEGENERATE),
            l1.agree,
            l1.admissible,
            l1.failures(),
        );
        sizes += &format!(
            "| {} | {} | {:.1e} | {:.1e} | {:.3} | {:.3} | {} | {} | {} |\n",
            class.name(),
            l1.agree_hits,
            l1.m_max,
            l1.m_special_max,
            l1.t_err_max,
            l1.b_err_max,
            l2.equal,
            l2.within,
            l2.differ
        );
    }
    println!("{counts}\n{sizes}");
}

#[test]
fn the_tree_and_every_triangle_tested_agree_bit_for_bit_off_the_ambiguous_rays() {
    // The software path's bar is its own CPU model, bit for bit. The model
    // answers two ways: through the boxes, and with every triangle tested.
    // Where the two differ the boxes or the order of the walk have decided
    // a ray, and that is only ever a ray the rule calls ambiguous, on which
    // both answers are admissible, or a yes-or-no ray on which both say
    // yes. The run is the same twice.
    println!(
        "| class | rays | records that differ between the tree and every triangle tested | of them on unambiguous rays, the yes-or-no answer aside |\n|---|---|---|---|"
    );
    for class in Class::ALL {
        let case = generate(class, DEFAULT_SEED);
        let image = SwScene::build(&case.scene, BuildOptions::for_rays(&case.rays)).unwrap();
        let again = SwScene::build(&case.scene, BuildOptions::for_rays(&case.rays)).unwrap();
        assert_eq!(image.words, again.words, "{}", class.name());
        let tree = trace_all(&image, &case.rays).unwrap();
        assert_eq!(
            tree,
            trace_all(&again, &case.rays).unwrap(),
            "{}",
            class.name()
        );
        let rules = Rule::new(&case.scene, C).trace_all(&case.rays);
        let (mut differ, mut unambiguous) = (0, 0);
        for (k, ray) in case.rays.iter().enumerate() {
            let (all, _) = trace(&image, ray, Boxes::AllVisited).unwrap();
            assert!(
                !rule::judge(&rules[k], &all).outcome.is_failure(),
                "{} ray {k}",
                class.name()
            );
            if all != tree[k] {
                differ += 1;
                // A yes-or-no ray's answer is whether it hit: the two
                // orders may end on different triangles.
                let yes_or_no = rules[k].workload == rule::Workload::AnyHit;
                let same = yes_or_no && all.is_miss() == tree[k].is_miss();
                unambiguous += u32::from(!rules[k].ambiguous && !same);
            }
        }
        println!(
            "| {} | {} | {differ} | {unambiguous} |",
            class.name(),
            case.rays.len()
        );
        assert_eq!(unambiguous, 0, "{}", class.name());
    }
}

#[test]
#[ignore = "a sweep run by hand: SWEEP=<seeds> cargo test -p victory-rt --test rule -- --ignored --nocapture"]
fn more_seeds_by_hand() {
    // The documented seed and the `SWEEP` that follow it. With SWEEP=200
    // this is the sample the tolerance constant is measured over.
    let seeds: u64 = std::env::var("SWEEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let mut k = KFit::NONE;
    let (mut rays, mut frames_seen) = (0u64, 0u64);
    let mut by_seed = vec![0.0f64; seeds as usize + 1];
    let mut carried = (0u64, "");
    println!(
        "| class | rays | ambiguous, of rays without a sliver or grazing pair | with a sliver or grazing pair | not accepted | worst t | worst weight | pairs in range | largest error for k | at seed + |\n|---|---|---|---|---|---|---|---|---|---|"
    );
    for class in Class::ALL {
        let (mut total, mut fits, mut worst_seed) = (Vec::new(), KFit::NONE, 0);
        for seed in 0..=seeds {
            let case = generate(class, DEFAULT_SEED + seed);
            let all = frames(&case);
            // `evaluate` gives an error, and this stops, on a ray where the
            // exact reference's answer is not admissible.
            let (evals, l1, l2) = judged(&case, &all, &C);
            assert_eq!(
                l1.failures() + l2.differ,
                0,
                "{} seed + {seed}",
                class.name()
            );
            frames_seen += all.len() as u64;
            if class.kind() == ClassKind::Random {
                let mut fit = KFit::NONE;
                for (scene, _) in &all {
                    fit.merge(&rule::measure_k(scene, &case.rays, C.graze_cos));
                }
                if fit.largest > fits.largest {
                    worst_seed = seed;
                }
                by_seed[seed as usize] = by_seed[seed as usize].max(fit.largest);
                fits.merge(&fit);
            }
            total.push(evals);
        }
        let parts: Vec<&Evaluation> = total.iter().flatten().collect();
        let (l1, _) = rule::sum(&parts);
        rays += u64::from(l1.rays);
        if fits.largest > k.largest {
            carried = (worst_seed, class.name());
        }
        k.merge(&fits);
        println!(
            "| {} | {} | {} of {} (1 in {:.0}) | {} | {} | {:.3} | {:.3} | {} | {:.3} | {worst_seed} |",
            class.name(),
            l1.rays,
            l1.ambiguous,
            l1.rays - l1.special,
            f64::from(l1.rays - l1.special) / f64::from(l1.ambiguous.max(1)),
            l1.special,
            l1.failures(),
            l1.t_err_max,
            l1.b_err_max,
            fits.pairs,
            fits.largest
        );
    }
    let over: Vec<usize> = (0..by_seed.len())
        .filter(|&s| 2.0 * by_seed[s] > K / 2.0)
        .collect();
    println!(
        "{} seeds, {frames_seen} frames, {rays} rays; {} pairs in range, largest error for k {:.4} at seed + {} ({}): k = {}; t part {:.4}, weight part {:.4}",
        seeds + 1,
        k.pairs,
        k.largest,
        carried.0,
        carried.1,
        k.k(2.0),
        k.t_part,
        k.b_part
    );
    println!(
        "the documented seed alone: {:.4}; seeds that alone give k = {K}: {} of {} (+ {over:?})",
        by_seed[0],
        over.len(),
        by_seed.len()
    );
    assert!(2.0 * k.largest <= K);
}

// ---- the classes hold what they are named for ----

#[test]
fn seams_are_closed_or_open_as_their_sides_see_the_ray() {
    let case = generate(Class::SeamEdge, DEFAULT_SEED);
    let (scene, hits) = frames(&case).remove(0);
    let rules = Rule::new(&scene, C).trace_all(&case.rays);
    // 168 rays a cube, in the generator's order of cubes.
    let wanted = [
        kind::SEAM_CLOSED,
        kind::SEAM_CLOSED,
        kind::SEAM_OPEN,
        kind::SEAM_OPEN,
        kind::SEAM_OPEN,
        kind::SHARED_EDGE,
    ];
    println!(
        "| cube | rays | kind | a hit behind the seam admissible | a miss admissible | software model: one side | the other | behind |\n|---|---|---|---|---|---|---|---|"
    );
    for (cube, want) in wanted.into_iter().enumerate() {
        let (mut behind, mut near, mut other, mut through, mut misses) = (0, 0, 0, 0, 0);
        for k in 168 * cube..168 * (cube + 1) {
            let (r, hit) = (&rules[k], &hits[k]);
            assert!(r.ambiguous && r.special == 0, "ray {k}");
            // The middle vertex of a face is shared with the seam's own
            // edges and, in the last cube, with edges inside each half.
            assert_ne!(r.kinds & want, 0, "ray {k}: {:#x}", r.kinds);
            // Closed (and D3D12's own shared edge): a hit on the seam is
            // certain, and nothing behind it is admissible. Open: the far
            // side of the cube is admissible; and a miss too, for the ray
            // runs in the cube's cutting plane and leaves through the seam
            // on the other side, unless it then meets the closed seam of
            // the cube behind this one.
            let open = want == kind::SEAM_OPEN;
            let far = r
                .admissible
                .iter()
                .any(|e| e.t > f64::from(r.primary.t) * 1.001);
            assert!(!r.primary.is_miss(), "ray {k}");
            assert_eq!(far, open, "ray {k}");
            assert!(open || !r.miss_admissible, "ray {k}");
            misses += u32::from(r.miss_admissible);
            behind += u32::from(far);
            // The software model tests both sides in one object space or
            // two; it never lets the ray through.
            assert_eq!(
                rule::judge(r, hit).outcome,
                RuleOutcome::Admissible,
                "ray {k}"
            );
            if hit.t > r.primary.t * 1.001 {
                through += 1;
            } else if hit.ids() == r.primary.ids() {
                near += 1;
            } else {
                other += 1;
            }
        }
        assert_eq!(through, 0, "cube {cube}");
        assert_eq!(
            misses >= 60,
            want == kind::SEAM_OPEN,
            "cube {cube}: {misses}"
        );
        let name = kind::NAMES.iter().find(|(b, _)| *b == want).unwrap().1;
        println!("| {cube} | 168 | {name} | {behind} | {misses} | {near} | {other} | {through} |");
    }
}

#[test]
fn a_planted_hole_at_a_closed_seam_fails_and_one_at_an_open_seam_is_admissible() {
    let case = generate(Class::SeamEdge, DEFAULT_SEED);
    let (scene, hits) = frames(&case).remove(0);
    let rules = Rule::new(&scene, C).trace_all(&case.rays);
    // Every ray answered by the far side of its cube, as a path that
    // leaks through the seam would.
    let leak: Vec<Hit> = rules
        .iter()
        .zip(&hits)
        .map(|(r, own)| {
            let far = r.admissible.iter().map(|e| e.t).fold(0.0, f64::max);
            r.admissible
                .iter()
                .find(|e| e.t == far && far > f64::from(r.primary.t) * 1.001)
                .map_or(*own, |e| e.hit)
        })
        .collect();
    let e = rule::evaluate(&scene, &case.rays, &leak, &C).unwrap();
    assert_eq!(
        e.level1.failures(),
        0,
        "the far side is admissible only at open seams"
    );
    assert!(e.level1.admissible == 1008);
    // The same through the closed seams and the shared edge: the far side
    // of those cubes is not on the list, so the leak has to be built from
    // the scene. The far side is what the ray hits once the near faces are
    // gone: every hit of the near half-cubes dropped.
    let mut reference = Reference::new(&scene);
    let mut failures = [0u32; 6];
    for (k, ray) in case.rays.iter().enumerate() {
        let near = rules[k].primary.t;
        let mut later = *ray;
        later.tmin = near * 1.001;
        let beyond = reference.trace(&later).primary;
        assert!(!beyond.is_miss(), "ray {k}: the cube has a far side");
        let j = rule::judge(&rules[k], &beyond);
        failures[k / 168] += u32::from(j.outcome.is_failure());
        if j.outcome.is_failure() {
            // A hit that is there, on another triangle: of this surface
            // or (the last two cubes have two) of the other.
            assert!(
                matches!(j.outcome, RuleOutcome::IdFlip | RuleOutcome::SurfaceFlip),
                "ray {k}: {:?}",
                j.outcome
            );
        }
        // A hole: a failure wherever a hit is certain, which at a closed
        // seam it always is.
        let hole = rule::judge(&rules[k], &Hit::MISS).outcome;
        let certain = !rules[k].miss_admissible;
        assert_eq!(hole == RuleOutcome::LostHit, certain, "ray {k}");
        assert!(certain || [2, 3, 4].contains(&(k / 168)), "ray {k}");
    }
    assert_eq!(failures, [168, 168, 0, 0, 0, 168]);
}

#[test]
fn nothing_escapes_a_closed_mesh() {
    // The closed class's rays from inside a mesh (random, after its 700
    // from outside) and the closed-edge class's (on a lattice, so that many
    // go exactly through an edge or a vertex).
    for (class, first) in [(Class::Closed, CLOSED_OUTSIDE), (Class::ClosedEdge, 0)] {
        let case = generate(class, DEFAULT_SEED);
        let (scene, hits) = frames(&case).remove(0);
        let rules = Rule::new(&scene, C).trace_all(&case.rays);
        assert_eq!(case.rays.len() - first, 700);
        let (mut edges, mut exact) = (0, 0);
        for k in first..case.rays.len() {
            let (r, hit) = (&rules[k], &hits[k]);
            // From inside, the first thing met is the mesh's own inner
            // side: a back face of the instance the ray started in, by the
            // reference and by the software model, and a miss is not
            // admissible.
            let inside = ((k - first) % 4) as u32;
            assert!(!r.miss_admissible, "{} ray {k}", class.name());
            for h in [&r.primary, hit] {
                assert_eq!(
                    (h.kind, h.instance_index),
                    (hit_kind::BACK, inside),
                    "{} ray {k}",
                    class.name()
                );
            }
            let edge = kind::SHARED_EDGE | kind::SEAM_CLOSED | kind::BOUNDARY_EDGE;
            if r.ambiguous && r.special == 0 && r.kinds & edge != 0 {
                edges += 1;
                // Exactly on an edge: a weight of the reference's own hit
                // is zero.
                let w = r.admissible[0].w;
                exact += u32::from(w.contains(&0.0));
            }
        }
        println!(
            "{}: of 700 rays from inside a mesh {edges} are ambiguous by an edge, {exact} of them exactly on one",
            class.name()
        );
        match class {
            // Aimed: some go exactly through an edge between two faces;
            // six faces are six surfaces, so there the two sides can be of
            // two.
            Class::ClosedEdge => assert!(edges >= 5 && exact >= 5, "{edges} {exact}"),
            // Not aimed: none is exactly on an edge, and those within
            // tolerance of one are a few in a thousand.
            _ => assert!(exact == 0 && edges <= 7, "{edges} {exact}"),
        }
        let surfaces: std::collections::HashSet<u32> = rules
            .iter()
            .filter(|r| !r.primary.is_miss())
            .map(|r| victory_rt::shade::surface(&r.primary))
            .collect();
        assert!(surfaces.len() >= 12, "{}", surfaces.len());
    }
}

#[test]
fn rays_from_and_to_surfaces_are_at_the_extent_boundary_up_to_a_bias() {
    let case = generate(Class::SurfaceRays, DEFAULT_SEED);
    let (scene, hits) = frames(&case).remove(0);
    let rules = Rule::new(&scene, C).trace_all(&case.rays);
    // The generator's unit is one tolerance of the rule: its constant is
    // the measured one.
    assert_eq!(f64::from(BIAS_UNIT), K * rule::SPACING);
    println!(
        "| bias, in tolerances | rays that leave: at the extent boundary | of | rays that end: at the extent boundary | of | software model: the surface's own triangle hit |\n|---|---|---|---|---|---|"
    );
    let mut shares = Vec::new();
    for (level, bias) in BIASES.into_iter().enumerate() {
        let mut at = [0u32; 2];
        let mut of = [0u32; 2];
        let mut own = 0;
        for (k, r) in rules.iter().enumerate().filter(|(k, _)| k % 4 == level) {
            let ends = k / 4 % 2;
            of[ends] += 1;
            if r.kinds & kind::EXTENT != 0 {
                at[ends] += 1;
                assert_eq!(rule::judge(r, &hits[k]).outcome, RuleOutcome::Admissible);
                // What the software model does there: sees the surface
                // the ray starts or ends on, or does not.
                own += u32::from(
                    r.admissible
                        .iter()
                        .any(|e| e.kinds & kind::EXTENT != 0 && e.hit.ids() == hits[k].ids()),
                );
            }
        }
        println!(
            "| {bias} | {} | {} | {} | {} | {own} |",
            at[0], of[0], at[1], of[1]
        );
        shares.push(at);
    }
    // A ray that leaves a surface with no bias or with one tolerance of it
    // is at the boundary; with 16 it is not. One that ends on a surface is
    // at the boundary when nothing is in front of that surface, and its
    // tolerance grows with the way it has come and with its slant, so 16
    // tolerances of bias at the surface still leave some there, and 256
    // nearly none.
    assert!(shares[0][0] >= 190 && shares[1][0] >= 190, "{shares:?}");
    assert!(shares[0][1] >= 100 && shares[1][1] >= 100, "{shares:?}");
    assert!(
        shares[2][0] <= 5 && shares[3][0] == 0 && shares[3][1] <= 5,
        "{shares:?}"
    );
}

#[test]
fn the_directed_classes_are_aimed_at_what_they_name() {
    let run = |class: Class| {
        let case = generate(class, DEFAULT_SEED);
        let (scene, hits) = frames(&case).remove(0);
        let rules = Rule::new(&scene, C).trace_all(&case.rays);
        (case, rules, hits)
    };
    let with =
        |rules: &[rule::RayRule], bit: u32| rules.iter().filter(|r| r.kinds & bit != 0).count();

    // Edge: every ray is at an edge or a vertex; the shared ones never
    // admit a miss, the boundary ones do unless something is behind.
    let (_, rules, _) = run(Class::Edge);
    assert!(rules.iter().all(|r| r.ambiguous && r.special == 0));
    assert!(with(&rules, kind::SHARED_EDGE) >= 400 && with(&rules, kind::BOUNDARY_EDGE) >= 300);
    for r in &rules {
        if r.kinds == kind::SHARED_EDGE {
            assert!(!r.miss_admissible);
        }
    }
    assert!(rules.iter().filter(|r| r.miss_admissible).count() >= 200);

    // Cull-edge: an edge between a front and a back face is not one of
    // like-facing triangles, with a cull or without: a boundary.
    let (case, rules, hits) = run(Class::CullEdge);
    let mut picks = [[0u32; 3]; 3];
    for (k, r) in rules.iter().enumerate().take(252) {
        assert_eq!(
            (r.kinds, r.miss_admissible),
            (kind::BOUNDARY_EDGE, true),
            "ray {k}"
        );
        let cull = [0, 0x10, 0x20]
            .iter()
            .position(|f| *f == case.rays[k].flags)
            .unwrap();
        // With a cull one of the two is gone and one answer is left
        // beside the miss; without, both.
        assert_eq!(r.admissible.len(), if cull == 0 { 2 } else { 1 }, "ray {k}");
        picks[cull][hits[k].kind as usize] += 1;
    }
    println!(
        "cull-edge, the software model on the shared edge, [miss, front, back] with no cull, back faces culled, front faces culled: {picks:?}"
    );
    assert_eq!(
        (picks[1][2], picks[2][1]),
        (0, 0),
        "a culled face is never reported"
    );

    // Extent: by the ray's place in its ten.
    let (case, rules, hits) = run(Class::Extent);
    let mut zero = [0u32; 2];
    for (k, r) in rules.iter().enumerate() {
        let at_boundary = r.kinds & kind::EXTENT != 0;
        match k % 10 {
            0..=7 => assert!(at_boundary, "ray {k}"),
            _ => assert!(!at_boundary, "ray {k}"),
        }
        if k % 10 == 6 || k % 10 == 7 {
            // From the surface: a hit at exactly t = 0 is admissible, and
            // so is going on. D3D12 excludes it (tmin < t); the emulated
            // node test includes it (t >= 0), and the traversal around it
            // here does not.
            assert_eq!(case.rays[k].origin[2], 2.0);
            let at_zero = r.admissible.iter().any(|e| e.t == 0.0);
            assert!(at_zero, "ray {k}");
            zero[usize::from(hits[k].t == 0.0 && !hits[k].is_miss())] += 1;
        }
    }
    println!(
        "extent, rays that start on a surface: the software model goes on {} times and reports t = 0 {} times",
        zero[0], zero[1]
    );

    // Tie: two answers within tolerance, of one surface or of two.
    let (_, rules, _) = run(Class::Tie);
    assert!(with(&rules, kind::TIE) >= 500);
    let two_surfaces = rules
        .iter()
        .filter(|r| {
            r.kinds & kind::TIE != 0
                && r.admissible.iter().any(|e| {
                    victory_rt::shade::surface(&e.hit) != victory_rt::shade::surface(&r.primary)
                })
        })
        .count();
    assert!(two_surfaces >= 100, "{two_surfaces}");

    // Thin, large-thin, far-graze: where every pair, or most, is a sliver
    // or grazing pair.
    for (class, least) in [
        (Class::Thin, 250),
        (Class::LargeThin, 720),
        (Class::FarGraze, 966),
    ] {
        let (_, rules, _) = run(class);
        let apart = rules.iter().filter(|r| r.special != 0).count();
        assert!(apart >= least, "{} {apart}", class.name());
    }
    let (_, rules, _) = run(Class::FarGraze);
    let grazing = rules
        .iter()
        .filter(|r| r.special & special::GRAZING != 0)
        .count();
    println!(
        "far-graze: {grazing} of {} rays with a grazing pair, the rest with a sliver pair alone",
        rules.len()
    );
    assert!(grazing >= 100, "{grazing}");

    // Degenerate: rays that pass a triangle of no area, on which the
    // software model's node test can report a hit; it is admissible.
    let (case, rules, hits) = run(Class::Degenerate);
    assert!(with(&rules, kind::DEGENERATE) >= 150);
    let on_nothing = rules
        .iter()
        .zip(&hits)
        .filter(|(r, h)| {
            r.admissible
                .iter()
                .any(|e| e.special & special::DEGENERATE != 0 && e.hit.ids() == h.ids())
        })
        .count();
    println!(
        "degenerate: the software model reports a hit on a triangle of no area on {on_nothing} of {} rays that pass one",
        with(&rules, kind::DEGENERATE)
    );
    // The exact reference never does.
    let none = &case.scene.bottoms[0].geometries;
    assert!(
        rules
            .iter()
            .all(|r| r.primary.is_miss() || r.primary.geometry_index >= 2)
    );
    assert_eq!(none.len(), 4);
}

#[test]
fn far_triangles_are_slivers_to_the_tolerance() {
    // 300 rays a cluster: 2^12, 2^14 and 2^16 from the origin in one
    // coordinate, 2^16 in all three, and 2^14 by an instance.
    let case = generate(Class::Far, DEFAULT_SEED);
    let (scene, hits) = frames(&case).remove(0);
    let e = rule::evaluate(&scene, &case.rays, &hits, &C).unwrap();
    println!(
        "| cluster | rays | unambiguous | ambiguous | with a sliver or grazing pair | largest weight tolerance among the unambiguous |\n|---|---|---|---|---|---|"
    );
    let names = [
        "2^12",
        "2^14",
        "2^16",
        "2^16 on three axes",
        "2^14 by an instance",
    ];
    let mut apart = Vec::new();
    for (c, name) in names.iter().enumerate() {
        let rules = &e.rules[300 * c..300 * (c + 1)];
        let special = rules.iter().filter(|r| r.special != 0).count();
        let ambiguous = rules.iter().filter(|r| r.ambiguous).count() - special;
        let b = rules
            .iter()
            .filter(|r| !r.ambiguous)
            .flat_map(|r| &r.admissible)
            .map(|a| a.b_tol)
            .fold(0.0, f64::max);
        println!(
            "| {name} | 300 | {} | {ambiguous} | {special} | {b:.2e} |",
            300 - ambiguous - special
        );
        apart.push(special);
    }
    // The farther, the more of them: at 2^16 two rays in three meet a
    // triangle whose weight tolerance is over 2^-4.
    assert!(
        apart[0] < apart[1] && apart[1] < apart[2] && apart[2] >= 200,
        "{apart:?}"
    );
    assert_eq!(
        apart[4], apart[1],
        "2^14 in the vertices or by an instance: the same count"
    );
    assert_eq!(e.level1.failures(), 0);
}

// ---- the rule sees what it should ----

#[test]
fn planted_defects_fail_under_the_rule() {
    let case = generate(Class::StaticSoup, DEFAULT_SEED);
    let (scene, hits) = frames(&case).remove(0);
    let count = |hits: &[Hit]| rule::evaluate(&scene, &case.rays, hits, &C).unwrap();
    let base = count(&hits);
    assert_eq!(base.level1.failures(), 0);
    // The weights the wrong way round: out of tolerance, at both levels.
    let swapped: Vec<Hit> = hits
        .iter()
        .map(|h| Hit {
            u: h.v,
            v: h.u,
            ..*h
        })
        .collect();
    let e = count(&swapped);
    assert!(e.level1.out_of_tol > 1000 && e.level1.failures() == e.level1.out_of_tol);
    assert!(e.level2.differ > 1000);
    // The faces the wrong way round.
    let turned: Vec<Hit> = hits
        .iter()
        .map(|h| Hit {
            kind: [0, 2, 1][h.kind as usize],
            ..*h
        })
        .collect();
    let e = count(&turned);
    assert!(e.level1.facing_flips > 1000 && e.level2.differ == e.level1.facing_flips);
    // Without a face, as the hybrid's records are: nothing to flip.
    let faceless: Vec<Hit> = hits
        .iter()
        .map(|h| Hit {
            kind: if h.is_miss() { 0 } else { hit_kind::PROCEDURAL },
            ..*h
        })
        .collect();
    let e = count(&faceless);
    assert_eq!((e.level1.failures(), e.level2.differ), (0, 0));
    assert!(e.level1.procedural > 1000);
    // t a part in a thousand too long; every hit a miss; every miss a hit.
    let long: Vec<Hit> = hits
        .iter()
        .map(|h| Hit {
            t: h.t * 1.001,
            ..*h
        })
        .collect();
    assert!(count(&long).level1.out_of_tol > 1000);
    let e = count(&vec![Hit::MISS; hits.len()]);
    assert!(e.level1.lost_hits > 1000 && e.level1.failures() == e.level1.lost_hits);
    let some = *hits.iter().find(|h| !h.is_miss()).unwrap();
    let e = count(&vec![some; hits.len()]);
    assert!(e.level1.gained_hits > 300 && e.level1.surface_flips + e.level1.id_flips > 1000);
    // Refusals: counted by reason, not failures, and past one ray in 16
    // the class validates nothing.
    let mut refused = hits.clone();
    for (k, h) in refused.iter_mut().enumerate().take(100) {
        *h = Hit::refused(if k % 4 == 0 {
            refusal::GEOMETRY
        } else {
            refusal::RANGE
        });
    }
    let e = count(&refused);
    assert_eq!(
        (e.level1.refused, e.level1.refused_by_reason),
        (100, [25, 75])
    );
    assert_eq!(
        (e.level1.failures(), e.level2.not_judged, e.level2.differ),
        (0, 100, 0)
    );
    let v = |e: &Evaluation| rule::verdict(ClassKind::Random, &e.level1, &e.level2, &C);
    assert_ne!(v(&e), RuleVerdict::Fail);
    for h in refused.iter_mut().take(200) {
        *h = Hit::refused(refusal::RANGE);
    }
    assert_eq!(v(&count(&refused)), RuleVerdict::ValidatesNothing);
    // A filter that decides otherwise, away from any threshold.
    let case = generate(Class::AlphaTested, DEFAULT_SEED);
    let scene = case.scene.clone();
    let image = SwScene::build(&scene, BuildOptions::for_rays(&case.rays)).unwrap();
    let opaque: Vec<Ray> = case.rays.iter().map(|r| Ray { flags: 1, ..*r }).collect();
    let e = rule::evaluate(&scene, &case.rays, &trace_all(&image, &opaque).unwrap(), &C).unwrap();
    assert!(e.level1.filter_flips > 200, "{:?}", e.level1);
}

/// A path of its own, for the test below: every triangle of every
/// instance tested in `f64` in the instance's object space, by edge
/// functions that give one number for an edge from either side. Each
/// coordinate of each vertex is moved by up to `moved` tolerances (`moved`
/// times `K` 2^-23 times the vertex's largest coordinate), by one amount
/// for one vertex wherever the same numbers appear under the same geometry
/// transform, so that edges the scene shares stay shared.
fn moved_path(scene: &Scene, ray: &Ray, moved: f64) -> Hit {
    type V = [f64; 3];
    let sub = |a: V, b: V| -> V { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] };
    let dot = |a: V, b: V| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: V, b: V| -> V {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    if ray.flags & ray_flag::SKIP_TRIANGLES != 0 {
        return Hit::MISS;
    }
    let first = ray.flags & ray_flag::ACCEPT_FIRST_HIT_AND_END_SEARCH != 0;
    let (ow, dw) = (ray.origin.map(f64::from), ray.dir.map(f64::from));
    let mut best: Option<(f64, Hit)> = None;
    for (i, inst) in scene.instances.iter().enumerate() {
        if u32::from(inst.mask) & ray.mask & 0xFF == 0 {
            continue;
        }
        let w = inst.transform.inverse().unwrap();
        let row = |r: usize, v: V| w[4 * r] * v[0] + w[4 * r + 1] * v[1] + w[4 * r + 2] * v[2];
        let o: V = std::array::from_fn(|r| row(r, ow) + w[4 * r + 3]);
        let d: V = std::array::from_fn(|r| row(r, dw));
        // The edge function of p -> q, the same number negated for q -> p.
        let edge = |p: V, q: V| {
            if p.partial_cmp(&q) == Some(std::cmp::Ordering::Less) {
                dot(d, cross(sub(p, o), sub(q, o)))
            } else {
                -dot(d, cross(sub(q, o), sub(p, o)))
            }
        };
        for (g, geo) in scene.bottoms[inst.blas as usize]
            .geometries
            .iter()
            .enumerate()
        {
            let mut opaque = geo.opaque;
            if inst.flags & instance_flag::FORCE_OPAQUE != 0 {
                opaque = true;
            }
            if inst.flags & instance_flag::FORCE_NON_OPAQUE != 0 {
                opaque = false;
            }
            if ray.flags & ray_flag::FORCE_OPAQUE != 0 {
                opaque = true;
            }
            if ray.flags & ray_flag::FORCE_NON_OPAQUE != 0 {
                opaque = false;
            }
            let cull = if opaque {
                ray_flag::CULL_OPAQUE
            } else {
                ray_flag::CULL_NON_OPAQUE
            };
            if ray.flags & cull != 0 {
                continue;
            }
            let salt = geo.transform.map_or(0, |t| {
                t.0.iter()
                    .fold(1, |h, x| victory_rt::shade::mix(h ^ x.to_bits()))
            });
            for k in 0..geo.primitive_count() {
                let p: [V; 3] = geo.triangle(k).map(|raw| {
                    let v = raw.map(f64::from);
                    let v: V = match geo.transform {
                        Some(m) => {
                            let m = m.0.map(f64::from);
                            std::array::from_fn(|r| {
                                m[4 * r] * v[0]
                                    + m[4 * r + 1] * v[1]
                                    + m[4 * r + 2] * v[2]
                                    + m[4 * r + 3]
                            })
                        }
                        None => v,
                    };
                    let h = raw.iter().fold(salt, |h, x| {
                        victory_rt::shade::mix(h ^ if *x == 0.0 { 0 } else { x.to_bits() })
                    });
                    let step =
                        moved * K * rule::SPACING * v[0].abs().max(v[1].abs()).max(v[2].abs());
                    std::array::from_fn(|a| {
                        let r = victory_rt::shade::mix(h ^ (a as u32 + 1));
                        v[a] + (f64::from(r) / 2_147_483_648.0 - 1.0) * step
                    })
                });
                let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                let s = dot(d, n);
                let e = [edge(p[1], p[2]), edge(p[2], p[0]), edge(p[0], p[1])];
                let sum = e[0] + e[1] + e[2];
                if s == 0.0
                    || sum == 0.0
                    || (e.iter().any(|x| *x < 0.0) && e.iter().any(|x| *x > 0.0))
                {
                    continue;
                }
                let t = dot(sub(p[0], o), n) / s;
                if !(t > f64::from(ray.tmin) && t < f64::from(ray.tmax)) {
                    continue;
                }
                let mut front = s < 0.0;
                if inst.flags & instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE != 0 {
                    front = !front;
                }
                let facing = if front {
                    ray_flag::CULL_FRONT_FACING_TRIANGLES
                } else {
                    ray_flag::CULL_BACK_FACING_TRIANGLES
                };
                if inst.flags & instance_flag::TRIANGLE_CULL_DISABLE == 0 && ray.flags & facing != 0
                {
                    continue;
                }
                let (u, v) = ((e[1] / sum) as f32, (e[2] / sum) as f32);
                if !opaque && !victory_rt::alpha_accepts(u, v, k as u32) {
                    continue;
                }
                if best.as_ref().is_none_or(|(nearest, _)| t < *nearest) {
                    let hit = Hit {
                        t: t as f32,
                        u,
                        v,
                        kind: if front {
                            hit_kind::FRONT
                        } else {
                            hit_kind::BACK
                        },
                        instance_index: i as u32,
                        instance_id: inst.id,
                        geometry_index: g as u32,
                        primitive_index: k as u32,
                    };
                    if first {
                        return hit;
                    }
                    best = Some((t, hit));
                }
            }
        }
    }
    best.map_or(Hit::MISS, |(_, hit)| hit)
}

#[test]
fn a_path_inside_the_position_tolerance_is_accepted() {
    // Another implementation than the software model, written apart from
    // the rule's reference, with its vertices exact and moved by an eighth
    // and a quarter of the position tolerance: no record of it is one the
    // rule does not accept, in any class and frame. (A triangle thinner
    // than the tolerance turns its face under such a move; the rule
    // compares no face there.)
    println!(
        "| class | rays | vertices exact: agree | admissible | moved by 1/8 of the tolerance: agree | admissible | by 1/4: agree | admissible | not accepted |\n|---|---|---|---|---|---|---|---|---|"
    );
    let mut turned = 0;
    for class in Class::ALL {
        let case = generate(class, DEFAULT_SEED);
        let mut row = format!(
            "| {} | {} |",
            class.name(),
            case.rays.len() * case.frame_count()
        );
        let mut failures = 0;
        for moved in [0.0, 0.125, 0.25] {
            let (mut agree, mut admissible) = (0, 0);
            for scene in case.scenes() {
                let hits: Vec<Hit> = case
                    .rays
                    .iter()
                    .map(|ray| moved_path(&scene, ray, moved))
                    .collect();
                let e = rule::evaluate(&scene, &case.rays, &hits, &C).unwrap();
                let text =
                    rule::to_text(class.name(), RuleVerdict::Fail, &e.level1, &e.level2, &[]);
                assert_eq!(e.level1.failures(), 0, "moved by {moved}: {text}");
                assert_eq!(e.level2.differ, 0, "moved by {moved}: {text}");
                failures += e.level1.failures() + e.level2.differ;
                agree += e.level1.agree;
                admissible += e.level1.admissible;
                // The faces this path reports the other way round, on
                // pairs whose face the rule does not compare.
                for ((r, j), hit) in e.rules.iter().zip(&e.judged).zip(&hits) {
                    let other = r.admissible.iter().any(|a| {
                        a.hit.ids() == hit.ids() && a.hit.kind != hit.kind && !a.facing_known
                    });
                    turned += u32::from(!j.outcome.is_failure() && !hit.is_miss() && other);
                }
            }
            row += &format!(" {agree} | {admissible} |");
        }
        println!("{row} {failures} |");
    }
    println!("faces reported the other way on a pair with no stable face: {turned}");
    // The move does turn faces: the test would not see the rule compare
    // them otherwise.
    assert!(turned >= 5, "{turned}");
}

// ---- level 3 ----

/// The reference's picture and the software model's of one camera, and
/// which rays the reference flagged.
fn pictures(
    scene: &Scene,
    camera: &picture::Camera,
    side: usize,
) -> (
    picture::Picture,
    picture::Picture,
    Vec<bool>,
    Vec<Ray>,
    Vec<Hit>,
) {
    let rays = camera.rays(side);
    let rules = Rule::new(scene, C).trace_all(&rays);
    let primaries: Vec<Hit> = rules.iter().map(|r| r.primary).collect();
    let image = SwScene::build(scene, BuildOptions::for_rays(&rays)).unwrap();
    let hits = trace_all(&image, &rays).unwrap();
    (
        picture::draw(scene, &rays, &primaries, side, true),
        picture::draw(scene, &rays, &hits, side, true),
        rules.iter().map(|r| r.flagged()).collect(),
        rays,
        hits,
    )
}

#[test]
fn the_software_model_s_pictures_are_the_reference_s() {
    // The classes dxr-validation.md draws, at 128 pixels a side here (the
    // rule's pictures are 1,024; the function is the same).
    const SIDE: usize = 128;
    println!(
        "| class | view | pixels | hit | flagged | differ | not flagged | groups | largest | verdict |\n|---|---|---|---|---|---|---|---|---|---|"
    );
    for class in [
        Class::Closed,
        Class::Instancing,
        Class::Far,
        Class::Seam,
        Class::Scale,
        Class::AlphaTested,
        Class::Transformed,
    ] {
        let case = generate(class, DEFAULT_SEED);
        for (v, camera) in picture::cameras(&case).iter().enumerate() {
            let (reference, model, flagged, _, hits) = pictures(&case.scene, camera, SIDE);
            let (r, sensitive) = picture::assess(&reference, &model, &flagged, &C).unwrap();
            let hit = hits.iter().filter(|h| !h.is_miss()).count();
            println!(
                "| {} | {v} | {} | {hit} | {} | {} | {} | {} | {} | {}{} |",
                class.name(),
                r.pixels,
                flagged.iter().filter(|f| **f).count(),
                r.differ,
                r.unflagged,
                r.groups,
                r.group_max,
                if r.pass { "pass" } else { "fail" },
                if sensitive.is_empty() {
                    String::new()
                } else {
                    format!("; sensitive ({})", sensitive.join(", "))
                },
            );
            // A picture shows something, and the model's is the
            // reference's: no pixel differs, and the picture passes.
            assert!(hit >= 100, "{} view {v}: {hit} pixels hit", class.name());
            assert_eq!(
                (r.differ, r.unflagged, r.pass),
                (0, 0, true),
                "{} view {v}",
                class.name()
            );
        }
    }
}

#[test]
fn a_seam_one_pixel_wide_fails_the_picture() {
    // One of the seam class's cubes, the one whose halves are two bottom
    // levels under one transform, seen close.
    const SIDE: usize = 96;
    let case = generate(Class::Seam, DEFAULT_SEED);
    let camera = picture::Camera::around([-24.0, -8.0, 4.0], 2.0);
    let (reference, model, flagged, rays, hits) = pictures(&case.scene, &camera, SIDE);
    let whole = picture::compare(&reference, &model, &flagged, &C).unwrap();
    assert!(whole.pass, "{whole:?}");
    // A path that loses the hit within a hair of the seam plane x = -24
    // and shows what is behind: the cube's far wall, or nothing.
    let mut reference_tracer = Reference::new(&case.scene);
    let mut leaked = hits.clone();
    let mut holes = 0;
    for (k, ray) in rays.iter().enumerate() {
        let h = hits[k];
        if h.is_miss() {
            continue;
        }
        let x = ray.origin[0] + ray.dir[0] * h.t;
        if (x + 24.0).abs() < 0.03 && h.instance_index < 2 {
            let mut later = *ray;
            later.tmin = h.t * 1.01;
            leaked[k] = reference_tracer.trace(&later).primary;
            holes += 1;
        }
    }
    let leak = picture::draw(&case.scene, &rays, &leaked, SIDE, true);
    let r = picture::compare(&reference, &leak, &flagged, &C).unwrap();
    println!(
        "a planted seam: {holes} rays let through, {} pixels differ in {} groups, the largest of {}, {} of them on rays the reference had not flagged",
        r.differ, r.groups, r.group_max, r.unflagged
    );
    assert!(holes >= 20 && r.differ >= 20 && r.group_max > 2 && !r.pass);
    // Every differing pixel is traced back to its ray, and here those rays
    // are not ones the reference flagged: the tolerance is far narrower
    // than a hair.
    assert!(
        r.unflagged > 0
            && r.differing
                .iter()
                .all(|d| leaked[d.pixel as usize] != hits[d.pixel as usize])
    );
}

// ---- the comparison by the error model, on the new classes ----

#[test]
fn the_rule_s_tolerance_beside_the_error_model_s() {
    // The two tolerances on the same hits: the rule's, from one measured
    // constant and the geometry of the pair, over the error model's, a
    // bound of modelled arithmetic with a term for an instance's inverse.
    // Neither bounds the other: without an instance transform the rule's
    // is the wider, behind a rotated or scaled instance the model's is.
    println!(
        "| class | hits | weights: rule over model, least | median | largest | t: least | median | largest |\n|---|---|---|---|---|---|---|---|"
    );
    let mut medians = Vec::new();
    for class in Class::FIRST {
        let case = generate(class, DEFAULT_SEED);
        let diags = Reference::new(&case.scene).trace_all(&case.rays);
        let rules = Rule::new(&case.scene, C).trace_all(&case.rays);
        let (mut b, mut t) = (Vec::new(), Vec::new());
        for (d, r) in diags.iter().zip(&rules) {
            let (Some(p), Some(a)) = (d.permitted.first(), r.admissible.first()) else {
                continue;
            };
            if r.ambiguous
                || p.wide
                || d.primary.is_miss()
                || p.hit != d.primary
                || a.hit != d.primary
            {
                continue;
            }
            b.push(a.b_tol / p.tol_uv);
            t.push(a.t_tol / p.tol_t);
        }
        let stats = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            (v[0], v[v.len() / 2], v[v.len() - 1])
        };
        let ((b0, b1, b2), (t0, t1, t2)) = (stats(&mut b), stats(&mut t));
        println!(
            "| {} | {} | {b0:.2} | {b1:.2} | {b2:.1} | {t0:.2} | {t1:.2} | {t2:.1} |",
            class.name(),
            b.len()
        );
        medians.push(b1);
    }
    // The soup has no instance transform; the transformed class has one on
    // every hit but those of its first instance.
    assert!(medians[0] > 2.0 && medians[6] < 0.5, "{medians:?}");
}

#[test]
fn the_older_comparison_finds_no_defect_on_the_rule_s_classes() {
    println!(
        "| class | rays | clear | ambiguous | excluded | wide | match | permitted, as reference | permitted, other | by ids only | defects | verdict |\n|---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for class in &Class::ALL[10..] {
        let case = generate(*class, DEFAULT_SEED);
        let (scene, hits) = frames(&case).remove(0);
        let diags = Reference::new(&scene).trace_all(&case.rays);
        let primaries: Vec<Hit> = diags.iter().map(|d| d.primary).collect();
        let r = compare(class.name(), &diags, &primaries, &hits).unwrap();
        let t = &r.candidate;
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:?} |",
            class.name(),
            r.rays,
            r.clear,
            r.ambiguous,
            r.excluded,
            r.wide,
            t.matched,
            t.permitted_primary,
            t.permitted_other,
            t.ids_only,
            t.defects(),
            r.verdict
        );
        assert_eq!(t.defects() + r.baseline.defects(), 0, "{}", r.to_text());
        assert_ne!(r.verdict, Verdict::Fail);
    }
}
