# DXR validation: the reference half

`crates/victory-rt` is the half of the validation prototype for an experimental native DXR backend
that needs no GPU: synthetic scenes, an exact CPU reference, a CPU model of the existing software
BVH path, the comparison policy. Implemented, covered by host tests on Linux, type-checked for
Windows. **Nothing here has run on a GPU; no number below is a hardware or performance result.**
One test is built for Windows only and has not run: the one in `victory-shader` that holds the node
test here to that crate's own reference (see "The software model").

## Records (little-endian, fixed)

| Record | Words (32-bit) |
|---|---|
| Ray, 48 bytes | origin x y z (f32), tmin, direction x y z (f32, not normalised), tmax, instance mask (low 8 bits used), ray flags (`D3D12_RAY_FLAG` values), two reserved (0) |
| Hit, 32 bytes | t, u, v (f32; u and v the weights of vertices 1 and 2), kind (0 miss, 1 front, 2 back, 3 procedural), instance index, instance id (24 bits), geometry index, primitive index; a miss is all zero |

* **Alpha** (geometry not marked opaque): accepted iff `((floor(u*8) + floor(v*8) + primitive_index) & 1) == 0`.
  No clamp: a weight an implementation reports just below zero is in cell -1.
* **Front face**, D3D12's default: with `n = (v1 - v0) x (v2 - v0)` on the vertices after the
  geometry's transform and `d` the direction in object space, front iff `d . n < 0` (clockwise from
  the origin, left-handed). No instance transform enters, mirrored or not; a mirrored geometry
  transform turns the winding; the instance's counter-clockwise flag swaps front and back.
* A triangle hit needs `tmin < t < tmax`, both strict. Opacity: the geometry flag, then the
  instance's force flags, then the ray's. Rays D3D12 forbids are refused.

## Scenes

A scene is DXR build descriptions (`scene.rs`): bottom levels of triangle geometries (no, 16-bit or
32-bit indices; optional 3x4 transform; opaque flag), instances (3x4 transform, id, mask, flags),
build flags, and frames of changes that ask each structure for a refit or a rebuild. The generator
uses an integer PRNG, exact conversions and the four `f32` operations; a test pins its bytes.

| Class | Holds |
|---|---|
| `static-soup` | random triangles (no indices) and a 16-bit indexed grid, one instance |
| `dynamic-refit` | vertices move with topology kept (built for update), instances move; 4 frames |
| `dynamic-rebuild` | triangle counts, geometry lists and the instance list change; 4 frames |
| `instancing` | 48 instances of two bottom levels: rotations, non-uniform scales, mirrors, repeated ids |
| `alpha-tested` | large non-opaque triangles before an opaque backdrop |
| `overlapping` | one triangle repeated in a geometry, across geometries, across instances; coplanar overlaps |
| `transformed` | geometry transforms (one mirrored) under rotated, scaled, sheared, mirrored instances; facing culls |
| `hit-ordering` | stacks of non-opaque layers: accepted and rejected candidates interleaved |
| `precision` | rays exactly through edges and vertices of closed meshes, grazing rays, slivers, triangles of no area, coordinates near 2^20 (in vertices and by instance), tmin and tmax exactly on a hit |
| `masks-and-flags` | instance masks and flags against ray masks and every ray flag |

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
instance index, geometry, primitive: its own rule, since D3D12 leaves the choice open.

**What may be returned instead.** A model of `f32` arithmetic (next section) classes each pair as
ruled out, sure, doubtful or unstable. A ray's permitted results are the sure and doubtful
candidates that can come out nearest, and a miss if nothing is sure. Doubtful candidates that share
an edge (the same vertices bit for bit, one instance, matching geometry transforms, opposite sides
as the ray sees them) or close a fan around a shared vertex count as one sure hit: D3D12 requires
no gap there and leaves open which triangle is hit. A ray is **clear** (one permitted result),
**ambiguous** (several) or **excluded** (an unstable candidate, or over 16 results). Diagnostics:
edge margin, gap in `t` to the next accepted hit and which ids differ, distances of `t` from `tmin`
and `tmax`, alpha-cell margin, and the reasons as flags. The flags are about the ray's permitted
candidates, not about the reference's own hit: `edge-exact` can belong to a triangle behind it. What
is true of that hit alone is in the margins (the edge margin is 0 exactly when it is exactly on an
edge). The reference's records are the exact numbers rounded to `f32`; an exact zero is `+0`.

## Tolerance: the derivation

The model is an implementation working in the instance's object space in `f32`, unit roundoff
`u = 2^-24`. Each quantity carries a bound per component:
* a vertex after a geometry transform: `4u * sum|terms|` (three products, three sums); else 0;
* the ray through the inverse `W`: origin `(5 + 4k)u * sum|W|(|o| + |T|)`, direction
  `(4 + 4k)u * sum|W||d|`, with `k` the transform's condition number for an inverse computed in
  `f32`; a pure translation is one rounded subtraction, `u|o'|`; the identity is exact;
