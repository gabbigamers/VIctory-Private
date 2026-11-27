# DXR validation: the reference half

`crates/victory-rt` is the half of the validation of the ray-tracing compatibility layer that needs
no GPU: synthetic scenes, an exact CPU reference, a CPU model of the existing software BVH path, and
two ways of judging hit records against the reference: the proposed rule for functionally
equivalent rendering ([dxr-compatibility.md](dxr-compatibility.md), "The rule"), levels 1 to 3, and
the older comparison by the reference's own error model. Implemented, covered by host tests on
Linux, type-checked for Windows. **Nothing here has run on a GPU; no number below is a hardware or
performance result, and no verdict below is a verdict on a device.** Every table is one CPU
implementation against another. The rule is a proposal the owner has not accepted.
One test is built for Windows only and has not run: the one in `victory-shader` that holds the node
test here to that crate's own reference (see "The software model").

## Records (little-endian, fixed)

| Record | Words (32-bit) |
|---|---|
| Ray, 48 bytes | origin x y z (f32), tmin, direction x y z (f32, not normalised), tmax, instance mask (low 8 bits used), ray flags (`D3D12_RAY_FLAG` values), two reserved (0) |
| Hit, 32 bytes | t, u, v (f32; u and v the weights of vertices 1 and 2), kind, instance index, instance id (24 bits), geometry index, primitive index |

The kind: 0 a miss (all eight words zero), 1 a front face, 2 a back face, 3 a **procedural** hit,
and with bit 31 a **refusal**.
* A procedural hit is what the hybrid form writes: the device enumerates one box a triangle and the
  shader tests the triangle of the same instance, geometry and primitive index
  ([dxr-shader-emission.md](dxr-shader-emission.md)). It names a triangle like any hit and is judged
  as that triangle's hit; the record has no place for the face, so none is compared.
* A refusal is a shader's statement that it could not decide the ray: bit 31, the reasons in the low
  bits (1 geometry of the other type, 2 its own test left single precision's range), seven zero
  words. It is its own outcome, counted by reason, never a miss and never malformed; the ray is
  traced another way and judged there. A refusal with no known reason, an unknown bit or another
  word set is malformed (`Hit::refusal_reasons`).
* **Alpha** (geometry not marked opaque): accepted iff `((floor(u*8) + floor(v*8) + primitive_index) & 1) == 0`.
  No clamp: a weight an implementation reports just below zero is in cell -1.
* **Front face**, D3D12's default: with `n = (v1 - v0) x (v2 - v0)` on the vertices after the
  geometry's transform and `d` the direction in object space, front iff `d . n < 0` (clockwise from
  the origin, left-handed). No instance transform enters, mirrored or not; a mirrored geometry
  transform turns the winding; the instance's counter-clockwise flag swaps front and back.
* A triangle hit needs `tmin < t < tmax`, both strict. Opacity: the geometry flag, then the
  instance's force flags, then the ray's. Rays D3D12 forbids are refused (`Ray::validate`);
  `generate::illegal_rays` lists eleven, one change each from a legal ray, for a screen in front of a
  device. No class holds one and nothing here traces one: `rule::evaluate` gives an error that names
  the ray, and no verdict, for a list that holds one (a test over the eleven), as it does for a scene
  that fails `Scene::validate`.

## Scenes

A scene is DXR build descriptions (`scene.rs`): bottom levels of triangle geometries (no, 16-bit or
32-bit indices; optional 3x4 transform; opaque flag), instances (3x4 transform, id, mask, flags),
build flags, and frames of changes that ask each structure for a refit or a rebuild. The generator
uses an integer PRNG, exact conversions and the four `f32` operations; a test pins the bytes of all
27 classes at the documented seed (the first ten are the bytes they were before the others were
added). **Random** classes are those the rule's bound on ambiguous rays applies to and its constant
is measured over; **directed** classes aim at a difference, are judged by admissibility and never
validate alone (`Class::kind`). A random class has at least 1,024 rays, the number the bound is
counted on (a test); **no ray of a random class is aimed at an edge**.

