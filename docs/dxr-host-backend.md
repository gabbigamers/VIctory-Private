# The experimental DXR host backend

Host side of an experimental backend (`crates/victory-gpu/src/rt/`): host
acceleration structures built with Direct3D 12 and traversed by inline
`RayQuery` (DXR 1.1), beside the software traversal translated programs
carry. **Nothing in a run calls it, and none of it has run on hardware.**
The software backend is the reference and the fallback for refused cases.

## Selection (by capability, never by vendor or model)

| `VICTORY_RT_BACKEND` | Device can run it | Backend |
|---|---|---|
| unset, empty, `software` | either | software |
| `hardware` | yes | hardware inline RayQuery |
| `hardware` | no | software; the report line names what is missing |
| `auto` | yes / no | hardware / software |
| anything else | either | software; a `note` line of the report holds the value |

"Can run it": ray tracing tier 1.1 or later, shader model 6.6, and the
device answers as `ID3D12Device5` (`HostCaps::raytracing_device`, new); the
list interface is asked for when the backend opens. `victory gpu-info` and
a run's log end with a `ray tracing backend:` line, which also says that
no run uses the hardware backend yet. A run logs that report at info level:
a fallback is a **warning** only where the backend is opened (`Rt::open`),
which nothing in a run does yet.

## Modules

| Module | What | Device |
|---|---|---|
| `select` | backend choice | none |
| `desc` | geometry, instances, D3D12's rules, instance encoding, refit shape | none |
| `update` | refit or rebuild; the rebuild-after-N-refits policy | none |
| `life` | one structure: empty, building, ready, stale, retired | none |
| `cache` | structures by caller identity and content generation; top levels hold their bottom levels | none |
| `retire` | memory waiting for a fence value | none |
| `scratch` | pooled, bounded scratch and instance memory | none |
| `budget` | bounds from the device's budget or the user | none |
| `timing` | timestamp regions to milliseconds | none |
| `stats` | counters of work and refusals | none |
| `device` | `Rt`: the D3D12 calls over all of the above, through new accessors in `host.rs` (submit without waiting, completed fence value, wait for a value, timestamp frequency, process memory) | Windows only |

## Ordering contract

Time is fence values: each submitted list gets the next one, complete when
the list has run (full text: the module doc of `rt.rs`).

1. **Inputs ready**: what a build reads is written before the build runs
   (by the CPU before submission, or by GPU commands already submitted or
   earlier in the same list), and stays alive and unwritten until the
   build's submission is complete. Otherwise: refused.
2. **Build**: recorded with a UAV barrier on its result right after. A
   refit also has one before it: it writes memory that a dispatch or a
   top-level build earlier in the list may read. A top level is built only
   over bottom levels readable at that point and holds them as a reader
   does. Builds in one list never share scratch.
3. **First trace**: only after that barrier, in the same list or a later
   submission. A reader pins the structure (and a top level's bottom
   levels) until its submission is complete.
4. **Change**: building into a bottom level makes every top level over it
   stale; a new content generation makes a structure stale. Stale
   structures are not bound (refused, counted), and giving a stale bottom
   level up does not make the top level over it readable again.
5. **Release**: only with the open list not holding it and the latest
   fence value that may touch it complete.

**One open list.** The cache models a single list being recorded: its
builds and reads are submitted or dropped together (`Cache::seal`,
`Cache::abandon`). Several lists open at once, as a title records, are not
modelled and cannot be expressed: they need each build and read to carry
its list, reads refused while the build is in another unsubmitted list,
and no refit of memory another open list reads. Not written.

## Lifetime and updates

* Identity is the caller's `(level, id)` plus a content generation (later:
  a guest structure's address and generation). Refit is in place; rebuild
  and compaction go into new memory, the old waits in the retire queue.
* A bottom level given up stays, memory included, while a top level built
  over it lives.
* A list dropped unsubmitted undoes its pins; its builds leave their
  structures stale (or empty) and due a whole rebuild. A list the queue
  took without a fence value: what it may use is leaked.
