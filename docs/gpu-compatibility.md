# Host GPU compatibility

An architectural requirement, not a tuning goal: **the compatibility layer
is NVIDIA-first and works across GPU generations.** The owner's RTX 5070 Ti
is the machine development and performance are validated on. It is not what
the code is written for. Turing, Ampere, Ada Lovelace, Blackwell and what
follows them are supported wherever the device has the capabilities listed
below, and AMD support is kept wherever keeping it does not cost the
NVIDIA objective.

Every GPU-related change is decided against this document.

## Guest and host

Two GPUs meet in this code and only one of them is fixed.

* The **guest** is the console's GPU. Its instruction set, float mode, wave
  sizes, descriptor layouts and texture layouts are facts of the title's
  data, and the translation reproduces them exactly. "AMD" and "RDNA 2" in
  `victory-shader` and in what the title is told about its adapter
  (`crates/victory-gpu/src/dxgi.rs`) are the guest. That is not a
  dependency to remove.
* The **host** is whatever Direct3D 12 device the user has. Nothing about
  it may be assumed. A result that is right only because the host happens
  to behave like the guest (it fuses multiply-adds, its waves are as wide,
  its heaps as large) is a host dependency, and a defect.

## Rules

1. **Capabilities, not names.** Behaviour follows what the device reports
   or what a measurement on it shows. No code branches on a vendor id, a
   device id or a model name. Those are recorded so a report says what it
   was taken on (`crates/victory-gpu/src/caps.rs`).
2. **Ask, do not assume.** Features, wave lane counts, heap limits and
   memory are asked of the device at run time. A limit that cannot be
   asked (the largest descriptor heap) is found by trying, never written
   down as a constant for one vendor.
3. **Guest semantics do not depend on the host.** The translation computes
   what the console computes on every host. Where a host operation's
   result is the host's choice (fusing, denormals, NaN conversion, wave
   width), the translation spells the guest's result out itself, or uses
   the host operation only where a measurement on that device shows it
   agrees.
4. **Standard Direct3D 12 first.** A vendor's extension or library is used
   only as an optional path: behind a capability check, with a standard
   path that gives the same picture, and with a measured gain recorded in
   [progress.md](progress.md). A vendor path that changes the picture is
   not an optimisation.
5. **Budgets from the device.** Video memory, descriptor counts, cache
   sizes and worker counts scale with what the device and the machine
   report now, not with the machine the code was first run on.
6. **A missing capability is a stated loss.** Where a device lacks
   something, the run says so once, by name, with what is left out, and
   goes on if it can. It does not fault, and it does not silently draw
   something else.
7. **Driver defects are named.** A workaround for a driver defect is the
   one place a vendor may be named: in a comment that says what the defect
   is and how it was seen. The preferred workaround is a form every driver
   runs correctly, emitted for all of them; a workaround applied only to
   some drivers needs a probe or a driver-version range, and a test.
8. **Derived data belongs to the device that made it.** Shader caches are
   keyed by what the translation was asked for; pipeline libraries by GPU
   and driver. Changing the GPU or the driver rebuilds them and changes
   nothing else. No user step and no code change follows from an upgrade.
9. **Correctness is not traded.** None of the above is a licence to lower
   what is drawn to make a device fit. See "Order of work" in
   [progress.md](progress.md).

## What the translation needs of a host

`victory_gpu::caps` holds the device's answers in one record and
`caps::assess` compares them with the list below. A run logs the result
when the device is made; `victory gpu-info` prints it without a title.

| Need | Without it | Standard feature |
|---|---|---|
| Shader model 6.6 | the world does not run | `D3D12_FEATURE_SHADER_MODEL` |
| Mesh shaders | the world does not run | `D3D12_OPTIONS7` |
| Resource binding tier 3 | the world does not run | `D3D12_OPTIONS` |
| 32-lane waves | programs that declare them are left out | `D3D12_OPTIONS1` |
| 64-lane waves | programs that declare them are left out (G2) | `D3D12_OPTIONS1` |
| A view heap of 2^21 slots | slots past the host's limit are not there (G3) | found by trying |
| Fused multiply-add | the console's fused results can differ in kind (G4) | measured by running one |
| 64-bit atomics on heap resources | translated 64-bit image atomics | `D3D12_OPTIONS9`, `D3D12_OPTIONS11` |
| Depth bounds test | those pipelines draw without the test | `D3D12_OPTIONS2` |
| Ray tracing tier 1.1 | no host ray-tracing structures (not built on any host yet) | `D3D12_OPTIONS5` |

