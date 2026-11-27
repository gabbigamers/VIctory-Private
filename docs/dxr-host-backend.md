# The experimental DXR host backend

Host side of an experimental backend (`crates/victory-gpu/src/rt/`): host
acceleration structures built with Direct3D 12 and traversed by inline
`RayQuery` (DXR 1.1), beside the software traversal translated programs
carry. **Nothing in a run calls it, and none of it has run on hardware.**
One tool does call it: the synthetic probe, `victory rt-probe`
([dxr-validation.md](dxr-validation.md), "The first version"), through
`Rt::open_on`, and only where `VICTORY_RT_BACKEND` says `native`,
`hybrid`, `auto` or the older name `hardware` (the probe reads the switch
itself, through `select::Request::parse`, before it makes a device, since
`open_on` does not; it measures both paths whichever is named); that use
is type-checked for Windows only.
The software backend is the reference and the fallback for refused cases.

## Selection (by capability and recorded validation, never by vendor or model)

Three paths (`select::Path`), in the order of trial: **native** (host
triangles traversed by inline `RayQuery`), **hybrid** (host boxes, the
title's triangle test in the shader), **software** (the loop the
translated program carries, over the title's own structure: the default,
the reference, and where every other row ends).

| `VICTORY_RT_BACKEND` | A workload runs |
|---|---|
| unset, empty, `software` | software |
| `auto` | of native and hybrid, each a candidate when its rows of the validation record pass all three gates (below): the hybrid where it was measured faster than the native path, else the native path; software where there is no candidate. **In this build there is none: no rule of equivalence is accepted, so `auto` gives every workload the software path, whatever is recorded** |
| `native`, `hybrid` | that path, **not validated** and logged so at each use, where the device can run it and the workload has a variant for it; software elsewhere. For measuring. The record is not read |
| `hardware` | as `native`: its older name, kept so that what was written against it still works; a `note` line of the report says so |
| anything else | software; a `note` line of the report holds the value |

`VICTORY_RT_CLASS=<class>` keeps a forced path to one workload class: 16
to 64 hexadecimal digits that begin the class's hash (a log line names a
class by its first 16). A value that names no class forces nothing, for
any class; a value given without a forced path narrows nothing. Each has
its `note` line.

"The device can run it" is the same for the native and the hybrid path:
ray tracing tier 1.1 or later, shader model 6.6, and the device answers
as `ID3D12Device5` (`HostCaps::raytracing_device`); the list interface is
asked for when the backend opens. Below that only software is left,
whatever is asked and whatever is recorded.

**`select::select_under(accepted, record, caps, switch, workload)`** is the
rule, a function of its arguments alone; `select(record, caps, switch,
workload)` is the same with what this build accepts (`select::ACCEPTED`).
A `Workload` is plain data the translator will fill: the class hash, the
translator's own hash (itself and its settings, opaque here, as the key
the shader cache names a translator's folder by), the hash of the software
container, and for the native and the hybrid path a `Variant` (shader
form, structure source, side tables, container hash) or none, which is
"not eligible". The result names the path, says whether it was forced,
and gives a `Reason` for each path that was tried and not taken:

| `Reason` (name in the log) | A path is not taken because |
|---|---|
| `device` | the device lacks what it needs |
| `not_eligible` | the workload has no variant for it |
| `other_class`, `class_unreadable` | it is forced for another class; or with a class that could not be read |
| `no_rule` | `auto`: the build accepts no rule of equivalence, so no verdict counts. Every workload's reason today |
| `no_driver` | `auto`: the adapter gave no driver version, so no record is kept for the device and none holds for it |
| `no_record`, `other_device` | `auto`: there is no record; or it was made on another device or with another driver |
| `not_recorded` | `auto`: the record has no row of either kind for the class, form, structure source, side tables and path |
| `probe_not_run`, `other_translator`, `probe_other_rule`, `probe_other_version`, `probe_failed` | gate 1: no verdict of the probe for the class on this path; one given for another translator's variant; under another rule than the accepted one; by another version of the probe than the accepted one; or one that failed |
| `title_not_compared`, `other_container`, `other_software`, `title_other_rule`, `title_failed`, `title_empty` | gate 2: nothing of the class was compared on the title on this path (or the row holds times and no verdict); other containers were and this workload's variant was not; the variant was compared with another software container than the workload's; the verdict was given under another rule; it failed; or it passed over no dispatch of the class or no counted frame |
| `not_timed`, `not_faster` | gate 3: no times measured on the title; or the path's median plus the larger of the two spreads is not below software's median |
| `slower` | `auto`, the native path: it passes every gate and the hybrid, which does too, was measured faster |
| `native_first` | `auto`, the hybrid: it passes every gate and was not measured faster than the native path, which passes too and comes first |

Under `software` nothing is tried and nothing is a downgrade; under a
forced path the other path is not tried; under `auto` a hybrid that fails
a gate is not a downgrade when the native path is taken.

