# Index: what to read for which question

Read `docs/STATUS.md` first. Then use this page to load only the section
a task needs. A section is given as `document § heading`.

## By question

| Question | Read |
|---|---|
| Where does the project stand, what is decided, what is next | `STATUS.md` |
| What the owner asked for, acceptance criteria, order of work | `progress.md § Objective` (lines 31 to 86) |
| What a session can prove (tiers 1, 1b, 2, 3, 4) and the commands | `../CLAUDE.md § Checking a change`; `progress.md § Validation tiers` |
| What happened on a topic, with its evidence | `grep -n "<topic>" docs/progress.md`, then that entry only |
| How the crates fit together | `architecture.md § Crates`, `§ Shaders`, `§ Host GPU`, `§ Memory` |
| Why a design is as it is (reconstructed records) | `decisions/README.md`, then the one record: `0001` address range, `0003` shader translation and its shortcut register, `0005` title memory and BVH, `0007` host GPU |
| Known defects and risks found by the audit | `audit-2026-11-26.md` (A-C correctness, A-S stability, A-M memory, A-P performance, A-B build) |
| Rules for GPU-related code; what a host must provide | `gpu-compatibility.md § Rules`, `§ What the translation needs of a host`, `§ Checklist for a GPU-related change` |
| Which hardware is expected, reported, tested | `gpu-compatibility.md § Hardware status` |
| What has actually run on a device, and what each run shows | `hardware-runs.md` |
| 64-lane programs on 32-lane hosts | `gpu-compatibility.md § The console's wave on any host (G2)` |
| Rules before touching an allocation; the four quantities | `memory.md § Before an allocation is removed or consolidated`, `§ Four quantities` |
| Who allocates what, which gauge counts it | `memory.md § What allocates` (rows T, G, V, C, R), `§ Cases` |
| How to measure memory and frame time | `memory.md § Measuring`; `progress.md § Measuring` |
| What may be committed (files, sizes, origin) | `asset-policy.md`; `../THIRD_PARTY_NOTICES.md` |

## Ray tracing

| Question | Read |
|---|---|
| What the title does, the guest structures, what a hit gives a loop | `ray-tracing-research.md § What the title does`, `§ The guest structures`, `§ A hit` |
| What this fork has and lacks; the forks; what is unknown | `ray-tracing-research.md § This fork today`, `§ The forks`, `§ Unknown, and what would settle it` |
| The semantic layer: one description, three paths | `dxr-architecture.md § The layer: one description, three generators` |
| Guest concept to native equivalent, and where each difference is resolved | `dxr-architecture.md § Guest concepts and their native equivalents` |
| Native path; hybrid and software paths | `dxr-architecture.md § The native path`, `§ The hybrid and the software path` |
| How a path is selected; the validation record; the switch | `dxr-architecture.md § Selection` |
| The validation record and `select()` as built (`crates/victory-gpu/src/rt/record.rs`, `rt/select.rs`): the values of `VICTORY_RT_BACKEND` and `VICTORY_RT_CLASS`, the three gates and each reason a path is not taken, what the build accepts verdicts under (nothing yet, so `auto` selects nothing), the one definition of the probe's version, the record's two kinds of row, its file and bounds, the counters | `dxr-host-backend.md § Selection (by capability and recorded validation, never by vendor or model)`, `§ Status`; `memory.md § What allocates` (row R6), `§ Cases` (R6) |
| Host structures: source, lifetime, memory, ordering | `dxr-architecture.md § Host structures`; `dxr-host-backend.md § Ordering contract`, `§ Lifetime and updates`, `§ Memory` |
| Leaving out the guest's own builds | `dxr-architecture.md § Leaving out the guest's own builds`; `ray-tracing-research.md § Consumers of the guest structures` |
| Where each piece lives; order of work; what evidence changes it | `dxr-architecture.md § Where each piece lives, and the smallest version`, `§ Order of work, and what would change it` |
| Recognising a traversal region, as built on synthetic programs (`crates/victory-shader/src/workload/`, loops and regions in `ir.rs`): the analyses, the description and its class hash, the one accepted shape, each refusal and its program, the walks as the software path, the census (`victory translate --rt-census`); how the class hash reaches the validation record once a generator exists, and that the two differ in size today (64 bits here, 32 bytes there) | `dxr-region-recognition.md § The analyses`, `§ The description`, `§ What is accepted`, `§ Refusals`, `§ The programs as the software path`, `§ The census`, `§ Where the class goes`, `§ Defaults in force` |
| "Functionally equivalent rendering": the proposed rule, tolerance, ambiguous rays; its seven corrections [C1] to [C7] (all proposals, none accepted by the owner) | `dxr-compatibility.md § The rule` |
| What DXR guarantees and what it leaves to the driver, per guest behaviour | `dxr-compatibility.md § Semantics, from the native path's side`, `§ What the layer may not assume` |
| The probe: paths, scenes, columns, pass rules, what the owner runs | `dxr-validation.md § What is compared`, `§ Scenes`, `§ Measurements`, `§ What the owner runs` |
| The probe as built (`victory rt-probe`, behind `VICTORY_RT_BACKEND`): what it runs (the reference crate's first ten classes), checks (by that crate's older comparison, not the rule) and writes, its columns, where it departs from the specification | `dxr-validation.md § The first version (victory rt-probe)`, `§ The columns of rt-probe.csv`, `§ Output`; `memory.md § What allocates` (row R5) |
| The reference crate as built: records (refused records and hits without a face among them), the 27 scene classes (random or directed), the exact reference | `dxr-validation-reference.md § Records`, `§ Scenes`, `§ The reference` |
| The rule as code (`crates/victory-rt/src/rule.rs`, `shade.rs`, `picture.rs`; proposed, not accepted): tolerance and the measured k = 8 with the sample that decides it, pairs and kinds, verdicts and the two floors, levels 2 and 3, each constant halved and doubled; what the CPU model does under it; what is still different | `dxr-validation-reference.md § The rule as built`, `§ Measured on the host, under the rule`, `§ What remains different from the rule and its procedure` |
| The result file of hit records (version 2, the line `rule k 8`; who refuses another k and who reads past it) | `dxr-validation-reference.md § The rule as built` ("The result file"); `dxr-validation.md § Output` |
| The older comparison by the error model (`compare.rs`, kept; the probe judges by it) | `dxr-validation-reference.md § The error model and the older comparison`, `§ Measured on the host, by the older comparison` |
| Ray queries in the emitter; the three shaders; the refused record | `dxr-shader-emission.md § What was added`, `§ The shaders`, `§ Open points for a Windows run` |
| The builder's build descriptions at its imports | `dxr-build-capture.md § Where a build description is visible`, `§ What a run logs, and what it decides` |

