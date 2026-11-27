# 0003: Console shader code is translated to DXIL

Status: Reconstructed on 2026-11-26 from source comments and code. The
original record is not in the repository; where this text goes beyond what
the citations say, it says so.

The title of this record is the reconstruction's. The original title is
not known. File and line numbers are as of commit `854470e`; two of the
cited files (`emit/lower.rs`, `emit/mod.rs`) had uncommitted changes by
other work when this was written, so their line numbers have moved since
(`git show 854470e:<path>` shows the cited lines). The tables below name
the item beside each line number for that reason.

## Context

A console shader arrives as a standard DXBC container whose DXIL part may
carry console additions, among them the compressed native GPU code; "The
bitcode may use console-only intrinsics, so the GFX10 code is translated
in general" (`crates/victory-shader/src/lib.rs:3-7`). The host accepts
only PC DXIL. The problem the cited modules answer is how to turn the
console GPU's machine code into a shader that host Direct3D 12 accepts and
that computes what the console computes, and when to do that work.

[architecture.md](../architecture.md) ("Shaders") describes the same
pipeline in the order it runs today. It does not cite the decision.

## Decision

Only what the eleven citing comments state and the cited code implements.

**The form of the translation.**

* The native program becomes a "One-to-one SSA form of a native program
  (decision 0003, step 1)" (`crates/victory-shader/src/ir.rs:1`). The
  header goes on: every machine instruction becomes one instruction of
  the form, and "Nothing is raised: an instruction keeps its opcode and
  decoded fields, so its exact semantics stay with it" (`ir.rs:3-9`).
