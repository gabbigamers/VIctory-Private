//! The reference against the CPU model of the software path, by the older
//! comparison (the reference's own error model), on the ten first scene
//! classes, on this host; `tests/rule.rs` has all 26 under the proposed
//! rule. Nothing here touches a GPU: what these tests show is
//! that the two CPU implementations, written apart and working differently
//! (exact arithmetic over every triangle in world space; `f32` node tests
//! over a BVH in object space), agree under the comparison policy, and that
//! the policy sees a defect when one is planted.
//!
//! `cargo test -p victory-rt --test agreement -- --nocapture` prints the
//! tables of docs/dxr-validation-reference.md.

use victory_rt::compare::{BAND, Outcome, Report, Tally, Verdict, compare, judge};
use victory_rt::console::{BuildOptions, SLOT, SwScene};
use victory_rt::generate::{Case, Class, DEFAULT_SEED, generate};
use victory_rt::record::{Hit, Ray, hit_kind, instance_flag, ray_flag};
use victory_rt::reference::{Diag, Reference, Stats, Status, flag};
use victory_rt::results::{ResultFile, Section};
use victory_rt::rng::Rng;
use victory_rt::scene::{
    Action, BottomLevel, BuildFlags, Geometry, Indices, Instance, Scene, Transform,
};
use victory_rt::traverse::{Boxes, trace, trace_all};

/// One frame of a class: the reference's answers and the software path's.
struct Frame {
    scene: Scene,
    image: SwScene,
    diags: Vec<Diag>,
    hits: Vec<Hit>,
    stats: Stats,
}

/// Every frame of a class, the image brought forward as each frame asks:
/// refits where the structures allow, builds where they do not.
fn frames(case: &Case, options: BuildOptions) -> Vec<Frame> {
    let mut scene = case.scene.clone();
    let mut image = SwScene::build(&scene, options).unwrap();
    let mut out = Vec::new();
    for f in 0..case.frame_count() {
        if f > 0 {
            let actions = case.frames[f - 1].apply(&mut scene).unwrap();
            image.update(&scene, &actions).unwrap();
        }
        let mut reference = Reference::new(&scene);
        let diags = reference.trace_all(&case.rays);
        let hits = trace_all(&image, &case.rays).unwrap();
        out.push(Frame {
            scene: scene.clone(),
            image: image.clone(),
            diags,
            hits,
            stats: reference.stats,
        });
    }
    out
}

fn primaries(diags: &[Diag]) -> Vec<Hit> {
    diags.iter().map(|d| d.primary).collect()
}

/// The reference's own answers as the baseline, the software path as the
/// candidate.
fn report(class: Class, frame: &Frame) -> Report {
    compare(
        class.name(),
        &frame.diags,
        &primaries(&frame.diags),
        &frame.hits,
    )
    .unwrap()
}

/// Why a ray on which the software path gives a permitted result other
/// than the reference's own is not clear: a first-hit query; an exact tie,
/// edge, vertex or range end among its candidates; or neither, and then
/// rounding alone.
#[derive(Default)]
struct Others {
    first_hit: u32,
    exact: u32,
    rounding: u32,
}

const EXACT: u32 = flag::EDGE_EXACT | flag::TIE_EXACT | flag::TMIN_EXACT | flag::TMAX_EXACT;
const NEAR: u32 = flag::EDGE_NEAR | flag::TIE_NEAR | flag::RANGE_NEAR | flag::ALPHA_NEAR;

