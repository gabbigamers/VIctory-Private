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
* Every change made for memory follows [memory.md](memory.md): the same
  bytes in two places are not a duplicate until a written case shows it
  (owner, every consumer, whether the title can touch it again, coherency,
  in-flight GPU use, the way back, identical rendering); it comes with
  regression tests and a measured before and after; correctness and
  streaming reliability are never traded for it.
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
| A1 | Graphics render visually correct | Correctness | captured frames reviewed against the console's picture; no draw dropped for want of a host pipeline (`dropped_per_frame` is 0); no translated loop cut short by the iteration limit |
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
| before 2026-11-26 | owner-reported | target machine | not recorded | not recorded | about 15 fps; about 27 GB of "reported memory" (which counter is not recorded: see [memory.md](memory.md)) |

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
| C7 | No loop-limit stop in a run. A translated loop cut short by the iteration limit is a correctness failure to be investigated (why does the loop not end: data with no host copy, a mistranslation, a loop that really is longer), never an accepted way to render. Stops are counted and reported by shader and stage, and a stopped lane ends in a defined, stage-appropriate way with no output left undefined | fix for conditional back edges committed, host-tested, not yet run on Windows. Next, after `lower.rs` is split and as changes of their own (owner's decisions of 2026-11-26, in the session log): a count per loop with an overall safeguard kept, stopped pixel lanes kept on as helpers where that is bounded and free of side effects, a defined end for the other stages, stops counted and reported by shader and stage |
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

Worked from [memory.md](memory.md): commit, working set, video memory and
paging file are four quantities and are recorded apart. Identical data in
the title's memory, host RAM and video memory is not assumed to be
duplication: an allocation is removed or consolidated only through a
written case, in the order remove what is shown redundant, reuse temporary
buffers, evict what can be restored exactly; never by reclaiming memory the
title can still touch. Every change has regression tests and a measured run
before and after; a larger paging file is not a fix.

| Milestone | State |
|---|---|
| M0 | A memory profile of a world run on the target machine (performance log, `cargo xtask perf-summary`): all four quantities, peak and sustained | instrumentation written; not started (needs the owner's machine) |
| M1 | Where it goes, by owner (title memory, host copies, caches, video memory): an allocation inventory from the code, then the profile's figures against it | inventory done and verified ([memory.md](memory.md), "What allocates": 26 entries, none measured); profile not started |
| M1c | The counters the inventory found missing, so each candidate has a measurable before | in: fifteen owners counted at their allocation and release sites and logged ([memory.md](memory.md), "What is counted now"); reviewed; not yet run on Windows |
| M1a | Ownership, residency and dirty-state tracking where a case needs it and it is missing, built and tested as its own change | not started |
| M1b | Allocations removed, reused or made evictable one at a time, each through a case, with tests and a before and an after in memory.md's ledger | not started |
| M2 | A5 met | not started |

The ledger is in [memory.md](memory.md).

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

## Blockers

From the audit of 2026-11-26 ([audit-2026-11-26.md](audit-2026-11-26.md):
98 claims checked against the code by a second reader, none measured).
Ordered by track, then by what can be done next. "Any host" means a cloud
session can do and validate it.

**Gate.** Where the title's files come from and how they became readable
is not recorded anywhere, and the reference repositories call the only
data of this build they know "circulating" and "leaked" (audit, "Gate").
It is the first item under "Needed from the owner". Until it is answered,
sessions work on this repository's own code, synthetic data and checks
that need no title files; the items marked "owner's machine, title" wait.

| # | Blocker | Audit | Next step | Where |
|---|---|---|---|---|
| 1 | A lane stopped by the loop limit keeps running with a truncated loop's values | A-C4 | **fixed and host-tested** (2026-11-26); to run on Windows: the DXIL validator and the hardware and WARP cases its tests now carry. Counters and a defined end per stage follow (C7) | owner's machine, no title |
| 2 | BCPack: a codebook difference against the independent decoder, untested block formats, no end-state check | A-C15 | the entropy stage (all six lane layouts) and BC1 blocks are compared with the vendored part of `bcpack-rs` on synthetic streams; two differences in the lane decoder are reproduced and their corrections held as a separate commit until validated against Microsoft's module. The block stages of BC3, BC4, BC5, BC6H and BC7 are compared with nothing from outside | any host |
| 3 | Lane-dependent compute programs declare the console's wave size; a host with narrower waves drops them, and the trigger is wider than needed | A-C1, A-C3, G2 | refusal reason and tally; narrow the trigger; lane-carrying lowering | any host; counts need the title |
| 4 | A host with a smaller view heap gets shaders that redirect past it | A-C2, G3 | heap layout as a tested function of the host's size | any host; `gpu-info` for the card's limit |
| 5 | Fused multiply-add and other float behaviour follow the host | A-C9, A-C10, G4, G5 | probe run on the target card; exact lowering where it differs | owner's machine, no title; then any host |
| 6 | Geometry-stage programs and under 100 others are untranslated; their draws are dropped | A-C5, A-C6, A-C14 | per-reason tally | owner's machine, title |
| 7 | Ray-tracing structures never appear; three candidate causes in the code, none shown | A-C7 | log which occurs | owner's machine, title |
| 8 | No test runs translated shader code without a Direct3D 12 device | A-B3 | an interpreter for the compute-probe subset, with the existing CPU models as oracle | any host |
| 9 | No commit check at start; the title's own 30 s rule ends a stalled run | A-S1, A-S2 | start-up check with a clear message | any host |
| 10 | Pipeline library held and rewritten whole | A-S3, G8 | measure, then map the file | owner's machine; then any host |
| 11 | Where memory and a frame's time go | A-M1 to A-M4, A-P1 to A-P8 | M0, P0, then M1 and P1 | owner's machine, title |
| 12 | The BCPack tables' origin; notices; identifiers in source | A-B6, A-B7, A-B8 | the derivable section now comes from attributed source and `THIRD_PARTY_NOTICES.md` exists; still the owner's: where the remaining 75,678 bytes of tables came from, and identifiers in source | owner |

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

The CSV has one row per five-second period: the frame times, draws and
dropped draws, the preparing and GPU split, and memory as four separate
things (process commit and working set with their peaks, page faults a
second, the machine's commit, RAM, free RAM and paging files, video memory
on the card and shared, and our own count of host textures and upload
memory). The columns are in `crates/victory-gpu/src/pacing.rs`.

`cargo xtask perf-summary run.csv` turns it into ledger figures (sustained
values as medians over the world's periods, peaks over the whole run), and
`--against before.csv` sets two runs side by side and names what got worse.
It runs anywhere. A ledger row is entered from it with the commit; a later
row that is worse on any track is recorded as a regression in that track's
notes, not left out.

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
* the memory queries behind the performance log's memory columns (process
  counters, the machine's memory, paging files) in `host.rs`. The row
  format and `cargo xtask perf-summary` are tested at tier 1.
* the counting of memory by owner: every add and release in `xmem.rs`,
  `dstorage.rs`, `shadow.rs`, `watch.rs`, `texture.rs`, `host.rs`,
  `descriptor/host.rs`, `memory.rs` and `pipeline/library.rs`, and the
  miscount warning. The gauges, the snapshot pool's bookkeeping and the
  log columns are tested at tier 1.
* `Host::caps` (the device and adapter queries, the heap-size probe) and
  `victory gpu-info`. The record, its assessment and the report text are
  tested at tier 1.

## Needed from the owner

Only what cannot be done from a cloud session.

1. Where the title's files used for runs come from and how they became
   readable (see "Blockers", gate). This project's rules are lawfully
   obtained data and no bypass of access controls, and no repository
   records the answer. Items 3 and 4 below wait on it; 2 and 5 do not.
2. The output of `victory gpu-info` on the target machine (no title files
   needed). It replaces expectation with the card's own answers on wave
   lanes, heap size and fused multiply-add, and is the first check of the
   capability code.
3. One run with `VICTORY_PERF_LOG` on the target machine at the current
   commit, following "Measuring", and the CSV. This is P0, M0 and S0 at
   once, and the first check of the new instrumentation. It is also what
   turns the 27 GB report into four separate figures.
4. For the 15 fps and 27 GB report: the build or commit, where in the
   game, the console mode, how the rate was read, and which tool and
   column showed the 27 GB (Task Manager's "Memory" or "Commit size",
   Resource Monitor, an overlay).
5. `cargo test --workspace --locked` on the target machine (C2): the shader
   probes have no recorded full run on an NVIDIA card.
6. The design decision records (0001, 0003, 0005, 0007) and the named
   measurement runs the code cites, if they exist outside the repository.
7. Where the tables left in `bcpack-tables.bin` came from (index rank
   tables, BC7 control words, dictionaries; audit, A-B6). The entropy
   models are no longer in the file: they come from attributed
   MIT-licensed source. The file's history does not say: it arrives whole
   in the decoder's first commit with no source named.

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
* Audit completed ([audit-2026-11-26.md](audit-2026-11-26.md)): GPU layer,
  shader translator, runtime and formats, and the two reference-repository
  research tracks, each re-read by a verifier. 72 claims confirmed, 25
  corrected, 1 unverifiable. "Blockers" above is its prioritised result;
  the reuse table and licence findings are in the audit file. It raised
  the provenance of the title's files as an open question for the owner.
* Memory investigation started on the owner's report of about 27 GB at
  about 15 fps ([memory.md](memory.md)). The performance log now records
  commit, working set, paging file and video memory apart, with peaks and
  the fault rate; `cargo xtask perf-summary` summarises a run and compares
  two (15 tests at tier 1; the Windows queries are type-checked only). An
  allocation inventory by owner and kind, verified against the code, is in
  progress.
* Memory rules extended on the owner's instruction: no copy is treated as
  redundant on the ground of identical data; the seven questions a case
  must answer, the order of preferred means, the tracking a case may need,
  and what every memory change comes with are in [memory.md](memory.md)
  and in the standing rules above.
* Owner's decisions on the loop limit: stops are counted and logged by
  shader and stage, aggregated so normal rendering pays nothing for it; a
  stopped lane must not leave outputs undefined, so each stage gets a
  defined termination; any stop is a correctness failure to investigate
  (C7); the regression tests that run full-length loops stay in the default
  test run. `lower.rs` is to be split into modules as a change of its own
  that alters no behaviour, apart from functional changes.
* Memory inventory completed and verified (67 claims: 40 confirmed, 27
  corrected, none refuted): [memory.md](memory.md) now holds the breakdown
  by owner and kind, what each log counter does and does not count, the
  counters still missing, and eleven candidates, none approved. The host
  copies of title buffers, depth twins and the title's own memory are
  recorded as needed today, not as waste. One figure of the run summary
  that subtracted two counters measuring different things was removed.
* Owner's decision on the BCPack decoder: VIctory's decoder is kept. The
  parts of `bcpack-rs` whose origin is independently verified (its entropy
  layer, ported from Microsoft's MIT-licensed hardware description, the
  model tables generated from it, and its synthetic encoder and vectors)
  are used for testing. Its block rebuild is not independent evidence (it
  was checked against VIctory and shares code shape with it) and its data
  files stay out. The two differences in the entropy layer that were
  reproduced by execution are to be investigated against Microsoft's
  source, which is the authority for that layer: a lane that has delivered
  its last symbol goes on asking for bytes, and a byte is dropped at the
  end of a fragment that was not flushed. Microsoft's source is cloned
  read-only at `/home/user/microsoft/silicon-rans` (commit `2f6827d`).
* Memory counters (M1c): the accounting module and its log columns are in
  (`crates/victory-gpu/src/account.rs`); putting the counting in at each
  allocation and release is in review.
* Committed after review: the component-by-component evaluation of
  `bcpack-rs` ([bcpack-rs-evaluation.md](bcpack-rs-evaluation.md)) and the
  reconstructed decision records ([decisions/](decisions/README.md)).
  VIctory's full history (692 commits) was searched for both: no decision
  record was ever a file; and VIctory's decoder with the code the two
  projects share was committed about four hours before `bcpack-rs`'s
  rebuild files, by author timestamp. The origin of VIctory's own table
  file is still not recorded anywhere.
* Owner's decision on the two lane-decoder corrections: adopt both if
  validation shows they handle valid synthetic streams correctly, as a
  separate, reversible commit with their regression tests enabled, and
  with accuracy, throughput, memory and error handling compared against
  the existing code; agreement between related implementations is not
  proof. Validation uses Microsoft's published decoder module simulated
  unmodified (Verilator) as the oracle for a single lane; how several
  lanes share one stream is not published and cannot be checked that way.
* Loop limit (A-C4) fixed, reviewed and committed. A lane stopped by the
  limit on a conditional back edge now ends in the stop block (a pixel
  discards, then returns) instead of leaving through the loop's exit with
  a truncated loop's values. The loop's own shape is unchanged: the test
  sits on the exit path. A second defect went with it: where both sides of
  a latch's branch are back edges, one of the two loops had no limit at
  all. 15 host tests (structure checked and one lane run on the host,
  including loops of exactly 2^20 iterations), a structural checker and a
  one-lane interpreter for written functions, used by the existing host
  lowering helpers too. The reviewer's own fuzz (3,000 programs) and
  separate interpreter agreed. Not run on Windows: DXIL validity and any
  GPU. Open for the owner under C7: a stopped pixel returns instead of
  running on as a helper lane, so neighbours' derivatives may be
  undefined; loops that share a header block share one count, so a genuine
  nest of inner times outer over 2^20 iterations is stopped. `lower.rs` is
  745 bytes under the asset-policy size limit: its split comes before the
  next functional change to it.
* Memory counters (M1c) in: each of the fifteen owners of
  `victory_gpu::account` is now added to where its memory is allocated and
  taken from where it is released, mostly by a count held as a field of
  what owns the memory. Written by two engineers, each checked by a
  reviewer who traced every allocation and release path (13 findings, all
  dealt with). Counting changes nothing else, with these disclosed
  exceptions: one small allocation per pool the title allocates (a bitmap
  of committed pages), one gauge update on the write-fault path, and a
  one-time redo of the pipeline pre-build's list because the library file's
  text is part of its tag. A test-only global allocator a corrector had
  added to the runtime crate's Windows tests was taken out before commit:
  it could not be run here and would have sat under every test of that
  crate. None of the counting has run.
* Owner's decisions on what a loop-limit stop does, to be made after the
  split of `lower.rs` and apart from it:
  - **Helper lanes.** A stopped pixel lane stays on as a helper where that
    keeps derivatives right for the other pixels of its quad, as the
    title's own discard does, provided its further execution is bounded
    and it can have no effect of its own: no store, no export, no write
    through a view, no atomic, and no loop it could spin in.
  - **A count per loop.** Each loop has its own iteration count, reset on
    entry to that loop, in place of one count per header block that nested
    loops sharing a header also share. A genuine nest is then not stopped
    because inner times outer passes the limit.
  - **An overall safeguard stays.** Per-loop counts alone would let a nest
    run for the product of its limits, so a bound on a lane's total work
    is kept against pathological shaders and against hanging the GPU.
  - **Tests.** Regression tests for nested loops, divergent control flow
    (lanes leaving a loop at different times and by different exits),
    helper-lane behaviour and termination.
  - **A stop is a defect.** A loop-limit stop that was not expected is a
    correctness defect to be reported and investigated, never counted as
    the shader having run (C7).

### 2026-11-27

* BCPack, in the tree (reviewed three times: the change, its finalising,
  and the corrections to each):
  * The entropy models are attributed source,
    `crates/victory-formats/src/bcpack/models.rs`: the frequency table of
    Microsoft's `mdu_minirans.v` (MIT), taken through `bcpack-rs`, with
    both notices. They are no longer the first 2,304 bytes of
    `bcpack-tables.bin`. The file's other 75,678 bytes are byte for byte
    what they were and the decoder reads them in one place
    (`Constants::shipped`). The asset policy's exception and the policy
    check now say which part has a recorded origin and which has not.
    `THIRD_PARTY_NOTICES.md` is new.
  * Part of `bcpack-rs` is vendored as `crates/bcpack-reference`, a
    dev-dependency of `victory-formats` and of nothing else: its entropy
    layer, model tables, format helpers, synthetic encoder and BC1
    rebuild, with no data file. Its BC7 and BC3/4/5 rebuild files are not
    taken: they share code with VIctory's decoder, which by the history
    had it first ([bcpack-rs-evaluation.md](bcpack-rs-evaluation.md)).
  * Differential tests on synthetic streams
    (`crates/victory-formats/src/bcpack/differential.rs`, tier 1): the
    entropy stage of all six lane layouts, and the block stage of BC1.
    Agreement there is with another project's reading of the format, not
    proof of either.
  * The runtime lane decoder is unchanged by this commit: shown by text
    (its region differs from the previous commit in a type's name) and by
    execution (old and new builds over the same streams: results, error
    text, buffers and every byte request equal, twice, by two people's
    separate harnesses).
* BCPack, held back: two corrections to `Lane::step` (a per-lane symbol
  count, so a lane that has delivered its symbols takes no more input; and
  the byte kept when a fragment without the reset bit ends on a read).
  The owner's decision of 2026-11-26 is to adopt them if validation shows
  they handle valid synthetic streams correctly, as a commit of their own
  that can be reverted. Until that commit: nine tests that need them are
  ignored with a reason that points here, and two tests named
  `known_difference_...` hold what the decoder and the vendored port each
  do today with one small stream per difference. What a reviewer measured
  for that decision: on streams the port accepts, the corrected decoder
  never refuses or changes one the present decoder accepts, except where
  the present decoder's symbols already differ from the port's; on streams
  in the present decoder's own byte order that the port refuses, it
  decodes other records for 2,320 and refuses 44. Which byte order real
  streams have is the unpublished part (the feeder), so that cannot be
  settled here.

