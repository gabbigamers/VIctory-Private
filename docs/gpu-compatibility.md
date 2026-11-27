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
   *For ray tracing, the owner's decision of 2026-11-27:* where a
   recognised traversal is answered by the host's own ray-tracing
   hardware, "agrees" does not mean bit for bit. It means functionally
   equivalent rendering, demonstrated by a recorded test on that device
   and driver before the path is selected. What "demonstrated" means is
   written in [dxr-compatibility.md](dxr-compatibility.md) as a
   proposal, and binds once the owner has accepted it. The hybrid is
   answered by host hardware too and is selected only after its own
   recorded test; whether its bar is that rule or equality with the
   software path is the owner's to say. Without a passing test the
   traversal runs the software path, which itself, like everything
   outside ray tracing, stays under the rule as written.
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
| 64-lane waves | programs that declare them are left out (G2), or carry the wave themselves where asked to ("The console's wave on any host" below) | `D3D12_OPTIONS1` |
| A view heap of 2^21 slots | slots past the host's limit are not there (G3) | found by trying |
| Fused multiply-add | the console's fused results can differ in kind (G4) | measured by running one |
| 64-bit atomics on heap resources | translated 64-bit image atomics | `D3D12_OPTIONS9`, `D3D12_OPTIONS11` |
| Depth bounds test | those pipelines draw without the test | `D3D12_OPTIONS2` |
| Ray tracing tier 1.1 | no native or hybrid ray-tracing path ([dxr-host-backend.md](dxr-host-backend.md); experimental, off by default, not run on any host yet): every workload stays on the software traversal, which does not need it | `D3D12_OPTIONS5` |

The middle rows are host dependencies the code has today: the title runs
on such a device with a loss. Each has a roadmap item that removes the
dependency rather than the device.

## Host dependencies in the code today

Read in the code on 2026-11-26. "Open" means the rule above is not met yet.

| # | Dependency | Where | State |
|---|---|---|---|
| G1 | Capability checks are scattered single queries; the wave lane count, binding tier and ray-tracing tier were never asked outside tests | `host.rs` (`mesh_shaders`, `heap_atomics64`, depth bounds), `sampler.rs`, `emit/host.rs` | **Started**: one record, assessed and logged (`caps.rs`, `Host::caps`). The older queries still stand beside it and move into it as they are touched |
| G2 | A compute program that depends on the lane layout declares the console's wave size (32 or 64) and is lowered so that host lane *n* is console lane *n*. A host whose waves cannot be that wide makes no pipeline for it | `emit/lower.rs` (where `wave_size` is set), `emit/lower/crosslane.rs`, `emit/shader.rs` (the `WaveSize` property) | **Started, behind a switch.** Counted per program and reason in every build. A second lowering carries the console's wave in the thread group itself, chosen from the device's lane range (`VICTORY_WAVE_EMULATION`, off by default): covered by host tests against a model of the console's wave, type-checked for Windows, not yet run on a GPU. See "The console's wave on any host" |
| G3 | Translated shaders clamp heap indices below 2^21 and read a null buffer view two slots below that; both numbers are compiled in. A host that gives a smaller heap gets a fallback the code itself marks as a shortcut, with the 1,000,000-slot size written as a constant | `emit/shader.rs` (`HEAP_VIEWS`, `NULL_BUFFER_VIEW`), `descriptor/host.rs` (`HostHeap::new`, `PORTABLE_VIEWS`) | **Open.** The heap layout becomes a function of the size the device gives, and the shaders' bound and null slot follow it |
| G4 | The console's fused multiply-adds are emitted as the host's multiply-add, which the driver may or may not fuse. Measured at device creation and warned about; a test marked as a known defect documents the difference | `emit/lower/arith.rs` (`fmad`), `emit/host.rs` (`fuses_fmad`), `emit/semantics_tests.rs` | **Open.** An exact lowering where the device does not fuse |
| G5 | Float behaviour the lowering leaves to the host (denormals in plain arithmetic, float-to-integer conversion of NaN and out-of-range values, integer data through float-typed views) is pinned only by the shader probes; code comments record single NVIDIA observations, but no full probe run on an NVIDIA card is recorded | `emit/lower/` (`arith.rs`, `valu.rs`, `buffer.rs`), `emit/semantics_tests.rs`, `emit/valu_tests.rs` | **Open.** The probes run on NVIDIA (C2 in progress.md); every probe that differs gets an explicit lowering |
| G6 | The device is made on the system's default adapter | `host.rs` (`create`) | **Open.** Choose the adapter deliberately (the high-performance one, a user override), from what adapters report, never by vendor |
| G7 | Host textures are given up against the budget the system reports for the card | `resource.rs` (`BUDGET_HIGH`, `BUDGET_LOW`, `evict`) | Met: follows the device |
| G8 | The pipeline library is read whole into memory; a code comment gives about 1.7 GB for it on NVIDIA against a small file on AMD | `pipeline/library.rs` | **Open** for the 16 GB memory goal: measure first (M1), then size it to the machine |
| G9 | Driver defects worked around in the lowering are handled by emitting a form both vendors' drivers run correctly, for every host | `emit/bitcode.rs`, `emit/lower/`, `emit/shader.rs` | Met: rule 7's preferred form. Keep it that way |
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

