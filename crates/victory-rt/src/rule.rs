//! The proposed rule for functionally equivalent rendering, level 1 (rays),
//! as docs/dxr-compatibility.md states it ("The rule"): the tolerance from
//! one measured constant, every ray and triangle classed as in, out or near,
//! every ray as unambiguous or ambiguous by kind, the answers that are
//! admissible, the bound on ambiguous rays, and the verdict recomputed with
//! each arbitrary constant halved and doubled. Levels 2 and 3 are in
//! [`crate::shade`] and [`crate::picture`]. **The rule is a proposal the
//! owner has not accepted; no verdict of this module is a hardware result.**
//!
//! **The tolerance.** For a ray and a triangle, in the instance's object
//! space (where the guest's test, the hybrid's shader and a device's
//! intersector work, and where `t` is the number it is in world space): `L`
//! the direction's length, `c` the cosine between the ray and the triangle's
//! normal as a magnitude, `h` the triangle's smallest altitude, `M` the
//! largest magnitude among the coordinates of the origin and of the
//! vertices, in world and in object space, and `t` times `L` (in either
//! space, `t` no further than `tmax`).
//!
//! ```text
//! x_tol = k * 2^-23 * M      b_tol = 2 x_tol / (h c)
//! t_tol = x_tol / (L c) + b_tol * D
//! ```
//!
//! `k` is [`K`], measured by [`measure_k`] against these tolerances as
//! they stand and pinned by a host test. `D`
//! is how far the depths of the triangle's vertices along the ray lie from
//! `t`, the farthest of the three, the depth taken both along the direction
//! and along its largest axis. The term is not in the rule as first
//! written; it is what the emulated test itself needs. That test, like any
//! watertight one, forms `t` as the mean of the vertices' depths weighted
//! by the barycentrics, so a weight off by `e` moves `t` by up to `e D`; on
//! a thin triangle met at a slant the host tests found it two tolerances
//! off in `t` with its weights within a hundredth of theirs
//! (docs/dxr-compatibility.md, "The rule", [C3]).
//!
//! **A pair** is *grazing* when `c` is below 2^-6, a *sliver pair* when
//! `b_tol` is above 2^-4, *degenerate* when the triangle has no area
//! (exactly). Any other pair is *out* when a weight is below `-b_tol` or `t`
//! is outside the extents by more than `t_tol`, *in* when every weight is
//! above `b_tol` and `t` inside the extents by more than `t_tol`, else
//! *near*. `b_tol` says nothing of a grazing, sliver or degenerate pair (it
//! is unbounded as `c` or `h` goes to zero), so those are placed by
//! distance: *out* when the ray, inside its extents, passes the triangle
//! (as a set of points) by more than `2 x_tol`, else *near*; never *in*.
//! `2 x_tol` is where `-b_tol` puts the ray of an ordinary pair. The face a
//! pair shows the ray is a fact, compared and culled by, for an ordinary
//! pair and for a sliver pair whose `b_tol` is at most 1/2
//! ([`FACE_B_TOL`]: no move of the vertices by `x_tol` turns it); a
//! thinner sliver pair and a grazing pair have none.
//!
//! **A filter** (the alpha rule of geometry not marked opaque) is evaluated
//! at the corners, edge midpoints and centre of the box `u, v` plus or minus
//! `b_tol`: all accept and the candidate stands; all reject and it is out;
//! otherwise it is near, at a *filter threshold*. A box as wide as a cell
//! of the pattern (`b_tol` of 2^-4 or more) is at a threshold by definition.
//!
//! **A ray** asks for the nearest hit, or for any hit when it carries
//! `ACCEPT_FIRST_HIT_AND_END_SEARCH` (a yes-or-no workload). A hit is
//! *certain* when a pair is in, or when near pairs *close* an edge or a
//! vertex between them (below). The answers that are admissible are a hit
//! on any in or near pair that does not lie behind a certain hit by more
//! than the tolerances, and a miss when no hit is certain. The ray is
//! unambiguous when its one admissible answer is a miss or a hit on a pair
//! that is in (for a yes-or-no workload: when a hit is certain or nothing
//! is near).
//!
//! **Edges.** The reference knows each triangle's neighbours: two triangles
//! share an edge when two of their world-space vertices are the same points
//! in exact arithmetic, whatever instances and bottom levels they belong
//! to. Two near pairs *close* a shared edge when each is near through that
//! edge alone, they lie on opposite sides of it as the ray sees them, and
//! they face the ray the same way; a fan closes a shared vertex the same
//! way. A near pair also closes an edge with a neighbour that is **out by
//! its own tolerance** (an ordinary pair no flag or mask takes away, out by
//! a weight alone: the two triangles' tolerances differ with their sizes),
//! when the ray is inside the near pair's triangle exactly: the neighbour
//! cannot be hit, the two still cover the ray, and the near one is the
//! answer. A fan with such members closes its vertex likewise. Where the
//! ray is not inside the near triangle it passes beyond that neighbour's
//! reach, and the edge is a boundary for it. What the two sides are to
//! each other decides the kind:
//!
//! * the same instance and matching geometry transforms: a **shared edge**.
//!   D3D12 promises no gap there. A miss is not admissible, nor a hit
//!   behind it.
//! * instances with the same transform, bit for bit, and no geometry
//!   transform: a **closed seam**. The guest's test sees one object-space
//!   ray and the same vertices and closes it; D3D12 promises nothing; the
//!   rule asks a device to show it. Judged as a shared edge.
//! * anything else: an **open seam**. Either triangle or a miss.
//! * no partner, a partner on the same side, one facing the other way, one
//!   that is culled, masked or filtered away, one that is itself a sliver
//!   or grazing pair, or one whose reach the ray passes beyond: a
//!   **boundary or silhouette edge**. A hit or a miss.
//!
//! The other kinds: an **extent boundary** (`t` within `t_tol` of `tmin` or
//! `tmax`, `t = 0` included), a **tie** (two in-pairs within their
//! tolerances of each other), a **filter threshold**, **degenerate**.
//!
//! **Judging a record** ([`judge`]): on an unambiguous ray hit or miss,
//! instance, primitive and facing exactly, `t` within `t_tol`, each of the
//! three weights within `b_tol`; on an ambiguous ray any admissible answer
//! under the same rules. A hit on a grazing, sliver or degenerate pair is
//! placed along the ray, between the nearest and the farthest of the
//! triangle's vertices (their depths taken both ways, widened by `x_tol /
//! (L 2^-6)`), and its weights are not compared: neither side has stable
//! numbers there. The facing of a pair that has none (above) is not
//! compared, nor that of a procedural hit, which carries none. A refused
//! record is its own outcome.
//!
//! **The verdict** ([`verdict`]) of a random class is a pass only with no
//! failure, the ambiguous rays inside the bound, the rays not held to
//! numbers (a sliver or grazing pair, or refused: together) inside their
//! share, and enough rays and unambiguous hits for that to say something.
//! [`evaluate`] gives an error, not a verdict, for a ray or a scene that
//! is not valid.

use std::collections::{BTreeMap, HashMap};

use crate::alpha_accepts;
use crate::exact::{V3, cross3, sub3};
use crate::generate::ClassKind;
use crate::node::{self, NodeRay, TRIANGLE_ID_PLAIN};
use crate::record::{Hit, Ray, hit_kind, instance_flag, ray_flag, refusal};
use crate::reference::Reference;
use crate::scene::Scene;
use crate::shade::{self, ConsumedOutcome};

/// The relative spacing of `f32`, 2^-23: the unit of the tolerance.
pub const SPACING: f64 = 1.0 / 8_388_608.0;

/// The rule's tolerance constant, **computed, not chosen**: twice the
/// largest error of the emulated triangle test's single-precision model
/// against the exact reference over the random scene classes, as a position
/// error in units of 2^-23 times `M`, rounded up to a power of two
/// ([`measure_k`]). **The sample is the documented seed and the 200 that
/// follow it.** Its largest error is 2.37 units; 9 of those 201 seeds carry
/// an error over 2 units and give 8 each alone, and the other 192, the
/// documented seed among them (1.41), give 4. `tests/rule.rs` measures the
/// seeds of [`K_SEEDS`] again and fails when they do not give this number;
/// docs/dxr-validation-reference.md has the measurement by class. It is a
/// property of those classes' triangles and rays, not a bound for every
/// shape.
pub const K: f64 = 8.0;

/// The seeds the host test measures [`K`] from, as offsets from the
/// documented seed: the documented seed itself, and of the 200 that follow
/// it the one that carries the sample's largest error (`transformed`). The
/// whole sample is measured by hand (`tests/rule.rs`,
/// `more_seeds_by_hand`, with `SWEEP=200`).
pub const K_SEEDS: [u64; 2] = [0, 105];

/// The pairs [`measure_k`] takes: `h c` at least this share of `M` (and `c`
/// at least [`Constants::graze_cos`]), as the rule states it.
pub const K_FIT_MIN_HC: f64 = 1.0 / 1024.0;

/// The face of a pair is a fact, compared and culled by, only while its
/// weight tolerance is at most this: `h c` at least `4 x_tol`. Not an
/// arbitrary constant: moving each vertex by `x_tol` changes `d . n` by at
/// most `x_tol` times `L` times the triangle's perimeter, which is at most
/// three longest edges, against `d . n = L h c` times the longest edge; so
/// the sign cannot turn while `h c` is above `3 x_tol`, which is `b_tol`
/// below 2/3, and 1/2 is the power of two under that. Past it the pair is
/// thinner than the tolerance it is judged with and has no stable face, any
/// more than a grazing pair has.
pub const FACE_B_TOL: f64 = 0.5;

/// The rule's constants. All but `k` are arbitrary: none is derived, and
/// each verdict is recomputed with each halved and doubled
/// ([`Constants::variants`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constants {
    /// The tolerance constant; its own arbitrary part is the factor 2 in
    /// it.
    pub k: f64,
    /// A pair with a cosine below this is grazing: 2^-6.
    pub graze_cos: f64,
    /// A pair with a weight tolerance above this is a sliver pair: 2^-4.
    pub sliver_b: f64,
    /// Among the rays of a random class with no sliver or grazing pair, at
    /// most one in this many may be ambiguous: 1,024.
    pub ambiguous_one_in: u32,
    /// At most one ray in this many of a random class may have a sliver or
    /// grazing pair or a refused record, the two together: 16.
    pub special_one_in: u32,
    /// At least one ray in this many of a random class must be an
    /// unambiguous ray that hits, with a record that agrees: 4. With fewer
    /// the class validates nothing.
    pub hit_one_in: u32,
    /// A pixel differs when a channel differs by more than this of 255: 2.
    pub pixel_step: u32,
    /// At most one pixel in this many may differ: 1,024.
    pub pixel_one_in: u32,
    /// No group of touching differing pixels may be larger than this: 2.
    pub pixel_group: u32,
}

impl Constants {
    pub const fn proposed(k: f64) -> Constants {
        Constants {
            k,
            graze_cos: 1.0 / 64.0,
            sliver_b: 1.0 / 16.0,
            ambiguous_one_in: 1024,
            special_one_in: 16,
            hit_one_in: 4,
            pixel_step: 2,
            pixel_one_in: 1024,
            pixel_group: 2,
        }
    }

    /// The rule as proposed, with the measured [`K`].
    pub const PROPOSED: Constants = Constants::proposed(K);

    /// Each constant halved and doubled, one at a time, by name. A share
    /// "one in n" halved is one in 2n.
    pub fn variants(&self) -> Vec<(&'static str, Constants)> {
        let c = *self;
        vec![
            ("k halved", Constants { k: c.k / 2.0, ..c }),
            ("k doubled", Constants { k: c.k * 2.0, ..c }),
            (
                "grazing cosine halved",
                Constants {
                    graze_cos: c.graze_cos / 2.0,
                    ..c
                },
            ),
            (
                "grazing cosine doubled",
                Constants {
                    graze_cos: c.graze_cos * 2.0,
                    ..c
                },
            ),
            (
                "sliver tolerance halved",
                Constants {
                    sliver_b: c.sliver_b / 2.0,
                    ..c
                },
            ),
            (
                "sliver tolerance doubled",
                Constants {
                    sliver_b: c.sliver_b * 2.0,
                    ..c
                },
            ),
            (
                "ambiguous share halved",
                Constants {
                    ambiguous_one_in: c.ambiguous_one_in * 2,
                    ..c
                },
            ),
            (
                "ambiguous share doubled",
                Constants {
                    ambiguous_one_in: (c.ambiguous_one_in / 2).max(1),
                    ..c
                },
            ),
            (
                "sliver and grazing share halved",
                Constants {
                    special_one_in: c.special_one_in * 2,
                    ..c
                },
            ),
            (
                "sliver and grazing share doubled",
                Constants {
                    special_one_in: (c.special_one_in / 2).max(1),
                    ..c
                },
            ),
            (
                "pixel step halved",
                Constants {
                    pixel_step: c.pixel_step / 2,
                    ..c
                },
            ),
            (
                "pixel step doubled",
                Constants {
                    pixel_step: c.pixel_step * 2,
                    ..c
                },
            ),
            (
                "pixel share halved",
                Constants {
                    pixel_one_in: c.pixel_one_in * 2,
                    ..c
                },
            ),
            (
                "pixel share doubled",
                Constants {
                    pixel_one_in: (c.pixel_one_in / 2).max(1),
                    ..c
                },
            ),
            (
                "pixel group halved",
                Constants {
                    pixel_group: c.pixel_group / 2,
                    ..c
                },
            ),
            (
                "pixel group doubled",
                Constants {
                    pixel_group: c.pixel_group * 2,
                    ..c
                },
            ),
            (
                "hit share halved",
                Constants {
                    hit_one_in: c.hit_one_in * 2,
                    ..c
                },
            ),
            (
                "hit share doubled",
                Constants {
                    hit_one_in: (c.hit_one_in / 2).max(1),
                    ..c
                },
            ),
        ]
    }

    /// Whether `other` classes rays as `self` does: the same `k`, grazing
    /// cosine and sliver tolerance.
    fn same_classes(&self, other: &Constants) -> bool {
        self.k == other.k && self.graze_cos == other.graze_cos && self.sliver_b == other.sliver_b
    }
}

/// Why a ray is ambiguous.
pub mod kind {
    /// An edge or a vertex shared inside one instance by triangles under
    /// matching geometry transforms.
    pub const SHARED_EDGE: u32 = 1 << 0;
    /// A seam whose sides see one object-space ray in the guest.
    pub const SEAM_CLOSED: u32 = 1 << 1;
    /// A seam whose sides have different transforms.
    pub const SEAM_OPEN: u32 = 1 << 2;
    /// A boundary or silhouette edge.
    pub const BOUNDARY_EDGE: u32 = 1 << 3;
    /// `t` within tolerance of `tmin` or `tmax`.
    pub const EXTENT: u32 = 1 << 4;
    /// Two in-pairs within their tolerances of each other.
    pub const TIE: u32 = 1 << 5;
    /// A filter decision that differs over the tolerance box.
    pub const FILTER: u32 = 1 << 6;
    /// A triangle of no area within tolerance of the ray.
    pub const DEGENERATE: u32 = 1 << 7;