## Scheduling, streaming and memory management

Designs for the three subsystems of the owner's direction of 2026-11-27.
Nothing in them is implemented, tested or measured; each statement is
marked as read from the code, reported, inferred, unknown or proposed.

| Question | Read |
|---|---|
| The one shared understanding of a resource: for identity and aliases, ownership, residency and placement, access dependencies, content generations and dirty ranges, and execution lifetime, what in the code already is it, what is extended, what is not to be duplicated | `resource-model.md § The six notions`, then the numbered section for the notion |
| What each of the three reads and writes of the model; the rules all three keep | `resource-model.md § Who reads and writes what`, `§ Rules all three keep` |
| The order of work for the three, and why | `resource-model.md § Order of work` |
| A missed page in work that is not repeated: what happens today, the options the code allows with what each guarantees and cannot, the one proposed first (a re-run inside the submission for pieces proven repeatable, its conditions and its round), its host tests, what the owner's runs must show | `unified-memory.md § Part 1. The missed page in work that is not repeated`, `§ Proposed first: B, for repeatable pieces`, `§ Tested here, and what the owner's run must show` |
| The paging's other limits (2^40, a page held in part, one lookup, giving up by last use) | `unified-memory.md § The paging's other limits` |
| Why separate system and video memory are not the console's one; what may live where by capability; when a copy may be given up or moved (reconstructible, not named, last use complete); pooling | `unified-memory.md § Two memories are not one`, `§ Part 2. What may live where`, `§ Giving up and moving`, `§ Pooling and suballocation` |
| Where each candidate K1 to K11 fits (none approved there) | `unified-memory.md § Pooling and suballocation` (last row) |
| The unit of GPU work today and what the title can observe of it | `gpu-scheduling.md § The unit of work today`, `§ What the title can observe` |
| The dependency record per list and what fills it; what keeps today's order and waits | `gpu-scheduling.md § The dependency record`, `§ The rule: unknown keeps today's order and waits` |
| What may be deferred or overlap, and the proof each needs; what deferred completion needs of the pump (a wake source of ours, the blocking fence wait); which waits can go | `gpu-scheduling.md § What may be deferred or overlap, and by which proof` |
| Host structure builds, refits, retirement and ray-query dispatches in the scheduling model; barriers | `gpu-scheduling.md § Native structures in the model`, `§ Barriers` |
| The scheduling columns and their definitions (CPU submission overhead, GPU idle time, queue wait, barrier cost, frame time) | `gpu-scheduling.md § Measurements` |
| What a streaming request is; the request log and what leaves the machine | `predictive-streaming.md § What a streaming request is today`, `§ Observation first: the request log` |
| What can be predicted from requests alone; that no spatial source was found; the rules for work done ahead | `predictive-streaming.md § What the sequence alone can predict`, `§ Prefetch and prepare-ahead` |
| The persistent cache of converted resources (key, bound, invalidation); host DirectStorage by capability | `predictive-streaming.md § The persistent cache of converted resources`, `§ Host DirectStorage, by capability` |
| The streaming columns and their definitions | `predictive-streaming.md § Measurements` |
| The switches proposed for each subsystem and their fallbacks | `unified-memory.md § Switches, instruments, tests`; `gpu-scheduling.md § Switches, bounds, tests`; `predictive-streaming.md § Order, switches, tests`; release by fence value: `resource-model.md § 6. Execution lifetime` |
| The bound, owner and gauge of each new long-lived structure proposed | `unified-memory.md § Bounds`; `gpu-scheduling.md § Switches, bounds, tests`; `predictive-streaming.md § Bounds` |

## BCPack

| Question | Read |
|---|---|
| The two lane-decoder changes, what they cost, how to undo them | `bcpack-lane-corrections.md § Verdict`, `§ Where new differs from old`, `§ Undoing it` |
| What of `bcpack-rs` is vendored, and why the rest is not | `bcpack-rs-evaluation.md § 2. Components`, `§ 3. Data tables one by one`; `crates/bcpack-reference/README.md` |

## By crate

| Crate | Holds | Start at |
|---|---|---|
| `victory-core`, `victory-pe`, `victory-loader` | shared types; PE parsing; mapping the title and its DLLs, imports | `architecture.md § Import resolution` |
| `victory-hle` | the console's system and D3D12.X entry points; DirectStorage; audio | `src/modules/` (one file per module) |
| `victory-formats` | BCPack, XMA and other data formats | `src/bcpack.rs`, `src/bcpack/` |
| `victory-shader` | GFX10 decode (`gfx10/`), the SSA form (`ir.rs`), structure and resource tracking, the DXIL emitter (`emit/`), the lowering (`emit/lower.rs`, `emit/lower/*.rs`), traversal-region recognition (`workload/`: description, recogniser, census, the synthetic walks; natural loops and regions in `ir.rs`) | `emit/mod.rs` module text; `emit/lower/limit.rs` for loops; `emit/lower/bvh.rs` for the node test; `emit/rayquery.rs` for ray queries; `emit/bitcode.rs` for the checker and the test interpreter; `workload.rs` and `dxr-region-recognition.md` for region recognition |
| `victory-gpu` | host D3D12: lists, draws, descriptors, title-memory copies (`memory.rs`, `paging.rs`, `shadow.rs`, `watch.rs`), pipelines and caches, capability record (`caps.rs`), memory owners (`account.rs`), frame pacing and the performance log (`pacing.rs`), the ray-tracing backend (`rt/`; the path selection in `rt/select.rs` and the validation record in `rt/record.rs` among it), build capture (`rtcapture/`), dispatch counts (`dispatch.rs`), wave census (`waves.rs`) | the file's own module text |
| `victory-rt` | synthetic scenes (27 classes), the exact reference, the software traversal's CPU model, the proposed rule as pure functions (`rule.rs`, level 2 in `shade.rs`, level 3 in `picture.rs`), the older comparison (`compare.rs`), the result file (`results.rs`) | `src/lib.rs`; `dxr-validation-reference.md` |
| `victory-rt-probe` | the synthetic probe: build inputs, ray and hit records, the run and its checks, the result files (`row.rs`, `file.rs`), a stand-in device for host tests (`standin.rs`), the Direct3D 12 side (`hardware.rs`, type-checked for Windows only); its command is `apps/victory/src/rtprobe.rs`, its summary `cargo xtask rt-summary` (`xtask/src/rt_summary.rs`) | `src/lib.rs`; `dxr-validation.md § The first version (victory rt-probe)` |
| `bcpack-reference` | the vendored, attributed part of `bcpack-rs`, tests only | its `README.md` |
| `apps/victory`, `xtask` | the commands (`run`, `translate`, `gpu-info`); `policy`, `perf-summary` | `README.md § Commands` |

## Scripts

| Script | Does |
|---|---|
| `scripts/check-linux.sh tests` / `windows` | tier 1 host tests; tier 2 Windows type-check with clippy |
| `scripts/validate-dxil.sh` | tier 1b: Microsoft's validator over every container the shader crate's host tests finish |
