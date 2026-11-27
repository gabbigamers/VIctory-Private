# Guest ray tracing against DXR: semantics and host support

What the guest does, what DXR promises, what DXR leaves to each
implementation, and what follows for a backend. Companion to
[dxr-architecture.md](dxr-architecture.md); evidence for the guest side is
in [ray-tracing-research.md](ray-tracing-research.md).

**Nothing was run.** "Guest" below means the console instruction *as this
fork emulates it* plus what the title's programs do around it. The
emulation is built from public sources and is not checked against a
console (`emit/lower/bvh.rs:4-8, 46-47`), and most of what the programs do
around it is unknown. `spec:` is the DXR functional specification by line
(the copy identified in ray-tracing-research.md). `emit/` is
`crates/victory-shader/src/emit/`.

Host paths in the last column: **A**, a loop replaced by a trace over host
triangles; **C**, a trace over host boxes (one per guest triangle pointer)
with the emulated triangle test run in the shader for each candidate;
**F**, C for one bottom level with the title's own top-level code kept.

## Semantics

| Guest behaviour | DXR guarantees | DXR leaves to the implementation | Consequence |
|---|---|---|---|
| **Hit distance form.** Two floats, a numerator and a denominator; the scale is the emulation's own. The program divides or compares them itself; which, is unknown (`emit/lower/bvh.rs:36-47`) | a float t for a candidate and for the committed hit (`spec:7327, 7344`). A procedural hit is committed as one float, which becomes TMax (`spec:1163-1164, 7226-7239`) | precision and rounding: no statement found. Whether a primitive beyond the committed TMax is still enumerated (`spec:1162-1163`) | A cannot be equal bit for bit by any guarantee. C computes the same two floats, but **a commit is a rounded float**: one rounded below the exact fraction can cull a nearer candidate, and any commit drops candidates a loop that keeps more than the nearest hit would test. Commit only in a loop proven nearest-hit, with the quotient rounded up; otherwise never commit, which gives up distance culling. Both forms are timed |
| **Barycentrics.** Two numerators over the same denominator, chosen by bits of the node's id dword (`emit/lower/bvh.rs:37-40`) | two floats, relative to the original vertex order of the build (`spec:2147-2149`) | precision | A needs the id bits mapped to a vertex order at build time, and still differs by rounding. C keeps the guest form |
| **Ordering and ties.** Entered children come back sorted by entry distance clamped at 0, through a fixed network; every box that contains the origin has key 0 and stays in network order, so visit order depends on the ray and on the guest box tree (`emit/lower/bvh.rs:26-30, 367-381`). What a loop does with two hits at equal t is unknown | the same input gives the same result on the same device and driver (`spec:2053-2064`). A primitive the ray intersects within the current extent is enumerated eventually (`spec:1159-1163`) | the order of candidates; it may change with primitive, box and instance order, flags, update history, device and driver (`spec:2087-2110`) | C does not walk the guest tree, so it cannot reproduce a winner decided by guest visit order (first or last among equals). C is eligible only for a loop proven independent of candidate order. Inferred, not shown: in a wave-uniform walk a ray's visit order may depend on the other lanes of its wave |
| **Extents and NaN.** Boxes: entry clamped at 0, the extent inclusive. Triangles: t >= 0 hits, no extent test (`emit/lower/bvh.rs:26-32, 44, 481-492`). A NaN extent bounds nothing: the same hits as an infinite one (`:357-359`). A negative extent misses every box but not a triangle. A NaN in the origin, or in the direction, misses every triangle (`:43, 419-473`). A NaN in the inverse alone misses boxes, except a plane the origin lies exactly on (`:340-351`), and no triangle | triangles hit only for TMin < t < TMax; procedural hits for TMin <= t <= TMax, checked by the shader (`spec:2551-2561`); +INF is a legal TMax (`spec:2564-2565`) | nothing; but TMin < 0, TMin > TMax or any NaN in the ray is undefined behaviour, not checked (`spec:2564-2572`) | A loses hits at exactly t = 0 and at the extent. Screening is per operand, with the software path as oracle: NaN extent traces with TMax = +INF; NaN origin or direction is a miss without a trace; a negative extent or a NaN inverse has no host form and takes the software loop |
| **Inverse direction and per-call extent.** The program supplies the inverse direction as its own operand, and an extent on every call. The box test reads only the inverse, the triangle test only the direction (`emit/lower/bvh.rs:110-115, 333-339, 419-446`). Whether the title passes 1/direction, a clamped value, or the running best t is unknown | a ray is origin, direction, TMin, TMax (`spec:6858`); TMax changes only on a commit (`spec:1163-1164`) | how the host derives and rounds its own inverse | Not delegable. Every difference between the title's inverse or extent and the host's lands in the count of rays where the two box cullings disagree. A program is eligible only where its inverse is proven to be 1/direction |
| **Masks.** An instance holds "id and mask" (a fork's report); the width and the test are the title's code and unknown | an 8-bit mask per instance, ANDed with the ray's low 8 bits; a zero mask is never included (`spec:4401, 6857`) | nothing | Three cases. 8 bits or fewer, tested by AND: delegable. Wider, tested by AND: a fold of guest bits onto 8 host bits on both sides is a superset filter, with the exact test kept in the shader. Any other test: every host mask is 0xFF and the guest test stays in the shader. F avoids the question |
| **Instance transforms.** A 3x4 matrix in the instance node (a fork's report). The ray is transformed by the title's code; the instruction does not read instance nodes (`emit/lower/bvh.rs:21`) | a 3x4 row-major instance-to-world matrix; the implementation transforms the ray (`spec:4399`) | how the inverse and the object-space ray are computed and rounded | The host's object-space ray is not the title's bit for bit. C must recompute it with the title's arithmetic, or its t differs. Which matrix the guest stores is unknown. F keeps the title's own transform |
| **Flags.** Instance flags and per-triangle id bits beyond the barycentric choice exist ("opaque and the like", `avatarmanz:crates/victory-shader/src/bvh.rs:634`); their meaning is unknown | instance, geometry and ray flags (`spec:4294-4303, 4408-4427, 5455-5469`) | **a ray-flag word with any undefined bit is undefined behaviour** (`spec:6856`; the same for pipelines, `spec:6128`) | No guest flag word reaches `TraceRayInline` unmapped; the host word is a constant or built bit by bit from proven meanings. Until a mapping is proven, host flags stay neutral and the guest's tests stay in the shader |
| **Culling.** Facing is the sign of the denominator: positive is a front face (`emit/lower/bvh.rs:40-41`). Culling is the program's code | front is clockwise from the ray origin in object space, left-handed, unaffected by the instance transform (`spec:4427`) | nothing | A's facing depends on the vertex order given to the build. C takes facing from the emulated denominator and uses no host cull flag |
| **Watertightness.** The emulated test counts edges and vertices as inside, so a ray through a shared edge hits both triangles; a zero-area triangle misses (`emit/lower/bvh.rs:41-44, 465-473`). The console's own edge result is unknown | a ray on a shared edge or vertex reports exactly one of the incident triangles (`spec:2378-2382, 2404-2409`) | which one; "not exactly the same results across implementations" (`spec:2381, 2439-2441`) | A reports one triangle where the guest reports two, and a different one per driver. C keeps the emulation's rule |
| **Per-candidate work.** Unknown: whether any loop samples a texture, tests an opacity bit, counts or accumulates between a hit and its acceptance | with `RayQuery`, every procedural candidate returns to the shader, which decides (`spec:1166-1171`). Observable duplication of a primitive is invalid, with exceptions for intersection-shader and any-hit invocations that the no-duplicate geometry flag removes (`spec:692-704, 2134-2144`) | **whether a `RayQuery` candidate may repeat, with or without the flag, is not stated**: the texts speak of pipeline shader invocations | C sets the flag and the probe counts repeats. Until a device records zero, only loops whose per-candidate action is idempotent (a minimum, a logical or) are eligible |
| **Early-out rays.** Unknown: which loops stop at the first hit | a ray flag ends the search at the first hit; the shader may also stop (`spec:5479, 7147`) | which hit is first | A yes-or-no result is the same whenever the set of hits is the same. A loop that keeps the first hit's distance or identity is order-dependent: not eligible |
| **Box bounds, 16-bit and 32-bit.** Sixteen-bit floats widened exactly, or 32-bit floats; then a plain single-precision slab test, no outward rounding. The only descriptor the translation accepts has box growth 0; whether any title program asks otherwise is not counted, and such a program has no software pipeline at all (`emit/lower/bvh.rs:13-18, 22-32`; `crates/victory-shader/src/resource.rs:1340-1347, 1370-1372`) | the space inside the input boxes is included (`spec:2155-2158`); a primitive the ray intersects is enumerated (`spec:1159-1163`). Input boxes are six 32-bit floats (`spec:4354`) | more, fewer or larger volumes (`spec:2155-2167`); enumeration of anything the ray does not exactly intersect (`spec:1162-1163`); the numerics of the host's ray and box test: no statement found, where triangles have a section (`spec:2374-2545`) | Two differences, in opposite directions. The host may present a triangle the guest's box chain culled. And the host may **not** present one the guest hits through float rounding: the emulated test's error grows with the distance from origin to vertex (`emit/lower/bvh.rs:445-455`), not with the box coordinate, and a triangle in an axis plane has a zero-thickness box. No padding derived from the specification closes the second. "Loses no hit" is a per-driver result on a finite sample, never a proof |
| **Triangle nodes.** Up to four triangles in a node; which exist is known only from parent pointers; spare bytes hold other data (`emit/lower/bvh.rs:33-36`; `avatarmanz:crates/victory-shader/src/bvh.rs:47-49`) | none | none | A mirror built by scanning nodes would bound bytes that are not vertices, or miss triangles. The primitive is a pointer found by a walk from the root |
| **Packed static structures.** Shipped packed, unpacked in place on the GPU once (forks' reports) | a structure comes only from a build; a serialized one only from the same driver's own (`spec:2001-2046, 4852`) | everything inside a structure | A mirror can be built only after the unpack has run and is known complete. A fork counts about 3,000 such structures in the opening; the build cost is unmeasured |
| **Dynamic rebuild.** Built each frame; the title reads a compacted size back a frame or two later and has the structure copied to memory of that size (a fork's report) | builds are commands on the GPU timeline, ordered by UAV barriers; a top level must be rebuilt or updated when a bottom level it names changes (`spec:2019-2021, 2360-2367`) | build time, structure size | A guest address is not a stable identity: staleness has to come from page writes. The mirror follows the guest's final copy and is at least one submission behind. Per-frame cost unmeasured |
| **Refit.** Dynamic structures are refitted (forks' reports); whether counts stay fixed is unknown. The builder knows a triangle "invalidated" by NaN (`lab:docs/CONSOLE-DLL-EXPORTS.md:164`) | an update may change vertex positions or boxes only; a primitive may not cross between active and inactive in an update (`spec:2185-2189, 2286-2300`). A box is inactive only if MinX is NaN; NaN elsewhere without it is undefined; inverted bounds become a point (`spec:2175-2176, 2200-2202, 2225-2227`) | an updated structure may trace slower (`spec:4216`) | A host refit is valid only while the guest's element count is unchanged and no primitive crosses NaN; otherwise rebuild. A NaN vertex gives a box with MinX = NaN, never a partly NaN or inverted box |

What the table leaves standing:

* Path A differs from the guest on five rows by specification, not by
  accident of one driver: distance form, extents, edges, ties, facing
  source. It cannot be offered as equivalent.
* Path C keeps the triangle arithmetic and nothing else by construction.
  It equals the software path only for a loop proven independent of
  candidate order, on rays where the two box cullings agree, on a driver
  that presents every candidate the guest accepts and repeats none. Where
  the box cullings disagree its answer is a different one, and in part the
  driver's: the objection rule 3 of
  [gpu-compatibility.md](gpu-compatibility.md) raises against A. The
  choices are in dxr-architecture.md, "Rule 3 and option C".
* Path F takes masks, instance transforms, instance flags and order across
  instances out of that list, for one query per instance visited. It is as
  unmeasured as C.
* Eight rows turn on what the title's loops do. Fork notes describe their
  shape; the rules that matter here are in none of them.

## Host support

Device words are those of [gpu-compatibility.md](gpu-compatibility.md):
Expected, Reported, Tested, Runs the title, Confirmed working. The state
of work uses the owner's words; **the host backend is speculative on every
device: it does not exist and nothing about it has been run.**

### By tier

The tier is what `D3D12_FEATURE_D3D12_OPTIONS5` reports; the record keeps
it as a number (`crates/victory-gpu/src/caps.rs:47-50`). `gpu-info` does
not print the number today, and a failed query reads as no ray tracing.

| Tier reported | What the specification puts in it | Host backend |
|---|---|---|
| none | no ray tracing | not available; software traversal, which needs no ray-tracing hardware |
| 1.0 | pipelines; no `RayQuery` (`spec:3211`) | not available: the design is inline only |
| 1.1 | `RayQuery` in any stage, compute included (`spec:1139-1141, 3212`) | the tier the design needs |
| 1.2 | adds opacity micromaps and shader execution reordering (`spec:3213`) | as 1.1; nothing from 1.2 is used |
| 2.0 (preview) | as the specification marks it (`spec:3214`) | as 1.1 |

### By device

"Node test" is the emulated instruction's tests in `cargo test`: the suite
has hardware and WARP cases, and no run of `bvh_tests` is recorded by name
for any device. "Traced picture" is the title's ray-traced light and
shadow in a run; it is absent on every host in this fork (`README.md:51`).
On a host with 32-lane waves a tracer that declares 64-lane waves has no
software pipeline either (G2), so "software" there may mean a dropped
dispatch; which tracers do is unknown.

| Host | Tier | Node test | Traced picture, software | Host backend | Evidence |
|---|---|---|---|---|---|
| NVIDIA Blackwell, RTX 5070 Ti (target) | not asked on this card | Expected | Expected; G2 if its waves are 32 lanes | speculative; tier unverified | none: no `victory gpu-info` report recorded |
| NVIDIA Ada Lovelace | another tool printed 1.2 and waves of 32 to 32 lanes for an RTX 4070 Ti SUPER in a virtual machine; not a `gpu-info` report | Expected. Fork: fxceless's line checked its own emulation "against a reference on the GPU" on an RTX 4070; their code, no timing, nothing lit | Expected; G2 applies on that report | speculative; tier unverified | `lab:docs/VM-GPU-RESULT.md:10-14`; `avatarmanz:docs/README-fxceless.md:38, 137` |
| NVIDIA Ampere, Turing | not asked | Expected | Expected | speculative; tier unverified | none |
| NVIDIA, later generations | not asked | Expected where the needs are reported | Expected | speculative; needs tier 1.1 reported; nothing is keyed to a model | none |
| AMD RDNA 2, RX 6700 XT | not recorded | Expected; the project reports shader probes in general, no run of `bvh_tests` by name | Expected | speculative; tier unverified | gpu-compatibility.md, status table |
| AMD RDNA 3 and 4 | not recorded | Expected | Expected; the forks' runs on these cards are of the forks' code | speculative; tier unverified | ray-tracing-research.md |
| Intel Arc | not asked | Expected | Expected | speculative; tier unverified | none |
| WARP | not asked | Expected; CI runs the suite, no result is recorded | too slow for the world | not known whether it reports tier 1.1 | gpu-compatibility.md, status table; `.github/workflows/ci.yml` |

Vendor statements, read only through page summaries and not verified
here: Microsoft's DXR 1.1 announcement says tier 1.0 devices can report
1.1 through a driver update; NVIDIA's driver notes speak of "GeForce RTX
GPUs" without naming a generation. No page read names a tier for any row
above, so "Expected" in the gpu-compatibility.md sense is not met for the
host backend on any of them.

A device moves for the host backend only by a recorded run of the probe in
dxr-validation.md on that device and driver. The result holds for that
driver: determinism is promised per device and driver (`spec:2053-2056`),
so a driver update reopens it.

## What a backend may not assume

* That a guest structure's bytes mean anything to the host, or the
  reverse.
* That two drivers, or two versions of one, agree on an edge, a tie, the
  last bit of t, or which candidates outside the guarantee they present.
* That a guest ray is a legal host ray, or a guest flag word a legal host
  one: an undefined flag bit is undefined behaviour (`spec:6856`).
* That a host box test finds everything the guest's rounding accepts.
* That a candidate is presented once.
* That a software pipeline exists to fall back to on this host.
* That the hardware path is faster. Inline tracing gives the system less
  room to optimise (`spec:1145`); the specification's "less efficient"
  for intersection shaders (`spec:683-684`) is about pipelines and applies
  here by analogy only; the only timings anywhere are a fork's, of its own
  software path, on one AMD card with title memory held on the card.
