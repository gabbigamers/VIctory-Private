# The shared resource model

The one understanding of a resource that the three subsystems of the
owner's direction of 2026-11-27 (`progress.md`, "The owner's direction of
2026-11-27 for after the work now active") share: streaming says what
will be needed ([predictive-streaming.md](predictive-streaming.md)),
memory management where it may safely live
([unified-memory.md](unified-memory.md)), scheduling when it may safely
run ([gpu-scheduling.md](gpu-scheduling.md)). This page is the contract
between them. **It introduces no type that an existing structure already
is**: each notion is a key or a stamp carried by structures that exist,
and one query that joins them.

**Nothing here was run: this container has no GPU and no title.** Standing
of each statement: a file and line in backticks means *read from the
code*; **doc** names a document here that reports it; **inferred** says
from what; **unknown** names the run on the owner's machine that would
settle it; **proposed** is this page's design, and nothing proposed is
implemented, tested or measured. `gpu/` is `crates/victory-gpu/src/`,
`hle/` is `crates/victory-hle/src/modules/`, `shader/` is
`crates/victory-shader/src/`, `pipe/` is `crates/victory-pipeline/src/`.

## The six notions

| Notion | What it is today | What is missing | Extension (proposed) |
|---|---|---|---|
| Identity, aliases | the title address range, looked up in separate maps | one answer to "every copy and view of this range" | a read-only query over those maps |
| Ownership | an owner class with a gauge; who frees; which side is the truth | the title's allocation extents on the GPU side | the runtime reports extents as it reports aliases |
| Residency, placement | system memory for every copy of title memory; the card for textures, with the one evictor | last use and "still named" for anything but textures; any placement choice | two stamps and holder counts by kind on a copy; placement chosen when its buffer is made |
| Access dependencies | per list, steps that perform declared reads and writes; per shader, flags; after the run, marks | the sets read as one answer; ranges for closures | a summary derived from the steps at submission, with a class |
| Content generations, dirty ranges | per holder "changed since taken"; flags for which side is newer; hashes for textures | a number saying which contents a copy holds | a content stamp advanced wherever a copy's bytes may change |
| Execution lifetime | one waited submission at a time; holders by reference count | the submission a thing was last used by | the host fence value stamped on what a submission names |

## 1. Identity and aliases

The key is the title address: a resource's GPU address is its CPU address
(`gpu/lib.rs:7`). No single record exists; the maps hang off the address.

| What | Where |
|---|---|
| Title buffers by (start, resource), with their copy once made | `gpu/shadow.rs:1033-1039` |
| Windows of 4 MiB for memory outside buffers; per-view copies | `gpu/shadow.rs:1161-1164`; `:1244-1251` |
| Pages held on demand, by page number and run | `gpu/memory.rs:414-420` |
| Host textures by address and shape (`MemoryKey`) | `gpu/resource.rs:1149-1161, 1200-1205` |
| View records, with a generation so a reused address is not mistaken | `gpu/descriptor.rs:483-489` |
| The runtime's physical pages, views, and where each is shown | `hle/xmem.rs:457-465` |
| Host structures by (level, id); captured builds by destination and sequence | `gpu/rt/cache.rs:7-9`; `gpu/rtcapture/registry.rs:42-56` |
| Joins that exist: what holds a page; every buffer copy and window; per view heap, the one copy each 64 KiB page is read through | `gpu/shadow.rs:1060-1091`; `:1375`; `gpu/paging.rs:222-270` |
| Alias: a physical page shown at several addresses, linked both ways | `gpu/watch.rs:23-31, 884`; `hle/xmem.rs:462-465` |
| Alias: a buffer inside another (the innermost of the last 64 is taken); title textures of one memory sharing one host texture | `gpu/shadow.rs:1113-1121`; `gpu/resource.rs:1200-1205` |
| Alias: a per-view copy holding bytes its buffer's copy also holds | **doc**: `memory.md` row G3 |
| **Extended (proposed):** one query, `holders(range)`, beside `shadow::holding`: for a range, each holder's kind, its range, and whether it holds the range whole or in part. It reads the maps above and stores nothing. A file a request reads from is not a resource until its bytes land in pages; its identity is streaming's own | predictive-streaming.md |
| **Not duplicated:** no resource record with a start, a length and a state of its own; no second alias table (the tracker's links and the runtime's `shown` are the truth); `MemoryKey` stays the texture key; a host structure's key later *is* a title structure's address | `gpu/rt/cache.rs:7-8` |