    pub const NAMES: [(u32, &str); 8] = [
        (SHARED_EDGE, "shared-edge"),
        (SEAM_CLOSED, "seam-closed"),
        (SEAM_OPEN, "seam-open"),
        (BOUNDARY_EDGE, "boundary-edge"),
        (EXTENT, "extent"),
        (TIE, "tie"),
        (FILTER, "filter"),
        (DEGENERATE, "degenerate"),
    ];
}

/// What makes a pair one whose numbers are not stable.
pub mod special {
    pub const GRAZING: u32 = 1;
    pub const SLIVER: u32 = 2;
    pub const DEGENERATE: u32 = 4;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workload {
    /// The nearest hit and what it is.
    Nearest,
    /// Whether anything is hit: the ray ends at its first accepted hit.
    AnyHit,
}

/// A hit that is admissible on one triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Admissible {
    /// The reference's record for a hit on this triangle. For the
    /// reference's own answer it is the exact one.
    pub hit: Hit,
    /// The pair is near, not in.
    pub near: bool,
    /// The reference values: `t` and the weights of vertices 0, 1 and 2.
    pub t: f64,
    pub w: [f64; 3],
    /// A record's `t` must lie in this interval.
    pub t_lo: f64,
    pub t_hi: f64,
    pub t_tol: f64,
    /// The weights' tolerance; infinite where they are not compared.
    pub b_tol: f64,
    pub x_tol: f64,
    /// The pair's `M`: the largest magnitude it involves.
    pub m: f64,
    /// The cosine between the ray and the normal, as a magnitude.
    pub cos: f64,
    /// [`special`] bits.
    pub special: u32,
    /// [`kind`] bits: why the pair is near.
    pub kinds: u32,
    /// The facing is compared: not a grazing or degenerate pair, nor a
    /// sliver pair with a weight tolerance over [`FACE_B_TOL`].
    pub facing_known: bool,
    /// The candidate went through the filter (its geometry is not opaque
    /// for this ray).
    pub filtered: bool,
    /// The bottom level the triangle is in.
    pub blas: u32,
}

/// The rule's reading of one ray.
#[derive(Clone, Debug, PartialEq)]
pub struct RayRule {
    pub workload: Workload,
    pub ambiguous: bool,
    /// [`kind`] bits; 0 on an unambiguous ray.
    pub kinds: u32,
    /// [`special::GRAZING`] and [`special::SLIVER`] where such a pair can
    /// change the answer: the ray is judged by admissibility and counted
    /// apart.
    pub special: u32,
    /// The exact reference's own answer ([`crate::reference::Diag::primary`]).
    pub primary: Hit,
    /// The reference's answer first when it is a hit, then by `t`.
    pub admissible: Vec<Admissible>,
    pub miss_admissible: bool,
    /// Instance index, geometry and primitive of candidates the filter
    /// rejects away from any threshold.
    pub rejected: Vec<[u32; 3]>,
    /// The largest `M` among the admissible pairs; 0 if there are none.
    pub m: f64,
}

impl RayRule {
    /// The ray is one the reference flags: ambiguous, by a kind or by a
    /// sliver or grazing pair.
    pub fn flagged(&self) -> bool {
        self.ambiguous
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn along(a: [f64; 3], d: [f64; 3], t: f64) -> [f64; 3] {
    [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t]
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

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn largest(a: [f64; 3]) -> f64 {
    a[0].abs().max(a[1].abs()).max(a[2].abs())
}

/// The distance between the segments `p1 q1` and `p2 q2`.
fn segment_segment(p1: [f64; 3], q1: [f64; 3], p2: [f64; 3], q2: [f64; 3]) -> f64 {
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let clamp = |x: f64| x.clamp(0.0, 1.0);
    let (s, t) = if a == 0.0 && e == 0.0 {
        (0.0, 0.0)
    } else if a == 0.0 {
        (0.0, clamp(f / e))
    } else {
        let c = dot(d1, r);
        if e == 0.0 {
            (clamp(-c / a), 0.0)
        } else {
            let b = dot(d1, d2);
            let den = a * e - b * b;
            let s = if den > 0.0 {
                clamp((b * f - c * e) / den)
            } else {
                0.0
            };
            let t = (b * s + f) / e;
            if t < 0.0 {
                (clamp(-c / a), 0.0)
            } else if t > 1.0 {
                (clamp((b - c) / a), 1.0)
            } else {
                (s, t)
            }
        }
    };
    norm(sub(along(p1, d1, s), along(p2, d2, t)))
}

/// The distance between the segment `a b` and the triangle `p`, which may
/// have no area.
fn segment_triangle(a: [f64; 3], b: [f64; 3], p: &[[f64; 3]; 3]) -> f64 {
    let mut best = f64::INFINITY;
    for j in 0..3 {
        best = best.min(segment_segment(a, b, p[j], p[(j + 1) % 3]));
    }
    let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
    let nn = dot(n, n);
    if nn > 0.0 {
        // A point whose foot on the plane is inside the triangle.
        let inside = |q: [f64; 3]| {
            (0..3).all(|j| dot(cross(sub(p[(j + 1) % 3], p[j]), sub(q, p[j])), n) >= 0.0)
        };
        let (da, db) = (dot(sub(a, p[0]), n), dot(sub(b, p[0]), n));
        if da * db <= 0.0 && da != db && inside(along(a, sub(b, a), da / (da - db))) {
            return 0.0;
        }
        for (q, d) in [(a, da), (b, db)] {
            if inside(along(q, n, -d / nn)) {
                best = best.min(d.abs() / nn.sqrt());
            }
        }
    }
    best
}

/// The depths of a triangle's vertices along the ray `ro + t rd`, in units
/// of `t`, taken two ways: the foot of the vertex on the ray, and the
/// vertex's distance along the direction's largest axis (which is how the
/// emulated test, shearing the ray onto that axis, measures it). The least
/// and the greatest of the six.
fn depths(p: &[[f64; 3]; 3], ro: [f64; 3], rd: [f64; 3]) -> (f64, f64) {
    let l2 = dot(rd, rd);
    let kz = (0..3).fold(0, |m, k| if rd[k].abs() > rd[m].abs() { k } else { m });
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in p {
        let to = sub(*v, ro);
        for d in [dot(to, rd) / l2, to[kz] / rd[kz]] {
            lo = lo.min(d);
            hi = hi.max(d);
        }
    }
    (lo, hi)
}

struct RTri {
    geometry: u32,
    primitive: u32,
    /// Which of the bottom level's distinct geometry transforms it is
    /// under.
    frame: u32,
    transformed: bool,
    opaque: bool,
    /// Object space, `f64`.
    p: [[f64; 3]; 3],
    n: [f64; 3],
    /// `|n|`, twice the area.
    area2: f64,
    /// The smallest altitude.
    h: f64,
    /// The largest magnitude of a coordinate.
    mag: f64,
    /// The triangle node the software path's builder writes for it.
    node: [u32; 16],
}

struct RInst {
    /// World to object, `f64`.
    w: [f64; 12],
    /// The same rounded to `f32`, as the software path's instance record
    /// holds it.
    w32: [f32; 12],
    bits: [u32; 12],
    /// By triangle: the world-space vertices, the ids of those points, the
    /// largest coordinate magnitude, and whether the triangle has no area
    /// (exactly).
    world: Vec<[[f64; 3]; 3]>,
    vid: Vec<[u32; 3]>,
    mag: Vec<f64>,
    zero: Vec<bool>,
}

struct Cand {
    inst: u32,
    tri: u32,
    near: bool,
    special: u32,
    edges: u8,
    extent: bool,
    filter_either: bool,
    filtered: bool,
    t: f64,
    w: [f64; 3],
    t_lo: f64,
    t_hi: f64,
    t_tol: f64,
    b_tol: f64,
    x_tol: f64,
    m: f64,
    cos: f64,
    /// The face D3D12's rule gives the ray: the winding in object space,
    /// turned by the instance's counter-clockwise flag.
    front: bool,
    facing_known: bool,
}

impl Cand {
    /// Near through its edges and nothing else.
    fn pure(&self) -> bool {
        self.near && self.special == 0 && !self.extent && !self.filter_either && self.edges != 0
    }
}

enum Pair {
    Out,
    /// An ordinary pair (not grazing, sliver or degenerate) that no flag
    /// culls and that is out by a weight alone: the ray passes beside the
    /// triangle by more than its tolerance. It cannot be hit, and it still
    /// closes an edge it shares with a triangle the ray is inside. The face
    /// it shows the ray.
    OutByWeights {
        front: bool,
    },
    Rejected,
    Cand(Cand),
}

/// What two triangles that share an edge are to each other; the larger
/// closes more.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Relation {
    Boundary,
    Open,
    Closed,
    Shared,
}

impl Relation {
    fn kind(self) -> u32 {
        match self {
            Relation::Boundary => kind::BOUNDARY_EDGE,
            Relation::Open => kind::SEAM_OPEN,
            Relation::Closed => kind::SEAM_CLOSED,
            Relation::Shared => kind::SHARED_EDGE,
        }
    }
}

/// The rule's reference for one scene.
pub struct Rule<'a> {
    scene: &'a Scene,
    pub constants: Constants,
    reference: Reference<'a>,
    tris: Vec<Vec<RTri>>,
    insts: Vec<RInst>,
    /// The triangles on each edge, by the ids of its end points.
    edges: HashMap<(u32, u32), Vec<(u32, u32)>>,
    /// The triangles at each point, by its id.
    corners: HashMap<u32, Vec<(u32, u32)>>,
    /// Rays on which the exact reference's own answer was not admissible.
    /// Never seen; the host tests fail on it.
    pub inconsistent: u64,
}

fn apply64(m: &[f32; 12], p: [f64; 3]) -> [f64; 3] {
    let m = m.map(f64::from);
    std::array::from_fn(|r| {
        m[4 * r] * p[0] + m[4 * r + 1] * p[1] + m[4 * r + 2] * p[2] + m[4 * r + 3]
    })
}

impl<'a> Rule<'a> {
    /// `scene` must pass [`Scene::validate`].
    pub fn new(scene: &'a Scene, constants: Constants) -> Self {
        let mut reference = Reference::new(scene);
        let tris: Vec<Vec<RTri>> = scene
            .bottoms
            .iter()
            .map(|bottom| {
                let mut out = Vec::new();
                let mut frames: Vec<Option<[u32; 12]>> = Vec::new();
                for (g, geo) in bottom.geometries.iter().enumerate() {
                    let bits = geo.transform.map(|t| t.0.map(f32::to_bits));
                    let frame = frames.iter().position(|f| *f == bits).unwrap_or_else(|| {
                        frames.push(bits);
                        frames.len() - 1
                    });
                    for k in 0..geo.primitive_count() {
                        let raw = geo.triangle(k);
                        let p = raw.map(|v| {
                            let v = v.map(f64::from);
                            geo.transform.map_or(v, |m| apply64(&m.0, v))
                        });
                        let placed = raw.map(|v| geo.transform.map_or(v, |m| m.apply(v)));
                        let mut node = [0u32; 16];
                        for (j, x) in placed.iter().flatten().enumerate() {
                            node[j] = x.to_bits();
                        }
                        node[15] = TRIANGLE_ID_PLAIN;
                        let (e1, e2) = (sub(p[1], p[0]), sub(p[2], p[0]));
                        let n = cross(e1, e2);
                        let area2 = norm(n);
                        let longest = norm(e1).max(norm(e2)).max(norm(sub(p[2], p[1])));
                        out.push(RTri {
                            geometry: g as u32,
                            primitive: k as u32,
                            frame: frame as u32,
                            transformed: geo.transform.is_some(),
                            opaque: geo.opaque,
                            p,
                            n,
                            area2,
                            h: if longest > 0.0 { area2 / longest } else { 0.0 },
                            mag: p.iter().fold(0.0, |m, v| m.max(largest(*v))),
                            node,
                        });
                    }
                }
                out
            })
            .collect();
        let mut ids: BTreeMap<V3, u32> = BTreeMap::new();
        let mut edges: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
        let mut corners: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
        let mut insts = Vec::new();
        for (i, inst) in scene.instances.iter().enumerate() {
            let w = inst.transform.inverse().expect("a validated scene");
            let count = tris[inst.blas as usize].len();
            let mut r = RInst {
                w,
                w32: w.map(|x| x as f32),
                bits: inst.transform.0.map(f32::to_bits),
                world: Vec::with_capacity(count),
                vid: Vec::with_capacity(count),
                mag: Vec::with_capacity(count),
                zero: Vec::with_capacity(count),
            };
            for k in 0..count {
                let exact = reference.world_vertices(i, k).clone();
                let n = cross3(&sub3(&exact[1], &exact[0]), &sub3(&exact[2], &exact[0]));
                let zero = n.iter().all(|x| x.is_zero());
                let world: [[f64; 3]; 3] =
                    std::array::from_fn(|j| std::array::from_fn(|a| exact[j][a].to_f64()));
                let vid: [u32; 3] = std::array::from_fn(|j| {
                    let next = ids.len() as u32;
                    *ids.entry(exact[j].clone()).or_insert(next)
                });
                if !zero {
                    for j in 0..3 {
                        let (a, b) = (vid[j], vid[(j + 1) % 3]);
                        edges
                            .entry((a.min(b), a.max(b)))
                            .or_default()
                            .push((i as u32, k as u32));
                        corners.entry(a).or_default().push((i as u32, k as u32));
                    }
                }
                r.mag
                    .push(world.iter().fold(0.0, |m, v| m.max(largest(*v))));
                r.world.push(world);
                r.vid.push(vid);
                r.zero.push(zero);
            }
            insts.push(r);
        }
        Rule {
            scene,
            constants,
            reference,
            tris,
            insts,
            edges,
            corners,
            inconsistent: 0,
        }
    }

    pub fn trace_all(&mut self, rays: &[Ray]) -> Vec<RayRule> {
        rays.iter().map(|r| self.trace(r)).collect()
    }

    /// The ray in instance `i`'s object space, `f64`.
    fn object_ray(&self, i: usize, ray: &Ray) -> ([f64; 3], [f64; 3]) {
        let w = &self.insts[i].w;
        let (o, d) = (ray.origin.map(f64::from), ray.dir.map(f64::from));
        let row =
            |r: usize, v: [f64; 3]| w[4 * r] * v[0] + w[4 * r + 1] * v[1] + w[4 * r + 2] * v[2];
        (
            std::array::from_fn(|r| row(r, o) + w[4 * r + 3]),
            std::array::from_fn(|r| row(r, d)),
        )
    }

