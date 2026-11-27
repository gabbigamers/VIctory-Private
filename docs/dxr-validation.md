# Validating the ray-tracing compatibility layer

How rendering equivalence and cost are tested and measured, on synthetic
programs first and with no file of any title. Companion to
[dxr-architecture.md](dxr-architecture.md) (steps 3 and 4 of its order)
and [dxr-compatibility.md](dxr-compatibility.md), which holds the rule
this procedure applies.

**None of this has been run on hardware, and the probe does not exist.**
The rule, its tolerance and its thresholds are a proposal written on
2026-11-27, before any measurement, and await the owner's acceptance;
after that a change is a dated revision. `emit/` is
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

Absent from the tree today (`apps/victory/src/main.rs` has no `rt-probe`;
no `RayQuery` anywhere in `crates/`):

* The smallest version of the layer (dxr-architecture.md): region
  analysis, the workload description, a recogniser and a native generator
  for the synthetic programs, `RayQuery` in the emitter.
* The CPU reference outside the Windows-only test module
  (`emit/mod.rs:199-229`): **R-bits**, the emulation's model in single
  precision (`emit/bvh_tests.rs:41-155`); **R-ref**, the same geometry
  in double precision with the classes of the rule; and the host test
  that computes the tolerance constant k from the two.
* A scene generator that knows each triangle's surface, material,
  neighbours, bottom level and instance; a builder of guest-format trees;
  a host build helper (prebuild sizes, builds, barriers, timestamp pairs).
* `victory gpu-info` printing the tier as a number, and "unknown" when
  the query fails (`gpu/caps.rs:177-181, 200-228`).

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

Five quantities, each in its own columns, never summed with another.

| Measurement | How | Columns |
|---|---|---|
| **1. GPU time of traversal dispatches** | a begin and an end timestamp around the trace dispatch of the **production** container alone, on a queue doing nothing else; 3 runs discarded, 30 kept | `trace_gpu_ms_min`, `trace_gpu_ms_p50`, `trace_gpu_ms_p95`, `trace_gpu_ms_max`, `rays_per_s`, `container_hash` |
| **2. CPU translation overhead** | timers in the translator and the recorder, per program and in total | `recognise_ms`, `variant_ms` (per path and form), `driver_compile_ms`, `convert_ms_p50` and `convert_ms_p95` (build descriptions, per iteration), `bind_extra_us_p50`; totals `recognise_ms_total`, `variant_ms_total` |
| **3. Video memory** | **bytes counted by owner, from the sizes allocated**, per path: structures, scratch, side tables, boxes. Cross-check only: the adapter's local-segment usage in a fresh device, read after the scene's buffers exist and again after one path's objects are built (it is process-wide, so nothing else may be alive) | `as_bytes`, `scratch_bytes`, `side_table_bytes`, `box_bytes`; `video_local_mb_delta` |
| **4. System memory** | bytes counted by owner: the variants, the library's growth, kept descriptions, and the guest tree where it lies in system memory (the default rows; on the card for the on-card rows, then under 3). On the title also the host copies, windows and snapshots the layer causes. Cross-check only: working set and private commit of the probe, read at the same two events (the generator and reference dominate them) | `variant_dxil_bytes`, `library_bytes_delta`, `registry_bytes`, `guest_bytes`, `guest_segment`; title: `own_buffer_copies_mb`, `own_windows_mb`, `own_snapshots_mb`, `video_shared_mb`; `working_set_mb_delta`, `commit_mb_delta` |
| **5. Rendering correctness** | the three levels above | `k`, `m_max`; level 1: `rays`, `unambiguous`, `sliver_rays`, `amb_edge`, `amb_seam`, `amb_extent`, `amb_tie`, `amb_filter`, `amb_degenerate`, `agree`, `admissible`, `lost_hits`, `gained_hits`, `id_flips`, `surface_flips`, `facing_flips`, `filter_flips`, `out_of_tol`, `t_err_max`, `bary_err_max` (in tolerances and in spacings); level 2: `consumed_equal`, `consumed_within`, `consumed_differ`; level 3: `px_differ`, `px_share`, `px_group_max`; and `dup_candidates`, `screened_nonfinite`, `repeat_equal`, `equal_to_s`, `sensitive` |

