//! The independent reference: every triangle of every instance against
//! every ray, no acceleration structure, DXR's closest-hit rules.
//!
//! Two things are computed for each ray and kept apart.
//!
//! **The truth**, in exact arithmetic ([`crate::exact`]). The vertices go to
//! world space by the forward transforms (products and sums of the `f32`
//! inputs, so exact), and with `A, B, C` the vertices minus the origin,
//! `e1 = B - A`, `e2 = C - A`, `n = e1 x e2` and `d` the direction:
//!
//! ```text
//! E1 = A . (d x e2)    E2 = A . (e1 x d)    S = d . n    D = A . n
//! E0 = S - E1 - E2     t = D / S            u = E1 / S   v = E2 / S
//! ```
//!
//! The ray meets the triangle when `S` is not zero and no `Ek / S` is
//! negative (an edge or a vertex counts as inside; a triangle of no area
//! has `S = 0` and is never hit), and the hit counts when `tmin < t < tmax`
//! (both strict, as D3D12 has it for triangles). Signs, the two range
//! tests, the order of two hits in `t` (by cross-multiplying) and the alpha
//! cell (`floor(8u)`, `floor(8v)`) are all decided on exact numbers, so "on
//! an edge", "the same `t`" and "exactly on `tmax`" are facts and not
//! thresholds. Among accepted hits with the same `t` the reference takes
//! the lowest instance index, then geometry, then primitive: a rule of its
//! own, flagged, since D3D12 leaves the choice open.
//!
//! Facing is D3D12's: a triangle is front facing when its vertices appear
//! clockwise from the ray origin in a left-handed system, which is `d . n <
//! 0` in object space with `n = (v1 - v0) x (v2 - v0)` taken on the
//! vertices after the geometry's transform. The instance transform does
//! not enter (in world terms the sign of `S` times the sign of its
//! determinant); `TRIANGLE_FRONT_COUNTERCLOCKWISE` swaps front and back,
//! for culling and for the reported kind alike.
//!
//! **What an implementation may return instead**, from a model of `f32`
//! arithmetic (docs/dxr-validation-reference.md has the derivation). For a
//! ray and a triangle the model bounds, component by component, the error
//! of each edge function, of `S` and of `D` as an implementation working in
//! the instance's object space would have them, and from those gives
//! tolerances for `t` and for the barycentrics. A pair is ruled out when
//! the edge functions have mixed signs beyond their bounds; it is unstable
//! when `S` is within its bound of zero (a ray in the triangle's plane, a
//! sliver, a triangle of no area the ray's line passes); otherwise it is a
//! sure hit, or a doubtful one where a weight, the range test or the alpha
//! cell is within tolerance of flipping. The permitted results are the
//! candidates that can come out nearest; a ray with one permitted result is
//! clear.
//!
//! A tolerance can come out so wide that it judges nothing: on a sliver far
//! from the ray's origin the model allows the barycentrics an error of more
//! than 1. Past [`MAX_TOL_UV`] or [`MAX_TOL_T`] a permitted result is
//! **wide** ([`Permitted::wide`], [`flag::WIDE`]): which triangle and which
//! face are still facts and still compared, its `t`, `u` and `v` are not,
//! and the comparison counts such results apart.
//!
//! The model runs in `f64` on every pair, and only the pairs it does not
//! rule out go through the exact arithmetic. Its margins are `f32`
//! roundings, 2^29 times an `f64`'s, so a pair it rules out is not a hit;
//! [`Reference::paranoid`] evaluates every pair exactly and counts a
//! contradiction, and a host test runs it.
//!
//! Triangles of no area: D3D12 says they generate no intersections, and the
//! truth here never hits one. A ray whose line passes one within the
//! model's bounds is still flagged ([`flag::DEGENERATE`]) and not compared:
//! the software path's node test can return a hit there
//! (tests/agreement.rs), and the test of an implementation that transforms
//! the vertices first may no longer see the triangle as degenerate.

use std::collections::HashMap;

use crate::exact::{Dy, V3, cross3, dot3, sub3, transform3, v3};
use crate::record::{Hit, Ray, hit_kind, instance_flag, ray_flag};
use crate::scene::{Scene, Transform};

/// The unit roundoff of `f32`, 2^-24.
pub const UNIT: f64 = 1.0 / 16_777_216.0;
/// The factor the modelled error bounds are multiplied by, for formulations
/// of the test other than the modelled one.
pub const SAFETY: f64 = 4.0;
/// More permitted results than this and the ray is not compared.
pub const MAX_PERMITTED: usize = 16;
/// A barycentric tolerance above this judges nothing worth the name: a
/// result at its edge is half a cell of the alpha pattern away from the
/// exact one. The numbers of such a result are not compared.
pub const MAX_TOL_UV: f64 = 1.0 / 16.0;
/// The same for `t`, as a share of [`Permitted::t_scale`].
pub const MAX_TOL_T: f64 = 1.0 / 16.0;

/// Why a ray is not clear, or not compared in full. The `_EXACT` flags are
/// facts about the exact geometry; the `_NEAR` flags come from the error
/// model. A ray's flags are about its candidates, not about the
/// reference's own hit: [`EDGE_EXACT`](flag::EDGE_EXACT),
/// [`TMIN_EXACT`](flag::TMIN_EXACT), [`TMAX_EXACT`](flag::TMAX_EXACT), the
/// `_NEAR` flags and [`WIDE`](flag::WIDE) say that one of the permitted
/// candidates has the property, which may be one behind the primary. What
/// is true of the primary alone is in [`Diag::edge_margin`],
/// [`Diag::tmin_margin`], [`Diag::tmax_margin`] and [`Diag::next_gap`].
pub mod flag {
    /// A permitted candidate is met exactly on one of its edges or
    /// vertices.
    pub const EDGE_EXACT: u32 = 1 << 0;
    /// Another accepted hit has exactly the primary's `t`.
    pub const TIE_EXACT: u32 = 1 << 1;
    /// A permitted candidate's `t` is exactly `tmin` (exactly, it does not
    /// count).
    pub const TMIN_EXACT: u32 = 1 << 2;
    /// A permitted candidate's `t` is exactly `tmax` (exactly, it does not
    /// count).
    pub const TMAX_EXACT: u32 = 1 << 3;
    /// A triangle of no area lies on the ray's line as far as `f32` can
    /// tell.
    pub const DEGENERATE: u32 = 1 << 4;
    /// A permitted candidate has a weight within tolerance of zero.
    pub const EDGE_NEAR: u32 = 1 << 5;
    /// Two permitted candidates are within their tolerances of each other
    /// in `t`: which is nearer is not decided.
    pub const TIE_NEAR: u32 = 1 << 6;
    /// A permitted candidate's `t` is within tolerance of `tmin` or `tmax`.
    pub const RANGE_NEAR: u32 = 1 << 7;
    /// A permitted candidate's `8u` or `8v` is within tolerance of a cell
    /// boundary; 0 is one (a weight that an implementation rounds below
    /// zero is in cell -1).
    pub const ALPHA_NEAR: u32 = 1 << 8;
    /// The ray ends at the first accepted hit: any accepted hit is right.
    pub const FIRST_HIT: u32 = 1 << 9;
    /// A candidate is too ill-conditioned for a tolerance; not compared.
    pub const UNSTABLE: u32 = 1 << 10;
    /// More than [`super::MAX_PERMITTED`] results; not compared.
    pub const OVERFLOW: u32 = 1 << 11;
    /// Doubtful candidates that share an edge or a vertex cover the ray
    /// between them: one of them must be hit (no crack).
    pub const WATERTIGHT: u32 = 1 << 12;
    /// The model and the exact result contradict each other; not compared.
    /// Never seen; a test fails on it.
    pub const INCONSISTENT: u32 = 1 << 13;
    /// A permitted candidate's tolerance is past [`super::MAX_TOL_UV`] or
    /// [`super::MAX_TOL_T`]: its ids and face are compared, its numbers
    /// are not.
    pub const WIDE: u32 = 1 << 14;

    pub const NAMES: [(u32, &str); 15] = [
        (EDGE_EXACT, "edge-exact"),
        (TIE_EXACT, "tie-exact"),
        (TMIN_EXACT, "tmin-exact"),
        (TMAX_EXACT, "tmax-exact"),
        (DEGENERATE, "degenerate"),
        (EDGE_NEAR, "edge-near"),
        (TIE_NEAR, "tie-near"),
        (RANGE_NEAR, "range-near"),
        (ALPHA_NEAR, "alpha-near"),
        (FIRST_HIT, "first-hit"),
        (UNSTABLE, "unstable"),
        (OVERFLOW, "overflow"),
        (WATERTIGHT, "watertight"),
        (INCONSISTENT, "inconsistent"),
        (WIDE, "wide"),
    ];
}

