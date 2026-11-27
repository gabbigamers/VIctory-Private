# Dependency-aware GPU scheduling

When a unit of GPU work may safely run: what it depends on, what must
stay in today's order, and where a wait can go without the title seeing
a difference. The shared notions are in
[resource-model.md](resource-model.md); placement and the missed page are
in [unified-memory.md](unified-memory.md); host structures in
[dxr-host-backend.md](dxr-host-backend.md).

**Nothing here was run.** Standing of each statement: a file and line in
backticks means *read from the code*; **doc** names the document that
reports it; **inferred** says from what; **unknown** names the run on the
owner's machine that would settle it; **proposed** is this page's design,
none of it implemented, tested or measured. `gpu/` and `shader/` as in
resource-model.md. No gain is stated: every column below is measured
before anything is changed.

## The unit of work today

| Fact | Where |
|---|---|
| A title list records into a host list as its methods are called, and is cut into segments at each memory-value wait recorded after commands | `gpu/list.rs:189-218` |
| A title submission pushes segments onto that title queue's FIFO; every consecutive ready head of one queue goes into one *piece*, whichever title lists they came from | `gpu/queue.rs:113-132`; `gpu/hold.rs:134-200` |
| Order within a title queue never changes, only the interleaving across queues | `gpu/hold.rs:14-15` |
| A piece is one `Host::execute`: an upload list of ours ahead, deferred CPU steps, one `ExecuteCommandLists` on the one direct host queue, one signal, one unbounded wait, then copy-back, after-run work and bottom-of-pipe writes | `gpu/host.rs:224-229, 1431-1439, 1461-1511, 1558-1649` |
| All of it under one process-wide lock, held across the wait; the pump holds a second across it | `gpu/host.rs:1381-1383, 1507`; `gpu/queue.rs:158-165, 194-211` |
| The reason given for the lock: "snapshots and the ranges shaders write are per host copy, not per submission" | `gpu/host.rs:1381-1382` |
| The wait itself is a stated shortcut | `gpu/host.rs:4-5` |

How many submissions, pieces and segments a frame has is **unknown**: the
held-work line prints them only in frames where something waited
(`gpu/queue.rs:293-320`).

## What the title can observe

Today each of these appears only when a waited submission has completed,
in queue order. A scheduler may change when the host does its work; it
may not change the order or the content of any of them.

| Observable | Where |
|---|---|
| Shader writes copied back into title memory, before the title is told the GPU is done | `gpu/host.rs:1558-1561` |
| Read-back rows and title-memory pages written by after-run work | `gpu/list.rs:811-814`; `gpu/memory.rs:875-923` |
| Bottom-of-pipe writes, and the timestamps they store (the CPU clock when the list is found complete) | `gpu/host.rs:1645-1649`; `gpu/queue.rs:429-451` |
| A queue's signal, behind the piece before it; fence values, which the title also reads from memory; an event the title registered and blocks on, set only by that signal | `gpu/hold.rs:159-167`; `gpu/fence.rs:86-134` |
| CPU-side copies made at submission: they read title memory then, after the completion effects of every piece before, and write it | `gpu/host.rs:1094-1096, 2559-2567`; `gpu/list.rs:626-640, 1086-1100` |
| The present | `gpu/queue.rs:487-493` |

Existing deviations, kept in view and not widened: the present does not
wait behind held work (`gpu/queue.rs:489-492`); page mappings run ahead
of held work (`gpu/queue.rs:656-659`); a 10 s valve forces held work
(`gpu/queue.rs:186-191`). A blocking fence wait returns `E_NOTIMPL`
because "nothing can reach it while no GPU work runs"
(`gpu/fence.rs:101-106`): false once work is in flight, so S3 below
requires it implemented first.

## The dependency record

One per host list (a segment); a piece's record is the union of its
lists' and our upload list's. It is the summary of resource-model.md,
section 4: derived at submission by a fold over the list's steps, texture
states and twin uses, and read only.