**Which hash each gate compares.** The probe runs a synthetic program of
its own for a class, and the title comparison runs the title's programs:
different containers, and a class has as many title containers as it has
programs. So gate 1 compares no container: the probe's row is found by the
class key and the path, and must have been given for the workload's
translator (the probe's own container is in the row for the report and is
compared with nothing). Gates 2 and 3 compare two: the title row is found
by the same key and by the hash of the workload's own variant and of its
own software container. A changed translator therefore leaves every
probe verdict behind (the probe is run again) and leaves the title rows
of exactly the containers it did not change.

**What a verdict must have been given under** (`select::Standard`): the
number of the rule of equivalence, its constants among it, and the
version of the probe. Each verdict in the record carries the rule's
number, the probe's also the probe's version, and a verdict under another
counts for nothing. `select::ACCEPTED` is what this build accepts:
**none**, because the rule is a proposal the owner has not accepted
(`STATUS.md`, "Waiting on the owner": until it is, `auto` selects nothing
under it). That default is a constant of the build and not the absence of
a writer: a record in which every verdict says passed selects nothing
here. Accepting the rule is setting that constant, in the change that
records the acceptance.

**The faster of two candidates** (part of the proposal for "fastest",
which the owner has not accepted, `dxr-validation.md`, last section). The
two paths were timed in two runs, each against software. Their medians
are set against each other only where the runs can be: software's medians
in the two differ by no more than the larger of its two spreads. Then the
hybrid is the faster if its median is below the native path's by more
than the larger of the two paths' spreads (`record::Times::faster_than`).
Otherwise nothing was measured between them and the native path, first in
the order of trial, is taken.

**The validation record** (`record`) is plain data for one device and
driver. The identity (`record::Identity`: vendor id, device id, driver
version) is an equality key with private parts: a record made on another
device, or with another driver version, or where no driver version is
known, holds for nothing. It has two kinds of row:

| Row | Key | Holds |
|---|---|---|
| probe (`ProbeRow`) | workload class, shader form (`dual`, `both-loops`), structure source (`descriptions`, `packed`, `nodes`), side tables (a set of `instance`, `node`, `id`), path. One row a key: a later verdict takes the earlier one's place, and `set_probe` hands the earlier row back | the verdict; the rule's number and the probe's version it was given under; the translator's hash; the hash of the probe's own production container |
| title (`TitleRow`) | the same, and the hash of the variant's container and of the software container it was compared with (`TitleKey`). A row for each comparison: a finding about one container leaves every other row as it was, and `set_title` and `set_times` hand back what they replace in their own row | the title's verdict with the rule's number and the dispatches and frames it counted; the title's times (the path's median and spread, software's median and spread, microseconds a counted frame); either may be missing |

Rows of containers that no translation makes any more stay until a tool
gives them up by name (`forget_title`, `forget_probe`, which hand the row
back); a full record refuses a new row and says so.

On disk it is text, a line a row (`probe ...`, `title ...`), in the
folder `rt-record/` of the cache folder, where `shaders/` and `pipelines/`
are (`record::FOLDER`, which a run would ask its configuration for as it
asks for those), one file a device and driver, named by the start of the
identity's hash as a translator's shader folder is named by its key: after
a driver update there is no record, and the old driver's file is left for
it. The first line is `victory-rt-record version=1`, the second the
`device` line. A file whose `device` line names another identity counts
as absent. A text with a second first line or a second `device` line (two
records put together) is refused whole, since rows taken on another
device would otherwise be read as this one's. Two lines for one key say
two things about one subject: neither is matched and both are kept.
Reading keeps what it does not understand: an unknown field of a row is
written back with it, a line that cannot be read (a verdict that does not
say what it was given under is one) is written back as it was and matched
by nothing. What a tool finds with a verdict and the selection does not
read (the probe's levels and largest coordinate, the value of the
tolerance constant, a scene, a date, capture ids) goes into the row the
same way, as a field of its own (`Record::set_probe_field`,
`Record::set_title_field`, 384 bytes a row at most). A later version is
refused whole, as an error the holder logs as a warning; so are a file
over 1 MiB and more than 1,024 rows. A file is read through that bound,
whatever the file system says its size is. A record whose text is over
the bound is not saved and the file stays as it was; a text read at the
bound can be a few bytes longer written back (a last line without its
end), and is then refused the same way. A container is named by
`record::Digest::of` (BLAKE3 of its bytes) by whatever writes a row and
by the run that looks for it.

**Counters.** `select::choose` is `select` for a run: it counts the choice
in the run's `select::Tally` (workloads by path, those forced, and each
path not taken by reason; relaxed atomics, no memory of its own), logs a
forced path as not validated at each use, and the frame meter logs
`ray tracing paths so far:` with the counts beside its five-second lines
whenever a choice was counted since it last did.