## 2. Ownership

| What | Where |
|---|---|
| Owner classes with bytes alive, peak, adds, underflows | `gpu/account.rs:19-146, 176` |
| A resource's CPU memory goes back to the title's allocator | `gpu/resource.rs:689-696` |
| The registry entry holds the buffer's copy; `Paging` holds pages on demand | `gpu/shadow.rs:1033-1036`; `gpu/memory.rs:414-420` |
| A section per allocation of physical pages | `hle/xmem.rs:360-466` |
| Truth: title memory, except from a submission's refresh to its copy-back | `gpu/host.rs:1558-1649`; `gpu/shadow.rs:78-80` |
| Truth: the host texture once a host copy or resolve wrote it | `gpu/texture.rs:224-236`; `gpu/resource.rs:808-810` |
| Truth: the newer side of a depth twin | `gpu/twin.rs:384-392` |
| **Missing:** "the GPU layer has no list of the runtime's allocations to ask"; `XMemVirtualAlloc` logs a reservation and keeps no table; accounting is per class, not per resource | `gpu/paging.rs:477-478`; `hle/xmem.rs:177-185`; `gpu/account.rs:176` |
| **Extended (proposed):** the runtime reports an allocation's extent to the GPU side where it already reports mappings and aliases; one table holds them, filled only with `VICTORY_MEMORY_EXTENTS` and bounded. "Which side is the truth" stays a question answered from `pending`, what is withheld, `host_written` and the twin's side | `gpu/watch.rs:884`; unified-memory.md, "Bounds" |
| **Not duplicated:** no second accounting; no "authority" field beside those | |

## 3. Residency and placement

