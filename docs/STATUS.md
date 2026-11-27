# Status

One page, kept current by the coordinator at each integration. The record
with the evidence behind every line is `progress.md` (search it; do not
read it whole). Words for the state of work are those of `../CLAUDE.md`.

**One thing has run on hardware** (`hardware-runs.md`): the synthetic
ray-tracing probe, short run under the debug layer, on the owner's RTX
5070 Ti (2026-11-27): native and hybrid paths agree with the CPU reference
on three synthetic classes under the crate's policy. That is synthetic
execution: not validation in the title, and not acceptance under the
proposed rule. Everything else below is tested (host tests), accepted by
Microsoft's validator on Linux, or type-checked for Windows only, as
marked.

## Goal

The title rendered correctly on Windows at a sustained 30 frames a second
within 16 GB of RAM, on NVIDIA first (reference: RTX 5070 Ti, Ryzen 7
5800X) and vendor-independent; ray tracing native on the host wherever
functionally equivalent rendering is demonstrated, hybrid and software
where it is not. Reported before this work: about 15 frames a second and
about 27 GB of "reported memory" (build, scene and counter not yet known).

## On the branch

| Area | State |
|---|---|
| Linux validation of the tree (tiers 1, 1b, 2), asset policy | in use; 1,110 host tests; the validator accepts all 924 containers the shader crate's host tests finish |
| Pixel outputs declared as the program writes them; what the earlier head translated is still translated and noted by reason (`victory translate` tallies the notes); `s_barrier` refused outside compute and mesh | tested; validator-accepted; the console-compiler tests (Windows) have not run with it |
| Frame-time and memory instruments (`VICTORY_PERF_LOG`, gauges per owner, `cargo xtask perf-summary`, `victory gpu-info`) | tested; the Windows queries type-checked only |
| Loop limits: a count per loop, a limit on a lane's loops together (2^22), a stopped lane's end by stage | tested, validator-accepted; one known defect (a `while` that begins a `do-while` body is counted with it) |
| BCPack lane decoder: two changes in a commit of their own (`e57872f`) | tested against a simulation of Microsoft's one-lane module; what it costs is in `bcpack-lane-corrections.md` |
| Title memory a shader reaches by address: misses counted by reason; pages on demand behind `VICTORY_MEMORY_PAGING` | tested (bookkeeping); device side type-checked only |
| Structure views (dimension 11): counted; their address behind `VICTORY_STRUCTURE_ADDRESSES` | tested; type-checked only on the device side |
| Builder build descriptions recorded at its imports, behind `VICTORY_RT_CAPTURE` | tested (parsing, registry); speculative as to the real DLL |
| Dispatch flags counted; counts as threads behind `VICTORY_DISPATCH_THREADS` | tested; validator-accepted guard; speculative as to the title |
| Wave sizes counted; an emulated console wave behind `VICTORY_WAVE_EMULATION`; 64-lane masks as numbers read their high dword (a default change) | tested against a model of the console's wave; validator-accepted |
| `victory-rt`: scenes (27 classes), reference tracer, software model, the older comparison, and the proposed rule as pure functions (k = 8 by measurement) | tested; the probe still judges by the older comparison |
| Traversal-region recognition: loops and regions in the IR, the workload description and class hash, a recogniser, a counts-only census (`victory translate --rt-census`) | tested on hand-assembled synthetic programs; validator-accepted; no title loop has been read; consumed by nothing |
| Ray queries in the emitter; native, hybrid and null prototype shaders | tested; validator-accepted; taken by the NVIDIA driver in the probe's short run |
| Host structure backend (`crates/victory-gpu/src/rt/`), called by no run | tested (pure parts); builds, refits and binding ran on hardware in the probe's short run |
| The synthetic probe `victory rt-probe` and `cargo xtask rt-summary` | tested with a stand-in device; ran on the owner's card (short run, 3 classes) |
| A path per workload and the validation record (`rt/select.rs`, `rt/record.rs`); `auto` selects nothing until the rule is accepted | tested; called by no run |

Every experimental path is off by default. Switches: `README.md`
("Debugging" table).

## In work