/// Bits of [`Diag::next_differs`].
pub mod differs {
    pub const INSTANCE: u32 = 1;
    pub const GEOMETRY: u32 = 2;
    pub const PRIMITIVE: u32 = 4;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// One result is right.
    Clear,
    /// Several are, listed in [`Diag::permitted`].
    Ambiguous,
    /// No list can be given; the ray is counted and not compared.
    Excluded,
}

/// A result an implementation may return, and how far its numbers may be
/// from the exact ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Permitted {
    /// The exact result rounded to `f32`.
    pub hit: Hit,
    pub t: f64,
    pub u: f64,
    pub v: f64,
    pub tol_t: f64,
    pub tol_uv: f64,
    /// What an error in `t` is measured against: the distance from the
    /// ray's origin to the triangle's farthest vertex, in lengths of the
    /// direction (object space). Not below `|t|`, and not zero when `t` is.
    pub t_scale: f64,
    /// `tol_uv` is past [`MAX_TOL_UV`] or `tol_t` past [`MAX_TOL_T`] of
    /// `t_scale`: the numbers of a result that names this triangle are not
    /// compared.
    pub wide: bool,
}

/// The reference's answer for one ray.
#[derive(Clone, Debug, PartialEq)]
pub struct Diag {
    /// The exact closest accepted hit, rounded to `f32`; a miss if none.
    pub primary: Hit,
    pub status: Status,
    /// [`flag`] bits.
    pub flags: u32,
    /// The primary's smallest barycentric weight: 0 exactly when the hit
    /// is exactly on an edge or a vertex, and only then; small near one.
    /// Infinite on a miss.
    pub edge_margin: f64,
    /// `t` of the next accepted hit minus the primary's (0 for an exact
    /// tie); infinite if there is none.
    pub next_gap: f64,
    /// [`differs`] bits: how the next accepted hit's ids differ.
    pub next_differs: u32,
    /// The primary's `t - tmin` and `tmax - t`; infinite on a miss.
    pub tmin_margin: f64,
    pub tmax_margin: f64,
    /// The smallest distance of `8u` or `8v` from a cell boundary (an
    /// integer, 0 among them) over the alpha-tested candidates on the ray;
    /// infinite if there are none.
    pub alpha_margin: f64,
    /// The primary first (if it is a hit), then the others by `t`.
    pub permitted: Vec<Permitted>,
    pub miss_permitted: bool,
}

/// Counts over every ray traced, for the tests and the report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Ray and triangle pairs looked at.
    pub pairs: u64,
    /// Pairs the model ruled out.
    pub ruled_out: u64,
    /// Pairs evaluated exactly.
    pub exact: u64,
    /// Pairs where the model and the exact result contradict.
    pub inconsistent: u64,
}

/// Three numbers and a bound on the error of each, as the modelled `f32`
/// implementation holds them.
#[derive(Clone, Copy)]
struct Ev {
    v: [f64; 3],
    e: [f64; 3],
}

impl Ev {
    /// `self x b`: two rounded products and a rounded difference a
    /// component, on top of what the inputs' errors do.
    fn cross(&self, b: &Ev) -> Ev {
        let (mut v, mut e) = ([0.0; 3], [0.0; 3]);
        for k in 0..3 {
            let (i, j) = ((k + 1) % 3, (k + 2) % 3);
            let (p, q) = (self.v[i] * b.v[j], self.v[j] * b.v[i]);
            v[k] = p - q;
            e[k] = product_error(self.v[i], self.e[i], b.v[j], b.e[j])
                + product_error(self.v[j], self.e[j], b.v[i], b.e[i])
                + UNIT * (p.abs() + q.abs() + v[k].abs());
        }
        Ev { v, e }
    }

    /// `self . b` and its error: three rounded products and two rounded
    /// sums.
    fn dot(&self, b: &Ev) -> (f64, f64) {
        let (mut v, mut e, mut size) = (0.0, 0.0, 0.0);
        for k in 0..3 {
            let p = self.v[k] * b.v[k];
            v += p;
            size += p.abs();
            e += product_error(self.v[k], self.e[k], b.v[k], b.e[k]);
        }
        (v, e + 3.0 * UNIT * size)
    }
}

/// What errors `da` in `a` and `db` in `b` do to `a * b`.
fn product_error(a: f64, da: f64, b: f64, db: f64) -> f64 {
    da * b.abs() + a.abs() * db + da * db
}

struct Tri {
    geometry: u32,
    primitive: u32,
    /// The vertices as given: what "the same vertex" means.
    raw: [[u32; 3]; 3],
    /// Which of the bottom level's distinct geometry transforms it is
    /// under (none is one of them).
    frame: u32,
    /// Object space: after the geometry's transform.
    p: [[f64; 3]; 3],
    /// The modelled error of the first vertex, and the edges and the normal
    /// with theirs.
    dp0: [f64; 3],
    e1: Ev,
    e2: Ev,
    n: Ev,
    opaque: bool,
    /// Three vertices with the same bits: nothing can hit it.
    point: bool,
}

/// How an instance's transform enters the error model.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The identity: the ray is used as it is.
    Identity,
    /// A translation only: one rounded subtraction a component.
    Translation,
    General,
}

struct Inst {
    /// World to object, `f64`.
    w: [f64; 12],
    shape: Shape,
    kappa: f64,
    /// The sign of the determinant, exact.
    det_sign: i32,
    translation: [f64; 3],
}

/// A ray in an instance's object space, with its modelled errors.
struct RayObj {
    o: [f64; 3],
    err_o: [f64; 3],
    d: Ev,
}

struct ExactRay {
    o: V3,
    d: V3,
    tmin: Dy,
    /// `None` for an infinite `tmax`.
    tmax: Option<Dy>,
}

enum Model {
    No,
    Unstable,
    Fine {
        w: [f64; 3],
        tol_w: [f64; 3],
        tol_t: f64,
        t_scale: f64,
        /// The sign of `S` in object space.
        side: i32,
    },
}

struct Cand {
    inst: u32,
    tri: u32,
    hit: Hit,
    t: f64,
    u: f64,
    v: f64,
    s: Dy,
    d: Dy,
    /// The exact truth.
    accepted: bool,
    /// `flag` bits: the `_NEAR` doubts, or `UNSTABLE`.
    doubt: u32,
    /// `flag` bits: the `_EXACT` facts.
    exact: u32,
    /// Which weights are within tolerance of zero.
    edges: u8,
    tol_t: f64,
    tol_uv: f64,
    t_scale: f64,
    side: i32,
    edge_margin: f64,
}

