# Validating a host ray-tracing path

How equivalence and cost would be measured on the owner's Windows machine
with no file of any title. Companion to
[dxr-architecture.md](dxr-architecture.md) (step 3 of its decision) and
[dxr-compatibility.md](dxr-compatibility.md).

**None of this has been run, and the probe does not exist.** Thresholds
below are fixed here, before any measurement; a change to one after
results are seen is a dated revision of this document. `emit/` is
`crates/victory-shader/src/emit/`, `gpu/` is `crates/victory-gpu/src/`.

## What must be built first

Absent from the tree today (`apps/victory/src/main.rs` has no `rt-probe`;
no `RayQuery` anywhere in `crates/`):

* `RayQuery` in the emitter: the `dx.op` calls, the shader flag, the
  structure resource kind and its binding; and whether a structure handle
  may come from the heap by index, against the validator.
* A host build helper: prebuild sizes, bottom levels of boxes and of
  triangles, top levels, barriers, timestamps.
* The CPU reference outside the Windows-only test module
  (`emit/mod.rs:201-202`), a scene generator, a
  builder of guest-format trees, a traversal loop for S.
* `victory gpu-info` printing the tier as a number, and "unknown" when the
  query fails: today it prints met or DEGRADED and a failed query reads as
  tier 0 (`gpu/caps.rs:177-181, 200-228`).

## What is compared

The same scene and the same rays through each path.

| Path | What it is | Role |
|---|---|---|
| **R** | CPU reference: the model in `emit/bvh_tests.rs:41-155`, driven by a plain stack loop. Two answers per ray: **R-tree** (with the box tests) and **R-all** (every triangle tested, no boxes) | the oracle for the emulation. Not for the console: the emulation is unchecked against one |
| **S** | software on the GPU: the same loop as a compute program, each node tested by the arithmetic of `emit/lower/bvh.rs`, emitted by the functions the lowering uses | today's path |
| **HB** | host boxes: `RayQuery` over one procedural box per triangle pointer; each candidate runs the same triangle arithmetic. Run twice: **HB-n** never commits; **HB-c** commits the quotient rounded up, valid only for a nearest-hit loop | option C. HB-n is the form with no order assumption; its cost is the one compared with S |
| **HI** | the probe's software top-level loop with one `RayQuery` per instance entered, over a single-instance host structure | option F |
| **HT** | host triangles: `RayQuery` over triangles made from the same vertices | option A; measured, never passed |

Node data for S and HB is read as the runtime reads it: a shared buffer in
system memory through the page-table path
(`gpu/host.rs:3955-3965`; `emit/lower/bvh.rs:259-288`).
Every timing is repeated with the buffer on the card, as separate rows.

Limits. The loops are the probe's own, not the title's: the probe measures
the node test, the structures and the style of traversal on generated
scenes. It does not show what any title program would gain or keep, and
its share of clean rays says nothing about the title's, whose inverse
direction and per-call extent are its own. Instances are the probe's table
and claim nothing about the guest's layout. Results hold for the device
and driver they were taken on.

## Scenes

Generated from one 64-bit seed (the generator of the existing tests,
`emit/bvh_tests.rs:427`). The guest-format tree comes from a small builder
in the probe: four-wide box nodes and 64-byte triangle nodes in the layout
the emulation reads. Each class runs with 32-bit and with 16-bit box
bounds (rounded outward by the builder).

