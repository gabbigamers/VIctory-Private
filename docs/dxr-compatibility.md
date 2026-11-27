# Guest ray tracing against DXR: semantics, the equivalence rule, host support

What the guest does, what DXR promises, what the native path does with
each difference, and what "functionally equivalent rendering" means.
Companion to [dxr-architecture.md](dxr-architecture.md); evidence for the
guest side is in [ray-tracing-research.md](ray-tracing-research.md); the
test procedure is in [dxr-validation.md](dxr-validation.md).

**Nothing was run on hardware; every verdict below is a reading of code
and specification, not a result.** "Guest" means the console instruction
*as this fork emulates it* plus what the title's programs do around it.
The emulation is built from public sources and unchecked against a console
(`emit/lower/bvh.rs:4-8, 46-47`), and most of what the programs do around
it is unknown. `spec:` is the DXR specification by line, `emit/` is
`crates/victory-shader/src/emit/`, `res:` is
`crates/victory-shader/src/resource.rs`; `av:` and `dexnom:` are forks,
`lab:` the reference notes.

Paths: **native**, a recognised region replaced by `RayQuery` over host
triangles; **hybrid**, `RayQuery` over one host box per guest triangle
pointer with the emulated triangle test in the shader; **software**, the
translated loop as today.

## The rule: functionally equivalent rendering (proposed)

The owner's decision of 2026-11-27: bit-identical numbers are not
required where functionally equivalent rendering can be demonstrated,
and what "demonstrated" means is to be written down before measuring
(`progress.md`, session log). This section is that writing. **It is a
proposal made on 2026-11-27, before any measurement, and not yet
accepted by the owner; every number in it is proposed.** Building its
reference on the CPU (`crates/victory-rt`, dxr-validation-reference.md)
and reviewing it showed seven places where the first wording did not
hold. They are changed below, each marked **[C1]** to **[C7]** and listed
with its reason at the end of this section: **changes proposed to the
owner with the rest, none accepted.** No device has run. Once accepted,
a change is a dated revision made before the results it affects are
read. **Demonstrated** means: for one workload class, on one device and
driver, a recorded test passes all three levels below. The rule is
written for the native path; the hybrid's bar follows it. The software
path keeps the old bar: equal to its CPU reference bit for bit.

**The reference.** On synthetic scenes: a CPU reference from the same
single-precision inputs, its own answer in exact arithmetic and its
tolerances and classes in double precision **[C1]**. It knows each
triangle's neighbours (two triangles share an edge when two of their
world-space vertices are the same points, exactly, whatever structures
they are in), bottom level, instance, surface (the instance's id and the
geometry's index; a material belongs to a surface) and the class's
filter. Only the host's error is under test. On the title: the software
path on the same device, on the same rays.

**Tolerance.** For a ray (origin o, direction d of length L) and a
triangle at reference distance t: M is the largest magnitude among the
coordinates of o and of the vertices, in world and in object space, and
t times L (t no further than the extent); c is the cosine between the
ray and the triangle's normal, as a magnitude; h is the triangle's
smallest altitude. L, c and h are taken in the instance's object space,
where the guest's test, the hybrid's shader and a device's intersector
work and t is the same number **[C2]**.

| Quantity | Tolerance |
|---|---|
| a position in space | `x_tol = k * 2^-23 * M` |
| a barycentric weight | `b_tol = 2 * x_tol / (h * c)` |
| the distance t | `t_tol = x_tol / (L * c) + b_tol * D` **[C3]** |

D is how far the depths of the triangle's vertices along the ray lie
from t, the farthest of the three, the depth taken along the direction
and along its largest axis. The emulated test, like any watertight one,
forms t as the mean of those depths weighted by the barycentrics, so a
weight off by e moves its t by up to e times D. For a triangle that lies
across the ray D is 0 and the first wording stands.

**k is computed, not chosen here.** A host test, on the CPU and before
any device, takes the random scene classes and, over pairs with c at
least 2^-6 and h times c at least 2^-10 times M, finds the largest error
of the emulated test's single-precision model (every triangle tested)
against the reference, in units of 2^-23 times M. The error of a pair is
the smallest x_tol at which its answer is inside the tolerances of the
table as they stand, t's with its term b_tol D **[C3]**. k is twice the
largest, rounded up to a power of two, written into the probe's output
and the record. The bar asked of the host is the emulation's own
accuracy with a stated margin. A count of roundings is not used: the
test rounds products of order M^2 and M^3 (`emit/lower/bvh.rs:457-480`),
and a count does not bound a difference of products. **A larger
tolerance is a weaker test**: more triangles near, more rays ambiguous,
larger errors passed. So k is never raised to fit a host; the host's
largest errors are reported against it and a host outside it fails.

**Measured on the CPU, 2026-11-27: k = 8**, over the fourteen random
classes at the documented seed and the 200 seeds that follow it
(1.83 billion pairs in range; run by hand). The largest error is 2.37
units, an error of t, at seed + 105 in the class of transformed
instances. **The sample decides the value, so the rule names it.** The
largest error of a sample grows with the sample, and here it crosses a
power of two inside it: the documented seed alone measures 1.41 units,
which gives 4, and so do 192 of the 201 seeds; 9 measure over 2 units
and give 8. The constant is the whole sample's. With 4 the margin over
the emulation's own largest error would be 1.7, not the stated 2. The
host test measures the documented seed and seed + 105 and fails unless
the two give 8 and the first alone 4. **k is a property of these
classes' triangles and rays** (their shapes, sizes, magnitudes and
slants), not a bound for every shape; what the same measurement gives on
a shape no class holds is under [C3] below. The table by class, the test
that pins the value and the result file's line that carries it are in
dxr-validation-reference.md.