## The console's wave on any host (G2)

The console runs a compute wave of 64 lanes (some programs: 32). Its
machine code has one program counter for the wave: branches test the whole
wave (EXEC or VCC empty, a scalar flag), EXEC says which lanes an
instruction touches, and some instructions move data between lanes. A host
wave may have another size, and nothing in Direct3D says which threads of a
group a host wave holds.

### What depends on the wave

Read in `emit/lower.rs` (where a program's reasons are collected,
`wave_need`), `emit/lower/crosslane.rs`, `emit/lower/masks.rs`,
`emit/wave.rs` and `structure.rs`.

| Class | Operations | Lowered today as |
|---|---|---|
| 1. Independent of the wave | everything a lane does with its own values; a branch on a wave-wide test of a mask, taken as the lane's own bit (`structure.rs`: compiled code only skips what lanes outside the mask would not change); masks joined at a block; a waterfall loop's first-lane read of a descriptor (each lane's own); a `v_readlane` that gives back the scalar a `v_writelane` parked in that lane (it is that scalar, in every stage; the write that parked it is class 2a in a compute program); in vertex, pixel and mesh programs, a mask's count, first lane, count below a lane and equality over the *host's* wave | per lane: no wave operation that depends on the size, no wave size |
| 2. Exact on any wave with a transformation that needs no exchange | **(a) the lane's own number only:** a mask made from a number or a constant lane set (the lane tests its own bit), `v_writelane` (a scalar parked in a lane included: in a compute program the write is lowered by the lane's number in the host's wave, so a program that only parks and reads back scalars still declares the console's wave size; such programs are counted apart, reasons "lane write" and "scalar read back from its lane" with no "lane read"), `v_mbcnt` of such a mask. Transformation: the lane number from the thread's place in its group instead of from the host wave. **(b) confined to a part of the wave:** DPP (rows of 16 lanes), `ds_swizzle` and `ds_bpermute` (halves of 32), `v_permlanex16` (a half). Transformation: host waves of a size the part divides, each host wave a run of console lanes; a 64-lane program then runs in 32-lane waves | host waves of the console's size (class 3's lowering): neither transformation is implemented as its own form. For a group of one wave the emulated wave covers both, at the price of exchanges the operations themselves would not need |
| 3. Exact only with the whole console wave | a mask read as a number (`s_bcnt1`, `s_ff1`, scalar arithmetic and compares, stored or pushed); `v_readlane` and `v_readfirstlane` across halves; `v_mbcnt` of a ballot; the steps of a whole-wave section where its value is used as the wave's; shared memory the console compiler orders without barriers in a group of one wave | **host waves** of exactly the console's size (`WaveSize` declared, host lane *n* is console lane *n*): no pipeline on a host without that size. Or the **emulated wave** (below), for a thread group of exactly one console wave |
| 4. Not known | whether the title's wave reductions and prefixes are only used in ways that do not care how threads are divided into waves (a count that reserves slots and each lane's place in them); such a program would be class 1 over host waves of any size. How many programs are in thread groups of several console waves, which the emulated wave does not take | host waves, as class 3. The counts a run reports ("What a run reports") size both questions |

Class 3's host-wave lowering also assumes that a host wave holds a run of
consecutive threads of the group in order, which Direct3D does not promise
(`emit/lower/stage.rs`, `thread_id`). The emulated wave does not.

### The emulated wave

`structure::waves` gives the program as the console's wave runs it: every
thread runs every block the wave runs, a branch on a mask is one decision
for all of them, EXEC is a thread's own bit and guards each vector
instruction. Nothing is assumed about what compiled code skips.
`emit/lower/emulated.rs` then lowers cross-lane operations as exchanges
through group-shared memory: a thread's lane is its flattened id in the
group; a lane sets or clears its own bit of a mask with two atomics, or
puts its value in its own dword; all threads meet at a group barrier; each
reads what it needs; they meet again before the memory is reused. No wave
operation and no wave size are left in the module, so it does not matter
how the host packs threads into waves.

Two things the console's wave gives for free are kept explicitly. Memory
order: on the console an instruction's access is done in every lane before
any lane's next one, so the threads meet before a memory access whenever a
lane may have written that kind of memory since they last met, and before
a write whenever a lane may have read it since (two kinds: the program's
shared memory, and everything reached through views or addresses, taken
as one). One load for the wave: a scalar load from memory is lane 0's load
in every thread (on the console it happens once; threads that read
different values could take different branches and never meet again).

Not kept: which of two lanes that write the same address *in the same
instruction* stays. The console's order there is not modelled; the tests
have no such program.