* `A = p0 - o'`: both bounds plus `u|A|`; an edge vector: its vertices' bounds plus `u|e|`;
* a product `da|b| + |a|db + da db`; a cross-product component two products plus
  `u(|p| + |q| + |p - q|)`; a dot product three products plus `3u * sum|a_i b_i|`.

This bounds `E1`, `E2`, `S`, `D` (and `E0`) to first order; every bound is multiplied by 4 for other
formulations (a sheared-space test, another order, fused multiplies). With `dX` the bound of `X`:
* edge functions of mixed sign beyond their bounds **rule the pair out**; otherwise `|S| <= 4 dS`
  makes it **unstable** (a ray in the plane, a sliver, no area);
* `tol_w = (dEk + |wk| dS) / (|S| - dS) + u` (a weight below `-tol_w` rules out); `tol_uv` is the larger of u's and v's;
* `tol_t = (dD + |t| dS) / (|S| - dS) + 2u|t| + sum_k tol_w,k * spread_k`, `spread_k` being how far
  vertex k's depth along the ray is from `t`. A watertight test forms `t` as the barycentric mean
  of the vertex depths, so its `t` carries the barycentric error times that spread. The host tests
  found this on slivers: without the term the software path was up to 93 tolerances off in `t`.

A candidate is **doubtful** when a weight is within `tol_w` of 0, `t` within `tol_t` of `tmin` or
`tmax`, or `8u`, `8v` within `8 tol_uv` of a cell boundary. 0 is a cell boundary: exactly, a weight
is never negative, and the software path's never is, but nothing says an implementation cannot round
one below zero on an edge, where the formula above turns its parity. An alpha-tested candidate at
such an edge is doubtful in its cell too, so it is not counted into a no-gap pair or fan: the
triangle that takes the edge may reject, and the other was never offered the hit.

The model bounds the stated arithmetic, not hardware: a hardware result outside it is a finding,
reported with its size. Two limits on what it is used for:
* **Wide.** On a sliver far from the origin the bound exceeds anything worth judging by: at the
  documented seed up to 8.7 in `u`, `v`, and on one such ray the software path itself is 0.23 off.
  A permitted result with `tol_uv > 1/16` (half a cell of the alpha pattern) or `tol_t` over 1/16
  of the distance to the triangle's farthest vertex (the scale `t` errors are reported against) is
  **wide**: its triangle and face are compared, its `t`, `u`, `v` are not, and it is counted apart.
* **Instances.** The `k` term allows an inverse computed in `f32` an error as large as the
  condition number permits, and it multiplies the distance from the world's origin. Behind rotated
  and scaled instances the tolerance is therefore some 10 to 40 times that of untransformed
  geometry (medians in the second table below), and with scales of 2^6 either way and positions in
  the thousands every compared ray has a wide result. A bound by the componentwise condition
  number might be tighter for scaled instances; that is not derived here and not done.

## Comparison policy (fixed before any hardware run)

`compare(class, diags, baseline, candidate)` judges two hit lists against the reference.
* **Exact**: hit or miss; instance index, instance id, geometry and primitive index; front or back;
  a miss is the all-zero record. **Within tolerance**: `t`, `u`, `v` against the exact values; for a
  wide result not at all, and it is counted as right **by ids only**. An ambiguous ray takes any
  listed result under the same rules; an excluded ray is counted, not judged. Anything else is a
  defect: missed, phantom, wrong triangle, wrong face, numeric, malformed.
* **The band**: a result within tolerance whose error is over 1/8 of it is counted as **beyond the
  band** and named on the verdict's line. It is not a defect (the model permits it) and does not
  change the verdict; it is an error larger than any the software path has shown on the host (worst
  0.096 of tolerance over the sweep below), to be looked at. The band is measured, not derived.
* **Verdict**: `Fail` (candidate defect), `BaselineFail`, `Useless` (fewer than 200 rays, under
  20 % clear, over 10 % excluded, or over 10 % with a wide permitted result), `Fallback` (no defect,
  but the two lists name different right results on rays that are not first-hit queries), `Pass`.
* **Needs the software fallback**: a `Fail` not traced to a translation defect; a `Fallback` where
  the title reads the differing field (which duplicate of a coplanar pair, which triangle at a
  shared edge, an alpha cell on a boundary, a hit exactly at `tmin` or `tmax`); a first-hit query
  read beyond hit or miss; geometry with triangles of no area that rays cross (property 3 below).