Reported apart from all five, and never inside trace time:

| Measurement | How | Columns |
|---|---|---|
| Build and refit | a timestamp pair of its own around each bottom-level build, update, top-level build and barrier, and around the pointer walk H-node needs; scratch as one range per build and as one build per barrier | `build_ms_p50`, `refit_ms_p50`, `tlas_ms_p50`, `derive_ms_p50`, `scratch_mode` |
| Synchronisation waits | from submission to fence for a build alone, and for a trace with and without a build ahead of it; time on the submitting thread before the wait (`gpu/host.rs:1531-1532`) | `submit_wait_ms_p50`, `build_then_trace_ms_p50`, `cpu_prep_ms_p50` |
| Loop mode | 600 iterations of what a path adds per frame. `iter_ms` is wall time on the submitting thread, from the start of an iteration's conversion to the fence of its last submission, the wait included; S's loop is its trace alone | `iter_ms_min`, `iter_ms_p50`, `iter_ms_p95`, `iter_ms_p99`, `iter_ms_max` |

**None of the probe's times selects a path.** A probe scene is not a
frame of the title: time follows triangle counts, what is rebuilt and
the number of bottom levels, not the workload class. No ratio is
promised. Memory has no threshold: host structures are added to the
guest's and reported. Host compaction is not designed and has no column.

## Output

CSV files of measurements and synthetic data only, safe to share.
`rt-probe.csv`: `#` lines hold `key=value` (probe version, commit, date,
seed, adapter, driver version, vendor and device id for the report only,
ray-tracing tier, wave lane range, video memory, validator version, k);
then one row per scene class, program, path, form, structure source, box
width and node-memory place. A column that does not apply is empty,
never 0. `rt-translate.csv`: one row per program, path and form with the
columns of measurement 2. `rt-probe-mismatch.csv`: the first 64 failing
rays per class and pair, as hex words, enough to replay one ray in a
unit test. `rt-record`: the validation record (dxr-architecture.md,
"Selection"); the probe writes its verdicts, `m_max` and container
hashes, the title comparison adds the title verdict and title times.

## What the owner runs

With the conditions of "Measuring" in [progress.md](progress.md).

```
scripts/check-linux.sh tests                           exists (tier 1)
VICTORY_DXV=.../dxv scripts/validate-dxil.sh           exists (tier 1b)
victory gpu-info                                       exists
cargo test -p victory-shader bvh                       exists
victory rt-probe --seed 1 --out rt-probe.csv           proposed
victory rt-probe --seed 1 --out rt-probe-b.csv         proposed (repeat)
victory rt-probe --seed 2 --out rt-probe-2.csv         proposed
victory rt-probe --seed 1 --loop 600 --out rt-loop.csv proposed
cargo xtask rt-summary rt-probe.csv --against rt-probe-b.csv   proposed
```

The third and fourth need nothing new and are already asked for
(`progress.md`, "Needed from the owner"): the wave lane range, which
decides whether 64-lane tracers have a software pipeline at all, and the
node test against its reference on an NVIDIA card, which has no recorded
run.

| Part | State | Tier (progress.md) |
|---|---|---|
| Validator over today's containers | implemented; a first run is recorded in `progress.md` | 1b |
| Reference shaders compiled and accepted by the validator | done in a session's scratch directory, not kept in the repository | tool run |
| Region analysis, description, recogniser, native generator, R-bits and R-ref, the constant k, scene generator, record and selection, CSV and summary | speculative; all testable on any host | 1, 1b |
| Native variant against a CPU model in the test-only interpreter | speculative; model against model only | 1 |
| Every path on a device; timestamp pairs; store redirection and the per-class switch for the title comparison; every timing and memory figure | speculative | 3 |

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
then the path runs only when forced, and says so.