| Class | Scene | Rays | Probes |
|---|---|---|---|
| `single` | one triangle | random hits and misses | basic agreement |
| `edge` | one triangle; two sharing an edge; a fan; a closed mesh | exactly at edges and vertices on exact coordinates; within 1, 2 and 8 units in the last place either side | the edge rule; a ray from inside a closed mesh must never escape |
| `far-graze` | the `edge` scenes at 2^16, 2^20 and 2^24 from the ray origin | edge and vertex grazing | guest hits accepted through rounding that a host box test may not present |
| `flat` | triangles lying in axis planes (zero-thickness boxes) | in-plane, grazing and crossing | the same; not the specification's "degenerate" box |
| `extent` | one triangle | hits at exactly t = 0 and t = extent; a negative extent; an extent that does not shrink between calls | inclusive against exclusive; rays with no host form |
| `thin` | slivers and needles, aspect up to 2^20 | random and grazing | precision |
| `degenerate` | zero-area and collinear triangles, a NaN vertex, inverted boxes | zero direction components; NaN in the extent, the origin, the direction, the inverse, one at a time; an infinite extent | per-operand screening. Expected: a NaN extent gives the hits of an infinite one |
| `inverse` | any | inverses that are not 1/direction: zero components replaced, clamped, off by a unit in the last place | what a title-supplied inverse does to box culling. S and R keep it; HB cannot |
| `tie`, `near-tie` | coplanar duplicates; hits at equal t; hits one to a few units in the last place of the quotient apart | through both | the tie rule; what a rounded commit culls |
| `partial` | triangle nodes holding one to three triangles, spare bytes filled with non-vertex data | random | the primitive is a pointer, not a node |
| `instances` | 1 to 4,096 instances of a few bottom levels: rotation, uneven scale, mirroring, large translation, heavy overlap | random | the object-space ray; facing under a mirror; HI against HB |
| `masks` | instance masks 0x01 to 0xFF and zero; masks wider than 8 bits folded onto 8 | rays with each mask | inclusion; the fold as a superset |
| `large` | small triangles at 2^16, 2^20 and 2^24 from the origin | long rays | precision of t far from the origin |
| `refit` | moving vertices over 120 steps, fixed topology | random, each step | agreement after a rebuild and after an update; their times |
| `many-small` | 64 to 4,096 small bottom levels rebuilt every iteration under a top level of thousands of instances | random | the regime a fork reports for dynamic structures |
| `burst` | N builds in one submission, N rising | none | build time against the runtime's 250 ms warning and 2 s limit (`gpu/host.rs:1533-1549`) |
| `scale` | 10^3 to 10^6 triangles | 2^20 rays, coherent and incoherent | cost |

## Comparing

Per ray, each path writes one record: hit flag, identity (instance index,
node address, triangle number), the four result words (t numerator,
denominator, two barycentric numerators) and the facing sign. HT has a
float t and two float barycentrics. HB also writes how often each
(ray, primitive) pair was presented.

The tie rule is fixed for all paths: among hits whose t is equal as exact
fractions, the lowest node address wins, then the lowest triangle number.
It is a static key, the probe's own. A title loop that keeps the first or
last candidate among equals has no such key, and nothing here covers it.

A ray is **clean** when R-tree and R-all agree in every field. On the
others the guest's own box test culled a triangle that the full test hits.

**Equal**: every record identical, bit for bit. **Close**: hit flag,
identity and facing identical for every ray; t and barycentrics within the
tolerance below. **Differs**: anything else.

### Rules, fixed before measuring

| Pair | Rule |
|---|---|
| S against R-tree | **Equal**, every class, every ray. A difference is a defect of the emulation on that driver: the existing tests use exact inputs and cannot see a driver contracting or reordering float operations |
| HB or HI against S, every ray | **Equal** is the only pass. One bar, the same in dxr-architecture.md |
| HB or HI against S where only non-clean rays differ | **Equal on clean rays only**: not a pass. The count (`box_culled`, with its direction) is what the owner would have to accept as an exception to rule 3, or what re-applied guest box tests would have to remove |
| A path misses where S hits, any ray | **fail**, always: a lost hit is a lost shadow or light |
| A (ray, primitive) pair presented more than once with the no-duplicate flag set | **fail** for any loop that is not idempotent per candidate |
| The same seed twice, and again after the device is re-created | records identical; otherwise **fail** (determinism is assumed by every other rule) |
| HT against S | **Close** needs t within 8 units in the last place of S's quotient and each barycentric within 2^-16. The numbers classify; they do not make HT acceptable |

Padding of host boxes, `pad`: none; 1 and 4 units in the last place of the
largest box coordinate; 4 units in the last place of the scene's diameter
(the largest origin-to-vertex distance the scene allows, since the
emulated test's error grows with that distance and a build does not know
the origin). A padding at which a hit is lost is recorded as failing. No
padding is a proof: a pass is a result on these rays and this driver.

## Measurements

| Measurement | How | Columns |
|---|---|---|
| Agreement | records read back and compared on the CPU | `rays`, `clean`, `equal`, `close`, `differ`, `hw_only_hits`, `sw_only_hits`, `id_flips`, `box_culled`, `t_ulp_max`, `bary_abs_max` |
| Repeats and determinism | presentations per pair, with and without the flag; the repeat runs | `dup_candidates`, `dup_candidates_noflag`, `repeat_equal` |
| GPU time | a timestamp pair around the dispatch on a queue doing nothing else; 3 runs discarded, 30 kept | `rays_per_s`, `gpu_ms_min`, `gpu_ms_p50`, `gpu_ms_p95`, `gpu_ms_max` |
| Build, refit, derive | timestamp pairs around each build and around the pointer walk that derives boxes; scratch as one range per build and as one build per barrier | `build_ms_p50`, `refit_ms_p50`, `derive_ms_p50`, `scratch_mode` |
| CPU preparation | time on the submitting thread before the fence wait (`gpu/host.rs:1531-1532` keeps the two apart) | `cpu_prep_ms_p50` |
| Video memory | prebuild sizes, bytes allocated, compacted size when asked for, the adapter's own usage before and after; the guest tree's bytes beside them | `as_bytes`, `scratch_bytes`, `as_compacted_bytes`, `video_mb_delta`, `guest_bytes` |
| System memory | working set and private commit before and after; never summed with the above ([memory.md](memory.md), "Four quantities") | `working_set_mb_delta`, `commit_mb_delta` |
| Synchronisation | wait from submission to fence for a build alone, and for a trace with and without a build ahead of it | `submit_wait_ms_p50`, `build_then_trace_ms_p50` |
| Loop mode | 600 iterations of derive, builds or refits, top-level build and trace (HB, HI) against 600 of trace alone (S) | `iter_ms_p50`, `iter_ms_p95`, `iter_ms_p99`, `iter_ms_max`, `break_even_iters` |