impl Cand {
    fn wide(&self) -> bool {
        self.tol_uv > MAX_TOL_UV || self.tol_t > MAX_TOL_T * self.t_scale
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The error model for one ray and one triangle, in the instance's object
/// space (the derivation is in docs/dxr-validation-reference.md).
fn model(tri: &Tri, r: &RayObj) -> Model {
    if tri.point {
        return Model::No;
    }
    let a = sub(tri.p[0], r.o);
    let a = Ev {
        v: a,
        e: std::array::from_fn(|k| tri.dp0[k] + r.err_o[k] + UNIT * a[k].abs()),
    };
    let (e1, de1) = a.dot(&r.d.cross(&tri.e2));
    let (e2, de2) = a.dot(&tri.e1.cross(&r.d));
    let (s, ds) = r.d.dot(&tri.n);
    let e0 = s - e1 - e2;
    let de0 = ds + de1 + de2 + 2.0 * UNIT * (s.abs() + e1.abs() + e2.abs());
    let e = [e0, e1, e2];
    let de = [de0, de1, de2].map(|x| SAFETY * x);
    let positive = (0..3).any(|k| e[k] > de[k]);
    let negative = (0..3).any(|k| e[k] < -de[k]);
    if positive && negative {
        return Model::No;
    }
    let ds = SAFETY * ds;
    if s.abs() <= 4.0 * ds {
        return Model::Unstable;
    }
    let den = s.abs() - ds;
    let (d, dd) = a.dot(&tri.n);
    let t = d / s;
    let w = e.map(|x| x / s);
    let tol_w: [f64; 3] = std::array::from_fn(|k| (de[k] + w[k].abs() * ds) / den + UNIT);
    if (0..3).any(|k| w[k] < -tol_w[k]) {
        return Model::No;
    }
    // t from the plane, and t as a watertight test forms it: the vertices'
    // depths along the ray weighted by the barycentrics, which carries
    // their error times how far the depths spread. The depth of a vertex
    // is taken both ways, along the direction's largest axis and along the
    // direction.
    let mut tol_t = (SAFETY * dd + t.abs() * ds) / den + 2.0 * UNIT * t.abs();
    let dir = r.d.v;
    let kz = (0..3).fold(0, |m, k| if dir[k].abs() > dir[m].abs() { k } else { m });
    let mut far = 0.0f64;
    for (p, tol) in tri.p.iter().zip(&tol_w) {
        let to = sub(*p, r.o);
        let depth = [to[kz] / dir[kz], dot(to, dir) / dot(dir, dir)];
        tol_t += tol * (depth[0] - t).abs().max((depth[1] - t).abs());
        far = far.max(dot(to, to));
    }
    Model::Fine {
        w,
        tol_w,
        tol_t,
        // The hit is inside the triangle, so no farther than its farthest
        // vertex: this is at least |t|.
        t_scale: (far / dot(dir, dir)).sqrt().max(t.abs()),
        side: if s > 0.0 { 1 } else { -1 },
    }
}

/// `floor(8 e / s)` exactly, for `e / s` in [0, 1]; `approx` is `e / s`.
fn floor8(e: &Dy, s: &Dy, approx: f64) -> i64 {
    let sign = s.sign();
    let e8 = e * &Dy::from_i64(8);
    let mut k = ((approx * 8.0).floor() as i64).clamp(0, 8);
    // k <= 8e/s < k + 1, by signs; the guess is off by at most one.
    loop {
        let below = (&e8 - &(s * &Dy::from_i64(k))).sign() * sign;
        let above = (&(s * &Dy::from_i64(k + 1)) - &e8).sign() * sign;
        if below < 0 {
            k -= 1;
        } else if above <= 0 {
            k += 1;
        } else {
            return k;
        }
    }
}

/// The distance of `x` from the nearest integer, where `floor` changes. 0
/// is one of them: exactly, a weight is never negative, and the software
/// path's never is; but nothing says an implementation cannot round one on
/// an edge to just below zero, and then its cell is -1 and the pattern's
/// parity turns.
fn cell_margin(x: f64) -> f64 {
    let above = x - x.floor();
    above.min(1.0 - above)
}

/// The reference for one scene.
pub struct Reference<'a> {
    scene: &'a Scene,
    bottoms: Vec<Vec<Tri>>,
    insts: Vec<Inst>,
    /// World-space vertices, exact, by instance and triangle.
    world: HashMap<(u32, u32), [V3; 3]>,
    pub stats: Stats,
    /// Evaluate every pair exactly, the ruled-out ones too, and count a
    /// contradiction: a check of the model's "no", at a cost.
    pub paranoid: bool,
}

impl<'a> Reference<'a> {
    /// `scene` must pass [`Scene::validate`].
    pub fn new(scene: &'a Scene) -> Self {
        let bottoms = scene
            .bottoms
            .iter()
            .map(|bottom| {
                let mut tris = Vec::new();
                let mut frames: Vec<Option<[u32; 12]>> = Vec::new();
                for (g, geo) in bottom.geometries.iter().enumerate() {
                    let bits = geo.transform.map(|t| t.0.map(f32::to_bits));
                    let frame = frames.iter().position(|f| *f == bits).unwrap_or_else(|| {
                        frames.push(bits);
                        frames.len() - 1
                    });
                    for k in 0..geo.primitive_count() {
                        tris.push(tri_of(geo, g as u32, k as u32, frame as u32));
                    }
                }
                tris
            })
            .collect();
        let insts = scene
            .instances
            .iter()
            .map(|inst| {
                let m = inst.transform.0;
                let w = inst.transform.inverse().expect("a validated scene");
                let row_sum = |m: &[f64; 12]| {
                    (0..3)
                        .map(|r| m[4 * r].abs() + m[4 * r + 1].abs() + m[4 * r + 2].abs())
                        .fold(0.0, f64::max)
                };
                let rows: [V3; 3] =
                    std::array::from_fn(|r| std::array::from_fn(|c| Dy::from_f32(m[4 * r + c])));
                let det = dot3(&rows[0], &cross3(&rows[1], &rows[2]));
                let linear = Transform::translation([m[3], m[7], m[11]]);
                Inst {
                    w,
                    shape: if inst.transform.is_identity() {
                        Shape::Identity
                    } else if linear == inst.transform {
                        Shape::Translation
                    } else {
                        Shape::General
                    },
                    kappa: row_sum(&m.map(f64::from)) * row_sum(&w),
                    det_sign: det.sign(),
                    translation: [m[3], m[7], m[11]].map(f64::from),
                }
            })
            .collect();
        Reference {
            scene,
            bottoms,
            insts,
            world: HashMap::new(),
            stats: Stats::default(),
            paranoid: false,
        }
    }

    pub fn trace_all(&mut self, rays: &[Ray]) -> Vec<Diag> {
        rays.iter().map(|r| self.trace(r)).collect()
    }

    fn ray_obj(&self, i: usize, ray: &Ray) -> RayObj {
        let inst = &self.insts[i];
        let (o, d) = (ray.origin.map(f64::from), ray.dir.map(f64::from));
        let exact = |v| Ev { v, e: [0.0; 3] };
        match inst.shape {
            Shape::Identity => RayObj {
                o,
                err_o: [0.0; 3],
                d: exact(d),
            },
            Shape::Translation => {
                let o = sub(o, inst.translation);
                RayObj {
                    o,
                    err_o: o.map(|x| UNIT * x.abs()),
                    d: exact(d),
                }
            }
            Shape::General => {
                let w = &inst.w;
                let row = |r: usize, v: [f64; 3]| {
                    w[4 * r] * v[0] + w[4 * r + 1] * v[1] + w[4 * r + 2] * v[2]
                };
                let abs_row = |r: usize, v: [f64; 3]| {
                    w[4 * r].abs() * v[0] + w[4 * r + 1].abs() * v[1] + w[4 * r + 2].abs() * v[2]
                };
                let reach: [f64; 3] =
                    std::array::from_fn(|c| o[c].abs() + inst.translation[c].abs());
                let (ko, kd) = (
                    (5.0 + 4.0 * inst.kappa) * UNIT,
                    (4.0 + 4.0 * inst.kappa) * UNIT,
                );
                RayObj {
                    o: std::array::from_fn(|r| row(r, o) + w[4 * r + 3]),
                    err_o: std::array::from_fn(|r| ko * abs_row(r, reach)),
                    d: Ev {
                        v: std::array::from_fn(|r| row(r, d)),
                        e: std::array::from_fn(|r| kd * abs_row(r, d.map(f64::abs))),
                    },
                }
            }
        }
    }

    fn world_tri(&mut self, i: usize, k: usize) -> &[V3; 3] {
        let (scene, bottoms) = (self.scene, &self.bottoms);
        self.world.entry((i as u32, k as u32)).or_insert_with(|| {
            let inst = &scene.instances[i];
            let tri = &bottoms[inst.blas as usize][k];
            let geo = &scene.bottoms[inst.blas as usize].geometries[tri.geometry as usize];
            tri.raw.map(|bits| {
                let mut p = v3(bits.map(f32::from_bits));
                if let Some(g) = &geo.transform {
                    p = transform3(&g.0, &p);
                }
                transform3(&inst.transform.0, &p)
            })
        })
    }

    /// The reference's answer for `ray`, which must pass [`Ray::validate`].
    pub fn trace(&mut self, ray: &Ray) -> Diag {
        let first_hit = ray.flags & ray_flag::ACCEPT_FIRST_HIT_AND_END_SEARCH != 0;
        let mut flags = if first_hit { flag::FIRST_HIT } else { 0 };
        let mut cands: Vec<Cand> = Vec::new();
        let mut objs: Vec<Option<RayObj>> = Vec::new();
        let mut alpha_margin = f64::INFINITY;
        let exact_ray = ExactRay {
            o: v3(ray.origin),
            d: v3(ray.dir),
            tmin: Dy::from_f32(ray.tmin),
            tmax: ray.tmax.is_finite().then(|| Dy::from_f32(ray.tmax)),
        };
        let skip = ray.flags & ray_flag::SKIP_TRIANGLES != 0;
        for i in 0..self.insts.len() {
            let inst = self.scene.instances[i];
            objs.push(None);
            if skip || u32::from(inst.mask) & ray.mask & 0xFF == 0 {
                continue;
            }
            let robj = self.ray_obj(i, ray);
            for k in 0..self.bottoms[inst.blas as usize].len() {
                self.stats.pairs += 1;
                let m = model(&self.bottoms[inst.blas as usize][k], &robj);
                if matches!(m, Model::No) {
                    self.stats.ruled_out += 1;
                    if !self.paranoid {
                        continue;
                    }
                }
                if let Some(c) = self.pair(i, k, ray, &exact_ray, &m, &mut flags, &mut alpha_margin)
                {
                    cands.push(c);
                }
            }
            objs[i] = Some(robj);
        }
        self.assemble(ray, cands, &objs, flags, alpha_margin)
    }

