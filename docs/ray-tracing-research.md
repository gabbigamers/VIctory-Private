# Ray tracing: what is known

The record behind [dxr-architecture.md](dxr-architecture.md),
[dxr-compatibility.md](dxr-compatibility.md) and
[dxr-validation.md](dxr-validation.md). First read on 2026-11-27 at commit
`ae1c915` and corrected after three reviews; revised the same day at
`dc43e7a` for the owner's revised priorities (native traversal first, a
semantic compatibility layer; `progress.md`, session log of 2026-11-27),
from three further readings (what a loop keeps from a hit, who consumes
the guest structures, where a layer can sit in the code), and corrected
after two challenges. **Nothing was run on a GPU and no title file or
title program was read.**

Every fact has a source and a place.

* **Code**: read in code or a document opened for this record.
* **Fork**: reported by a fork or a reference repository. Their runs,
  their machines, their words; not observed here.
* **Inferred**: reasoning over the two above.
* **Unknown**: looked for and not found.

The state of work uses the owner's words (implemented, tested,
type-checked for Windows only, hardware-validated, speculative). Every
part of the layer named in these four documents is **speculative**.

Paths without a prefix are this repository (`emit/` is
`crates/victory-shader/src/emit/`, `gpu/` is `crates/victory-gpu/src/`,
`res:` is `crates/victory-shader/src/resource.rs`). `marvisak:`,
`dexnom:`, `belobug:` and `avatarmanz:` are the forks, at `63bcca4`,
`8114ff7`, `4eb6cfd` and `014d52d`. `lab:` is `xbox-hle-lab` at `8863028`.
`spec:` is `d3d/Raytracing.md` of microsoft/DirectX-Specs by line: 9,755
lines, last change-log row v1.49, sha256 `0c3d494b...59caa7`; the copy is
in a session scratch directory, not kept anywhere durable, and the
upstream commit was not recorded.

## What the title does