| What | Where |
|---|---|
| Every copy of title memory: a CPU-mapped buffer in system memory | `gpu/host.rs:3983-3993` |
| Host textures on the card; given up when idle or over the device's budget, least recently used first | `gpu/resource.rs:1527-1604`; the rule as a function, `:1624` |
| Only textures whose contents come from title memory alone are given up | `gpu/resource.rs:1575-1581` |
| Suballocation: 64 MiB texture heaps; 16 MiB chunks for view copies; staging pools | `gpu/placement.rs:1-24`; `gpu/shadow.rs:1278-1372`; `gpu/texture.rs:94-99` |
| Pages on demand under a hard bound | `gpu/paging.rs:151` |
| A diagnostics switch that puts every shared buffer in CPU-visible video memory, where the device has it | `gpu/host.rs:3954-3999` |
| **Missing:** a copy carries no last use; whether a list not yet reset or a heap slot still names a copy is not tracked; each kind of copy has one hard-wired heap type | `gpu/shadow.rs:68-101`; **doc**: `memory.md` candidate K8; `gpu/host.rs:3986-3999` |
| **Extended (proposed):** two stamps on a copy: the submission value of its last use (section 6) and the frame of its last refresh request. Placement is chosen when a copy's buffer is made | unified-memory.md, "Part 2" |
| **Extended (proposed):** holder counts by kind on a copy, since *named* cannot be read from stamps (a heap slot names a copy with no refresh and no submission). Each count is kept where that holder takes and drops its reference: list steps; heap slots and slot records; page tables with their view pairs; and a hold `find` has handed out that no step holds yet, from its return. Each kind of copy is counted and judged under the lock that hands it out: the registry's for buffer copies, the windows' own, the per-view map's. *In flight*, *named* and *reconstructible* are then read, not stored as a state | `gpu/host.rs:2296-2325`; **doc**: `memory.md` row G1, `gpu/account.rs:196-204`; `gpu/memory.rs:11-14`; `gpu/shadow.rs:1113-1152, 1164-1169, 1251`; `gpu/draw.rs:209-219`; unified-memory.md, "Giving up and moving" |
| **Not duplicated:** no second evictor (the texture pass's rule is generalised); no second suballocator; no holder list beside the references that exist | |

## 4. Access dependencies

| What | Where |
|---|---|
| Per host list, nine kinds of step in recorded order: `Refresh` (a read range), `GpuWrite` (a write range), `Texture`, `Heap` (all a heap reaches), `Memory` (every copy the table names), `Copy` and `Fill` (CPU copies that read title memory at submission), `Run` and `Host` (closures; a `Host` one writes host memory only and may read title memory, `gpu/draw.rs:1760-1770`) | `gpu/host.rs:1092-1122` |
| Filled at draw and dispatch from vertex, index, root and table views | `gpu/draw.rs:203-227, 564-570, 934-958` |
| Per list, each texture's first and last state; twin uses | `gpu/host.rs:915-925` |
| Per shader: reads memory by address, indexes the heap, may write through it, marks its writes | `shader/emit/mod.rs:479-500` |
| After the run: pages written and missed, per list | `gpu/memory.rs:864-878` |
| After the run: pages written through views (global bitmaps); texture slots read | `gpu/shadow.rs:836-842, 882-890` |
| **Missing:** `Run` and `Host` are closures with no range; a root view has no size, so 64 KiB or 1 MiB is declared while the view reaches the rest of its copy; which root-constant dwords a program uses as a pointer is known to the analysis and dropped | `gpu/host.rs:1103-1106`; `gpu/draw.rs:207-219, 932-958`; `shader/resource.rs:829-857` |
| **Missing:** a read mark for buffers and for title-memory pages | **doc**: `memory.md`, case G11, "Tracking needed" |
| **Extended (proposed):** a summary per host list, *derived* at submission by a pure fold over the list's steps, texture states and twin uses, as `Host::execute` already reads ranges out of the steps. Stored beside the steps is only what they lack: the shader flags of what was bound, indirect use, and a declared read and write range for each `Run` and `Host` closure. Its **class**: *declared* (all of it named), *heap-wide*, *everything* (a program using title memory), *opaque* (a closure with no declared range). The fold and the proofs over it are a portable type, tested on any host | `gpu/host.rs:1401-1427, 1586-1599`; gpu-scheduling.md, "The dependency record" |
| **Not duplicated:** no stored second copy of what the steps hold, which could drift from what they perform; no second walk of a draw's bindings; no second mark bitmap | |

## 5. Content generations and dirty ranges

| What | Where |
|---|---|
| Per holder, the 64 KiB pages changed since it took them; alias links | `gpu/watch.rs:41-56` |
| Writers no guard sees announce themselves (file reads, DirectStorage) | `gpu/watch.rs:33-35` |
| What the GPU wrote: a snapshot per declared range, or armed pages and marks | `gpu/shadow.rs:350-380, 443-452` |
| Ranges whose newest bytes are in the copy only | `gpu/shadow.rs:78-80` |
| Textures: a hash per subresource of the title's bytes | `gpu/texture.rs:611-627` |
| Counters named generation that are **not** content: the set of copies; a view's identity; a destination's records | `gpu/shadow.rs:1045-1058`; `gpu/descriptor.rs:483-489`; `gpu/rtcapture/registry.rs:50-53` |
| A content generation the caller passes in, for host structures | `gpu/rt/cache.rs:7-9` (no run calls it: **doc**, `STATUS.md`) |
| **Missing:** anything that says which contents a copy holds; the tracker answers "changed since taken" per holder and no more | `gpu/watch.rs:44-49` |
| **Extended (proposed):** a **content stamp** on a copy, a counter advanced wherever its bytes may change. Before a run: a refresh that copies pages into it (an untracked copy at every refresh); a declared shader write; a host texture becoming the host's | `gpu/shadow.rs:660, 621-640, 379`; `gpu/texture.rs:233-235` |
| The stamp after a run, since writes by address and through undeclared heap views never pass `writing`: each copy the list's marks name; a copy-back that merged title bytes in; a retake; a page's place zeroed. Until a run shows the marks complete, a submission with a `Memory` step or a `heap_writes` program advances every copy its table or heap names, and a stamp a reader took at submission is valid only if no such submission ran since | `gpu/host.rs:2599-2617`; `gpu/shadow.rs:541-543, 966, 28-37, 497-499`; `gpu/memory.rs:876-920, 478-487`; `gpu/watch.rs:858` |
| One stamp per copy at first; it is the value `rt/cache.rs` is to be given. Across runs contents are named by hash: a stamp never leaves the run, a hash never orders two contents | `pipe/cache.rs:15-22` |
| **Not duplicated:** no second write watch; no finer unit than 64 KiB ("finer units made the checks ... cost more than the copies they saved"); no second hash scheme; the stamp does not replace the tracker | `gpu/watch.rs:41-44` |

## 6. Execution lifetime

| What | Where |
|---|---|
| One host queue and fence; a counter under the submission lock gives each `ExecuteCommandLists` its value | `gpu/host.rs:224-229, 1494-1504` |
| Every submission is waited for (a stated shortcut) | `gpu/host.rs:4-5, 1499-1511` |
| The one unwaited path takes its value from the same counter | `gpu/host.rs:4052-4061` |
| A list holds what it names until reset; a queue holds the list | `gpu/host.rs:915-921, 1128-1140`; `gpu/list.rs:275-280` |
| Release and reuse keyed on "no submission is running" | `gpu/host.rs:4089-4112`; `gpu/texture.rs:94-99`; `gpu/bindless.rs:202-206` |
| Pins, a last-use fence value, and a release queue by fence value, with no D3D12 type in them | `gpu/rt/life.rs:86-103`; `gpu/rt/retire.rs:1-17` |
| **Missing:** outside `rt/`, nothing stores the submission that last used it | `gpu/shadow.rs:68-101`; `gpu/resource.rs:802-830` |
| **Extended (proposed):** one clock, the host fence value. A last-use value on a host copy, a host texture and a staging buffer, set where a submission's steps run; a piece run again or timed list by list has several values, and last use takes the last. "In flight" is last use above the completed value. For many lists open at once the pin is the reference a list already holds, not the one-open-list pin of `rt/cache.rs` | unified-memory.md, "Proposed first"; `gpu/host.rs:1479-1494`; `gpu/rt/cache.rs:23-34` |
| With no switch the stamps are written and read by nothing that decides. Release through `RetireQueue` in place of "none running" is behind `VICTORY_RELEASE_BY_FENCE=1` (**proposed**), today's code the fallback: the modules it touches are Windows-only and their tests have not run | `gpu/lib.rs:23-81`; **doc**: `STATUS.md` |
| **Not duplicated:** no second fence or counter; no second release queue; `rt::life::Life` stays the state machine for host structures | |

## Who reads and writes what

| | Streaming | Memory | Scheduling |
|---|---|---|---|
| Says | what will be needed (advice) | where it may live | when it may run |
| Writes | nothing of the model: bytes in a pool and a cache of its own, each with its owner | residency, placement, *pressure* | submission values, completion, last use |
| Reads | pressure and its bounds | last use, in flight, holder counts | the summaries, content stamps, what is resident |
| Never | writes title memory, a status entry or a resource's contents | gives up what is not reconstructible, is named or is in flight | lets a completion effect be seen out of order |

*Pressure* (**proposed**) is one signal memory computes once a log period
from the machine's side, as "does it fit" is read today (**doc**:
`memory.md`, "Four quantities"): system commit against its limit, the
paging file's growth and the fault rate, with a high and a low mark.
`physical_free_mb` alone cannot serve: it counts standby pages as free.
Above the high mark nothing speculative starts and streaming's pool is
given up first. The marks are **unknown** until the baseline run.