    /// One pair past the model's first cut: the exact truth and the model's
    /// view of it. `None` when it cannot be a result.
    #[allow(clippy::too_many_arguments)]
    fn pair(
        &mut self,
        i: usize,
        k: usize,
        ray: &Ray,
        er: &ExactRay,
        m: &Model,
        flags: &mut u32,
        alpha_margin: &mut f64,
    ) -> Option<Cand> {
        self.stats.exact += 1;
        let p = self.world_tri(i, k);
        let a = sub3(&p[0], &er.o);
        let (e1, e2) = (sub3(&p[1], &p[0]), sub3(&p[2], &p[0]));
        let n = cross3(&e1, &e2);
        let ex1 = dot3(&a, &cross3(&er.d, &e2));
        let ex2 = dot3(&a, &cross3(&e1, &er.d));
        let s = dot3(&er.d, &n);
        let d = dot3(&a, &n);
        let ex0 = &(&s - &ex1) - &ex2;
        let degenerate = n.iter().all(Dy::is_zero);
        let sign = s.sign();
        let weights = [ex0.sign() * sign, ex1.sign() * sign, ex2.sign() * sign];
        let inside = sign != 0 && weights.iter().all(|&w| w >= 0);

        let inst = self.scene.instances[i];
        let tri = &self.bottoms[inst.blas as usize][k];
        // Opacity: the geometry's flag, then the instance's, then the ray's.
        // A pair the ray's opacity cull drops is dropped by everyone,
        // whatever its geometry.
        let mut opaque = tri.opaque;
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
        let cull_opacity = if opaque {
            ray_flag::CULL_OPAQUE
        } else {
            ray_flag::CULL_NON_OPAQUE
        };
        if ray.flags & cull_opacity != 0 {
            return None;
        }
        let inconsistent = |stats: &mut Stats, flags: &mut u32| {
            stats.inconsistent += 1;
            *flags |= flag::INCONSISTENT;
        };
        let (w, tol_w, tol_t, t_scale, side) = match *m {
            Model::No => {
                if inside {
                    inconsistent(&mut self.stats, flags);
                }
                return None;
            }
            Model::Unstable => {
                *flags |= flag::UNSTABLE;
                if degenerate {
                    *flags |= flag::DEGENERATE;
                }
                ([0.0; 3], [f64::INFINITY; 3], f64::INFINITY, 0.0, 0)
            }
            Model::Fine {
                w,
                tol_w,
                tol_t,
                t_scale,
                side,
            } => (w, tol_w, tol_t, t_scale, side),
        };
        let unstable = side == 0;
        if sign == 0 {
            // Parallel to the plane, or no area: no hit, and no `t` to give.
            if !unstable {
                inconsistent(&mut self.stats, flags);
            }
            return None;
        }
        if !unstable && side != sign * self.insts[i].det_sign {
            inconsistent(&mut self.stats, flags);
        }

        // Facing, in object space.
        let mut front = sign * self.insts[i].det_sign < 0;
        if inst.flags & instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE != 0 {
            front = !front;
        }
        if inst.flags & instance_flag::TRIANGLE_CULL_DISABLE == 0 {
            let culled = if front {
                ray_flag::CULL_FRONT_FACING_TRIANGLES
            } else {
                ray_flag::CULL_BACK_FACING_TRIANGLES
            };
            if ray.flags & culled != 0 {
                return None;
            }
        }
        // The range, exactly and by the model.
        let after_tmin = (&d - &(&er.tmin * &s)).sign() * sign;
        let before_tmax = match &er.tmax {
            Some(tmax) => (&(tmax * &s) - &d).sign() * sign,
            None => 1,
        };
        let mut exact = 0;
        if after_tmin == 0 {
            exact |= flag::TMIN_EXACT;
        }
        if before_tmax == 0 {
            exact |= flag::TMAX_EXACT;
        }
        let in_range = after_tmin > 0 && before_tmax > 0;
        let t = Dy::ratio(&d, &s);
        let (u, v) = (Dy::ratio(&ex1, &s), Dy::ratio(&ex2, &s));
        // The smallest weight, each from its own exact numerator, which
        // `1 - u - v` would not be. 0 exactly when the point is exactly on
        // an edge, and for a point inside never 0 otherwise, however far
        // down the ratio rounds.
        let least = Dy::ratio(&ex0, &s).min(u).min(v);
        let edge_margin = if inside && weights.contains(&0) {
            0.0
        } else if inside {
            least.max(f64::MIN_POSITIVE)
        } else {
            least
        };
        let (tmin, tmax) = (f64::from(ray.tmin), f64::from(ray.tmax));
        let mut doubt = if unstable { flag::UNSTABLE } else { 0 };
        if !unstable {
            if t < tmin - tol_t || t > tmax + tol_t {
                if inside && in_range {
                    inconsistent(&mut self.stats, flags);
                }
                return None;
            }
            if t - tmin <= tol_t || tmax - t <= tol_t {
                doubt |= flag::RANGE_NEAR;
            }
        }

        // The edges.
        let mut edges = 0u8;
        for j in 0..3 {
            if !unstable && w[j].abs() <= tol_w[j] {
                edges |= 1 << j;
            }
        }
        if edges != 0 {
            doubt |= flag::EDGE_NEAR;
        }
        if inside && weights.contains(&0) {
            exact |= flag::EDGE_EXACT;
        }
        let tol_uv = tol_w[1].max(tol_w[2]);

        // The alpha cell: exact for a point inside, by the model for the
        // rest.
        let mut alpha = true;
        if !opaque {
            let margin = cell_margin(8.0 * u).min(cell_margin(8.0 * v));
            let cell = if inside {
                floor8(&ex1, &s, u) + floor8(&ex2, &s, v)
            } else {
                (8.0 * u).max(0.0).floor() as i64 + (8.0 * v).max(0.0).floor() as i64
            };
            alpha = (cell + i64::from(tri.primitive)) & 1 == 0;
            if !unstable {
                *alpha_margin = alpha_margin.min(margin);
                if margin <= 8.0 * tol_uv {
                    doubt |= flag::ALPHA_NEAR;
                } else if !alpha {
                    return None;
                }
            }
        }
        let accepted = inside && in_range && alpha;
        if doubt == 0 && !accepted {
            inconsistent(&mut self.stats, flags);
        }
        if unstable && !accepted {
            // Nothing to list: the flag excludes the ray.
            return None;
        }
        let hit = Hit {
            t: t as f32,
            u: u as f32,
            v: v as f32,
            kind: if front {
                hit_kind::FRONT
            } else {
                hit_kind::BACK
            },
            instance_index: i as u32,
            instance_id: inst.id,
            geometry_index: tri.geometry,
            primitive_index: tri.primitive,
        };
        Some(Cand {
            inst: i as u32,
            tri: k as u32,
            hit,
            t,
            u,
            v,
            edge_margin,
            s,
            d,
            accepted,
            doubt,
            exact,
            edges,
            tol_t,
            tol_uv,
            t_scale,
            side,
        })
    }

