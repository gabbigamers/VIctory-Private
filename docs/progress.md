# Progress

The working record of the compatibility effort: the objective, what has been
measured, what is blocked and on what, and what to do next. Read this first
in a new session; update it in the commit that changes the facts.

Rules that do not change between sessions:

* Only this repository is modified. The supporting repositories are
  read-only engineering references.
* Nothing from the game enters the repository ([asset-policy.md](asset-policy.md)).
  No content-derived fingerprints of shipped data (shader or executable
  hashes) in documents either.
* No package decryption, console keys or access-control bypass, in code or
  in documents. The user supplies files they are entitled to use.
* Code is reused only under a licence compatible with GPL-3.0-or-later, with
  attribution, as a module or dependency. No licence file means no code
  reuse (documented facts only).
* Every GPU-related change follows [gpu-compatibility.md](gpu-compatibility.md):
  NVIDIA-first, across generations, by capability and never by model.
* A number in this document says where it comes from: **measured** (a run,
  with its commit and machine), **owner-reported**, or **project-reported**
  (the README). Estimates are marked as estimates and never fill a ledger.

## Objective

The title runs on the owner's machine, correctly first and then at speed.
The machine is where development and performance are validated; it is not
what the code is written for (see [gpu-compatibility.md](gpu-compatibility.md)
for the hardware the layer supports and what has been shown on each):

| | Target machine | Tested by the project so far |
|---|---|---|
| GPU | NVIDIA RTX 5070 Ti, 16 GB | AMD RX 6700 XT, 12 GB |
| CPU | Ryzen 7 5800X (8 cores, 16 threads, AVX2) | not recorded |
| RAM | 16 GB | 32 GB, commit limit about 80 GB |
| Storage | NVMe SSD | not recorded |
| OS | Windows | Windows 11 |

### Acceptance criteria

All seven, on the target machine, in the default console mode
(`--console series-x`), in one unbroken run.

| # | Criterion | Track | Measured by |
|---|---|---|---|
| A1 | Graphics render visually correct | Correctness | captured frames reviewed against the console's picture; no draw dropped for want of a host pipeline (`dropped_per_frame` is 0) |
| A2 | The map renders accurately and completely | Correctness | captures at fixed places and times of day; no missing or stand-in geometry |
| A3 | Textures, geometry, materials, lighting and shaders are correct | Correctness | every shader program translated; shader reference tests pass on the target GPU; ray-traced shadows and indirect light present |
| A4 | The world streams reliably, nothing missing or corrupted | Stability | a long traversal with no missing draws, no stand-in assets and nothing in `crashes.log` |
| A5 | Stable with 16 GB of physical RAM | Memory | the performance log over a long run: no error 1455, commit under the machine's limit, working set within physical memory |
| A6 | A sustained 30 fps with consistent pacing | Performance | the performance log: see thresholds below |
| A7 | A frame time of 33.33 ms | Performance | the performance log: `p50_ms` |

Proposed thresholds for A5 to A7, to be confirmed by the owner once the
first measured run shows what the numbers look like. Over a world run of at
least 30 minutes, in every five-second period after loading:

* `p50_ms` within 33.33 ± 1 ms and `p99_ms` at most 50 ms;
* `late_share` (frames over 50 ms) at most 0.005 and `jitter_ms` at most 2;
* `system_commit_mb` under `commit_limit_mb` with the paging file at the
  Windows default, and `working_set_mb` flat (no growth across the run).

### Order of work

1. **Correctness before speed.** A change that makes frames faster by
   leaving something out, lowering the picture's quality or approximating a
   shader is not an optimisation and is not accepted. Every performance
   change is checked against captures taken before it.
2. **No frame generation.** The frame rate is frames the title rendered.
   The upscaler shim already refuses every FidelityFX effect but upscaling
   (`crates/victory-hle/src/modules/fidelityfx.rs`). Driver-level frame
   generation (NVIDIA Smooth Motion and the like) is off for every
   measurement. The title's own upscaling in Series S mode is the title's
   behaviour, but acceptance is measured in Series X mode.
3. **Measured, not assumed.** Memory and speed are investigated from
   profiles of real runs on the target machine. The README's requirement
   figures come from another machine and are a starting hypothesis, not a
   limit.

## Baselines

| Date | Source | Machine | Build | Scene | Result |
|---|---|---|---|---|---|
| 2026-11-25 | project-reported (README) | RX 6700 XT 12 GB, 32 GB RAM | not recorded | intro cutscene and prologue | about 1 fps (0.9 to 1.0 s a frame, warm cache); about 33 GB of commit, about 10 GB of video memory |
| before 2026-11-26 | owner-reported | target machine | not recorded | not recorded | about 15 fps |