#[test]
fn the_software_path_agrees_with_the_reference_on_every_class() {
    let mut counts = String::from(
        "| class | frames | rays | clear | ambiguous | excluded | wide | match | permitted, as the reference | permitted, another | of the three, by ids only | defects | beyond the band |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let mut sizes = String::from(
        "| class | hits | tolerance uv: median | largest | tolerance t, of its scale: median | largest | software path's worst error: uv | t, of its scale | worst error over tolerance: t | uv |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let mut reasons = String::new();
    let mut total = 0;
    for class in Class::FIRST {
        let case = generate(class, DEFAULT_SEED);
        let all = frames(&case, BuildOptions::for_rays(&case.rays));
        let mut others = Others::default();
        for (f, frame) in all.iter().enumerate() {
            let r = report(class, frame);
            let at = format!("{} frame {f}:\n{}", class.name(), r.to_text());
            // The reference passes its own policy, and the software path
            // has no defect: every disagreement is a permitted one.
            assert_eq!(r.baseline.defects(), 0, "{at}");
            assert_eq!(r.baseline.permitted_other, 0, "{at}");
            assert_eq!(r.candidate.defects(), 0, "{at}");
            assert!(
                matches!(r.verdict, Verdict::Pass | Verdict::Fallback),
                "{at}"
            );
            // The software path's errors are inside the band, with room:
            // that is what makes the band a statement about it.
            assert_eq!(r.candidate.beyond_band, 0, "{at}");
            let worst = r.candidate.worst_t_ratio.max(r.candidate.worst_uv_ratio);
            assert!(worst < BAND / 2.0, "{at}");
            // The reference's own records are the exact numbers rounded to
            // `f32`, and that rounding is well inside the band too.
            assert_eq!(r.baseline.beyond_band, 0, "{at}");
            let rounding = r.baseline.worst_t_ratio.max(r.baseline.worst_uv_ratio);
            assert!(rounding < BAND / 2.0, "{at}");
            assert_eq!(frame.stats.inconsistent, 0, "{at}");
            assert!(
                frame
                    .diags
                    .iter()
                    .all(|d| d.flags & flag::INCONSISTENT == 0),
                "{at}"
            );
            assert!(frame.stats.exact * 20 < frame.stats.pairs, "{at}");
            for (diag, hit) in frame.diags.iter().zip(&frame.hits) {
                // A zero of the reference is the number zero: no sign.
                for p in &diag.permitted {
                    assert!(p.hit.to_words()[..3].iter().all(|w| *w != 0x8000_0000));
                }
                if judge(diag, hit).outcome != Outcome::PermittedOther {
                    continue;
                }
                if diag.flags & flag::FIRST_HIT != 0 {
                    others.first_hit += 1;
                } else if diag.flags & EXACT != 0 {
                    others.exact += 1;
                } else {
                    // Nothing exact about the ray: a candidate within
                    // rounding of an edge, of another, of the range's end
                    // or of a cell's.
                    assert_ne!(diag.flags & NEAR, 0, "{at}");
                    others.rounding += 1;
                }
            }
        }
        // The whole class as one comparison, for the tables.
        let diags: Vec<Diag> = all.iter().flat_map(|f| f.diags.clone()).collect();
        let hits: Vec<Hit> = all.iter().flat_map(|f| f.hits.clone()).collect();
        let r = compare(class.name(), &diags, &primaries(&diags), &hits).unwrap();
        let (t, s) = (&r.candidate, &r.tolerance);
        assert_eq!(
            t.permitted_other,
            others.first_hit + others.exact + others.rounding
        );
        // Divergence from the reference's own pick is expected where the
        // class is built of exact ties, and nowhere else.
        let ties = matches!(class, Class::Overlapping | Class::Precision);
        assert_eq!(
            r.divergent > 0,
            ties,
            "{} divergent {}",
            class.name(),
            r.divergent
        );
        // First-hit rays with a choice are in one class only.
        assert_eq!(
            others.first_hit > 0,
            class == Class::MasksAndFlags,
            "{}",
            class.name()
        );
        total += r.rays;
        counts += &format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            class.name(),
            all.len(),
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
            t.beyond_band,
        );
        sizes += &format!(
            "| {} | {} | {:.1e} | {:.1e} | {:.1e} | {:.1e} | {:.1e} | {:.1e} | {:.3} | {:.3} |\n",
            class.name(),
            s.hits,
            s.uv_median,
            s.uv_max,
            s.t_median,
            s.t_max,
            t.worst_uv,
            t.worst_t,
            t.worst_t_ratio,
            t.worst_uv_ratio,
        );
        if t.permitted_other > 0 {
            reasons += &format!(
                "{}: {} permitted results other than the reference's: {} on rays with an exact tie, edge, vertex or range end, {} within rounding of one, {} on first-hit rays\n",
                class.name(),
                t.permitted_other,
                others.exact,
                others.rounding,
                others.first_hit
            );
        }
    }
    println!("{counts}\n{sizes}\n{reasons}{total} rays in all");
}

#[test]
fn other_seeds_agree_inside_the_band_too() {
    // The table is of one seed. Three more, every class and frame: no
    // defect, nothing beyond the band, no class without meaning.
    for seed in 1..=3 {
        for class in Class::FIRST {
            let case = generate(class, DEFAULT_SEED + seed);
            for (f, frame) in frames(&case, BuildOptions::for_rays(&case.rays))
                .iter()
                .enumerate()
            {
                let r = report(class, frame);
                let at = format!("seed + {seed} frame {f}:\n{}", r.to_text());
                assert_eq!(r.candidate.defects() + r.baseline.defects(), 0, "{at}");
                assert_eq!(r.candidate.beyond_band, 0, "{at}");
                assert!(
                    matches!(r.verdict, Verdict::Pass | Verdict::Fallback),
                    "{at}"
                );
                assert_eq!(frame.stats.inconsistent, 0, "{at}");
            }
        }
    }
}

// ---- a third implementation, as plain as possible ----