* The result file (`results.rs`): text; per class, seed and frame a line of eight hex words per
  ray and a checksum. A Windows run writes it; the rays are regenerated from class and seed, and
  each section carries the fingerprint of its generated case (every frame's geometry and instances,
  and the rays), which `Section::check` compares with the case generated on the reading side: a
  generator that differs between two machines shows as that, not as defects. Numbers are digits
  only; a repeated section, or a name the format cannot hold, is refused on writing and on reading.

## The software model

`node.rs` is the per-node test as `victory-shader` emits it (`emit/lower/bvh.rs`), operation for
operation. `console.rs` writes a scene as an image: the 64-byte triangle node and the 128-byte
32-bit box node as the test reads them, in four-wide trees; the top level, instance records, header
and the node fields the test does not read are **our own, synthetic, not any title's**, so nothing
here shows how a title's own traversal behaves. `traverse.rs` walks the image: a 64-entry stack,
nearest child first, `tmin < t < committed t` (of equal hits the first met stays), the alpha test,
the end at an empty stack or a first-hit ray's first accepted hit. Stack overflow, a bad pointer or
node type, the step limit and an origin beyond the padded reach are errors, never a quiet miss; each
has a test that reaches it (the stack at its 64th entry and its 65th, the build's refusal through a
lowered limit).

`node.rs` is a second copy of the CPU reference in `victory-shader`'s Windows tests
(`emit/bvh_tests.rs`), which those tests compare with the GPU. One test there,
`the_validation_crates_node_test_equals_the_reference`, compares the two copies word for word on
400,000 nodes and rays, so a change to the lowering cannot leave this model behind. It is built for
Windows only: type-checked here, and the same text run on this host against a copy of that
reference (passing, and failing on five planted differences), but not run as the test it is.

## Measured on the host (seed `0x5EED20261127`, reference against software model)

23,635 rays. Counts (`wide`: rays with a wide permitted result, among the clear and ambiguous):

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

Sizes, over the hits whose numbers are compared: what the tolerance lets pass, and what the software
path does (`t` as a share of its scale):

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

**What a pass means.** The software path's hits on clear rays, one number moved by a known amount:
the share the comparison calls a defect, and in brackets the share it at least sees (a defect, or
beyond the band). By construction an error of 1/8 in `u` (or of the scale in `t`) is a defect in
every class and one of 1/64 is seen; a test asserts both. Below that it depends on the class:

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

So behind instance transforms (`instancing`, `transformed`, `masks-and-flags`) an error of 1e-4 in
`u` or `v` mostly passes as no defect and one of 1e-3 often does in `transformed`; the band is what
reports them. **Strongly scaled or far instances are `Useless`, not passed** (a test; six instances
of one bottom level, 1200 rays; no defect among what is still compared):

| Scales up to | Moved by up to | Clear | Ambiguous | Excluded | Wide | Verdict |
|---|---|---|---|---|---|---|
| 2^1 either way | 2^6 | 1169 | 27 | 4 | 5 | Pass |
| 2^6 | 2^12 | 0 | 1189 | 11 | 1189 | Useless |
| 2^10 | 2^18 | 0 | 602 | 598 | 602 | Useless |

Every disagreement is classified by a test. Of the 411 "other" results, 397 are on rays with an
exact tie, edge, vertex or range end among their candidates (220 `overlapping`, 177 `precision`), 10
on first-hit rays (`masks-and-flags`), and 4 in `precision` have nothing exact about them: rounding
alone. One is within 2e-8 of an edge. In two, `tmin` is one `f32` step below the hit and the
software path skips it for the next; in one, `tmax` is one step above and it misses. **The software
path can lose a hit that is one step inside the range**: its `t` is a rounded quotient. The
reference permits both answers there, so a native path that keeps such a hit diverges, not fails.

A plain `f64` tracer, a third implementation, gives the reference's ids on every clear ray; planted
defects (swapped weights, turned faces, dropped instance flags, ignored masks, no alpha test, a
stale structure) are caught. Three more seeds are a test (no defect, nothing beyond the band). 1000
more, run by hand and not a test (16,000 frames, 23,635,000 rays): no defect, no `Useless`, nothing
beyond the band; worst error 0.096 of the tolerance; at most 5.4 % of a class's rays wide and 7.6 %
excluded, at least 46 % clear. Three properties of the existing node test came out of what was
followed up:
1. **`t` on slivers** follows the barycentric error (the last term of `tol_t`).
2. **Tight boxes leak.** The node test has no box growth and decides boxes and triangles by
   different arithmetic: a ray within rounding of an edge that lies on its triangle's box face can
   be turned away by the box though the triangle test gives it to that triangle, and it leaves a
   closed mesh by the far side. 7 of 40 seeds of `precision` have such a ray, all near 2^20 (a test
   keeps them). So the builder grows every box by 2^-20 of the largest distance a ray origin can
   have from it, and the traversal refuses origins beyond that reach.
