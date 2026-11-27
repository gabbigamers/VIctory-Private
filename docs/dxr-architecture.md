# A host ray-tracing backend: options, design, decision

The question: can the host's ray-tracing hardware replace or supplement the
traversal the title's own translated compute programs do, with the same
picture. Facts and their evidence are in
[ray-tracing-research.md](ray-tracing-research.md); guest and host
semantics side by side in [dxr-compatibility.md](dxr-compatibility.md); how
it would be measured in [dxr-validation.md](dxr-validation.md).

**Nothing here is built and nothing was run; every part is speculative.**
The owner decided on 2026-11-27 that an experimental backend is to be
developed, behind a switch, off by default (`progress.md` at `0eb1e82`).
This document does not reopen that. It says what is known not to hold,
what each piece waits on, and in which order the pieces can be judged.
`gpu/` is `crates/victory-gpu/src/`, `emit/` is
`crates/victory-shader/src/emit/`, `spec:` is the DXR specification by line.

## Prerequisites, stated first

| # | Prerequisite | Evidence |
|---|---|---|
| 1 | **There is nothing to traverse yet.** The title's structures never appear in this fork (C3). No guest structure means no input for a host one, no software result to compare with, no trace cost | `README.md:51` |
| 2 | **A software pipeline may not exist on the target.** A program that declares 64-lane waves gets no pipeline on a host whose waves are 32 lanes wide (G2). At least one tracer is a wave-dependent walk; the wave size of no tracer is recorded. For such a program the fallback is a dropped dispatch, and there is no software result on that device to validate a host path against. If the tally shows 64-lane tracers or builder stages, G2's lane-carrying lowering comes before everything else on that host | `gpu/caps.rs:94-96, 121-131`; `emit/synthetic_tests.rs:521-525` |
| 3 | **The unit of translation is one node test.** Hardware traversal replaces a whole loop. Nothing recognises a loop. Fork notes describe loop behaviour; no disassembly or fixture is committed | `emit/lower/bvh.rs:1-4` |
| 4 | **Host geometry has no source yet**: build descriptions at the DLL boundary (dynamic), rehydrate descriptions or node decoding (static), slot-148 upload memory (instances). None is read here | ray-tracing-research.md |
| 5 | **No timing exists for NVIDIA.** The forks' AMD figures were taken with title memory on the card, which this fork does not do | the same |
| 6 | **The title's files are gated** | `progress.md`, "Blockers", gate |

## Options

| | Option | Keeps the guest's results? | Finding |
|---|---|---|---|
| A | **Whole loop, host triangles**: a recognised loop becomes one `RayQuery` over host triangles | No, by specification: exclusive extents, one triangle on a shared edge, a float t of unstated precision, results that differ between implementations (`spec:2374-2441, 2551-2552`). Rule 3 of [gpu-compatibility.md](gpu-compatibility.md) forbids it as an equal path | measured in the probe to size the difference; offered only under a written tolerance the owner accepts |
| B | **Node level**: the host answers single node tests | n/a | no DXR operation returns per-child entry distances or a sorted child list (`spec:1166-1171`; `emit/lower/bvh.rs:26-30`). A structure per node is expressible (`spec:5320-5338`) but the shader would redo the box arithmetic and gain nothing |
| C | **Whole loop, host boxes, the title's triangle test**: one procedural box per guest triangle pointer; each candidate runs the emulated test | Only for a loop whose result does not depend on candidate order, on rays where the guest's box chain culls nothing the full test hits, and where the host presents every candidate the guest accepts. The first is a property to prove per loop. The second fails by a counted rate. The third is promised only for rays that exactly intersect the input box (`spec:1159-1163, 2155-2158`); beyond that it is each driver's behaviour | the main candidate. Not equivalent in general: see "Rule 3 and option C". Unproven for speed |
| D | **Structure conversion**: host structures from guest data | n/a (a means for A, C and F) | byte-level reuse is excluded (`spec:2001-2046`) |
| E | **Software only**: today's path | Yes: it is the reference | the default, and the fallback wherever a software pipeline exists |
| F | **Bottom level only**: the title's own top-level code stays (instances, transforms, masks, flags, order across instances); only the walk of one bottom level becomes a `RayQuery` over a single-instance host top level reached by heap slot (`spec:5320-5338, 6855`) | as C inside one bottom level; masks, instance transforms and flags leave C's open rows because they stay the title's arithmetic | not shown feasible: needs the instance entry recognised inside the loop, one query per instance visited, a wrapper structure per bottom level (a fork counts 3,000 static ones). Measured beside C in the probe |