/// The closest accepted hit by the textbook test (Moller and Trumbore) in
/// `f64` and world space, with the flags, the mask and the alpha test.
/// Good only away from every boundary, which is where it is used: on rays
/// the reference calls clear.
fn plain(scene: &Scene, ray: &Ray) -> Hit {
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let place = |m: &[f32; 12], p: [f64; 3]| -> [f64; 3] {
        let m = m.map(f64::from);
        std::array::from_fn(|r| {
            m[4 * r] * p[0] + m[4 * r + 1] * p[1] + m[4 * r + 2] * p[2] + m[4 * r + 3]
        })
    };
    let (o, d) = (ray.origin.map(f64::from), ray.dir.map(f64::from));
    let (mut best, mut best_t) = (Hit::MISS, f64::from(ray.tmax));
    if ray.flags & ray_flag::SKIP_TRIANGLES != 0 {
        return best;
    }
    for (i, inst) in scene.instances.iter().enumerate() {
        if u32::from(inst.mask) & ray.mask & 0xFF == 0 {
            continue;
        }
        let mirrored = inst.transform.det() < 0.0;
        for (g, geo) in scene.bottoms[inst.blas as usize]
            .geometries
            .iter()
            .enumerate()
        {
            for k in 0..geo.primitive_count() {
                let v = geo.triangle(k).map(|v| {
                    let v = v.map(f64::from);
                    let v = geo.transform.map_or(v, |t| place(&t.0, v));
                    place(&inst.transform.0, v)
                });
                let (e1, e2) = (sub(v[1], v[0]), sub(v[2], v[0]));
                let p = cross(d, e2);
                let det = dot(e1, p);
                if det == 0.0 {
                    continue;
                }
                let s = sub(o, v[0]);
                let u = dot(s, p) / det;
                let q = cross(s, e1);
                let w = dot(d, q) / det;
                let t = dot(e2, q) / det;
                if u < 0.0 || w < 0.0 || u + w > 1.0 || t <= f64::from(ray.tmin) || t >= best_t {
                    continue;
                }
                // Facing in object space: the world normal against the ray,
                // turned back if the instance mirrors.
                let mut front = (dot(cross(e1, e2), d) < 0.0) != mirrored;
                if inst.flags & instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE != 0 {
                    front = !front;
                }
                let cull = ray.flags
                    & if front {
                        ray_flag::CULL_FRONT_FACING_TRIANGLES
                    } else {
                        ray_flag::CULL_BACK_FACING_TRIANGLES
                    };
                if cull != 0 && inst.flags & instance_flag::TRIANGLE_CULL_DISABLE == 0 {
                    continue;
                }
                let opaque = match (ray.flags & 3, inst.flags >> 2 & 3) {
                    (1, _) => true,
                    (2, _) => false,
                    (_, 1) => true,
                    (_, 2) => false,
                    _ => geo.opaque,
                };
                let culled = if opaque {
                    ray_flag::CULL_OPAQUE
                } else {
                    ray_flag::CULL_NON_OPAQUE
                };
                if ray.flags & culled != 0 {
                    continue;
                }
                let cell = (u * 8.0).floor() as i64 + (w * 8.0).floor() as i64 + k as i64;
                if !opaque && cell & 1 != 0 {
                    continue;
                }
                best_t = t;
                best = Hit {
                    t: t as f32,
                    u: u as f32,
                    v: w as f32,
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
            }
        }
    }
    best
}

#[test]
fn a_plain_f64_tracer_gives_the_reference_s_answer_on_clear_rays() {
    for class in Class::FIRST {
        let case = generate(class, DEFAULT_SEED);
        for (f, scene) in case.scenes().iter().enumerate() {
            let diags = Reference::new(scene).trace_all(&case.rays);
            let mut clear = 0;
            for (k, (ray, diag)) in case.rays.iter().zip(&diags).enumerate() {
                // A first-hit ray has no one answer for a plain closest-hit
                // tracer to give.
                if diag.status != Status::Clear || diag.flags & flag::FIRST_HIT != 0 {
                    continue;
                }
                clear += 1;
                let (got, want) = (plain(scene, ray), diag.primary);
                let at = format!("{} frame {f} ray {k}: {got:?} {want:?}", class.name());
                assert_eq!((got.kind, got.ids()), (want.kind, want.ids()), "{at}");
                // Both are f64 results rounded to f32: a step apart at most,
                // but for where the plain test loses digits to the far
                // coordinates.
                let close = |a: f32, b: f32, scale: f32| (a - b).abs() <= 1e-4 * scale;
                assert!(
                    close(got.t, want.t, want.t)
                        && close(got.u, want.u, 1.0)
                        && close(got.v, want.v, 1.0),
                    "{at}"
                );
            }
            assert!(
                clear * 3 > case.rays.len(),
                "{} frame {f}: {clear} clear rays",
                class.name()
            );
        }
    }
}

#[test]
fn what_the_model_rules_out_is_not_a_hit_when_evaluated_exactly() {
    // Every pair of the first rays of each class goes through the exact
    // arithmetic, the ones the error model ruled out included. A pair ruled
    // out and yet inside would be counted, and the answers must not change.
    for class in Class::FIRST {
        let case = generate(class, DEFAULT_SEED);
        let rays = &case.rays[..300];
        let mut quick = Reference::new(&case.scene);
        let mut thorough = Reference::new(&case.scene);
        thorough.paranoid = true;
        assert_eq!(
            quick.trace_all(rays),
            thorough.trace_all(rays),
            "{}",
            class.name()
        );
        assert_eq!(thorough.stats.inconsistent, 0, "{}", class.name());
        assert_eq!(thorough.stats.exact, thorough.stats.pairs);
        assert!(quick.stats.exact < thorough.stats.exact / 20);
    }
}

// ---- the policy sees what it should ----

fn defects(class: Class, diags: &[Diag], hits: &[Hit]) -> Tally {
    compare(class.name(), diags, &primaries(diags), hits)
        .unwrap()
        .candidate
}

#[test]
fn planted_defects_are_caught() {
    // The weights the wrong way round.
    let case = generate(Class::StaticSoup, DEFAULT_SEED);
    let frame = &frames(&case, BuildOptions::for_rays(&case.rays))[0];
    let swapped: Vec<Hit> = frame
        .hits
        .iter()
        .map(|h| Hit {
            u: h.v,
            v: h.u,
            ..*h
        })
        .collect();
    let t = defects(Class::StaticSoup, &frame.diags, &swapped);
    assert!(t.numeric > 1000 && t.defects() == t.numeric, "{t:?}");
    // The faces the wrong way round.
    let turned: Vec<Hit> = frame
        .hits
        .iter()
        .map(|h| Hit {
            kind: [0, 2, 1][h.kind as usize],
            ..*h
        })
        .collect();
    let t = defects(Class::StaticSoup, &frame.diags, &turned);
    assert!(t.wrong_face > 1000 && t.defects() == t.wrong_face, "{t:?}");

    // Instance flags dropped from the image: the counter-clockwise
    // instances show the wrong face and cull the wrong way.
    let case = generate(Class::Transformed, DEFAULT_SEED);
    let frame = &frames(&case, BuildOptions::for_rays(&case.rays))[0];
    let mut image = frame.image.clone();
    for slot in (0..image.top_words / SLOT).skip(2) {
        let at = image.top_base + slot * SLOT;
        if image.words[at + 28] == 5 && image.words[at + 13] != 0 {
            image.words[at + 13] = 0;
        }
    }
    let t = defects(
        Class::Transformed,
        &frame.diags,
        &trace_all(&image, &case.rays).unwrap(),
    );
    assert!(
        t.wrong_face > 50 && t.missed + t.wrong_triangle > 0,
        "{t:?}"
    );

    // Instance masks ignored.
    let case = generate(Class::MasksAndFlags, DEFAULT_SEED);
    let frame = &frames(&case, BuildOptions::for_rays(&case.rays))[0];
    let open: Vec<Ray> = case.rays.iter().map(|r| Ray { mask: 0xFF, ..*r }).collect();
    let mut image = frame.image.clone();
    for slot in (0..image.top_words / SLOT).skip(2) {
        let at = image.top_base + slot * SLOT;
        if image.words[at + 28] == 5 {
            image.words[at + 12] |= 0xFF00_0000;
        }
    }
    let t = defects(
        Class::MasksAndFlags,
        &frame.diags,
        &trace_all(&image, &open).unwrap(),
    );
    assert!(t.phantom > 100 && t.wrong_triangle > 10, "{t:?}");

    // The alpha test left out.
    let case = generate(Class::HitOrdering, DEFAULT_SEED);
    let frame = &frames(&case, BuildOptions::for_rays(&case.rays))[0];
    let forced: Vec<Ray> = case
        .rays
        .iter()
        .map(|r| Ray {
            flags: ray_flag::FORCE_OPAQUE,
            ..*r
        })
        .collect();
    let t = defects(
        Class::HitOrdering,
        &frame.diags,
        &trace_all(&frame.image, &forced).unwrap(),
    );
    assert!(t.wrong_triangle + t.phantom > 300, "{t:?}");

    // A structure that was not updated: frame 0's image against frame 2's
    // scene, for the refit class and the rebuild class.
    for class in [Class::DynamicRefit, Class::DynamicRebuild] {
        let case = generate(class, DEFAULT_SEED);
        let all = frames(&case, BuildOptions::for_rays(&case.rays));
        let t = defects(class, &all[2].diags, &all[0].hits);
        assert!(t.defects() > 100, "{} {t:?}", class.name());
    }
}

/// The share of `n` in `of`, as a percentage.
fn percent(n: usize, of: usize) -> f64 {
    100.0 * n as f64 / of.max(1) as f64
}

#[test]
fn what_size_of_error_the_comparison_sees() {
    // The software path's hits on clear rays, each number moved by a known
    // amount: how often the move is a defect, and how often it is at least
    // seen (a defect, or counted as beyond the band). The table says what
    // a pass means in each class; wide results are left out, their numbers
    // are not compared at all.
    type Shift = (&'static str, fn(&Hit, f64) -> Hit);
    let shifts: [Shift; 9] = [
        ("u + 1e-5", |h, _| Hit {
            u: h.u + 1e-5,
            ..*h
        }),
        ("u + 1e-4", |h, _| Hit {
            u: h.u + 1e-4,
            ..*h
        }),
        ("u + 1e-3", |h, _| Hit {
            u: h.u + 1e-3,
            ..*h
        }),
        ("u + 1/64", |h, _| Hit {
            u: h.u + 1.0 / 64.0,
            ..*h
        }),
        ("u + 1/8", |h, _| Hit {
            u: h.u + 0.125,
            ..*h
        }),
        ("t + 1e-5 of its scale", |h, s| Hit {
            t: h.t + (1e-5 * s) as f32,
            ..*h
        }),
        ("t + 1e-4", |h, s| Hit {
            t: h.t + (1e-4 * s) as f32,
            ..*h
        }),
        ("t + 1/64", |h, s| Hit {
            t: h.t + (s / 64.0) as f32,
            ..*h
        }),
        ("t + 1/8", |h, s| Hit {
            t: h.t + (s / 8.0) as f32,
            ..*h
        }),
    ];
    let names: Vec<&str> = shifts.iter().map(|s| s.0).collect();
    println!(
        "| class | hits | {} |\n|---|---|{}",
        names.join(" | "),
        "---|".repeat(shifts.len())
    );
    for class in Class::FIRST {
        let case = generate(class, DEFAULT_SEED);
        let frame = &frames(&case, BuildOptions::for_rays(&case.rays))[0];
        let mut hits = 0;
        let mut defect = [0usize; 9];
        let mut seen = [0usize; 9];
        for (diag, hit) in frame.diags.iter().zip(&frame.hits) {
            if diag.status != Status::Clear || hit.is_miss() || diag.permitted[0].wide {
                continue;
            }
            hits += 1;
            for (k, (_, shift)) in shifts.iter().enumerate() {
                let j = judge(diag, &shift(hit, diag.permitted[0].t_scale));
                assert!(matches!(j.outcome, Outcome::Match | Outcome::Numeric));
                defect[k] += usize::from(j.outcome == Outcome::Numeric);
                seen[k] += usize::from(j.outcome == Outcome::Numeric || j.beyond_band());
            }
        }
        assert!(hits >= 300, "{} {hits}", class.name());
        let cells: Vec<String> = (0..shifts.len())
            .map(|k| {
                format!(
                    "{:.1} % ({:.1} %)",
                    percent(defect[k], hits),
                    percent(seen[k], hits)
                )
            })
            .collect();
        println!("| {} | {hits} | {} |", class.name(), cells.join(" | "));
        // What holds by construction, in every class: no compared result
        // has a tolerance past a sixteenth, so an error of an eighth is a
        // defect, and one of a sixty-fourth is past the band.
        for k in [4, 8] {
            assert_eq!(defect[k], hits, "{} {}", class.name(), names[k]);
        }
        for k in [3, 7] {
            assert_eq!(seen[k], hits, "{} {}", class.name(), names[k]);
        }
        // Seeing is never less than calling a defect, and a larger move is
        // never seen less often.
        for k in 0..shifts.len() {
            assert!(seen[k] >= defect[k]);
        }
        for k in [1, 2, 3, 4, 6, 7, 8] {
            assert!(seen[k] >= seen[k - 1] && defect[k] >= defect[k - 1]);
        }
        // Where no instance transform is in the way the comparison is
        // sharp: a ten-thousandth in u is a defect on nine hits in ten.
        // Behind rotated, scaled and far instances it is not, and the band
        // is what sees a thousandth there.
        match class {
            Class::StaticSoup | Class::HitOrdering => {
                assert!(defect[1] * 10 >= hits * 9, "{}", class.name());
            }
            Class::Transformed | Class::Instancing => {
                assert!(defect[1] * 2 < hits, "{}", class.name());
                assert!(seen[2] * 10 >= hits * 8, "{}", class.name());
            }
            _ => {}
        }
    }
}

/// Instances of one bottom level, each scaled by powers of two up to
/// 2^`scale` either way along each axis, turned, and moved by up to 64
/// times 2^`shift`; and rays aimed into their triangles from a few triangle
/// sizes away.
fn instanced(bottom: &BottomLevel, scale: i32, shift: i32, seed: u64) -> (Scene, Vec<Ray>) {
    let mut r = Rng::new(seed);
    let pow2 = |k: i32| f32::from_bits(((k + 127) as u32) << 23);
    let instances: Vec<Instance> = (0..6)
        .map(|i| {
            let s: [f32; 3] = std::array::from_fn(|_| pow2(r.range(-scale, scale)));
            let turn = Transform::rotation(r.below(3) as usize, r.dyadic(-8, 8, 3));
            let to: [f32; 3] = std::array::from_fn(|_| r.dyadic(-64, 64, 0) * pow2(shift));
            Instance {
                transform: Transform::translation(to)
                    .after(&turn)
                    .after(&Transform::scale(s)),
                id: i,
                mask: 0xFF,
                flags: 0,
                blas: 0,
            }
        })
        .collect();
    let scene = Scene {
        bottoms: vec![bottom.clone()],
        instances,
        flags: BuildFlags::default(),
    };
    scene.validate().unwrap();
    let mut rays = Vec::new();
    while rays.len() < 1200 {
        let inst = r.pick(&scene.instances).transform;
        let geo: &Geometry = r.pick(&bottom.geometries);
        let t = geo
            .triangle(r.below(geo.primitive_count() as u32) as usize)
            .map(|v| inst.apply(v));
        let (a, b) = (r.unit() * 0.5, r.unit() * 0.5);
        let target: [f32; 3] =
            std::array::from_fn(|k| t[0][k] + (t[1][k] - t[0][k]) * a + (t[2][k] - t[0][k]) * b);
        let size = (0..3).fold(0f32, |m, k| {
            m.max((t[1][k] - t[0][k]).abs())
                .max((t[2][k] - t[0][k]).abs())
        });
        let origin: [f32; 3] =
            std::array::from_fn(|k| target[k] - r.dyadic(-16, 16, 2) * size * pow2(r.range(0, 3)));
        let dir: [f32; 3] = std::array::from_fn(|k| target[k] - origin[k]);
        let ray = Ray::new(origin, dir, 0.0, 1000.0);
        if ray.validate().is_ok() {
            rays.push(ray);
        }
    }
    (scene, rays)
}

#[test]
fn strongly_scaled_and_far_instances_are_useless_not_passed() {
    // The closed octahedron and the soup of the instancing class, a few
    // units across, with indices as the class has them.
    let bottom = generate(Class::Instancing, DEFAULT_SEED).scene.bottoms[0].clone();
    assert!(matches!(bottom.geometries[0].indices, Indices::U16(_)));
    println!("| scales up to | moved by up to | clear | ambiguous | excluded | wide | verdict |");
    println!("|---|---|---|---|---|---|---|");
    let mut verdicts = Vec::new();
    for (scale, shift) in [(1, 0), (6, 6), (10, 12)] {
        let (scene, rays) = instanced(&bottom, scale, shift, 0x1257 + scale as u64);
        let diags = Reference::new(&scene).trace_all(&rays);
        let image = SwScene::build(&scene, BuildOptions::for_rays(&rays)).unwrap();
        let hits = trace_all(&image, &rays).unwrap();
        let r = compare("instanced", &diags, &primaries(&diags), &hits).unwrap();
        // Whatever is still compared is still right: the tolerance gives
        // out before the software path does.
        assert_eq!(r.candidate.defects(), 0, "{}", r.to_text());
        assert!(diags.iter().all(|d| d.flags & flag::INCONSISTENT == 0));
        println!(
            "| 2^{scale} | 2^{} | {} | {} | {} | {} | {:?} |",
            shift + 6,
            r.clear,
            r.ambiguous,
            r.excluded,
            r.wide,
            r.verdict
        );
        verdicts.push((r.verdict, r.excluded + r.wide));
    }
    // Mild transforms are judged. With scales of 2^6 either way and
    // positions in the thousands, and beyond, the model's bound for an
    // inverse computed in `f32` (the condition number times the distance
    // from the world's origin) leaves hardly a ray with a list of results
    // or a tolerance worth comparing by, and the class says so itself
    // instead of passing on ids alone.
    assert_eq!(verdicts[0].0, Verdict::Pass);
    assert_eq!(verdicts[1].0, Verdict::Useless);
    assert_eq!(verdicts[2].0, Verdict::Useless);
    assert!(verdicts[0].1 < 60 && verdicts[1].1 > 1000 && verdicts[2].1 > 1000);
}

#[test]
fn a_refit_and_a_build_of_the_same_frame_give_permitted_results() {
    let case = generate(Class::DynamicRefit, DEFAULT_SEED);
    let refitted = frames(&case, BuildOptions::for_rays(&case.rays));
    let mut scene = case.scene.clone();
    for (f, frame) in refitted.iter().enumerate().skip(1) {
        // The frames of this class are refits throughout.
        let actions = case.frames[f - 1].apply(&mut scene).unwrap();
        assert!(!actions.bottoms.contains(&Action::Rebuild) && actions.top == Action::Refit);
        assert!(actions.bottoms.contains(&Action::Refit));
        let built = SwScene::build(&frame.scene, BuildOptions::for_rays(&case.rays)).unwrap();
        // The trees differ (the refit keeps frame 0's), the results agree
        // under the policy, with the refit as the baseline.
        assert_ne!(built.words, frame.image.words);
        let hits = trace_all(&built, &case.rays).unwrap();
        let r = compare("dynamic-refit", &frame.diags, &frame.hits, &hits).unwrap();
        assert_eq!(r.verdict, Verdict::Pass, "{}", r.to_text());
    }
}

/// One image of a class against the reference: the defects of the tested
/// traversal, how many of them remain when every box is visited, and the
/// triangle tests of both.
fn boxes(case: &Case, diags: &[Diag], options: BuildOptions) -> (Vec<usize>, usize, u64, u64) {
    let image = SwScene::build(&case.scene, options).unwrap();
    let (mut bad, mut still, mut tests, mut every) = (Vec::new(), 0, 0u64, 0u64);
    for (k, (ray, diag)) in case.rays.iter().zip(diags).enumerate() {
        let (hit, work) = trace(&image, ray, Boxes::Tested).unwrap();
        let (all, all_work) = trace(&image, ray, Boxes::AllVisited).unwrap();
        tests += u64::from(work.triangle_tests);
        every += u64::from(all_work.triangle_tests);
        assert!(work.deepest_stack <= image.stack && all_work.deepest_stack <= image.stack);
        // With every box visited the traversal is right whatever the boxes.
        assert!(!judge(diag, &all).outcome.is_defect(), "ray {k}");
        if judge(diag, &hit).outcome.is_defect() {
            bad.push(k);
            still += usize::from(judge(diag, &all).outcome.is_defect());
        } else if hit != all {
            assert_ne!(diag.status, Status::Clear, "ray {k}");
        }
    }
    (bad, still, tests, every)
}

#[test]
fn tight_boxes_leak_through_shared_edges_and_padded_ones_do_not() {
    println!(
        "| class | tight boxes: defects | triangle tests | padded boxes: defects | triangle tests | with every box visited |"
    );
    println!("|---|---|---|---|---|---|");
    for class in Class::FIRST {
        let case = generate(class, DEFAULT_SEED);
        let diags = Reference::new(&case.scene).trace_all(&case.rays);
        let tight = boxes(&case, &diags, BuildOptions::tight());
        let padded = boxes(&case, &diags, BuildOptions::for_rays(&case.rays));
        assert!(padded.0.is_empty(), "{} {:?}", class.name(), padded.0);
        println!(
            "| {} | {} | {} | {} | {} | {} |",
            class.name(),
            tight.0.len(),
            tight.2,
            padded.0.len(),
            padded.2,
            padded.3
        );
    }
    // At the documented seed tight boxes happen to cost nothing. These are
    // seeds of the precision class at which they do: a ray within rounding
    // of an edge two triangles share, which the triangle test gives to the
    // triangle whose box the box test has turned it away from. The ray
    // comes out on the far side of the mesh or misses it. The reference
    // demands one of the two triangles; visiting every box finds it;
    // padding finds it.
    let mut leaks = 0;
    for seed in LEAKY {
        let case = generate(Class::Precision, seed);
        let diags = Reference::new(&case.scene).trace_all(&case.rays);
        let (bad, still, ..) = boxes(&case, &diags, BuildOptions::tight());
        assert!(
            !bad.is_empty() && still == 0,
            "seed {seed}: {bad:?} {still}"
        );
        for &k in &bad {
            let d = &diags[k];
            assert!(
                d.flags & flag::WATERTIGHT != 0 && !d.miss_permitted,
                "seed {seed} ray {k}"
            );
            // All of them at the mesh whose vertices are near 2^20, where
            // the grid of representable positions is coarse enough for a
            // random ray to land on an edge.
            assert!(case.rays[k].origin[0] > 1.0e6, "seed {seed} ray {k}");
        }
        leaks += bad.len();
        let (bad, ..) = boxes(&case, &diags, BuildOptions::for_rays(&case.rays));
        assert!(bad.is_empty(), "seed {seed}: {bad:?}");
    }
    println!(
        "tight boxes: {leaks} leaks over {} seeds of the precision class; padded: none",
        LEAKY.len()
    );
}

#[test]
fn the_node_test_can_hit_a_triangle_of_no_area() {
    // D3D12: "Degenerate triangles ... do not generate any intersections."
    // The software path's node test misses one when its determinant comes
    // out exactly zero, which rounding does not always grant: a ray aimed at
    // a point of a triangle that is a line can come back with a hit on it.
    // The reference never hits one, flags every ray whose line passes one
    // within `f32` error, and such rays are not compared; so the difference
    // shows here and nowhere else.
    let (mut flagged, mut hits) = (0, 0);
    for seed in 0..8 {
        let case = generate(Class::Precision, DEFAULT_SEED + seed);
        let diags = Reference::new(&case.scene).trace_all(&case.rays);
        let image = SwScene::build(&case.scene, BuildOptions::for_rays(&case.rays)).unwrap();
        for (diag, hit) in diags.iter().zip(trace_all(&image, &case.rays).unwrap()) {
            let degenerate = diag.flags & flag::DEGENERATE != 0;
            flagged += usize::from(degenerate);
            // Geometry 1 of instance 1 is the triangles of no area.
            assert!(diag.primary.is_miss() || diag.primary.ids()[..3] != [1, 2, 1]);
            if !hit.is_miss() && hit.ids()[..3] == [1, 2, 1] {
                assert!(degenerate && diag.status == Status::Excluded);
                hits += 1;
            }
        }
    }
    println!("{hits} software-path hits on triangles of no area, on {flagged} flagged rays");
    assert!(hits >= 8 && flagged > hits, "{hits} {flagged}");
}

/// Seeds at which a tight image of the precision class leaks, found by a
/// sweep of forty.
const LEAKY: [u64; 7] = [
    8 * 7919,
    12 * 7919,
    14 * 7919,
    17 * 7919,
    26 * 7919,
    36 * 7919,
    40 * 7919,
];

// ---- the classes hold what they are built for ----

fn flagged(diags: &[Diag], bit: u32) -> usize {
    diags.iter().filter(|d| d.flags & bit != 0).count()
}

#[test]
fn the_precision_class_has_its_exact_cases() {
    let case = generate(Class::Precision, DEFAULT_SEED);
    let frame = &frames(&case, BuildOptions::for_rays(&case.rays))[0];
    let d = &frame.diags;
    // Three closed meshes, each with 100 rays through edges and 70 through
    // vertices, exactly: facts of the exact arithmetic, not of a tolerance.
    assert!(
        flagged(d, flag::EDGE_EXACT) >= 3 * 170,
        "{}",
        flagged(d, flag::EDGE_EXACT)
    );
    assert!(flagged(d, flag::TIE_EXACT) >= 3 * 150);
    // 16 rays a mesh end exactly on their hit and 16 start exactly on it.
    assert_eq!(flagged(d, flag::TMAX_EXACT), 48);
    assert_eq!(flagged(d, flag::TMIN_EXACT), 48);
    assert!(flagged(d, flag::DEGENERATE) >= 20);
    assert!(flagged(d, flag::WATERTIGHT) >= 300);
    assert!(flagged(d, flag::UNSTABLE) >= 15);
    // Through an edge inside a face the reference demands a hit on one of
    // the two triangles; the software path gives one, never a hole.
    let mut edges = 0;
    for (diag, hit) in d.iter().zip(&frame.hits) {
        if diag.status == Status::Ambiguous && diag.flags & flag::WATERTIGHT != 0 {
            assert!(!diag.miss_permitted);
            assert!(matches!(
                judge(diag, hit).outcome,
                Outcome::PermittedPrimary | Outcome::PermittedOther
            ));
            edges += 1;
        }
    }
    assert!(edges >= 300, "{edges}");
    // A ray that ends exactly on its hit does not see it; one a step longer
    // does. Both the reference and the software path.
    let mut ends = 0;
    for (k, (ray, diag)) in case.rays.iter().zip(d).enumerate() {
        let on = diag.permitted.iter().find(|p| p.t == f64::from(ray.tmax));
        if let Some(p) = on {
            assert_ne!(diag.primary.ids(), p.hit.ids(), "ray {k}");
            assert!(
                frame.hits[k].is_miss() || frame.hits[k].ids() != p.hit.ids(),
                "ray {k}"
            );
            ends += 1;
        }
    }
    assert_eq!(ends, 48);
}

#[test]
fn the_other_classes_have_what_they_are_named_for() {
    let run = |class: Class| {
        let case = generate(class, DEFAULT_SEED);
        let mut all = frames(&case, BuildOptions::for_rays(&case.rays));
        (case, all.remove(0))
    };
    // Overlapping: exact ties, between primitives, geometries and instances.
    let (_, frame) = run(Class::Overlapping);
    let ties: Vec<&Diag> = frame
        .diags
        .iter()
        .filter(|d| d.flags & flag::TIE_EXACT != 0)
        .collect();
    assert!(ties.len() >= 600, "{}", ties.len());
    // By which id a permitted result differs from the reference's pick:
    // the instance index, the geometry, the primitive.
    for field in [0, 2, 3] {
        let differing = |d: &&&Diag| {
            d.permitted
                .iter()
                .any(|p| p.hit.ids()[field] != d.primary.ids()[field])
        };
        assert!(ties.iter().filter(differing).count() >= 50, "{field}");
    }
    assert!(
        ties.iter()
            .all(|d| d.next_gap == 0.0 && d.next_differs != 0 && d.permitted.len() >= 2)
    );

    // Hit ordering: hits behind rejected candidates. Layer 0 of the stack
    // is primitives 0-7 of geometry 0; a ray from below it that ends on a
    // later layer went through a rejected one.
    let (case, frame) = run(Class::HitOrdering);
    let behind = case
        .rays
        .iter()
        .zip(&frame.diags)
        .filter(|(r, d)| {
            let h = d.primary;
            r.dir[2] > 0.0
                && r.tmin == 0.0
                && !h.is_miss()
                && h.instance_index == 0
                && h.geometry_index == 0
                && h.primitive_index >= 8
        })
        .count();
    assert!(behind >= 60, "{behind}");
    assert!(
        frame
            .diags
            .iter()
            .filter(|d| d.alpha_margin.is_finite())
            .count()
            >= 1000
    );

    // Alpha: accepted and rejected both, and some rays near a cell border.
    let (_, frame) = run(Class::AlphaTested);
    let on_alpha = frame
        .diags
        .iter()
        .filter(|d| !d.primary.is_miss() && d.primary.geometry_index < 2)
        .count();
    let on_backdrop = frame
        .diags
        .iter()
        .filter(|d| d.primary.geometry_index == 2)
        .count();
    assert!(
        on_alpha >= 500 && on_backdrop >= 300,
        "{on_alpha} {on_backdrop}"
    );
    assert!(flagged(&frame.diags, flag::ALPHA_NEAR) >= 5);

    // Instancing and transforms: both faces, every instance hit, mirrored
    // instances among them.
    for class in [Class::Instancing, Class::Transformed] {
        let (case, frame) = run(class);
        let mut seen = vec![[0u32; 2]; case.scene.instances.len()];
        for d in &frame.diags {
            if !d.primary.is_miss() {
                seen[d.primary.instance_index as usize][(d.primary.kind - 1) as usize] += 1;
            }
        }
        assert!(
            seen.iter().all(|s| s[0] + s[1] > 0),
            "{} {seen:?}",
            class.name()
        );
        let mirrored = case
            .scene
            .instances
            .iter()
            .filter(|i| i.transform.det() < 0.0)
            .count();
        assert!(mirrored >= 2, "{}", class.name());
        assert!(
            seen.iter().map(|s| s[1]).sum::<u32>() >= 20,
            "{} {seen:?}",
            class.name()
        );
    }

    // Masks and flags: every ray flag in use, first-hit rays with a choice.
    let (case, frame) = run(Class::MasksAndFlags);
    for bit in 0..10 {
        assert!(
            case.rays.iter().filter(|r| r.flags >> bit & 1 != 0).count() >= 20,
            "flag bit {bit}"
        );
    }
    assert!(case.rays.iter().filter(|r| r.mask & 0xFF == 0).count() >= 50);
    let choice = frame
        .diags
        .iter()
        .filter(|d| d.flags & flag::FIRST_HIT != 0 && d.permitted.len() >= 2)
        .count();
    assert!(choice >= 20, "{choice}");
}

// ---- determinism and the result file ----

#[test]
fn a_run_is_the_same_twice_and_survives_the_result_file() {
    let mut file = ResultFile {
        producer: "victory-rt software traversal, CPU model, host test".into(),
        sections: Vec::new(),
    };
    let mut reports = Vec::new();
    for class in [Class::DynamicRefit, Class::Precision, Class::MasksAndFlags] {
        let case = generate(class, DEFAULT_SEED);
        let (first, second) = (
            frames(&case, BuildOptions::for_rays(&case.rays)),
            frames(&case, BuildOptions::for_rays(&case.rays)),
        );
        for (f, (a, b)) in first.iter().zip(&second).enumerate() {
            assert_eq!(a.image.words, b.image.words);
            assert_eq!(a.hits, b.hits);
            assert_eq!(a.diags, b.diags);
            file.sections
                .push(Section::of(&case, f as u32, a.hits.clone()));
            reports.push((class, f as u32, a.diags.clone(), report(class, a)));
        }
    }
    // What a later run would do: write, read back, find each section by
    // class, seed and frame, check that it was written for the case as
    // generated here, compare.
    let back = ResultFile::parse(&file.to_text().unwrap()).unwrap();
    assert_eq!(back, file);
    for (class, f, diags, want) in &reports {
        let section = back.section(class.name(), DEFAULT_SEED, *f).unwrap();
        section.check(&generate(*class, DEFAULT_SEED)).unwrap();
        let got = compare(class.name(), diags, &primaries(diags), &section.hits).unwrap();
        assert_eq!(&got, want);
    }
    // A section of one seed against the case of another is refused before
    // any hit is looked at.
    let section = back.section("precision", DEFAULT_SEED, 0).unwrap();
    assert!(
        section
            .check(&generate(Class::Precision, DEFAULT_SEED + 1))
            .is_err()
    );
}