3. **Triangles of no area can be hit**: the node test misses one only when its determinant rounds
   to exactly zero. Over 8 seeds it returned 59 hits on line-shaped triangles; D3D12 generates
   none. Rays near such a triangle are flagged `degenerate` and excluded from the comparison.

## Against the proposed rule for functional equivalence

[dxr-compatibility.md](dxr-compatibility.md) ("The rule: functionally equivalent rendering") proposes
what a native path must show, with its own tolerance; [dxr-validation.md](dxr-validation.md) is its
procedure. This crate was written in parallel with that rule, not from it, and **the two are not
reconciled**. This document says what the code does; the rule is what the owner is asked to accept.
Both are proposals, and no count or verdict of this crate is a result under the rule.

**What of the rule this crate implements**: its level 1 (rays), on synthetic scenes, on the CPU.
* `reference.rs`: a reference from the same single-precision inputs, every triangle tested, that
  classes each ray and triangle. The rule's in, out and near are here sure, ruled out and doubtful;
  its unambiguous and ambiguous rays are here clear and ambiguous; its admissible answers are here
  the permitted results. A miss is not permitted at an edge shared by like-facing triangles inside
  one bottom level, as in the rule.
* `compare.rs`: on a clear ray hit or miss, instance, primitive and facing exactly, `t` and the
  weights within a tolerance; on an ambiguous ray any permitted answer.
* Of what dxr-validation.md lists as to be built: the emulated node test outside the Windows-only
  test module (`node.rs`, tied to it by a test that has not run), a seeded generator, a builder of
  console-format nodes (`console.rs`; the top level is our own).

**Where it differs**:
1. **The tolerance is another one.** The rule: `x_tol = k * 2^-23 * M`, `t_tol` and `b_tol` from it by
   the ray's length, the cosine and the smallest altitude, `k` measured from the emulated test. Here:
   the bound of modelled `f32` arithmetic per pair, times 4, with a condition-number term behind an
   instance; no `k`, `M`, `c` or `h`. Neither is shown to bound the other.
2. **`k` is not computed.** The host test the rule asks for does not exist. What is measured here is
   the software model's error as a share of this crate's tolerance (worst 0.096), and the band of
   1/8 made from it is reported, not a verdict; the rule has no band.
3. **The truth** is exact arithmetic here, double precision in the rule.
4. **What the reference knows.** The rule: neighbours, bottom level, instance, surface, material and
   the class's filter. Here: no surface and no material; neighbours only as bit-equal shared edges
   and fans; one filter, the alpha pattern, judged by the distance of `8u`, `8v` from a cell
   boundary and not at the corners of the tolerance box.
5. **Seams.** The rule never admits a miss at a seam whose sides see one object-space ray (the same
   instance transform, no geometry transform), across bottom levels and instances. Here the no-gap
   rule holds inside one instance only, between geometries of matching transforms: at any seam
   across instances or bottom levels a miss is permitted. No class here is aimed at seams.
6. **Grazing rays and slivers.** The rule: a cosine under 2^-6 or `b_tol` over 2^-4 makes a pair
   near, judged by admissibility and counted apart; over one ray in 16 and a random class validates
   nothing. Here: an unstable pair excludes its ray, a tolerance over 1/16 makes the result wide
   (ids and face only), and a class is `Useless` over 10 % of either.
7. **Degenerate input.** The rule judges rays near a triangle of no area by admissibility and takes
   a NaN vertex as out. Here such rays are excluded and a scene with a NaN vertex is refused.
8. **The bound on ambiguous rays** (one in 1,024, each kind in its own column, else "not decidable")
   is not here: a class needs 20 % of its rays clear, and the reasons are flags (edge, tie, range,
   alpha, degenerate, wide), not the rule's kinds.
9. **Levels 2 and 3** (what shading consumes; pictures) are not here, nor the recomputation with
   each constant halved and doubled (`sensitive`).
10. **Verdicts and the baseline.** `Fallback` (two right answers that differ) and `BaselineFail` have
    no counterpart in the rule. The rule holds the software path to its CPU reference bit for bit;
    `compare` judges the baseline against the exact reference within tolerance, as it does the
    candidate.
11. **Classes and constants.** The ten classes are this crate's, not those of dxr-validation.md; the
    constants (4, 1/16, 1/8, 200 rays, 20 %, 10 %, 10 %) are another set than the rule's.

**Reconciling the two is the next step**, before any device run is read against either: one
tolerance, one set of names, classes and constants, and the test that computes `k`, for which
`node.rs` and the reference here are the two halves. Which side gives way on each point above is
the owner's decision and is not made here.