The middle rows are host dependencies the code has today: the title runs
on such a device with a loss. Each has a roadmap item that removes the
dependency rather than the device.

## Host dependencies in the code today

Read in the code on 2026-11-26. "Open" means the rule above is not met yet.

| # | Dependency | Where | State |
|---|---|---|---|
| G1 | Capability checks are scattered single queries; the wave lane count, binding tier and ray-tracing tier were never asked outside tests | `host.rs` (`mesh_shaders`, `heap_atomics64`, depth bounds), `sampler.rs`, `emit/host.rs` | **Started**: one record, assessed and logged (`caps.rs`, `Host::caps`). The older queries still stand beside it and move into it as they are touched |
| G2 | A compute program that depends on the lane layout declares the console's wave size (32 or 64) and is lowered so that host lane *n* is console lane *n*. A host whose waves cannot be that wide makes no pipeline for it | `emit/lower.rs` (where `wave_size` is set), `emit/shader.rs` (the `WaveSize` property) | **Open.** The lowering must carry the guest's lanes itself where the host's waves do not match (rule 3), chosen from the device's lane range |
| G3 | Translated shaders clamp heap indices below 2^21 and read a null buffer view two slots below that; both numbers are compiled in. A host that gives a smaller heap gets a fallback the code itself marks as a shortcut, with the 1,000,000-slot size written as a constant | `emit/shader.rs` (`HEAP_VIEWS`, `NULL_BUFFER_VIEW`), `descriptor/host.rs` (`HostHeap::new`, `PORTABLE_VIEWS`) | **Open.** The heap layout becomes a function of the size the device gives, and the shaders' bound and null slot follow it |
| G4 | The console's fused multiply-adds are emitted as the host's multiply-add, which the driver may or may not fuse. Measured at device creation and warned about; a test marked as a known defect documents the difference | `emit/lower.rs`, `emit/host.rs` (`fuses_fmad`), `emit/semantics_tests.rs` | **Open.** An exact lowering where the device does not fuse |
| G5 | Float behaviour the lowering leaves to the host (denormals in plain arithmetic, float-to-integer conversion of NaN and out-of-range values, integer data through float-typed views) is pinned only by the shader probes, and their recorded results are from AMD cards and WARP | `emit/lower.rs`, `emit/semantics_tests.rs`, `emit/valu_tests.rs` | **Open.** The probes run on NVIDIA (C2 in progress.md); every probe that differs gets an explicit lowering |
| G6 | The device is made on the system's default adapter | `host.rs` (`create`) | **Open.** Choose the adapter deliberately (the high-performance one, a user override), from what adapters report, never by vendor |
| G7 | Host textures are given up against the budget the system reports for the card | `resource.rs` (`BUDGET_HIGH`, `BUDGET_LOW`, `evict`) | Met: follows the device |
| G8 | The pipeline library is read whole into memory; a code comment gives about 1.7 GB for it on NVIDIA against a small file on AMD | `pipeline/library.rs` | **Open** for the 16 GB memory goal: measure first (M1), then size it to the machine |
| G9 | Driver defects worked around in the lowering are handled by emitting a form both vendors' drivers run correctly, for every host | `emit/bitcode.rs`, `emit/lower.rs`, `emit/shader.rs` | Met: rule 7's preferred form. Keep it that way |
| G10 | Shader cache keyed by translation inputs; pipeline library per GPU and driver | `shader_cache.rs`, `pipeline/library.rs` | Met: rule 8. Translation options added for G2 to G4 enter the cache key |
| G11 | Vendor extensions | none used | Met. Any future one follows rule 4 |

Not a dependency: the upscaler shim serves the title's own FidelityFX
calls with AMD's PC library where that offers an upscaler for the device,
and with the title's own otherwise; it refuses frame generation.

## Hardware status

Five words, used only as defined. A device moves down the list on recorded
evidence, never on expectation.