## Rules all three keep

Each is a hazard the readers found in the code, kept as a rule.

| # | Rule | Where |
|---|---|---|
| 1 | From a submission's start to its copy-back, or to the retake of what was withheld (the frame's end), the host copy alone holds what shaders wrote: nothing moves, replaces or drops it then (the one exception is the replay's rollback inside a submission, which runs the same work again) | `gpu/host.rs:1558-1649`; `gpu/shadow.rs:78-84, 457-459`; `gpu/memory.rs:342-388` |
| 2 | Copy-back puts back only the bytes the GPU changed | `gpu/shadow.rs:443-449` |
| 3 | A write at one address of a physical page is a change at all of them; a new copy is linked to every alias | `gpu/watch.rs:23-31` |
| 4 | A write no guard sees is announced | `gpu/watch.rs:33-35` |
| 5 | Completion effects keep their order: copy-back, after-run work, bottom-of-pipe writes, then the queue's signal | `gpu/host.rs:1558-1561, 1645-1649`; `gpu/hold.rs:159-167` |
| 6 | One page is read at one place in a submission; tables and views are written only while none runs | `gpu/paging.rs:53-57`; `gpu/memory.rs:712-715` |
| 7 | A copy's host address is recorded into lists and slots long before they run: it does not move while one lives | `gpu/draw.rs:209-223` |
| 8 | Only what title memory alone restores is given up: a texture nothing on the host wrote; a buffer copy no shader wrote since title memory last matched it | `gpu/resource.rs:1575-1581`; unified-memory.md, "Giving up and moving" |
| 9 | Withholding after a miss is a correctness rule, not a cost to remove | `gpu/memory.rs:340-357` |
| 10 | Unknown is conservative: the classes *heap-wide* (readers too), *everything* and *opaque*, and memory the tracker does not track, keep today's order and today's copies | `shader/emit/mod.rs:491-496`; `gpu/shadow.rs:621-640` |
| 11 | A new long-lived allocation is bounded, owned, counted and has its case | `../CLAUDE.md`, "Rules that always hold" |

## Order of work

The coordinator's order is kept; the findings support it and move a few
things forward (marked).

| Step | What | Why here |
|---|---|---|
| 0 | Instruments and observation for all three, with no change of behaviour: the columns each document names; the summary as a portable type with its proofs; the census of lists and pieces by class, and of the pieces that missed, whether the repeatable proof would take each and by which condition not (counts only); the request log. Then one baseline run by the owner | no figure exists for any of them (**doc**: `memory.md`, "Ledger": no measured run), and the replay is not built before its one deciding fact is counted. *Moved forward:* the request log (two runs over one route come before any streaming decision) and the summary |
| 1 | Memory: the missed page in work that is not repeated, in three changes (the completion path split so that marks are read before anything goes back; the round; admission before submission); then the stamps, the holder counts and extents; then release by fence value; then residency for what is reconstructible; then placement by capability | it carries the correctness fix; residency needs "in flight" and "named", and so does scheduling. *Moved forward:* the last-use value and the release queue are built here once, in fence values, though submissions are still waited for |
| 2 | Scheduling: the summary checked by marks; state kept per submission instead of per copy, behind its switch; a blocking fence wait and a wake source of ours; completion deferred where independence is proven; barriers | every overlap rests on step 1's stamps and on per-submission state |
| 3 | Streaming: the predictor against recorded logs; read-ahead in a bounded pool; the persistent cache; host DirectStorage by capability | it needs memory's bounds to be reclaimed under, and its log from step 0 |

Each behaviour has its own switch, off by default, with today's path as
the fallback (named in the three documents); so has each change to the
default submission path (release by fence value, state per submission).
With no switch only stamps and counts nothing reads to decide, and
columns, are added. Tested here: host tests of every decision that needs
no device, on synthetic lists and request sequences. Only the owner's
machine shows that a path runs on a driver, what it costs and whether
the picture is unchanged. **No gain is stated.**