    /// The primary, the bound a sure hit puts on the result, and the list
    /// of what may come out.
    fn assemble(
        &self,
        ray: &Ray,
        cands: Vec<Cand>,
        objs: &[Option<RayObj>],
        mut flags: u32,
        alpha_margin: f64,
    ) -> Diag {
        let first_hit = flags & flag::FIRST_HIT != 0;
        // `a` before `b` in exact `t`, then in ids.
        let order = |a: &Cand, b: &Cand| {
            let cross = &(&a.d * &b.s) - &(&b.d * &a.s);
            (cross.sign() * a.s.sign() * b.s.sign())
                .cmp(&0)
                .then_with(|| a.hit.ids().cmp(&b.hit.ids()))
        };
        let same_t = |a: &Cand, b: &Cand| (&(&a.d * &b.s) - &(&b.d * &a.s)).is_zero();
        let mut accepted: Vec<&Cand> = cands.iter().filter(|c| c.accepted).collect();
        accepted.sort_by(|a, b| order(a, b));
        let primary = accepted.first().copied();
        let mut diag = Diag {
            primary: primary.map_or(Hit::MISS, |c| c.hit),
            status: Status::Clear,
            flags: 0,
            edge_margin: primary.map_or(f64::INFINITY, |c| c.edge_margin.max(0.0)),
            next_gap: f64::INFINITY,
            next_differs: 0,
            tmin_margin: primary.map_or(f64::INFINITY, |c| c.t - f64::from(ray.tmin)),
            tmax_margin: primary.map_or(f64::INFINITY, |c| f64::from(ray.tmax) - c.t),
            alpha_margin,
            permitted: Vec::new(),
            miss_permitted: false,
        };
        if let (Some(p), Some(next)) = (primary, accepted.get(1)) {
            if same_t(p, next) {
                flags |= flag::TIE_EXACT;
                diag.next_gap = 0.0;
            } else {
                diag.next_gap = next.t - p.t;
            }
            let (a, b) = (p.hit.ids(), next.hit.ids());
            for (field, bit) in [
                (0, differs::INSTANCE),
                (2, differs::GEOMETRY),
                (3, differs::PRIMITIVE),
            ] {
                if a[field] != b[field] {
                    diag.next_differs |= bit;
                }
            }
        }

        // The latest `t` a result can have: a sure hit is always found, and
        // so is one member of a watertight group.
        let listed: Vec<&Cand> = cands
            .iter()
            .filter(|c| c.doubt & flag::UNSTABLE == 0)
            .collect();
        let mut bound = f64::INFINITY;
        for c in listed.iter().filter(|c| c.doubt == 0) {
            bound = bound.min(c.t + c.tol_t);
        }
        for group in self.watertight(&listed, objs) {
            flags |= flag::WATERTIGHT;
            bound = bound.min(group);
        }
        diag.miss_permitted = bound == f64::INFINITY;
        if first_hit {
            bound = f64::INFINITY;
        }
        let mut permitted: Vec<&Cand> = listed
            .iter()
            .copied()
            .filter(|c| c.t - c.tol_t <= bound)
            .collect();
        permitted.sort_by(|a, b| {
            let first = |c: &Cand| primary.is_some_and(|p| std::ptr::eq(p, c));
            first(b)
                .cmp(&first(a))
                .then_with(|| a.t.total_cmp(&b.t))
                .then_with(|| a.hit.ids().cmp(&b.hit.ids()))
        });
        if let Some(p) = primary {
            // The primary is listed unless it was unstable.
            if p.doubt & flag::UNSTABLE == 0
                && !permitted.first().is_some_and(|c| std::ptr::eq(*c, p))
            {
                flags |= flag::INCONSISTENT;
            }
        }
        for c in &permitted {
            flags |= c.doubt | c.exact;
            if c.wide() {
                flags |= flag::WIDE;
            }
        }
        let near = |a: &Cand, b: &Cand| (a.t - b.t).abs() <= a.tol_t + b.tol_t;
        if (0..permitted.len()).any(|i| permitted[i + 1..].iter().any(|b| near(permitted[i], b))) {
            flags |= flag::TIE_NEAR;
        }
        diag.permitted = permitted
            .iter()
            .map(|c| Permitted {
                hit: c.hit,
                t: c.t,
                u: c.u,
                v: c.v,
                tol_t: c.tol_t,
                tol_uv: c.tol_uv,
                t_scale: c.t_scale,
                wide: c.wide(),
            })
            .collect();
        if diag.permitted.len() > MAX_PERMITTED {
            flags |= flag::OVERFLOW;
            diag.permitted.truncate(MAX_PERMITTED);
        }
        let outcomes = diag.permitted.len() + usize::from(diag.miss_permitted);
        diag.status = if flags & (flag::UNSTABLE | flag::OVERFLOW | flag::INCONSISTENT) != 0 {
            Status::Excluded
        } else if outcomes == 1 {
            Status::Clear
        } else {
            Status::Ambiguous
        };
        diag.flags = flags;
        diag
    }