A pair with c below 2^-6 is **grazing**, one with b_tol above 2^-4 a
**sliver pair**; neither has stable numbers on either side. b_tol is
unbounded as c or h goes to zero and says nothing of such a pair, nor of
a triangle of no area, so these are placed by distance **[C4]**: out
when the ray, inside its extents, passes the triangle (as a set of
points) by more than 2 x_tol, which is where a weight of -b_tol puts the
ray of an ordinary pair; otherwise near, never in. A hit on one must lie
along the ray between the nearest and the farthest of the triangle's
vertices, widened by x_tol / (L 2^-6); its weights are not compared.
**The face** a pair shows the ray is a fact, compared and culled by, for
a pair that is neither grazing nor a sliver pair, and for a sliver pair
while its b_tol is at most 1/2 (h c at least 4 x_tol). Moving each
vertex by x_tol changes d . n by at most x_tol times L times the
perimeter, which is at most three longest edges, against L h c times the
longest edge: the face cannot turn while h c is above 3 x_tol, which is
b_tol under 2/3, and 1/2 is the power of two below that. A thinner
sliver pair has no stable face, as a grazing pair has none: it is not
culled by a facing flag and its face is not compared **[C4]**.

**Level 1: rays.** The reference classes every ray and triangle as **in**
(each weight above b_tol, t inside the extents by more than t_tol),
**out** (a weight below -b_tol, t outside the extents by more than t_tol,
a NaN vertex; a grazing, sliver or zero-area pair by its distance, above)
or **near** (the rest, and every grazing, sliver or zero-area pair that
is not out). A ray is **unambiguous** when no near
triangle and no filter decision can change the workload's answer: for
yes or no, there is an in-hit or nothing is near; for a nearest hit,
nothing near lies at or in front of the nearest in-hit within tolerance,
and the two nearest in-hits are more than their tolerances apart. Every
other ray is **ambiguous**, by kind: shared edge or vertex; **seam** (an
edge shared across bottom levels, instances, or geometries with
different transforms); boundary or silhouette edge; extent boundary (t
within t_tol of TMin or TMax, t = 0 included); coplanar tie; **filter
threshold** (the reference evaluates the class's filter at the corners,
edge midpoints and centre of the tolerance box of its inputs, and the
decisions differ; a box as wide as a cell of the filter is at a
threshold whatever they say); degenerate.

* Unambiguous rays: hit or miss, instance, primitive and facing agree
  exactly; t within t_tol; each weight within b_tol.