* **Expected**: the vendor's published feature support suggests the
  required needs are met. Nothing has been run.
* **Reported**: a `victory gpu-info` report from that device is recorded
  here.
* **Tested**: the test suite (`cargo test --workspace --locked`) has been
  run on it and the results recorded, including what failed.
* **Runs the title**: a run is in a ledger in [progress.md](progress.md),
  with the defects seen.
* **Confirmed working**: a recorded run meets acceptance criteria A1 to A5
  (correct, stable, within memory). Speed is recorded beside it, not
  folded into the word.

| Host | Status | Evidence | Known losses |
|---|---|---|---|
| NVIDIA Blackwell, RTX 5070 Ti 16 GB (primary target) | Expected; owner reports the title runs at about 15 fps | owner's report, build and scene not recorded | G2 and G3 expected by analogy with the report below; not asked on this card yet |
| NVIDIA Blackwell, other | Expected | a test comment names an RTX 5090D for one pipeline check | as above |
| NVIDIA Ada Lovelace | Expected | a probe recorded in the `xbox-hle-lab` reference for an RTX 4070 Ti SUPER: waves of 32 to 32 lanes, binding tier 3, mesh shaders, ray tracing; a code comment records an out-of-memory abort on an RTX 4050 Laptop while loading the pipeline library | G2 (64-lane waves), G3, G8 |
| NVIDIA Ampere | Expected | none | unknown |
| NVIDIA Turing | Expected | none | unknown |
| NVIDIA, future | Expected where the needs above are reported | none | none by design: nothing is keyed to a model |
| AMD RDNA 2, RX 6700 XT 12 GB | Runs the title (project-reported, about 1 fps); shader probes recorded | README, test comments | see README |
| AMD RDNA 4, RX 9070 XT | Runs the title with device removals in earlier builds | a code comment in `descriptor/host.rs` | not recorded since |
| Intel Arc | Expected | none | unknown |
| WARP (software) | Tested for the shader probes; too slow for the world | README, CI | does not fuse multiply-adds |

No host is confirmed working.

The "Expected" rows for whole generations are placeholders until a
`gpu-info` report replaces them: this table does not assert what a
generation supports.

### Recording a device

On the device, with nothing of a title's needed:

```
victory gpu-info
cargo test --workspace --locked
```

The first prints the adapter, its driver version, its memory and each need
as met, degraded, missing or unknown. Its text and the test summary go into
the table above with the commit and date. A title run adds a ledger row in
progress.md.

## Roadmap

In this order where they compete; correctness items first.

| Item | Track | Next step | Where it can be done |
|---|---|---|---|
| G1 | architecture | `gpu-info` report from the RTX 5070 Ti; then move the remaining single queries into the record | owner's machine; then any |
| G5 | correctness | run the shader probes on the RTX 5070 Ti and record every difference | owner's machine |
| G2 | correctness | count the programs that declare each wave size and why (a tally in `victory translate`); then the lane-carrying lowering for hosts whose waves do not match | tally: any host; counts: owner's machine |
| G3 | correctness | the heap layout as a tested function of the device's size; then the shaders' bound and null slot from it | logic: any host; confirmation: owner's machine |
| G4 | correctness | an exact fused multiply-add for devices that do not fuse, checked against a CPU model | any host; cost measured on the owner's machine |
| G6 | stability | adapter choice by report, with an override | owner's machine to confirm |
| G8 | memory | measure the library's size and load on NVIDIA (M1); then map it instead of reading it whole | owner's machine; then any |

## Checklist for a GPU-related change

* Does it read a vendor id, a device id or a model name to decide
  anything? Then it is wrong (rule 1), unless it is a named driver defect
  (rule 7).
* Does it add a constant that is really one device's limit? Ask the device
  or find it by trying (rule 2).
* Does a shader result now depend on what the host chooses? Spell it out or
  measure it (rule 3), and add the probe.
* Does it need a capability? Add it to `caps.rs` with its loss stated, and
  to the table above (rule 6).
* Does it add a translation option? It enters the shader cache key
  (rule 8).
* Does it change the picture on any device? Then it is a correctness
  change and is compared against captures (rule 9).
* Which hosts was it run on? Say so in the commit; the status table moves
  only on that.