| Fact | Source | Where |
|---|---|---|
| The only hardware step is one instruction in two forms. One call tests one ray against one node. Its operands are a node pointer, an extent, an origin, a direction and an **inverse direction the program supplies** | Code | `emit/lower/bvh.rs:1-4, 10-12, 88, 110-115` |
| The stack, instance transforms, culling and the comparison of t are the program's own code. The instruction does no extent test for triangles | Code (the emulation's contract); Fork | `emit/lower/bvh.rs:44`; `avatarmanz:crates/victory-shader/src/bvh.rs:8-11` |
| The title uses no ray-tracing API to trace | Fork | `avatarmanz:docs/fork-ray-tracing.md:16-19`; `lab:docs/BARE-METAL-RUN-13-15.md:147-153` |
| Loops, as the forks describe them: a per-thread stack in shared memory; loads through raw 40-bit addresses; lane masks kept as numbers on some paths; forms written for 64-lane waves; 59 of 64 trace programs translate there | Fork (notes; **no disassembly, IR or fixture of a title loop is committed in any tree**) | `avatarmanz:docs/fork-ray-tracing.md:18-19, 55-58` |
| At least one shadow tracer walks the structure as a whole wave, lane masks pushed to memory and popped as numbers. Other tracers sit inside lighting and shadow programs, where the code after the hit is in the loop's own program | Code (comments naming title entry points) | `emit/synthetic_tests.rs:521-525`; `emit/lower/flow.rs:125-127` |
| Some effects have separate passes after tracing: a trace pass timed as `RaytraceVBuffer`, a `CS_RaytracingResolve` program, an irradiance-cache shading program, ray binning before the trace. That hits go to a buffer a later program shades is an inference from names; no buffer layout or program body supports it | Fork; Inferred | `avatarmanz:docs/fork-ray-tracing.md:55-56, 123-126`; `dexnom:crates/victory-gpu/src/pipeline/ray_tracing.rs:115-121` |
| Five shadow-mask tracers take a structure's address from a heap descriptor picked by a computed index; the title created none of them in the fork's runs | Fork | `avatarmanz:docs/fork-ray-tracing.md:129-132` |
| The structures are built by `XboxBvhBuilder.dll`, shipped with the title: 12 exports, all methods of one class. They take **standard Direct3D 12 build descriptions**: a build (which tail-calls a batched console form), three rehydrate forms, a copy, a prebuild-size query, two validators. Sizes are asked for with a build | Fork (static analysis; the DLL was never executed) | `lab:docs/CONSOLE-DLL-EXPORTS.md:42-66`; `lab:docs/GHIDRA-BVH.md:84-92` |
| The builder works in compute: 52 embedded shaders, 16 root signatures holding only constants and root views. A root view has no size, so a stage's stores are bounded by nothing but the addresses it is given | Fork | `lab:docs/GHIDRA-BVH.md:14, 80, 97-104` |
| Its pass names include LoadInstances, LoadProceduralGeometry, ReloadTriangles, RearrangeElementsPass and TreeletReorder: the build reorders primitives and has a pass for procedural geometry | Fork | `lab:docs/GHIDRA-BVH.md:55, 63`; `lab:docs/CONSOLE-DLL-EXPORTS.md:169-172` |
| The builder dispatches through `DispatchX` and puts its **instance descriptions** in upload memory from list slot 148, which the layer hands out and can read. The forks treat the last 8 bytes of each as a structure address, which fits the 64-byte D3D12 instance record (Inferred) | Code; Fork | `gpu/list.rs:1111-1113, 1205-1209`; `avatarmanz:crates/victory-gpu/src/memory.rs:745-761`; `spec:4375-4385` |
| `DispatchX` flag 0x20 means the counts are threads, not groups; taken for groups, a rehydration stage wrote over neighbouring structures | Fork | `dexnom:crates/victory-gpu/src/draw.rs:2410-2416` (DexNom `bc3cd4b`) |
| The static world's structures ship packed and are unpacked on the GPU once (3,000 in the opening); dynamic ones are built or refitted every frame, and the title reads a compacted size back a frame or two later without checking it arrived, then has the structure copied | Fork | `avatarmanz:docs/fork-ray-tracing.md:77-81, 110-118` |
| A rehydrate description names index and vertex buffers beside the packed source. If packed structures leave triangle data to those buffers, the static geometry is described at the export boundary (Inferred, not verified) | Fork | `lab:docs/GHIDRA-BVH.md:68-76` |
| The title makes shader views of its structures: no resource, view dimension 11, the address in the description. 64 programs hold traversal loops | Fork | `dexnom:crates/victory-gpu/src/descriptor/host.rs:2804-2815`; `dexnom:docs/fork-ray-tracing.md:25-26, 35-36` |
| Both forks' documents call a mapping onto DXR "not practical" | Fork (their position, stated before any attempt) | `avatarmanz:docs/fork-ray-tracing.md:32-36` |
| Per loop: the rule for equal t, where alpha and material tests sit, exit on a first hit, mask width and test, what is stored from a hit, whether later code loads from the structure through the hit, whether the inverse is 1/direction, how the extent is formed, any iteration cap, the root pointer's source, the wave size | **Unknown** | not in any note read |

## The guest structures

| Fact | Source | Where |
|---|---|---|
| A node pointer's low three bits are its kind: 0-3 a triangle of a triangle node, 4 and 5 box nodes of 16- and 32-bit floats, 6 an instance, 7 a procedural primitive. The node is at `base * 256 + (pointer & !7) * 8` | Code, from public sources the file names (AMD's ISA guide, GPURT, Mesa RADV), not from title data | `emit/lower/bvh.rs:4-8, 19-24` |
| A triangle node is 64 bytes: up to four triangles over five vertices; the kind picks which three. Which triangles exist is not in the node; a node with fewer keeps other data in the spare bytes | Code; Fork | `emit/lower/bvh.rs:33-36`; `avatarmanz:crates/victory-shader/src/bvh.rs:41-49` |
| The id dword at byte 60 has a byte per triangle: two bit pairs (I, J) choose the barycentrics returned; higher bits are "opaque and the like". By the fork's reading the builder stores an input triangle's second vertex where I points and its third where J points | Code; Fork | `emit/lower/bvh.rs:37-40`; `avatarmanz:crates/victory-shader/src/bvh.rs:42-46, 634` |
| Programs build a 128-bit descriptor in code. The translation accepts one configuration (sorted children, no box growth, no node bound, barycentric return) and refuses every other, the id-returning mode among them | Code | `res:1340-1347, 1370-1372` |
| The base and the pointer are run-time register values; a base built around a root parameter is recognised, any other is a raw address | Code | `emit/lower/bvh.rs:125-172`; `res:1340-1407` |
| The instruction does not read instance or procedural nodes: four invalid words here, a triangle miss in the other emulation | Code | `emit/lower/bvh.rs:21, 215-224`; `avatarmanz:crates/victory-shader/src/bvh.rs:66-70` |
| Header, 64 bytes: version 6 and marks (bottom level, compacted, rehydrated, still packed), size, element count, bounds, offsets, root pointer, a parent table. Instance, 128 bytes: a 3x4 matrix, "id and mask", flags, the bottom level's root pointer and address | Fork (from their dumps; none committed) | `avatarmanz:docs/fork-ray-tracing.md:68-76` |
| Mask width and test, which matrix is stored, flag meanings, whether a node stores geometry or primitive ids, what the spare bytes hold, whether procedural geometry is submitted | **Unknown** | the layout files `emit/lower/bvh.rs:4-8` names are not in this container |

