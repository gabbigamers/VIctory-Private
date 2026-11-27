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
| "Functionally equivalent rendering": the proposed rule, tolerance, ambiguous rays | `dxr-compatibility.md § The rule` |
| What DXR guarantees and what it leaves to the driver, per guest behaviour | `dxr-compatibility.md § Semantics, from the native path's side`, `§ What the layer may not assume` |
| The probe: paths, scenes, columns, pass rules, what the owner runs | `dxr-validation.md § What is compared`, `§ Scenes`, `§ Measurements`, `§ What the owner runs` |
| The probe as built (`victory rt-probe`, behind `VICTORY_RT_BACKEND`): what it runs, checks and writes, its columns, where it departs from the specification | `dxr-validation.md § The first version (victory rt-probe)`, `§ The columns of rt-probe.csv`, `§ Output`; `memory.md § What allocates` (row R5) |
| The reference crate as built: records, reference, tolerance, policy | `dxr-validation-reference.md § Records`, `§ The reference`, `§ Comparison policy`, `§ Against the proposed rule` |
| Ray queries in the emitter; the three shaders; the refused record | `dxr-shader-emission.md § What was added`, `§ The shaders`, `§ Open points for a Windows run` |
| The builder's build descriptions at its imports | `dxr-build-capture.md § Where a build description is visible`, `§ What a run logs, and what it decides` |

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
| `victory-shader` | GFX10 decode (`gfx10/`), the SSA form (`ir.rs`), structure and resource tracking, the DXIL emitter (`emit/`), the lowering (`emit/lower.rs`, `emit/lower/*.rs`) | `emit/mod.rs` module text; `emit/lower/limit.rs` for loops; `emit/lower/bvh.rs` for the node test; `emit/rayquery.rs` for ray queries; `emit/bitcode.rs` for the checker and the test interpreter |
| `victory-gpu` | host D3D12: lists, draws, descriptors, title-memory copies (`memory.rs`, `paging.rs`, `shadow.rs`, `watch.rs`), pipelines and caches, capability record (`caps.rs`), memory owners (`account.rs`), frame pacing and the performance log (`pacing.rs`), the ray-tracing backend (`rt/`; the path selection in `rt/select.rs` and the validation record in `rt/record.rs` among it), build capture (`rtcapture/`), dispatch counts (`dispatch.rs`), wave census (`waves.rs`) | the file's own module text |
| `victory-rt` | synthetic scenes, the reference tracer, the software traversal's CPU model, comparison, result file | `src/lib.rs`; `dxr-validation-reference.md` |
| `victory-rt-probe` | the synthetic probe: build inputs, ray and hit records, the run and its checks, the result files (`row.rs`, `file.rs`), a stand-in device for host tests (`standin.rs`), the Direct3D 12 side (`hardware.rs`, type-checked for Windows only); its command is `apps/victory/src/rtprobe.rs`, its summary `cargo xtask rt-summary` (`xtask/src/rt_summary.rs`) | `src/lib.rs`; `dxr-validation.md § The first version (victory rt-probe)` |
| `bcpack-reference` | the vendored, attributed part of `bcpack-rs`, tests only | its `README.md` |
| `apps/victory`, `xtask` | the commands (`run`, `translate`, `gpu-info`); `policy`, `perf-summary` | `README.md § Commands` |

## Scripts

| Script | Does |
|---|---|
| `scripts/check-linux.sh tests` / `windows` | tier 1 host tests; tier 2 Windows type-check with clippy |
| `scripts/validate-dxil.sh` | tier 1b: Microsoft's validator over every container the shader crate's host tests finish |