    /// The emulated triangle test's answer for one ray and one triangle,
    /// in the software path's arithmetic: the ray through the instance
    /// record's inverse as the traversal takes it there
    /// ([`crate::traverse`]), the node as the builder writes it
    /// ([`crate::console`]), the node test ([`crate::node`]), and the three
    /// divisions. `t`, `u`, `v`, or `None` for a miss. No extent, flag,
    /// mask or alpha test: every triangle is tested as it stands.
    pub fn emulated(&self, i: usize, k: usize, ray: &Ray) -> Option<[f32; 3]> {
        let m = &self.insts[i].w32;
        let row =
            |r: usize, p: [f32; 3]| (m[4 * r] * p[0] + m[4 * r + 1] * p[1]) + m[4 * r + 2] * p[2];
        let origin: [f32; 3] = std::array::from_fn(|r| row(r, ray.origin) + m[4 * r + 3]);
        let dir: [f32; 3] = std::array::from_fn(|r| row(r, ray.dir));
        let blas = self.scene.instances[i].blas as usize;
        let r = node::intersect(
            &self.tris[blas][k].node,
            node::node_type::TRIANGLE,
            &NodeRay::new(origin, dir, f32::INFINITY),
        );
        if r[0] == f32::INFINITY.to_bits() {
            return None;
        }
        let [tn, d, a, b] = r.map(f32::from_bits);
        Some([tn / d, a / d, b / d])
    }

    /// Triangles of instance `i`'s bottom level.
    pub fn triangles(&self, i: usize) -> usize {
        self.tris[self.scene.instances[i].blas as usize].len()
    }

    /// The rule's reading of `ray`, which must pass [`Ray::validate`].
    pub fn trace(&mut self, ray: &Ray) -> RayRule {
        let workload = if ray.flags & ray_flag::ACCEPT_FIRST_HIT_AND_END_SEARCH != 0 {
            Workload::AnyHit
        } else {
            Workload::Nearest
        };
        let (ow, dw) = (ray.origin.map(f64::from), ray.dir.map(f64::from));
        let mut cands = Vec::new();
        let mut rejected = Vec::new();
        if ray.flags & ray_flag::SKIP_TRIANGLES == 0 {
            for i in 0..self.insts.len() {
                let inst = self.scene.instances[i];
                if u32::from(inst.mask) & ray.mask & 0xFF == 0 {
                    continue;
                }
                let (ro, rd) = self.object_ray(i, ray);
                let origin = largest(ow).max(largest(ro));
                for k in 0..self.tris[inst.blas as usize].len() {
                    match self.pair(i, k, ray, ro, rd, dw, origin) {
                        Pair::Out | Pair::OutByWeights { .. } => {}
                        Pair::Rejected => {
                            let t = &self.tris[inst.blas as usize][k];
                            rejected.push([i as u32, t.geometry, t.primitive]);
                        }
                        Pair::Cand(c) => cands.push(c),
                    }
                }
            }
        }
        self.assemble(ray, workload, cands, rejected, dw)
    }

    /// One ray and one triangle: out, rejected by the filter, or a
    /// candidate that is in or near.
    #[allow(clippy::too_many_arguments)]
    fn pair(
        &self,
        i: usize,
        k: usize,
        ray: &Ray,
        ro: [f64; 3],
        rd: [f64; 3],
        dw: [f64; 3],
        origin: f64,
    ) -> Pair {
        let c = &self.constants;
        let inst = self.scene.instances[i];
        let tri = &self.tris[inst.blas as usize][k];
        let rin = &self.insts[i];
        // Opacity: the geometry's flag, then the instance's, then the
        // ray's; and the ray's opacity culls.
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
            return Pair::Out;
        }
        let (tmin, tmax) = (f64::from(ray.tmin), f64::from(ray.tmax));
        let l = norm(rd);
        let reach = l.max(norm(dw));
        let coords = origin.max(tri.mag).max(rin.mag[k]);
        let zero = rin.zero[k] || tri.area2 == 0.0;
        let s = dot(rd, tri.n);
        let cos = if zero { 0.0 } else { s.abs() / (l * tri.area2) };
        let mut kinds = 0;
        if zero {
            kinds |= special::DEGENERATE;
        } else if cos < c.graze_cos {
            kinds |= special::GRAZING;
        }
        let a = sub(tri.p[0], ro);
        let (e1, e2) = (sub(tri.p[1], tri.p[0]), sub(tri.p[2], tri.p[0]));
        let (t, w) = if s != 0.0 {
            let (x1, x2) = (dot(a, cross(rd, e2)), dot(a, cross(e1, rd)));
            (dot(a, tri.n) / s, [(s - x1 - x2) / s, x1 / s, x2 / s])
        } else {
            (f64::NAN, [f64::NAN; 3])
        };
        // Facing in object space, D3D12's rule.
        let mut front = s < 0.0;
        if inst.flags & instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE != 0 {
            front = !front;
        }
        let culled = |front: bool| {
            let flag = if front {
                ray_flag::CULL_FRONT_FACING_TRIANGLES
            } else {
                ray_flag::CULL_BACK_FACING_TRIANGLES
            };
            inst.flags & instance_flag::TRIANGLE_CULL_DISABLE == 0 && ray.flags & flag != 0
        };
        let mut x_tol = 0.0;
        let mut b_tol = f64::INFINITY;
        let mut m = coords;
        if kinds == 0 {
            m = coords.max(t.abs().min(tmax) * reach);
            x_tol = c.k * SPACING * m;
            b_tol = 2.0 * x_tol / (tri.h * cos);
            if b_tol > c.sliver_b {
                kinds |= special::SLIVER;
            }
        }
        if kinds == 0 {
            if w.iter().any(|x| *x < -b_tol) {
                return if culled(front) {
                    Pair::Out
                } else {
                    Pair::OutByWeights { front }
                };
            }
            let (lo, hi) = depths(&tri.p, ro, rd);
            let t_tol = x_tol / (l * cos) + b_tol * (t - lo).abs().max((hi - t).abs());
            if t < tmin - t_tol || t > tmax + t_tol {
                return Pair::Out;
            }
            if culled(front) {
                return Pair::Out;
            }
            let mut edges = 0u8;
            for (j, x) in w.iter().enumerate() {
                if *x <= b_tol {
                    edges |= 1 << j;
                }
            }
            let extent = t - tmin <= t_tol || tmax - t <= t_tol;
            let mut filter_either = false;
            if !opaque {
                match filter(w[1], w[2], b_tol, tri.primitive) {
                    Filter::Accept => {}
                    Filter::Reject => return Pair::Rejected,
                    Filter::Either => filter_either = true,
                }
            }
            return Pair::Cand(Cand {
                inst: i as u32,
                tri: k as u32,
                near: edges != 0 || extent || filter_either,
                special: 0,
                edges,
                extent,
                filter_either,
                filtered: !opaque,
                t,
                w,
                t_lo: t - t_tol,
                t_hi: t + t_tol,
                t_tol,
                b_tol,
                x_tol,
                m,
                cos,
                front,
                facing_known: true,
            });
        }
        // A grazing, sliver or degenerate pair: placed by distance.
        let (lo, hi) = depths(&tri.p, ro, rd);
        m = coords.max(lo.abs().max(hi.abs()).min(tmax) * reach);
        x_tol = c.k * SPACING * m;
        let t_tol = x_tol / (l * c.graze_cos);
        let (t_lo, t_hi) = (
            (lo - t_tol).max(tmin - t_tol),
            (hi + t_tol).min(tmax + t_tol),
        );
        if t_lo > t_hi {
            return Pair::Out;
        }
        let far = segment_triangle(along(ro, rd, t_lo), along(ro, rd, t_hi), &tri.p);
        if far > 2.0 * x_tol {
            return Pair::Out;
        }
        // The face of a sliver pair is a fact while its triangle is not
        // thinner than the tolerance ([`FACE_B_TOL`]: no move of the
        // vertices by `x_tol` turns it); past that, and for a grazing ray,
        // it is not: such a pair is not culled by a facing flag and its
        // face is not compared.
        let facing_known = kinds == special::SLIVER && b_tol <= FACE_B_TOL;
        if facing_known && culled(front) {
            return Pair::Out;
        }
        Pair::Cand(Cand {
            inst: i as u32,
            tri: k as u32,
            near: true,
            special: kinds,
            edges: 0,
            extent: false,
            filter_either: !opaque,
            filtered: !opaque,
            t: if t.is_finite() {
                t.clamp(lo, hi)
            } else {
                0.5 * (lo + hi)
            },
            w: if w.iter().all(|x| x.is_finite()) {
                w
            } else {
                [1.0, 0.0, 0.0]
            },
            t_lo,
            t_hi,
            t_tol,
            b_tol: f64::INFINITY,
            x_tol,
            m,
            cos,
            front,
            facing_known,
        })
    }

    /// The world-space position of the point `id`, a vertex of triangle
    /// `at` (instance, triangle).
    fn corner(&self, at: (u32, u32), id: u32) -> [f64; 3] {
        let rin = &self.insts[at.0 as usize];
        let j = (0..3)
            .find(|&j| rin.vid[at.1 as usize][j] == id)
            .expect("a vertex of the triangle");
        rin.world[at.1 as usize][j]
    }

    /// The corner of triangle `at` that is neither `p` nor `q`, and where
    /// it is in world space.
    fn third(&self, at: (u32, u32), p: u32, q: u32) -> Option<(usize, [f64; 3])> {
        let rin = &self.insts[at.0 as usize];
        let j = (0..3).find(|&j| {
            let id = rin.vid[at.1 as usize][j];
            id != p && id != q
        })?;
        Some((j, rin.world[at.1 as usize][j]))
    }

    /// What two triangles (instance, triangle) are to each other across an
    /// edge or a vertex they share.
    fn relation(&self, a: (u32, u32), b: (u32, u32)) -> Relation {
        let tri = |c: (u32, u32)| {
            let blas = self.scene.instances[c.0 as usize].blas as usize;
            &self.tris[blas][c.1 as usize]
        };
        let (ta, tb) = (tri(a), tri(b));
        if a.0 == b.0 && ta.frame == tb.frame {
            Relation::Shared
        } else if self.insts[a.0 as usize].bits == self.insts[b.0 as usize].bits
            && !ta.transformed
            && !tb.transformed
        {
            Relation::Closed
        } else {
            Relation::Open
        }
    }

    /// For a triangle that is not among a ray's candidates: the face it
    /// shows the ray when it is an ordinary pair the ray may see (its
    /// instance not masked, no flag culling it) that is out by a weight
    /// alone; `None` for any other.
    fn beside(&self, at: (u32, u32), ray: &Ray, dw: [f64; 3]) -> Option<bool> {
        let (i, k) = (at.0 as usize, at.1 as usize);
        if u32::from(self.scene.instances[i].mask) & ray.mask & 0xFF == 0 {
            return None;
        }
        let (ro, rd) = self.object_ray(i, ray);
        let origin = largest(ray.origin.map(f64::from)).max(largest(ro));
        match self.pair(i, k, ray, ro, rd, dw, origin) {
            Pair::OutByWeights { front } => Some(front),
            _ => None,
        }
    }