* "Control-flow structurisation (decision 0003): the program as one lane
  runs it, without EXEC" (`crates/victory-shader/src/structure.rs:1-2`).
  Lane masks become per-lane booleans, and the control-flow graph "keeps
  its shape and must be reducible (DXIL's only requirement)"
  (`structure.rs:4-5`, `22-23`).
* "I/O raising (decision 0003): what a program's input registers, its
  interpolation and its exports mean" (`crates/victory-shader/src/io.rs:1-2`).
* "Resource tracking (decision 0003): where each descriptor a program uses
  comes from, in terms of the root signature"
  (`crates/victory-shader/src/resource.rs:1-2`).
* "Whole-wave sections (decision 0003)": code the console runs in every
  lane of a wave is simulated on the console's lanes with symbolic values
  and lowered by what a value turned out to be; "No instruction sequence
  is matched" (`crates/victory-shader/src/emit/wave.rs:1-12`).
* "IR to DXIL, one machine instruction at a time (decision 0003)"
  (`crates/victory-shader/src/emit/lower.rs:1`).
* "DXIL emission (decision 0003, step 3): a console vertex, pixel, compute
  or mesh shader as a PC DXIL container (SM 6.6) that host D3D12 accepts"
  (`crates/victory-shader/src/emit/mod.rs:1-2`). The same header is the
  contract of what the host binds for a translated shader
  (`emit/mod.rs:7-197`).

**When it runs and what is kept.**

* "Shader containers are shared by content hash and translated once, on
  first use (decision 0003)" (`crates/victory-gpu/src/pipeline.rs:8-10`).
* "Translated shaders kept on disk (decision 0003): under the configured
  cache folder, keyed by the container's content and by what the
  translation depends on" (`crates/victory-gpu/src/shader_cache.rs:1-4`).
  A container that does not translate is kept too, with the reason
  (`shader_cache.rs:9-10`).
* "The offline batch: every shader found goes through the translator, and
  what it cannot handle yet is tallied (decision 0003)"
  (`apps/victory/src/translate.rs:1-2`).
* The warm-up sends every distinct container "through the shader cache
  `run` reads (`<cache>/shaders`, decision 0003), by the same code, so a
  run finds it translated or refused"
  (`apps/victory/src/translate.rs:320-322`).

Inferred: the decision chose translation of the native machine code over
using the container's own bitcode. The citing comments do not say so; the
crate header's sentence about console-only intrinsics is the nearest
statement and it carries no citation.

Inferred: the decision asked for exactness per instruction rather than
recognition of compiler patterns. Three citing headers say this in their
own terms ("Nothing is raised", "No instruction sequence is matched", "one
machine instruction at a time"), but none attributes the principle itself
to the record.

## Consequences visible in the code

* A value the emitter cannot express is `Missing` with a reason, "and only
  fails the program when something with an effect (an export, a branch)
  needs it" (`emit/lower.rs:5-7`). A program that fails is refused, and
  its draws are dropped by the GPU layer (audit, A-C5, A-C6, A-C8).
* A compute program that depends on the lane layout "declares the
  console's wave size (`WaveSize`), so the host device must support it"
  (`emit/mod.rs:3-5`). This is the host dependency G2 in
  [gpu-compatibility.md](../gpu-compatibility.md).
* The stage order is fixed in code: `translate` takes container bytes as
  the title passes them at pipeline creation, and `emit` is "the same with
  the earlier stages already run" (`emit/mod.rs:178-180`).
* The translator's identity is part of the cache key, so a changed
  translator never serves an old translation (`shader_cache.rs:2-4`,
  `52-69`).

`shortcut:` comments in the citing files, which name what was left open.
Counts are at `854470e`: `emit/lower.rs` 33, `resource.rs` 5,
`emit/mod.rs` 1 (all under `crates/victory-shader/src/`),
`crates/victory-gpu/src/pipeline.rs` 3 (listed under decision
[0007](0007-host-gpu.md), which that file also cites), and none in the
other citing files. Those in the shader crate that bear on the audit's
correctness findings (paths under `crates/victory-shader/src/`):

| Where | Item | What the shortcut leaves open |
|---|---|---|
| `emit/lower.rs:490` | const `LOOP_LIMIT` | "a genuine loop of more iterations than this is cut short too" (the loop limit of 2^20; audit A-C4, A-P3; the rule that applies now is milestone C7 in [progress.md](../progress.md), see below) |
| `emit/lower.rs:525` | const `LOOP_TOTAL_LIMIT` | "a genuine nest that goes round more often than this in one lane is cut short too" (the limit of 2^22 back edges on all of a lane's loops together, added 2026-11-27; the same rule of milestone C7 applies) |
| `emit/lower/crosslane.rs:114` | fn `shape` | an f16 sum is taken in f32 and rounded once, where the console rounds each step |
| `emit/lower/half.rs:16` | fn `narrow16` | rounding toward an infinity, or a mode not known at that point, rounds to nearest even |
| `emit/lower/masks.rs:239` | fn `atom` | "some lane of the quad is set" is taken as true; wrong if such a section had a side effect |
| `emit/lower/crosslane.rs:238` | fn `cross_lane` | no wave size for mesh shaders, so host lanes cannot follow the console's there |
| `emit/lower/stage.rs:284` | fn `mesh_prologue` | mesh output counts are the declared maxima (audit A-C14) |
| `emit/lower/scalar.rs:34` | fn `sop` | the float MODE register is not modelled |
| `emit/lower/valu.rs:334` | fn `valu_op` | "DXIL has no fused f32 multiply-add; FMad lets the driver fuse" (G4, audit A-C9) |
| `emit/lower/half.rs:428` | fn `valu16_op` | fused f16 multiply-adds as FMad in f32: the sum rounds twice |
| `resource.rs:1814` | fn `lane_phi` | what a lane holds after some passes of a loop is not traced |
| `resource.rs:1896-1900` | fn `waterfall` | "only the accesses are proven"; other per-lane work in such a loop is taken to run once for each lane |
| `emit/mod.rs:791` | fn `emit_with` | older containers fall back to the group's lane count for the mesh stride |

The loop limit is the one shortcut here that a dated decision of the
owner now governs. [progress.md](../progress.md) (milestone C7 under
"Correctness", and the session log of 2026-11-26, "Owner's decisions on
the loop limit") rules that a loop cut short by the limit is a
correctness failure to investigate and never an accepted way to render,
that stops are counted and logged by shader and stage, and that a stopped
lane ends in a defined, stage-appropriate way with no output left
undefined. That ruling is later than anything the citing comments say and
is not part of decision 0003 as far as the citations show.

Where a citing comment and the code differ:

* `shader_cache.rs:3-4` says the key is such that "rebuilding the rest of
  VIctory keeps" the translations. The key's translator part is
  `victory_shader::SOURCE_HASH`, which the crate's build script computes
  over every file under the crate's `src`, its manifest, the build script,
  the workspace lock file and the compiler's version
  (`crates/victory-shader/build.rs:1-41`). A change to the lock file, or
  to a test module under `src`, therefore changes the key although the
  translation is the same. The audit records this as A-P8.
* The doc comment of `LOOP_LIMIT` (`emit/lower.rs:347-353` at `854470e`)
  says a lane that reaches the limit "leaves the program as killed". The
  audit found the code let such a lane run on (A-C4). The comment does not
  cite the decision; it is listed here because it sits in a citing file.
  The correction, under milestone C7, was uncommitted work by others in
  the working tree when this record was written, and had already changed
  this comment and its line numbers; this record does not describe or
  assess it.

## Steps

| Step | Cited at | What the citing code does |
|---|---|---|
| 1 | `crates/victory-shader/src/ir.rs:1` | builds the one-to-one SSA form of the native program |
| 2 | not cited anywhere | unknown |
| 3 | `crates/victory-shader/src/emit/mod.rs:1` | emits the DXIL container and defines what the host binds |

Whether the record had steps after 3 is unknown.

Inferred: step 2 covered what runs between the SSA form and emission,
which in the code is structurisation, I/O raising and resource tracking.
Each of those headers cites the decision without a step number, so this
cannot be confirmed from the source. The history does not settle it
either: commit `754619f` (2026-11-23 14:42) added the step 1 citation
together with the structurisation header, which cited the decision with
no step from the start, and commit `2849477` (2026-11-23 23:10) added
the step 3 citation together with the I/O, resource-tracking, wave and
lowering headers. No citation of a step 2 was ever added.

## Unknown

* The original title, author and date of writing. The history gives the
  dates of the citations: the first were added by commit `754619f`
  (2026-11-23 14:42, the third commit of the repository), the last by
  `c47b529` (2026-11-24). Those commits are not in a shallow checkout cut
  at `552208c`; see "What the history shows" in [README.md](README.md)
  for how they were read. Inferred: the record existed in some form by
  2026-11-23. No record was ever a file in the repository.
* Which alternatives were considered. No citing comment names one.
  (Candidates the reconstruction can think of, as examples only: using
  the container's own bitcode where it has no console intrinsics,
  translating to HLSL or SPIR-V instead of DXIL, interpreting, or
  matching compiler idioms.)
* What the record said about pipeline creation. At `754619f` the header
  of `crates/victory-gpu/src/pipeline.rs` named it as open work:
  "Translate through the cache here (TODO.md, GPU: pipeline creation,
  decision 0003)". `2849477` replaced that sentence with the one cited
  today. `TODO.md` was never a file in the repository either.
* Why shader model 6.6 was chosen as the target. The code uses its
  directly indexed heaps (`emit/mod.rs:17-21`), which is a consequence
  and not a stated reason.
* What step 2 was, and whether later steps existed.
* Whether the items the `emit/mod.rs` header marks as "added" on
  2026-11-23 to 2026-11-25 (comparison sampling, vertex fetch, constant
  buffers in the heap, heap index clamps and the rest) were part of the
  record or came after it. One of them, title memory, cites decision
  [0005](0005-title-memory-and-bvh.md) instead.
* Any measurements: translation times, the number of programs covered at
  the time, cache sizes.
* What the record said about programs that cannot be translated, beyond
  that they are tallied.

## References

Eleven citations in code.

| Where | Item | Step | What the citing sentence says |
|---|---|---|---|
| `apps/victory/src/translate.rs:2` | module header | none | the offline batch tallies what the translator cannot handle yet |
| `apps/victory/src/translate.rs:321` | doc comment of `warm` | none | the warm-up fills the shader cache a run reads, by the same code |
| `crates/victory-gpu/src/pipeline.rs:10` | module header, lines 8-10 | none | containers are shared by content hash and translated once, on first use |
| `crates/victory-gpu/src/shader_cache.rs:1` | module header | none | translated shaders are kept on disk, keyed by content and by what the translation depends on |
| `crates/victory-shader/src/emit/lower.rs:1` | module header | none | IR to DXIL, one machine instruction at a time |
| `crates/victory-shader/src/emit/mod.rs:1` | module header | 3 | DXIL emission for vertex, pixel, compute and mesh shaders |
| `crates/victory-shader/src/emit/wave.rs:1` | module header | none | whole-wave sections are simulated and lowered to host wave operations |
| `crates/victory-shader/src/io.rs:1` | module header | none | I/O raising: inputs, interpolation, exports |
| `crates/victory-shader/src/ir.rs:1` | module header | 1 | the one-to-one SSA form |
| `crates/victory-shader/src/resource.rs:1` | module header | none | resource tracking in terms of the root signature |
| `crates/victory-shader/src/structure.rs:1` | module header | none | control-flow structurisation without EXEC |

`crates/victory-gpu/src/pipeline.rs` cites this decision and decision
[0007](0007-host-gpu.md) in the same paragraph.

Mentioned as missing in `docs/audit-2026-11-26.md:127` (A-B5) and in
`docs/progress.md` ("Needed from the owner", item 6).
