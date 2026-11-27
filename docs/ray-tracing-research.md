# Ray tracing: what is known

The record behind [dxr-architecture.md](dxr-architecture.md),
[dxr-compatibility.md](dxr-compatibility.md) and
[dxr-validation.md](dxr-validation.md). Read on 2026-11-27 at commit
`ae1c915`, from this repository, four forks of VIctory, the `xbox-hle-lab`
reference and the DXR functional specification; corrected the same day
after three reviews. **Nothing was run: no GPU and no title file was
available.** The owner's decisions of 2026-11-27 (an experimental DXR
backend is to be developed) are recorded one commit later, in
`progress.md` at `0eb1e82`; this worktree does not hold that entry.

Every fact has a source and a place.

* **Code**: read in code or a document opened for this record.
* **Fork**: reported by a fork or a reference repository. Their runs, their
  machines, their words; not observed here.
* **Inferred**: reasoning over the two above.
* **Unknown**: looked for and not found.

The state of work uses the owner's words (implemented, tested, type-checked
for Windows only, hardware-validated, speculative). Every part of a host
backend named in these four documents is **speculative**.

Paths without a prefix are this repository (`emit/` is
`crates/victory-shader/src/emit/`, `gpu/` is `crates/victory-gpu/src/`).
`marvisak:`, `dexnom:`, `belobug:` and `avatarmanz:` are the forks, at
`63bcca4`, `8114ff7`, `4eb6cfd` and `014d52d`. `lab:` is `xbox-hle-lab` at
`8863028`. `spec:` is `d3d/Raytracing.md` of microsoft/DirectX-Specs by
line: 9,755 lines, last change-log row v1.49, sha256 `0c3d494b...59caa7`.
The copy read is in a session scratch directory and is not kept anywhere
durable; the upstream commit was not recorded.

## What the title does