    fn assemble(
        &mut self,
        ray: &Ray,
        workload: Workload,
        cands: Vec<Cand>,
        rejected: Vec<[u32; 3]>,
        dw: [f64; 3],
    ) -> RayRule {
        let n = cands.len();
        let mut kinds = vec![0u32; n];
        let mut covers: Vec<f64> = Vec::new();
        for a in 0..n {
            let ca = &cands[a];
            if !ca.near {
                continue;
            }
            if ca.extent {
                kinds[a] |= kind::EXTENT;
            }
            if ca.filter_either && ca.special == 0 {
                kinds[a] |= kind::FILTER;
            }
            if ca.special & special::DEGENERATE != 0 {
                kinds[a] |= kind::DEGENERATE;
            }
            if ca.special != 0 || ca.edges == 0 {
                continue;
            }
            let va = self.insts[ca.inst as usize].vid[ca.tri as usize];
            let here = (ca.inst, ca.tri);
            let mut best = Relation::Boundary;
            match ca.edges.count_ones() {
                1 => {
                    let j = ca.edges.trailing_zeros() as usize;
                    let (p, q) = (va[(j + 1) % 3], va[(j + 2) % 3]);
                    let (p, q) = (p.min(q), p.max(q));
                    let (pp, pq) = (self.corner(here, p), self.corner(here, q));
                    let side = |r: [f64; 3]| dot(dw, cross(sub(pq, pp), sub(r, pp)));
                    let ra = self.insts[ca.inst as usize].world[ca.tri as usize][j];
                    // Whether the ray's line meets this triangle, exactly:
                    // asked only where a neighbour that is out decides.
                    let mut inside: Option<bool> = None;
                    let on_edge = self.edges.get(&(p, q)).cloned().unwrap_or_default();
                    for nb in on_edge {
                        if nb == here {
                            continue;
                        }
                        let Some((nj, rb)) = self.third(nb, p, q) else {
                            continue;
                        };
                        if side(ra) * side(rb) >= 0.0 {
                            continue;
                        }
                        let rel = self.relation(here, nb);
                        if let Some(cb) = cands.iter().find(|c| (c.inst, c.tri) == nb) {
                            // The two cover the ray between them, and show
                            // it the same face.
                            if cb.special != 0 || ca.front != cb.front {
                                continue;
                            }
                            best = best.max(rel);
                            let through_it = cb.pure() && cb.edges == 1 << nj;
                            if rel >= Relation::Closed && ca.pure() && through_it {
                                covers.push(ca.t_hi.max(cb.t_hi));
                            }
                        } else if self.beside(nb, ray, dw) == Some(ca.front) {
                            // The neighbour shows the same face and is out
                            // by its own, smaller tolerance: it cannot be
                            // hit. Where the ray is in this triangle, the
                            // two still cover it and this one is found;
                            // where it is not, it passes beyond the
                            // neighbour's reach and the edge is a boundary
                            // for it.
                            let (i, k) = (ca.inst as usize, ca.tri as usize);
                            let reference = &mut self.reference;
                            if *inside.get_or_insert_with(|| reference.exact_pair(i, k, ray).inside)
                            {
                                best = best.max(rel);
                                if rel >= Relation::Closed && ca.pure() {
                                    covers.push(ca.t_hi);
                                }
                            }
                        }
                    }
                }
                2 => {
                    let h = (!ca.edges & 7).trailing_zeros() as usize;
                    let hub = va[h];
                    let mut rim: Vec<u32> = Vec::new();
                    let mut latest = f64::NEG_INFINITY;
                    let mut like = true;
                    let mut rel = Relation::Shared;
                    // The fan's members among the candidates, and whether
                    // it has one the ray is beside (out by its own
                    // tolerance, which cannot be hit and still closes the
                    // fan).
                    let mut members: Vec<(u32, u32)> = Vec::new();
                    let mut beside = false;
                    let around = self.corners.get(&hub).cloned().unwrap_or_default();
                    for nb in around {
                        let vb = self.insts[nb.0 as usize].vid[nb.1 as usize];
                        let Some(jb) = (0..3).find(|&j| vb[j] == hub) else {
                            continue;
                        };
                        let front = if let Some(cb) = cands.iter().find(|c| (c.inst, c.tri) == nb) {
                            if !cb.pure() || cb.edges & (1 << jb) != 0 {
                                continue;
                            }
                            latest = latest.max(cb.t_hi);
                            members.push(nb);
                            cb.front
                        } else if let Some(front) = self.beside(nb, ray, dw) {
                            beside = true;
                            front
                        } else {
                            continue;
                        };
                        rim.extend([vb[(jb + 1) % 3], vb[(jb + 2) % 3]]);
                        like &= front == ca.front;
                        rel = rel.min(self.relation(here, nb));
                    }
                    // A closed fan: every point of the rim ends two of its
                    // triangles' far edges, whichever way each is wound (a
                    // mirrored instance turns its own).
                    rim.sort_unstable();
                    let closed = rim.len() >= 6
                        && rim.chunks(2).all(|pair| pair[0] == pair[1])
                        && rim
                            .chunks(2)
                            .zip(rim.chunks(2).skip(1))
                            .all(|(a, b)| a[0] != b[0]);
                    if closed && like && ca.pure() {
                        // With a member the ray is beside, the fan covers
                        // the ray where the ray is in one of the others,
                        // exactly; elsewhere it passes beyond that member's
                        // reach.
                        let reference = &mut self.reference;
                        let held = !beside
                            || members.iter().any(|m| {
                                reference.exact_pair(m.0 as usize, m.1 as usize, ray).inside
                            });
                        if held {
                            best = rel;
                            if rel >= Relation::Closed {
                                covers.push(latest);
                            }
                        }
                    }
                }
                _ => {}
            }
            kinds[a] |= best.kind();
        }

        // The latest `t` an answer can have: a pair that is in is found,
        // and so is one member of a closed group.
        let nearest = workload == Workload::Nearest;
        let mut bound = f64::INFINITY;
        for c in cands.iter().filter(|c| !c.near) {
            bound = bound.min(c.t_hi);
        }
        for g in &covers {
            bound = bound.min(*g);
        }
        let certain = bound < f64::INFINITY;
        let mut relevant: Vec<usize> = (0..n)
            .filter(|&k| !nearest || cands[k].t_lo <= bound)
            .collect();
        let ins = relevant.iter().filter(|&&k| !cands[k].near).count();
        let ambiguous = match workload {
            Workload::Nearest => !(relevant.is_empty() || (relevant.len() == 1 && ins == 1)),
            Workload::AnyHit => !(certain || relevant.is_empty()),
        };
        let (mut ray_kinds, mut ray_special) = (0, 0);
        if ambiguous {
            for &k in &relevant {
                if cands[k].near {
                    ray_kinds |= kinds[k];
                    ray_special |= cands[k].special & (special::GRAZING | special::SLIVER);
                }
            }
            if nearest && ins >= 2 {
                ray_kinds |= kind::TIE;
            }
        }
        relevant.sort_by(|&a, &b| {
            cands[a]
                .t
                .total_cmp(&cands[b].t)
                .then(cands[a].inst.cmp(&cands[b].inst))
                .then(cands[a].tri.cmp(&cands[b].tri))
        });
        let mut admissible: Vec<Admissible> = relevant
            .iter()
            .map(|&k| {
                let c = &cands[k];
                let inst = self.scene.instances[c.inst as usize];
                let tri = &self.tris[inst.blas as usize][c.tri as usize];
                Admissible {
                    hit: Hit {
                        t: c.t as f32,
                        u: c.w[1] as f32,
                        v: c.w[2] as f32,
                        kind: if c.front {
                            hit_kind::FRONT
                        } else {
                            hit_kind::BACK
                        },
                        instance_index: c.inst,
                        instance_id: inst.id,
                        geometry_index: tri.geometry,
                        primitive_index: tri.primitive,
                    },
                    near: c.near,
                    t: c.t,
                    w: c.w,
                    t_lo: c.t_lo,
                    t_hi: c.t_hi,
                    t_tol: c.t_tol,
                    b_tol: c.b_tol,
                    x_tol: c.x_tol,
                    m: c.m,
                    cos: c.cos,
                    special: c.special,
                    kinds: if c.near { kinds[k] } else { 0 },
                    facing_known: c.facing_known,
                    filtered: c.filtered,
                    blas: inst.blas,
                }
            })
            .collect();
        let m = relevant.iter().fold(0.0f64, |m, &k| m.max(cands[k].m));
        // The exact reference's own answer leads the list, as the record
        // it gives.
        let primary = self.reference.trace(ray).primary;
        if primary.is_miss() {
            if certain {
                self.inconsistent += 1;
            }
        } else if let Some(at) = admissible.iter().position(|e| e.hit.ids() == primary.ids()) {
            let mut first = admissible.remove(at);
            first.hit = primary;
            admissible.insert(0, first);
        } else {
            self.inconsistent += 1;
        }
        RayRule {
            workload,
            ambiguous,
            kinds: ray_kinds,
            special: ray_special,
            primary,
            admissible,
            miss_admissible: !certain,
            rejected,
            m,
        }
    }
}

enum Filter {
    Accept,
    Reject,
    Either,
}

/// The alpha rule over the tolerance box of its inputs.
fn filter(u: f64, v: f64, b_tol: f64, primitive: u32) -> Filter {
    // A box as wide as a cell holds a boundary whatever its corners say.
    if b_tol.is_nan() || b_tol >= 1.0 / 16.0 {
        return Filter::Either;
    }
    let (mut yes, mut no) = (false, false);
    for du in [-b_tol, 0.0, b_tol] {
        for dv in [-b_tol, 0.0, b_tol] {
            if alpha_accepts((u + du) as f32, (v + dv) as f32, primitive) {
                yes = true;
            } else {
                no = true;
            }
        }
    }
    match (yes, no) {
        (true, false) => Filter::Accept,
        (false, true) => Filter::Reject,
        _ => Filter::Either,
    }
}

/// How one record stands under the rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleOutcome {
    /// The one answer of an unambiguous ray.
    Agree,
    /// An admissible answer of an ambiguous ray.
    Admissible,
    /// A miss where none is admissible.
    LostHit,
    /// A hit where only a miss is admissible.
    GainedHit,
    /// A triangle that is not admissible, of a surface that is.
    IdFlip,
    /// A triangle that is not admissible, of another surface.
    SurfaceFlip,
    /// An admissible triangle with the other face.
    FacingFlip,
    /// A filter decision that differs on a candidate not at a threshold.
    FilterFlip,
    /// An admissible triangle and face; a number out of tolerance.
    OutOfTol,
    /// The shader refused the ray: neither a miss nor a failure. Counted
    /// by reason.
    Refused,
    /// Not a record: an unknown kind, a miss or a refusal with other words
    /// set, a hit with a number that is not finite.
    Malformed,
}

impl RuleOutcome {
    pub fn is_failure(self) -> bool {
        !matches!(
            self,
            RuleOutcome::Agree | RuleOutcome::Admissible | RuleOutcome::Refused
        )
    }
}

/// A judged record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuleJudged {
    pub outcome: RuleOutcome,
    /// The errors in `t` and in the weights (the largest of the three): in
    /// tolerances, and as position errors in spacings of the pair's largest
    /// magnitude (units of 2^-23 `M`, the scale the constant `k` is on). 0
    /// where the numbers are not compared.
    pub t_err: f64,
    pub t_spacings: f64,
    pub b_err: f64,
    pub b_spacings: f64,
    /// An admissible hit on another surface than the reference's own
    /// answer.
    pub other_surface: bool,
    /// A hit of kind 3: no face to compare.
    pub procedural: bool,
    /// The [`refusal`] bits of a refused record.
    pub reasons: u32,
    /// The `M` of the pair the record's numbers were compared with; 0 where
    /// none were (a miss, a refusal, a failure before the numbers, a hit on
    /// a sliver, grazing or degenerate pair).
    pub m: f64,
}

/// One record against the rule's reading of its ray.
pub fn judge(rule: &RayRule, got: &Hit) -> RuleJudged {
    let plain = |outcome| RuleJudged {
        outcome,
        t_err: 0.0,
        t_spacings: 0.0,
        b_err: 0.0,
        b_spacings: 0.0,
        other_surface: false,
        procedural: false,
        reasons: 0,
        m: 0.0,
    };
    let right = if rule.ambiguous {
        RuleOutcome::Admissible
    } else {
        RuleOutcome::Agree
    };
    if got.is_refused() {
        return match got.refusal_reasons() {
            Some(reasons) => RuleJudged {
                reasons,
                ..plain(RuleOutcome::Refused)
            },
            None => plain(RuleOutcome::Malformed),
        };
    }
    // The filter decided otherwise when every admissible hit went through
    // it and the record names none of them.
    let all_filtered = !rule.admissible.is_empty() && rule.admissible.iter().all(|e| e.filtered);
    if got.is_miss() {
        return plain(if *got != Hit::MISS {
            RuleOutcome::Malformed
        } else if rule.miss_admissible {
            right
        } else if all_filtered {
            RuleOutcome::FilterFlip
        } else {
            RuleOutcome::LostHit
        });
    }
    let procedural = got.kind == hit_kind::PROCEDURAL;
    if !matches!(
        got.kind,
        hit_kind::FRONT | hit_kind::BACK | hit_kind::PROCEDURAL
    ) || [got.t, got.u, got.v].iter().any(|x| !x.is_finite())
    {
        return plain(RuleOutcome::Malformed);
    }
    let Some(e) = rule.admissible.iter().find(|e| e.hit.ids() == got.ids()) else {
        let named = [got.instance_index, got.geometry_index, got.primitive_index];
        let surface = shade::surface(got);
        return plain(if rule.rejected.contains(&named) || all_filtered {
            RuleOutcome::FilterFlip
        } else if rule.admissible.is_empty() {
            RuleOutcome::GainedHit
        } else if rule
            .admissible
            .iter()
            .any(|e| shade::surface(&e.hit) == surface)
        {
            RuleOutcome::IdFlip
        } else {
            RuleOutcome::SurfaceFlip
        });
    };
    let mut j = RuleJudged {
        procedural,
        other_surface: !rule.primary.is_miss()
            && shade::surface(&rule.primary) != shade::surface(got),
        ..plain(right)
    };
    if !procedural && e.facing_known && got.kind != e.hit.kind {
        j.outcome = RuleOutcome::FacingFlip;
        return j;
    }
    let t = f64::from(got.t);
    let mut out = t < e.t_lo || t > e.t_hi;
    if e.b_tol.is_finite() {
        let k = e.x_tol / (SPACING * e.m);
        j.t_err = (t - e.t).abs() / e.t_tol;
        j.t_spacings = j.t_err * k;
        let (u, v) = (f64::from(got.u), f64::from(got.v));
        let b = (1.0 - u - v - e.w[0])
            .abs()
            .max((u - e.w[1]).abs())
            .max((v - e.w[2]).abs());
        j.b_err = b / e.b_tol;
        j.b_spacings = j.b_err * k;
        j.m = e.m;
        out |= b > e.b_tol;
    }
    if out {
        j.outcome = RuleOutcome::OutOfTol;
    }
    j
}

/// The counts of level 1 over a class: dxr-validation.md's columns.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level1 {
    pub rays: u32,
    /// Rays with one admissible answer.
    pub unambiguous: u32,
    /// Ambiguous rays with no sliver or grazing pair: what the bound is
    /// on.
    pub ambiguous: u32,
    /// Ambiguous rays with a sliver or grazing pair, judged by
    /// admissibility and counted apart.
    pub special: u32,
    /// The ambiguous rays, sliver and grazing ones included, by kind: one
    /// count per [`kind::NAMES`] entry (a ray can have several).
    pub by_kind: [u32; kind::NAMES.len()],
    pub agree: u32,
    /// Of `agree`, the hits: unambiguous rays whose record names the one
    /// triangle with its numbers inside their tolerances. What the class
    /// held the path to by numbers.
    pub agree_hits: u32,
    pub admissible: u32,
    pub lost_hits: u32,
    pub gained_hits: u32,
    pub id_flips: u32,
    pub surface_flips: u32,
    pub facing_flips: u32,
    pub filter_flips: u32,
    pub out_of_tol: u32,
    pub malformed: u32,
    /// Refused records, and by reason: geometry, range.
    pub refused: u32,
    pub refused_by_reason: [u32; refusal::NAMES.len()],
    /// Of `refused`, those on rays with a sliver or grazing pair (counted
    /// in `special` too): taken once where the two are added.
    pub refused_special: u32,
    /// Admissible hits of kind 3.
    pub procedural: u32,
    /// Admissible hits on another surface than the reference's own.
    pub other_surface: u32,
    /// The largest errors over the records whose numbers were compared, in
    /// tolerances and in spacings.
    pub t_err_max: f64,
    pub t_spacings_max: f64,
    pub b_err_max: f64,
    pub b_spacings_max: f64,
    /// How far the class reaches: the largest `M` among the unambiguous
    /// rays whose hit agreed, the pairs whose numbers were compared. A ray
    /// with a sliver or grazing pair adds nothing to it.
    pub m_max: f64,
    /// The largest `M` among the rays with a sliver or grazing pair, whose
    /// numbers are not compared: reported apart, and no reach.
    pub m_special_max: f64,
}

impl Level1 {
    pub fn failures(&self) -> u32 {
        self.lost_hits
            + self.gained_hits
            + self.id_flips
            + self.surface_flips
            + self.facing_flips
            + self.filter_flips
            + self.out_of_tol
            + self.malformed
    }

    pub fn add(&mut self, rule: &RayRule, j: &RuleJudged) {
        self.rays += 1;
        if !rule.ambiguous {
            self.unambiguous += 1;
        } else {
            if rule.special != 0 {
                self.special += 1;
            } else {
                self.ambiguous += 1;
            }
            for (k, (bit, _)) in kind::NAMES.iter().enumerate() {
                self.by_kind[k] += u32::from(rule.kinds & bit != 0);
            }
        }
        let count = match j.outcome {
            RuleOutcome::Agree => &mut self.agree,
            RuleOutcome::Admissible => &mut self.admissible,
            RuleOutcome::LostHit => &mut self.lost_hits,
            RuleOutcome::GainedHit => &mut self.gained_hits,
            RuleOutcome::IdFlip => &mut self.id_flips,
            RuleOutcome::SurfaceFlip => &mut self.surface_flips,
            RuleOutcome::FacingFlip => &mut self.facing_flips,
            RuleOutcome::FilterFlip => &mut self.filter_flips,
            RuleOutcome::OutOfTol => &mut self.out_of_tol,
            RuleOutcome::Refused => &mut self.refused,
            RuleOutcome::Malformed => &mut self.malformed,
        };
        *count += 1;
        for (k, (bit, _)) in refusal::NAMES.iter().enumerate() {
            self.refused_by_reason[k] += u32::from(j.reasons & bit != 0);
        }
        if j.outcome == RuleOutcome::Refused && rule.special != 0 {
            self.refused_special += 1;
        }
        if j.outcome == RuleOutcome::Agree && j.m > 0.0 {
            self.agree_hits += 1;
            self.m_max = self.m_max.max(j.m);
        }
        if rule.special != 0 {
            self.m_special_max = self.m_special_max.max(rule.m);
        }
        if !j.outcome.is_failure() {
            self.procedural += u32::from(j.procedural);
            self.other_surface += u32::from(j.other_surface);
        }
        self.t_err_max = self.t_err_max.max(j.t_err);
        self.t_spacings_max = self.t_spacings_max.max(j.t_spacings);
        self.b_err_max = self.b_err_max.max(j.b_err);
        self.b_spacings_max = self.b_spacings_max.max(j.b_spacings);
    }