**What a run does with all this today: nothing but the report.** Nothing
in a run calls `select` or `choose`, reads the record or builds a
`Workload`, so the counters stay at zero and that log line never appears.
`victory gpu-info` and a run's log end with a `ray tracing backend:` line
that says what the switch asks on this device and that nothing in a run
selects a path yet, then the `note` lines. Under `auto` the line says
software and why: that the build accepts no rule of equivalence (today),
or that the adapter gave no driver version, or what the device lacks. The
backend's own entry (`Rt::open`) opens only where a native or hybrid path
may be selected (a forced path on a device that can run it; `auto` only
once a rule is accepted), and nothing in a run calls it either.

## Modules

| Module | What | Device |
|---|---|---|
| `select` | the three paths, the switch, what the build accepts verdicts under, the device's line of the report, `select()` and its reasons, the counters | none |
| `record` | the validation record: probe rows and title rows, their keys, verdicts, times, text form, file, bounds | none (files) |
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
| `RtRecord` | `own_rt_record_mb` | the validation record while a run keeps it (`record::Held`); process heap. Zero today: no run keeps one | 1,024 rows and 1 MiB of text read; under 1.5 MiB |

The first two are video memory (new `Kind::Video`). Over a bound a build
is refused and counted; nothing is evicted. A record over its bound is
refused whole. All bounds are unmeasured.

## For a validation tool (`rt::device::Rt`)

`open` (honours the switch) or `open_on(device)` (own queue); `upload`;
`build_bottom`, `build_top` (build, refit, rebuild); `compact`; `free`;
`bind_top(root, handle)` or `top_address`; `record` for its own pipeline
and dispatch; `timed(label, ..)` and `timings()` for GPU time (a region
lives in one list: one open at a submit is cut, has no time and is
counted; the latest 4096 results are kept); `submit`, `wait` (returns the
CPU time waited), `flush`; `output`, `read_back`; a buffer's `state()`
(for a caller that records a copy into an output buffer, as the probe
does to poison hit records before a dispatch); `stats()`, `memory()`,
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
| `desc`, `update`, `life`, `cache`, `retire`, `scratch`, `budget`, `timing`, `stats` | implemented; host-tested (75 tests, two of them randomised lifetime properties) |
| `select`: the three paths, the switch with `VICTORY_RT_CLASS` and the older name, what the build accepts verdicts under (nothing), `select()` with every reason, the counters | implemented; host-tested (30 tests). Called by nothing in a run; speculative as to the title, since no workload description exists yet to give it |
| `record`: the validation record, its two kinds of row, text form, file and bounds | implemented; host-tested (21 tests, the file in a temporary folder). Nothing writes one yet (the probe and the title comparison will), and no run reads one |
| Capability record, `gpu-info` line with its `note` lines (the environment to the report tested in processes of their own), four owners and columns | implemented; host-tested |
| `device` (`Rt`), `host.rs` accessors, filling `raytracing_device` | implemented; type-checked for Windows only |
| Smoke test `structures_are_built_refitted_compacted_and_released_on_the_host` | type-checked for Windows only; not yet run on hardware. Compacts a 20 000-triangle bottom level and builds the top level over the copy; where the device reports no smaller size it prints `COMPACTION NOT EXERCISED` and the copy stays untested |
| Every D3D12 call, timestamp readings, memory figures | not yet run on hardware |
| Use by the synthetic probe (`crates/victory-rt-probe/src/hardware.rs`): `open_on`, uploads, builds and refits as timed regions, `bind_top`, `record`, `output`, `read_back` | implemented; type-checked for Windows only |
| RayQuery in translated programs, guest structure interception, use by a run's submissions | not part of this step |
| Selection joined to a run: a `Workload` from the translator for each recognised region, with the translator's hash; the record read at start (`record::Held::open` on the cache folder's `rt-record`); `choose` at variant creation | not written. Until then the switch changes the report and nothing else |
| Before `auto` can select anything, each of them: the owner accepts the rule of equivalence and `select::ACCEPTED` is set to its number and the probe's version (the probe's version is defined once, as `select::PROBE_VERSION`, which the probe's crate takes as its own `PROBE_VERSION` since `victory-gpu` cannot depend on that crate; a test in `select` fails if `ACCEPTED` names another number); the probe and the title comparison write their rows with those numbers; the title's coordinates are checked against the probe's largest (`m_max`, today a field the selection does not read); the second clause of the proposed "faster" (a normal-build run) has a place in the record or is dropped | not done, and not this step's to decide |
| Open for hardware | whether empty levels are accepted; whether a barrier per build, and one more before each refit, serialises builds; what a result buffer per structure wastes (up to 64 KiB each); whether the choices stricter than D3D12 (no refit after compaction, or when an instance's bottom-level address or the index address changed; no compaction after a refit) cost rebuilds or memory that matter |
| Before a run's submissions use it | several open lists (see the ordering contract); the debug layer's view of upload memory as a build input and of timestamps resolved into read-back memory; whether time blocked on the host's submission lock belongs in the measured wait |