| Fact | Source | Where |
|---|---|---|
| The only hardware step is one instruction in two forms, `image_bvh_intersect_ray` and `image_bvh64_intersect_ray`. One call tests one ray against one node. Its operands are a node pointer, an extent, an origin, a direction and an **inverse direction the program supplies** | Code | `emit/lower/bvh.rs:1-4, 10-12, 88, 110-115` |
| The stack, instance transforms, culling and the comparison of t are the program's own code. The instruction does no extent test for triangles | Code (the emulation's contract); Fork | `emit/lower/bvh.rs:44`; `avatarmanz:crates/victory-shader/src/bvh.rs:8-11` |
| The title uses no ray-tracing API to trace | Fork (both forks' documents). A reference run shows only that the tier was never queried in two runs and that builds arrive through the DLL | `avatarmanz:docs/fork-ray-tracing.md:16-19`; `lab:docs/BARE-METAL-RUN-13-15.md:147-153` |
| Loops, as the forks describe them: a per-thread stack in shared memory; loads through raw 40-bit addresses; lane masks kept as numbers on some paths; forms written for 64-lane waves; an invalid first sorted child ends a walk; 59 of 64 trace programs translate there | Fork (notes; no disassembly, IR or fixture of a loop is committed anywhere) | `avatarmanz:docs/fork-ray-tracing.md:18-19, 55-58`; `avatarmanz:crates/victory-shader/src/bvh.rs:200-202` |
| Five shadow-mask tracers take a structure's address from a heap descriptor picked by a computed index; the title created none of them in the fork's runs | Fork | `avatarmanz:docs/fork-ray-tracing.md:129-132` |
| At least one shadow tracer walks the structure as a whole wave: uniform branches, lane masks pushed to memory and popped as numbers | Code (a comment that names a title entry point; no title program is in the repository) | `emit/synthetic_tests.rs:521-525`; `emit/lower/flow.rs:125-127` |
| A counter on one traversal shader saw node visits and finite, in-range triangle candidates | Fork | `dexnom:BENCHMARK.md:7-9` |
| The structures are built by `XboxBvhBuilder.dll`, shipped with the title. Its exports take Direct3D 12 build descriptions: a build, a batched console form, three rehydrate forms, a copy, a prebuild-size query | Fork (static analysis of the DLL, never executed) | `lab:docs/CONSOLE-DLL-EXPORTS.md:42-66`; `lab:docs/GHIDRA-BVH.md:84-92` |
| The builder works in compute: 52 embedded shaders, 16 root signatures, root views and constants only | Fork | `lab:docs/GHIDRA-BVH.md:14, 80, 104` |
| The builder dispatches through `DispatchX` and takes upload memory from list slot 148, where it puts its **instance descriptions**: memory the layer hands out and can read | Code | `gpu/list.rs:1111-1113, 1205-1209, 1830` |
| `DispatchX` flag 0x20 means the counts are threads, not groups. Taken for groups, a rehydration stage ran up to 64 times its threads and wrote over neighbouring structures | Fork (no public source cited) | `dexnom:crates/victory-gpu/src/draw.rs:2410-2416` (DexNom `bc3cd4b`); `avatarmanz:crates/victory-gpu/src/draw.rs:4310-4314` (`810efe5`) |
| The static world's structures ship packed and are unpacked on the GPU once (3,000 in the opening); dynamic ones are built or refitted every frame, and the title reads a compacted size back before a copy | Fork | `avatarmanz:docs/fork-ray-tracing.md:77-81, 110-118` |
| A rehydrate path exists in the builder; its description names index and vertex buffers as well as the packed source, and refuses an update flag | Fork | `lab:docs/GHIDRA-BVH.md:68-76` |
| The title makes shader views of its structures: no resource, view dimension 11, the structure's address in the description | Fork | `dexnom:crates/victory-gpu/src/descriptor/host.rs:2804-2815` (DexNom `5f4c3df`) |
| 64 programs hold traversal loops; the builder has 52 programs | Fork | `dexnom:docs/fork-ray-tracing.md:25-26, 35-36` |
| Both forks' documents call a mapping onto DXR "not practical": the title never describes the geometry to the layer and the loops live in 64 programs | Fork (their position, stated before any attempt) | `avatarmanz:docs/fork-ray-tracing.md:32-36`; `dexnom:docs/fork-ray-tracing.md:33-37` |
| Structures must lie below 2^40 | Code | `crates/victory-hle/src/modules/xmem.rs:72-78` |
| Per loop: the rule for two hits at equal t, alpha-tested hits, exit on a first hit, mask width and test, what is stored from a hit, whether the inverse passed is 1/direction, whether the extent passed is the running best t, where the root pointer comes from, the wave size declared | **Unknown** | not in any note read |

## The guest structures

| Fact | Source | Where |
|---|---|---|
| A node pointer's low three bits are its kind: 0-3 a triangle of a triangle node, 4 a box node of 16-bit floats, 5 one of 32-bit floats, 6 an instance, 7 a procedural primitive. The node is at `base * 256 + (pointer & !7) * 8` | Code, from public sources the file names (AMD's ISA guide, AMD GPURT, Mesa RADV), not from title data | `emit/lower/bvh.rs:4-8, 19-21` |
| Box nodes hold four child pointers and four boxes: 64 bytes with 16-bit float bounds, 128 bytes with 32-bit | Code | `emit/lower/bvh.rs:22-24` |
| A triangle node is 64 bytes and holds up to four triangles over five vertices; the pointer's kind picks which three. **Which triangles exist is not in the node**: a node with fewer keeps other data in the spare vertex bytes | Code; Fork | `emit/lower/bvh.rs:33-36`; `avatarmanz:crates/victory-shader/src/bvh.rs:41-49` |
| The id dword at byte 60 has a byte per triangle: two bit pairs choose the barycentrics returned; higher bits are "opaque and the like" | Code; Fork | `emit/lower/bvh.rs:37-40`; `avatarmanz:crates/victory-shader/src/bvh.rs:634` |
| Programs build a 128-bit descriptor in code. The translation accepts one configuration (sorted children, no box growth, no node bound, barycentric return) and refuses every other. How many programs it refuses for this is not counted | Code | `crates/victory-shader/src/resource.rs:1340-1347, 1370-1372`; `audit-2026-11-26.md` A-C6 |
| The base and the pointer are run-time register values. In the 64-bit form programs use one descriptor with base 0 and fold a structure's address into the pointer | Code; Fork | `emit/lower/bvh.rs:150-171`; `avatarmanz:crates/victory-shader/src/bvh.rs:17-20` |
| The instruction does not read instance or procedural nodes. This fork returns four invalid words; the other emulation returns a triangle miss | Code | `emit/lower/bvh.rs:21, 215-224`; `avatarmanz:crates/victory-shader/src/bvh.rs:66-70` |
| Header, 64 bytes: version 6 and marks, size in 64-byte nodes, element count, bounds, offsets of box nodes and elements, root pointer, offset of a parent table | Fork (from their dumps of title memory; none committed) | `avatarmanz:docs/fork-ray-tracing.md:68-73` |
| Instance, 128 bytes: a 3x4 matrix, id and mask, flags, root pointer, the bottom level's address in 256-byte units | Fork (same) | `avatarmanz:docs/fork-ray-tracing.md:74-76` |
| The header has a top-level flag; a triangle made NaN is "invalidated" | Fork (validator strings) | `lab:docs/CONSOLE-DLL-EXPORTS.md:164` |
| Mask width, the meaning of flag bits, geometry and primitive ids, whether procedural geometry is submitted | **Unknown** | `lab:docs/CONSOLE-DLL-EXPORTS.md:169-173` names a pass only |