* Ambiguous rays: the answer is **admissible**. A hit is admissible on a
  triangle that is in or near and not behind an in-hit by more than
  t_tol, with numbers within tolerance of that triangle's reference
  values; at a filter threshold either decision is. A miss is admissible
  only when no triangle is in and every near one is near through an
  extent boundary, a filter threshold, a degenerate case, an edge with
  one triangle or two facing opposite ways for this ray, or a seam whose
  sides have different transforms. A miss is **never** admissible at an
  edge shared by like-facing triangles of one instance under matching
  geometry transforms (the scope of the specification's promise: two
  instances of one bottom level are a seam), nor at a seam whose sides
  see one object-space ray in the guest (the same instance transform, no
  geometry transform): the guest's test closes both
  (`emit/lower/bvh.rs:41-44`, per triangle, whatever structure it is
  in), the specification promises only the first (`spec:2404-2409`), so
  a device must show the second. **Nor is a hit behind such an edge
  admissible [C5]**: two near triangles that each are near through that
  edge alone, lie on opposite sides of it as the ray sees them and show
  it the same face (the face the path reports: object space, the
  instance's flag applied) cover the ray between them, and count as a
  certain hit like a triangle that is in. A fan closes a shared vertex
  the same way. So does **one** near triangle whose neighbour across
  such an edge is out by its own tolerance (the two tolerances differ
  with the triangles' sizes; the neighbour is an ordinary pair that no
  flag or mask takes away, out by a weight alone), where the ray is
  inside the near triangle, exactly: the neighbour cannot be hit, the
  near triangle is the one answer, and a miss beside that edge is a
  hole. Where the ray is not inside the near triangle it passes beyond
  the neighbour's reach, and the edge is a boundary for it. A fan with
  such members closes its vertex likewise, where the ray is inside one
  of its near members.
* Two records are neither **[C6]**: one a shader refused, which is
  counted by reason and is no miss and no failure, its ray being traced
  by the next path and judged there; and a hit that carries no face (the
  hybrid's), which is the hit of its triangle with the face not compared.
* The bound: in a class of random rays, among rays with no sliver or
  grazing pair, at most one in 1,024 is ambiguous, all kinds together,
  each kind in its own column. A class over the bound is "not decidable
  at this tolerance", which is not a pass. Rays with a sliver or grazing
  pair are judged by admissibility and counted apart; a random class
  where more than one ray in 16 has one or was refused, the two together
  **[C6]**, validates nothing. **So does a random class that judges too
  little [C7]**: one with fewer than 1,024 rays without a sliver or
  grazing pair, on which a bound of one in 1,024 cannot be counted (no
  ray, three rays), and one in which fewer than one ray in 4 is an
  unambiguous hit whose record agreed (rays that meet nothing hold a
  path to nothing). A list that holds a ray D3D12 forbids, or a scene
  that is not valid, is not judged: it gets an error and no verdict.
  Classes aimed at edges, seams, ties, slivers and boundaries are judged
  by admissibility and never validate a workload class alone.

**Level 2: what shading consumes.** What the program stores after the
region is compared. Integers and booleans are equal on unambiguous rays
and admissible on the others; floats are within the tolerance carried
through that code's arithmetic (for the synthetic programs a hit point
on the ray within t_tol of the reference's, which is x_tol / c in space
where the triangle lies across the ray and the instance does not scale;
an interpolated attribute within b_tol times its range over the
triangle, each of the three weights being within b_tol). The synthetic
function is fixed in dxr-validation-reference.md ("Level 2"). **On the
title the same rays are compared, never two runs**: in a comparison run each trace dispatch of the class executes
twice in one submission on the same inputs, the candidate first with its
stores redirected to scratch copies of the pages it writes, then the
software variant as always (unbuilt; dxr-validation.md). Output layouts
are unknown, so records are compared whole. A yes-or-no workload: at
most one record in 1,024 of a dispatch differs, and no more than 2
touching records in the dispatch's thread grid. Any other workload
differs in the last bits of whole records by design, so only the count
and the largest group are reported: **title level 2 does not bound
those workloads**; the verdict says so and level 3 carries it.

**Level 3: pictures.** Synthetic: each scene is drawn at 1,024 by 1,024
from the hit data alone by a fixed function (dxr-validation-reference.md,
"Level 3"): the material colour of the surface hit, darkened on a back
face, with a ramp of the distance and an attribute interpolated by the
weights, 8 bits a channel. A pixel differs
when a channel differs by more than 2 of 255. Pass: at most one pixel in
1,024 differs, each from a ray the reference flagged, and no group of
touching pixels (8 neighbours) is larger than 2. A picture of no pixels,
or one whose reference drawing shows no surface, is refused, not passed
**[C7]**. Title: the owner's
paired captures, same place, time of day and settings, software against
the candidate, at least three pairs a scene and three software pairs as
noise. In tiles of 16 by 16 pixels each tile is compared with **its
own** noise: n is its largest mean absolute difference per channel over
the noise pairs, p its largest single-pixel difference. A pixel differs
when it exceeds p by more than 8 of 255. Pass: no tile's mean exceeds n
by more than 2 of 255; at most one pixel in 1,024 differs; no group of
touching differing pixels is larger than 2, so a seam one pixel wide
fails whatever its tile's mean. A tile whose n is over 8 is reported as
not judged: noise that large can hide a difference. A failure is looked
at by the owner and either named in the ledger as an accepted class or
counted.

**The constants** (the factor 2 in k, 2^-6, 2^-4, one in 1,024, one in
16, one in 4 **[C7]**, 2 and 8 of 255, groups of 2, 16-pixel tiles) are
arbitrary: none is derived. Each verdict is recomputed with each
constant halved and doubled, and one that changes is flagged
`sensitive`. Not varied: the range of the measurement of k (c at least
2^-6, h times c at least 2^-10 times M), which is this section's own;
the 2 in 2 x_tol **[C4]**, which follows from b_tol; and the 1/2 past
which a pair has no face **[C4]**, which follows from x_tol. The floor
of 1,024 rays **[C7]** is the bound's own number and moves with it.

| Accepted | Not accepted |
|---|---|
| a number within its tolerance | a lost hit on an unambiguous ray, at an edge shared by like-facing triangles, or at a seam with one object-space ray: a hole, leaked light |
| either triangle at a shared edge, vertex or seam when both belong to one surface and shade the same | a gained hit on an unambiguous ray: a false shadow |
| either answer at a boundary or silhouette edge, at an extent boundary (a hit at exactly t = 0 included), or at a seam with different transforms, within the bound | a different surface at a tie or an edge where the two shade differently, in more rays than the bound or in a group |
| either decision of a filter on a candidate at a filter threshold, within the bound | a filter decision that differs on a candidate not at a threshold |
| either of two coplanar triangles of one surface and material | a whole surface wrong: facing, mask, flag or instance mapped wrongly |
| the facing of a grazing ray, and of a triangle thinner than the tolerance (b_tol over 1/2) | two identical runs that differ (`spec:2053-2056`) |

**The hybrid's bar.** The hybrid exists for the guest's own intersection
semantics, so it is asked for them. Unambiguous rays are judged as
above. On rays ambiguous by edge, seam, extent or tie its answer must
equal, bit for bit, the CPU model of the emulated test on the same
vertices with every triangle tested; those rays are outside its bound.
Whether the owner wants this bar, the rule alone, or equality with
software is open; this is the default meanwhile.

**On a difference that is not accepted**: the class is recorded as failed
for that path on that device and driver, and every region of that class
runs the next path: the hybrid where its conditions hold and it has
passed, otherwise software. The change is counted and logged. No effect
is switched off, shortened or replaced by an estimate.

**Changes to the first wording, proposed to the owner (2026-11-27; none
accepted).** Each came from building the rule's reference and judging
the CPU model of the software path under it, or from the review of that
reference by a second implementation written for it (host tests; no
device). Where the review corrected what was first built, the item says
so.

* **[C1] The reference's own answer is exact, not double precision.** An
  input is an integer times a power of two, so which triangle is nearest,
  whether a hit is on an edge and whether t is exactly an extent can be
  decided without rounding, and are. The tolerances and the classes stay
  in double precision: a threshold is not sharper than the constants
  behind it.
* **[C2] L, c and h are those of object space.** The first wording did
  not say. Under an instance that scales unevenly the cosine and the
  altitude differ between the spaces; the tests under judgement work in
  object space.
* **[C3] t_tol carries the weights' tolerance times the spread of the
  vertices' depths.** Without the term the emulated test itself is out
  of tolerance on thin triangles met at a slant: host test, `precision`
  class, the documented seed + 3, ray 1661, a sliver with a weight
  tolerance of 0.04: its t is 2.1 tolerances off with its weights within
  0.004 of theirs (six such rays in 12 seeds). The rule's bar is the
  emulation's own accuracy with a margin, so the tolerance has to hold
  what the emulation's formulation does with weights that are within
  theirs. With the term: none in 201 seeds (8.36 million rays), and
  the model's largest error in t is 0.27 of the tolerance.
  **k is measured against this tolerance** (corrected in review: as
  first built the measurement still read t against the first wording's
  x_tol / (L c), a tolerance the rule no longer had, and that t part
  carried all of it: 2.16 units at the documented seed, where the
  tolerance as it stands measures 1.41). How much the reading matters
  shows on a shape no class holds, a needle 384 to 512 times as long as
  it is high, met at a slant and wholly inside the measured range: 4.3
  to 4.7 units by the first wording, which would have made k 16, and
  0.01 by the tolerance as it stands (a host test).
* **[C4] Grazing, sliver and zero-area pairs are placed by distance.**
  By the first wording a grazing pair was out only by a weight below
  -b_tol, and b_tol is unbounded there: every triangle whose plane the
  ray nearly parallels, anywhere in the scene, was near, and every ray a
  grazing ray. A zero-area triangle was out by definition and yet a kind
  of ambiguity: the emulated test can report a hit on one (its
  determinant rounds to zero only sometimes; 14 of 210 rays that pass
  one in the `degenerate` class), and a device that transforms the
  vertices first may no longer see it as one. **The face of a triangle
  thinner than the tolerance is not a fact either** (corrected in
  review: as first built the face of every sliver pair was compared and
  culled by). A path with each vertex moved by an eighth of the position
  tolerance then failed, on faces alone: 81 rays in 12 seeds of the
  classes at an eighth, 231 at a quarter, and no failure of another
  kind. With the face compared only while b_tol is at most 1/2: none.
* **[C5] Nothing behind a closed edge is admissible.** The first wording
  forbade a miss there and, read literally, admitted a hit on what is
  behind: the two triangles are near, so neither is an in-hit that
  hides it. That is the hole the rule exists to refuse, showing the
  surface behind instead of nothing. **And the neighbour that closes an
  edge need not itself be near** (corrected in review: as first built
  the neighbour was looked for among the in and near triangles only).
  Of two triangles of unequal size the larger one's weight tolerance
  reaches further from their edge than the smaller one's, so a ray
  inside the larger one, up to 1.5 units from the edge in the case
  tried, was beside a "boundary" and a miss was admissible there.
* **[C6] A refused record and a hit without a face are outcomes of their
  own.** A shader that cannot decide a ray writes a refusal
  (dxr-shader-emission.md); it is counted by reason, is neither a miss
  nor a failure, and leaves its ray unjudged for that path. Refused rays
  and rays with a sliver or grazing pair are the rays a path was not
  held to by numbers, and **the share of one ray in 16 is for the two
  together**, a ray that is both counted once (corrected in review: as
  first built each was held to one in 16 apart, which is one ray in 8).
  The hybrid's records carry no face; its hits are judged as the
  triangle's, the face not compared.
* **[C7] A class that judges nothing validates nothing** (from the
  review). The first wording gave a random class a pass for no failure
  and few ambiguous rays, and so gave one to an empty list, to three
  rays, and to 1,600 rays aimed past every triangle and answered with
  misses: what a broken harness produces. Two floors. At least 1,024
  rays without a sliver or grazing pair: the bound's own number, since
  one in 1,024 cannot be counted on fewer; no new constant. At least one
  ray in 4 an unambiguous hit whose record agreed: a new arbitrary
  constant, halved and doubled with the others. A class over the bound
  stays "not decidable" whatever its size. The largest magnitude a pass
  covers (`m_max`, dxr-validation.md) is that of those unambiguous hits
  alone; rays with a sliver or grazing pair, whose numbers are not
  compared, add nothing to it and their magnitudes are reported apart
  (as first built they counted, and a class could pass and report a
  reach of 2^20 on triangles it had checked near the origin). A list
  with an illegal ray or a scene that is not valid, and a picture of no
  pixels or of no surface, get an error and no verdict.

**What the host tests say of the bound, for the owner's decision.** A
ray that meets a triangle is within b_tol of one of its edges about six
times b_tol of the time (three edges, both sides), so with k = 8 the
bound of one ambiguous ray in 1,024 holds only where b_tol stays under
about 1/6,000: h times c above about M / 85. Over 201 seeds of each
random class (run by hand; CPU only) the reference counts, among rays
without a sliver or grazing pair, one ambiguous ray in 1,028 to 6,787
for seven classes (one triangle; soups a few units across within 60
units of the origin; layered sheets; the closed meshes; the seam cubes),
which is inside the bound on average though one seed of 1,400 to 2,000
rays falls either side of it; one in 330 to 500 behind rotated and
scaled instances, with instance masks and in a soup of 2,048 triangles;
one in 127 with a filter of 64 cells a triangle; and at 2^12 and beyond
59 % of the rays have a sliver pair (dxr-validation-reference.md, "What
the bound comes to"). By the rule six of the fourteen random classes are
"not decidable at this tolerance" on average or validate nothing, and a
seventh, `surface-rays`, is over the bound by what it is made to count;
this on the CPU, with a model that makes no error the rule refuses,
before any device is asked. (As first built the closed meshes were among
them, at one ray in 80: that number was the generator's, whose rays from
inside a mesh lay on a lattice and went exactly through edges. Those
rays are a directed class now, and the closed meshes are at one in
1,975.) Whether the bound, the shares or the classes' triangle sizes
should move is the owner's decision; nothing was moved to make a class
pass.

## Semantics, from the native path's side

Standing: **exact** (the same meaning, by construction); **accepted**
(differs only within the classes above, to be shown per device);
**side table** (reproduced with data written at the host build); **not
native** (the workload goes to the path named).

| Guest behaviour | DXR | Native path | Standing |
|---|---|---|---|
| **Distance.** Two floats, numerator and denominator, in the emulation's own scale; a miss is (+inf, 1, 0, 0) (`emit/lower/bvh.rs:46-47, 513-522`) | a float t; precision not stated (`spec:7327, 7344-7355`) | (t, +1) or (-t, -1) by facing: every quotient and sign the code forms is the host's t, exactly | accepted (rounding). **Not native** for code that uses the denominator's size, compares raw numerators of two triangles, or mixes them with guest-scale values: hybrid |
| **Barycentrics.** Two numerators chosen by id bits, unnormalised, carrying the denominator's sign (`emit/lower/bvh.rs:462-475, 493-512`) | weights of the build's second and third vertex, non-negative (`spec:2147-2149, 7756-7766`) | (x, y) on a front face, (-x, -y) on a back face, by the vertex order given to the build | exact in meaning, accepted in value; per hit a select and two sign flips. A selector of the first vertex is 1 - x - y: accepted |
| **Facing.** The denominator's sign; positive is a front face (`emit/lower/bvh.rs:40-41`) | clockwise from the origin, object space, left-handed, reversible per instance, untouched by the instance transform (`spec:4427`) | vertex order and the instance flag | exact, **once one open point is settled**: this repository tests a node's vertices in fixed order, the forks reorder them by the id bits first (research). By the forks' reading: no cost. By this repository's: a decoded node is built in node order and the id bits pick the weights per hit (a table read, or none if triangles are grouped into geometries by id byte); over geometry from build descriptions no table can be made, so a facing-dependent class is **not native** there once an odd id byte occurs: hybrid over decoded nodes, else software. Grazing rays: accepted |
| **Ordering and ties.** Children sorted by entry distance through a fixed network; what a loop does with equal t is unknown (`emit/lower/bvh.rs:26-30, 367-381`) | candidate order is the implementation's; TMax follows each commit, so a triangle at t equal to the committed t is not a hit (`spec:1163, 2087-2110, 2553`) | not reproduced | by the description's order field. *None* (yes or no; a minimum of t alone): exact. *Tie by visit* (`t < best` or `t <= best`, keeping the first or last among equals): accepted under the tie classes above. *Tie by key* of the candidate: **not native**, hybrid (procedural extents are inclusive, so equals are presented, `spec:2556-2562`). *Visit order* (a first hit kept with its distance or identity, an iteration or stack cap, order-dependent accumulation): **not native**, software |
| **Extents.** Triangles hit for t >= 0; no extent test in the instruction, the loop compares t; boxes inclusive at the extent (`emit/lower/bvh.rs:26-32, 44, 481-492`) | triangles only for TMin < t < TMax; TMin non-negative and at most TMax; +INF legal (`spec:2553, 2564-2565`) | from the form of the loop's compares. `t < e`: TMax = e. `t <= e`: TMax = the next float above e for a finite e >= 0; +INF stays +INF (one bit more is a NaN); -0.0 is taken as +0.0. A minimum `t > m`: TMin = m; none beyond the instruction's own: TMin = 0 | exact in form, accepted in value, **for a finite extent**. **At e = +INF an inclusive compare is not the host's meaning**: the emulated miss word is a candidate at t = +INF, `+INF <= +INF` accepts it, and the software walk keeps it as a hit (distance +INF, weights 0, front face, the missed triangle's pointer; a first-hit walk ends there). A ray that hits something is unaffected (a nearer hit replaces it); one that hits nothing leaves that "hit" or a miss by whether it came to test a triangle, which is the guest's box culling: **no native and no hybrid answer** for such a ray. Shown on the synthetic walks against the traversal model (dxr-region-recognition.md: 33 of 384 rays); the description carries the form (`miss_word`), and such rays are counted like infinite ones, the class not validated while a ray of that kind hits nothing. A loop that accepts a triangle without comparing its t to the extent its box calls got depends on the guest's boxes: **not native**, software. A hit at exactly t = 0, or within t_tol of an extent, is the extent-boundary class: accepted within the bound, counted. How many rays start or end on a surface is data, not something the recogniser can prove: the probe's `surface-rays` class and the title comparison count it, and a class over the bound fails native and goes to the hybrid |
| **Illegal rays.** For boxes a NaN extent bounds nothing and a negative one misses all; anything NaN misses every triangle; a zero direction misses every triangle (1/0, then 0 times inf: inferred, not run) (`emit/lower/bvh.rs:43, 357-360, 441-444`). The loop's own compare decides what a NaN extent does to triangles (`emit/lower/bvh.rs:44`) | TMin < 0, TMin > TMax or any NaN is undefined behaviour; nothing is said of a zero or infinite direction (`spec:2564-2572`) | screened before the trace, so the host never sees one: NaN origin or direction, zero direction, negative extent: a miss. NaN extent: a miss where the recognised compare is ordered, TMax = +INF where it is unordered or negated; an unrecognised predicate refuses the loop | exact for those, to be shown on the CPU and by the `degenerate` class, **with one exception found on the synthetic walks**: under an unordered or negated compare a NaN extent also lets the emulated miss word through (`!(+INF >= NaN)`), so a ray that hits nothing leaves a hit at +INF if it tested any triangle, as at an infinite extent under an inclusive compare (the row above). TMax = +INF is the software walk's answer only for a ray that hits something; the others have no native or hybrid answer, are counted, and leave the class not validated while there are any. The ordered case is shown: every ray stores a miss. **An infinite origin or direction has no native answer** (the emulation's is not a constant; by reading, one infinite direction component gives hits at t = 0): screened to a miss and counted; a non-zero count leaves the class not validated. Undefined behaviour that happens to work on a driver validates nothing: the probe never hands the host an illegal ray. Whether a root can be a triangle node is unknown |
| **Inverse direction and per-call extent.** The program's own operands; the box test reads only the inverse (`emit/lower/bvh.rs:110-115, 333-339`). The forks' triangle test reads it too (`av:crates/victory-shader/src/bvh.rs:244`) | none: a ray is origin, direction, TMin, TMax (`spec:6858`) | dropped when proven 1/direction | exact then. Not proven: **not native** and not hybrid; software |
| **Guest box culling.** Sixteen- or 32-bit bounds, a plain slab test, no outward rounding (`emit/lower/bvh.rs:22-32`) | a primitive the ray intersects is enumerated (`spec:1159-1163`) | not reproduced: a triangle the guest's boxes culled through rounding becomes a hit | accepted where such rays are near an edge or grazing; a gained hit on an unambiguous ray is not |
| **Masks.** "Id and mask" in the instance record; width and test unknown (`av:docs/fork-ray-tracing.md:74-76`) | 8 bits, ANDed with the ray's low 8; zero never matches (`spec:4401, 6857`) | copied at the top-level build | exact for 8 bits or fewer tested by AND. Otherwise **not native**: hybrid, which sees every candidate |
| **Instance transforms.** A 3x4 matrix; the title's code transforms the ray; the instruction does not read instance nodes (`emit/lower/bvh.rs:21`) | a 3x4 instance-to-world matrix; the implementation transforms the ray; t is the same parameter in both spaces (`spec:4399, 7344-7355`) | the host's transform | accepted (rounding). Which matrix the guest stores is unknown. A loop that renormalises the direction per instance keeps a different t: its live-out must be recognised or the loop refused |
| **Per-instance guest code** beyond transform, mask and culling (a distance or level-of-detail test, a change to the ray); whether any loop has it is unknown | none: control returns to the shader only for candidates (`spec:1153-1179`) | none | **not native**. Hybrid only where the code is a pure skip-or-enter test of the instance record that can be re-run per candidate from the instance index; a change to the ray: software |
| **Flags.** Instance flags and id-byte bits beyond the barycentric choice, "opaque and the like"; meanings unknown (`av:crates/victory-shader/src/bvh.rs:634`) | instance, geometry and ray flags; an undefined ray-flag bit is undefined behaviour (`spec:4294-4303, 4408-4427, 6856`) | proven constants in the `RayQuery` template | exact for a proven meaning. An unproven one: **not native**, hybrid, where the title's own test of the flag runs per candidate; software where the flag steers the walk itself |
| **Culling.** The loop's own code on the denominator's sign | cull ray flags; off per instance by a flag (`spec:5455-5469, 4408-4427`) | a proven "skip this facing" becomes the flag | exact given facing. Unknown: which triangle a driver reports on an edge shared by a front and a back face under a cull flag; the specification is silent, the probe asks |
| **Shared edges and seams.** The emulated test counts edges and vertices as inside, per triangle, whatever structure it is in: both triangles hit; a zero-area triangle misses (`emit/lower/bvh.rs:41-44, 465-473`) | exactly one of the incident triangles; which one may vary; watertight **only inside one bottom level, for geometries with matching transforms** (`spec:2378-2382, 2404-2409, 2439-2441`); a degenerate triangle never hits (`spec:2235-2236`) | the host's rule | accepted inside that scope (either triangle of one surface). Across bottom levels, instances or geometry transforms nothing is promised: the `seam` classes ask the device, and a miss at a seam with one object-space ray fails the class. A loop that needs both hits: **not native**; hybrid if its per-candidate action is idempotent, else software. Zero area: exact |
| **Per-candidate work.** Where an alpha test sits is unknown; one fork comment puts material tests after a candidate (`dexnom:crates/victory-shader/src/emit/intersect.rs:299-300`) | a non-opaque triangle candidate returns to the shader, already the closest so far; the shader commits or not (`spec:1166-1179, 7207-7215`). Repeats are excluded for any-hit and intersection shaders by a geometry flag; nothing is said for `RayQuery` candidates (`spec:692-704, 2134-2144, 4303`). Order and repeats may change with every build input (`spec:2087-2110`) | the filter is the loop body; opaque geometry never returns (`spec:4301-4302`); every other geometry is built with the no-duplicate flag | exact in structure when the decision depends on the candidate alone; in value the decision can flip at a filter threshold (accepted within the bound). Data the filter reads from the guest node: side table, per candidate. A filter that counts or accumulates: **not native and not hybrid**, software, unless the recogniser proves the action idempotent (a minimum, a maximum, an OR). A zero count of repeats on the probe's scenes licenses nothing: repeats depend on how a driver boxed that geometry |
| **Early-out.** Which loops stop at a first hit is unknown | a ray flag ends the search at the first hit (`spec:5460, 1175-1176`) | the flag, for a yes-or-no loop | exact for yes or no. A loop that keeps the first hit's distance or identity: **not native**, software |
| **Identifiers, instance.** The loop's own state: values it loads from the instance record, or the record's address | instance index, a 24-bit id and 24-bit contribution, which are fields of the title's own instance description (`spec:4381-4383, 7534-7576`) | the top level is built from the title's instance descriptions; their id and contribution come back as given | exact for the title's own id and contribution, no cost. A guest record address or index: side table by instance index, one read per committed hit, and the guest top level must exist; the builder reorders (`lab:GHIDRA-BVH.md:55, 63`), so guest order is not assumed |
| **Identifiers, triangle.** The node pointer the loop passed in; the instruction returns no id (`res:1370-1372`). Whether a node stores geometry or primitive ids is unknown | geometry and primitive index, autogenerated; inactive ones counted (`spec:2192-2198, 7583-7608`); vertex positions not readable at this shader model (`spec:7788-7794`) | a table by (bottom level, geometry, primitive) | side table, one read per committed hit. From build descriptions the mapping to guest nodes is not known (the builder reorders): then hybrid over decoded nodes, else software |
| **Loads after a hit.** Unknown: whether later code reads the node or the instance record by an address from the hit | none | left as guest code, given the exact guest pointer | side table, and the guest structure must exist: such a workload forbids leaving out the guest build |
| **Triangle nodes.** Up to four triangles over five vertices; which exist is known only from parents (`emit/lower/bvh.rs:33-36`) | none | not used when geometry comes from build descriptions | n/a; a decoded source walks pointers from the root |
| **Packed static structures.** Shipped packed, unpacked on the GPU once (`av:docs/fork-ray-tracing.md:77-79`) | a structure comes only from a build (`spec:2001-2046`) | built from a rehydrate description's buffers if they hold the triangles (unverified), else decoded after the unpack | n/a for semantics; cost unmeasured (a fork counts 3,000 in the opening) |
| **Dynamic rebuild and refit.** Built or refitted every frame; a compacted size read back (`av:docs/fork-ray-tracing.md:80-81, 110-118`) | builds ordered by UAV barriers; a top level follows its bottom levels; an update moves vertices only, and no primitive may cross NaN (`spec:2019-2021, 2185-2189, 2286-2300, 2360-2367`) | a host build or update from the same description, at the same place in the list | exact in content; the size read-back stays the guest builder's |
| **NaN triangles, procedural geometry.** The builder knows a triangle "invalidated" by NaN and has a pass for procedural geometry (`lab:CONSOLE-DLL-EXPORTS.md:164-172`) | a triangle is inactive when a vertex's x is NaN; NaN elsewhere without it is undefined (`spec:2174-2176, 2199-2201`) | a NaN vertex is written as an inactive triangle | exact for NaN. Procedural nodes: **not native**, software; whether the title submits any is unknown |

What the table leaves standing:

* No row is hardware-validated. "Exact" is a claim about construction
  and "accepted" a claim per device and driver; the probe must show both.
* Three guest properties are not reproduced by design: visit order, the
  guest's own box culling, and the numerator's scale. A workload that
  depends on one of them is not native.
* The hybrid reproduces the triangle arithmetic, both hits on an edge or
  seam and hits at the extents, and sees every candidate; not visit
  order or the guest's box culling. Over decoded nodes it tests the
  guest's node bytes; over boxes from build descriptions it tests the
  title's input vertices: the same test, on what may be other bytes in
  another vertex order (dxr-architecture.md).
* Most rows turn on what the title's loops do. No loop has been read.

## Host support

Device words are those of [gpu-compatibility.md](gpu-compatibility.md).
The state of work uses the owner's words; **the layer is speculative on
every device: no path but software exists.**

The tier is what `D3D12_FEATURE_D3D12_OPTIONS5` reports, kept as a number
(`crates/victory-gpu/src/caps.rs:47-50`). `gpu-info` does not print the
number today, and a failed query reads as no ray tracing.

| Tier reported | In it | Native and hybrid |
|---|---|---|
| none, or 1.0 | no ray tracing, or pipelines without `RayQuery` (`spec:3210-3211`) | not available; software, which needs no ray-tracing hardware |
| 1.1 | `RayQuery` in any stage, compute included (`spec:1139-1141, 3212`) | the tier both need |
| 1.2, 2.0 (preview) | opacity micromaps, shader execution reordering and later additions (`spec:3213-3214`) | as 1.1; nothing beyond it is used |

"Node test" is the emulated instruction's tests in `cargo test`; no run
of `bvh_tests` is recorded by name for any device. "Traced picture" is
the title's ray-traced light in a run; it is absent on every host in this
fork (`README.md:51`). On a 32-lane host a tracer that declares 64-lane
waves has no software pipeline either (G2); which tracers do is unknown.

| Host | Tier | Node test | Traced picture, software | Native, hybrid | Evidence |
|---|---|---|---|---|---|
| NVIDIA Blackwell, RTX 5070 Ti (target) | not asked on this card | Expected | Expected; G2 if its waves are 32 lanes | speculative; no record | none: no `victory gpu-info` report recorded |
| NVIDIA Ada Lovelace | another tool printed 1.2 and waves of 32 to 32 lanes for an RTX 4070 Ti SUPER in a virtual machine | Expected. A fork line checked its own emulation on an RTX 4070; their code, nothing lit | Expected; G2 on that report | speculative; no record | `lab:docs/VM-GPU-RESULT.md:10-14`; `av:docs/README-fxceless.md:38, 137` |
| NVIDIA Ampere, Turing, later | not asked | Expected | Expected | speculative; needs tier 1.1 reported; nothing keyed to a model | none |
| AMD RDNA 2, 3 and 4 | not recorded | Expected | Expected; the forks' runs are of the forks' code | speculative; no record | gpu-compatibility.md; research |
| Intel Arc | not asked | Expected | Expected | speculative; no record | none |
| WARP | not asked | Expected; CI runs the suite, no result recorded | too slow for the world | not known whether it reports 1.1 | gpu-compatibility.md |

A device moves only by a recorded run of the probe on that device and
driver. A result holds for that driver: determinism is promised per
device and driver (`spec:2053-2056`), so a driver update empties the
record.

## What the layer may not assume

* That the native path equals the guest on any class before a recorded
  test on that device and driver says so.
* That a guest structure's bytes mean anything to the host, or the reverse.
* That two drivers, or two versions of one, agree on an edge, a tie, the
  last bit of t, or which candidates outside the guarantee they present.
* That a guest ray is a legal host ray, or a guest flag word a legal host
  one (`spec:2564-2572, 6856`).
* That a candidate is presented once, that a seam between two bottom
  levels is closed, or that a host box test presents everything the
  guest's rounding accepts.
* That a candidate a guest loop accepts is a hit: the emulated miss word
  is a candidate at t = +INF, which an inclusive compare accepts at an
  infinite extent and a negated one at a NaN extent.
* That a software pipeline exists to fall back to on this host, or that
  the software path is the console: it is one emulation of two.
* That any path is faster than another. Inline tracing gives the system
  less room to optimise (`spec:1145`); the only timings anywhere are a
  fork's, of its own software path, on one AMD card.