The owner-reported 15 fps is the preliminary baseline for the target
machine: it shows the machine runs the title, at about half the target
rate. Its build, scene, console mode, window size and how it was read (the
window caption?) are not recorded yet, so it is not comparable with later
measured rows until a run with the performance log replaces it (P0 below).
The fifteen-fold gap to the project's own figure is unexplained; the first
measured run's split between `preparing_ms` and `gpu_ms` says which side of
a frame the two machines differ on.

## Milestone tracks

Four tracks, kept apart so that an improvement on one cannot hide a
regression on another. Each has its own ledger; a row is one measured run.
Tracks are worked in this order where they compete.

### Correctness

| Milestone | State |
|---|---|
| C0 | A capture set on the target machine: fixed frames of the intro cutscene and the prologue (`VICTORY_CAPTURE`, `VICTORY_CAPTURE_WORLD`), kept outside the repository, as the reference every later change is compared with | not started (needs the owner's machine) |
| C1 | No dropped draws: every shader program the title asks for is translated (README: under 300 of 24,387 are not) | open |
| C2 | Shader reference tests (`cargo test -p victory-shader`) pass on the target GPU; NVIDIA has not been tested | not started (needs the owner's machine) |
| C3 | Ray-tracing structures built: ray-traced shadows and indirect light | open |
| C4 | Known picture defects gone: the local-light shadow mask, the pink band over nearer bodies, the stand-in head | open |
| C5 | Map completeness at fixed places | not started |
| C6 | Host independence: the wave-size, heap-size, fused multiply-add and float-behaviour dependencies removed (G2 to G5 in [gpu-compatibility.md](gpu-compatibility.md)) | open; G1 (one capability record) started |

Ledger (measured runs only):

| Date | Commit | Scene | Dropped draws a frame | Untranslated programs | Known defects seen | Notes |
|---|---|---|---|---|---|---|
| | | | | | | no measured run yet |

### Stability

| Milestone | State |
|---|---|
| S0 | A run of the intro cutscene and prologue on the target machine without a crash, a device removal or a driver reset | owner reports the title runs; not yet recorded as a measured run |
| S1 | Thirty minutes in the world, nothing new in `crashes.log` | not started |
| S2 | A long traversal with streaming: no missing or corrupted assets (A4) | not started |

Ledger:

| Date | Commit | Scene | Duration | Ended by | `crashes.log` entries | Notes |
|---|---|---|---|---|---|---|
| | | | | | | no measured run yet |

### Memory

| Milestone | State |
|---|---|
| M0 | A memory profile of a world run on the target machine (performance log) | not started (needs the owner's machine) |
| M1 | Where the commit comes from, by owner (title memory, host copies, caches, video memory in system RAM), from the profile and the diagnostics build's memory lines | not started |
| M2 | A5 met | not started |

Ledger (peaks over the run, MB):

| Date | Commit | Scene | Process commit | Working set | System commit / limit | Video used / budget | Video in system RAM | Notes |
|---|---|---|---|---|---|---|---|---|
| | | | | | | | | no measured run yet |

### Performance

Worked only where it does not cost correctness.

| Milestone | State |
|---|---|
| P0 | A measured baseline on the target machine replacing the owner-reported 15 fps | not started (needs the owner's machine) |
| P1 | Where a frame's time goes: `preparing_ms` against `gpu_ms`, then `VICTORY_GPU_TIMING` for the slowest lists and commands | not started |
| P2 | A6 and A7 met | not started |

Ledger (medians over the run's periods after loading unless noted):

| Date | Commit | Scene | fps | p50 ms | p95 ms | p99 ms | Longest ms | Late share | Jitter ms | Preparing ms | GPU ms | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| | | | | | | | | | | | | no measured run yet |

## Measuring

One procedure, so rows are comparable.

1. Build the commit under test: `cargo build --release -p victory`. Note the
   commit (`git rev-parse --short HEAD`).
2. Warm the caches first: `victory translate --cache --pipelines ...` (see
   the README), or the run measures shader and pipeline compilation.
3. Close other applications. Driver frame generation off, driver frame-rate
   limits off. Note the driver version.
4. Set `VICTORY_PERF_LOG` to a file outside the repository and run the
   title. The same scene each time: from the start screen through the intro
   cutscene into the prologue, then stand still for two minutes, then the
   same route.
5. Keep the CSV, the console log and any new `crashes.log` entries. The CSV
   holds measurements only; it can be shared as it is.

The CSV has one row per five-second period:
`seconds, frame, frames, fps, mean_ms, p50_ms, p95_ms, p99_ms, max_ms,
late_share, jitter_ms, draws_per_frame, dropped_per_frame, preparing_ms,
gpu_ms, commit_mb, working_set_mb, system_commit_mb, commit_limit_mb,
video_mb, video_budget_mb, video_shared_mb`
(`crates/victory-gpu/src/pacing.rs`). A ledger row is entered from it with
the commit; a later row that is worse on any track is recorded as a
regression in that track's notes, not left out.

For the breakdown by owner (M1, P1), repeat with the diagnostics build
(`cargo build --profile diag -p victory --features diagnostics`) and
`VICTORY_GPU_TIMING=1`; that build is slower, so its frame times do not
enter the performance ledger.

## Validation tiers

What a session can prove depends on where it runs.

| Tier | Needs | Command | Proves |
|---|---|---|---|
| 1 | any Linux host | `scripts/check-linux.sh tests` | host tests of every crate but `apps/victory` |
| 2 | Linux host with LLVM (clang-cl, llvm-lib, llvm-ml) | `scripts/check-linux.sh windows` | CI's clippy for `x86_64-pc-windows-msvc`, both feature sets; type-checked, not linked |
| 3 | Windows with MSVC build tools | `cargo test --workspace --locked` | everything in CI, including the Windows-only tests (loader, DXIL validator, shader probes on the GPU and WARP) |
| 4 | Windows, the target GPU, the user's game files | `victory translate --cache`, `victory run ... --execute` | the title itself; every ledger row |

A cloud session (Linux container, 4 cores, 15 GB RAM, no GPU, no game data,
no Wine, no MSVC CRT or Windows SDK) reaches tiers 1 and 2. Tiers 3 and 4
are manual tests on the owner's machine or the Windows CI job. Code that
only runs at tier 3 or 4 and was written at tier 2 is listed below as
unverified until a run confirms it.

Written at tier 2, not yet run on Windows:

* the frame-time line and `VICTORY_PERF_LOG` wiring in
  `crates/victory-gpu/src/host.rs` (`Meter::frame`). The statistics and the
  file format themselves are tested at tier 1.
* `Host::caps` (the device and adapter queries, the heap-size probe) and
  `victory gpu-info`. The record, its assessment and the report text are
  tested at tier 1.

## Needed from the owner

Only what cannot be done from a cloud session.

1. The output of `victory gpu-info` on the target machine (no title files
   needed). It replaces expectation with the card's own answers on wave
   lanes, heap size and fused multiply-add, and is the first check of the
   capability code.
2. One run with `VICTORY_PERF_LOG` on the target machine at the current
   commit, following "Measuring", and the CSV. This is P0, M0 and S0 at
   once, and the first check of the new instrumentation.
3. For the 15 fps report: the build or commit, where in the game, the
   console mode, and how the rate was read.
4. `cargo test --workspace --locked` on the target machine (C2): the shader
   probes have only run on an AMD card.

## Session log

### 2026-11-26

* Verified access to the nine repositories and the tools in the cloud
  container. The pinned toolchain installs; nothing can be linked or run
  for Windows there.
* Build: the library crates compile and test off Windows again (446 host
  tests); `scripts/check-linux.sh`; a Linux CI job.
* Objective refined around the target machine: acceptance criteria A1 to
  A7, four milestone tracks with ledgers, correctness before performance,
  no frame generation, empirical profiling.
* Instrumentation: frame-time distribution logged every five seconds in
  every build; `VICTORY_PERF_LOG` (8 new tests at tier 1; the Windows wiring
  is type-checked only).
* Architecture: NVIDIA-first, cross-generation host support by capability
  ([gpu-compatibility.md](gpu-compatibility.md)): rules, the host
  dependencies in the code today (G1 to G11), a hardware status table that
  separates expected, reported, tested, running and confirmed, and a
  roadmap. First step done: one capability record with a stated loss for
  each unmet need (`crates/victory-gpu/src/caps.rs`, 8 tests at tier 1),
  logged by every run and printed by `victory gpu-info` (tier 2 only).
* A subsystem audit with adversarial verification (GPU layer, shader
  translator, runtime and formats, and the two reference-repository
  research tracks) was started; its verified findings are entered here when
  it completes.