| Field | From | Known |
|---|---|---|
| Copies and ranges shaders read | `Refresh` steps (`gpu/host.rs:2296-2309`); `Heap` and `Memory` steps as "all" | when recorded |
| Title memory the CPU reads at submission, by title address range and not by copy: it bypasses the copies, so it meets a shader write of that range in any copy or alias | the source of a `Copy` and of a `Fill`; a `Run` as "any" (`CopyMemoryToMemoryX` is one, `gpu/list.rs:1086-1100`); a `Host` closure by its declared range (the rectangle expansion reads a title buffer, `gpu/draw.rs:1760-1770`), else "any" | when recorded |
| Title memory the CPU writes at submission | a `Copy`'s destination; `Run` as "any" (`gpu/host.rs:1094-1104`) | when recorded |
| Copies and ranges written by shaders, with *marked* and *uses title memory* | `GpuWrite` steps (`gpu/host.rs:2311-2325`) | when recorded |
| Textures read and written, whole | the list's first and last states, twin uses, host writes (`gpu/host.rs:915-925`; `gpu/texture.rs:232-236`) | when recorded |
| Structures built, refitted, read | the host structure cache, once joined (below) | when recorded |
| Class | *declared*; *heap-wide* (a program with `heap`, reading or writing); *everything* (`memory`); *opaque* (a `Run`, or a `Host` with no declared range) (`shader/emit/mod.rs:479-496`; `gpu/host.rs:1103-1106`) | when recorded |
| Depends on every earlier shader write | an indirect execution (`gpu/indirect.rs:217-240`) | when recorded |
| Waits before, with the addresses they test; completion effects after, with the addresses bottom-of-pipe writes go to | `Deferred.waits`, `.after`, `.writes`, `.missed` (`gpu/host.rs:1146-1161`); `gpu/hold.rs:142-156` | when recorded |
| Content stamps of what it read; its submission value | resource-model.md, sections 5 and 6 | at submission |
| Pages written, slots read, pages missed. They widen nothing ahead of time; they check the record: a mark outside a *declared* list's write set is a defect of the record, counted, and that pipeline's lists become *everything* | marks (`gpu/memory.rs:864-878`; `gpu/shadow.rs:836-842, 882-890`) | after it ran |
| A root view's range: the rest of its copy, as for the rollback of unified-memory.md | it has no size, and its step declares 64 KiB or 1 MiB (`gpu/draw.rs:207-219, 932-958`) | when recorded |

## The rule: unknown keeps today's order and waits

A piece is submitted, waited for and completed exactly as today when any
of these holds. This is the default for every piece until a proof below
applies.

| Condition | Why |
|---|---|
| Class *everything*: a program reads title memory by address | its verdict (missed or not) decides what the next submission's copies hold; a miss withholds a frame's writes (`gpu/memory.rs:340-357`) |
| Class *heap-wide* (readers too), or *opaque* | the write set cannot be named (`shader/emit/mod.rs:491-496`); the views made for texture slots a heap reader read are a completion effect the next submission uses (`gpu/host.rs:1622-1624`); rule 10 of resource-model.md |
| An indirect execution | its arguments are whatever earlier shaders wrote (`gpu/indirect.rs:219-222`) |
| A range of memory the tracker does not track | what the title wrote under a shader's range is not known (`gpu/shadow.rs:621-640`) |
| The frame is withholding, or the piece before it missed | later lists compute from results that are being rolled back (`gpu/memory.rs:363-388`) |
| A query result, a read-back or a timestamp among its effects | its value is the moment of completion (`gpu/queue.rs:429-451`) |
| Diagnostics timing is on | it runs each list as its own submission (`gpu/host.rs:1479-1489`) |
| Any field the record could not fill | rule 10 of resource-model.md |

Which share of a frame's pieces matches no row is **unknown**. One
*everything* list makes its whole piece conservative, so the census
counts pieces as well as lists.

## What may be deferred or overlap, and by which proof