* Refit only if the source allowed updates, is not compacted, has the same
  shape (counts, formats, strides, flags, transform presence, index
  address), the caller states that only positions changed, and, for a top
  level, every instance refers to the same bottom-level address. After
  `VICTORY_RT_REBUILD_AFTER_REFITS` refits in a row (default 16) the next
  change is a rebuild. **That number is a policy placeholder, not a
  measured optimum.** Where the memory bound has no room for that rebuild
  (it needs new memory beside the old) the structure is refitted once more
  and counted (`refits_past_limit`); with the number 0 it is refused.
* Compaction uses the size the build reported: a structure refitted since
  is not compacted (`Compacted::Refitted`, counted) until it is rebuilt.

## Memory

Counted in `account.rs` as performance-log columns (the xtask summary
finds owners by prefix and is unchanged); not the query heap or allocators.

| Owner | Column | What | Bound |
|---|---|---|---|
| `RtStructures` | `own_rt_structures_mb` | result buffers, live and waiting for release; one committed buffer each (64 KiB granularity) | a quarter of the device's budget, 64 MiB to 2 GiB, or `VICTORY_RT_STRUCTURES_MB` |
| `RtScratch` | `own_rt_scratch_mb` | scratch chunks, compacted-size buffer, a validation run's output buffers | scratch 256 MiB live, 32 MiB kept idle; outputs as asked |
| `RtBuffers` | `own_rt_buffers_mb` | instance chunks, uploads, read-backs | instances 64 MiB live, 4 MiB kept idle; the rest as asked |

The first two are video memory (new `Kind::Video`). Over a bound a build
is refused and counted; nothing is evicted. All bounds are unmeasured.

## For a validation tool (`rt::device::Rt`)

`open` (honours the switch) or `open_on(device)` (own queue); `upload`;
`build_bottom`, `build_top` (build, refit, rebuild); `compact`; `free`;
`bind_top(root, handle)` or `top_address`; `record` for its own pipeline
and dispatch; `timed(label, ..)` and `timings()` for GPU time (a region
lives in one list: one open at a submit is cut, has no time and is
counted; the latest 4096 results are kept); `submit`, `wait` (returns the
CPU time waited), `flush`; `output`, `read_back`; `stats()`, `memory()`,
`video_memory_mb()` (the process's use on the adapter of this backend's
own device), `commit_and_working_set()`.

Refused today (software backend instead): empty levels; triangles and
boxes in one bottom level; vertex formats beyond the six of tier 1.0;
strides over 32 bits (D3D12 reads the low 32); primitive counts over
D3D12's limit, however they sum; non-finite instance transforms; ids over
24 bits; inputs not yet written; builds over a memory bound; a top level
over a stale, unbuilt or retired bottom level. NaN vertices and index
contents are the caller's to keep.
The rules were read from the DirectX Raytracing functional specification.

## Status

| Part | State |
|---|---|
| `select`, `desc`, `update`, `life`, `cache`, `retire`, `scratch`, `budget`, `timing`, `stats` | implemented; host-tested (83 tests, two of them randomised lifetime properties) |
| Capability record, `gpu-info` line, three owners and columns | implemented; host-tested |
| `device` (`Rt`), `host.rs` accessors, filling `raytracing_device` | implemented; type-checked for Windows only |
| Smoke test `structures_are_built_refitted_compacted_and_released_on_the_host` | type-checked for Windows only; not yet run on hardware. Compacts a 20 000-triangle bottom level and builds the top level over the copy; where the device reports no smaller size it prints `COMPACTION NOT EXERCISED` and the copy stays untested |
| Every D3D12 call, timestamp readings, memory figures | not yet run on hardware |
| RayQuery in translated programs, guest structure interception, use by a run's submissions | not part of this step |
| Open for hardware | whether empty levels are accepted; whether a barrier per build, and one more before each refit, serialises builds; what a result buffer per structure wastes (up to 64 KiB each); whether the choices stricter than D3D12 (no refit after compaction, or when an instance's bottom-level address or the index address changed; no compaction after a refit) cost rebuilds or memory that matter |
| Before a run's submissions use it | several open lists (see the ordering contract); the debug layer's view of upload memory as a build input and of timestamps resolved into read-back memory; whether time blocked on the host's submission lock belongs in the measured wait |