| Item | State |
|---|---|
| The probe's next change: result files class by class, a bound on waits, the two debug-layer warnings of the first run fixed, the working set split into private and shareable and broken down by kind | starting |
| The three subsystems: designed, challenged and corrected (`resource-model.md`, `unified-memory.md`, `gpu-scheduling.md`, `predictive-streaming.md`); nothing built | step 0 (instruments, the portable summary, the census, the request log) is next |

## Next, in order (the critical path is the first four)

1. The probe's full run by the owner without the debug layer, once its
   result files are written class by class (the short run under the
   debug layer is done: `hardware-runs.md`).
2. Native variant generator: a recognised region replaced by a ray query,
   checked against the CPU model in the test interpreter and by the
   validator; the software path on the GPU added to the probe.
3. Host structures from captured build descriptions; structure binding
   and currency; variant slots in the shader cache and pipelines.
4. The title comparison on the same rays (owner's machine), then `auto`.
5. Loop-count defect; stops counted and reported by shader and stage.
6. The three cooperating subsystems (`resource-model.md` and the three
   plans): step 0 is instruments and observation only (memory, scheduling
   and streaming columns; a portable summary of a list's steps with the
   proofs of "repeatable" and "independent"; a counts-only census; a
   request log), then the owner's baseline run. Only then: the missed
   page in work that is not repeated (a re-run of the missed piece
   inside its own submission, behind `VICTORY_MEMORY_REPLAY`, for pieces
   proven repeatable; built only if the census shows such pieces miss),
   holder counts and release by fence value, deferred completion,
   decode-ahead and the cache of decoded resources. Each behind its own
   switch, off by default.

## Decided (not reopened; the entry is in `progress.md`)

- Correctness before performance; no frame generation; no effect switched
  off; empirical profiling, not assumed limits.
- NVIDIA first, vendor-independent, capability-detected.
- A loop-limit stop is a correctness failure to investigate, never an
  accepted way to render; counts are per loop with an overall safeguard;
  stopped pixel lanes run on as bounded helpers.
- Ray tracing: a semantic compatibility layer; native traversal and
  triangle intersection first; no bit-identical requirement where
  functional equivalence is demonstrated; hybrid and software retained;
  compatibility resolved at translation or pipeline creation where
  possible; the fastest validated path per workload.
- `bcpack-rs` evaluated part by part; its verified parts vendored for
  tests; the decoder's two corrections adopted in a revertible commit.
- A condition the head translated is not turned into a refusal without
  evidence from the title; it is counted and logged.
- `README.md` is the owner's page: changes there are limited to the rows
  a change must add (a switch, a command).

## Waiting on the owner

Decisions (a default is in force until answered):

| Question | Default |
|---|---|
| Accept the proposed rule for functional equivalence and its constants (`dxr-compatibility.md § The rule`) | `auto` selects nothing under it |
| The overall loop limit of 2^22 back edges per lane | in force |
| A conditional `continue` that falls through counted as its own loop | in force |
| Keep the ten-line dispatch entry test that follows a fork's (origin recorded in `THIRD_PARTY_NOTICES.md`) or have it rewritten | kept |
| Where the title's files come from and how they became readable | title runs and the census wait on it |
| The three subsystems' open points (in `unified-memory.md`, `gpu-scheduling.md` and `predictive-streaming.md`, and listed in their entry in `progress.md`): the replay as first resolution of the missed page and its 250 ms budget; whether completion effects may land from a thread of ours; the disk budget of a decoded-resource cache; host DirectStorage | nothing of it is built before the baseline run |
| Origin of the remaining BCPack tables (`bcpack-tables.bin`, 75,678 bytes) | recorded as unknown |

From the owner's machine, cheapest first:

1. `victory gpu-info`: the card's wave lane range and ray-tracing tier.
   If trace programs are 64-lane and the card runs 32, that work comes
   first on it.
2. `cargo test --workspace --locked`: the Windows-only tests, none of
   which has run.
3. `victory translate` over the shader folder: counts of refusals by
   reason, wave sizes, output notes (no title run needed).
4. One title run with `VICTORY_PERF_LOG` in a ray-traced scene, no switch
   set: frame times, memory by owner, dispatch flags, structure views,
   missed pages, wave refusals. Then one switch at a time.
5. The probe again after its next change (the same short run under the
   debug layer, then the full run without it).
6. `victory translate <shader folder> --rt-census`: counts of traversal
   forms and refusal reasons only; send that file alone.