| Step (proposed) | What changes | Proof required |
|---|---|---|
| S1. Record | nothing runs differently: the summary (a portable type, from step 0) is checked by the marks, and the records of a few frames are kept to be intersected offline | none; the marks check the record |
| S2. State per submission, behind `VICTORY_SCHEDULE_STATE=1` | snapshots and pending ranges (`gpu/shadow.rs:78-86`), the view-mark bitmaps (`gpu/shadow.rs:836-842, 963-975`), staging, the prelude and upload lists (`gpu/host.rs:2146-2177`; `gpu/bindless.rs:202-206`) and texture eviction (`gpu/host.rs:1389-1391`) keyed by submission value instead of "none is running" | the switch is the proof's place: the code is Windows-only and its tests have not run here (`gpu/lib.rs:23-81`), so today's keying stays the default until an owner run under the debug layer shows bytes, order and allocations unchanged with one submission at a time |
| S3. Deferred completion | a *declared* piece P is submitted without the wait; its completion effects are queued by fence value and applied in order by the wake source below, before anything behind them | P matches no row of the rule above; S2 on; the blocking fence wait implemented |
| S3, continued: the next piece Q is prepared while P runs | CPU preparation overlaps GPU work | all of: Q's shader reads, CPU reads, writes and refresh ranges touch nothing P writes, and Q's writes and refresh ranges touch nothing P reads, by title address range through the alias links (resource-model.md, section 1), textures whole; Q has no `Copy` or `Run` step and no undeclared `Host` step, whose place is after P's completion effects; no wait of Q tests an address P's shaders or bottom-of-pipe writes may write. Else the pump waits for P first, as today |
| S4. A second host queue | compute or copy work beside graphics | not proposed until S3's columns show GPU idle time that S3 does not remove; cross-queue state rules are then a design of their own |

What S3 needs of the held-work pump, as part of S3 and not as a choice:

| Requirement | Because |
|---|---|
| A piece in flight is an item that keeps its place in its queue's FIFO until its fence value is complete, as a fence wait does (`gpu/hold.rs:168-174`); it counts as held work | a signal behind it stays behind it; the pump returns at once when nothing is held (`gpu/queue.rs:194-197`) |
| A wake source of ours applies completion effects in order: a thread waiting on the host fence's event, or the existing 2 ms timer running whenever a piece is in flight | a title thread blocked on an event it registered makes no further call; its event is set only by the signal behind P (`gpu/fence.rs:91-131`), and the timer starts today only once work stays blocked (`gpu/queue.rs:235-246`) |
| While P is in flight, the head of any other queue has its waits tested only if none tests an address in P's shader write ranges or bottom-of-pipe writes, aliases included; else that head waits for P's completion effects | the pump tests waits by reading title memory at every pump (`gpu/hold.rs:142-156`; `gpu/queue.rs:249-254`); today it reads after P's effects |
| In flight at once: one piece at first | |
| Left to the owner | whether the title's timing tolerates effects landing from that thread and not from its own call (**unknown**: a watch on shader-written ranges in one run would say) |
| Waits that can go under S3, each by the same proof | a declared piece's; the present's own waited submission (`gpu/host.rs:3800-3839`), whose list draws the title's frame into the back buffer (a resize relies on every submission having been waited for, `gpu/host.rs:3626-3630`, and so waits first); the builds of the host structure backend, which has the only unwaited submission (`gpu/host.rs:4052-4061`). A submission that runs about 2 s gets the device reset (`gpu/host.rs:1540-1547`): nothing here merges work into longer submissions |

## Native structures in the model

A host structure is a resource: its key, a content generation and a life
in fence values exist (`gpu/rt/cache.rs:7-9`; `gpu/rt/life.rs:86-103`),
for one open list, called by no run (`gpu/rt/cache.rs:23-34`).

| Operation | Reads | Writes | Rule kept | Where |
|---|---|---|---|---|
| Bottom-level build | its input ranges (positions, indices, transforms) at their content stamps | the structure at a generation; a scratch range of its own | inputs hold their contents before it runs and stay unwritten until its submission is complete; a barrier after | `gpu/rt.rs:44-53` |
| Top-level build | the same, its instance descriptions, and its bottom levels | the structure | bottom levels are held as by a reader | `gpu/rt.rs:57-59` |
| Refit or rebuild in place | as a build | memory earlier readers may hold | a barrier before as well as after; every top level over it is stale | `gpu/rt.rs:53-56, 65-69` |
| Ray-query dispatch | the top level and the bottom levels under it | its own outputs | pinned from recording until its submission is complete; a stale structure is never bound | `gpu/rt.rs:60-69` |
| Retirement | | | released at the highest fence value that may read or write it | `gpu/rt.rs:70-74`; `gpu/rt/retire.rs:1-17` |