Refused, and then translated the first way with the reason kept
(`HostShader::wave`): a thread group of several console waves or of less
than one; `v_permlanex16` under a lane mask; an exchange asked for inside
code only some lanes run; a scalar load of more than sixteen dwords from
memory; an instruction the lowering refuses in either form.

A program the per-lane form refuses altogether (its structure or its
lowering: a mask pushed as two numbers and joined with numbers at a loop
header is one case the tests have) has no first way. Where the switch asks
for its wave size, it is given its emulated wave if that can be made, and
the reason there is no other form is kept; otherwise the translation fails
as it does without the switch.

**State.** Implemented. Covered by host tests
(`emit/emulated_tests.rs`): hand-assembled programs for one wave of 64 or
32 lanes (masks read as numbers and made from numbers, every kind of branch
on a mask, a loop the wave leaves together, first-lane reads, lanes read
and written by number, counts below a lane, DPP patterns under full and
partial EXEC, a wave reduction step by step, a scalar load, a small packet
walk that pushes and pops masks; and memory order in shared memory and in
a buffer: write then read, read then write, write then write, and round a
loop), each run on a model of the console's wave written from the
instruction set and as the emulated wave under ten thread schedules (host
waves of 4 to 128 lanes, threads one at a time, arbitrary interleavings),
with every lane's stores and what the buffer holds in the end compared;
the runner refuses a barrier some thread never reaches and a read of
shared memory bits nothing wrote. The same tests show the walk, which has
no per-lane form, translated through the translation's own entry when the
switch asks. "Exact" above means agreement on these programs; it is not a
proof for programs unlike them. Type-checked for Windows. **Not run on any
GPU.** Microsoft's validator outside Windows (`scripts/validate-dxil.sh`,
the `dxv` of a DirectXShaderCompiler release) accepts every container
these tests emit: well-formed DXIL by its rules, which is not a driver
compiling or running them; `dxil.dll` on Windows has not seen them. Its
cost is unknown: two barriers for every exchange (a scalar load from
memory is one), and one before a memory access that follows a write or a
write that follows a read. Drivers are untested with group barriers inside
loops and branches, which this form has by design.

The loop limits (`VICTORY_SHADER_LOOP_LIMIT`, `_TOTAL_LIMIT`) count in an
emulated wave as in any translation. Every thread counts its own back
edges, and all count alike because every thread takes the wave's branches,
so a limit stops the whole wave at one back edge and no thread waits at a
meeting for one that left (host tests: a loop and a nest against small
limits, under the ten schedules). Thread limits
(`VICTORY_DISPATCH_THREADS`) are not given to an emulated wave: a thread
that left at entry would never reach the meetings. Such a program is
translated as without them, and the host treats it as it does any program
that cannot stop its own threads (a dispatch that fills no whole groups
is dropped and counted, or with `=round` run whole).

**Use.** `VICTORY_WAVE_EMULATION` (any build; part of the shader cache
key): unset or `0`, off; `1` or `auto`, the wave sizes this device cannot
run; `32`, `64` or `all`, those sizes on any device, to compare the two
forms on a device that runs both. Off by default until it has run.

### What a run reports

In every build, with or without the switch:

* the log, once per kind of program: `wave sizes: compute programs
  declaring 64-lane waves, which the device does not run ...: NO HOST
  PIPELINE ...` with the reasons, and every five seconds the count of each
  kind when it has changed;
* the log, once per kind: compute programs that do not translate at all,
  by their console wave and whether their thread group is one console wave
  (`wave sizes: compute programs of 64-lane console waves that do not
  translate ...`). Such a program declares nothing, so it is in none of
  the counts above; whether it depends on the wave is not known from a run;
* the performance log: programs that declare a wave size the device runs,
  that carry the wave themselves, that get no pipeline for their wave
  size, and compute programs that do not translate
  (`wave_programs_host`, `_emulated`, `_refused`, `_untranslated`); the
  refused ones by reason (`wave_refused_*`);
* `victory translate`: the same counts over a shader folder without a run,
  with what the emulated form of each program comes to (made, or refused
  and why), for the programs that declare a wave size and for the compute
  programs with no per-lane form (listed with the per-lane form's
  refusal), and with `--validate` and `--pipelines` whether the validator
  and this device's driver take the emulated modules.

## Roadmap

In this order where they compete; correctness items first.

| Item | Track | Next step | Where it can be done |
|---|---|---|---|
| G1 | architecture | `gpu-info` report from the RTX 5070 Ti; then move the remaining single queries into the record | owner's machine; then any |
| G5 | correctness | run the shader probes on the RTX 5070 Ti and record every difference | owner's machine |
| G2 | correctness | the counts from the owner's machine (`victory translate`, or a run's log); the emulated form validated and run on the card (`translate --validate --pipelines`, then a run with `VICTORY_WAVE_EMULATION=1`), compared with the host-wave form on a card that runs 64-lane waves (`VICTORY_WAVE_EMULATION=64` there); then thread groups of several waves | owner's machine; comparison: a 64-lane host |
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