    /// The rays the class did not hold the path to by numbers: those with
    /// a sliver or grazing pair and those refused, each ray once.
    pub fn set_apart(&self) -> u32 {
        self.special + self.refused - self.refused_special.min(self.refused)
    }
}

/// The counts of level 2 over a class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Level2 {
    /// What the record gives shading is the reference's, word for word.
    pub equal: u32,
    /// Integers of an admissible answer, floats within the carried
    /// tolerance.
    pub within: u32,
    pub differ: u32,
    /// Refused records: nothing was consumed.
    pub not_judged: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleVerdict {
    /// A random class: no failure, the ambiguous rays inside the bound, and
    /// enough rays and hits for that to say something.
    Pass,
    /// A directed class: every answer admissible. Never a pass alone.
    Admissible,
    /// A record that is not accepted, at level 1 or 2.
    Fail,
    /// A random class with more ambiguous rays than the bound: not
    /// decidable at this tolerance, which is not a pass.
    NotDecidable,
    /// A random class that validates nothing: more rays than the share
    /// allows have a sliver or grazing pair or were refused, the two
    /// together; or it has fewer rays without such a pair than the bound
    /// can count; or too few of its rays are unambiguous hits that agreed.
    ValidatesNothing,
}

/// The verdict of levels 1 and 2 for a class of `kind`.
///
/// For a random class, in this order: `Fail` on any failure;
/// `ValidatesNothing` when more than one ray in `special_one_in` has a
/// sliver or grazing pair or was refused ([`Level1::set_apart`]);
/// `NotDecidable` when, among the rays without such a pair, more than one
/// in `ambiguous_one_in` is ambiguous; `ValidatesNothing` when there are
/// fewer than `ambiguous_one_in` rays without such a pair (a bound of one
/// in 1,024 cannot be counted on fewer: no ray, three rays), or fewer than
/// one ray in `hit_one_in` is an unambiguous hit that agreed (rays that
/// meet nothing hold a path to nothing); else `Pass`.
pub fn verdict(kind: ClassKind, l1: &Level1, l2: &Level2, c: &Constants) -> RuleVerdict {
    if l1.failures() > 0 || l2.differ > 0 {
        return RuleVerdict::Fail;
    }
    if kind == ClassKind::Directed {
        return RuleVerdict::Admissible;
    }
    let (rays, special) = (u64::from(l1.rays), u64::from(l1.special));
    if u64::from(l1.set_apart()) * u64::from(c.special_one_in) > rays {
        return RuleVerdict::ValidatesNothing;
    }
    let plain = rays.saturating_sub(special);
    if u64::from(l1.ambiguous) * u64::from(c.ambiguous_one_in) > plain {
        return RuleVerdict::NotDecidable;
    }
    if plain < u64::from(c.ambiguous_one_in)
        || u64::from(l1.agree_hits) * u64::from(c.hit_one_in) < rays
    {
        return RuleVerdict::ValidatesNothing;
    }
    RuleVerdict::Pass
}

/// Levels 1 and 2 of one list of records.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    pub rules: Vec<RayRule>,
    pub judged: Vec<RuleJudged>,
    pub consumed: Vec<ConsumedOutcome>,
    pub level1: Level1,
    pub level2: Level2,
}

/// Judges `hits`, one per ray, against the rule with `constants`.
///
/// An error, and no verdict, for: a scene that fails [`Scene::validate`];
/// a ray that fails [`Ray::validate`] (one D3D12 forbids or leaves
/// undefined: it is to be screened and counted before it reaches a path,
/// and a list that holds one was not screened), named by its place; lists
/// of different lengths; and a ray on which the exact reference's own
/// answer is not among those the rule admits, which would be a defect of
/// this reference and has never been seen.
pub fn evaluate(
    scene: &Scene,
    rays: &[Ray],
    hits: &[Hit],
    constants: &Constants,
) -> Result<Evaluation, String> {
    if rays.len() != hits.len() {
        return Err(format!("{} rays, {} records", rays.len(), hits.len()));
    }
    scene
        .validate()
        .map_err(|e| format!("a scene the rule does not read: {e}"))?;
    for (k, ray) in rays.iter().enumerate() {
        ray.validate()
            .map_err(|e| format!("ray {k} is not one to judge: {e}"))?;
    }
    let mut rule = Rule::new(scene, *constants);
    let mut rules = Vec::with_capacity(rays.len());
    for (k, ray) in rays.iter().enumerate() {
        rules.push(rule.trace(ray));
        if rule.inconsistent != 0 {
            return Err(format!(
                "ray {k}: the exact reference's own answer is not among those the rule admits; the reference contradicts itself and gives no verdict"
            ));
        }
    }
    let mut e = Evaluation {
        judged: Vec::with_capacity(rays.len()),
        consumed: Vec::with_capacity(rays.len()),
        level1: Level1::default(),
        level2: Level2::default(),
        rules,
    };
    for ((ray, r), hit) in rays.iter().zip(&e.rules).zip(hits) {
        let j = judge(r, hit);
        e.level1.add(r, &j);
        let c = shade::judge_consumed(scene, ray, r, hit);
        let count = match c {
            ConsumedOutcome::Equal => &mut e.level2.equal,
            ConsumedOutcome::Within => &mut e.level2.within,
            ConsumedOutcome::Differ => &mut e.level2.differ,
            ConsumedOutcome::NotJudged => &mut e.level2.not_judged,
        };
        *count += 1;
        e.judged.push(j);
        e.consumed.push(c);
    }
    Ok(e)
}

/// Sums the counts of several evaluations (the frames of a class).
pub fn sum(parts: &[&Evaluation]) -> (Level1, Level2) {
    let (mut a, mut b) = (Level1::default(), Level2::default());
    for e in parts {
        let l = &e.level1;
        a.rays += l.rays;
        a.unambiguous += l.unambiguous;
        a.ambiguous += l.ambiguous;
        a.special += l.special;
        for k in 0..a.by_kind.len() {
            a.by_kind[k] += l.by_kind[k];
        }
        a.agree += l.agree;
        a.agree_hits += l.agree_hits;
        a.admissible += l.admissible;
        a.lost_hits += l.lost_hits;
        a.gained_hits += l.gained_hits;
        a.id_flips += l.id_flips;
        a.surface_flips += l.surface_flips;
        a.facing_flips += l.facing_flips;
        a.filter_flips += l.filter_flips;
        a.out_of_tol += l.out_of_tol;
        a.malformed += l.malformed;
        a.refused += l.refused;
        for k in 0..a.refused_by_reason.len() {
            a.refused_by_reason[k] += l.refused_by_reason[k];
        }
        a.refused_special += l.refused_special;
        a.procedural += l.procedural;
        a.other_surface += l.other_surface;
        a.t_err_max = a.t_err_max.max(l.t_err_max);
        a.t_spacings_max = a.t_spacings_max.max(l.t_spacings_max);
        a.b_err_max = a.b_err_max.max(l.b_err_max);
        a.b_spacings_max = a.b_spacings_max.max(l.b_spacings_max);
        a.m_max = a.m_max.max(l.m_max);
        a.m_special_max = a.m_special_max.max(l.m_special_max);
        b.equal += e.level2.equal;
        b.within += e.level2.within;
        b.differ += e.level2.differ;
        b.not_judged += e.level2.not_judged;
    }
    (a, b)
}

/// The verdict of levels 1 and 2 with the constants as given, and again
/// with each constant halved and doubled.
#[derive(Clone, Debug, PartialEq)]
pub struct Assessment {
    pub evaluation: Evaluation,
    pub verdict: RuleVerdict,
    /// The constants whose halving or doubling changes the verdict, with
    /// the verdict it changes to. Not empty: the verdict is `sensitive`.
    pub sensitive: Vec<(&'static str, RuleVerdict)>,
}

/// [`evaluate`], the verdict for a class of `kind`, and the verdict's
/// sensitivity to each constant. The pixel constants do not enter levels 1
/// and 2 ([`crate::picture::assess`] has them).
pub fn assess(
    kind: ClassKind,
    scene: &Scene,
    rays: &[Ray],
    hits: &[Hit],
    constants: &Constants,
) -> Result<Assessment, String> {
    let evaluation = evaluate(scene, rays, hits, constants)?;
    let base = verdict(kind, &evaluation.level1, &evaluation.level2, constants);
    let mut sensitive = Vec::new();
    for (name, c) in constants.variants() {
        let v = if c.same_classes(constants) {
            verdict(kind, &evaluation.level1, &evaluation.level2, &c)
        } else {
            let e = evaluate(scene, rays, hits, &c)?;
            verdict(kind, &e.level1, &e.level2, &c)
        };
        if v != base {
            sensitive.push((name, v));
        }
    }
    Ok(Assessment {
        evaluation,
        verdict: base,
        sensitive,
    })
}

fn kinds_text(counts: &[u32; kind::NAMES.len()]) -> String {
    let parts: Vec<String> = kind::NAMES
        .iter()
        .zip(counts)
        .filter(|(_, n)| **n > 0)
        .map(|((_, name), n)| format!("{name} {n}"))
        .collect();
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(", ")
    }
}

/// Levels 1 and 2 as a few lines of text.
pub fn to_text(
    class: &str,
    v: RuleVerdict,
    l1: &Level1,
    l2: &Level2,
    sensitive: &[(&str, RuleVerdict)],
) -> String {
    let mut head = format!("{class}: {v:?}");
    if !sensitive.is_empty() {
        let parts: Vec<String> = sensitive
            .iter()
            .map(|(name, v)| format!("{name}: {v:?}"))
            .collect();
        head += &format!(", sensitive ({})", parts.join("; "));
    }
    format!(
        "{head}\n  rays {} unambiguous {} ambiguous {} with a sliver or grazing pair {} (ambiguous by kind: {})\n  agree {} (hits {}) admissible {} (other surface {}, procedural {}) | lost {} gained {} id-flips {} surface-flips {} facing-flips {} filter-flips {} out-of-tolerance {} malformed {} | refused {} (geometry {}, range {})\n  worst t {:.3} of tolerance ({:.1} spacings), weights {:.3} ({:.1} spacings); largest magnitude among the unambiguous hits {:.3e}, among the rays with a sliver or grazing pair {:.3e}\n  consumed: equal {} within {} differ {} not judged {}\n",
        l1.rays,
        l1.unambiguous,
        l1.ambiguous,
        l1.special,
        kinds_text(&l1.by_kind),
        l1.agree,
        l1.agree_hits,
        l1.admissible,
        l1.other_surface,
        l1.procedural,
        l1.lost_hits,
        l1.gained_hits,
        l1.id_flips,
        l1.surface_flips,
        l1.facing_flips,
        l1.filter_flips,
        l1.out_of_tol,
        l1.malformed,
        l1.refused,
        l1.refused_by_reason[0],
        l1.refused_by_reason[1],
        l1.t_err_max,
        l1.t_spacings_max,
        l1.b_err_max,
        l1.b_spacings_max,
        l1.m_max,
        l1.m_special_max,
        l2.equal,
        l2.within,
        l2.differ,
        l2.not_judged,
    )
}

/// The measurement behind [`K`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KFit {
    /// The largest error found, in units of 2^-23 times `M`.
    pub largest: f64,
    /// Pairs in the measured range (`c` and `h c` large enough).
    pub pairs: u64,
    /// Of those, pairs one side or the other hits: the ones with an error
    /// to measure.
    pub compared: u64,
    /// Where the largest error is: ray, instance, triangle; and whether it
    /// is a difference of numbers (both hit) or of hit and miss.
    pub worst: [u32; 3],
    pub worst_is_numbers: bool,
    /// Over the pairs both sides hit: the largest error of `t` and the
    /// largest of a weight, each in the same unit. `largest` is the larger
    /// of the two unless a difference of hit and miss carries it.
    pub t_part: f64,
    pub b_part: f64,
}

impl KFit {
    pub const NONE: KFit = KFit {
        largest: 0.0,
        pairs: 0,
        compared: 0,
        worst: [0; 3],
        worst_is_numbers: false,
        t_part: 0.0,
        b_part: 0.0,
    };

    /// `k` for this measurement: `factor` (the rule's 2) times the largest
    /// error, rounded up to a power of two.
    pub fn k(&self, factor: f64) -> f64 {
        let x = factor * self.largest;
        let mut k = 1.0;
        while k < x {
            k *= 2.0;
        }
        while k / 2.0 >= x && k > f64::MIN_POSITIVE {
            k /= 2.0;
        }
        k
    }

    pub fn merge(&mut self, other: &KFit) {
        if other.largest > self.largest {
            (self.largest, self.worst, self.worst_is_numbers) =
                (other.largest, other.worst, other.worst_is_numbers);
        }
        self.pairs += other.pairs;
        self.compared += other.compared;
        self.t_part = self.t_part.max(other.t_part);
        self.b_part = self.b_part.max(other.b_part);
    }
}