What the join needs (**proposed**; none written, `gpu/rt/cache.rs:30-34`):

| Point | Design |
|---|---|
| The record | each build and read carries its list in that list's record; no build goes into memory another open list reads |
| A read whose build is not yet submitted | cannot be refused at submission: its list is closed with the ray-query pipeline bound, and dropping the dispatch is against the standing rule. The path is decided when the dispatch is recorded, where the contract refuses today (**doc**: `dxr-host-backend.md`, "Ordering contract", 4): a structure is bound only if its build is earlier in the same list or in a list already submitted, and current at its inputs' stamps (resource-model.md, section 5, with its validity condition); else the software variant is recorded, counted by reason |
| How often | title lists are recorded on many threads and only the order within a queue is fixed (`gpu/hold.rs:13-15`), so a trace recorded before its build is submitted is the ordinary case (**inferred**). If the count is high, holding the reading segment behind the build's list in the FIFO, as a wait holds one, is the second design |
| "Inputs unwritten until complete" | the S3 proof with the build as P: no refresh and no shader write of an input range while the build is in flight |
| The title's own builds | programs using title memory: class *everything*, always in today's order. The capture gives their destination, sequence and list in recorded order, which is not submission order (`gpu/rtcapture/registry.rs:3-8, 42-56`) |

## Barriers

| Today | Where |
|---|---|
| A title transition on a texture becomes a whole-resource host transition from the list's tracked state; the first use is left to the prelude | `gpu/list.rs:507-511`; `gpu/host.rs:2146-2177` |
| Any barrier naming a buffer adds one global unordered-access barrier | `gpu/list.rs:566-576` |
| Every enhanced-barrier call becomes one global unordered-access barrier, whatever its groups say | `gpu/list.rs:1711-1733` |
| Ours: a global barrier before and after the clamp dispatch of each indirect execution | `gpu/indirect.rs:217-240` |
| Buffer copies are never transitioned and rely on promotion, with a known case outside the letter of D3D12 | `gpu/indirect.rs:195-205` |

Proposed, in order: count them (below); then, behind its own switch, a
barrier that names the one host buffer where the title's barrier names
one buffer, that buffer has exactly one host copy, and the list is
*declared*. Anything else keeps the global barrier (a range held by two
copies, a group that is not read, any other class). A barrier is never
narrower than the title asked.

## Measurements

New columns of the performance log, added before any change. It writes a
row per period of at least 5 s (`gpu/pacing.rs:6-8`; `gpu/host.rs:3335,
3395-3396`): each column is a period mean per frame unless p95 or max is
named. Of a submission it holds `preparing_ms` and `gpu_ms` only today,
and `gpu_ms` is CPU wall time from "lists ready" to the end of the wait
(`gpu/pacing.rs:184-185`; `gpu/host.rs:1476, 1536-1539`).

| Reused, not built again | Where |
|---|---|
| The arithmetic of timestamp regions per list, tested on any host | `gpu/rt/timing.rs:1-60` |
| The per-list timestamp heap, resolve and read-back of the diagnostics feature, freed from its list-by-list submissions, which change the unit they would measure | `gpu/host.rs:961, 978, 1479-1489, 1891-1996` |
| The diagnostics counters of bytes and time per step kind, prelude and uploads, promoted to columns | `gpu/copystat.rs:11-106` |