The loop mode is not a frame of the title.

## Output

Two CSV files, measurements and synthetic data only, safe to share.
`rt-probe.csv`: `#` lines hold `key=value` (probe version, commit, date,
seed, adapter, driver version, vendor and device id for the report only,
ray-tracing tier, wave lane range, video memory). Then one row per scene,
path, padding, box width, commit form and node-memory place, with the
columns `class, scene, triangles, instances, box_bits, path, pad,
node_mem` and those of the table above. A column that does not apply is
empty, never 0. `rt-probe-mismatch.csv`: the first 64 differing rays per
class and pair, as hex words, enough to replay one ray in a unit test.

## Pass and fail

Per device and driver. A result is not carried to another driver.

* S against R-tree not Equal on any class: stop. The software path is
  itself host-dependent on this device, a defect to fix before anything
  is compared with it (rule 3 of
  [gpu-compatibility.md](gpu-compatibility.md)).
* HB or HI **passes** a class only when Equal to S on every ray, with no
  repeat and identical repeat runs. A program whose loop depends on a
  class that did not pass is not eligible, unless the owner has accepted
  that class's `box_culled` count as an exception in writing.
* HT has no pass. Its row says Equal, Close or Differs and by how much.

**Cost.** HB's cost per iteration is derive, plus bottom-level builds or
refits, plus the top-level build, plus the trace, plus CPU preparation.
HB pays on a class and size when that median is below S's by more than
the larger of the two spreads (maximum minus minimum over the 30 runs).
One-time builds are reported as `break_even_iters`: the iterations after
which they are repaid, or never. No ratio is promised. Memory is reported
with no threshold: host structures are added to the guest's.

**What the probe can end.** On the target card: a lost hit or a repeated
candidate on `single`, `edge`, `far-graze` or `instances` ends C and F on
that driver. So does HB-n not paying on `scale` **with build time left
out**. If HB pays only without builds, or only as HB-c, nothing ends:
the answer waits for per-dispatch timing and structure counts on the
title, because the probe's software loop is not the title's.

## What the owner would run

With the conditions of "Measuring" in [progress.md](progress.md).

```
victory gpu-info                                   exists
cargo test -p victory-shader bvh                   exists
victory rt-probe --seed 1 --out rt-probe.csv       proposed
victory rt-probe --seed 1 --out rt-probe-b.csv     proposed (repeat)
victory rt-probe --seed 2 --out rt-probe-2.csv     proposed
victory rt-probe --seed 1 --loop 600 --out rt-loop.csv   proposed
cargo xtask rt-summary rt-probe.csv --against rt-probe-b.csv   proposed
```

The first two need nothing new and are already asked for (`progress.md`,
"Needed from the owner", items 2 and 5): the wave lane range, which
decides whether 64-lane tracers have a software pipeline at all, and the
node test against its reference on an NVIDIA card, which has no recorded
run.

| Part | Tier (progress.md) | Without a GPU |
|---|---|---|
| Scene generator, guest-tree builder, R-tree and R-all, the tie rule, CSV, summary, pass rules | 1 | yes |
| `RayQuery` in the emitter: container bytes, flags | 1 for the bytes; 3 for the validator | in part |
| S, HB, HI, HT on a device; every timing and memory figure | 3 | no |

## After the probe, on the title

Gated on the title's files, on C3 and on a software pipeline existing for
the tracers. In outline: the same scene with the switch on `software` and
on `auto`; the output buffers of each trace dispatch compared record for
record; captures compared (C0); `own_shaders_mb`,
`own_pipeline_library_mb` and working set before and after; and the
performance log extended with dispatches answered by the host, dispatches
that took the software loop, structure memory and build time per frame.
A host path is offered by default only after that.