No document maps a guest structure to a host one. The specification rules
out reuse of the bytes: a host structure is opaque and comes only from a
build (`spec:2001-2046`).

**Where host geometry could come from**, none read by this fork today:
dynamic structures from build descriptions at the DLL's export boundary
(static analysis only); the static world, which ships pre-built, from
rehydrate descriptions or by decoding nodes (layouts are a fork's report);
instances from the slot-148 upload memory (Code: the one place instance
data is visible to the layer now).

## This fork today

Has (state: implemented; tested only as far as stated):

* The instruction emulated in DXIL, one node at a time, the node read from
  a root view or through the title-memory page table
  (`emit/lower/bvh.rs:74-225`; `gpu/memory.rs:1-33`). Both the box test
  and the triangle test are computed for every call
  (`emit/lower/bvh.rs:212-224`), after a read of 16 dwords and a guarded
  read of 12 more (`:193-201`).
* Tests of it against a CPU reference on a hardware device and WARP, with
  inputs built so the arithmetic is exact. They compile only on Windows; no
  run of them is recorded by name for any device, and no shader test run
  at all for NVIDIA (`emit/mod.rs:201-202`; `emit/bvh_tests.rs:1-7`;
  `progress.md:115`).
* The host's ray-tracing tier in the capability record. `gpu-info` prints
  met or DEGRADED for "tier 1.1", not the number, and a failed query reads
  as tier 0 (`gpu/caps.rs:47-50, 177-181, 200-228`; `gpu/host.rs:1214`).
* GPU time per noted command, summed by kind and pipeline, in the
  diagnostics build (`gpu/host.rs:425-461, 990-997`). Timing mode runs each
  list alone (`gpu/host.rs:1474-1482`).

Lacks:

* **Working ray tracing.** "The title's ray-tracing structures are never
  built" (`README.md:51`; C3). Seven candidate causes, none shown by a
  run. From the audit: writes withheld after a miss
  (`gpu/memory.rs:310-323`), eight device feature queries answered
  (`gpu/device.rs:265-290`), no ray-tracing device methods
  (`audit-2026-11-26.md:64`). From the forks' differences: `DispatchX`
  flags ignored (`gpu/list.rs:1111-1113`); a structure view turned into a
  null 2D view with its address lost (`gpu/descriptor/host.rs:1158-1163,
  1779-1783`); memory no copy holds reads 0 (`gpu/memory.rs:27-29`);
  programs that declare 64-lane waves get no pipeline on a 32-lane host
  (`gpu/caps.rs:94-96, 121-131`; `README.md:52`), builder programs among
  them in a fork's history (avatarmanz `8eefadc`). **Inferred**: which
  matters is not known.
* Any host acceleration structure, `RayQuery` or DXR call, and any
  recogniser for loops: lowering is per instruction
  (`audit-2026-11-26.md:64`; `emit/shader.rs:906-912`).
* A software baseline and ground truth. With no structures there is
  nothing to compare a host path with on title data. The numerators' scale
  is the emulation's own (`emit/lower/bvh.rs:46-47`), and the two
  emulations disagree on kinds 6 and 7, on the sorting network and on a
  box entered exactly at the extent. Neither knows the console's rounding,
  tie order or edge result.

## The forks

All carry the same `LICENSE` text and `license = "GPL-3.0-or-later"`
(`Cargo.toml:11`). None has host ray tracing.

| Fork | Provenance | Ray-tracing pieces and their authors | Candidates |
|---|---|---|---|
| Marvisak | upstream of this fork; `emit/lower/bvh.rs` is byte-identical here | the same emulation | none |
| DexNom | names `ZSlayerHQ/victory-world-runtime` at a commit as its basis (`README.md:110`; not examined) and credits an XCL repository, `xcl-dev/xcl`, as having "informed" its work (`:111`; licence not opened). Source holds title-derived identifiers and one captured view description | view address (`5f4c3df`, DexNom); thread-count flag (`bc3cd4b`, DexNom); `pipeline/ray_tracing.rs` (`ab34a8b`, Codex); pass switches, a counter probe, 64-lane programs on 32-lane hosts | ideas only, re-written |
| avatarmanz | started from `fxceless/victory-fork` (`README.md:181-182`). The CPU reference `bvh.rs`, the builder all-or-none rule (`crates/victory-gpu/src/pipeline.rs:246-262`) and the "what the title does" text are fxceless's squashed commit `578d459`, whose message says "XCL by ivall and contributors was used as a reference". The README says of `ivall/xcl` (GPL-2.0-only, their label): read, "no code of it is here" (`:192`) | all-or-none (`578d459`, fxceless); per-dispatch timestamps (`84917c3`, local); thread counts (`810efe5`, after DexNom); fault-in copies; an ambient stand-in | the timestamp idea; all-or-none only after the same XCL clearance. Not the stand-in: it changes the picture |
| Belobug | the avatarmanz line to a shared commit; no ray-tracing commit of its own | as avatarmanz, less its last 17 commits | none |

The two forks link different XCL repositories. Nothing is copied from
DexNom, avatarmanz, Belobug or fxceless's line until authorship and the XCL
question are cleared; `xbox-hle-lab`'s xcl patches stay excluded
(`audit-2026-11-26.md`, licence table). GPURT and RADV have no entry in
`THIRD_PARTY_NOTICES.md`; their licences were not opened.

**Their measurements (Fork).** One machine, an RX 7900 XTX, the title's
renderer patched (patches 1, 2, 5, 6, 7) and an ambient stand-in set by
the ray-tracing launch switch (`avatarmanz:README.md:32-34, 48-49`).

* 99 ms a frame standing with ray tracing, 49 ms without (`README.md:38-39`).
* Run rts-1: programs that "walk the structures" 56 to 60 ms of the card a
  frame in 16 dispatches, the builder 1.5 ms in about 275 (commit
  `d13e9e0`). "Walks" there means any program that reads title memory by
  address (`crates/victory-gpu/src/host/rtstamps.rs:5-7, 31`); the commit's
  bench finds two copies a wave double the time. The node test is not
  timed apart.
* Run light-13, a room: a builder copy stage 13.3 ms in 63 dispatches,
  traces 24 ms, split by pass (`docs/fork-ray-tracing.md:123-126`).

None transfers to this fork: their title memory lies in pages on the card
(`docs/fork-ray-tracing.md:85`), this fork's host copies are system memory
the GPU reads (`gpu/host.rs:3955-3965`), and their dynamic structures are
partly corrupt (`README.md:53-55`). fxceless's line was developed on an
RTX 4070 with ray tracing off by default and nothing "lit by it yet"
(`avatarmanz:docs/README-fxceless.md:38, 137`): the only NVIDIA note, no
timing. DexNom timed no pass alone (`dexnom:docs/fork-ray-tracing.md:78`).

## Which operations run where

| Operation | Today | Could a host structure take it? |
|---|---|---|
| Box-node test: four children, sorted by entry | DXIL, `emit/lower/bvh.rs:299-382` | not as a node test: no DXR operation returns entry distances or a sorted child list. Only as whole box traversal by `RayQuery` |
| Triangle test: numerators over one denominator, edges inside | DXIL, `emit/lower/bvh.rs:385-523` | the host's own is a different test. The title's can stay in the shader behind procedural boxes (`spec:1166-1171`) |
| Instance nodes, transforms, masks, culling | the title's code | inside a recognised whole loop; or left to the title's code, with only one bottom level's walk replaced (option F). Neither is recognised today |
| Stack, order, extent, hit handling | the title's code | only inside a recognised loop whose result does not depend on order |
| Build, refit, unpack, compact | the title's builder | no: the title reads sizes back and the software path reads the result. A host build is added work |

## Committed data met

Paths and sizes only; none was opened.
`crates/victory-formats/src/bcpack-tables.bin`, 75,678 bytes (the documented exception in `asset-policy.md`; 77,982 bytes
in three forks). In other reference repositories:
`gta6-explorer/recovered/paths.txt` (212,510 bytes),
`gta6-explorer/gta6x/data/radio_catalog.json` (1,665,603 bytes),
`SixNG/crates/sixng-core/src/names_exe.txt` (47,414 bytes). No structure
dump, shader dump or node table is committed in any tree read. The origin
of the instruction fixtures in `crates/victory-shader/src/resource.rs`
(test module) is not stated there.

## Unknown, and what would settle it

| Question | Settled by |
|---|---|
| Which cause keeps the structures from appearing here | one title run with the logs of dxr-architecture.md, step 1 |
| Wave size and lane dependence of each trace program; the host's lane range | a tally over the title's programs; `victory gpu-info` |
| Loop shapes, and per loop the unknowns of the first table's last row | a census over `victory shader-info --disasm`; counts only leave the machine |
| How many programs are refused for "a BVH descriptor with other flags" | `translate --list`, per-reason counts (A-C6) |
| Whether builder and trace dispatches share a host list or a host piece | a count in one run (`gpu/hold.rs:8-15`) |
| Bytes and count of guest structures, static and dynamic; structures per 64 KiB page | the same run |
| What the builder's exports are given | a log at the import boundary; nothing intercepts it today |
| What the console returns where the two emulations differ | no source |