| Column | Definition |
|---|---|
| `sched_prepare_ms`, `sched_submit_call_ms`, `sched_complete_ms` | on the submitting thread, in thread-milliseconds: lock taken to lists ready (today's `preparing_ms`); the `ExecuteCommandLists` and `Signal` calls alone; copy-back, after-run work and bottom-of-pipe writes (a log line today, `gpu/host.rs:1562, 1642`) |
| **CPU submission overhead** | prepare + submit call + complete. Not in it: waiting |
| `sched_wait_ms`; `sched_lock_wait_gpu_ms`, `sched_lock_wait_cpu_ms` | the fence wait alone; time threads wait for the submission or held-work lock, split into the part overlapping another submission's fence wait (the lock is held across it) and the rest. Summed over threads: thread-milliseconds, which can pass the frame's wall time |
| `sched_record_ms` | time title threads spend inside list methods, pipeline waits apart; reported beside the overhead, not in it |
| `gpu_busy_ms` | the union of the intervals between a start and an end timestamp of each host list, upload list and prelude included, in ticks of the queue's frequency; behind `VICTORY_SCHEDULE_TIMESTAMPS=1` until its cost is known |
| **GPU idle time** `gpu_idle_prepare_ms`, `gpu_idle_complete_ms`, `gpu_idle_held_ms`, `gpu_idle_other_ms` | the frame interval less `gpu_busy_ms`, split by what our side was doing: preparing a submission; completing one; blocked held work only; nothing pending (the title's own CPU time and its pacing wait for the flip, `gpu/frame.rs:1-7`). Only the first two are S3's target |
| **Queue wait** `queue_latency_ms`, p95 too | a piece's first GPU timestamp (its upload list's, if it has one) less the CPU time of its `ExecuteCommandLists`, both on one clock by the queue's clock calibration, which the code does not call yet |
| `complete_delay_ms`, p95 and max | under S3: host fence complete to completion effects applied |
| `held_ms`, `held_segments` | time and count of segments a title queue held (a log line today, `gpu/queue.rs:293-320`) |
| `title_submissions`, `host_submissions`, `segments`, `cuts` | per frame |
| **Barrier cost** `barriers_texture`, `barriers_global`, `barriers_prelude`, `twin_copies` | counts by kind. The cost is the difference in `gpu_busy_ms` and frame time between two runs over one route, switch off and on. A timestamp interval across a barrier is the drain of the work before it, "not the cost of those commands alone" (`gpu/rt/timing.rs:6-13`): a diagnostic only, in at most 64 sampled barriers a period |
| `lists_declared`, `lists_heap_wide`, `lists_everything`, `lists_opaque`, `lists_indirect`; `pieces_*` by the worst class among a piece's lists; `pieces_free` | the census, in step 0 under `VICTORY_PERF_LOG` alone. From flags and steps: an upper bound on what S3 may take. `pieces_free`: pieces matching no row of the rule |
| **Frame time** | the existing columns, one switch at a time against a baseline row |

## Switches, bounds, tests

| Switch (proposed; off by default) | Behaviour | Fallback |
|---|---|---|
| `VICTORY_SCHEDULE_RECORD=1` | S1: the check by marks; the records of 8 frames kept, 4 MiB at most, then dropped | no check, no records |
| `VICTORY_SCHEDULE_TIMESTAMPS=1` | two timestamps per host list | no GPU-side columns |
| `VICTORY_SCHEDULE_STATE=1` | S2 | state keyed on "none is running" |
| `VICTORY_SCHEDULE_ASYNC=1` | S3; refused, with a logged reason, without S2 and the blocking fence wait | every piece waited for |
| `VICTORY_SCHEDULE_BARRIERS=1` | named barriers where proven | global barriers |

| Bound (proposed; its case in `memory.md` first) | Figure | Gauge |
|---|---|---|
| A list's record | lives in its list and goes at the reset; the kept frames (above) under a new owner | that owner's column |
| Per-submission state with P in flight: Q's snapshots, bitmaps and staging beside P's | Q is not prepared while both pieces' snapshot bytes together pass the pool's 512 MiB (**doc**: `memory.md` G4) | `own_snapshots_mb` |
| What waits for a fence value | what the one piece in flight names | `release_queue_items` |

Tested here: the fold over synthetic step sequences, an unknown closure
never yielding *declared*; the independence proof against a byte-level
model over random pairs, with CPU copies and aliases; the held-work pump
with in-flight items (`gpu/hold.rs` is tested on any host); a simulation
in which, for random lists, waits and completion times, the sequence of
observable events (memory-value waits among them) equals the waited
run's; random recording and submission orders over several queues never
giving a trace before its build's barrier. Only the owner's machine
shows: the census, every column before and after, a clean debug layer
and GPU-based validation with each switch on, and the captures of the
reference scenes unchanged.
