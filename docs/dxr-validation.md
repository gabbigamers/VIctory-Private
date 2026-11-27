# Validating the ray-tracing compatibility layer

How rendering equivalence and cost are tested and measured, on synthetic
programs first and with no file of any title. Companion to
[dxr-architecture.md](dxr-architecture.md) (steps 3 and 4 of its order)
and [dxr-compatibility.md](dxr-compatibility.md), which holds the rule
this procedure applies.

**None of this has been run on hardware.** A first version of the probe
exists (`victory rt-probe`; "The first version" below says what it runs
and where it departs from this specification): implemented; its logic
tested (host tests); its three shaders accepted by Microsoft's validator on
Linux; its Direct3D part type-checked for Windows only. It is behind the
switch a run's native and hybrid ray tracing is behind: without
`VICTORY_RT_BACKEND=native`, `hybrid` or `auto` (`hardware` is the older
name of `native`) it opens no device. The rule, its
tolerance and its thresholds are a proposal written on 2026-11-27, before
any measurement, and await the owner's acceptance; after that a change is
a dated revision. `emit/` is
`crates/victory-shader/src/emit/`, `gpu/` is `crates/victory-gpu/src/`,
`ref:` is the reference-shader directory named in
[ray-tracing-research.md](ray-tracing-research.md).

## The four means

| Means | What it is here | What it can show | Where |
|---|---|---|---|
| **Synthetic geometry** | scenes and rays generated from one seed; guest programs written as GFX10 words, one per workload class; a CPU reference | agreement of each path with the reference under the rule, on rays built to hit every difference | any host for the reference; a device for the paths |
| **Reference shaders, written independently** | for each workload class an HLSL shader written from the class's description alone, never from the emitter's output, compiled by Microsoft's compiler. Three exist (`ref:rq_tri.hlsl`, `rq_proc.hlsl`, and a heap-indexed form) | the DXIL the emitter must match; on a device, whether a difference is the emitter's or the host's | any host to compile; a device to run |
| **Microsoft's validator** | `scripts/validate-dxil.sh`: `dxv` over every container the host tests of `victory-shader` finish (`progress.md`, tier 1b) | each variant is well-formed DXIL by that validator. Nothing about running it, or about a driver's compiler | any host with the tool |
| **Measured GPU benchmarks** | a begin and an end timestamp around each dispatch and each build on the owner's card | time and memory of each path, kept apart as below | the owner's machine |

## What must be built first

Still absent from the tree (the first version below runs without them,
and is the less for it):

* The smallest version of the layer (dxr-architecture.md): region
  analysis, the workload description, a recogniser and a native generator
  for the synthetic programs. `RayQuery` is in the emitter
  (`emit/rayquery.rs`), with three prototype shaders that no translated
  program reaches.
* **R-ref**, the geometry in double precision with the classes of the
  rule, and the host test that computes the tolerance constant k.
  **R-bits**, the emulation's model in single precision, exists outside
  the Windows-only test module as `crates/victory-rt` (`node.rs`), beside
  an exact reference with a tolerance of its own that is not the rule's
  ([dxr-validation-reference.md](dxr-validation-reference.md)).