| Class | Kind | Holds |
|---|---|---|
| `static-soup` | random | random triangles (no indices) and a 16-bit indexed grid, one instance |
| `dynamic-refit` | random | vertices move with topology kept (built for update), instances move; 4 frames |
| `dynamic-rebuild` | random | triangle counts, geometry lists and the instance list change; 4 frames |
| `instancing` | random | 48 instances of two bottom levels: rotations, non-uniform scales, mirrors, repeated ids |
| `alpha-tested` | random | large non-opaque triangles before an opaque backdrop |
| `overlapping` | directed | one triangle repeated in a geometry, across geometries, across instances; coplanar overlaps |
| `transformed` | random | geometry transforms (one mirrored) under rotated, scaled, sheared, mirrored instances; facing culls |
| `hit-ordering` | random | stacks of non-opaque layers: accepted and rejected candidates interleaved |
| `precision` | directed | rays exactly through edges and vertices of closed meshes, grazing rays, slivers, triangles of no area, coordinates near 2^20 (in vertices and by instance), tmin and tmax exactly on a hit |
| `masks-and-flags` | random | instance masks and flags against ray masks and every ray flag |
| `single` | random | one triangle; 2,048 rays |
| `closed` | random | two cubes of six surfaces each (one geometry a face) and two octahedra, moved and scaled; 700 rays from outside and 700 from inside a mesh, where none may escape, origins and directions of 24 random bits a coordinate |
| `far` | random | triangles with edges of 2^-4 to 2^4 units at 2^12, 2^14 and 2^16 from the origin on one axis, at 2^16 on three, and at 2^14 by an instance; 300 rays a cluster |
| `seam` | random | six cubes, each cut in two and put together again: two bottom levels under one transform (a translation; a rotation and a translation), one bottom level plain and mirrored, two bottom levels with different translations, a geometry transform, and two geometries of one bottom level (no seam: D3D12's own shared edge, between two surfaces); random rays |
| `surface-rays` | random | rays that leave a surface and rays whose extent ends on one, with biases of 0, 1, 16 and 256 tolerances along the normal |
| `scale` | random | 2,048 triangles; 768 incoherent rays and a coherent bundle of 768 from one origin |
| `seam-edge` | directed | the `seam` scene; rays exactly on the seams and 1, 2 and 8 spacings either side, 168 a cube |
| `edge` | directed | two triangles sharing an edge, a fan of six, an octahedron; rays exactly on every edge and vertex and 1, 2 and 8 spacings off |
| `thin` | directed | slivers and needles, base 8 and height 1 down to 2^-17 (aspect to 2^20); rays through inside, long edge and vertices |
| `large-thin` | directed | triangles a few spacings across at 2^16, 2^20 and 2^24 from the origin |
| `cull-edge` | directed | an edge shared by a front and a back face, under each facing cull and none |
| `extent` | directed | hits at exactly `tmin` and `tmax` and one spacing either side, rays that start on a surface (t = 0; tmin 0 and -0.0), tmax infinite and the largest float |
| `tie` | directed | the same triangles twice in one geometry and once in each of two; parallel squares 1, 2, 4 and 8 spacings apart, of one surface and of two |
| `far-graze` | directed | the `edge` shapes at 2^18 from the origin, met at rises of 1/2 to 1/16 over runs of 4 to 8 |
| `flat` | directed | a cube and a grid in axis planes; rays along an axis through grid lines and vertices, and rays lying in a face's plane |
| `degenerate` | directed | triangles of no area (collinear, two vertices equal, a point), plain, under a turned geometry transform and in a turned instance |
| `closed-edge` | directed | the `closed` meshes; 700 rays from inside one whose origins are multiples of 1/8 and whose directions are multiples of 1/4, so that many go exactly through an edge or a vertex of its faces |

A **spacing** is the distance between neighbouring `f32` at the coordinate moved. The unit of the
`surface-rays` biases is 8 spacings of the largest coordinate at the surface point, fixed in the
generator (it cannot take the rule's constant, which is measured on this class among others); a
test fails when it is not one tolerance.

Two of these were changed after the review of the first build. The rays of `closed` from inside a
mesh had lattice origins and directions, and lay exactly on edges of its faces: over 20 seeds 353 of
its rays were ambiguous, every one by an edge, 205 with a weight of exactly zero, where the other
random classes have none to one such ray. Those rays are now the directed class `closed-edge`, and
`closed` has random ones (10 ambiguous rays in the same 20 seeds, none exactly on an edge). `single`
had 1,024 rays; a few of them always have a sliver or grazing pair, which left fewer than the 1,024
the bound is counted on ("The verdict", below), so it has 2,048.

## The reference

**Truth, exact**, over every triangle of every instance, no acceleration structure. An `f32` is
an integer times a power of two, so the world-space vertices (forward transforms only) and what
follows are exact in a big-integer type (`exact.rs`). With
`A, B, C` the vertices minus the origin, `e1 = B - A`, `e2 = C - A`, `n = e1 x e2`:
`E1 = A.(d x e2)`, `E2 = A.(e1 x d)`, `S = d.n`, `D = A.n`, `E0 = S - E1 - E2`; `t = D/S`,
`u = E1/S`, `v = E2/S`. A hit needs `S != 0` and no negative `Ek/S`: edges and vertices count, and
no area means `S = 0`, never a hit, as in D3D12. Signs, range tests, the order of two hits
(cross-multiplied) and the alpha cell are decided exactly: "on an edge", "the same t", "exactly on
tmax" are facts, not thresholds. Among accepted hits with one `t` the reference takes the lowest
instance index, geometry, primitive: its own rule, since D3D12 leaves the choice open. The
reference's records are the exact numbers rounded to `f32`; an exact zero is `+0`. This answer
(`Diag::primary`) is the one both ways of judging start from: `rule.rs` takes it as it is and
checks on every ray that it is among the answers the rule admits (a host test over every class:
never otherwise).

## The rule as built (`rule.rs`, `shade.rs`, `picture.rs`)

What [dxr-compatibility.md](dxr-compatibility.md) ("The rule") states, as pure functions. Where
building it, or reviewing what was built, showed the rule's first wording not to hold, the wording
was changed there (marked [C1] to [C7], proposed to the owner); this section describes the code,
which follows the changed wording.

### Tolerance, and the constant

For a ray and a triangle, in the instance's object space in `f64`: `L` the direction's length, `c`
the cosine between ray and normal as a magnitude, `h` the smallest altitude (twice the area over
the longest edge), `M` the largest magnitude among the coordinates of the origin and of the
vertices in world and in object space and `t L` (either space; `t` no further than `tmax`).

```text
x_tol = k * 2^-23 * M      b_tol = 2 x_tol / (h c)      t_tol = x_tol / (L c) + b_tol * D
```

`D` is how far the depths of the three vertices along the ray lie from `t`, the farthest; a
vertex's depth is taken two ways, as its foot on the ray and as its distance along the direction's
largest axis (how the emulated test, which shears the ray onto that axis, measures it).

**The derivation of k.** `rule::measure_k` takes every ray and every triangle of every instance
(no flag, mask, extent or filter), keeps the pairs with `c >= 2^-6` and `h c >= 2^-10 M`, and
compares the emulated triangle test's answer with the exact one. The emulated answer
(`Rule::emulated`) is the software path's own arithmetic: the ray through the instance record's
inverse rounded to `f32` as the traversal takes it there, the triangle node as the builder writes
it, the node test, the three divisions; a host test holds it bit for bit to what the traversal
computes for the triangle it ends on. **The error of a pair is the smallest `x_tol` at which its
answer is inside the tolerances as they stand**: for a weight (the largest of the three errors, the
first weight as `1 - u - v`), `|db| h c / 2`; for `t`, `|dt| / (1 / (L c) + 2 D / (h c))`, the
divisor being `t_tol` for an `x_tol` of one. So `k` times the unit is exactly the tolerance a pair
is judged with, and an error of `e` units is a record at `e / k` of its tolerance. Where one side
hits and the other does not, the error is at least as far as the exact numbers are from the edge or
from `t = 0`, and that is counted. The unit is `2^-23 M`. `k` is twice the largest error, rounded
up to a power of two.

The documented seed (`0x5EED20261127`), by class:

| Class | Pairs in range | With an error to measure | Largest error, in `2^-23 M` | Of `t` | Of a weight | Twice the largest, rounded up |
|---|---|---|---|---|---|---|
| static-soup | 543145 | 3207 | 0.744 | 0.744 | 0.274 | 2 |
| dynamic-refit | 1400023 | 6563 | 1.076 | 1.076 | 0.408 | 4 |
| dynamic-rebuild | 449561 | 5051 | 0.678 | 0.678 | 0.360 | 2 |
| instancing | 969414 | 3865 | 1.288 | 1.288 | 0.441 | 4 |
| alpha-tested | 216599 | 4540 | 0.950 | 0.950 | 0.430 | 2 |
| transformed | 881459 | 2744 | 1.225 | 1.225 | 0.445 | 4 |
| hit-ordering | 278400 | 9587 | 1.348 | 1.348 | 0.312 | 4 |
| masks-and-flags | 675228 | 3277 | 1.407 | 1.407 | 0.435 | 4 |
| single | 2024 | 1641 | 0.495 | 0.495 | 0.234 | 1 |
| closed | 305995 | 2156 | 0.875 | 0.875 | 0.241 | 2 |
| far | 63 | 13 | 0.006 | 0.006 | 0.001 | 2^-6 |
| seam | 415308 | 2819 | 0.781 | 0.781 | 0.369 | 2 |
| surface-rays | 413353 | 1845 | 1.050 | 1.050 | 0.238 | 4 |
| scale | 2501538 | 1084 | 0.479 | 0.479 | 0.218 | 1 |

All fourteen random classes at the documented seed: 9,052,110 pairs in range, 48,392 with an error
to measure, largest error 1.407 (`masks-and-flags`; a difference of numbers, both sides hitting).
**Alone, the documented seed gives k = 4.** The error of `t` carries it in every class; a weight's
is under half a unit. `far` contributes almost nothing: at 2^12 and beyond its triangles are below
the measurement's `h c >= 2^-10 M`.

**The sample is the documented seed and the 200 that follow it, and over it k = 8.** By hand and not
a test (`more_seeds_by_hand` with `SWEEP=200`: 201 seeds, 6,633 frames, 8,363,007 rays over all 27
classes, 1,826,752,753 pairs in range in the random ones): the largest error is 2.369 units, of `t`,
at seed + 105 in `transformed`; the largest of a weight is 0.986. Of the 201 seeds 9 carry an error
over 2 units and give 8 alone (+ 22, 48, 50, 70, 105, 110, 135, 136 and 148); the other 192 give 4.
A largest error grows with its sample, and this one crosses the power of two inside the sample, so
which seeds are measured decides between 4 and 8. `rule::K` is the whole sample's, 8: the margin
over the emulation's own largest error is then 3.4, and with 4 it would be 1.7, under the factor 2
the rule states. `tests/rule.rs` measures the seeds of `rule::K_SEEDS` (the documented one and +
105) and fails unless together they give `rule::K` and the documented one alone gives half of it; at
three more seeds it asserts that no error is over half the constant. The result file carries the
constant (below).

**k is a property of these classes' triangles and rays**: their shapes, sizes, magnitudes and the
slants at which the rays meet them. It is not a bound for other shapes, and the measurement gives
other numbers on them. A needle 384 to 512 times as long as it is high, 2 to 4 units from the
origin, met at a slant, every pair inside the measured range (a test): 0.010 to 0.012 units, a
hundredth of what the classes show. And the reading matters as much as the shape: by the first
wording's `t` tolerance, `x_tol / (L c)` without the term [C3] added, the same needles measure
4.3 and 4.7 units, which is k = 16, and the classes measure 2.156 at the documented seed. That is
what this measurement did as first built (it read `t` against a tolerance the rule no longer had;
found in review); the numbers above are by the tolerance as it stands.

**How the error model's tolerance and band stand to it.** Before the rule was built this crate
judged by a bound of modelled `f32` arithmetic ("The error model", below) and reported a band at
1/8 of that bound, made from the software model's worst error (0.096 of the bound). For judging a
path under the rule both are **replaced**: the tolerance by the three formulas above, the band by
the factor 2 in `k`, which is the same statement made once for all classes (an error twice the
emulation's largest over the sample is the limit, instead of an error larger than any it has shown
being reported). The error model stays in two places: it is the first cut that decides which pairs the
exact arithmetic looks at, and `compare.rs` still judges by it, for callers that use it and for
the tables it made. The two tolerances on the same unambiguous hits (the rule's over the model's):

| Class | Hits | Weights: least | median | largest | t: least | median | largest |
|---|---|---|---|---|---|---|---|
| static-soup | 1566 | 1.09 | 2.15 | 45.2 | 0.28 | 0.69 | 20.9 |
| dynamic-refit | 892 | 0.20 | 1.81 | 49.2 | 0.09 | 0.59 | 22.2 |
| dynamic-rebuild | 575 | 1.00 | 2.04 | 34.0 | 0.28 | 0.62 | 15.8 |
| instancing | 1202 | 0.02 | 0.32 | 7.7 | 0.01 | 0.13 | 1.8 |
| alpha-tested | 1397 | 0.15 | 0.86 | 18.0 | 0.07 | 0.35 | 10.8 |
| overlapping | 350 | 0.90 | 2.06 | 17.5 | 0.31 | 0.63 | 6.2 |
| transformed | 1185 | 0.06 | 0.23 | 6.3 | 0.02 | 0.06 | 0.7 |
| hit-ordering | 1410 | 0.27 | 1.98 | 16.3 | 0.12 | 0.46 | 5.0 |
| precision | 227 | 1.11 | 3.02 | 7066 | 0.29 | 1.06 | 1256 |
| masks-and-flags | 683 | 0.16 | 0.42 | 52.6 | 0.06 | 0.18 | 28.5 |

Neither bounds the other. Without an instance transform the rule allows about twice the model's
weight error and two thirds of its `t` error; behind a rotated or scaled instance the model, whose
bound carries the transform's condition number, allows three to fifteen times what the rule does.
The rule's is the sharper test exactly where the older one was weak (its own table, below: an
error of 1e-4 in `u` behind an instance mostly passes there).

### Pairs, rays and kinds

* A pair is **grazing** when `c < 2^-6`, a **sliver pair** when `b_tol > 2^-4`, **degenerate** when
  the triangle has no area (exactly). Any other pair is **out** when a weight is below `-b_tol` or
  `t` is outside the extents by more than `t_tol`, **in** when every weight is above `b_tol` and
  `t` inside by more than `t_tol`, else **near**. A grazing, sliver or degenerate pair is placed by
  distance: out when the ray inside its extents passes the triangle, as a set of points, by more
  than `2 x_tol`; else near; never in.
* **The face** a pair shows the ray is a fact for an ordinary pair and for a sliver pair whose
  `b_tol` is at most 1/2 (`rule::FACE_B_TOL`; `h c` at least `4 x_tol`, where no move of the
  vertices by `x_tol` turns it: dxr-compatibility.md, "The rule"). A thinner sliver pair and a
  grazing pair have none: such a pair is not culled by a facing flag and its face is not compared.
  As first built every sliver pair had a face; a path with its vertices moved by an eighth of the
  position tolerance then failed on faces alone, in `far`, `large-thin` and `precision` (found in
  review; "A path inside the position tolerance", below).
* **The filter** is evaluated at nine points of the box `u, v` plus or minus `b_tol`: all accept
  and the candidate stands, all reject and it is out (and a record that names it is a filter flip),
  otherwise it is near, at a **filter threshold**. A box as wide as a cell (`b_tol >= 2^-4`) is at
  a threshold.
* **Neighbours.** Every triangle of every instance gets the ids of its three world-space vertices
  as exact points; two triangles share an edge when two ids agree, and a vertex when one does. Two
  near pairs **close** an edge when each is near through that edge alone, they lie on opposite
  sides of it as the ray sees them, and they show the ray the same face; a fan closes a vertex when
  every point of its rim ends two of its members. **A near pair also closes an edge with a
  neighbour that is out by its own tolerance**: an ordinary pair on the other side that shows the
  same face, that no flag or mask takes away, and that is out by a weight alone. The two
  tolerances differ with the triangles' sizes, so the ray can be within the one's and outside the
  other's. Where the ray is inside the near triangle (asked of the exact arithmetic) the neighbour
  cannot be hit, the two still cover the ray, and the near triangle is the one answer; where it is
  not, the ray passes beyond the neighbour's reach and the edge is a boundary for it. A fan with
  such members closes its vertex the same way, where the ray is inside one of its near members.
  As first built the neighbour was looked for among the in and near pairs only, and a miss was
  admissible up to 1.5 units inside a large triangle beside a small one (found in review). A
  neighbour that is out by the extents, not by a weight, closes nothing.
* What the sides are to each other names the kind: the same instance and matching geometry
  transforms, a **shared edge**; instances with bit-equal transforms and no geometry transform, a
  **closed seam**; anything else, an **open seam**; no partner, a partner on the same side, one
  showing the other face, one that is culled, masked or filtered away, one that is itself a sliver
  or grazing pair, or one whose reach the ray passes beyond, a **boundary or silhouette edge**.
* A hit is **certain** when a pair is in or a shared edge or closed seam is closed. For a nearest
  hit the admissible answers are a hit on any in or near pair that is not behind a certain hit by
  more than the tolerances, and a miss when no hit is certain; for a ray with
  `ACCEPT_FIRST_HIT_AND_END_SEARCH` (a yes-or-no workload) a hit on any in or near pair, and a miss
  when none is certain. A ray is **unambiguous** when its one admissible answer is a miss or a hit
  on a pair that is in (yes or no: when a hit is certain or nothing is near); a ray whose one
  answer is a hit on a near pair is ambiguous, by the kind of its edge. The other kinds: **extent**
  (`t` within `t_tol` of `tmin` or `tmax`, `t = 0` included), **tie** (two in-pairs within their
  tolerances), **degenerate**. A ray is counted as having a sliver or grazing pair when such a
  pair is among its admissible answers, that is, where it can change the answer: one behind a
  certain hit counts for nothing.
* **Judging** (`rule::judge`): on an unambiguous ray hit or miss, instance, primitive and face
  exactly, `t` within `t_tol`, each of the three weights within `b_tol` (`agree`); on an ambiguous
  ray any admissible answer under the same rules (`admissible`). A hit on a grazing, sliver or
  degenerate pair must have its `t` between the nearest and the farthest of the triangle's
  vertices along the ray, widened by `x_tol / (L 2^-6)`; its weights are not compared. Failures:
  `lost_hits`, `gained_hits`, `id_flips` (another triangle of an admissible surface),
  `surface_flips` (of another surface), `facing_flips`, `filter_flips`, `out_of_tol`, malformed.
  A refusal is `refused`, by reason, and no failure.
* **What is not judged** (`rule::evaluate`): a list with a ray that fails `Ray::validate`, or a
  scene that fails `Scene::validate`, gets an error that names the ray and no verdict; as first
  built such a ray was judged or brought the reference down (found in review). So does a ray on
  which the exact reference's own answer is not among the admissible ones: a defect of this
  reference, never seen, and no longer left to the tests alone to notice.
* **The verdict** (`rule::verdict`), levels 1 and 2 together: `Fail` on any failure; for a directed
  class otherwise `Admissible`, which is never a pass. For a random class, in this order:
  `ValidatesNothing` when more than one ray in 16 has a sliver or grazing pair or was refused, the
  two together and a ray that is both counted once; `NotDecidable` when, among the rays without
  such a pair, more than one in 1,024 is ambiguous; `ValidatesNothing` when it has fewer than 1,024
  rays without such a pair (the bound cannot be counted on fewer: no ray, three rays) or fewer than
  one ray in 4 is an unambiguous hit that agreed (`Level1::agree_hits`; rays that meet nothing hold
  a path to nothing); else `Pass`. The two floors and the sum of the two shares came from the
  review: as first built an empty list, three rays, and 1,600 rays aimed past every triangle each
  passed, and a class with one ray in 16 refused and one in 16 with a sliver pair passed.
* **The reach** (`Level1::m_max`): the largest `M` among the unambiguous hits that agreed, the
  pairs whose numbers were compared. Rays with a sliver or grazing pair add nothing to it; their
  largest `M` is `Level1::m_special_max`, reported apart. As first built `m_max` took every
  admissible pair, and a class of triangles checked near the origin with a few slivers at 2^20
  passed with a reach of 2^20 (found in review).

### Level 2: what shading consumes (`shade.rs`)

The fixed function that stands for the code after a traversal region. Of a hit record, its ray and
the scene's vertex data it takes:

* the **surface**: the instance's 24-bit id with the geometry index's low 8 bits above it. Two
  triangles are of one surface, and one material, when both agree;
* the **face**: the kind (front, back, or none for a procedural hit);
* the **hit point**: `origin + direction * t`, each component a product and a sum in `f32`;
* an **attribute**: `(a0 + u (a1 - a0)) + v (a2 - a0)` in `f32`, where a vertex's value `a` is a
  multiple of 1/256 in [0, 1) made from the bits of its position as the geometry holds it (three
  rounds of the MurmurHash3 finaliser, which is public domain; a negative zero counts as zero).
  Triangles that share a vertex's bits share its value: the attribute is continuous across a
  shared edge and a closed seam and not across a seam whose sides hold other numbers.

A miss consumes eight zero words; a refusal is not consumed. `shade::judge_consumed` compares what
a record gives with what the admissible answers give: **equal** (the words of the reference's own
answer, bit for bit), **within** (the surface and face of an admissible answer; the hit point on
the ray inside that answer's interval of `t`, to four spacings of the largest coordinate; the
attribute within `b_tol` times its range over the triangle plus four spacings, and not compared
where the weights are not), or **differ**. The face is not compared where the record or the pair
has none.

### Level 3: pictures (`picture.rs`)

* **Rays**: from one eye at `centre + half * (1.5, -3, 2.25)`, one through the middle of each pixel
  of a square image plane through the centre of the cube around the scene (half-extents
  `0.8125 half (2, 1, 0)` and `0.30078125 half (-1.5, 3, 5)`: across the view and of one length),
  `tmin` 0, `tmax` 1, the plane at `t = 1/2`. The `far` and `large-thin` classes, whose clusters
  would be smaller than a pixel, get one camera a cluster.
* **A pixel**: the surface's colour (three channels of 96 to 223 from its number), times 1/2 on a
  back face, times the ramp `1 - t/2` with `t` held to 0..1, times `1/2 + attribute/2`, each
  channel `floor(x + 1/2)`. A miss is (16, 16, 24); a refused record (255, 0, 255), which no hit
  has. A path whose records carry no face is drawn, and compared with a reference drawn, without
  the darkening.
* **Comparing** (`picture::compare`): a pixel differs when a channel differs by more than 2 of 255.
  A pass: at most one pixel in 1,024 differs, each from a ray the reference flagged (ambiguous, by
  a kind or by a sliver or grazing pair), and no group of touching differing pixels (8 neighbours)
  is larger than 2. The report lists every differing pixel with its ray and whether that ray was
  flagged, the share, the number of groups and the largest, and how many pixels of the reference's
  drawing show a surface. A picture of no pixels, and one whose reference drawing shows no
  surface, are an error and no verdict (as first built the first passed; found in review).

The pictures are 1,024 a side (`picture::SIDE`); the host tests draw 128 and 96 with the same
functions.

### Each constant halved and doubled

`Constants::variants` gives eighteen variants, each of one constant: the factor in `k` (so `k`
itself), the grazing cosine, the sliver tolerance, the two shares of rays, the pixel step, the
pixel share, the pixel group, the share of unambiguous hits. The floor of 1,024 rays is the
ambiguous share's own number and moves with it: a pass on fewer than 2,048 rays is flagged under
"ambiguous share halved", which says that it could not be counted under a halved bound. `rule::assess` recomputes the verdict of levels 1 and 2 under each
(`k` and the two thresholds change which rays are ambiguous, so those re-read every ray);
`picture::assess` recomputes the picture's. A verdict that changes is **sensitive**, and the
variants that change it are named with what it changes to. The title-level constants (8 of 255,
16-pixel tiles) belong to captures and are not here.

### The result file (`results.rs`)

Text; a header line, a producer line, a line `rule k 8` with the constant of the writer's build,
then per class, seed and frame a section of eight hex words a ray and a checksum. The rays are
regenerated from class and seed, and each section carries the fingerprint of its generated case,
which `Section::check` compares with the case generated on the reading side: a generator that
differs between two machines shows as that, not as failures. `check_k` does the same for the
constant: it is measured from the generator and the model, so another value in a file means
another build of either. `ResultFile::parse` refuses such a file with that message;
`ResultFile::parse_with_k` reads it and hands the constant over (as first built `parse` read it
and dropped the line; found in review). A file of version 1 has no such line and reads as before. Numbers are
digits only; a repeated section, or a name the format cannot hold, is refused on writing and on
reading.

## The error model and the older comparison (`reference.rs`, `compare.rs`)

**What may be returned instead, by the model.** A model of `f32` arithmetic classes each pair as
ruled out, sure, doubtful or unstable. A ray's permitted results are the sure and doubtful
candidates that can come out nearest, and a miss if nothing is sure. Doubtful candidates that share
an edge (the same vertices bit for bit, one instance, matching geometry transforms, opposite sides
as the ray sees them) or close a fan around a shared vertex count as one sure hit. A ray is
**clear** (one permitted result), **ambiguous** (several) or **excluded** (an unstable candidate,
or over 16 results). The flags are about the ray's permitted candidates, not about the reference's
own hit; what is true of that hit alone is in the margins.

**The model** is an implementation working in the instance's object space in `f32`, unit roundoff
`u = 2^-24`. Each quantity carries a bound per component:
* a vertex after a geometry transform: `4u * sum|terms|` (three products, three sums); else 0;
* the ray through the inverse `W`: origin `(5 + 4k)u * sum|W|(|o| + |T|)`, direction
  `(4 + 4k)u * sum|W||d|`, with `k` the transform's condition number for an inverse computed in
  `f32`; a pure translation is one rounded subtraction, `u|o'|`; the identity is exact;
* `A = p0 - o'`: both bounds plus `u|A|`; an edge vector: its vertices' bounds plus `u|e|`;
* a product `da|b| + |a|db + da db`; a cross-product component two products plus
  `u(|p| + |q| + |p - q|)`; a dot product three products plus `3u * sum|a_i b_i|`.

This bounds `E1`, `E2`, `S`, `D` (and `E0`) to first order; every bound is multiplied by 4 for other
formulations. With `dX` the bound of `X`:
* edge functions of mixed sign beyond their bounds **rule the pair out**; otherwise `|S| <= 4 dS`
  makes it **unstable** (a ray in the plane, a sliver, no area);
* `tol_w = (dEk + |wk| dS) / (|S| - dS) + u` (a weight below `-tol_w` rules out); `tol_uv` is the larger of u's and v's;
* `tol_t = (dD + |t| dS) / (|S| - dS) + 2u|t| + sum_k tol_w,k * spread_k`, `spread_k` being how far
  vertex k's depth along the ray is from `t`: the term the rule's `t_tol` now has too ([C3]).

A candidate is **doubtful** when a weight is within `tol_w` of 0, `t` within `tol_t` of `tmin` or
`tmax`, or `8u`, `8v` within `8 tol_uv` of a cell boundary (0 is one). A permitted result with
`tol_uv > 1/16` or `tol_t` over 1/16 of the distance to the triangle's farthest vertex is **wide**:
its triangle and face are compared, its numbers are not. Behind rotated and scaled instances the
`k` term makes the tolerance some 10 to 40 times that of untransformed geometry.

**The comparison** `compare(class, diags, baseline, candidate)` judges two hit lists against the
reference by that model.
* **Exact**: hit or miss; instance index, instance id, geometry and primitive index; front or back
  (not for a procedural hit); a miss is the all-zero record. **Within tolerance**: `t`, `u`, `v`
  against the exact values; for a wide result not at all, counted as right **by ids only**. An
  ambiguous ray takes any listed result; an excluded ray is counted, not judged. Anything else is
  a defect: missed, phantom, wrong triangle, wrong face, numeric, malformed. A refusal is counted
  by reason and is none of these.
* **The band**: a result within tolerance whose error is over 1/8 of it is counted as **beyond the
  band** and named on the verdict's line; not a defect.
* **Verdict**: `Fail` (candidate defect), `BaselineFail`, `Useless` (fewer than 200 rays, under
  20 % clear, over 10 % excluded or refused, or over 10 % with a wide permitted result),
  `Fallback` (no defect, but the two lists name different right results on rays that are not
  first-hit queries), `Pass`.

It is kept as it was, with the two new records, because other code calls it. A verdict of it is
not a verdict under the rule.

## The software model

`node.rs` is the per-node test as `victory-shader` emits it (`emit/lower/bvh.rs`), operation for
operation. `console.rs` writes a scene as an image: the 64-byte triangle node and the 128-byte
32-bit box node as the test reads them, in four-wide trees; the top level, instance records, header
and the node fields the test does not read are **our own, synthetic, not any title's**, so nothing
here shows how a title's own traversal behaves. `traverse.rs` walks the image: a 64-entry stack,
nearest child first, `tmin < t < committed t` (of equal hits the first met stays), the alpha test,
the end at an empty stack or a first-hit ray's first accepted hit. Stack overflow, a bad pointer or
node type, the step limit and an origin beyond the padded reach are errors, never a quiet miss; each
has a test that reaches it.

`node.rs` is a second copy of the CPU reference in `victory-shader`'s Windows tests
(`emit/bvh_tests.rs`), which those tests compare with the GPU. One test there,
`the_validation_crates_node_test_equals_the_reference`, compares the two copies word for word on
400,000 nodes and rays, so a change to the lowering cannot leave this model behind. It is built for
Windows only: type-checked here, and the same text run on this host against a copy of that
reference (passing, and failing on five planted differences), but not run as the test it is.

**Its bar is unchanged by the rule: a GPU run of the software path is to equal this model bit for
bit**, through the boxes (`Boxes::Tested`) and with every triangle tested (`Boxes::AllVisited`).
Nothing in `node.rs`, `console.rs` or `traverse.rs` was changed for the rule. The rule is what a
native path is judged by; the model is judged under it below only to show that the rule's
reference and the model, written apart, agree, and what the model does where the rule lets an
answer vary.

## Measured on the host, under the rule (seed `0x5EED20261127`, k = 8)

The software model's records against the rule's reference. **No device is in any row.**

Level 1 (41,607 rays; `ambiguous` counts rays without a sliver or grazing pair, which are in the
next column; the kinds count every ambiguous ray, and a ray can have several):

| Class | Kind | Rays | Unambiguous | Ambiguous | Sliver or grazing | Shared edge | Closed seam | Open seam | Boundary edge | Extent | Tie | Filter | Degenerate | Agree | Admissible | Not accepted | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| static-soup | random | 2000 | 1989 | 2 | 9 | 0 | 0 | 0 | 1 | 0 | 1 | 0 | 0 | 1989 | 11 | 0 | NotDecidable; sensitive (k halved: Pass, ambiguous share doubled: Pass) |
| dynamic-refit | random | 4800 | 4776 | 3 | 21 | 1 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 4776 | 24 | 0 | Pass; sensitive (k doubled: NotDecidable, ambiguous share halved: NotDecidable) |
| dynamic-rebuild | random | 4800 | 4783 | 6 | 11 | 4 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 4783 | 17 | 0 | NotDecidable; sensitive (ambiguous share doubled: Pass) |
| instancing | random | 1500 | 1481 | 5 | 14 | 0 | 0 | 0 | 5 | 0 | 1 | 0 | 0 | 1481 | 19 | 0 | NotDecidable |
| alpha-tested | random | 2000 | 1963 | 15 | 22 | 0 | 0 | 0 | 0 | 1 | 0 | 14 | 0 | 1963 | 37 | 0 | NotDecidable |
| overlapping | directed | 1600 | 842 | 743 | 15 | 0 | 0 | 0 | 1 | 0 | 744 | 3 | 0 | 842 | 758 | 0 | Admissible |
| transformed | random | 1600 | 1583 | 4 | 13 | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 1583 | 17 | 0 | NotDecidable |
| hit-ordering | random | 1600 | 1600 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1600 | 0 | 0 | Pass; sensitive (k doubled: NotDecidable, ambiguous share halved: ValidatesNothing) |
| precision | directed | 1735 | 293 | 324 | 1118 | 147 | 0 | 0 | 65 | 64 | 0 | 0 | 68 | 293 | 1442 | 0 | Admissible |
| masks-and-flags | random | 2000 | 1985 | 4 | 11 | 0 | 0 | 0 | 2 | 0 | 0 | 3 | 0 | 1985 | 15 | 0 | NotDecidable |
| single | random | 2048 | 2033 | 1 | 14 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 2033 | 15 | 0 | Pass; sensitive (ambiguous share halved: NotDecidable) |
| closed | random | 1400 | 1397 | 0 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1397 | 3 | 0 | Pass; sensitive (ambiguous share halved: ValidatesNothing) |
| far | random | 1500 | 562 | 61 | 877 | 0 | 0 | 0 | 123 | 5 | 6 | 0 | 0 | 562 | 938 | 0 | ValidatesNothing |
| seam | random | 1536 | 1522 | 3 | 11 | 2 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 1522 | 14 | 0 | NotDecidable; sensitive (k halved: Pass) |
| surface-rays | random | 1600 | 857 | 736 | 7 | 1 | 0 | 0 | 0 | 736 | 0 | 0 | 0 | 857 | 743 | 0 | NotDecidable |
| scale | random | 1536 | 1529 | 3 | 4 | 0 | 0 | 0 | 3 | 0 | 0 | 0 | 0 | 1529 | 7 | 0 | NotDecidable |
| seam-edge | directed | 1008 | 0 | 1008 | 0 | 171 | 345 | 504 | 0 | 0 | 0 | 0 | 0 | 0 | 1008 | 0 | Admissible |
| edge | directed | 966 | 0 | 966 | 0 | 546 | 0 | 0 | 420 | 0 | 0 | 0 | 0 | 0 | 966 | 0 | Admissible |
| thin | directed | 720 | 132 | 301 | 287 | 0 | 0 | 0 | 301 | 0 | 0 | 0 | 0 | 132 | 588 | 0 | Admissible |
| large-thin | directed | 720 | 0 | 0 | 720 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 720 | 0 | Admissible |
| cull-edge | directed | 552 | 300 | 252 | 0 | 0 | 0 | 0 | 252 | 0 | 0 | 0 | 0 | 300 | 252 | 0 | Admissible |
| extent | directed | 600 | 120 | 480 | 0 | 6 | 0 | 0 | 0 | 480 | 0 | 0 | 0 | 120 | 480 | 0 | Admissible |
| tie | directed | 800 | 180 | 617 | 3 | 0 | 0 | 0 | 0 | 0 | 617 | 0 | 0 | 180 | 620 | 0 | Admissible |
| far-graze | directed | 966 | 0 | 0 | 966 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 966 | 0 | Admissible |
| flat | directed | 800 | 208 | 164 | 428 | 172 | 0 | 0 | 430 | 29 | 0 | 0 | 0 | 208 | 592 | 0 | Admissible |
| degenerate | directed | 520 | 309 | 211 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 210 | 309 | 211 | 0 | Admissible |
| closed-edge | directed | 700 | 683 | 17 | 0 | 17 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 683 | 17 | 0 | Admissible |

**No record of the software model is one the rule does not accept, in any class** (a test), and on
every unambiguous ray its record agrees. Every disagreement between the model and the reference's
own answer is therefore on a ray the rule calls ambiguous, by the kinds in the table; none is
unclassified. The verdicts of the random classes are the rule's reading of *these scenes at this
tolerance*, with a model that makes no error the rule refuses: with 1,500 to 2,000 rays the bound
allows one ambiguous ray, and most classes have a few more ("What the bound comes to", below).
`hit-ordering` and `closed` pass with no ambiguous ray among fewer than 2,048, which a halved bound
could not count: flagged. `far` validates nothing twice over: 877 of its 1,500 rays have a sliver or
grazing pair, and 277 are unambiguous hits, under one in 4.

Sizes: the unambiguous hits that agreed, the reach (the largest magnitude among them, `m_max`) and
beside it the largest magnitude among the rays with a sliver or grazing pair, which is no reach;
the worst error of the model in tolerances, where numbers are compared; level 2:

| Class | Unambiguous hits | Largest magnitude among them | Among rays with a sliver or grazing pair | Worst t, of tolerance | Worst weight, of tolerance | Consumed: equal | within | differ |
|---|---|---|---|---|---|---|---|---|
| static-soup | 1566 | 5.7e1 | 5.1e1 | 0.062 | 0.034 | 601 | 1399 | 0 |
| dynamic-refit | 3687 | 6.6e1 | 4.7e1 | 0.134 | 0.051 | 1480 | 3320 | 0 |
| dynamic-rebuild | 2620 | 4.2e1 | 3.6e1 | 0.085 | 0.037 | 2463 | 2337 | 0 |
| instancing | 1203 | 1.9e2 | 1.0e2 | 0.075 | 0.050 | 329 | 1171 | 0 |
| alpha-tested | 1397 | 1.7e2 | 1.6e2 | 0.111 | 0.050 | 743 | 1257 | 0 |
| overlapping | 350 | 9.7e1 | 7.8e1 | 0.096 | 0.039 | 550 | 1050 | 0 |
| transformed | 1189 | 2.0e2 | 1.3e2 | 0.144 | 0.056 | 417 | 1183 | 0 |
| hit-ordering | 1410 | 2.3e1 | 0.0e0 | 0.140 | 0.038 | 749 | 851 | 0 |
| precision | 227 | 3.6e1 | 1.0e6 | 0.056 | 0.023 | 1141 | 594 | 0 |
| masks-and-flags | 683 | 7.1e1 | 5.1e1 | 0.113 | 0.053 | 1330 | 670 | 0 |
| single | 1431 | 8.8e0 | 8.6e0 | 0.062 | 0.029 | 1048 | 1000 | 0 |
| closed | 1257 | 1.1e2 | 2.7e1 | 0.094 | 0.024 | 450 | 950 | 0 |
| far | 277 | 6.6e4 | 6.6e4 | 0.001 | 0.000 | 427 | 1073 | 0 |
| seam | 1200 | 9.7e1 | 8.1e1 | 0.091 | 0.046 | 429 | 1107 | 0 |
| surface-rays | 478 | 5.3e1 | 4.6e1 | 0.131 | 0.029 | 962 | 638 | 0 |
| scale | 817 | 2.0e2 | 1.4e2 | 0.060 | 0.027 | 735 | 801 | 0 |
| seam-edge | 0 | 0.0e0 | 0.0e0 | 0.078 | 0.048 | 735 | 273 | 0 |
| edge | 0 | 0.0e0 | 0.0e0 | 0.060 | 0.010 | 804 | 162 | 0 |
| thin | 132 | 3.8e1 | 3.8e1 | 0.000 | 0.001 | 714 | 6 | 0 |
| large-thin | 0 | 0.0e0 | 1.7e7 | 0.000 | 0.000 | 177 | 543 | 0 |
| cull-edge | 198 | 9.0e0 | 0.0e0 | 0.032 | 0.015 | 529 | 23 | 0 |
| extent | 120 | 8.4e0 | 0.0e0 | 0.110 | 0.036 | 557 | 43 | 0 |
| tie | 0 | 0.0e0 | 8.3e1 | 0.049 | 0.030 | 221 | 579 | 0 |
| far-graze | 0 | 0.0e0 | 2.6e5 | 0.000 | 0.000 | 762 | 204 | 0 |
| flat | 174 | 1.8e1 | 1.6e1 | 0.085 | 0.009 | 789 | 11 | 0 |
| degenerate | 309 | 5.5e1 | 0.0e0 | 0.039 | 0.040 | 361 | 159 | 0 |
| closed-edge | 683 | 1.2e1 | 0.0e0 | 0.046 | 0.015 | 360 | 340 | 0 |

**What the model does where the rule lets the answer vary** (each a test):

* **Seams** (`seam-edge`, 168 rays a cube, every one within 8 spacings of the seam):

  | Cube | Kind | A hit behind the seam admissible | A miss admissible | Model: the reference's side | The other side | Behind |
  |---|---|---|---|---|---|---|
  | two bottom levels, one translation | closed seam | 0 | 0 | 145 | 23 | 0 |
  | two bottom levels, one rotation and translation | closed seam | 0 | 0 | 145 | 23 | 0 |
  | one bottom level, plain and mirrored | open seam | 168 | 165 | 130 | 38 | 0 |
  | two bottom levels, two translations | open seam | 168 | 159 | 137 | 31 | 0 |
  | a geometry transform | open seam | 168 | 161 | 137 | 31 | 0 |
  | two geometries, no transform | shared edge | 0 | 0 | 157 | 11 | 0 |

  The model never lets a ray through a seam, open or closed: it tests each triangle by one
  arithmetic, whatever structure the triangle is in. A miss is not admissible at every open-seam
  ray because some go on into the closed seam of the cube behind. A planted leak (every ray
  answered by the far side of its cube) fails all 504 rays of the two closed seams and the shared
  edge and none of the 504 of the open seams; a planted hole fails wherever a hit is certain.
* **Closed meshes**: of the 700 rays that start inside a mesh of `closed`, every one meets a back
  face of that mesh, by the reference and by the model; a miss is admissible on none, and none is
  within tolerance of an edge. Of the 700 of `closed-edge`, which start and point on a lattice, 17
  are ambiguous by an edge between two faces and 10 lie on one exactly; the same holds of every
  one, and a miss is admissible on none.
* **Extents** (`surface-rays`, 200 rays a row and column):

  | Bias, in tolerances | Rays that leave a surface: at the extent boundary | Rays that end on one: at the extent boundary | Model reports the surface's own triangle |
  |---|---|---|---|
  | 0 | 198 | 128 | 79 |
  | 1 | 200 | 130 | 71 |
  | 16 | 1 | 77 | 39 |
  | 256 | 0 | 2 | 1 |

  A ray that ends on a surface is at the boundary only when nothing is in front of that surface,
  and its tolerance grows with the way it has come and with its slant, so a bias of 16 tolerances
  at the surface still leaves 77 of 200 there. In `extent`, the 120 rays that start on a surface
  (t = 0 exactly) all go on in the model: the node test accepts t = 0, the traversal around it
  here asks for `tmin < t` as D3D12 does. What the title's own loops compare is not known
  (dxr-compatibility.md, "Extents").
* **An edge between a front and a back face** (`cull-edge`, 84 rays a cull): with no cull the model
  reports the front 42 times and the back 42; with back faces culled the front 47 times and a miss
  37; with front faces culled the back 48 and a miss 36; never a culled face. The rule admits all
  of it: the two do not show the ray one face, so the edge is a boundary.
* **Triangles of no area** (`degenerate`): the model's node test reports a hit on one on 14 of the
  210 rays that pass one within tolerance (its determinant rounds to zero only sometimes); the
  exact reference never does; both are admissible there.
* **Far triangles** (`far`, 300 rays a cluster):

  | Cluster | Unambiguous | Ambiguous | With a sliver or grazing pair | Largest weight tolerance among the unambiguous |
  |---|---|---|---|---|
  | 2^12 | 163 | 15 | 122 | 6.2e-2 |
  | 2^14 | 123 | 5 | 172 | 6.1e-2 |
  | 2^16 | 84 | 10 | 206 | 6.0e-2 |
  | 2^16 on three axes | 81 | 14 | 205 | 6.0e-2 |
  | 2^14 by an instance | 111 | 17 | 172 | 6.2e-2 |

  With edges of 2^-4 to 2^4 units, two rays in five at 2^12 and two in three at 2^16 meet a
  triangle whose weight tolerance is over 2^-4. The class validates nothing at any of the three
  magnitudes; `m_max`, the largest magnitude a validation under the rule covers, is therefore
  below 2^12 for triangles of these sizes (the 6.6e4 in the table above is the magnitude of the
  277 hits that were compared, in a class that validates nothing). In `large-thin` and `far-graze` every ray has such a
  pair, as those classes intend.
* **The tree against every triangle tested**: over the 27 classes the model's two answers differ
  on 437 rays (`overlapping` 157, `seam-edge` 104, `precision` 65, `flat` 49, six classes more);
  every one is a ray the rule calls ambiguous, or a yes-or-no ray on which both say yes.

**Level 3** (the classes dxr-validation.md draws; 128 pixels a side; the software model's picture
against the reference's):

| Class | View | Pixels | Hit | Flagged | Differ | Not flagged | Groups | Largest | Verdict |
|---|---|---|---|---|---|---|---|---|---|
| closed | 0 | 16384 | 1491 | 2 | 0 | 0 | 0 | 0 | pass |
| instancing | 0 | 16384 | 1903 | 6 | 0 | 0 | 0 | 0 | pass |
| far | 0 | 16384 | 236 | 32 | 0 | 0 | 0 | 0 | pass |
| far | 1 | 16384 | 175 | 118 | 0 | 0 | 0 | 0 | pass |
| far | 2 | 16384 | 801 | 575 | 0 | 0 | 0 | 0 | pass |
| far | 3 | 16384 | 574 | 655 | 0 | 0 | 0 | 0 | pass |
| far | 4 | 16384 | 453 | 176 | 0 | 0 | 0 | 0 | pass |
| seam | 0 | 16384 | 629 | 2 | 0 | 0 | 0 | 0 | pass |
| scale | 0 | 16384 | 1525 | 3 | 0 | 0 | 0 | 0 | pass |
| alpha-tested | 0 | 16384 | 1538 | 4 | 0 | 0 | 0 | 0 | pass |
| transformed | 0 | 16384 | 679 | 6 | 0 | 0 | 0 | 0 | pass |

No pixel differs: the model's errors are far below a step of a channel, and where it names the
other triangle at an edge the attribute is the same from both sides. A planted seam (one cube of
`seam` seen close at 96 pixels, every hit within 0.03 of the seam plane replaced by what is behind
it) gives 62 differing pixels in 11 groups, the largest of 10: a fail, and all 62 traced to rays
the reference had not flagged.

**Planted defects** (a test): swapped weights and a `t` a thousandth too long are out of tolerance
at level 1 and differ at level 2; turned faces are facing flips, and no failure once the records
carry no face; every hit a miss is lost hits; one hit everywhere is gained hits and flips; an
alpha test left out is filter flips; refusals are counted by reason and fail nothing until, with
the rays that have a sliver or grazing pair, they pass one ray in 16.

**A path inside the position tolerance** (a test, added in review). A third implementation, written
apart from the reference and the model: every triangle tested in `f64` in the instance's object
space by edge functions that give one number for an edge from either side, each coordinate of each
vertex moved by up to an eighth, and up to a quarter, of the position tolerance (`K 2^-23` times
the vertex's largest coordinate; one move for one vertex wherever its numbers appear, so that
shared edges stay shared). Every frame of every class at the documented seed; `agree` and
`admissible` are the counts of level 1:

| Class | Rays | Vertices exact: agree | admissible | Moved by 1/8 of the tolerance: agree | admissible | By 1/4: agree | admissible | Not accepted |
|---|---|---|---|---|---|---|---|---|
| static-soup | 2000 | 1989 | 11 | 1989 | 11 | 1989 | 11 | 0 |
| dynamic-refit | 4800 | 4776 | 24 | 4776 | 24 | 4776 | 24 | 0 |
| dynamic-rebuild | 4800 | 4783 | 17 | 4783 | 17 | 4783 | 17 | 0 |
| instancing | 1500 | 1481 | 19 | 1481 | 19 | 1481 | 19 | 0 |
| alpha-tested | 2000 | 1963 | 37 | 1963 | 37 | 1963 | 37 | 0 |
| overlapping | 1600 | 842 | 758 | 842 | 758 | 842 | 758 | 0 |
| transformed | 1600 | 1583 | 17 | 1583 | 17 | 1583 | 17 | 0 |
| hit-ordering | 1600 | 1600 | 0 | 1600 | 0 | 1600 | 0 | 0 |
| precision | 1735 | 293 | 1442 | 293 | 1442 | 293 | 1442 | 0 |
| masks-and-flags | 2000 | 1985 | 15 | 1985 | 15 | 1985 | 15 | 0 |
| single | 2048 | 2033 | 15 | 2033 | 15 | 2033 | 15 | 0 |
| closed | 1400 | 1397 | 3 | 1397 | 3 | 1397 | 3 | 0 |
| far | 1500 | 562 | 938 | 562 | 938 | 562 | 938 | 0 |
| seam | 1536 | 1522 | 14 | 1522 | 14 | 1522 | 14 | 0 |
| surface-rays | 1600 | 857 | 743 | 857 | 743 | 857 | 743 | 0 |
| scale | 1536 | 1529 | 7 | 1529 | 7 | 1529 | 7 | 0 |
| seam-edge | 1008 | 0 | 1008 | 0 | 1008 | 0 | 1008 | 0 |
| edge | 966 | 0 | 966 | 0 | 966 | 0 | 966 | 0 |
| thin | 720 | 132 | 588 | 132 | 588 | 132 | 588 | 0 |
| large-thin | 720 | 0 | 720 | 0 | 720 | 0 | 720 | 0 |
| cull-edge | 552 | 300 | 252 | 300 | 252 | 300 | 252 | 0 |
| extent | 600 | 120 | 480 | 120 | 480 | 120 | 480 | 0 |
| tie | 800 | 180 | 620 | 180 | 620 | 180 | 620 | 0 |
| far-graze | 966 | 0 | 966 | 0 | 966 | 0 | 966 | 0 |
| flat | 800 | 208 | 592 | 208 | 592 | 208 | 592 | 0 |
| degenerate | 520 | 309 | 211 | 309 | 211 | 309 | 211 | 0 |
| closed-edge | 700 | 683 | 17 | 683 | 17 | 683 | 17 | 0 |

No record of it is one the rule does not accept, at level 1 or 2. On 559 of its hits it reports
the other face than the reference, each on a pair whose face the rule does not compare (a sliver
thinner than the tolerance, or a grazing pair). While every sliver pair had a face such hits were
facing flips, and this test fails when that is put back. The reviewer's own tracer of the same
kind over 12 seeds of every class: 81 failures at an eighth and 231 at a quarter then, every one a
facing flip; none now (by hand).

**What the bound comes to.** The documented seed and the 200 that follow it, the random classes
(by hand, not a test; over all 27 classes, 6,633 frames and 8,363,007 rays no record of the model
was one the rule does not accept and the exact reference's answer was admissible on every ray; the
model's worst error was 0.266 of the tolerance in `t` and 0.123 in a weight, both in
`instancing`):

| Class | Rays | Ambiguous, of the rays without a sliver or grazing pair | One in | With a sliver or grazing pair | Pairs in range | Largest error for k | At seed + |
|---|---|---|---|---|---|---|---|
| static-soup | 402000 | 317 of 399200 | 1259 | 2800 | 109094627 | 1.360 | 200 |
| dynamic-refit | 964800 | 667 of 960085 | 1439 | 4715 | 283012540 | 1.849 | 146 |
| dynamic-rebuild | 964800 | 486 of 960733 | 1977 | 4067 | 90159034 | 1.929 | 2 |
| instancing | 301500 | 622 of 298701 | 480 | 2799 | 195518219 | 2.129 | 48 |
| alpha-tested | 402000 | 3150 of 398572 | 127 | 3428 | 43225979 | 2.110 | 22 |
| transformed | 321600 | 932 of 318875 | 342 | 2725 | 179898173 | 2.369 | 105 |
| hit-ordering | 321600 | 104 of 321600 | 3092 | 0 | 55958400 | 2.007 | 136 |
| masks-and-flags | 402000 | 799 of 400115 | 501 | 1885 | 136127172 | 1.922 | 92 |
| single | 411648 | 60 of 407231 | 6787 | 4417 | 403852 | 1.039 | 115 |
| closed | 281400 | 142 of 280486 | 1975 | 914 | 61773525 | 1.511 | 167 |
| far | 301500 | 7786 of 124912 | 16 | 176588 | 62693 | 0.012 | 51 |
| seam | 308736 | 298 of 306439 | 1028 | 2297 | 83521880 | 1.949 | 66 |
| surface-rays | 321600 | 148597 of 319493 | 2 | 2107 | 83073762 | 1.459 | 56 |
| scale | 308736 | 919 of 307357 | 334 | 1379 | 504922897 | 1.020 | 168 |

The bound is one in 1,024. Seven classes are inside it on average (`single`, `hit-ordering`,
`dynamic-rebuild`, `closed`, `dynamic-refit`, `static-soup`, and `seam` at one in 1,028), which
with 1,400 to 2,000 rays a seed still leaves a single seed on either side: at the documented seed
`static-soup` has 2 ambiguous rays of 1,991 and is `NotDecidable`, `dynamic-refit` has 3 of 4,779
and passes, and both verdicts are flagged sensitive. Five are over it by a factor of 2 to 8
(`masks-and-flags`, `instancing`, `transformed`, `scale`, `alpha-tested`); `surface-rays` is over
it by construction (it counts rays at an extent); `far` validates nothing (59 % of its rays have a
sliver or grazing pair). The arithmetic behind it: a ray that meets a triangle is within `b_tol` of
an edge about `6 b_tol` of the time, so the bound holds where `b_tol` is under about 1/6,000, which
with k = 8 is `h c` above `M / 85`. That is a property of the rule's numbers and of these scenes'
triangle sizes, found before any device was asked; what to do about it is the owner's decision
(dxr-compatibility.md, the end of "The rule").

As first built this table had `closed` at one ambiguous ray in 80, the worst of the classes that
count, and the documents gave it as the share "for closed meshes of small faces". It was the
generator's: the class's rays from inside a mesh lay on a lattice and went exactly through edges
(found in review; "Scenes", above). With random rays the class is at one in 1,975; the lattice rays,
now `closed-edge`, are at one in 42.

## Measured on the host, by the older comparison (seed `0x5EED20261127`)

The ten first classes, reference against software model, 23,635 rays; these tables are what they
were before the rule was built (the tests print them again, unchanged). Counts (`wide`: rays with a
wide permitted result, among the clear and ambiguous):

| Class | Frames | Rays | Clear | Ambiguous | Excluded | Wide | Match | Permitted, as reference | Permitted, other | By ids only | Defects | Beyond the band |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| static-soup | 1 | 2000 | 1999 | 1 | 0 | 0 | 1999 | 1 | 0 | 0 | 0 | 0 |
| dynamic-refit | 4 | 4800 | 4792 | 8 | 0 | 1 | 4792 | 8 | 0 | 1 | 0 | 0 |
| dynamic-rebuild | 4 | 4800 | 4793 | 7 | 0 | 0 | 4793 | 7 | 0 | 0 | 0 | 0 |
| instancing | 1 | 1500 | 1476 | 24 | 0 | 4 | 1476 | 24 | 0 | 3 | 0 | 0 |
| alpha-tested | 1 | 2000 | 1968 | 32 | 0 | 1 | 1968 | 32 | 0 | 1 | 0 | 0 |
| overlapping | 1 | 1600 | 845 | 755 | 0 | 1 | 845 | 535 | 220 | 1 | 0 | 0 |
| transformed | 1 | 1600 | 1552 | 48 | 0 | 14 | 1552 | 48 | 0 | 12 | 0 | 0 |
| hit-ordering | 1 | 1600 | 1599 | 1 | 0 | 0 | 1599 | 1 | 0 | 0 | 0 | 0 |
| precision | 1 | 1735 | 829 | 800 | 106 | 74 | 829 | 619 | 181 | 73 | 0 | 0 |
| masks-and-flags | 1 | 2000 | 1945 | 55 | 0 | 1 | 1945 | 45 | 10 | 0 | 0 | 0 |

Sizes, over the hits whose numbers are compared: what the model's tolerance lets pass, and what the
software path does (`t` as a share of its scale):

| Class | Hits | tol uv, median | largest | tol t, median | largest | Worst uv error | Worst t error | Worst t / tol | Worst uv / tol |
|---|---|---|---|---|---|---|---|---|---|
| static-soup | 1577 | 2.9e-5 | 7.0e-3 | 8.1e-6 | 1.2e-3 | 4.6e-5 | 1.7e-6 | 0.052 | 0.043 |
| dynamic-refit | 3710 | 3.6e-5 | 8.1e-3 | 8.3e-6 | 3.4e-3 | 3.6e-5 | 9.6e-7 | 0.039 | 0.054 |
| dynamic-rebuild | 2637 | 3.2e-5 | 5.8e-2 | 8.2e-6 | 9.0e-3 | 1.0e-4 | 7.3e-6 | 0.034 | 0.036 |
| instancing | 1219 | 3.3e-4 | 5.5e-2 | 4.0e-5 | 5.3e-3 | 3.4e-4 | 6.5e-6 | 0.025 | 0.026 |
| alpha-tested | 1412 | 3.6e-5 | 3.6e-2 | 1.5e-5 | 3.0e-3 | 1.2e-4 | 2.1e-6 | 0.036 | 0.046 |
| overlapping | 1107 | 5.9e-5 | 1.7e-2 | 1.1e-5 | 2.5e-3 | 1.0e-4 | 5.3e-6 | 0.037 | 0.038 |
| transformed | 1194 | 1.2e-3 | 5.0e-2 | 8.5e-5 | 6.9e-3 | 5.7e-5 | 1.4e-6 | 0.040 | 0.029 |
| hit-ordering | 1410 | 2.9e-6 | 4.9e-5 | 3.8e-6 | 1.9e-5 | 5.3e-7 | 1.7e-7 | 0.046 | 0.041 |
| precision | 1433 | 1.4e-5 | 6.1e-2 | 6.9e-6 | 3.8e-2 | 1.3e-3 | 3.9e-5 | 0.035 | 0.040 |
| masks-and-flags | 693 | 2.6e-4 | 3.4e-2 | 3.2e-5 | 3.9e-3 | 1.6e-4 | 3.4e-6 | 0.023 | 0.032 |

**What a pass of that comparison means.** The software path's hits on clear rays, one number moved
by a known amount: the share the comparison calls a defect, and in brackets the share it at least
sees (a defect, or beyond the band). By construction an error of 1/8 in `u` (or of the scale in
`t`) is a defect in every class and one of 1/64 is seen; a test asserts both. Below that:

| Class | u + 1e-5 | u + 1e-4 | u + 1e-3 | t + 1e-5 of its scale | t + 1e-4 |
|---|---|---|---|---|---|
| static-soup | 11.2 % (91.7 %) | 94.4 % (99.4 %) | 99.5 % (100 %) | 69.4 % (98.2 %) | 98.4 % (99.9 %) |
| dynamic-refit | 6.6 % (71.8 %) | 75.0 % (97.9 %) | 98.3 % (100 %) | 66.4 % (97.4 %) | 98.5 % (100 %) |
| dynamic-rebuild | 7.5 % (91.2 %) | 93.9 % (99.7 %) | 99.8 % (100 %) | 67.6 % (98.8 %) | 99.3 % (100 %) |
| instancing | 0.1 % (3.8 %) | 7.1 % (78.0 %) | 82.6 % (98.3 %) | 1.3 % (80.5 %) | 86.1 % (99.2 %) |
| alpha-tested | 24.5 % (68.7 %) | 72.3 % (94.3 %) | 95.2 % (99.6 %) | 39.8 % (93.0 %) | 94.9 % (99.6 %) |
| overlapping | 6.8 % (76.2 %) | 81.6 % (98.6 %) | 99.2 % (100 %) | 57.8 % (96.6 %) | 97.5 % (100 %) |
| transformed | 0.0 % (1.7 %) | 2.6 % (36.6 %) | 43.3 % (96.8 %) | 0.9 % (48.6 %) | 57.5 % (97.8 %) |
| hit-ordering | 73.1 % (100 %) | 100 % (100 %) | 100 % (100 %) | 92.1 % (100 %) | 100 % (100 %) |
| precision | 29.2 % (86.8 %) | 87.5 % (90.3 %) | 91.3 % (96.5 %) | 71.2 % (89.4 %) | 89.7 % (93.8 %) |
| masks-and-flags | 0.2 % (10.1 %) | 13.4 % (87.9 %) | 90.3 % (99.1 %) | 7.0 % (84.6 %) | 88.3 % (99.2 %) |

**Strongly scaled or far instances are `Useless` there, not passed** (a test; six instances of one
bottom level, 1200 rays; no defect among what is still compared):

| Scales up to | Moved by up to | Clear | Ambiguous | Excluded | Wide | Verdict |
|---|---|---|---|---|---|---|
| 2^1 either way | 2^6 | 1169 | 27 | 4 | 5 | Pass |
| 2^6 | 2^12 | 0 | 1189 | 11 | 1189 | Useless |
| 2^10 | 2^18 | 0 | 602 | 598 | 602 | Useless |

Of the 411 "other" results, 397 are on rays with an exact tie, edge, vertex or range end among
their candidates (220 `overlapping`, 177 `precision`), 10 on first-hit rays (`masks-and-flags`),
and 4 in `precision` have nothing exact about them: rounding alone. One is within 2e-8 of an edge.
In two, `tmin` is one `f32` step below the hit and the software path skips it for the next; in one,
`tmax` is one step above and it misses. **The software path can lose a hit that is one step inside
the range**: its `t` is a rounded quotient. Both ways of judging permit both answers there (under
the rule: the extent kind), so a native path that keeps such a hit differs admissibly.

The seventeen classes added for the rule, by the same comparison (frame 0, reference against
software model): no defect in any (a test).

| Class | Rays | Clear | Ambiguous | Excluded | Wide | Match | Permitted, as reference | Permitted, other | By ids only | Defects | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|
| single | 2048 | 2047 | 1 | 0 | 0 | 2047 | 1 | 0 | 0 | 0 | Pass |
| closed | 1400 | 1400 | 0 | 0 | 0 | 1400 | 0 | 0 | 0 | 0 | Pass |
| far | 1500 | 1489 | 11 | 0 | 3 | 1489 | 11 | 0 | 3 | 0 | Pass |
| seam | 1536 | 1533 | 3 | 0 | 1 | 1533 | 3 | 0 | 0 | 0 | Pass |
| surface-rays | 1600 | 986 | 614 | 0 | 0 | 986 | 546 | 68 | 0 | 0 | Fallback |
| scale | 1536 | 1534 | 2 | 0 | 0 | 1534 | 2 | 0 | 0 | 0 | Pass |
| seam-edge | 1008 | 127 | 881 | 0 | 0 | 127 | 724 | 157 | 0 | 0 | Useless |
| edge | 966 | 127 | 743 | 96 | 0 | 127 | 690 | 53 | 0 | 0 | Useless |
| thin | 720 | 201 | 519 | 0 | 117 | 201 | 519 | 0 | 116 | 0 | Useless |
| large-thin | 720 | 720 | 0 | 0 | 0 | 720 | 0 | 0 | 0 | 0 | Pass |
| cull-edge | 552 | 300 | 252 | 0 | 0 | 300 | 251 | 1 | 0 | 0 | Fallback |
| extent | 600 | 120 | 480 | 0 | 0 | 120 | 478 | 2 | 0 | 0 | Fallback |
| tie | 800 | 180 | 620 | 0 | 0 | 180 | 517 | 103 | 0 | 0 | Fallback |
| far-graze | 966 | 611 | 259 | 96 | 0 | 611 | 211 | 48 | 0 | 0 | Fallback |
| flat | 800 | 143 | 158 | 499 | 0 | 143 | 126 | 32 | 0 | 0 | Useless |
| degenerate | 520 | 259 | 1 | 260 | 0 | 259 | 1 | 0 | 0 | 0 | Useless |
| closed-edge | 700 | 681 | 17 | 2 | 0 | 681 | 10 | 7 | 0 | 0 | Fallback |

The two ways differ most where the model's tolerance is small and the rule's is not: `far` and
`large-thin` are nearly all clear to the model (the triangles are far from the origin but the rays
start near them, and its bound follows the distances that enter the arithmetic) and nearly all
slivers to the rule, whose `M` is the largest coordinate.

A plain `f64` tracer, a third implementation, gives the reference's ids on every clear ray of the
ten first classes; planted defects (swapped weights, turned faces, dropped instance flags, ignored
masks, no alpha test, a stale structure) are caught. Three more seeds are a test (no defect, nothing
beyond the band). 1000 more of the ten classes, run by hand before the rule was built and not a
test (16,000 frames, 23,635,000 rays): no defect, no `Useless`, nothing beyond the band; worst
error 0.096 of the model's tolerance. Three properties of the existing node test came out of what
was followed up:
1. **`t` on slivers** follows the barycentric error (the last term of `tol_t`, and now of the
   rule's `t_tol`).
2. **Tight boxes leak.** The node test has no box growth and decides boxes and triangles by
   different arithmetic: a ray within rounding of an edge that lies on its triangle's box face can
   be turned away by the box though the triangle test gives it to that triangle, and it leaves a
   closed mesh by the far side. 7 of 40 seeds of `precision` have such a ray, all near 2^20 (a test
   keeps them). So the builder grows every box by 2^-20 of the largest distance a ray origin can
   have from it, and the traversal refuses origins beyond that reach.
3. **Triangles of no area can be hit**: the node test misses one only when its determinant rounds
   to exactly zero. Over 8 seeds of `precision` it returned 59 hits on line-shaped triangles; D3D12
   generates none. The older comparison excludes such rays; the rule admits either answer on them
   (the degenerate kind).

## What remains different from the rule and its procedure

The eleven differences this document listed before the rule was built, and where each went:
the tolerance, the missing `k`, what the reference knows, seams, grazing rays and slivers,
degenerate triangles, the bound and the kinds, levels 2 and 3 and the sensitivity are implemented
as the rule states them, with the seven corrections to its wording that building and reviewing
them required ([C1] to [C7] in dxr-compatibility.md); the exact truth was kept and the rule's
wording follows it ([C1]). What is still different, each with its reason:

1. **Two ways of judging are in the crate.** `compare` and its error model, band and verdicts
   (`Fallback`, `BaselineFail`, `Useless`) were kept, because the probe calls them
   (`crates/victory-rt-probe/src/adapter.rs`, which runs the first ten classes and judges by
   `compare` alone, not by the rule) and because the model is still the exact reference's first cut.
   They are not the rule, and a `Pass` of `compare` says nothing under it. Removing them is a
   decision for when no caller is left.
2. **A NaN vertex.** The rule takes a triangle with a NaN vertex as out. `Scene::validate` refuses
   a scene that has one, so no class holds one: writing such a triangle as inactive, with the
   indices kept aligned, is the host build's convention ([dxr-host-backend.md](dxr-host-backend.md)),
   and the builder of the software model's image has no inactive triangles. Nothing here tests it.
3. **Illegal rays** are a list (`generate::illegal_rays`) and a refusal (`Ray::validate`, and an
   error from `rule::evaluate`), not rays of the `degenerate` class: the reference and the model
   take legal rays only, and the screen that turns an illegal ray into a counted miss is the
   probe's and the layer's.
4. **Classes of dxr-validation.md that are not here**: `odd-id` (a scene here is build
   descriptions; the triangle id byte is the image builder's, always 9) and `inverse` (the ray
   record has no inverse direction): both are tests of the node test's operands, held by
   `victory-shader`'s tests. `refit`, `many-small`, `burst` and the large sizes of `scale` are cost
   classes: the reference tests every triangle against every ray. `masks`, `cull`, `alpha` and
   `instances` are this crate's `masks-and-flags`, `transformed`, `alpha-tested` and `instancing`
   under their older names; `flat` and `far-graze` are one class each. One class is here that the
   table there does not have: `closed-edge`, the lattice rays that were `closed`'s.
5. **Levels 2 and 3 are of records.** The probe's production containers store what their own code
   computes; here the consumed words are a function of the instrumented record. A difference
   between a container's stored words and `shade::consumed` of its record is the probe's to find.
6. **The title half of the rule** (the same rays twice in one submission; paired captures in tiles
   of 16 pixels against their own noise) is not here: it needs the title.
7. **The hybrid's bar** (bit for bit the CPU model of the emulated test with every triangle tested,
   on rays ambiguous by edge, seam, extent or tie) has its two halves here, `Rule::emulated` per
   pair and `traverse::trace` with `Boxes::AllVisited` per ray, and no function that judges a list
   by it: which of the bars the owner wants is open (dxr-compatibility.md, "The hybrid's bar").