## A hit: what a loop gets, and what `RayQuery` returns

| Fact | Source | Where |
|---|---|---|
| The instruction writes four dwords. A triangle hit is the numerator of t, the denominator, and two barycentric numerators; a miss is (+inf, 1.0, 0, 0). No id, status, child index or node address comes back; everything else a loop keeps is its own state or its own load | Code | `emit/lower/bvh.rs:116-118, 183-188, 474-522` |
| The numerators' scale is the emulation's own; its header says programs use the ratios and the denominator's sign. That is the emulation's contract, not an observation of a title program | Code | `emit/lower/bvh.rs:46-47` |
| **The two emulations disagree on facing.** This repository tests a node's vertices in fixed order and uses the id bits only to choose the numerators returned. The forks reorder the vertices by the id bits first. For the same node bytes with an odd permutation in the id byte the denominator's sign is opposite | Code | `emit/lower/bvh.rs:405-409, 493-512`; `avatarmanz:crates/victory-shader/src/bvh.rs:164-175` |
| The forks' triangle test scales by the program's inverse direction; this repository computes 1/direction itself | Code | `avatarmanz:crates/victory-shader/src/bvh.rs:244`; `emit/lower/bvh.rs:441-442` |
| After a committed triangle hit `RayQuery` returns t, two barycentrics, front face, instance index, a 24-bit instance id and 24 more user bits, geometry and primitive index, both rays and transforms. Vertex positions need a later shader model | Code (specification) | `spec:7344-7355, 7534-7608, 7756-7782, 7788-7794` |
| The barycentric numerators are unnormalised edge functions and carry the denominator's sign: inside means all three >= 0 or all three <= 0 | Code | `emit/lower/bvh.rs:462-475, 498-512` |
| A front face as (t, 1, x, y) and a back face as (-t, -1, -x, -y) gives every quotient and sign exactly. It is wrong for code that uses the denominator's size or compares raw numerators | Inferred; not run | over `emit/lower/bvh.rs:513-522` |
| Rays at the limits. Boxes: a NaN extent bounds nothing, and -0.0 is a usable extent (0 <= -0 holds). Triangles: no extent test at all, so what a NaN or negative extent does to a hit is the loop's own compare. A zero direction gives 1/0 and NaN shears: every triangle misses. One infinite direction component gives a zero scale and so a zero numerator: a hit at t = 0 on every triangle whose projection along that axis covers the origin. An infinite origin gives no constant answer | Code for the box test; Inferred, not run, for the rest | `emit/lower/bvh.rs:44, 357-360, 441-453, 476-492` |
| The host forbids NaN anywhere in a ray, a negative TMin and TMin above TMax, as undefined behaviour; +INF is a legal TMax; nothing is said of a zero or infinite direction. Watertightness is promised only inside one bottom level, for geometries with matching transforms | Code (specification) | `spec:2404-2409, 2553, 2564-2572` |

## Consumers of the guest structures