* A scene generator that knows each triangle's surface, material and
  neighbours (`victory-rt`'s knows bottom level and instance); the scene
  classes of "Scenes". A builder of guest-format trees
  (`victory-rt`, `console.rs`; its top level is our own) and a host build
  helper (`gpu/rt/`: sizes, builds, barriers, timestamp pairs;
  type-checked for Windows only) exist.
* `victory gpu-info` printing the tier as a number, and "unknown" when
  the query fails (`gpu/caps.rs:177-181, 200-228`).

## The first version (`victory rt-probe`)

Step 4 of the order of work (dxr-architecture.md), built from what is in
the tree ahead of step 3: no region is recognised and no variant is
generated yet. `crates/victory-rt-probe` holds it; `apps/victory` has the
command, `xtask` the summary. State: implemented; everything that needs
no device tested (host tests: the scene as build inputs, the mapping of
rays, record decoding with refusals, the statistics, the result files,
the run itself over a brute-force stand-in on the CPU, the summary and
its comparison of two files); the three containers accepted by
Microsoft's validator on Linux; the Direct3D part
(`crates/victory-rt-probe/src/hardware.rs` over `gpu/rt/device.rs`)
type-checked for Windows only. **Nothing has run on a device, and no
figure of any kind exists.**

Where it departs from the rest of this document, and why:

| Specified | First version | Why |
|---|---|---|
| Synthetic guest programs through the real translator, one per workload class; the paths N, N-d, N2, NR, H-node, H-desc, S | The three prototype shaders of `emit/rayquery.rs`, written with the emitter: `native` (the triangles shader over host triangles), `hybrid` (the procedural shader over one host box a triangle, the vertices beside), `null` (writes misses, traces nothing). `software-gpu` is a path the file names with status `not built` | nothing recognises a region or generates a variant, and no shader walks console-format nodes outside a translated title loop. These rows show a host's structures, traversal and triangle test, and the hybrid's shader-side test: not a translated program |
| A production and an instrumented container per variant | one container a path: the record per ray is the shader's only output, so the container that is timed is the container that is compared; `container_hash` is its header digest | the prototype shaders store nothing else |
| S on the device, equal to R-bits bit for bit, as the reference there | the CPU model of the software traversal (`victory-rt`) beside each path's answers (`model_*` columns) | S is not built. The model's top level and the node fields the test does not read are our own |
| R-ref in double precision, the rule's classes, k | `victory-rt`'s exact reference with its own tolerance and verdicts. **Every verdict is "under the crate's policy, not yet the rule"**; the file's header says so (`policy`, `k`) | the crate and the rule are not reconciled (dxr-validation-reference.md, "Against the proposed rule"). The probe's use of the crate is one module (`adapter.rs`), where the reconciliation will be met |
| The scene classes of "Scenes": to 10^6 triangles, seams, `far`, the cost classes | `victory-rt`'s ten classes: 28 to 405 triangles, 1 to 48 instances, 1,200 to 2,000 rays each; `dynamic-refit` and `dynamic-rebuild` have four frames | those are the scenes there are. **Scenes this small say nothing of cost**: their times exercise the instrument. No `scale`, `seam`, `far`, `refit`, `many-small` or `burst` class exists |
| Every class's rays through the variant in the test-only interpreter first | each ray is screened on the CPU (`rayquery::ray_refusal`) before a list is made: a ray it names is in no list a device reads, and is counted with its reason (`refused_before`). `SKIP_CLOSEST_HIT_SHADER` is cleared and the mask cut to eight bits there, counted | no translated variant exists to interpret. The rule the screen serves is kept: no illegal ray reaches a device |
| Levels 2 and 3; N against NR; each constant halved and doubled | not built | no program stores anything after a region; the reference HLSL (`crates/victory-shader/tests/dxc-reference/`) is compiled on Linux and run by nothing |
| `rt-translate.csv`, `rt-record` | not written. Written instead: each path's hit records in `victory-rt`'s result format | no translator runs in the probe. The validation record has a format now (`gpu/rt/record.rs`) and the probe writes no row of it: a probe row carries the number of the rule its verdict was given under, and these verdicts are under the crate's policy, not the rule |
| `rays_per_s`; `video_local_mb_delta`, `working_set_mb_delta`, `commit_mb_delta`; medians of build times | not written. The count and the time, and the figure before and the figure after, stand in columns of their own; a frame's builds are measured once and summed by kind | no column is one figure divided by, or taken from, another |
| `--out rt-probe.csv` in the working directory | `--out` names a file outside the repository, or the command refuses | the repository's asset policy lists what it may hold, and a result file is not on the list |
| The timed dispatches alone | after each series of timed dispatches one more dispatch, of the same records over poisoned hit records, in no time of the file: the **check** ("A run" below) | a timed submission holds the dispatch and nothing else, so its records cannot be poisoned first, and what the hit buffer holds after it may be an earlier dispatch's. Without the check nothing shows that a dispatch of `timed_rays` records writes every record |

**Refusals.** A refusal is the record the shaders write for one: a kind
with bit 31 (`rayquery::HIT_REFUSED`) and one or both of the two reasons
in the low bits, the other seven words 0. It is counted by reason
(`refused_geometry`, `refused_range`) and its ray is **not judged for
that path, never read as a miss**. Every listed ray is traced by both
paths, so a ray the native path refuses has the hybrid's record beside it
(`refused_taken_by_next` where that is a hit or a miss); a ray no built
path took is counted (`refused_unanswered`): the hybrid's next path is
the software traversal on the GPU, which is not built. A verdict speaks
of the rays judged and says nothing of the refused ones; the counts stand
in the columns before it.

**Records that are neither answer nor refusal.** Before each dispatch
whose records are read, the hit records are overwritten with a poison
word, so that a record no thread wrote is neither a stale hit nor a miss.
The poison has bit 31 and is not what a shader writes for a refusal; no
other record with bit 31 is either (no reason, a reason bit the probe
does not know, other words that are not 0). **None of these is taken out
of the comparison**: each is counted (`unwritten`, `refused_malformed`)
and judged with the answers, where it is a record that is no hit and no
miss, a defect (`malformed`). A row whose dispatch left records unwritten
therefore cannot read `pass` on the records that were written.

**A run**, per scene class, with the class's rays as one list in upload
memory:

1. The null shader: one dispatch over the listed rays, the timed
   dispatches, then the check; every record read must be a miss
   (`null_not_miss`).
2. Each tracing path, native then hybrid, one at a time so that the
   memory counted is one path's. Per frame: the scene is turned into
   build inputs on the CPU; bottom levels and the top level are built,
   refitted or kept as the frame asks, each structure between timestamps
   of its own, in one submission that is waited for; one dispatch lists
   every ray once and its records are read back (the answers); then the
   timed dispatches, each a submission of its own that holds a timestamp,
   the dispatch and a timestamp and nothing else, list `timed_rays`
   records (the class's rays, begun again as often as it takes); then the
   check. With `--loop`, the loop mode follows, with a check of its own.
   Then the path's structures are given up.
3. Each path's answers are compared, frame by frame, with the CPU
   reference and the CPU model through `victory-rt`'s comparison.

**The check.** One more dispatch after the timed ones: the same shader
and structures and the `timed_rays` records a timed one lists, over
poisoned hit records, in no time of the file. Its records are read back
and held against the answers (`repeat_differing`; a record it left
unwritten counts). It shows that a dispatch of that many records writes
every record and gives the same answers. It does not show that each timed
dispatch did: those differ from it by the copy that poisons, and their
own records are not read. The loop's check is one more dispatch over the
listed rays after the last iteration (`loop_last_differing`).

A step that fails on the device fails its row, with what failed in
`note`; the row keeps the columns of the steps before it (its verdict
among them, where the answers had come back, and its records in the hit
file), has none of the failed step or of those after it, and holds in its
memory columns what was held when it failed; the path's
later frames are `not run`; the run goes on with the next path and class.
Nothing is retried and nothing a device did not do is filled in.

**What runs nothing.** Each of these prints `rt-probe: NOT RUN:` with the
reason, writes no file and ends the command with an error:
`VICTORY_RT_BACKEND` asking for no path over host structures (unset is
software, as in a run; `native`, `hybrid`, `auto` and `hardware`, the
older name of `native`, open it, read through the selection's parser in
`gpu/rt/select.rs`, and with none of them not even the output folder is
made); a place a result file cannot
be written to, found before the device is opened; a device below ray
tracing tier 1.1 (or without shader model 6.6, or `ID3D12Device5`), with
what it reports and what is missing; `--debug-layer` where the layer is
not installed.

**The switch's value limits the run to no path.** Whichever of the four
values opens the backend, the probe measures the native and the hybrid
path both, and the header's `rt_backend_switch` line says so. A forced
path (`native`, `hybrid`) is a run's matter, one workload at a time; the
probe is a comparison, and its rows depend on each other: a ray the
native path refuses is judged by the hybrid's record for it
(`refused_taken_by_next`, "Refusals" above), and both paths' times stand
against one null dispatch of one run. Under `auto` it selects nothing
either: it measures. `VICTORY_RT_CLASS` is not read. A probe that ran one
path's rows alone would be a different run with different counts; whether
that is wanted is the owner's to say.

**Memory.** The probe's allocations are bounded, owned and counted by the
backend's three owners; the case is R5 of [memory.md](memory.md).

### The columns of `rt-probe.csv`

One row per scene class, frame and path; a null-shader row per class; a
loop row per class and path with `--loop`. A column that does not apply,
or that could not be measured, is empty: never 0. `crates/victory-rt-probe/src/row.rs`
holds the same list, and a host test keeps the two the same: the names,
their order and each definition word for word.

| Column | What it holds |
|---|---|
| `class` | the scene class, by victory-rt's name |
| `seed` | the seed the class's scene and rays were generated from, hex |
| `frame` | the frame of the class's case, from 0; empty in the null shader's row, which reads no scene; `loop` in a loop row |
| `path` | `native` (host triangles, the triangles shader), `hybrid` (one host box a triangle, the procedural shader), `null` (the null shader), `software-gpu` (not built) |
| `status` | `ran`; `failed` (a step on the device failed and `note` says which: the columns of that step and of the steps after it are empty, those of the steps before it are kept, the verdict among them where the answers had come back, and the memory columns hold what was held when it failed); `not built` (the path has no shader here); `not run` (the class has no ray to list, or an earlier frame of the path failed: `note` says which) |
| `note` | what failed, where the backend or the probe did other than the frame asked, and what a validation layer said, if one was on |
| `container_hash` | the digest in the header of the DXIL container the row's pipeline was made from, 32 hex digits |
| `bottoms` | bottom-level structures of the frame's scene |
| `instances` | instances in its top level |
| `primitives` | triangles over its bottom levels, each counted once |
| `flat_boxes` | hybrid: boxes with no extent on at least one axis (a triangle in an axis plane) |
| `rays` | rays of the class |
| `rays_listed` | ray records in the list a dispatch reads: `rays` less `refused_before` |
| `timed_rays` | ray records one timed dispatch lists: the list's rays, begun again as often as it takes |
| `skip_closest_hit_cleared` | rays whose SKIP_CLOSEST_HIT_SHADER flag was cleared on the CPU (an inline query runs no such shader) |
| `mask_high_bits_cut` | rays whose instance mask had bits above the low eight, which were cut on the CPU |
| `refused_before` | rays the shaders cannot take as DXR defines them, refused on the CPU and in no list |
| `refused_before_reasons` | those by reason, `reason: count` with `;` between |
| `refused_geometry` | records the path's shader wrote as refused (kind bit 31) with reason 1: geometry of the other type |
| `refused_range` | the same with reason 2: the procedural shader's test left single precision |
| `unwritten` | records of the dispatch that listed each ray once that still hold the poison written before it: no thread wrote them. No refusal: each is judged, as a record that is no hit and no miss |
| `refused_malformed` | other records with kind bit 31 that are no refusal a shader writes (no reason, a reason bit this probe does not know, or other words that are not zero). No refusal: each is judged, as a record that is no hit and no miss |
| `refused_taken_by_next` | rays this path refused, before or in the dispatch, that the next path gave a hit or a miss (native: the hybrid) |
| `refused_unanswered` | rays this path refused that no built path took (the hybrid's next path, software on the GPU, is not built) |
| `verdict` | victory-rt's verdict on the rays judged: `pass`, `fallback`, `fail`, `baseline-fail`, `useless`. It says nothing of the rays the path refused, which are counted in the columns before this one. Under the crate's policy, not yet the rule |
| `judged` | rays judged: every ray the path has a record for that is no refusal, unwritten records among them |
| `clear` | of those, rays with one permitted result |
| `ambiguous` | rays with several |
| `excluded` | rays counted and not judged (an unstable candidate, or over 16 results) |
| `wide` | clear or ambiguous rays with a result whose tolerance is too wide to compare numbers by |
| `not_clear_reasons` | the rays that are not clear by the reference's reasons, `reason count` with `;` between |
| `matched` | clear rays answered rightly |
| `permitted_primary` | ambiguous rays answered as the reference itself answers |
| `permitted_other` | ambiguous rays answered with another permitted result |
| `ids_only` | right answers compared by ids and face alone (a wide result) |
| `beyond_band` | right answers whose error is over 1/8 of the tolerance: no defect, reported |
| `missed` | defects: a miss where a hit is required |
| `phantom` | defects: a hit where none is permitted |
| `wrong_triangle` | defects: a hit on a triangle that is not permitted |
| `wrong_face` | defects: the right triangle, the other face |
| `numeric` | defects: t, u or v outside the tolerance |
| `malformed` | defects: a record that is no hit and no miss (an unwritten record is one, unless its ray is excluded) |
| `worst_t_ratio` | the largest error of t among compared hits, as a share of its tolerance |
| `worst_uv_ratio` | the same for u and v |
| `faces_not_reported` | hybrid: hits judged with the face of the permitted result they name, since the procedural shader reports none |
| `model_defects` | defects of the CPU model of the software traversal against the same reference on the same rays; empty where the model refused the frame |
| `model_identical` | rays the path and the model answer rightly and word for word alike |
| `model_divergent` | rays both answer rightly with different results, first-hit rays apart |
| `model_divergent_first_hit` | the same on first-hit rays |
| `model_refusal` | why the CPU model has no answers for the frame |
| `repeat_differing` | hit records of the checking dispatch (one more dispatch after the timed ones, of the `timed_rays` records a timed one lists, over poisoned records and in no time of the row) that differ, word for word, from the dispatch that listed each ray once; a record it left unwritten differs |
| `null_not_miss` | null row: records that are not eight zero words, of its dispatch over the listed rays and of its checking dispatch; a record left unwritten is one |
| `trace_samples` | timed dispatches kept that have a GPU time |
| `trace_untimed` | timed dispatches kept that have none |
| `trace_gpu_ms_min` | GPU time between a timestamp before and one after the dispatch alone (`timed_rays` records, the production container, a queue doing nothing else, a submission that holds nothing else), over the kept repeats: the least |
| `trace_gpu_ms_p50` | the median |
| `trace_gpu_ms_p95` | the 95th percentile by nearest rank |
| `trace_gpu_ms_max` | the greatest |
| `null_gpu_ms_min` | tracing rows: the null shader's `trace_gpu_ms_min` for the same class and `timed_rays`, beside |
| `null_gpu_ms_p50` | its median |
| `null_gpu_ms_p95` | its 95th percentile |
| `null_gpu_ms_max` | its greatest |
| `builds_asked` | structures the frame asks to be built, bottom levels and the top level together |
| `refits_asked` | structures it asks to be refitted |
| `builds_done` | structures the backend built |
| `refits_done` | structures the backend refitted in place |
| `build_gpu_ms` | GPU time of the frame's bottom-level builds, each between timestamps of its own with its barrier, summed; empty where one has no time |
| `refit_gpu_ms` | the same for bottom-level refits |
| `tlas_build_gpu_ms` | the same for the top level where it was built |
| `tlas_refit_gpu_ms` | the same for the top level where it was refitted |
| `build_untimed` | build and refit regions without a time |
| `convert_cpu_ms` | processor time turning the frame's scene into the bytes a build reads (vertices, indices, boxes, triangle data, instances) |
| `upload_cpu_ms` | processor time making upload memory and copying those bytes in; null row: the same for the class's ray list |
| `record_cpu_ms` | processor time describing the builds to the backend and recording them, the device's size queries and making each new structure's memory among it |
| `pipeline_cpu_ms` | processor time creating the path's pipeline, the driver's compile among it; in the path's first row of a class only the first time |
| `input_bytes` | bytes uploaded for the frame's builds (a refit uploads positions or boxes alone); null row: the bytes of the class's ray list |
| `build_wait_ms` | time the submitting thread was blocked from the builds' submission until the card had run it |
| `trace_wait_ms_p50` | the same for a timed dispatch's submission, median over the kept repeats |
| `readback_wait_ms` | the same for the copy that brings the hit records back |
| `own_rt_structures_bytes` | the account gauge of structure memory (on the card) after the frame's builds and dispatches |
| `own_rt_scratch_bytes` | the gauge of build scratch and the hit buffer (on the card) then |
| `own_rt_buffers_bytes` | the gauge of uploads, instance descriptions and read-backs (system memory the card reads) then |
| `video_local_mb_before` | the adapter's figure for this process on the card before the path's first build of the class; cross-check only |
| `video_local_mb_after` | the same after the frame's builds and dispatches |
| `video_budget_mb` | the adapter's budget for this process on the card then |
| `video_shared_mb_before` | the adapter's figure in system memory the card reads, before |
| `video_shared_mb_after` | and after |
| `commit_mb_before` | the process's private commit before the path's first build of the class |
| `commit_mb_after` | and after the frame's builds and dispatches |
| `working_set_mb_before` | the process's working set before |
| `working_set_mb_after` | and after |
| `loop_iterations` | loop row: iterations run |
| `iter_ms_min` | loop row: wall time on the submitting thread of one iteration, from the start of its conversion through its builds' submission and wait to the end of the wait for its dispatch: the least |
| `iter_ms_p50` | the median |
| `iter_ms_p95` | the 95th percentile |
| `iter_ms_p99` | the 99th percentile |
| `iter_ms_max` | the greatest |
| `loop_last_differing` | loop row: hit records of the loop's checking dispatch (one more dispatch after the last iteration, over poisoned records and in no time of the row) that differ from those of the same frame's first dispatch |

## What is compared

One synthetic guest program per workload class, translated by the real
translator into its variants; the same scene and rays through each.
Each variant exists as two containers. The **production** container is
what `auto` would bind: it is the one timed, and it is validated at
levels 2 and 3 through what its own code stores. The **instrumented**
container adds a record per ray and counts of candidates; it serves
level 1 and is never timed. A driver's result may depend on code shape
(`spec:2053-2056` promises determinism for identical code only), so a
row is written only when the two containers' stored words are equal on
the same rays, and it carries the production container's hash.

| Path | What it is | Role |
|---|---|---|
| **N** | the **native** variant, dual form: the region as `RayQuery` over host triangles built from the scene's build description. No node pointer can be kept | the primary candidate |
| **N-d** | native over triangles built from decoded guest nodes, with the node-pointer side table | the only pairing in which a class that keeps a node pointer occurs on the title |
| **N2** | the both-loops form of N and of N-d, with its table read | its own container and its own rows: selectable only after them |
| **NR** | the reference shader of the class over the same host structure | N against NR separates the emitter from the host |
| **H** | the **hybrid** variant over decoded nodes (**H-node**) and over boxes from the build description (**H-desc**); each never committing, and committing the quotient rounded up (nearest-hit classes only) | the compatibility option |
| **S** | the **software** variant: the region lowered as today, each node tested by `emit/lower/bvh.rs` | today's path; the reference on a device |
| **R** | the CPU reference. R-bits answers twice, with the box tests (R-tree) and with every triangle tested (R-all, over node bytes or over input vertices to match the hybrid's source); R-ref classes each ray | the oracle for S, bit for bit, and for N and H under the rule. Not for the console: the emulation is unchecked against one |

The first programs: a yes-or-no loop; a nearest-hit loop that keeps the
distance pair, barycentrics, facing and the instance id; the same
keeping a node pointer (N-d and H-node only); the same with a candidate
filter (the form of `ref:rq_tri.hlsl`); one with face culling; one with
instance masks; one whose order is tie by key (N must refuse it). Each
has code after its region that stores what level 2 compares. Node data
for S and H is read as the runtime reads it, through the page-table path
(`gpu/host.rs:3955-3965`; `emit/lower/bvh.rs:259-288`); timings are
repeated with the buffer on the card, as separate rows.

Limits. The programs are the probe's own: a class the census finds in
the title and the probe has no program for is not validated. Instances
are the probe's table and claim nothing about the guest's layout.
Results hold for the device and driver they were taken on.

## Scenes

Generated from one 64-bit seed (the generator of the existing tests,
`emit/bvh_tests.rs:427`). Each class runs with 32-bit and with 16-bit box
bounds in the guest tree. "Random" classes are those the bound on
ambiguous rays applies to; "directed" classes aim at a difference.
Scene parameters are fixed from R-ref alone, before a device runs.

| Class | Kind | Scene and rays | Probes |
|---|---|---|---|
| `single`, `scale` | random | one triangle; 10^3 to 10^6 triangles, 2^20 rays, coherent and incoherent | basic agreement; cost |
| `closed` | random | closed meshes of several surfaces and materials, rays from inside and outside | no ray from inside escapes; the pictures of level 3 |
| `instances` | random | 1 to 4,096 instances: rotation, uneven scale, mirroring, large translation, overlap | the object-space ray; facing under a mirror; instance identity |
| `far` | random | triangles with edges of 2^-4 to 2^4 units at 2^12, 2^14 and 2^16 from the origin, until the boundary log (architecture, step 2) gives the title's magnitudes and sizes | the tolerance where the title's geometry lies; sets the largest magnitude a validation covers |
| `seam` | random and directed | closed surfaces split across bottom levels, across instances with the same and with different transforms, and across geometries with a geometry transform; random rays, and rays exactly on the seams and 1, 2, 8 spacings either side | whether the device closes seams the specification does not promise (`spec:2404-2409`) |
| `surface-rays` | random | rays that leave scene surfaces with a bias and rays whose extent ends on one; biases of 0 and 1, 16 and 256 tolerances until the census gives the title's | the share of extent-boundary rays; hits at t = 0 |
| `alpha` | random | non-opaque geometry with a filter on barycentrics and a per-triangle bit; opaque beside it | the filter as candidate body; decisions at a filter threshold; repeats; side-table reads |
| `masks`, `cull` | random | instance masks 0x01 to 0xFF and zero; front, back and no culling | mask and flag mapping |
| `edge` | directed | two triangles sharing an edge, a fan, a closed mesh; rays exactly on edges and vertices and 1, 2, 8 spacings either side | admissibility at shared, boundary and silhouette edges |
| `thin`, `large-thin` | directed | slivers and needles to aspect 2^20; small triangles at 2^16, 2^20, 2^24 from the origin | admissibility where every pair is a sliver pair. Reported, never a pass alone |
| `cull-edge` | directed | an edge shared by a front and a back face, under each cull flag | which triangle a driver reports; the specification is silent |
| `extent` | directed | hits at exactly t = 0, at the extent, one spacing either side; extents of +INF, -0.0 and the largest finite float | the next-float-up bound and its special cases |
| `tie` | directed | coplanar duplicates of one material and of two; hits a few spacings apart | which wins; stability between runs. Reported, never a pass alone |
| `far-graze`, `flat` | directed | the `edge` scenes far from the origin; triangles in axis planes | guest hits accepted through rounding that a host may not present (H); gained hits (N) |
| `degenerate` | directed | zero-area and collinear triangles, a NaN vertex; NaN in the extent, origin, direction, one at a time; a zero and an infinite direction, an infinite origin; a negative extent | the screens; inactive triangles keep indices aligned |
| `odd-id` | directed | triangle nodes whose id byte is an odd permutation | the facing disagreement between the two emulations; R-bits alone shows it at tier 1 |
| `inverse` | directed | inverses that are not 1/direction | S and R keep them; N and H must refuse the program |
| `refit`, `many-small`, `burst` | cost | moving vertices over 120 steps; 64 to 4,096 small bottom levels rebuilt each iteration; N builds in one submission | rebuild against update; the regime a fork reports; build time against the 250 ms warning and 2 s limit (`gpu/host.rs:1533-1549`) |

A workload class **depends on** `single`, `scale`, `closed`, `far` and
`surface-rays` always; on `instances` and `seam` when it traces a top
level; on `alpha` with a filter, `masks` with a mask, `cull` with
culling. **No illegal ray reaches a device**: every class's rays go
through the variant in the test-only interpreter first, and a class in
which a ray arrives at the trace call illegal is not run. Undefined
behaviour that happens to work on a driver would validate nothing.

## Comparing

Per ray the instrumented container writes one record: hit flag,
instance, triangle (in the scene's own numbering), facing, t as the
program's code formed it, two barycentrics, the ray as given to the
host; then the words the program stored after its region. H also writes
how often each (ray, primitive) pair was presented, N the same for
non-opaque candidates. R-ref writes per ray: its class (unambiguous, or
which kind of ambiguous), whether a sliver or grazing pair is involved,
the admissible answers, and the tolerances.

1. **Level 1.** Each record is placed against R-ref: `agree`,
   `admissible`, or one of the failures `lost_hits`, `gained_hits`,
   `id_flips`, `surface_flips`, `facing_flips`, `filter_flips`,
   `out_of_tol`.
2. **Level 2.** The production container's stored words against those
   R-ref's answer gives through the same code: integers equal or
   admissible, floats within the carried tolerance.
3. **Level 3.** For `closed`, `instances`, `far`, `seam`, `scale`,
   `alpha` and `cull` those words are drawn by the fixed shading
   function and compared with R-ref's picture.

### Rules (proposed with the rule; applied only once accepted)

| Pair | Rule |
|---|---|
| The validator over every variant's container | accepted, or nothing of that variant is run |
| S against R-bits (R-tree) | **equal bit for bit**, every class, every ray. A difference is a defect of the emulation on that driver, to be fixed before anything is compared with S |
| N against NR, H against its reference shader; production against instrumented | equal in every field. A difference means the emitted code differs in meaning from the reference or the driver's result depends on code shape; the path is not validated for the class until it is explained |
| N against R-ref, random classes | **pass** when: no failure of level 1; among rays with no sliver or grazing pair, ambiguous rays at most 1 in 1,024 by R-ref's own count, all kinds together; rays with a sliver or grazing pair at most 1 in 16; level 2 holds on every ray; level 3 holds on every picture |
| H against R-ref, random classes | as N on unambiguous rays; on rays ambiguous by edge, seam, extent or tie, equal bit for bit to R-all on the same vertices, and outside the bound. H against S is counted (`equal_to_s`), not required |
| N or H against R-ref, directed classes | every answer admissible. `tie`, `cull-edge`, `thin` and `large-thin` are reported: they say what the driver does, not that it is right |
| A lost or gained hit on an unambiguous ray; a miss at an edge shared by like-facing triangles or at a seam with one object-space ray; a filter decision that differs on a candidate not at a threshold | **fail**, always |
| A candidate presented more than once | counted (`dup_candidates`) and reported. A zero licenses nothing: repeats depend on how a driver boxed that geometry |
| The same seed twice, and again after the device is re-created | records identical; otherwise fail |
| `inverse`, and any class the recogniser must refuse | the variant does not exist; the dispatch ran S and was counted |
| Each constant of the rule halved and doubled | the verdict is recomputed; one that changes is flagged `sensitive` |

A workload class has **passed the probe** for a path, form, structure
source and side-table kind on this device and driver when its program
passes every random class it depends on, is admissible on every directed
class, and has identical repeat runs. The pass reaches only to the
largest coordinate magnitude its random classes covered (`m_max`). On a
failure the row says which level failed, and regions of that class run
the next path. The probe is one of three gates for `auto` (last section).

## Measurements

Five quantities, each in its own columns, never summed with another. The
third column is the specification's; the fourth is what the first version
writes, defined column by column in "The columns of `rt-probe.csv`"
above. Where the two differ, the file has the fourth.

| Measurement | How | Columns specified | First version |
|---|---|---|---|
| **1. GPU time of traversal dispatches** | a begin and an end timestamp around the trace dispatch of the **production** container alone, on a queue doing nothing else; 3 runs discarded, 30 kept | `trace_gpu_ms_min`, `trace_gpu_ms_p50`, `trace_gpu_ms_p95`, `trace_gpu_ms_max`, `rays_per_s`, `container_hash` | `trace_gpu_ms_min`, `trace_gpu_ms_p50`, `trace_gpu_ms_p95`, `trace_gpu_ms_max` and `container_hash`, with `trace_samples`, `trace_untimed`, `timed_rays`, and the null shader's four figures for the same class beside them (`null_gpu_ms_*`); the two are never subtracted. `--discard` and `--repeat` set the 3 and the 30. One timed dispatch lists `timed_rays` ray records (`--rays`, 2^20 unless asked): the class's rays, begun again as often as it takes. No `rays_per_s`: the count and the time stand in columns of their own |
| **2. CPU translation overhead** | timers in the translator and the recorder, per program and in total | `recognise_ms`, `variant_ms` (per path and form), `driver_compile_ms`, `convert_ms_p50` and `convert_ms_p95` (build descriptions, per iteration), `bind_extra_us_p50`; totals `recognise_ms_total`, `variant_ms_total` | the processor's side of the host builds only, per frame: `convert_cpu_ms` (the scene into the bytes a build reads), `upload_cpu_ms` (in the null row: of the class's ray list), `record_cpu_ms` (describing and recording the builds, the device's size queries and making each new structure's memory among it), `pipeline_cpu_ms` (pipeline creation, the driver's compile among it; once a shader). Nothing of recognition or variants: no translator runs |
| **3. Video memory** | **bytes counted by owner, from the sizes allocated**, per path: structures, scratch, side tables, boxes. Cross-check only: the adapter's local-segment usage in a fresh device, read after the scene's buffers exist and again after one path's objects are built (it is process-wide, so nothing else may be alive) | `as_bytes`, `scratch_bytes`, `side_table_bytes`, `box_bytes`; `video_local_mb_delta` | the account gauges of the backend's three owners after each frame (`gpu/account.rs`): `own_rt_structures_bytes` (structures, on the card), `own_rt_scratch_bytes` (build scratch and the hit buffer, on the card), `own_rt_buffers_bytes` (uploads, the hybrid's boxes and triangle data among them, instance descriptions, read-backs: **system memory the card reads, not to be added to the other two**). Beside them the adapter's figure, not in a fresh device and not as a difference: read before a path's first build of a class and after each frame (`video_local_mb_before`, `video_local_mb_after`, `video_budget_mb`, `video_shared_mb_before`, `video_shared_mb_after`), with the class's ray list and hit buffer alive at both. No `video_local_mb_delta`. No side tables exist |
| **4. System memory** | bytes counted by owner: the variants, the library's growth, kept descriptions, and the guest tree where it lies in system memory (the default rows; on the card for the on-card rows, then under 3). On the title also the host copies, windows and snapshots the layer causes. Cross-check only: working set and private commit of the probe, read at the same two events (the generator and reference dominate them) | `variant_dxil_bytes`, `library_bytes_delta`, `registry_bytes`, `guest_bytes`, `guest_segment`; title: `own_buffer_copies_mb`, `own_windows_mb`, `own_snapshots_mb`, `video_shared_mb`; `working_set_mb_delta`, `commit_mb_delta` | the process's counters only, before a path's first build of a class and after each frame: `commit_mb_before`, `commit_mb_after`, `working_set_mb_before`, `working_set_mb_after`; no `*_delta` column. The CPU reference's and the generator's own memory is in them. No variant, library, registry or guest tree exists to count |
| **5. Rendering correctness** | the three levels above | `k`, `m_max`; level 1: `rays`, `unambiguous`, `sliver_rays`, `amb_edge`, `amb_seam`, `amb_extent`, `amb_tie`, `amb_filter`, `amb_degenerate`, `agree`, `admissible`, `lost_hits`, `gained_hits`, `id_flips`, `surface_flips`, `facing_flips`, `filter_flips`, `out_of_tol`, `t_err_max`, `bary_err_max` (in tolerances and in spacings); level 2: `consumed_equal`, `consumed_within`, `consumed_differ`; level 3: `px_differ`, `px_share`, `px_group_max`; and `dup_candidates`, `screened_nonfinite`, `repeat_equal`, `equal_to_s`, `sensitive` | level 1 only, in `victory-rt`'s own counts and names, under that crate's policy: `verdict`, `judged`, `clear`, `ambiguous`, `excluded`, `wide`, `not_clear_reasons`, `matched`, `permitted_primary`, `permitted_other`, `ids_only`, `beyond_band`, the six kinds of defect, `worst_t_ratio`, `worst_uv_ratio`; the refusal counts, `unwritten` and `refused_malformed`; `model_*` for the CPU model of the software traversal; `repeat_differing` (the check) |

Reported apart from all five, and never inside trace time:

| Measurement | How | Columns specified | First version |
|---|---|---|---|
| Build and refit | a timestamp pair of its own around each bottom-level build, update, top-level build and barrier, and around the pointer walk H-node needs; scratch as one range per build and as one build per barrier | `build_ms_p50`, `refit_ms_p50`, `tlas_ms_p50`, `derive_ms_p50`, `scratch_mode` | per frame, each kind summed over the frame's structures and empty where one of them has no time: `build_gpu_ms`, `refit_gpu_ms`, `tlas_build_gpu_ms`, `tlas_refit_gpu_ms`, `build_untimed`; what was asked and what the backend did (`builds_asked`, `refits_asked`, `builds_done`, `refits_done`). A structure's region holds its build and the barriers the backend puts around it; a barrier has no pair of its own. A frame is built once, so there is no median. Scratch is the backend's pool: there is no choice of mode and no `scratch_mode`. No pointer walk exists |
| Synchronisation waits | from submission to fence for a build alone, and for a trace with and without a build ahead of it; time on the submitting thread before the wait (`gpu/host.rs:1531-1532`) | `submit_wait_ms_p50`, `build_then_trace_ms_p50`, `cpu_prep_ms_p50` | `build_wait_ms`, `trace_wait_ms_p50`, `readback_wait_ms`: time the submitting thread is blocked from a submission to its fence, for the builds, for a timed dispatch, for the copy that brings records back. The time before the wait is measurement 2's `record_cpu_ms`. Builds and traces are never in one submission, so there is no trace with a build ahead of it and no `build_then_trace_ms_p50` |
| Loop mode | 600 iterations of what a path adds per frame. `iter_ms` is wall time on the submitting thread, from the start of an iteration's conversion to the fence of its last submission, the wait included; S's loop is its trace alone | `iter_ms_min`, `iter_ms_p50`, `iter_ms_p95`, `iter_ms_p99`, `iter_ms_max` | `iter_ms_min`, `iter_ms_p50`, `iter_ms_p95`, `iter_ms_p99`, `iter_ms_max`, with `loop_iterations` and `loop_last_differing` (the loop's check), for `native` and `hybrid`; an iteration ends with the end of the wait for its dispatch; the builds' figures in a loop row are medians over the iterations. Off unless `--loop` is given (600 in the owner's run). No loop of S |

**None of the probe's times selects a path.** A probe scene is not a
frame of the title: time follows triangle counts, what is rebuilt and
the number of bottom levels, not the workload class. No ratio is
promised. Memory has no threshold: host structures are added to the
guest's and reported. Host compaction is not designed and has no column.

## Output

Text files of measurements and synthetic data only, safe to share.
`rt-probe.csv`: `#` lines hold `key=value` (probe version, commit, date,
seed, adapter, driver version, vendor and device id for the report only,
ray-tracing tier, wave lane range, video memory, validator version, k);
then one row per scene class, program, path, form, structure source, box
width and node-memory place. A column that does not apply is empty,
never 0. `rt-translate.csv`: one row per program, path and form with the
columns of measurement 2. `rt-probe-mismatch.csv`: the first 64 failing
rays per class and pair, as hex words, enough to replay one ray in a
unit test. The validation record (dxr-architecture.md, "Selection") is
not one of these files: it is kept in the cache folder (`rt-record/`, one
text file a device and driver, `gpu/rt/record.rs`). The probe writes a
row for each class, path, form, structure source and side-table kind: its
verdict, the rule's number and its own version, the translator's hash,
the production container's hash, and `m_max` and its levels as fields
the selection does not read. The title comparison adds rows of its own,
one for each pair of title containers it compared, with the title verdict
and the title times: the probe's container is its own program's and is no
title program's, so the two findings cannot share a row.

**What the first version writes**, beside the table `--out` names and
named after it (`rt-probe.csv` here):

| File | Holds |
|---|---|
| `rt-probe.csv` | a first line `# victory rt-probe results 1`; `#` lines of `key=value`: `probe_version`, `policy` (the verdicts are under the crate's policy, not yet the rule), `reading`, `k` (not computed), `seed`, `classes`, `timed_rays`, `repeats`, `discard`, `loop`, `unix_time`, the device's own words (`adapter`, `adapter_vendor_id` and `adapter_device_id` for the report only, `adapter_software`, `adapter_dedicated_mb`, `adapter_shared_mb`, `driver`, `video_budget_mb_at_start`, `raytracing_tier`, `wave_lanes`, `shader_model_6_6`), `rt_backend_switch` (what the switch asked for that let the probe open the backend, as read: `hardware` is written `native`; and that both paths were measured whichever it names), `d3d12_debug_layer` (`off`, or `on` with the statement that no time of the file is a measurement), `rt_structures_bound_mb`, `rt_rebuild_after_refits`, `planned` (the run's dispatches and rays, counted before it starts), `path.software-gpu` (not built), per shader `container.<form>` (header digest and size) and `pipeline.<form>`, `backend_counters`, `rows`, and `files_not_written` only where a file of the run could not be written (below); then the line of column names and the rows. No commit (the binary does not know it; send it with the file) and no validator version (the validator runs on Linux; `scripts/validate-dxil.sh` names each container it checks by the same header digest) |
| `rt-probe.native.hits`, `rt-probe.hybrid.hits` | the path's hit records in `victory-rt`'s result format (dxr-validation-reference.md): a section per class and frame, one record of eight hex words per ray, the fingerprint of the generated case, a checksum. The rays are not in the file: class and seed name them. A refusal is a record like any other, kind bit 31 set; so is a record the dispatch left unwritten, which holds the poison word eight times (`a5c3f0f0`); a ray refused before the dispatch has the probe's own kind `0xC0000000`, which no shader writes. A row of a tracing path has a section exactly when its refusal counts are filled (the answers came back), also where a later step of the row failed. A path with no records gets no file, and an earlier run's file of that name is removed |
| `rt-probe-mismatch.csv` | the first 64 defects of each class, frame and path: class, seed, frame, path, ray, `victory-rt`'s name for the defect, the ray's twelve words and the record's eight as hex: enough to replay one ray in a host test. The column line alone when there is none |

The hit records are written first and the table last, so a table on disk
has its records beside it or says which are missing: a file that cannot
be written does not stop the others, the command prints `rt-probe: NOT
WRITTEN:` with the file and the reason and ends with an error, and the
table then holds a `files_not_written` line that names it. Where the
table itself cannot be written the command prints it to the console,
between two lines of dashes. That any of this happens after a run is
made unlikely beforehand: the folder is made and each of the four names
is tried before the device is opened (a file that is there is opened for
writing and left as it is; a name that is free is created and removed
again).

## What the owner runs

**Nothing below has run anywhere**: the probe has executed on no device,
on no WARP adapter and under no debug layer. A first run is as likely to
stop at a Direct3D call written from documentation as to produce figures,
and the line it stops at is then the result to send back.

With the conditions of "Measuring" in [progress.md](progress.md) that
apply without a title (steps 1 and 3: the release build of the commit,
other applications closed, the driver version noted), from the
repository's folder on the Windows machine, in a Command Prompt. The
`set` line before the probe's runs turns the experimental host
backend on for this window: the probe is behind the same switch as a
run's native and hybrid ray tracing and runs nothing without it. Any of
`native`, `hybrid`, `auto` and the older name `hardware` does the same
for the probe, which measures both paths under each; `native` is used
here. The `set` line
after them turns it off again, so that nothing else in the list runs
with it (in PowerShell: `$env:VICTORY_RT_BACKEND = "native"` and
`Remove-Item Env:VICTORY_RT_BACKEND`). The result files go to a folder
beside the repository, which the command makes:

```
cargo build --release -p victory
target\release\victory gpu-info
set VICTORY_RT_BACKEND=native
target\release\victory rt-probe --seed 1 --classes static-soup,dynamic-refit,dynamic-rebuild --rays 4096 --repeat 2 --discard 1 --debug-layer --out ..\rt-results\rt-first.csv
target\release\victory rt-probe --seed 1 --out ..\rt-results\rt-probe.csv
target\release\victory rt-probe --seed 1 --out ..\rt-results\rt-probe-b.csv
target\release\victory rt-probe --seed 2 --out ..\rt-results\rt-probe-2.csv
target\release\victory rt-probe --seed 1 --loop 600 --out ..\rt-results\rt-loop.csv
set VICTORY_RT_BACKEND=
cargo xtask rt-summary ..\rt-results\rt-probe.csv
cargo xtask rt-summary ..\rt-results\rt-probe-b.csv --against ..\rt-results\rt-probe.csv
cargo xtask rt-summary ..\rt-results\rt-probe-2.csv
cargo xtask rt-summary ..\rt-results\rt-loop.csv
cargo test -p victory-shader bvh
```

The first `rt-probe` line is a short run of three classes under the
Direct3D 12 debug layer (105 dispatches; builds, refits and rebuilds
among its 18 build submissions), to be made before the others: every call
of the probe's Direct3D part is unverified, and the layer says in words
why a call is refused where a run without it would stop or hang. It
needs the optional Windows feature "Graphics Tools"; without it the
command prints `rt-probe: NOT RUN:` and that line is what to send. Its
messages are printed as `rt-probe: validation layer, ...` lines and
counted in the rows' notes. **No time in `rt-first.csv` is a
measurement**; its header says so. If that run prints messages, send
them before making the long runs.

Options of `rt-probe`: `--seed <n>` (a number, or hex after `0x`; without
it the seed of the reference's own tables), `--classes all` or names with
commas between, `--rays <n>` (ray records in one timed dispatch, 2^20
unless given, at most 4,194,240), `--repeat <n>` (timed dispatches kept,
30), `--discard <n>` (run before those and left out, 3), `--loop <n>`
(iterations of the loop mode, none unless given), `--debug-layer` (the
Direct3D 12 debug layer on and its messages printed; for finding out why
a call is refused, never for a time), `--out <file>` (the table;
required, outside the repository).

| Command | What it prints |
|---|---|
| `victory gpu-info` | the adapter, driver and capabilities, the ray-tracing tier and the wave lane range among them; its last line is `ray tracing backend: ...`. Already asked for (`progress.md`, "Needed from the owner") |
| `victory rt-probe ...` | every line begins `rt-probe:`. First that no title file is read; then `device: key = value` lines (the header's device words); `this run: ...`, the dispatches and rays it will make, counted before it starts; a `pipeline, <form> shader: made in ... ms of processor time` or `not made: ...` line per shader; per class one line when the CPU reference starts and one when the class is done (`<class>: null 1/1 ran, 0 records not a miss; native <frames ran>, <defects> (<records unwritten>), <rays refused>, <records of the checking dispatch differing>; hybrid ...`); with `--debug-layer`, a `validation layer, ...` line per message; a `wrote <file> (<bytes> bytes)` line per file, and `NOT WRITTEN: <file>: <why>` for one that could not be; `rows: <n> ran, <n> failed, <n> not run, <n> not built`; how the verdicts and times are to be read; the `rt-summary` command for the file. Where nothing runs: `rt-probe: NOT RUN: <why>` (the switch asking for no path over host structures; a result file that cannot be written; `<adapter> cannot run inline ray queries: ...`; no debug layer), then that nothing was dispatched and no file written, and the command ends with an error |
| `cargo xtask rt-summary <file>` | the header's device and options (and, where they apply, that the run was under the debug layer and which files were not written); correctness by class and path (frames ran, rays, judged, clear, ambiguous, excluded, defects, unwritten, refused, records differing, the crate's verdicts on the rays judged); then, each in a table of its own: GPU time of the traversal dispatch alone with the null shader's beside it; GPU time of builds and refits; processor time; time blocked waiting for the card; video memory by owner with the adapter's figure beside it; system memory; the loop mode if it ran; `to look at`, a line for each row that failed, has a defect, an unwritten record, a refusal, an untimed region or records of a checking dispatch that differ; and how to read it: no row is a pass under the rule, and the times select no path and say nothing about the title |
| `... --against <earlier file>` | the two headers where they differ; row by row, every count that differs (two runs of one seed on one device and driver must agree on all of them); the two runs' hit-record files, section by section, with how many records differ and which sections the rows have that were not compared; the median traversal times side by side, neither called better; and one line of three: `the two runs agree` (equal counts in every row, and every section of hit records the rows have was found in both runs' files and is identical; the line gives the numbers), `THE TWO RUNS DIFFER`, or `NOT COMPARED` (counts equal, but a hit-record file is missing or unreadable, a section is in neither file, or neither run has a hit record: nothing is said about agreement) |

**How long a default run is**, as counts (no time is predicted; the
`this run:` line prints the same before anything is dispatched). At any
of the seeds above, a run with no option but `--seed` and `--out` is:

> 10 classes, 16 frames; 1470 dispatches (350 of the null shader; 1260 timed and kept, 126 timed and left out, 42 that check them and are in no time); 1140897958 rays traced; a ray list of up to 50331648 bytes and a hit buffer of up to 33554432 bytes

Each dispatch is a submission of its own that is waited for. Beside them
there are 32 submissions of builds (one per path and frame; over the 16
frames the scenes ask each path for 37 structures built and 9 refitted,
bottom and top levels together) and 84 copies of hit records back. The
CPU reference and the CPU model trace every ray of every frame once
(23,635 rays over the 16 frames) before a class is dispatched. With
`--loop 600` a run is 13,490 dispatches and 1,160,652,828 rays traced,
104 copies back, and the two classes with four frames add 2,400
submissions of builds.

**What to send back**: the whole `rt-results` folder (five tables, each
with its two `.hits` files and its mismatch list: a `.hits` file of a
default run is 23,635 lines, about 1.7 MB of text, a table under 200 KB),
the console output of each command, the `gpu-info` report, and the commit
the binary was built from (`git rev-parse HEAD`), which the files do not
hold. The files hold measurements and generated data only, and no path of
the machine. A run that hangs or is ended early has written nothing: the
files are written when the last class is done, so what it printed is what
there is to send. A run that finishes loses nothing to a file that cannot
be written: the others are written, and a table that cannot be is
printed.

`gpu-info` and the `bvh` tests need nothing new and are already asked for
(`progress.md`, "Needed from the owner"): the wave lane range, which
decides whether 64-lane tracers have a software pipeline at all, and the
node test against its reference on an NVIDIA card, which has no recorded
run.

| Part | State | Tier (progress.md) |
|---|---|---|
| Validator over today's containers | implemented; a first run is recorded in `progress.md` | 1b |
| Reference shaders compiled and accepted by the validator | the HLSL and the command are kept (`crates/victory-shader/tests/dxc-reference/`); no compiled output is | tool run |
| The first version's device-free parts: a scene as build inputs, the mapping of rays, record decoding with refusals and unwritten records, the statistics, the result files and the check of where they go, the switch, the run over a stand-in (every measurement column held against what the stand-in reported for it), `rt-summary` and its comparison of two files | implemented; tested (host tests) | 1 |
| The first version's three containers | accepted by Microsoft's validator on Linux (dxr-shader-emission.md) | 1b |
| The first version on a device: `hardware.rs` (the debug layer and its messages among it), `gpu/rt/device.rs`, the command | type-checked for Windows only; hardware-validated: nothing is | 2 |
| Region analysis, description, recogniser, native generator, R-ref, the constant k, the scene classes of "Scenes", the record and selection, levels 2 and 3 | speculative; all testable on any host | 1, 1b |
| Native variant against a CPU model in the test-only interpreter | speculative; model against model only | 1 |
| The software traversal as a shader over console-format nodes (S, `software-gpu`) | speculative; a named path with status `not built` | 3 |
| Store redirection and the per-class switch for the title comparison; every timing and memory figure | speculative | 3 |

## After the probe, on the title

Gated on the title's files, on the structures appearing (`progress.md`
C3) and on a software pipeline existing for the tracers. Per workload
class the census finds and the probe has passed, with the title's
coordinates inside the pass's `m_max`, and with `VICTORY_RT_CLASS`
naming that class alone, so that a difference belongs to it.

| Matter | Rule |
|---|---|
| **Frames that count** | Only frames with no dropped builder or trace dispatch (`gpu/draw.rs:161-163, 1099-1111`), no withholding (`gpu/memory.rs:310-325`) and no missed page: in the others the guest structure is partial or discarded while a host structure built from descriptions is whole, so software is not a reference. A comparison also needs a non-zero count of the class's dispatches on the forced path: a class that never ran passes nothing. Both counts go in the verdict |
| **Level 2, on the same rays** | Each trace dispatch of the class runs twice in one submission: the candidate first, its stores redirected to scratch copies of the pages it writes, then the software variant as always; the pages are compared under the rule. Unbuilt: it needs the written pages ahead of the run (today's marks come after it, `emit/mod.rs:132-137`) and a snapshot for a program that reads what it writes. A program whose stores cannot be redirected has no title level 2, and its class is not selected by `auto` |
| **Level 3** | The owner's paired captures (`progress.md` C0) and the noise pairs, judged tile by tile against each tile's own noise |
| **Rays with no native answer** | The screen's count of infinite rays is read back each frame; a non-zero count fails the class |
| **Title times** | The existing per-command timing (`gpu/host.rs:425-461`) is a starting point, not the instrument. Its key is the command's kind and the pipeline's name, which is the title's shader hashes, so two variants of one program share a bucket (`gpu/host.rs:436-441`; `gpu/pipeline/host.rs:628-635`). A command's time runs to the next noted command, so a host build or barrier recorded in between is charged to the command before it (`gpu/host.rs:1947-1950, 2226-2232`). Past 4,096 notes a list is not timed (`gpu/host.rs:970-972`). Needed: a begin and an end stamp per trace dispatch with path and form in the key, and a pair of its own around every host build, update and barrier. The timing exists only in the diagnostics build and runs each list on its own (`gpu/host.rs:985-997`), which changes waits and interleaving: its figures are per-command costs, and frame time comes from a normal build (`progress.md`, "Measuring") |
| **The other measurements** | `recognise_ms` and `variant_ms` on the "shader translated" line and in `victory translate --cache`'s totals (`apps/victory/src/translate.rs:414-425`); owner-counted bytes as in measurements 3 and 4; per-frame counts of dispatches by path and by downgrade reason |

**Faster, for the selection rule** (a proposal; the owner has not said
what "fastest" means). From the comparison run, per counted frame:
`title_trace_ms`, the sum of the class's trace dispatches on a path, and
`title_build_ms`, the host builds, updates and barriers the path adds
for the structures the class traces, charged in full to every class
tracing them (an overstatement, on the safe side). A path is faster than
software for a class when the median of its trace plus build time is
below the median of software's trace time by more than the larger of
the two interquartile ranges, and a normal-build run with the class
forced shows frame time no worse in the ledger.

`auto` selects a row only when all three gates hold: the probe passed,
the title verdict passed, and the title times show it faster. Until
then the path runs only when forced, and says so. As built
(`gpu/rt/select.rs`), the third gate is the comparison of medians above,
from the times in the title row. The second clause, the normal-build run
with the class forced, is not a field of the record and `select()` does
not check it.

Part of the same proposal, and as unaccepted: which of two paths that
pass all three gates is "the fastest". The two were timed in two runs,
each against software. As built, the hybrid is taken only where the two
runs can be compared (software's medians in them differ by no more than
the larger of its two interquartile ranges) and the hybrid's median is
below the native path's by more than the larger of the two paths'
ranges; otherwise the native path, first in the order of trial. Two
medians from runs of different scenes, or a difference inside the
spread, decide nothing.

Every verdict in the record names the rule it was given under by a
number that stands for the rule's text and constants, and the probe's
names the probe's version. `select()` counts a verdict only under the
rule and version its build accepts. The rule above and in
dxr-compatibility.md is not accepted, so the build accepts none and
`auto` selects nothing, whatever a record holds.
