# Runs on hardware

What has actually run on a device, in order. Each entry says what ran,
what it shows and what it does not. Figures are the owner's result files'
as sent; the files themselves stay with the owner (the repository holds
no result file). Nothing here is a statement about the title.

## 2026-11-27: the synthetic ray-tracing probe, short run under the debug layer

**Who and what.** The owner, on the reference machine. `victory rt-probe`,
probe version 1, seed 1, at commit `eed082f`. Device as the probe read
it: NVIDIA GeForce RTX 5070 Ti, driver 32.0.16.1692, ray tracing tier 1.2.
Classes `static-soup`, `dynamic-refit`, `dynamic-rebuild`; 4,096 ray
records a timed dispatch, 2 repeats kept after 1 left out, no loop;
Direct3D 12 debug layer on.

**Size of the run.** 3 classes, 9 frames, 105 dispatches (15 of the null
shader, 42 timed and kept, 21 timed and left out, 21 checking dispatches),
318,112 rays traced. Rows: 21 ran, 0 failed, 0 not run, 9 not built (the
software traversal as a shader does not exist yet).

**Correctness, under `victory-rt`'s comparison as the crate has it (not
the proposed rule of `dxr-compatibility.md`).**

| Class | Path | Frames | Rays judged | Clear | Ambiguous | Defects | Unwritten | Refused | Check differing | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| static-soup | native | 1 | 2,000 | 2,000 | 0 | 0 | 0 | 0 | 0 | pass |
| static-soup | hybrid | 1 | 2,000 | 2,000 | 0 | 0 | 0 | 0 | 0 | pass |
| dynamic-refit | native | 4 | 4,800 | 4,795 | 5 | 0 | 0 | 0 | 0 | pass (4) |
| dynamic-refit | hybrid | 4 | 4,800 | 4,795 | 5 | 0 | 0 | 0 | 0 | pass (4) |
| dynamic-rebuild | native | 4 | 4,800 | 4,800 | 0 | 0 | 0 | 0 | 0 | pass (4) |
| dynamic-rebuild | hybrid | 4 | 4,800 | 4,800 | 0 | 0 | 0 | 0 | 0 | pass (4) |

**What it shows.** On this device and driver: the three containers the
emitter writes for inline ray queries (native, hybrid, null) are taken by
the driver and their pipelines are created; bottom and top levels are
built, and refitted in place, by the host backend; the native and the
hybrid path each answer 11,600 rays as the CPU reference does within the
crate's policy, with no record left unwritten, no ray refused, and every
checking dispatch equal to the timed ones. This is the first time any
code of this work has run on a card.

**What it does not show.** The scenes hold 70 to 288 triangles and three
of the classes: nothing about cost, and nothing about the other classes.
The verdicts are under the crate's policy, not the proposed rule, which
the owner has not accepted. The debug layer was on, so no time in the
file is a measurement. No title program, structure or frame is involved:
it is synthetic execution, not validation in the title.

**Figures worth keeping (debug layer on, so orders of magnitude only).**

| What | Value |
|---|---|
| GPU time of a 4,096-ray dispatch | null 0.001 ms; native 0.003 ms; hybrid 0.009 to 0.014 ms |
| GPU time of builds | bottom levels 0.08 to 0.25 ms a frame; top level 0.02 to 0.03 ms; refits 0.012 to 0.018 ms (bottom) and 0.008 ms (top) |
| Pipeline creation, processor time | null 2,677 ms (the first pipeline of the process); native 382 ms; hybrid 467 ms |
| Recording the first build | 165 ms (first use); 0.07 to 1.5 ms afterwards |
| Blocked waiting for the card | builds 0.07 to 0.43 ms; a dispatch 0.04 to 0.13 ms; read-back 0.04 to 0.20 ms |
| Video memory by our count | structures 0.13 to 0.26 MB; scratch 4.3 MB; buffers 1.6 to 1.8 MB |
| The adapter's figure for the process | 16.8 MB before the first build, 89.3 MB after; budget 15,967 MB |
| Private commit | 195.7 MB before the first build, 317.3 MB after it, 319.4 MB at the end |
| Working set | 7,155 MB before the first build, 7,217 MB at the end |

**Messages of the validation layer (4 rows had one or two).**

* Warning 1424, once, at the first null-shader row:
  `ID3D12Fence1::SetEventOnCompletion`: a wait for fence value zero is
  always satisfied. Read from the code: the backend's own queue numbers
  its submissions from 1 (`rt/device.rs`, `Queue::submit`), so value 0 is
  no submission; some wait is asked for before anything was submitted.
  No result depends on it (no record was unwritten), but a wait that
  cannot wait should not be made. Being fixed with the next change to the
  probe.
* Warning 1328, three rows: `CreateCommittedResource` ignores the initial
  state `UNORDERED_ACCESS` of a buffer; buffers are created in the common
  state. Read from the code: `rt/device.rs` creates buffers the shaders
  write in that state and records it as their state. Being fixed with the
  same change: such a buffer is created in the common state and its
  recorded state says so.

**Open: the working set.** The process's working set was 7.2 GB against
0.32 GB of private commit, and it was already 7.16 GB before the first
build. Both figures are read from the right fields (`WorkingSetSize` and
`PagefileUsage` of the process counters). A working set far above private
commit is pages that are not the process's private commit: shared
sections, mapped files, images. Nothing in the probe or in the
application's start maps memory of that size (read from the code), so the
pages came with the device, the driver or the validation layer; which, is
not known. The next change to the probe adds the private and the
shareable part of the working set as columns, sampled at the process's
start, after the debug layer is on, after the device is made and after
the first pipeline, and a breakdown of the working set by kind of region.
Until that runs, the cause is unknown, and the working set must not be
read as memory this code allocated. It bears on the owner's earlier
report of about 27 GB of "reported memory" in the title: if that figure
was a working set, it may likewise hold pages that are no private commit.