| Consumer | Source | Where |
|---|---|---|
| The trace programs: box and triangle nodes through the instruction, instance records through their own loads; bottom levels are reached only through pointers in instance records | Code; Fork | `emit/lower/bvh.rs:21, 125-171`; `avatarmanz:docs/fork-ray-tracing.md:74-76` |
| The builder itself: stage to stage; a top-level build reading each bottom level; the size program reading the header; copy and update reading a source. A fork runs it **all or none**: with 14 of 35 stages the title died within a second, on pointers that had become box bit patterns | Fork | `avatarmanz:crates/victory-gpu/src/pipeline.rs:246-263`; `avatarmanz:crates/victory-gpu/src/memory.rs:40-43` |
| The title's CPU, for the compacted size ("sizes, counters"). Whether it reads node bytes is shown nowhere. Weak evidence against a fatal dependency: a fork keeps structures only on the card and the title ran; here they are never built and the title runs | Fork; **Unknown** | `avatarmanz:docs/fork-ray-tracing.md:110-118`; `avatarmanz:crates/victory-gpu/src/memory.rs:45-56` |
| The builder DLL's own CPU code. Checks of a rehydrate description: a destination below 2^40, a source that is CPU-readable. The export `CpuValidateResultX` takes a list and a build description and calls a CPU-side check of the built structure. **Whether the title calls it is unknown**; nothing intercepts the boundary | Fork (static analysis); Code | `lab:docs/GHIDRA-BVH.md:67-76, 82`; `lab:docs/CONSOLE-DLL-EXPORTS.md:62`; `crates/victory-hle/src/modules/xmem.rs:72-78` |
| One of the three rehydrate exports takes a device and no command list. Where its unpacking runs (the CPU, or a list of the builder's own) is unknown, and so is whether the title calls it | Fork; **Unknown** | `lab:docs/CONSOLE-DLL-EXPORTS.md:48-50` |
| Not settled: the lab note calls copy mode 1 "CLONE"; in the public enumeration 1 is COMPACT | **Unknown** | `lab:docs/GHIDRA-BVH.md:92`; `spec:4836-4842` |

This fork has no all-or-none rule: a dispatch without a pipeline is
dropped singly (`gpu/draw.rs:1096-1118`), and what programs using title
memory wrote is withheld for two frames after a missed page
(`gpu/memory.rs:310-325`). It sees CPU writes to tracked pages, not reads
(`gpu/watch.rs:1-12, 663-670`), and pages shaders wrote or missed, not
pages they read (`emit/mod.rs:132-137`). Its guards work on 64 KiB
pages, open a page on its first fault, and make a system call into a
guarded block fail instead of fault (`gpu/watch.rs:5-8, 33-35, 41-44`);
the structures lie between the title's own objects
(`avatarmanz:crates/victory-gpu/src/pipeline.rs:249-251`). A census by
guards therefore over-approximates and cannot show "no read".

## This fork today

Has (state: implemented; tested only as far as stated):

* The instruction emulated in DXIL, one node at a time, both tests
  computed for every call (`emit/lower/bvh.rs:74-225`), with tests against
  a CPU reference that compile only on Windows; no run of them is recorded
  by name for any device (`emit/mod.rs:199-229`; `emit/bvh_tests.rs:1-7`).
* The host's ray-tracing tier in the capability record; `gpu-info` prints
  met or DEGRADED, not the number (`gpu/caps.rs:47-50, 177-181`).
* GPU time per noted command in the diagnostics build, keyed by kind
  and pipeline name (the title's shader hashes), each command timed to
  the next note, 4,096 notes a list, each list run on its own
  (`gpu/host.rs:425-461, 970-997, 1947-1950`;
  `gpu/pipeline/host.rs:628-635`).
* Analyses a recogniser can stand on: reverse post-order, dominators,
  per-lane exits and masks, address tracing, a single-block proof with
  named refusals (`crates/victory-shader/src/ir.rs:352-460`;
  `res:1340-1407, 1892-1910`), and one precedent for replacing code by
  the values that leave it, one block wide (`emit/wave.rs:125-170`).
* One translation per container and one host pipeline per title pipeline
  (`gpu/pipeline.rs:61-68`; `gpu/pipeline/make.rs:30-45`); a pipeline
  library that names pipelines by their DXIL
  (`gpu/pipeline/library.rs:254-262`).

Lacks:

* **Working ray tracing.** "The title's ray-tracing structures are never
  built" (`README.md:51`; `progress.md` C3). Seven candidate causes, none
  shown by a run: writes withheld after a miss; eight device feature
  queries answered (`gpu/device.rs:265-290`); no ray-tracing device
  methods; `DispatchX` flags ignored (`gpu/list.rs:1111-1113`); a
  structure view turned into a null view with its address lost
  (`gpu/descriptor/host.rs:1158-1163, 1779-1783`); memory no copy holds
  reads 0 (`gpu/memory.rs:27-29`); 64-lane programs on a 32-lane host
  (`gpu/caps.rs:94-96, 121-131`).
* Any host structure, `RayQuery` operation, structure resource kind or
  flag bit in the emitter (`emit/shader.rs:57-70, 667-705, 929-959`); any
  natural-loop analysis, region live-in and live-out, or memory-effect
  summary (`emit/lower.rs:474-477, 662-672`).
* Any registry of structures, interception of the builder's exports
  (the loader has a place for one, `crates/victory-loader/src/windows.rs:163-183`),
  builder classification, read marks, variant slots or path selector.
* A console reference. The numerators' scale is the emulation's own, and
  the two emulations disagree on facing, on kinds 6 and 7, on the sorting
  network and on a box entered exactly at the extent.

## Checked with Microsoft's tools (in this container, no device)

DirectXShaderCompiler v1.8.2505.1 for Linux, kept outside the repository
(`progress.md`, tier 1b; `scripts/validate-dxil.sh`). The files below are
in session scratch directories (`dxc-ref/` and `rt-layer-reader/` under
the session's scratchpad) and are not durable.

| Fact | Where |
|---|---|
| Reference shaders written here, compiled at `cs_6_6` and accepted by the validator: inline `RayQuery` over triangles with a rule on non-opaque candidates; over procedural boxes with a shader-side triangle test | `dxc-ref/rq_tri.hlsl`, `rq_proc.hlsl`, `README.md` |
| What the emitter must match: a structure is an SRV record of kind 16; handle properties {16, 0}; shader flags bit 25 with bit 4; feature word 0x100000; ops 178 (`allocateRayQuery` with the constant flags), 179, 180 and the state getters to 215 | `dxc-ref/rq_tri.ll:95, 123-125, 267, 275` |
| The validator accepts a structure taken from the resource heap by index, also when the module declares validator version 1.6 as this emitter does (`emit/shader.rs:903-905`). No driver has compiled it | `rt-layer-reader/heap_as.ll:84-86, 100, 109`; `heap_as_v16.ll` |

## The forks

All carry the same `LICENSE` text and `license = "GPL-3.0-or-later"`.
None has host ray-tracing code. DexNom's benchmark note speaks of "a
standalone DXR 1.1 reflection test" on an RX 7900 XTX
(`dexnom:BENCHMARK.md:69`); no `RayQuery` or host structure is in that
tree and its README describes a translated ray-node regression with three
reflections (`dexnom:README.md:92`), so it is read as a test of the
emulated instruction (Inferred).

| Fork | Provenance | Ray-tracing pieces | Candidates |
|---|---|---|---|
| Marvisak | upstream of this fork; `emit/lower/bvh.rs` is byte-identical here | the same emulation | none |
| DexNom | names `ZSlayerHQ/victory-world-runtime` as its basis and credits `xcl-dev/xcl` as having "informed" its work (`README.md:110-111`; neither examined). Source holds title-derived identifiers | view address, thread-count flag, pass classes matched on title shader names, a mode that leaves the whole builder out | ideas only, re-written; never the name matches |
| avatarmanz | started from `fxceless/victory-fork`; the CPU reference, the all-or-none rule and the "what the title does" text are fxceless's commit `578d459`, whose message names XCL as a reference; the README says of `ivall/xcl` "no code of it is here" (`README.md:181-192`) | all-or-none, per-dispatch timestamps, structures kept on the card, an ambient stand-in | the timestamp idea; all-or-none only after the XCL clearance. Not the stand-in: it changes the picture |
| Belobug | the avatarmanz line to a shared commit | as avatarmanz, less its last 17 commits | none |

Nothing is copied from DexNom, avatarmanz, Belobug or fxceless's line
until authorship and the XCL question are cleared. GPURT and RADV have no
entry in `THIRD_PARTY_NOTICES.md`; their licences were not opened.

**Their measurements (Fork).** One machine, an RX 7900 XTX, the title's
renderer patched and an ambient stand-in set
(`avatarmanz:README.md:32-34, 38-39, 48-49`): 99 ms a frame with ray
tracing, 49 without; programs that read title memory by address 56 to 60
ms of the card in 16 dispatches, the builder 1.5 ms in about 275 (commit
`d13e9e0`); in a room a pass their timing names `CopyForBVHUpdate` 13.3
ms in 63 dispatches (a builder stage or a title pass: not established;
the lab note's builder shader names do not include it), traces 24 ms
(`avatarmanz:docs/fork-ray-tracing.md:123-126`). None transfers: their
title memory lies on the card, this fork's host copies are system memory
the GPU reads (`gpu/host.rs:3955-3965`), and their dynamic structures are
partly corrupt. There is no timing for any NVIDIA card.

## Which operations run where

| Guest operation | Today | Native equivalent |
|---|---|---|
| Box-node test: four children, sorted by entry | DXIL, `emit/lower/bvh.rs:299-382` | none per node: no DXR operation returns entry distances or a sorted child list. Whole traversal by `Proceed()` |
| Triangle test: numerators over one denominator, edges inside | DXIL, `emit/lower/bvh.rs:385-523` | the host's fixed-function test, a different one (`spec:2374-2441`); its result is given to the later code in the guest's form. The guest's own test stays available behind host boxes (hybrid) |
| Instance nodes, transforms, masks, culling | the title's code | instance descriptions, the host's transform, the 8-bit mask, cull flags, inside a recognised region |
| Stack, order, extent, hit handling | the title's code | not reproduced. A region is replaced only where its order is proven none or a tie among exactly equal t; the loop's own compares become the host's extents |
| Material and alpha tests on a candidate | the title's code, place unknown | the same code as the body run per non-opaque candidate; a test that counts or accumulates stays software |
| Build, refit, unpack, compact | the title's builder | a host build from the same description, beside the guest's. The guest's build stays until its other consumers are shown absent |

## Committed data met

Paths and sizes only; none was opened.
`crates/victory-formats/src/bcpack-tables.bin`, 75,678 bytes (the
documented exception in `asset-policy.md`). In other reference
repositories: `gta6-explorer/recovered/paths.txt`,
`gta6-explorer/gta6x/data/radio_catalog.json`,
`SixNG/crates/sixng-core/src/names_exe.txt`. No structure dump, shader
dump or node table is committed in any tree read.

## Unknown, and what would settle it

| Question | Settled by |
|---|---|
| Which cause keeps the structures from appearing here | one title run with the logs of dxr-architecture.md, step 2 |
| Wave size and lane dependence of each trace program; the host's lane range | a tally over the title's programs; `victory gpu-info` |
| Per loop, the fields of the workload description: what is kept from a hit, the filter's place and action, the tie rule, mask, the form of each compare, caps, per-instance code, the inverse | a census over `victory shader-info --disasm`; counts only leave the machine |
| How often a cap fires, the biases rays start and end with, whether an infinite ray occurs | counters in a title run of the software path |
| Which id bytes occur, and so whether the facing disagreement matters | a histogram over one bottom level in the same census; a synthetic node shows the disagreement today |
| What the builder's exports are given, how often each is called (`CpuValidateResultX` and the device-only rehydrate among them), whether one dispatch serves one structure or a batch, and the magnitude and size of the title's triangles | a log at the import boundary; nothing intercepts it today |
| Whether the title's CPU or the builder DLL's code reads structure pages; what is read back beside sizes | not by page guards, which over-approximate on shared pages: a standing guard with the fault address attributed to a structure, unbuilt |
| How many programs are refused for "a BVH descriptor with other flags" | `translate --list`, per-reason counts |
| Bytes and count of guest structures; update chains between full builds; frames from build to first trace | the same title run |
| Whether a driver accepts a structure from the heap by index, closes a seam between two bottom levels, repeats a `RayQuery` candidate, and how it treats the other cases the specification leaves open | the probe of dxr-validation.md, per device and driver |
| What the console returns where the two emulations differ | no source |