**Inline or pipelines.** No difference in trace results between the two
forms is shown: both leave candidate order to the system and both can run
a custom test per procedural primitive (`spec:678-704, 1159-1171`).
Pipelines are ruled out on program structure. A trace program is a compute
program with thread groups, and at least one walks as a whole wave;
ray-tracing shaders have no thread execution syncing, and wave results are
valid only up to `TraceRay`, `CallShader` and `ReportHit` (`spec:2726-2755`).
`RayQuery` works inside a compute shader at tier 1.1 (`spec:1139-1141`),
which the capability record holds (`gpu/caps.rs:50`). The emitter would
gain the RayQuery `dx.op` calls (178 to 215, less 213), one shader flag and
one resource kind (`emit/shader.rs:899-912`). Speed is unmeasured; inline
gives the system "far less opportunity" to optimise (`spec:1145`), and the
"less efficient" said of intersection shaders (`spec:683-684`) applies to
inline procedural candidates only by analogy.

## Rule 3 and option C

On a ray where the guest's box chain culls a triangle the full test hits,
C can answer differently from the software path. If the ray exactly
intersects the host box, C hits where software misses, on every driver.
If it does not, whether the candidate appears is the implementation's
choice (`spec:1162-1163, 2087-2110`). Guest hits accepted through float
rounding can likewise be lost. That is the defect held against A. The
owner's decision allows results that are not bit-identical only with a
tolerance written first and the software-only cases named. So one of:

* **(a)** re-apply the guest's box tests to each candidate with guest
  arithmetic, along its ancestors (a fork reports a parent table in the
  header, `avatarmanz:docs/fork-ray-tracing.md:72-73`). Removes extra hits,
  not lost ones. Cost unmeasured; a shrinking extent makes the guest's
  culling depend on visit order, which is unexamined.
* **(b)** an exception to rule 3 the owner accepts, per loop class, with
  the counted rate on title data beside it.
* **(c)** C only for programs and scenes where the count is zero.

A lost hit has no tolerance in any of them.

## Design of the hybrid

For C (and F) behind E. Written to be judged.

**Selection happens on the GPU, at execution.** The pipeline is bound when
the title records a list, not when it is submitted (`gpu/draw.rs:646-658,
1096-1128`); lists are held, cut and reset before they run
(`gpu/hold.rs:1-15`; `gpu/queue.rs:115-131`; `gpu/list.rs:178-181`). A
choice made in `State::bind` could trace a stale structure. And the top
level a dispatch traces may be known only on the GPU: base and pointer are
run-time register values (`emit/lower/bvh.rs:150-171`). So:

* An eligible program's **hardware variant holds both loops**. It looks the
  top level's guest address up in a table the runtime rewrites at
  submission, as the title-memory page table already is
  (`gpu/memory.rs:11-20`): heap slot of the host top level, and whether it
  is current. Not current: the software loop, in the same program.
* The plain software variant stays a pipeline of its own.
* Open: whether shader model 6.6 heap indexing may yield a structure handle
  was not checked against the validator; the size and compile time of a
  two-loop program are unmeasured.
* Capability from `HostCaps` only (`raytracing_tier >= 11`), no names
  (rule 1). `VICTORY_RT_BACKEND=software|auto`, default `software`; it
  enters the shader cache key (`gpu/shader_cache.rs:57-80`, rule 8).

**Eligibility, per program.** All of:

1. a software pipeline of it exists on this host;
2. the loop matches a recognised shape with its live values proven, and
   its result is **proven independent of candidate order**: a yes-or-no
   result with nothing else kept, or a strict minimum whose ties the
   title's own code breaks by a key computable from the candidate alone;
3. its per-candidate action is idempotent (minimum, logical or), until the
   device has a recorded zero for repeated candidates;