    /// Groups of doubtful candidates of which one must be hit, each as the
    /// latest `t` its hit can have. A candidate qualifies when only its
    /// edges are in doubt. Two that share an edge (the same two vertices,
    /// bit for bit, in one instance) and lie on opposite sides of it as the
    /// ray sees them cover the ray between them; so does a closed fan
    /// around a shared vertex whose triangles all face the ray the same
    /// way. A watertight test hits one of them (D3D12 requires that shared
    /// edges leave no gap, and leaves open which triangle takes a hit on
    /// one).
    fn watertight(&self, listed: &[&Cand], objs: &[Option<RayObj>]) -> Vec<f64> {
        let members: Vec<&Cand> = listed
            .iter()
            .copied()
            .filter(|c| c.doubt == flag::EDGE_NEAR)
            .collect();
        let tri = |c: &Cand| {
            let blas = self.scene.instances[c.inst as usize].blas as usize;
            &self.bottoms[blas][c.tri as usize]
        };
        // D3D12 asks for no gaps within one bottom level between geometries
        // with matching transforms: bit-equal vertices under the same
        // transform, in one instance.
        let joined = |a: &Cand, b: &Cand| {
            let (ta, tb) = (tri(a), tri(b));
            a.inst == b.inst && ta.frame == tb.frame
        };
        let mut out = Vec::new();
        for (ia, a) in members.iter().enumerate() {
            let ta = tri(a);
            let robj = objs[a.inst as usize].as_ref().expect("a traced instance");
            match a.edges.count_ones() {
                1 => {
                    let k = a.edges.trailing_zeros() as usize;
                    let ends = |t: &Tri, k: usize| {
                        let mut e = [t.raw[(k + 1) % 3], t.raw[(k + 2) % 3]];
                        e.sort();
                        e
                    };
                    // The side of the third vertex across the shared edge,
                    // as the ray sees it.
                    let side = |t: &Tri, k: usize| {
                        let (i, j) = ((k + 1) % 3, (k + 2) % 3);
                        let (i, j) = if t.raw[i] <= t.raw[j] { (i, j) } else { (j, i) };
                        dot(robj.d.v, cross(sub(t.p[j], t.p[i]), sub(t.p[k], t.p[i])))
                    };
                    for b in members.iter().skip(ia + 1) {
                        let tb = tri(b);
                        if b.edges.count_ones() != 1 || !joined(a, b) {
                            continue;
                        }
                        let kb = b.edges.trailing_zeros() as usize;
                        if ends(ta, k) == ends(tb, kb) && side(ta, k) * side(tb, kb) < 0.0 {
                            out.push((a.t + a.tol_t).max(b.t + b.tol_t));
                        }
                    }
                }
                2 => {
                    let hub = ta.raw[(!a.edges & 7).trailing_zeros() as usize];
                    let mut rim: Vec<([u32; 3], [u32; 3])> = Vec::new();
                    let mut latest = f64::NEG_INFINITY;
                    let mut same_side = true;
                    for b in members.iter().filter(|b| joined(a, b)) {
                        let tb = tri(b);
                        let Some(j) = (0..3).find(|&j| tb.raw[j] == hub && b.edges & (1 << j) == 0)
                        else {
                            continue;
                        };
                        rim.push((tb.raw[(j + 1) % 3], tb.raw[(j + 2) % 3]));
                        latest = latest.max(b.t + b.tol_t);
                        same_side &= b.side == a.side;
                    }
                    let mut from: Vec<_> = rim.iter().map(|r| r.0).collect();
                    let mut to: Vec<_> = rim.iter().map(|r| r.1).collect();
                    from.sort();
                    to.sort();
                    let distinct = from.windows(2).all(|w| w[0] != w[1]);
                    if rim.len() >= 3 && same_side && distinct && from == to {
                        out.push(latest);
                    }
                }
                _ => {}
            }
        }
        out
    }
}

fn tri_of(geo: &crate::scene::Geometry, geometry: u32, primitive: u32, frame: u32) -> Tri {
    let raw = geo.triangle(primitive as usize);
    // Each vertex with the error of the geometry's transform: three
    // rounded products and three rounded sums a component.
    let placed = raw.map(|v| {
        let v = v.map(f64::from);
        match &geo.transform {
            None => Ev { v, e: [0.0; 3] },
            Some(g) => {
                let m = g.0.map(f64::from);
                let terms = |r: usize| {
                    [
                        m[4 * r] * v[0],
                        m[4 * r + 1] * v[1],
                        m[4 * r + 2] * v[2],
                        m[4 * r + 3],
                    ]
                };
                Ev {
                    v: std::array::from_fn(|r| terms(r).iter().sum()),
                    e: std::array::from_fn(|r| {
                        4.0 * UNIT * terms(r).iter().map(|x| x.abs()).sum::<f64>()
                    }),
                }
            }
        }
    });
    let edge = |to: &Ev| {
        let v = sub(to.v, placed[0].v);
        Ev {
            v,
            e: std::array::from_fn(|k| to.e[k] + placed[0].e[k] + UNIT * v[k].abs()),
        }
    };
    let (e1, e2) = (edge(&placed[1]), edge(&placed[2]));
    let bits = raw.map(|v| v.map(f32::to_bits));
    Tri {
        geometry,
        primitive,
        raw: bits,
        frame,
        p: placed.map(|p| p.v),
        dp0: placed[0].e,
        n: e1.cross(&e2),
        e1,
        e2,
        opaque: geo.opaque,
        point: bits[0] == bits[1] && bits[1] == bits[2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{BottomLevel, BuildFlags, Geometry, Indices, Instance};

    /// A unit square in the plane z = `z`, two triangles that share the
    /// diagonal from (1, 0) to (0, 1), front faces toward -z.
    fn square(z: f32, opaque: bool) -> Geometry {
        Geometry {
            vertices: vec![[0.0, 0.0, z], [0.0, 1.0, z], [1.0, 0.0, z], [1.0, 1.0, z]],
            indices: Indices::U16(vec![0, 1, 2, 2, 1, 3]),
            transform: None,
            opaque,
        }
    }

    fn scene(geometries: Vec<Geometry>, instances: &[(Transform, u32)]) -> Scene {
        Scene {
            bottoms: vec![BottomLevel {
                geometries,
                flags: BuildFlags::default(),
            }],
            instances: instances
                .iter()
                .enumerate()
                .map(|(i, &(transform, flags))| Instance {
                    transform,
                    id: 40 + i as u32,
                    mask: 0x0F,
                    flags,
                    blas: 0,
                })
                .collect(),
            flags: BuildFlags::default(),
        }
    }

    fn one(flags: u32) -> Scene {
        scene(vec![square(2.0, true)], &[(Transform::IDENTITY, flags)])
    }

    /// A ray along +z through (x, y), meeting z = 2 at t = 3.
    fn toward(x: f32, y: f32) -> Ray {
        Ray::new([x, y, -4.0], [0.0, 0.0, 2.0], 0.0, 100.0)
    }

    fn trace(scene: &Scene, ray: &Ray) -> Diag {
        scene.validate().unwrap();
        ray.validate().unwrap();
        let mut reference = Reference::new(scene);
        let d = reference.trace(ray);
        assert_eq!(reference.stats.inconsistent, 0);
        d
    }

    #[test]
    fn a_plain_hit_is_clear_with_its_numbers_and_margins() {
        let d = trace(&one(0), &toward(0.25, 0.5));
        assert_eq!((d.status, d.flags), (Status::Clear, 0));
        let h = d.primary;
        // Vertex 1 is (0, 1) and vertex 2 is (1, 0): u = y, v = x.
        assert_eq!((h.t, h.u, h.v, h.kind), (3.0, 0.5, 0.25, hit_kind::FRONT));
        assert_eq!(h.ids(), [0, 40, 0, 0]);
        assert_eq!(
            (d.edge_margin, d.tmin_margin, d.tmax_margin),
            (0.25, 3.0, 97.0)
        );
        assert_eq!((d.next_gap, d.alpha_margin), (f64::INFINITY, f64::INFINITY));
        assert_eq!(d.permitted.len(), 1);
        assert!(!d.miss_permitted && d.permitted[0].tol_t < 1e-4 && d.permitted[0].tol_uv < 1e-4);
        // Beside the square, and with a mask that leaves the instance out.
        let d = trace(&one(0), &toward(1.25, 0.5));
        assert_eq!(
            (d.status, d.primary, d.miss_permitted),
            (Status::Clear, Hit::MISS, true)
        );
        let mut ray = toward(0.25, 0.5);
        ray.mask = 0x1F0;
        assert_eq!(trace(&one(0), &ray).primary, Hit::MISS);
    }

    #[test]
    fn facing_is_decided_in_object_space_by_the_clockwise_rule() {
        // (v1 - v0) x (v2 - v0) = (0, 1, 0) x (1, 0, 0) = -z: against a ray
        // along +z, so clockwise from its origin in a left-handed system.
        let front = toward(0.25, 0.5);
        let behind = Ray::new([0.25, 0.5, 9.0], [0.0, 0.0, -1.0], 0.0, 100.0);
        assert_eq!(trace(&one(0), &front).primary.kind, hit_kind::FRONT);
        assert_eq!(trace(&one(0), &behind).primary.kind, hit_kind::BACK);
        // The counter-clockwise flag swaps the two.
        let ccw = one(instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE);
        assert_eq!(trace(&ccw, &front).primary.kind, hit_kind::BACK);
        assert_eq!(trace(&ccw, &behind).primary.kind, hit_kind::FRONT);
        // A mirrored instance does not change the facing.
        let mirror = Transform::scale([-1.0, 1.0, 1.0]);
        let s = scene(vec![square(2.0, true)], &[(mirror, 0)]);
        let h = trace(&s, &toward(-0.25, 0.5)).primary;
        assert_eq!((h.kind, h.u, h.v), (hit_kind::FRONT, 0.5, 0.25));
        // A mirrored geometry transform does: the winding turns.
        let mut turned = square(2.0, true);
        turned.transform = Some(mirror);
        let s = scene(vec![turned], &[(Transform::IDENTITY, 0)]);
        assert_eq!(trace(&s, &toward(-0.25, 0.5)).primary.kind, hit_kind::BACK);
    }

    #[test]
    fn culls_follow_the_facing_and_the_instance_can_switch_them_off() {
        let mut ray = toward(0.25, 0.5);
        ray.flags = ray_flag::CULL_BACK_FACING_TRIANGLES;
        assert_eq!(trace(&one(0), &ray).primary.kind, hit_kind::FRONT);
        ray.flags = ray_flag::CULL_FRONT_FACING_TRIANGLES;
        let d = trace(&one(0), &ray);
        assert_eq!((d.status, d.primary), (Status::Clear, Hit::MISS));
        let off = one(instance_flag::TRIANGLE_CULL_DISABLE);
        assert_eq!(trace(&off, &ray).primary.kind, hit_kind::FRONT);
        // With counter-clockwise fronts the same triangle is culled as a
        // back face.
        let ccw = one(instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE);
        assert_eq!(trace(&ccw, &ray).primary.kind, hit_kind::BACK);
        ray.flags = ray_flag::CULL_BACK_FACING_TRIANGLES;
        assert_eq!(trace(&ccw, &ray).primary, Hit::MISS);
        ray.flags = ray_flag::SKIP_TRIANGLES;
        assert_eq!(trace(&off, &ray).primary, Hit::MISS);
    }

    #[test]
    fn the_range_is_exclusive_and_exactly_on_it_is_a_fact() {
        let s = one(0);
        let mut ray = toward(0.25, 0.5);
        ray.tmax = 3.0;
        let d = trace(&s, &ray);
        assert_eq!((d.primary, d.status), (Hit::MISS, Status::Ambiguous));
        assert_eq!(d.flags, flag::TMAX_EXACT | flag::RANGE_NEAR);
        // The hit a rounding away is permitted, and so is the miss.
        assert!(d.miss_permitted && d.permitted.len() == 1 && d.permitted[0].hit.t == 3.0);
        ray.tmax = 3.0f32.next_up();
        let d = trace(&s, &ray);
        assert_eq!((d.primary.t, d.flags), (3.0, flag::RANGE_NEAR));
        assert!(d.tmax_margin > 0.0 && d.tmax_margin < 1e-6);
        (ray.tmin, ray.tmax) = (3.0, 100.0);
        let d = trace(&s, &ray);
        assert_eq!(
            (d.primary, d.flags),
            (Hit::MISS, flag::TMIN_EXACT | flag::RANGE_NEAR)
        );
        ray.tmin = 3.0f32.next_down();
        assert_eq!(trace(&s, &ray).primary.t, 3.0);
        ray.tmin = 2.5;
        assert_eq!(trace(&s, &ray).flags, 0);
    }

    #[test]
    fn a_shared_edge_is_a_tie_one_of_whose_triangles_must_be_hit() {
        // On the diagonal the two triangles share.
        let d = trace(&one(0), &toward(0.5, 0.5));
        assert_eq!(d.status, Status::Ambiguous);
        let near = flag::EDGE_NEAR | flag::TIE_NEAR | flag::WATERTIGHT;
        assert_eq!(d.flags, flag::EDGE_EXACT | flag::TIE_EXACT | near);
        // The reference takes the lower primitive; the other is permitted,
        // a miss is not.
        assert_eq!((d.primary.primitive_index, d.edge_margin), (0, 0.0));
        assert_eq!((d.next_gap, d.next_differs), (0.0, differs::PRIMITIVE));
        let ids: Vec<u32> = d.permitted.iter().map(|p| p.hit.primitive_index).collect();
        assert_eq!((ids, d.miss_permitted), (vec![0, 1], false));
        // On the square's outer edge nothing shares it: hit or miss.
        let d = trace(&one(0), &toward(0.0, 0.5));
        assert_eq!(d.flags, flag::EDGE_EXACT | flag::EDGE_NEAR);
        assert!(d.miss_permitted && d.permitted.len() == 1 && !d.primary.is_miss());
        // The weight that is exactly zero is the number zero, with no sign,
        // though it is a zero over a negative S (the face is a front one):
        // the records are compared word for word.
        assert_eq!((d.primary.kind, d.edge_margin), (hit_kind::FRONT, 0.0));
        assert_eq!(d.primary.to_words()[1..3], [0.5f32.to_bits(), 0]);
        assert_eq!(d.permitted[0].v.to_bits(), 0);
        // A vertex: both weights.
        let d = trace(&one(0), &toward(0.0, 0.0));
        assert_eq!(d.primary.to_words()[1..3], [0, 0]);
        assert_eq!(d.edge_margin, 0.0);
        // Just inside the edge of one triangle, within what f32 can tell.
        let d = trace(&one(0), &toward(0.5, 0.5f32.next_down()));
        assert_eq!(d.flags, near);
        assert!(d.edge_margin > 0.0 && d.permitted.len() == 2 && !d.miss_permitted);
        // A fold: the second triangle hangs behind the first across the
        // shared edge, so a ray on the edge may slip past both.
        let fold = Geometry {
            vertices: vec![
                [0.0, 0.0, 2.0],
                [0.0, 1.0, 2.0],
                [1.0, 0.0, 2.0],
                [0.25, 0.25, 4.0],
            ],
            indices: Indices::U16(vec![0, 1, 2, 2, 1, 3]),
            transform: None,
            opaque: true,
        };
        let s = scene(vec![fold], &[(Transform::IDENTITY, 0)]);
        let d = trace(&s, &toward(0.5, 0.5));
        assert_eq!(d.flags & flag::WATERTIGHT, 0);
        assert!(d.miss_permitted && d.permitted.len() == 2);
    }

    #[test]
    fn equal_hits_go_to_the_lowest_ids_and_all_are_permitted() {
        // The same square twice as geometries, and the whole twice as
        // instances.
        let two = vec![square(2.0, true), square(2.0, true)];
        let s = scene(two, &[(Transform::IDENTITY, 0), (Transform::IDENTITY, 0)]);
        let d = trace(&s, &toward(0.25, 0.5));
        assert_eq!(d.flags, flag::TIE_EXACT | flag::TIE_NEAR);
        assert_eq!((d.primary.ids(), d.next_gap), ([0, 40, 0, 0], 0.0));
        assert_eq!(d.next_differs, differs::GEOMETRY);
        let ids: Vec<[u32; 4]> = d.permitted.iter().map(|p| p.hit.ids()).collect();
        assert_eq!(
            ids,
            [[0, 40, 0, 0], [0, 40, 1, 0], [1, 41, 0, 0], [1, 41, 1, 0]]
        );
        // A square a little behind is another hit, not a tie.
        let s = scene(
            vec![square(2.0, true), square(2.5, true)],
            &[(Transform::IDENTITY, 0)],
        );
        let d = trace(&s, &toward(0.25, 0.5));
        assert_eq!(
            (d.status, d.next_gap, d.next_differs),
            (Status::Clear, 0.25, differs::GEOMETRY)
        );
        // With the first-hit flag either is right.
        let mut ray = toward(0.25, 0.5);
        ray.flags = ray_flag::ACCEPT_FIRST_HIT_AND_END_SEARCH;
        let d = trace(&s, &ray);
        assert_eq!((d.status, d.flags), (Status::Ambiguous, flag::FIRST_HIT));
        assert_eq!(
            (
                d.primary.geometry_index,
                d.permitted.len(),
                d.miss_permitted
            ),
            (0, 2, false)
        );
    }

    #[test]
    fn the_alpha_cell_is_decided_exactly_and_a_boundary_is_flagged() {
        let s = scene(
            vec![square(2.0, false), square(5.0, true)],
            &[(Transform::IDENTITY, 0)],
        );
        // u = 0.3, v = 0.3: cells 2 and 2, primitive 0: accepted.
        let d = trace(&s, &toward(0.3, 0.3));
        assert_eq!((d.status, d.primary.geometry_index), (Status::Clear, 0));
        assert!(d.alpha_margin > 0.3 && d.alpha_margin < 0.5);
        // u = 0.3, v = 0.2: cells 2 and 1: rejected, the square behind is
        // the hit.
        let d = trace(&s, &toward(0.2, 0.3));
        assert_eq!(
            (d.status, d.primary.geometry_index, d.primary.t),
            (Status::Clear, 1, 4.5)
        );
        // u = 0.25 exactly: 8u is 2, the cell's own edge. Exactly, floor is
        // 2 and the candidate is accepted; within f32 it could be 1.
        let d = trace(&s, &toward(0.3, 0.25));
        assert_eq!((d.status, d.flags), (Status::Ambiguous, flag::ALPHA_NEAR));
        assert_eq!((d.primary.geometry_index, d.alpha_margin), (0, 0.0));
        let behind: Vec<u32> = d.permitted.iter().map(|p| p.hit.geometry_index).collect();
        assert_eq!(behind, [0, 1]);
        // Opacity forced by the ray, and culled by it.
        let mut ray = toward(0.2, 0.3);
        ray.flags = ray_flag::FORCE_OPAQUE;
        assert_eq!(trace(&s, &ray).primary.geometry_index, 0);
        ray.flags = ray_flag::CULL_OPAQUE;
        assert_eq!(trace(&s, &ray).primary, Hit::MISS);
        ray.flags = ray_flag::CULL_NON_OPAQUE;
        assert_eq!(trace(&s, &ray).primary.geometry_index, 1);
        // The instance forces non-opaque; the ray's flag wins over it.
        let forced = scene(
            vec![square(2.0, true)],
            &[(Transform::IDENTITY, instance_flag::FORCE_NON_OPAQUE)],
        );
        assert_eq!(trace(&forced, &toward(0.2, 0.3)).primary, Hit::MISS);
        assert_eq!(trace(&forced, &ray).primary, Hit::MISS);
        ray.flags = ray_flag::FORCE_OPAQUE;
        assert_eq!(trace(&forced, &ray).primary.t, 3.0);
    }

    /// One large opaque triangle in the plane z = `z`, front toward -z,
    /// that the rays of these tests meet well inside.
    fn backdrop(z: f32) -> Geometry {
        Geometry {
            vertices: vec![[-4.0, -4.0, z], [-4.0, 8.0, z], [8.0, -4.0, z]],
            indices: Indices::None,
            transform: None,
            opaque: true,
        }
    }

    #[test]
    fn a_weight_at_zero_is_a_boundary_of_the_alpha_pattern() {
        use crate::alpha_accepts;
        use crate::compare::{Outcome, judge};
        let s = scene(
            vec![square(2.0, false), backdrop(5.0)],
            &[(Transform::IDENTITY, 0)],
        );
        // On the edge u = 0 of primitive 0, at v = 0.2: cells 0 and 1, so
        // exactly the candidate is rejected and the backdrop is the hit.
        let d = trace(&s, &toward(0.2, 0.0));
        assert_eq!((d.primary.geometry_index, d.primary.t), (1, 4.5));
        // An implementation that rounds that weight to just below zero is
        // in cell -1 and accepts: the shared formula says so.
        assert!(!alpha_accepts(0.0, 0.2, 0) && alpha_accepts(-1e-9, 0.2, 0));
        // So the front triangle is permitted too, and the ray is not clear.
        assert_eq!(d.status, Status::Ambiguous);
        assert_eq!(d.flags & flag::ALPHA_NEAR, flag::ALPHA_NEAR);
        assert_eq!(d.alpha_margin, 0.0);
        let listed: Vec<u32> = d.permitted.iter().map(|p| p.hit.geometry_index).collect();
        assert_eq!((listed, d.miss_permitted), (vec![1, 0], false));
        let front = Hit {
            t: 3.0,
            u: -1e-9,
            v: 0.2,
            kind: hit_kind::FRONT,
            instance_index: 0,
            instance_id: 40,
            geometry_index: 0,
            primitive_index: 0,
        };
        assert_eq!(judge(&d, &front).outcome, Outcome::PermittedOther);
        assert_eq!(judge(&d, &d.primary).outcome, Outcome::PermittedPrimary);
        // The edge flag is the front candidate's: the reference's own hit
        // is inside the backdrop, and its margin says so.
        assert_eq!(d.flags & flag::EDGE_EXACT, flag::EDGE_EXACT);
        assert!(d.edge_margin > 0.1, "{}", d.edge_margin);
        // The other way round, at v = 0.3: cells 0 and 2, accepted exactly;
        // rounded below zero it is rejected and the backdrop shows.
        let d = trace(&s, &toward(0.3, 0.0));
        assert!(alpha_accepts(0.0, 0.3, 0) && !alpha_accepts(-1e-9, 0.3, 0));
        assert_eq!((d.primary.geometry_index, d.status), (0, Status::Ambiguous));
        let listed: Vec<u32> = d.permitted.iter().map(|p| p.hit.geometry_index).collect();
        assert_eq!((listed, d.miss_permitted), (vec![0, 1], false));
        // Away from every boundary nothing changes.
        let d = trace(&s, &toward(0.3, 0.3));
        assert_eq!((d.status, d.flags), (Status::Clear, 0));

        // Two triangles that share the edge from (0, 0) to (0, 1): it is
        // the edge u = 0 of the left one and v = 0 of the right one, and at
        // y = 0.3 both accept exactly (cells 0 and 2).
        let wing = |x: f32, second: bool, opaque: bool| {
            let (other, top) = ([x, 0.0, 2.0], [0.0, 1.0, 2.0]);
            Geometry {
                vertices: if second {
                    vec![[0.0, 0.0, 2.0], top, other]
                } else {
                    vec![[0.0, 0.0, 2.0], other, top]
                },
                indices: Indices::None,
                transform: None,
                opaque,
            }
        };
        let pair = |opaque: bool| {
            scene(
                vec![
                    wing(-1.0, false, opaque),
                    wing(1.0, true, opaque),
                    backdrop(5.0),
                ],
                &[(Transform::IDENTITY, 0)],
            )
        };
        // Opaque, the shared edge leaves no gap: one of the two is hit and
        // nothing behind them can be the answer.
        let d = trace(&pair(true), &toward(0.0, 0.3));
        assert_eq!(d.flags & flag::WATERTIGHT, flag::WATERTIGHT);
        let listed: Vec<u32> = d.permitted.iter().map(|p| p.hit.geometry_index).collect();
        assert_eq!((listed, d.miss_permitted), (vec![0, 1], false));
        // Alpha-tested, the one that takes the edge may see its weight
        // below zero and reject, and the other was never offered the hit:
        // the backdrop is a permitted answer.
        let d = trace(&pair(false), &toward(0.0, 0.3));
        assert_eq!(d.flags & flag::WATERTIGHT, 0);
        assert_eq!(d.primary.geometry_index, 0);
        let listed: Vec<u32> = d.permitted.iter().map(|p| p.hit.geometry_index).collect();
        assert_eq!((listed, d.miss_permitted), (vec![0, 1, 2], false));
    }

    #[test]
    fn a_tolerance_too_wide_to_judge_by_marks_the_result() {
        use crate::compare::{Outcome, judge};
        // A triangle 1/64 across, 8192 away, met at an angle: the model
        // allows its weights an error of more than a sixteenth.
        let (h, far) = (1.0 / 64.0, 8192.0);
        let small = Geometry {
            vertices: vec![[0.0, 0.0, far], [0.0, h, far], [h, 0.0, far]],
            indices: Indices::None,
            transform: None,
            opaque: true,
        };
        let s = scene(vec![small], &[(Transform::IDENTITY, 0)]);
        let (origin, target) = ([-4096.0, -2048.0, 0.0], [h / 4.0, h / 4.0, far]);
        let dir: [f32; 3] = std::array::from_fn(|k| target[k] - origin[k]);
        let d = trace(&s, &Ray::new(origin, dir, 0.0, 100.0));
        assert_eq!(d.primary.kind, hit_kind::FRONT);
        let p = d.permitted[0];
        assert!(p.tol_uv > MAX_TOL_UV && p.tol_uv < 1.0, "{}", p.tol_uv);
        assert!(p.wide && d.flags & flag::WIDE != 0);
        // The ray is still compared: by its ids and its face.
        assert_ne!(d.status, Status::Excluded);
        let mut got = d.primary;
        got.u += 0.05;
        let j = judge(&d, &got);
        assert_eq!((j.outcome.is_defect(), j.ids_only), (false, true));
        got.primitive_index = 1;
        assert_eq!(judge(&d, &got).outcome, Outcome::WrongTriangle);
        // t is measured against the distance to the triangle, which is
        // about one length of this direction.
        assert!(p.t_scale >= p.t && p.t_scale < 1.01, "{}", p.t_scale);
        // The same triangle from near by is judged in full.
        let near = Ray::new(
            [h / 4.0 - 0.25, h / 4.0 - 0.125, far - 0.5],
            [0.5, 0.25, 1.0],
            0.0,
            9.0,
        );
        let d = trace(&s, &near);
        let p = d.permitted[0];
        assert_eq!((d.status, d.flags, p.wide), (Status::Clear, 0, false));
        assert!(p.tol_uv < 1e-3, "{}", p.tol_uv);
        // A ray that starts on a triangle has t = 0 and still a scale.
        let on = Ray::new([0.25, 0.5, 2.0], [0.0, 0.0, 2.0], 0.0, 100.0);
        let d = trace(&one(0), &on);
        assert_eq!(d.flags & flag::TMIN_EXACT, flag::TMIN_EXACT);
        let p = d.permitted[0];
        assert!(p.t == 0.0 && p.t_scale > 0.25 && !p.wide, "{}", p.t_scale);
    }

    #[test]
    fn a_triangle_of_no_area_is_never_the_answer_and_marks_the_rays_that_meet_it() {
        let line = Geometry {
            vertices: vec![[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [2.0, 0.0, 2.0]],
            indices: Indices::None,
            transform: None,
            opaque: true,
        };
        let point = Geometry {
            vertices: vec![[0.25, 0.5, 1.0]; 3],
            ..line.clone()
        };
        let s = scene(
            vec![line, point, square(5.0, true)],
            &[(Transform::IDENTITY, 0)],
        );
        // Through the line: the square behind is the answer, and the ray is
        // set aside.
        let d = trace(&s, &toward(0.5, 0.0));
        assert_eq!((d.primary.geometry_index, d.status), (2, Status::Excluded));
        assert_eq!(
            d.flags & (flag::DEGENERATE | flag::UNSTABLE),
            flag::DEGENERATE | flag::UNSTABLE
        );
        // Beside the line, and through the point: nothing to mark.
        let d = trace(&s, &toward(0.25, 0.5));
        assert_eq!(
            (d.primary.geometry_index, d.status, d.flags),
            (2, Status::Clear, 0)
        );
        // A ray in the square's own plane is unstable, not degenerate.
        let flat = Ray::new([-1.0, -0.5, 5.0], [1.0, 1.0, 0.0], 0.0, 100.0);
        let d = trace(&s, &flat);
        assert_eq!((d.primary, d.status), (Hit::MISS, Status::Excluded));
        assert_eq!(
            d.flags & (flag::DEGENERATE | flag::UNSTABLE),
            flag::UNSTABLE
        );
    }

    #[test]
    fn tolerances_grow_with_distance_and_at_a_grazing_angle() {
        let s = one(0);
        let near = trace(&s, &toward(0.25, 0.5)).permitted[0];
        let far = Ray::new([-3999.75, 0.5, -3998.0], [4000.0, 0.0, 4000.0], 0.0, 100.0);
        let far = trace(&s, &far).permitted[0];
        // t counts in direction lengths: compare distances along the ray.
        assert!(far.tol_uv > 100.0 * near.tol_uv && far.tol_t * 5656.0 > 100.0 * near.tol_t * 2.0);
        let graze = Ray::new([-63.75, 0.5, 1.0], [64.0, 0.0, 1.0], 0.0, 100.0);
        let d = trace(&s, &graze);
        assert_eq!((d.primary.t, d.primary.u, d.primary.v), (1.0, 0.5, 0.25));
        assert!(d.permitted[0].tol_t * 64.0 > 10.0 * near.tol_t * 2.0);
        // A translated instance costs one rounding, a rotated one more.
        let moved = scene(
            vec![square(2.0, true)],
            &[(Transform::translation([64.0, 0.0, 0.0]), 0)],
        );
        let moved = trace(&moved, &toward(64.25, 0.5)).permitted[0];
        assert!(
            moved.tol_uv < 4.0 * near.tol_uv,
            "{} {}",
            moved.tol_uv,
            near.tol_uv
        );
        let turn = Transform::translation([64.0, 0.0, 0.0]).after(&Transform::rotation(2, 1.0));
        let turned = scene(vec![square(2.0, true)], &[(turn, 0)]);
        // A quarter turn about z: (x, y) lands at (64 - y, x).
        let d = trace(&turned, &toward(63.5, 0.25));
        assert_eq!((d.primary.u, d.primary.v), (0.5, 0.25));
        assert!(d.permitted[0].tol_uv > 10.0 * near.tol_uv);
    }
}
