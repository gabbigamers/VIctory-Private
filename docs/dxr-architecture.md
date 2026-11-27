# A host ray-tracing backend: the semantic compatibility layer

How the title's ray-tracing work is to be carried by the host's own
hardware: what is translated, into what, where each difference is met,
and which path runs when. Facts: [ray-tracing-research.md](ray-tracing-research.md).
Semantics and the equivalence rule: [dxr-compatibility.md](dxr-compatibility.md).
Testing and measuring: [dxr-validation.md](dxr-validation.md).

**Nothing here is built and nothing was run on hardware; every part is
speculative.** The only things done are tool runs in a Linux container
(research, "Checked with Microsoft's tools"). `gpu/` is
`crates/victory-gpu/src/`, `emit/` is `crates/victory-shader/src/emit/`,
`res:` is `crates/victory-shader/src/resource.rs`, `spec:` is the DXR
specification by line, `av:` and `lab:` are a fork and the reference notes.

## What the owner decided (2026-11-27, three entries in `progress.md`)

* **Native first.** Host bottom- and top-level structures from the
  title's geometry; inline `RayQuery` in place of a recognised traversal
  region; the title's code around it left as translated guest code.
  **Hybrid** (host boxes, the title's triangle test in the shader) is
  the compatibility option; **software** (today's translated loop) the
  reference and the fallback of everything.
* **A semantic layer, not an instruction replacement**: the guest
  hardware's internals are not reproduced where the code after the
  region can be given equivalent values. **Not bit-identical, but
  demonstrated** per device and driver, never assumed; the written
  meaning of "demonstrated" (dxr-compatibility.md) is a proposal the
  owner has not yet accepted.
* Behind a switch, off by default. Selection is by reported capability
  and recorded results, never by vendor or model (rule 1 of
  [gpu-compatibility.md](gpu-compatibility.md)). No effect is dropped,
  shortened or estimated to make a path fit.

## Prerequisites, unchanged by the decision

| # | Prerequisite | Evidence |
|---|---|---|
| 1 | **Nothing to traverse yet.** The title's structures never appear in this fork (`progress.md` C3): no guest structure, no software result to compare with | `README.md:51` |
| 2 | **A software pipeline may not exist on the target.** A program that declares 64-lane waves gets no pipeline on a 32-lane host (G2); the wave size of no tracer is recorded. Without it there is no reference on that device | `gpu/caps.rs:94-96, 121-131` |
| 3 | **No title trace loop has been read by anyone.** No disassembly, IR or fixture is in any tree. Every shape below is designed against a synthetic loop | research, "What the title does" |
| 4 | **Nothing recognises a region.** Lowering is per instruction; there is no natural-loop analysis | `emit/lower.rs:662-672`; `emit/lower/flow.rs:170-183` |
| 5 | **Host geometry has no source read yet**, and the builder's exports are not intercepted | `crates/victory-loader/src/windows.rs:163-183` |
| 6 | **No timing exists for any path on any NVIDIA card**, and the title's files are gated | research, "The forks" |

## The layer: one description, three generators

A **workload** is one recognised traversal region of one program; a
program may hold several. The recogniser runs where every product of
translation is still in hand (between `resource::track`/`io::raise` and
`emit_with`, `emit/mod.rs:391-415`) and writes a description naming no path:

| Field | Content | Why it is needed |
|---|---|---|
| Region | header block, body blocks, exit edges; SSA values read inside and defined outside (live-ins), defined inside and read outside (live-outs), lane masks among them | the lowering makes a block and a counter per IR block and wires phis by edge (`emit/lower.rs:654-672`; `emit/lower/flow.rs:895-953`) |
| Ray source | origin, direction; the minimum compare and the extent compare, each with its form (strict or inclusive, ordered or unordered) and how the extent shrinks; whether the inverse operand is proven to be 1/direction | the instruction takes inverse and extent as the program's own operands and does no extent test for triangles (`emit/lower/bvh.rs:10-12, 44, 110-115`) |
| Structure source | root pointer from a root parameter, from a heap descriptor at a computed index, or from data | `res:1340-1407`; decides what is known when a dispatch is recorded |
| Flags, mask | proven constants only: first-hit exit, face culling, mask width and test | an undefined host flag bit is undefined behaviour (`spec:6856`) |
| Filter | the sub-region run on a candidate before it is accepted (an alpha test), what it reads, and whether its action is a decision alone, idempotent (a minimum, a maximum, an OR), or counting or accumulating | becomes the body run per non-opaque candidate; a candidate may repeat |
| Instance code | guest code run on entering an instance beyond transform, mask and culling | the host gives no call on entering an instance (`spec:1153-1179`) |
| Kept from a hit | nothing (yes or no); distance as quotient, as pair, sign only, magnitude; barycentrics and their selectors; facing; node pointer; instance values or the instance record's address; loads from structure memory by an address derived from the hit | decides what must be synthesised |
| Order | **none** (yes or no, or a minimum of t with nothing else kept); **tie by visit** (one compare of t against the running minimum, `<` or `<=`, everything kept replaced with it: order matters only among exactly equal t); **tie by key** (equal t settled by a value of the candidate); **visit order** (a first hit kept with its distance or identity, an iteration or stack cap, order-dependent accumulation) | visit order is not reproducible (`spec:2087-2110`); the recogniser must prove the value, and anything unproven is visit order |
| Memory effects, lanes | stores and shared-memory use that outlive the region; per-lane or whole-wave walk | no analysis summarises these today (`emit/lower/ds.rs:19-36`; `emit/lower.rs:474-477`) |

The hash of every field but the region is the **workload class**: what
the loop means, never a title shader's hash or name. A synthetic program
and a title loop with the same fields are one class.

| Generator | Emits | Needs at run time |
|---|---|---|
| **Software** | the region lowered as today, untouched | the guest structure in title memory |
| **Hybrid** | `RayQuery` over one procedural box per triangle; each candidate runs the emulated triangle test | a host box structure and the triangle's vertices: guest nodes, or the title's input buffers (below) |
| **Native** | `RayQuery` over host triangles; the live-outs synthesised; the filter as the candidate body | a host triangle structure; side tables only where the description asks |

Each returns a `HostShader` with its binding needs, or a named refusal in
the style of the waterfall proof (`res:1892-1910`). The precedent for
replacing code by "the values that leave it" is one block wide
(`emit/wave.rs:125-170`; `emit/lower/crosslane.rs:16-67`); the
multi-block form must be added. **The native generator** gives the code
after the region equivalent values and nothing of how the guest got them.

## Guest concepts and their native equivalents

Cost classes count operations, not time; no time is claimed anywhere.

| Guest concept | Native equivalent | Adaptation | Resolved at | Cost class |
|---|---|---|---|---|
| **Structure construction.** The title hands the builder D3D12 build descriptions; its compute stages write console nodes (`lab:CONSOLE-DLL-EXPORTS.md:45`, static analysis) | a host build from the same description (`spec:4105-4110`) | addresses converted to host buffers; a NaN vertex written as an inactive triangle (`spec:2174-2176, 2199-2201`); every geometry not opaque is built with the no-duplicate flag (`spec:4303`). Bytes are never reused (`spec:2001-2046`) | structure build | per build |
| **Which structure, and is it current.** A root pointer in a register (`res:1340-1407`) | a structure bound as a root view, or taken from the heap by index | *dual* form: both variants recorded, counts set at submission. *Both-loops* form: one variant holds the native and the software loop and reads a table written at submission ("Selection") | per dispatch; per ray in the both-loops form | dual: none in the shader. Both-loops: one read and one branch per region entry |
| **Traversal.** The program's loop: a stack, four sorted children per box call, one triangle per call (`emit/lower/bvh.rs:26-36`) | `Proceed()` (`spec:1153-1164`) | none: visit order, the guest's box culling and caps are not reproduced. The order field decides whether that is allowed | translation | none |
| **Ray parameters.** Origin, direction, inverse and extent on every call | one `RayDesc` (`spec:6858`) | the inverse is dropped once proven 1/direction. Screens, so the host never gets a ray it forbids (`spec:2564-2572`): NaN origin or direction, zero direction, negative extent: a miss without a trace (`emit/lower/bvh.rs:43, 357-360, 441-444`). NaN extent, by the recognised compare: ordered, a miss; unordered or negated, TMax = +INF; not recognised, the loop is refused. Infinite origin or direction: no native answer; a miss, counted, and the class is not validated while the count is not zero | translation; per ray | per ray: compares and selects, no memory read |
| **Extents.** Triangles hit for t >= 0 with no extent test; the loop compares t itself (`emit/lower/bvh.rs:44, 481-492`) | TMin < t < TMax, TMin >= 0, +INF legal (`spec:2553, 2564-2565`) | `t < e`: TMax = e. `t <= e`: the next float above e for a finite e >= 0, +INF unchanged (one bit more is a NaN), -0.0 taken as +0.0. A minimum `t > m`: TMin = m; none: TMin = 0, and a hit at exactly t = 0 falls in the extent-boundary class | translation; per ray | strict: none. Inclusive: a compare, a select and an integer add per ray |
| **Flags, first hit.** The loop's code | the `RayQuery` template constant (`spec:6554`; `allocateRayQuery(178, flags)` in the reference) | a proven constant set; the dynamic argument is 0; a yes-or-no loop gets accept-first-hit; procedural primitives are always skipped | translation | none |
| **Masks.** "Id and mask" in the instance record, tested by the title's code (`av:docs/fork-ray-tracing.md:74-76`) | an 8-bit mask ANDed with the ray's low 8 bits (`spec:4401, 6857`) | 8 bits or fewer tested by AND: copied. Anything else: refused | top-level build | none |
| **Facing and culling.** The denominator's sign, positive in front (`emit/lower/bvh.rs:40-41`); culling is the loop's code | clockwise from the origin, object space, reversible per instance (`spec:4427`); cull ray flags (`spec:5455-5469`) | vertex order and the instance flag; a proven "skip this facing" becomes a cull flag. Open: the two emulations give opposite facing for odd id bytes (research). By the forks' reading the build order settles it. By this repository's, a decoded node is built in node order and the id bits pick the weights per hit; from build descriptions a facing-dependent class is refused once an odd id byte occurs | structure build; translation | forks' reading: none. This repository's: per hit a table read, or none with triangles grouped into geometries by id byte |
| **Per-instance guest code** beyond transform, mask and culling | none (`spec:1153-1179`) | refused; the hybrid may re-run a pure skip-or-enter test per candidate from the instance index | translation | hybrid: per candidate |
| **Identifiers, instance.** Values the loop loads from the instance record, or the record's address | `InstanceIndex`; `InstanceID` and the contribution, which are fields of the title's own description (`spec:4381-4383, 7534-7576`) | the title's id and contribution come back as given. A guest record address or index: a side table by instance index, since the builder reorders (`lab:GHIDRA-BVH.md:55, 63`); the guest top level must then exist | top-level build | own id: none. Record address: one read per committed hit |
| **Identifiers, triangle.** The node pointer the loop passed in and whatever it loads from the node; the instruction returns no id (`res:1370-1372`) | `GeometryIndex`, `PrimitiveIndex`, autogenerated, inactive ones counted (`spec:2192-2198, 7583-7608`) | a side table by (bottom level, geometry, primitive) of guest node pointer or id byte; only over decoded nodes | structure build | per committed hit: one read; none for a yes-or-no loop |
| **Result, distance.** Numerator and denominator in the emulation's own scale; a miss is (+inf, 1, 0, 0) (`emit/lower/bvh.rs:46-47, 513-522`) | `CommittedRayT`, front face (`spec:7344-7355, 7772-7782`) | (t, +1) for a front face, (-t, -1) for a back face, the miss words as constants: exact for every quotient and sign, since negation and division by 1 do not round | translation | per hit: a select and a sign flip |
| **Result, barycentrics.** Two numerators chosen by id bits, carrying the denominator's sign (`emit/lower/bvh.rs:462-475, 493-512`) | non-negative weights of the build's second and third vertex (`spec:2147-2149, 7756-7766`) | (x, y) for a front face, (-x, -y) for a back face. Vertex order: the title's own from a description (by a fork's reading the id bits weigh its second and third vertex, `av:crates/victory-shader/src/bvh.rs:42-46`); for a decoded node, as the facing row says. A selector of the first vertex's weight is 1 - x - y | translation; structure build | per hit: the same select and two sign flips |
| **Material filtering.** The title's test on a candidate | a non-opaque candidate returns to the shader, already the closest so far; `CommitNonOpaqueTriangleHit` (`spec:1166-1179, 7207-7215`) | the filter is the loop body, as translated guest code on synthesised candidate values; geometry flagged opaque at build never returns (`spec:4301-4302`) | translation; structure build | per non-opaque candidate |

## The native path

**What the code after the region is given.** The instruction gives a loop
four float words and nothing else (`emit/lower/bvh.rs:116-118, 183-188,
513-522`), but a region's live-outs are whatever the loop computed, so
the list is not closed. Those with a recipe: the hit flag; the distance
in the form the code expects; two barycentrics with their sign; facing;
the title's own instance id and contribution; a guest node pointer, an
instance record address, or bytes read through them (side tables). Lane
masks are rebuilt from the hit flag. Any other live-out (an iteration
count, the last node visited, the object-space ray) refuses the region.

**Material processing.** Where it sits in any title loop is unknown.
Code after the loop, or in a later program reading a hit buffer, is
untouched guest code fed by the live-outs. An alpha test inside the loop
becomes the body run per non-opaque candidate; the nearest accepted
candidate wins whatever the order, if the decision depends on the
candidate alone. Its inputs are the host's weights and t, so a decision
can flip within tolerance of its threshold (the rule's filter-threshold
class). A candidate may come twice (dxr-compatibility.md, "Per-candidate
work"), so a filter that counts or accumulates is not native.

**Refused, and sent on** (each a named reason, counted):

| Condition found by the recogniser | Goes to |
|---|---|
| The code uses the denominator's magnitude, compares raw numerators of different triangles, or mixes them with guest-scale values from elsewhere | hybrid |
| Order is tie by key; or it needs both triangles on a shared edge with an idempotent action | hybrid |
| It reads guest structure memory through an address from the hit and no side table can be made (geometry from build descriptions: the builder reorders triangles, `lab:GHIDRA-BVH.md:55, 63`) | hybrid over decoded nodes |
| A mask wider than 8 bits or not tested by AND; a flag whose meaning is not proven; per-instance code that is a pure skip-or-enter test | hybrid |
| Order is visit order; a filter or per-candidate action counts or accumulates; per-instance code changes the ray; the inverse is not proven 1/direction; a compare's predicate is not recognised, or triangles are accepted with no compare of t against the extent the box calls got; procedural nodes | software |
| No region recognised, or a live-out with no recipe | software |

Not the recogniser's to decide, because they are data: how many rays
start or end on a surface, how often a cap fires, whether an id byte is
odd, whether a ray is infinite. The probe, the census and the title
comparison count them, and a class fails there.

## The hybrid and the software path

The hybrid keeps the guest's triangle test and nothing else. It runs a
workload only where order is none, tie by visit or tie by key and the
per-candidate action is a decision or idempotent, and commits only in a
nearest-hit loop, with the quotient rounded up.

| Box source | What the test reads | Current | Limits |
|---|---|---|---|
| **Decoded nodes**: one box per guest triangle pointer | the guest's node bytes; the node pointer is exact | one submission behind: a top level with a bottom level built or refitted in the frame is stale every frame, and its workloads run software, counted | serves static geometry |
| **Build descriptions**: one box per input triangle | the title's vertex and index buffers, by index | in the same submission | the guest's test, not the guest's bytes: whether the builder stores vertices unchanged is unverified; vertex order may differ and the test's sums are order-sensitive in the last bit (`emit/lower/bvh.rs:474-480`); no node pointer; an odd id byte flips facing under this repository's reading |

So a workload refused natively for guest intersection semantics that
traces dynamic geometry and needs node bytes ends in software. The
software path stays a pipeline of its own for every program, equal to
its CPU reference bit for bit on every device (rule 3). It is not a
console reference: the emulation is unchecked against one, and two
emulations disagree on facing.

## Selection

**Rule.** For each workload: the fastest path validated for its class on
this device and driver; else the next; software last. The order of trial
is native, hybrid, software. `VICTORY_RT_BACKEND=software|auto|native|hybrid`,
default `software`; the last two force a path for measuring, on eligible
workloads only, each use logged as not validated;
`VICTORY_RT_CLASS=<class>` narrows a forced path to one workload class.

| When | What is decided | Evidence |
|---|---|---|
| A test run | **The validation record**, beside the pipeline library, for one adapter identity, driver version, probe version and translator hash; a driver update empties it. One row per workload class, path, shader form (below), structure source and side-table kind, holding: the hash of the container that was validated and timed; the probe's verdict per level and the largest coordinate magnitude it covered; the **title verdict** (scene, date, capture ids, dispatches compared, clean frames); the **title times**. `auto` selects a row only when the probe passed, the title verdict passed, and the title times show it faster than software (dxr-validation.md). Otherwise the path runs only when forced. The probe's synthetic times are reported and select nothing. A verdict is not in the shader cache key, so a changed verdict retranslates nothing | `spec:2053-2056`; `gpu/pipeline/library.rs:7-10`; `gpu/caps.rs:55-57` |
| Pipeline creation | The software variant is made first, as today. A program with a region whose class has a selectable row gets one more variant, each region on its own best path. It queues behind every software pipeline: a dispatch whose only pipeline is still being made is dropped today, and a variant may not add such a drop. The path and form enter the shader cache key; the library names pipelines by their DXIL, so variants coexist. Below tier 1.1 the variant is refused by its feature word, as 64-bit atomics are | `gpu/pipeline/make.rs:30-45, 193-205`; `gpu/draw.rs:1103-1111`; `gpu/shader_cache.rs:57-93`; `gpu/pipeline/library.rs:254-262`; `gpu/pipeline/host.rs:437-459` |
| Each dispatch | Only readiness. A pipeline is bound when the title records, and lists are held, cut and interleaved before they run, so a choice at record time is safe only towards software. **Dual form**, where the structure is known from root arguments: both variants are recorded and submission gives one the title's counts and the other zero. **Both-loops form**, where it is known only on the GPU. The two forms are different DXIL, so each is its own container with its own row: a form with no passing row is not selected. Both are unbuilt and unmeasured | `gpu/draw.rs:1096-1132, 1845-1883`; `gpu/hold.rs:1-15`; `gpu/indirect.rs:55-61`; `gpu/memory.rs:11-20` |
| Structure binding | A root view takes an address when the trace is recorded. So each registry entry has a host range that does not move while a recorded or held list names it: a rebuild goes in place, a copy makes a new entry. A trace recorded before its entry exists is recorded software only, counted. The both-loops form takes its structure from a heap slot written at submission | `gpu/hold.rs:8-15` |
| Currency | A host structure is current when it was built or updated from the latest description for its guest address, earlier in submission order. A top level is current only if every bottom level it names is (`spec:2019-2021`); a bottom-level build put off by the bound puts off its top level. Otherwise every workload tracing that top level is downgraded | `gpu/host.rs:1533-1549` |
| Each downgrade | Counted and logged per frame by program, region and reason: not eligible, not validated, variant not made, structure absent, stale, over budget, writes withheld. A structure is never bound null: that is a miss for every ray | `spec:5341-5345, 6855` |

## Host structures

| Matter | Design | Evidence |
|---|---|---|
| Source 1: build descriptions | A wrapper at the builder's import boundary logs and forwards each call, counted per export. A bottom level is built from the description's vertex and index buffers, a top level from the instance descriptions in list upload memory, which fit the 64-byte D3D12 record (inferred). The host build is recorded where the title called the builder, so it is current in the same submission | `lab:CONSOLE-DLL-EXPORTS.md:45-49`; `gpu/list.rs:1205-1209`; `spec:4375-4385` |
| Source 2: packed static structures | A rehydrate description names index and vertex buffers beside the packed source. If packed structures leave triangle data to those buffers, the static world is described at the boundary too. Not verified. One of the three rehydrate exports takes a device and no list: where its unpacking runs, and so where a host build would be recorded, is unknown | `lab:GHIDRA-BVH.md:68-76`; `lab:CONSOLE-DLL-EXPORTS.md:48-50` |
| Source 3: node decoding | The last resort: a pointer walk from the root, one submission behind, never while writes are withheld. It yields the node-pointer side table and the hybrid's decoded boxes | `emit/lower/bvh.rs:19-21, 33-36`; `gpu/memory.rs:310-325` |
| Identity and lifetime | A registry: guest address and build generation to host structure, state, source, last frame traced. A guest address is not stable; a copy call makes a new entry. A structure, its scratch and tables outlive the last submission naming them | `av:docs/fork-ray-tracing.md:80-81` |
| Synchronisation | A UAV barrier between a build and a trace; a top level rebuilt or updated after its bottom levels; an update only where the title's own description is one. Builds per submission are bounded, since one that runs about 2 s ends the run; the rest queue, and "Currency" above says what then traces in software | `spec:2019-2021, 2286-2300, 2358-2370`; `gpu/host.rs:1533-1549` |

**Memory**, in [memory.md](memory.md)'s form. Owed before code: a case
there (none is written, `memory.md:357-379`) and an owner per row (none
exists, `gpu/account.rs:172-212`).

| What | Kind | Size | Lifetime | Way back | Counted as |
|---|---|---|---|---|---|
| Host structures | video memory, default heap, in the acceleration-structure state (`spec:2326-2341`) | the driver's prebuild size each, 256-byte aligned | the registry entry, and the last list naming it | a rebuild from the kept description while its inputs are still in title memory. A static structure's inputs may be gone after the one unpack: **then it is not given up** | owner `HostStructures`, `own_rt_structures_mb` |
| Build and update scratch | video memory | prebuild scratch size; pooled, one range per build of a submission | transient; whether the pool's high-water mark decays is not designed | none needed | `own_rt_scratch_mb` |
| Side tables, hybrid boxes | video memory | 4 or 8 bytes a primitive or instance for a pointer, 1 for an id byte; 24 a box | with their structure | derived again from the guest structure | `own_rt_tables_mb` |
| Kept build descriptions, the registry | process heap | a description and its geometry records per structure | with the entry | none: it is the way back of the rows above | `own_rt_registry_mb` |
| Host copies, windows and snapshots made only because a host build reads vertex and index buffers, or the guest builder writes pages | system memory the GPU reads (memory.md G1, G2, G4: copies never released, windows never freed) | the whole buffer rounded up to 64 KiB; 4 MiB a window | as G1, G2, G4 | title memory | `own_buffer_copies_mb`, `own_windows_mb`, `own_snapshots_mb` |

The three video rows share one byte budget from what the device reports
(rule 5), with marks of its own: on the textures' marks
(`gpu/resource.rs:1541-1547`) a full card would give up every structure
and rebuild it the next frame. Values are not chosen. Giving one up is a
counted downgrade, least recently traced first, never while a list names
it. The guest's structures and build inputs are system memory here
(`gpu/host.rs:3955-3965`): while every guest structure is built, the
layer **adds** video and system memory and removes none.

## Leaving out the guest's own builds

**Consumers** are tabled in research ("Consumers of the guest
structures"); whether the title's own code reads node bytes is shown
nowhere and ruled out nowhere.

| Position | State, and what it would need |
|---|---|
| **Always build** | the default, and the only position available |
| **Build, keep on the host** | **Not available.** "No CPU read over a session" cannot be observed with page guards: pages are 64 KiB and shared with the title's own objects, a guard opens its page on the first fault, and a system call into a guarded block fails instead of faulting (`gpu/watch.rs:5-8, 33-35, 41-44`; `av:crates/victory-gpu/src/pipeline.rs:249-251`). One session bounds no later one. It would need a standing no-access guard with the fault address attributed to a structure, a copy-back completed before the faulting read resumes, the kernel's paths opened first, and the size read-back's arrival logged. memory.md does not reclaim on the expectation that memory will not be wanted |
| **Do not dispatch** | **Not available.** Nothing restores a structure after its submission, so the only sound mechanism forwards and records every builder call and gives a closed set's dispatches zero counts, all or none, at submission (a fork ran 14 of 35 stages and the title died within a second, `av:crates/victory-gpu/src/pipeline.rs:246-263`). It then needs, each with a signal: (1) no later update or copy names the structure: seen only when it arrives, too late; the description's allow-update and allow-compaction flags (`spec:4211-4212`) would say so ahead only if the console builder honours them, unverified, and a plain copy needs no flag. (2) No program reads it from title memory: provable only for tracers whose root comes from a root parameter, never for a root computed on the GPU; read marks are per shared page and after the fact. (3) No downgrade can follow: the variant is made, the host structure is pinned against give-up and built in this submission inside the build bound. (4) A stand-in for the compacted size: that number comes from a stage reading the guest header; the only other source is the CPU-only prebuild query, an upper bound (`lab:CONSOLE-DLL-EXPORTS.md:47`), and the title allocates from it, so the stand-in changes the title's allocations; nothing compares them. (5) No comparison, capture or software mode |

**Signals needed, none present**: the registry; the boundary wrapper
with a count per export; a structure view's address, lost today
(`gpu/descriptor/host.rs:1158-1163`); fault-address attribution; a log
of size read-backs; the title's allocation sizes; the selector's
counters. **Every structure is built** until each has a passing run.

## The CPU side

The native path adds: recognition, for every trace program; one more
lowering per accepted program, path and form (at most a full translation
each; today's log gives one total per shader, `gpu/pipeline.rs:127-153`);
the driver's compile of each variant; conversion of each build
description on the recording thread; a second bind for dual-recorded
dispatches. By count the bound is two variants of 64 programs (a fork's
count) against 24,387 (`README.md:52`); by time and bytes it is unknown.
Nothing here is shown to serve or to cost the objectives (16 GB, a
sustained 30 frames a second, no loss of picture).

## Where each piece lives, and the smallest version

| Piece | Place | Testable now |
|---|---|---|
| Natural loops, region live-in and live-out | `crates/victory-shader/src/ir.rs` | host tests on the loop fixtures |
| Workload description, recogniser, class hash | a module of `victory-shader`; the plan reaches the lowering through `lower::Parts` (`emit/lower.rs:106-118`) | host tests: one synthetic single-lane stack loop as GFX10 words, one accept and one refusal per reason |
| `RayQuery` ops 178 to 215, resource kind 16, flag bit 25, feature word | `emit/shader.rs:57-70, 667-705, 929-959`; a native generator in `emit/lower/` | `scripts/validate-dxil.sh`, and against the reference disassembly |
| Native variant against a CPU model, the ray screens among it | the test-only interpreter, which runs integer and bit-pattern work and hands each `dx.op` to a callback (`emit/bitcode.rs:1382-1392`); the CPU reference moved out of the Windows-only tests | host tests: model against model |
| The tolerance constant k | a host test over the random scene classes | host test; its value goes into the record |
| Record and `select()` | plain data beside `gpu/caps.rs` | host tests |
| Variant slots, structure binding, registry, builds, the boundary wrapper | `gpu/shader_cache.rs`, `gpu/pipeline.rs:61-68`, `gpu/pipeline/host.rs:527-546`, new modules; `victory-loader` | type-checked for Windows only |

All rows but the last are the smallest version: validator acceptance and
agreement between two models; nothing about a driver, a card or speed.

## Order of work, and what would change it

| Step | Work |
|---|---|
| 1 | **Wave sizes** of each tracer and builder stage, and `gpu-info` on the card: whether G2 comes first on the target |
| 2 | **Groundwork** for `progress.md` C3 and the boundary, logging only: a structure view's address, `DispatchX` flags, calls per export and what each is given, the magnitude of vertex coordinates |
| 3 | **The smallest version** above, on synthetic programs |
| 4 | **The synthetic probe** on the owner's machine, native path first; it writes the probe's part of the validation record |
| 5 | **A census of trace loops** (title files; counts only leave the machine): the description's fields per loop, compare forms, ray biases, how often a cap fires, one bottom level's id bytes |
| 6 | **Host structures from build descriptions**; GPU time by kind |
| 7 | **Native variants** for the classes steps 4 and 5 leave, compared on the title on the same rays; then the hybrid for what was sent to it |
| 8 | **The consumer measurements** for leaving builds out |

| Evidence | Change |
|---|---|
| Tracers declare 64-lane waves and the target runs 32 | G2 first; a native variant with no software twin stays unselected unless the owner accepts another reference |
| Probe: native fails a class on the target | that class goes to the hybrid on that driver; if the hybrid fails too, software, and effort goes to its cost |
| Probe: a seam between bottom levels leaks on the target | every class tracing more than one bottom level fails native there; the hybrid is tried |
| Title comparison: a validated path is not faster than software for a class | software stays selected for it |
| Census: most loops have a cap, or keep a first hit | they are software by the order field; the owner is asked whether a cap never seen firing may be treated as absent |
| Census: loops keep a node pointer and load through it | node decoding and side tables before native; guest builds cannot be left out for them; dynamic geometry stays software |
| Census: alpha tests inside loops read textures or guest nodes per candidate | the filter body and per-candidate tables are built first; yes-or-no loops without one still go first |
| Census: no loop is order-independent, or all are whole-wave walks with popped masks | no recogniser for those; software, and effort goes to its cost |
| Id bytes with an odd permutation occur | the facing disagreement must be settled before any facing-dependent class is validated |
| The builder's exports are not given full geometry | node decoding is the only source; host structures run one submission behind |
| Structures appear and tracers are a small share of GPU time on the target | nothing to gain; the backend stays an experiment |

Not taken: per-node host answers (no DXR operation returns a sorted
child list, `spec:1166-1171`); DXR pipelines (a trace program is a compute
program with thread groups, `spec:2726-2734`); replacing only one bottom
level's walk; host compaction (a read-back, a copy and two footprints at
once: not designed); tier 1.2 or vendor extensions (rule 4); a stand-in
for light.