4. the accepted descriptor; the inverse proven to be 1/direction and the
   per-call extent's form known;
5. its structures lie in tracked memory, and the root pointer's source is
   known;
6. the rule-3 choice above is made for its class, and the probe classes it
   depends on are recorded on this device and driver.

Computed from the program's structure or read from a file a validation run
writes on the user's machine. This design adds nothing keyed to a title
shader's hash or name; identifiers in source are the owner's open decision
(`audit-2026-11-26.md` A-B8; one is in `emit/synthetic_tests.rs:523`).

**Fallbacks.** Nothing is dropped that is not dropped today.

| Condition | What runs |
|---|---|
| Tier below 1.1, or switch `software` | the software pipeline, every program; one line at device creation |
| Program not eligible | the software pipeline; said once with the reason |
| Hardware variant not made yet | the software variant. Hardware variants queue behind every software pipeline and never delay one; a dispatch whose only pipeline is still being made is dropped today (`gpu/pipeline.rs:258-267`; `gpu/draw.rs:1103-1111`) |
| Mirror absent, pending, stale, over budget, or writes withheld | the software loop inside the hardware variant, chosen from the table; counted per frame |
| No software pipeline (wave size, untranslated, refused descriptor) | today's dropped dispatch, unchanged and still counted. Not eligible: a host loop there could not be compared with anything |

Never: a null structure (it reads as a miss for every ray), a shorter ray,
a skipped dispatch, an estimate in place of traced light.

**Host structures.**