/// The largest error of the emulated test's single-precision model
/// ([`Rule::emulated`], every triangle of every instance tested) against
/// the exact reference over `rays`, in units of 2^-23 times `M`: **the
/// tolerances as they stand, read backwards.** The error of a pair is the
/// smallest `x_tol` at which the emulated answer is inside both
/// tolerances: for a weight the error times `h c / 2`; for `t` the error
/// over `1 / (L c) + 2 D / (h c)`, which is `t_tol` for an `x_tol` of one
/// (with `D` the spread of the vertices' depths, as [`Rule`] takes it). So
/// `k` times this unit is exactly the tolerance the pair is judged with,
/// and an error of `e` here is a record at `e / k` of its tolerance.
///
/// Taken over the pairs with `c` at least `graze_cos` and `h c` at least
/// [`K_FIT_MIN_HC`] of `M`. Where one side hits and the other does not,
/// the error is at least as far as the exact numbers are from the edge or
/// from `t = 0`, and that is what is counted.
///
/// What is measured is a property of the pairs it is given: the shapes,
/// sizes and magnitudes of the scene classes' triangles and the way their
/// rays meet them. It is not a bound for other shapes.
///
/// `scene` must pass [`Scene::validate`] and every ray [`Ray::validate`].
pub fn measure_k(scene: &Scene, rays: &[Ray], graze_cos: f64) -> KFit {
    let mut rule = Rule::new(scene, Constants::proposed(1.0));
    let mut fit = KFit::NONE;
    for (r, ray) in rays.iter().enumerate() {
        let (ow, dw) = (ray.origin.map(f64::from), ray.dir.map(f64::from));
        for i in 0..rule.insts.len() {
            let (ro, rd) = rule.object_ray(i, ray);
            let origin = largest(ow).max(largest(ro));
            let l = norm(rd);
            let reach = l.max(norm(dw));
            let blas = scene.instances[i].blas as usize;
            for k in 0..rule.tris[blas].len() {
                let tri = &rule.tris[blas][k];
                if rule.insts[i].zero[k] || tri.area2 == 0.0 {
                    continue;
                }
                let s = dot(rd, tri.n);
                let cos = s.abs() / (l * tri.area2);
                if cos < graze_cos {
                    continue;
                }
                let a = sub(tri.p[0], ro);
                let (e1, e2) = (sub(tri.p[1], tri.p[0]), sub(tri.p[2], tri.p[0]));
                let (x1, x2) = (dot(a, cross(rd, e2)), dot(a, cross(e1, rd)));
                let t = dot(a, tri.n) / s;
                let least = ((s - x1 - x2) / s).min(x1 / s).min(x2 / s);
                let m = origin
                    .max(tri.mag)
                    .max(rule.insts[i].mag[k])
                    .max(t.abs() * reach);
                let hc = tri.h * cos;
                if hc < K_FIT_MIN_HC * m {
                    continue;
                }
                fit.pairs += 1;
                let got = rule.emulated(i, k, ray);
                // Both miss, and far from any doubt.
                if got.is_none() && (least < -1.0 / 1024.0 || t * l * cos < -m / 1024.0) {
                    continue;
                }
                // The two tolerances for an `x_tol` of one.
                let (lo, hi) = depths(&tri.p, ro, rd);
                let b_unit = 2.0 / hc;
                let t_unit = 1.0 / (l * cos) + b_unit * (t - lo).abs().max((hi - t).abs());
                let exact = rule.reference.exact_pair(i, k, ray);
                let hit = exact.inside && exact.t >= 0.0;
                let exact_least = exact.w[0].min(exact.w[1]).min(exact.w[2]);
                let (x, numbers) = match (got, hit) {
                    (None, false) => continue,
                    (Some([gt, gu, gv]), true) => {
                        let (gt, gu, gv) = (f64::from(gt), f64::from(gu), f64::from(gv));
                        let b = (1.0 - gu - gv - exact.w[0])
                            .abs()
                            .max((gu - exact.w[1]).abs())
                            .max((gv - exact.w[2]).abs());
                        let (xt, xb) = ((gt - exact.t).abs() / t_unit, b / b_unit);
                        fit.t_part = fit.t_part.max(xt / (SPACING * m));
                        fit.b_part = fit.b_part.max(xb / (SPACING * m));
                        (xt.max(xb), true)
                    }
                    // The emulated test hits what is exactly outside or
                    // behind: its weights are not negative and its t is
                    // not, so it is off by at least the exact margins.
                    (Some(_), false) => (
                        (-exact_least / b_unit).max(-exact.t / t_unit).max(0.0),
                        false,
                    ),
                    // It misses what is exactly inside and ahead: off by
                    // at least the smaller margin.
                    (None, true) => ((exact_least / b_unit).min(exact.t / t_unit), false),
                };
                fit.compared += 1;
                let err = x / (SPACING * m);
                if err > fit.largest {
                    fit.largest = err;
                    fit.worst = [r as u32, i as u32, k as u32];
                    fit.worst_is_numbers = numbers;
                }
            }
        }
    }
    fit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{BottomLevel, BuildFlags, Geometry, Indices, Instance, Transform};

    /// A unit square in the plane z = `z`: (0,0), (0,1), (1,0) and (1,0),
    /// (0,1), (1,1), front faces toward -z, sharing the diagonal.
    fn square(z: f32, opaque: bool) -> Geometry {
        Geometry {
            vertices: vec![[0.0, 0.0, z], [0.0, 1.0, z], [1.0, 0.0, z], [1.0, 1.0, z]],
            indices: Indices::U16(vec![0, 1, 2, 2, 1, 3]),
            transform: None,
            opaque,
        }
    }

    /// One triangle of a square: the first (`0`) or the second.
    fn half(z: f32, second: bool) -> Geometry {
        let s = square(z, true);
        Geometry {
            indices: Indices::U16(if second { vec![2, 1, 3] } else { vec![0, 1, 2] }),
            ..s
        }
    }

    fn scene(bottoms: Vec<Vec<Geometry>>, instances: &[(Transform, u32, u32)]) -> Scene {
        Scene {
            bottoms: bottoms
                .into_iter()
                .map(|geometries| BottomLevel {
                    geometries,
                    flags: BuildFlags::default(),
                })
                .collect(),
            instances: instances
                .iter()
                .map(|&(transform, blas, id)| Instance {
                    transform,
                    id,
                    mask: 0xFF,
                    flags: 0,
                    blas,
                })
                .collect(),
            flags: BuildFlags::default(),
        }
    }

    fn one(geometries: Vec<Geometry>) -> Scene {
        scene(vec![geometries], &[(Transform::IDENTITY, 0, 7)])
    }

    /// Along +z through (x, y); z = 2 is at t = 3.
    fn toward(x: f32, y: f32) -> Ray {
        Ray::new([x, y, -4.0], [0.0, 0.0, 2.0], 0.0, 100.0)
    }

    fn read(scene: &Scene, ray: &Ray) -> RayRule {
        scene.validate().unwrap();
        ray.validate().unwrap();
        let mut rule = Rule::new(scene, Constants::PROPOSED);
        let r = rule.trace(ray);
        assert_eq!(rule.inconsistent, 0);
        r
    }

    #[test]
    fn the_tolerance_is_the_rule_s_formula() {
        let s = one(vec![square(2.0, true)]);
        let r = read(&s, &toward(0.25, 0.5));
        assert!(!r.ambiguous && r.kinds == 0 && !r.miss_admissible);
        let e = r.admissible[0];
        // M is the distance to the hit, 3 direction lengths of 2, which is
        // more than any coordinate; the triangle's smallest altitude is
        // half its diagonal; the ray is along the normal.
        let x_tol = K * SPACING * 6.0;
        assert_eq!(e.x_tol, x_tol);
        let b_tol = 2.0 * x_tol / 0.5f64.sqrt();
        assert!((e.b_tol - b_tol).abs() < 1e-15);
        // The triangle lies across the ray: its vertices are all at the
        // depth of the hit, and t has the plane's tolerance alone.
        assert_eq!(e.t_tol, x_tol / 2.0);
        assert_eq!((e.t, e.w), (3.0, [0.25, 0.5, 0.25]));
        assert_eq!((e.cos, e.near, e.special), (1.0, false, 0));
        assert_eq!(r.primary, e.hit);
        // A slanted ray: the cosine divides both tolerances.
        let slant = Ray::new([0.25 - 6.0, 0.5, -4.0], [3.0, 0.0, 3.0], 0.0, 100.0);
        let e = read(&s, &slant).admissible[0];
        assert!((e.cos - 0.5f64.sqrt()).abs() < 1e-12);
        // M is now the distance to the hit, 2 direction lengths of 18^(1/2).
        let x_tol = K * SPACING * 2.0 * 18f64.sqrt();
        assert!((e.x_tol - x_tol).abs() < 1e-12);
        // The vertices are now up to 1/4 of a direction length before the
        // hit and 1/12 behind it (the feet of (0, 0) and (1, 0) on the
        // ray; by the largest axis they are level): the weights' tolerance
        // times that is added.
        let plane = x_tol / 3.0;
        assert!(
            (e.t_tol - (plane + e.b_tol * 0.25)).abs() < 1e-12,
            "{} {plane}",
            e.t_tol
        );
    }

    #[test]
    fn a_shared_edge_is_closed_and_a_boundary_is_not() {
        let s = one(vec![square(2.0, true), square(5.0, true)]);
        // On the diagonal the two triangles share: either, never a miss,
        // and never the square behind.
        let r = read(&s, &toward(0.5, 0.5));
        assert!(r.ambiguous && !r.miss_admissible);
        assert_eq!((r.kinds, r.special), (kind::SHARED_EDGE, 0));
        let ids: Vec<[u32; 4]> = r.admissible.iter().map(|e| e.hit.ids()).collect();
        assert_eq!(ids, [[0, 7, 0, 0], [0, 7, 0, 1]]);
        assert_eq!(
            judge(&r, &r.admissible[1].hit).outcome,
            RuleOutcome::Admissible
        );
        assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::LostHit);
        let behind = Hit {
            t: 4.5,
            geometry_index: 1,
            ..r.primary
        };
        assert_eq!(judge(&r, &behind).outcome, RuleOutcome::SurfaceFlip);
        // On the outer edge: the triangle, or what is behind it.
        let r = read(&s, &toward(0.0, 0.5));
        assert_eq!(r.kinds, kind::BOUNDARY_EDGE);
        assert!(
            r.admissible.len() == 2 && r.miss_admissible,
            "both squares end there"
        );
        let r = read(&one(vec![square(2.0, true)]), &toward(0.0, 0.5));
        assert!(r.miss_admissible && r.admissible.len() == 1);
        assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::Admissible);
        // A vertex inside a fan closes too: four squares around (1, 1).
        let mut fan = Vec::new();
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            let mut g = square(2.0, true);
            for v in &mut g.vertices {
                (v[0], v[1]) = (v[0] + x, v[1] + y);
            }
            fan.push(g);
        }
        let r = read(&one(fan), &toward(1.0, 1.0));
        assert_eq!((r.kinds, r.miss_admissible), (kind::SHARED_EDGE, false));
        assert_eq!(r.admissible.len(), 6);
        // Away from every edge nothing is flagged.
        let r = read(&s, &toward(0.25, 0.5));
        assert!(!r.ambiguous && r.admissible.len() == 1);
    }

    #[test]
    fn seams_are_closed_by_one_object_space_ray_and_open_otherwise() {
        let diagonal = toward(0.5, 0.5);
        let moved = Transform::translation([0.0, 0.0, 0.0]);
        // The two halves in two bottom levels under one transform.
        let s = scene(
            vec![vec![half(2.0, false)], vec![half(2.0, true)]],
            &[(moved, 0, 7), (moved, 1, 7)],
        );
        let r = read(&s, &diagonal);
        assert_eq!((r.kinds, r.miss_admissible), (kind::SEAM_CLOSED, false));
        assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::LostHit);
        // The second half lowered by 4 in its vertices and raised by its
        // instance: the same points, another transform.
        let s = scene(
            vec![vec![half(2.0, false)], vec![half(-2.0, true)]],
            &[
                (Transform::IDENTITY, 0, 7),
                (Transform::translation([0.0, 0.0, 4.0]), 1, 7),
            ],
        );
        let r = read(&s, &diagonal);
        assert_eq!((r.kinds, r.miss_admissible), (kind::SEAM_OPEN, true));
        assert_eq!(r.admissible.len(), 2);
        assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::Admissible);
        // The same by a geometry transform inside one bottom level.
        let mut lowered = half(-2.0, true);
        lowered.transform = Some(Transform::translation([0.0, 0.0, 4.0]));
        let r = read(&one(vec![half(2.0, false), lowered]), &diagonal);
        assert_eq!((r.kinds, r.miss_admissible), (kind::SEAM_OPEN, true));
        // Two geometries without a transform in one bottom level are a
        // shared edge between two surfaces.
        let r = read(&one(vec![half(2.0, false), half(2.0, true)]), &diagonal);
        assert_eq!((r.kinds, r.miss_admissible), (kind::SHARED_EDGE, false));
        let other = r.admissible[1].hit;
        let j = judge(&r, &other);
        assert_eq!(
            (j.outcome, j.other_surface),
            (RuleOutcome::Admissible, true)
        );
        // Opposite windings: covered, and not like-facing. A miss is
        // admissible.
        let mut turned = half(2.0, true);
        turned.indices = Indices::U16(vec![2, 3, 1]);
        let r = read(&one(vec![half(2.0, false), turned]), &diagonal);
        assert_eq!((r.kinds, r.miss_admissible), (kind::BOUNDARY_EDGE, true));
    }

    #[test]
    fn extents_ties_and_the_yes_or_no_workload() {
        let s = one(vec![square(2.0, true), square(2.5, true)]);
        let mut ray = toward(0.25, 0.5);
        ray.tmax = 3.0;
        let r = read(&s, &ray);
        assert_eq!((r.kinds, r.miss_admissible), (kind::EXTENT, true));
        assert_eq!(r.primary, Hit::MISS);
        // A hit at exactly t = 0 is at the extent's boundary.
        let on = Ray::new([0.25, 0.5, 2.0], [0.0, 0.0, 2.0], 0.0, 100.0);
        let r = read(&s, &on);
        assert_eq!(r.kinds, kind::EXTENT);
        assert!(!r.miss_admissible && r.admissible.len() == 2);
        // Two squares a spacing apart are a tie; a quarter apart are not.
        let close = one(vec![square(2.0, true), square(2.0f32.next_up(), true)]);
        let r = read(&close, &toward(0.25, 0.5));
        assert_eq!((r.kinds, r.admissible.len()), (kind::TIE, 2));
        let r = read(&s, &toward(0.25, 0.5));
        assert!(!r.ambiguous);
        // Any hit: with a hit certain the answer is yes, whichever square.
        let mut any = toward(0.25, 0.5);
        any.flags = ray_flag::ACCEPT_FIRST_HIT_AND_END_SEARCH;
        let r = read(&s, &any);
        assert_eq!((r.workload, r.ambiguous), (Workload::AnyHit, false));
        assert_eq!(r.admissible.len(), 2);
        assert_eq!(judge(&r, &r.admissible[1].hit).outcome, RuleOutcome::Agree);
        assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::LostHit);
    }

    #[test]
    fn the_filter_is_read_over_its_tolerance_box() {
        let s = one(vec![square(2.0, false), square(5.0, true)]);
        // u = 0.3, v = 0.3: accepted, far from a cell's border.
        let r = read(&s, &toward(0.3, 0.3));
        assert!(!r.ambiguous && r.admissible[0].filtered);
        // u = 0.3, v = 0.2: rejected; the square behind is the answer, and
        // a record that names the rejected one is a filter flip.
        let r = read(&s, &toward(0.2, 0.3));
        assert!(!r.ambiguous);
        assert_eq!((r.primary.geometry_index, r.rejected.len()), (1, 1));
        let through = Hit {
            t: 3.0,
            geometry_index: 0,
            ..r.primary
        };
        assert_eq!(judge(&r, &through).outcome, RuleOutcome::FilterFlip);
        // u = 0.25 exactly: 8u is on a border. Either decision.
        let r = read(&s, &toward(0.3, 0.25));
        assert_eq!((r.kinds, r.admissible.len()), (kind::FILTER, 2));
        assert!(!r.miss_admissible);
        // Passing an accepted candidate over is a filter flip too.
        let alone = one(vec![square(2.0, false)]);
        let r = read(&alone, &toward(0.3, 0.3));
        assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::FilterFlip);
    }

    #[test]
    fn grazing_sliver_and_degenerate_pairs_are_near_and_placed_by_distance() {
        // A ray at a rise of 1 in 128 over the square: grazing.
        let s = one(vec![square(2.0, true)]);
        let graze = Ray::new([-63.75, 0.5, 1.5], [64.0, 0.0, 0.5], 0.0, 100.0);
        let r = read(&s, &graze);
        assert_eq!(
            (r.ambiguous, r.kinds, r.special),
            (true, 0, special::GRAZING)
        );
        let e = r.admissible[0];
        assert!(e.near && !e.facing_known && e.b_tol.is_infinite() && r.miss_admissible);
        // Any t over the triangle's depth along the ray, either face, any
        // weights.
        let mut got = e.hit;
        (got.t, got.u, got.kind) = (1.005, 0.9, hit_kind::BACK);
        assert_eq!(judge(&r, &got).outcome, RuleOutcome::Admissible);
        got.t = 1.1;
        assert_eq!(judge(&r, &got).outcome, RuleOutcome::OutOfTol);
        // The same ray a little higher passes the square by more than the
        // tolerance: out, and the ray is unambiguous.
        let above = Ray::new([-63.75, 0.5, 1.25], [64.0, 0.0, 0.5], 0.0, 100.0);
        let r = read(&s, &above);
        assert!(!r.ambiguous && r.admissible.is_empty() && r.miss_admissible);
        // A grazing pair behind a certain hit changes nothing.
        let mut wall = square(2.0, true);
        wall.vertices = vec![
            [0.25, 0.0, 0.0],
            [0.25, 1.0, 0.0],
            [0.25, 0.0, 1.9],
            [0.25, 1.0, 1.9],
        ];
        let r = read(&one(vec![square(2.0, true), wall]), &graze);
        assert!(r.ambiguous, "the wall ends within tolerance of the ray");
        // A sliver: a base of 8 and a height of 2^-16.
        let sliver = Geometry {
            vertices: vec![[0.0, 0.0, 2.0], [8.0, 0.0, 2.0], [4.0, 1.0 / 65536.0, 2.0]],
            indices: Indices::None,
            transform: None,
            opaque: true,
        };
        let r = read(&one(vec![sliver]), &toward(4.0, 1.0 / 131_072.0));
        assert_eq!((r.special, r.miss_admissible), (special::SLIVER, true));
        assert!(r.admissible[0].b_tol.is_infinite());
        // No area: a hit on it is admissible where the ray passes it, and
        // so is what is behind.
        let line = Geometry {
            vertices: vec![[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [2.0, 0.0, 2.0]],
            indices: Indices::None,
            transform: None,
            opaque: true,
        };
        let s = one(vec![line, square(5.0, true)]);
        let r = read(&s, &toward(0.5, 0.0));
        assert_eq!(r.kinds & kind::DEGENERATE, kind::DEGENERATE);
        assert_eq!(r.primary.geometry_index, 1);
        let on_line = r.admissible.iter().find(|e| e.special != 0).unwrap();
        assert_eq!(judge(&r, &on_line.hit).outcome, RuleOutcome::Admissible);
        let r = read(&s, &toward(0.5, 0.25));
        assert!(!r.ambiguous && r.admissible.len() == 1);
    }

    #[test]
    fn records_are_judged_by_the_rule() {
        let s = one(vec![square(2.0, true)]);
        let r = read(&s, &toward(0.25, 0.5));
        let e = r.admissible[0];
        assert_eq!(judge(&r, &r.primary).outcome, RuleOutcome::Agree);
        // A procedural hit names the same triangle and carries no face.
        let mut got = r.primary;
        got.kind = hit_kind::PROCEDURAL;
        let j = judge(&r, &got);
        assert_eq!((j.outcome, j.procedural), (RuleOutcome::Agree, true));
        got.kind = hit_kind::BACK;
        assert_eq!(judge(&r, &got).outcome, RuleOutcome::FacingFlip);
        // Numbers: half a tolerance passes, two do not; the third weight
        // counts.
        got = r.primary;
        got.t += (0.5 * e.t_tol) as f32;
        let j = judge(&r, &got);
        assert_eq!(j.outcome, RuleOutcome::Agree);
        // Half a tolerance is K / 2 spacings of the largest magnitude.
        assert!(j.t_err > 0.4 && j.t_err < 0.6 && j.t_spacings == j.t_err * K);
        got.t = r.primary.t + (2.0 * e.t_tol) as f32;
        assert_eq!(judge(&r, &got).outcome, RuleOutcome::OutOfTol);
        got = r.primary;
        (got.u, got.v) = (
            got.u + (0.75 * e.b_tol) as f32,
            got.v + (0.75 * e.b_tol) as f32,
        );
        assert_eq!(judge(&r, &got).outcome, RuleOutcome::OutOfTol);
        // Another triangle, a miss, a hit where there is nothing.
        got = r.primary;
        got.primitive_index = 1;
        assert_eq!(judge(&r, &got).outcome, RuleOutcome::IdFlip);
        got.instance_id = 9;
        assert_eq!(judge(&r, &got).outcome, RuleOutcome::SurfaceFlip);
        assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::LostHit);
        let beside = read(&s, &toward(1.25, 0.5));
        assert_eq!(judge(&beside, &Hit::MISS).outcome, RuleOutcome::Agree);
        assert_eq!(judge(&beside, &r.primary).outcome, RuleOutcome::GainedHit);
        // A refusal is counted by its reasons and is neither.
        let j = judge(&r, &Hit::refused(refusal::RANGE));
        assert_eq!(
            (j.outcome, j.reasons),
            (RuleOutcome::Refused, refusal::RANGE)
        );
        assert!(!j.outcome.is_failure());
        assert_eq!(
            judge(&beside, &Hit::refused(1)).outcome,
            RuleOutcome::Refused
        );
        for bad in [
            Hit::refused(0),
            Hit {
                kind: 4,
                ..r.primary
            },
            Hit {
                t: f32::NAN,
                ..r.primary
            },
            Hit {
                primitive_index: 1,
                ..Hit::MISS
            },
        ] {
            assert_eq!(judge(&r, &bad).outcome, RuleOutcome::Malformed);
        }
    }

    #[test]
    fn the_verdict_follows_the_bounds_and_names_what_it_is_sensitive_to() {
        let c = Constants::PROPOSED;
        let l2 = Level2::default();
        let mut l1 = Level1 {
            rays: 4096,
            unambiguous: 4092,
            ambiguous: 4,
            agree_hits: 2048,
            ..Level1::default()
        };
        assert_eq!(verdict(ClassKind::Random, &l1, &l2, &c), RuleVerdict::Pass);
        l1.ambiguous = 5;
        assert_eq!(
            verdict(ClassKind::Random, &l1, &l2, &c),
            RuleVerdict::NotDecidable
        );
        assert_eq!(
            verdict(ClassKind::Directed, &l1, &l2, &c),
            RuleVerdict::Admissible
        );
        // Sliver and grazing rays are outside the bound's count, up to one
        // ray in 16.
        (l1.ambiguous, l1.special) = (3, 256);
        assert_eq!(verdict(ClassKind::Random, &l1, &l2, &c), RuleVerdict::Pass);
        l1.special = 257;
        assert_eq!(
            verdict(ClassKind::Random, &l1, &l2, &c),
            RuleVerdict::ValidatesNothing
        );
        (l1.special, l1.refused) = (0, 257);
        assert_eq!(
            verdict(ClassKind::Random, &l1, &l2, &c),
            RuleVerdict::ValidatesNothing
        );
        l1.refused = 3;
        assert_eq!(verdict(ClassKind::Random, &l1, &l2, &c), RuleVerdict::Pass);
        // The two shares are one: rays with a sliver or grazing pair and
        // refused rays together are the rays not held to numbers. Each
        // under one in 16 and together over it validates nothing; a ray
        // that is both is one ray.
        (l1.special, l1.refused) = (200, 100);
        assert_eq!(l1.set_apart(), 300);
        assert_eq!(
            verdict(ClassKind::Random, &l1, &l2, &c),
            RuleVerdict::ValidatesNothing
        );
        l1.refused_special = 100;
        assert_eq!(l1.set_apart(), 200);
        assert_eq!(verdict(ClassKind::Random, &l1, &l2, &c), RuleVerdict::Pass);
        (l1.special, l1.refused, l1.refused_special) = (0, 3, 0);
        l1.id_flips = 1;
        assert_eq!(verdict(ClassKind::Random, &l1, &l2, &c), RuleVerdict::Fail);
        assert_eq!(
            verdict(ClassKind::Directed, &l1, &l2, &c),
            RuleVerdict::Fail
        );
        l1.id_flips = 0;
        let differ = Level2 {
            differ: 1,
            ..Level2::default()
        };
        assert_eq!(
            verdict(ClassKind::Random, &l1, &differ, &c),
            RuleVerdict::Fail
        );
        // Eighteen variants, each of one constant.
        let v = c.variants();
        assert_eq!(v.len(), 18);
        assert!(v.iter().all(|(_, x)| *x != c));
        assert_eq!(v[0].1.k, K / 2.0);
        assert_eq!(v[6].1.ambiguous_one_in, 2048);
        assert_eq!(v[11].1.pixel_step, 4);
        assert_eq!((v[16].1.hit_one_in, v[17].1.hit_one_in), (8, 2));
    }

    #[test]
    fn a_class_that_judges_nothing_validates_nothing() {
        let c = Constants::PROPOSED;
        let l2 = Level2::default();
        let random = |l1: &Level1| verdict(ClassKind::Random, l1, &l2, &c);
        // No ray at all; three rays that agree.
        assert_eq!(random(&Level1::default()), RuleVerdict::ValidatesNothing);
        let few = Level1 {
            rays: 3,
            unambiguous: 3,
            agree: 3,
            agree_hits: 3,
            ..Level1::default()
        };
        assert_eq!(random(&few), RuleVerdict::ValidatesNothing);
        // One in 1,024 is counted on 1,024 rays, sliver and grazing rays
        // not among them.
        let mut l1 = Level1 {
            rays: 1024,
            unambiguous: 1024,
            agree: 1024,
            agree_hits: 1024,
            ..Level1::default()
        };
        assert_eq!(random(&l1), RuleVerdict::Pass);
        l1.rays = 1023;
        assert_eq!(random(&l1), RuleVerdict::ValidatesNothing);
        (l1.rays, l1.special) = (1087, 64);
        assert_eq!(random(&l1), RuleVerdict::ValidatesNothing);
        l1.rays = 1088;
        assert_eq!(random(&l1), RuleVerdict::Pass);
        // With fewer rays the class is still over the bound when it is.
        let over = Level1 {
            rays: 3,
            unambiguous: 2,
            ambiguous: 1,
            ..Level1::default()
        };
        assert_eq!(random(&over), RuleVerdict::NotDecidable);
        // Rays that meet nothing: one ray in four has to be an unambiguous
        // hit that agreed.
        let mut l1 = Level1 {
            rays: 1600,
            unambiguous: 1600,
            agree: 1600,
            ..Level1::default()
        };
        assert_eq!(random(&l1), RuleVerdict::ValidatesNothing);
        l1.agree_hits = 399;
        assert_eq!(random(&l1), RuleVerdict::ValidatesNothing);
        l1.agree_hits = 400;
        assert_eq!(random(&l1), RuleVerdict::Pass);
        let halved = Constants { hit_one_in: 8, ..c };
        l1.agree_hits = 200;
        assert_eq!(
            verdict(ClassKind::Random, &l1, &l2, &halved),
            RuleVerdict::Pass
        );
        // A directed class is judged by admissibility whatever it holds.
        assert_eq!(
            verdict(ClassKind::Directed, &Level1::default(), &l2, &c),
            RuleVerdict::Admissible
        );
        // The same through the lists: nothing, three rays, and 1,600 rays
        // aimed past the only triangle and answered with misses.
        let s = one(vec![square(2.0, true)]);
        let nothing = assess(ClassKind::Random, &s, &[], &[], &c).unwrap();
        assert_eq!(
            (nothing.verdict, nothing.sensitive.len()),
            (RuleVerdict::ValidatesNothing, 0)
        );
        let rays: Vec<Ray> = (0..3)
            .map(|k| toward(0.25, 0.3 + 0.125 * k as f32))
            .collect();
        let hits: Vec<Hit> = rays.iter().map(|r| read(&s, r).primary).collect();
        let three = assess(ClassKind::Random, &s, &rays, &hits, &c).unwrap();
        assert_eq!(three.evaluation.level1.agree_hits, 3);
        assert_eq!(three.verdict, RuleVerdict::ValidatesNothing);
        let away: Vec<Ray> = (0..1600).map(|k| toward(5.0 + k as f32, 5.0)).collect();
        let past = assess(ClassKind::Random, &s, &away, &vec![Hit::MISS; 1600], &c).unwrap();
        assert_eq!(
            (
                past.evaluation.level1.agree,
                past.evaluation.level1.agree_hits
            ),
            (1600, 0)
        );
        assert_eq!(past.verdict, RuleVerdict::ValidatesNothing);
    }

    #[test]
    fn rays_and_scenes_that_are_not_valid_are_refused_not_judged() {
        let s = one(vec![square(2.0, true)]);
        let c = Constants::PROPOSED;
        let good = toward(0.25, 0.5);
        for (n, (ray, what)) in crate::generate::illegal_rays().into_iter().enumerate() {
            // Alone, and among rays that are legal: named by its place.
            for (rays, at) in [(vec![ray], 0), (vec![good, good, ray, good], 2)] {
                let hits = vec![Hit::MISS; rays.len()];
                let e = evaluate(&s, &rays, &hits, &c).unwrap_err();
                assert!(
                    e.starts_with(&format!("ray {at} is not one to judge: ")),
                    "{n} {what}: {e}"
                );
                assert_eq!(
                    assess(ClassKind::Random, &s, &rays, &hits, &c).unwrap_err(),
                    e
                );
            }
        }
        assert_eq!(crate::generate::illegal_rays().len(), 11);
        // A scene with a vertex that is no number, or an instance of a
        // bottom level that is not there.
        let mut nan = s.clone();
        nan.bottoms[0].geometries[0].vertices[1][2] = f32::NAN;
        let mut dangling = s.clone();
        dangling.instances[0].blas = 3;
        for bad in [nan, dangling] {
            let e = evaluate(&bad, &[good], &[Hit::MISS], &c).unwrap_err();
            assert!(e.starts_with("a scene the rule does not read: "), "{e}");
        }
        assert!(evaluate(&s, &[good], &[Hit::MISS, Hit::MISS], &c).is_err());
    }

    #[test]
    fn the_reach_is_that_of_the_rays_whose_numbers_were_compared() {
        // A triangle near the origin and small ones at 2^20, where every
        // pair is a sliver pair: the far rays are judged by admissibility,
        // their weights not compared, and add nothing to how far the class
        // reaches.
        let far = 1_048_576.0f32;
        let mut vertices = vec![[0.0, 0.0, 2.0], [0.0, 4.0, 2.0], [4.0, 0.0, 2.0]];
        for k in 0..8 {
            let x = far + k as f32;
            vertices.extend([[x, 0.0, 2.0], [x, 0.5, 2.0], [x + 0.5, 0.0, 2.0]]);
        }
        let s = one(vec![Geometry {
            vertices,
            indices: Indices::None,
            transform: None,
            opaque: true,
        }]);
        let mut rays = Vec::new();
        for n in 0..1100 {
            let (i, j) = ((n % 33) as f32, (n / 33) as f32);
            rays.push(toward(0.25 + i / 32.0, 0.25 + j / 32.0));
        }
        for k in 0..60 {
            let x = far + (k % 8) as f32 + 0.125;
            rays.push(Ray::new([x, 0.125, 1.0], [0.0, 0.0, 2.0], 0.0, 100.0));
        }
        let hits: Vec<Hit> = rays.iter().map(|r| read(&s, r).primary).collect();
        let a = assess(ClassKind::Random, &s, &rays, &hits, &Constants::PROPOSED).unwrap();
        let l = &a.evaluation.level1;
        assert_eq!(a.verdict, RuleVerdict::Pass);
        assert_eq!((l.unambiguous, l.special, l.agree_hits), (1100, 60, 1100));
        // The near rays reach 6 (t times the direction's length); the far
        // ones are at 2^20 and are reported apart.
        assert_eq!((l.m_max, l.m_special_max), (6.0, f64::from(far) + 7.5));
        // Other weights on the far hits are admissible, as the rule says of
        // a sliver pair: which is why they are no reach.
        let mut other = hits.clone();
        for h in other.iter_mut().skip(1100) {
            assert!(!h.is_miss());
            (h.u, h.v) = (0.9, 0.05);
        }
        let b = assess(ClassKind::Random, &s, &rays, &other, &Constants::PROPOSED).unwrap();
        assert_eq!(b.verdict, RuleVerdict::Pass);
        assert_eq!(b.evaluation.level1.m_max, 6.0);
        let text = to_text(
            "far",
            b.verdict,
            &b.evaluation.level1,
            &b.evaluation.level2,
            &[],
        );
        assert!(
            text.contains("largest magnitude among the unambiguous hits 6.000e0, among the rays with a sliver or grazing pair 1.049e6"),
            "{text}"
        );
    }

    #[test]
    fn the_face_of_a_triangle_thinner_than_the_tolerance_is_not_a_fact() {
        // A base of 8 and a height of 2^-14, 2^-15 and 2^-16, met head on
        // at M = 8: weight tolerances of 1/4, 1/2 and 1.
        let sliver = |height: f32| {
            one(vec![Geometry {
                vertices: vec![[0.0, 0.0, 2.0], [8.0, 0.0, 2.0], [4.0, height, 2.0]],
                indices: Indices::None,
                transform: None,
                opaque: true,
            }])
        };
        for (height, known) in [
            (1.0 / 16384.0, true),
            (1.0 / 32768.0, true),
            (1.0 / 65536.0, false),
        ] {
            let s = sliver(height);
            let ray = toward(4.0, height / 2.0);
            let r = read(&s, &ray);
            assert_eq!(r.special, special::SLIVER, "{height}");
            let e = r.admissible[0];
            assert_eq!(e.facing_known, known, "{height}");
            // The triangle is wound counter-clockwise from the ray's
            // origin: a back face.
            assert_eq!(e.hit.kind, hit_kind::BACK);
            let turned = Hit {
                kind: hit_kind::FRONT,
                ..e.hit
            };
            assert_eq!(
                judge(&r, &turned).outcome,
                if known {
                    RuleOutcome::FacingFlip
                } else {
                    RuleOutcome::Admissible
                },
                "{height}"
            );
            assert_eq!(
                shade::judge_consumed(&s, &ray, &r, &turned),
                if known {
                    ConsumedOutcome::Differ
                } else {
                    ConsumedOutcome::Within
                },
                "{height}"
            );
            // A cull of back faces takes it away only where its face is a
            // fact; a cull of front faces never does.
            let mut culled = ray;
            culled.flags = ray_flag::CULL_BACK_FACING_TRIANGLES;
            assert_eq!(read(&s, &culled).admissible.len(), usize::from(!known));
            culled.flags = ray_flag::CULL_FRONT_FACING_TRIANGLES;
            assert_eq!(read(&s, &culled).admissible.len(), 1);
        }
        assert_eq!(FACE_B_TOL, 0.5);
    }

    /// Two triangles of one geometry wound alike that share the edge
    /// (0, 0)-(0, 1) in the plane z = 0: a small one to x = -1 (primitive
    /// 0) and a large one to x = 1,000 (primitive 1), whose tolerance is
    /// 250 times the small one's.
    fn unequal(second: [u16; 3]) -> Geometry {
        Geometry {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [-1.0, 0.5, 0.0],
                [1000.0, 0.5, 0.0],
            ],
            indices: Indices::U16([[0, 2, 1], second].concat()),
            transform: None,
            opaque: true,
        }
    }

    /// Straight down on the plane z = 0 at (x, 1/2).
    fn down(x: f32) -> Ray {
        Ray::new([x, 0.5, 4.0], [0.0, 0.0, -2.0], 0.0, 100.0)
    }

    #[test]
    fn a_shared_edge_is_closed_where_the_neighbour_is_out_by_its_own_tolerance() {
        let s = one(vec![unequal([0, 1, 3])]);
        // Inside the large triangle and within its weight tolerance of the
        // shared edge (1.9 units here), while the small one's tolerance
        // puts the ray outside it: the large one is the only answer, and a
        // miss is a hole.
        for x in [1.0e-5f32, 1.0e-3, 0.1, 0.5, 1.0, 1.5] {
            let r = read(&s, &down(x));
            assert!(r.ambiguous && !r.miss_admissible, "{x}");
            assert_eq!((r.kinds, r.special), (kind::SHARED_EDGE, 0), "{x}");
            assert_eq!(r.admissible.len(), 1, "{x}");
            assert_eq!(r.primary.primitive_index, 1, "{x}");
            assert_eq!(judge(&r, &r.primary).outcome, RuleOutcome::Admissible);
            assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::LostHit, "{x}");
            // The small triangle is not an answer there.
            let other = Hit {
                primitive_index: 0,
                ..r.primary
            };
            assert_eq!(judge(&r, &other).outcome, RuleOutcome::IdFlip, "{x}");
        }
        // Within both tolerances: either. Beyond the large one's: in.
        let r = read(&s, &down(1.0e-7));
        assert_eq!((r.kinds, r.admissible.len()), (kind::SHARED_EDGE, 2));
        assert!(!r.miss_admissible);
        let r = read(&s, &down(2.5));
        assert!(!r.ambiguous && !r.miss_admissible);
        // On the small one's side and beyond its far corner the ray meets
        // neither, and is still within the large one's tolerance of the
        // edge: a boundary there, a hit on the large one or a miss.
        let r = read(&s, &down(-1.5));
        assert_eq!((r.kinds, r.miss_admissible), (kind::BOUNDARY_EDGE, true));
        assert_eq!((r.admissible.len(), r.primary), (1, Hit::MISS));
        // The neighbour wound the other way shows the other face: the edge
        // is a silhouette, and a miss beside it is admissible.
        let r = read(&one(vec![unequal([0, 3, 1])]), &down(0.5));
        assert_eq!((r.kinds, r.miss_admissible), (kind::BOUNDARY_EDGE, true));
        // The two in two bottom levels under one transform: a closed seam,
        // closed the same way; with the small one's instance masked away
        // for the ray, an edge with one triangle.
        let half = |tri: [u16; 3]| Geometry {
            indices: Indices::U16(tri.to_vec()),
            ..unequal([0, 1, 3])
        };
        let mut seam = scene(
            vec![vec![half([0, 2, 1])], vec![half([0, 1, 3])]],
            &[(Transform::IDENTITY, 0, 7), (Transform::IDENTITY, 1, 7)],
        );
        let r = read(&seam, &down(0.5));
        assert_eq!((r.kinds, r.miss_admissible), (kind::SEAM_CLOSED, false));
        seam.instances[0].mask = 0x01;
        let mut ray = down(0.5);
        ray.mask = 0xFE;
        let r = read(&seam, &ray);
        assert_eq!((r.kinds, r.miss_admissible), (kind::BOUNDARY_EDGE, true));
        // Under different transforms the seam is open, as it is when both
        // sides are candidates.
        let apart = scene(
            vec![
                vec![half([0, 2, 1])],
                vec![Geometry {
                    transform: Some(Transform::translation([0.0, 0.0, 0.0])),
                    ..half([0, 1, 3])
                }],
            ],
            &[(Transform::IDENTITY, 0, 7), (Transform::IDENTITY, 1, 7)],
        );
        let r = read(&apart, &down(0.5));
        assert_eq!((r.kinds, r.miss_admissible), (kind::SEAM_OPEN, true));
    }

    #[test]
    fn a_fan_is_closed_where_some_of_its_members_are_out_by_their_own_tolerance() {
        // Four triangles around the origin in the plane z = 0, wound
        // alike: two small ones above the x axis and two that reach to
        // y = -1,000 below it.
        let fan = |indices: Vec<u16>| {
            one(vec![Geometry {
                vertices: vec![
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [-1.0, 0.0, 0.0],
                    [0.0, -1000.0, 0.0],
                ],
                indices: Indices::U16(indices),
                transform: None,
                opaque: true,
            }])
        };
        let at = |x: f32, y: f32| Ray::new([x, y, 4.0], [0.0, 0.0, -2.0], 0.0, 100.0);
        let whole = fan(vec![0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 1]);
        // Inside a large one, half a unit from the middle vertex: within
        // the large ones' tolerance of it and far outside the small ones'.
        let r = read(&whole, &at(0.001, -0.5));
        assert_eq!((r.kinds, r.miss_admissible), (kind::SHARED_EDGE, false));
        let ids: Vec<u32> = r.admissible.iter().map(|e| e.hit.primitive_index).collect();
        assert_eq!((r.primary.primitive_index, ids), (3, vec![3, 2]));
        assert_eq!(judge(&r, &Hit::MISS).outcome, RuleOutcome::LostHit);
        // Above the small ones' far edge the ray meets nothing, and is
        // still within the large ones' tolerance of the vertex: a hit on
        // one of them or a miss.
        let r = read(&whole, &at(0.001, 1.5));
        assert_eq!((r.kinds, r.miss_admissible), (kind::BOUNDARY_EDGE, true));
        assert_eq!((r.admissible.len(), r.primary), (2, Hit::MISS));
        // With one small member missing the fan is open: a boundary.
        let open = fan(vec![0, 1, 2, 0, 3, 4, 0, 4, 1]);
        let r = read(&open, &at(0.001, -0.5));
        assert_eq!((r.kinds, r.miss_admissible), (kind::BOUNDARY_EDGE, true));
    }

    #[test]
    fn an_assessment_recomputes_with_each_constant() {
        // 2,048 rays into a square, of which two are on its outer edge:
        // inside the bound, and over it when the bound is halved.
        let s = one(vec![square(2.0, true)]);
        let mut rays = Vec::new();
        for n in 0..2046 {
            let (i, j) = ((n % 31) as f32, (n / 31 % 31) as f32);
            rays.push(toward(0.03125 + i / 64.0, 0.01 + j / 64.0));
        }
        rays.push(toward(0.0, 0.25));
        rays.push(toward(0.25, 0.0));
        let mut reference = Rule::new(&s, Constants::PROPOSED);
        let hits: Vec<Hit> = reference
            .trace_all(&rays)
            .iter()
            .map(|r| r.primary)
            .collect();
        let a = assess(ClassKind::Random, &s, &rays, &hits, &Constants::PROPOSED).unwrap();
        assert_eq!(a.verdict, RuleVerdict::Pass);
        assert_eq!(
            (a.evaluation.level1.ambiguous, a.evaluation.level1.agree),
            (2, 2046)
        );
        assert_eq!(a.evaluation.level2.equal, 2048);
        assert_eq!(
            a.sensitive,
            [("ambiguous share halved", RuleVerdict::NotDecidable)]
        );
        let text = to_text(
            "test",
            a.verdict,
            &a.evaluation.level1,
            &a.evaluation.level2,
            &a.sensitive,
        );
        assert!(
            text.starts_with(
                "test: Pass, sensitive (ambiguous share halved: NotDecidable)\n  rays 2048 unambiguous 2046 ambiguous 2 with a sliver or grazing pair 0 (ambiguous by kind: boundary-edge 2)\n"
            ),
            "{text}"
        );
        // One wrong record fails it, whatever the constants.
        let mut wrong = hits.clone();
        wrong[5] = Hit::MISS;
        let a = assess(ClassKind::Random, &s, &rays, &wrong, &Constants::PROPOSED).unwrap();
        assert_eq!((a.verdict, a.sensitive.len()), (RuleVerdict::Fail, 0));
        assert_eq!(a.evaluation.level1.lost_hits, 1);
        assert_eq!(a.evaluation.level2.differ, 1);
        assert!(evaluate(&s, &rays, &hits[1..], &Constants::PROPOSED).is_err());
    }

    #[test]
    fn the_distance_between_a_segment_and_a_triangle() {
        let t = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]];
        let d = |a: [f64; 3], b: [f64; 3]| segment_triangle(a, b, &t);
        // Through it, above it, beside an edge, beyond a vertex.
        assert_eq!(d([1.0, 1.0, -1.0], [1.0, 1.0, 1.0]), 0.0);
        assert_eq!(d([1.0, 1.0, 2.0], [1.0, 1.0, 5.0]), 2.0);
        assert_eq!(d([1.0, -3.0, 0.0], [2.0, -3.0, 0.0]), 3.0);
        assert_eq!(d([7.0, 0.0, 0.0], [9.0, 0.0, 4.0]), 3.0);
        assert_eq!(d([-1.0, -1.0, 3.0], [5.0, 5.0, 3.0]), 3.0);
        // A triangle that is a line, and one that is a point.
        let line = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [4.0, 0.0, 0.0]];
        assert_eq!(
            segment_triangle([1.0, 3.0, -1.0], [1.0, 3.0, 1.0], &line),
            3.0
        );
        let point = [[1.0, 1.0, 1.0]; 3];
        assert_eq!(
            segment_triangle([1.0, 1.0, 3.0], [1.0, 5.0, 3.0], &point),
            2.0
        );
    }

    #[test]
    fn k_is_twice_the_largest_error_rounded_up_to_a_power_of_two() {
        let fit = |largest: f64| KFit {
            largest,
            ..KFit::NONE
        };
        assert_eq!(fit(3.0).k(2.0), 8.0);
        assert_eq!(fit(4.0).k(2.0), 8.0);
        assert_eq!(fit(4.01).k(2.0), 16.0);
        assert_eq!(fit(0.3).k(2.0), 1.0);
        assert_eq!(fit(0.2).k(2.0), 0.5);
        assert_eq!(fit(4.0).k(1.0), 4.0);
        // A plain scene: the emulated test is within a few spacings.
        let s = one(vec![square(2.0, true)]);
        let rays: Vec<Ray> = (0..200)
            .map(|n| {
                let (i, j) = ((n % 14) as f32, (n / 14) as f32);
                Ray::new(
                    [0.07 * i - 3.0, 0.07 * j + 2.0, -4.0],
                    [3.0 + 0.011 * j, -2.0 + 0.013 * i, 6.0],
                    0.0,
                    100.0,
                )
            })
            .collect();
        let m = measure_k(&s, &rays, Constants::PROPOSED.graze_cos);
        assert_eq!(m.pairs, 400);
        assert!(m.compared >= 150, "{m:?}");
        assert!(m.largest > 0.0 && m.largest < 4.0, "{m:?}");
    }
}