| Matter | Design | Evidence |
|---|---|---|
| Primitive | One per guest triangle pointer (node address plus kind 0-3), found by walking box nodes from the root: a pointer walk, not a linear pass, because which triangles a node holds is known only from its parents. The box bounds exactly the three vertices that kind names. A side table gives node address and triangle number per (instance, primitive). A NaN vertex gives a box with MinX = NaN (inactive); never a partly NaN or inverted box. The walk needs header and instance layouts, which are a fork's report | `emit/lower/bvh.rs:19-21, 33-36`; `spec:2175-2176, 2200-2202, 2225-2227` |
| One submission behind | A build's box count, prebuild sizes, instance count and instance descriptions with host bottom-level addresses must be on the CPU when the build list is recorded. They come from memory the title's builder wrote on the GPU: readable after that submission's copy-back, never while writes are withheld. Counts are checked against the structure's own size and its copy's length (a corrupt size sent a fork's copy through gigabytes). The build list is recorded after every list's `run_deferred` and placed first; the box buffer is in the non-pixel shader-resource state; a UAV barrier before any trace | `spec:4153, 4342-4350, 4404, 2358-2370`; `gpu/host.rs:1423-1468, 1551-1600`; `avatarmanz:docs/fork-ray-tracing.md:119-122` |
| Stale | Each mirror has its own change tracker (a copy's is emptied when the copy takes its pages). Writers and signals: CPU writes (the tracker); shader writes through views and title memory (marks and declared ranges, known only after the fence); the title's buffer copies and CPU-side list steps; retakes after withholding, which mark only buffer-copy trackers today. Untracked memory has no signal: not eligible. A free and reallocation is not seen, so a mirror is retired by non-use for N frames. The unit is 64 KiB: one structure rewritten each frame makes every mirror sharing its page stale | `gpu/watch.rs:4-5, 14-17, 33-35, 41-44, 846-866`; `gpu/draw.rs:661-688`; `gpu/host.rs:1087-1105, 1570-1575` |
| Refit or rebuild | A refit may change boxes only, not counts, and no primitive may cross NaN; a top level is rebuilt or updated when a bottom level it names changes. Which build flags pay is a measurement | `spec:2019-2021, 2185-2189, 2286-2300` |
| Budget | Submissions are waited for without limit, and one that runs about 2 s ends the run. Builds per submission are bounded by count and estimated time from the probe; the rest wait in a queue and their dispatches trace in software. A build returns nothing, so "build failed" is not observable: only a failed allocation or inputs the pre-check refuses | `gpu/host.rs:1500, 1533-1549`; `spec:4054` |

**Memory.** Structures are sub-allocated at 256-byte alignment from a few
buffers in the acceleration-structure state (`spec:2326-2341, 4107`).
Scratch is one range per build in a batch or one build per barrier: the
text read does not say builds may share it (`spec:4110`); both are
measured. Compaction (`spec:4212`) is a later option for static mirrors,
its transient double footprint counted. In system memory: a second DXIL
per eligible program in a map that only inserts, pipeline-library growth,
instance staging, side tables, mirror records ([memory.md](memory.md), C1
to C3). Mirrors raise the adapter's reported use, and host textures are
given up from 90 % of budget (`gpu/resource.rs:1545-1547, 1588-1601`). So
mirrors are counted by their own owner and given up before textures at the
same thresholds, least recently traced first, never while a recorded or
submitted list names them, with a minimum residence time. That needs a
written "evict" case in memory.md and residency tracking that does not
exist (K8). The guest structures stay: **a mirror adds video memory,
system memory and build time to what the software path costs.**

**Synchronisation.** Host submissions are composed by the runtime from
consecutive runnable segments of one queue (`gpu/hold.rs:8-15`;
`gpu/queue.rs:207-213`), so the unit to count is the host piece and the
host list, not the title's submission.

1. Any earlier list in a piece that declares writes or uses title memory
   makes the table say "not current". A piece boundary can be placed by
   the pump; a builder and a tracer in one host list need a cut at record
   time.
2. The title's builder always runs: the title reads sizes back and every
   software loop reads the guest structure.
3. While `withholding()` holds or withheld ranges await a retake
   (`gpu/memory.rs:302-325`), every entry is "not current" and no mirror
   is built.
4. A mirror, its scratch and box buffer outlive the last submission that
   names them.

## Decision

Inside the owner's decision, an order. Each step is useful without the
next; none weakens an effect. Worktrees for several are already open
(`git worktree list`: `wip/p-*`, `wip/rt-*`); their contents were not read
for this record.

1. **Wave sizes** (any host for the tally, `gpu-info` for the card): the
   wave size and lane dependence of each tracer and builder stage. Decides
   whether G2 precedes the rest on the target.
2. **C3 groundwork** (code on any host; evidence from one title run):
   keep and log a structure view's address; log `DispatchX` flags per
   pipeline; log pipelines refused by declared wave size. With the
   existing warnings (`gpu/device.rs:288-289`) one run names the cause.
3. **The synthetic probe** (Windows with a device, no title file):
   `RayQuery` in the emitter, a host build helper, the CPU reference at
   tier 1. Buildable now; what it can and cannot end is in
   dxr-validation.md.
4. **A census of trace loops** (title files; counts only leave the
   machine): the unknowns listed in ray-tracing-research.md.
5. **GPU time by kind** on the target once C3 works: a classification
   (builder, tracer, other) on top of the existing per-command timing
   (`gpu/host.rs:425-461`).
6. **Recogniser, mirror manager, table and two-loop variant**, for the
   loop classes steps 3 to 5 leave eligible. Their design is fixed against
   the census, not before it.

Not built: DXR pipelines; anything from tier 1.2 or a vendor extension
(rule 4); a stand-in for traced light.

| Evidence | Change |
|---|---|
| Tracers declare 64-lane waves and the target runs 32 | G2 first; no host path is validated before it |
| Probe: host boxes lose a hit on the target, or repeat a candidate with the flag set | C and F stop on that driver; software only, and effort goes to its cost |
| Probe: host boxes do not pay even with build time left out | the same |
| Probe: host boxes pay only without builds | wait for step 5's figures and the structure counts of step 2 |
| Census: no loop is order-independent, or all are wave-wide walks with popped masks | no recogniser; software only, or A under an owner-accepted tolerance |
| Census: a few shapes, yes-or-no shadow loops among them | recognise those first; they need no tie rule and no commit |
| C3 fixed and tracers a small share of GPU time on the target | nothing to gain; the backend stays an experiment |
| The builder's exports are called with full geometry | D takes its input there instead of decoding nodes |

Candidate levers for the software path, each to be measured, none yet:
both node tests computed per call, loads through the page table, host
copies in system memory (an existing diagnostics switch puts them on the
card, `gpu/host.rs:3966-3971`; making that a default needs its own
memory.md case).
