# Unified memory management: placement and residency

Where a resource may safely live, on top of the ownership, accounting and
paging that exist. Shared notions: [resource-model.md](resource-model.md).
The rules for touching an allocation, the inventory (rows T, G, V, C, R)
and the candidates (K1 to K11): [memory.md](memory.md), not repeated here.

**Nothing here was run.** Marks and paths as in resource-model.md: a file
and line in backticks is *read from the code*; **doc**, **inferred**,
**unknown** (with the run that would settle it) and **proposed** (this
page's design: not implemented, tested or measured) are written out. No
gain is stated: a change is kept only with a measured before and after.

## Two memories are not one

The console has one memory, and the code keeps that only for addresses: a
title GPU address is a CPU address (`gpu/lib.rs:7`). What a shader reads
there is a host copy, and every copy of title memory is in system memory
the CPU maps (`gpu/host.rs:3983-3993`), because the CPU reads the copies
back: snapshots, compares, copy-back (`gpu/shadow.rs:350-380, 443-452`).
Only textures are on the card (`gpu/resource.rs:1527-1604`). What a
shader's read of system memory costs, and a CPU read of video memory, are
**unknown**; the code expects the second to be slow and has a diagnostics
switch to measure the first (`gpu/host.rs:3954-3981`). Neither is free.

## Part 1. The missed page in work that is not repeated

| What happens today | Where |
|---|---|
| A copy exists only for memory a draw, a view or a root argument named | `gpu/memory.rs:31-33` |
| A page with no copy reads 0 and takes no writes; the shader marks the miss in its list's bitmap | `gpu/memory.rs:27-29` |
| After the wait each list is asked whether it missed, before anything goes back | `gpu/host.rs:1158-1161, 1577-1582` |
| A miss keeps what every program using title memory wrote out of title memory, in that frame and the next, whether its own submission missed or not | `gpu/memory.rs:340-357`; `gpu/host.rs:1582-1585` |
| What a submission that missed wrote is retaken at once; what one that did not miss wrote stays in the host copies, where the frame's later submissions compute from it, until the frame's end | `gpu/memory.rs:363-388, 643-646` |
| A page written by address goes back only if the title has not written that 64 KiB page since the copy took it; else the whole page keeps the title's bytes and the shaders' are lost, counted only | `gpu/shadow.rs:198-216, 747-750` |
| With `VICTORY_MEMORY_PAGING=1` the missed page and the pages after it in its aligned 4 MiB get a copy *after* that submission; a first reach into a 256 MiB region with no block names only the region, and the page one miss later, after a probe is added (96 at most) | `gpu/paging.rs:8-17, 27-35, 109-112, 1390-1410` |
| Admission stops for the frame after 256 pages or 16 new runs, and for 30 frames after a refused buffer; what was found of a page is kept 30 to 960 frames; a first look is a few system calls | `gpu/paging.rs:96-104, 119-129, 713-731` |
| Nothing runs again: a program the title runs once has lost its output | `gpu/paging.rs:19-25`; `gpu/memory.rs:887-906` |
| A page cannot be held when the tracker does not track it whole, another copy holds it in part, it lies at or above 2^40, or the bound is reached | `gpu/paging.rs:239, 555` |
| A read that returns 0 *without a mark* (the rest of a page held in part, an access past a copy's end) is seen by nothing, and by none of the options below | `gpu/memory.rs:48-55` |

| Source of an address | Known before the submission |
|---|---|
| A table view, a root view, a vertex or index view | yes: this is what makes copies today (`gpu/draw.rs:203-227, 564-570`) |
| A root constant | its value is in hand when recorded (`gpu/draw.rs:28-33`); that the program uses it as an address is known to the analysis and dropped (`shader/resource.rs:829-857`, "an address from a root parameter that is no address") |
| A pointer the program loads from memory | no: only by running the program |
| Which pages a list missed | after the wait, while its host lists still exist and before the title is told anything (`gpu/host.rs:1577`; lists are recycled after `execute` returns, `gpu/queue.rs:207-214`) |

### The options the code allows

| | A. Admit before submission | B. Run the missed piece again, inside its submission | C. Widen what is resident |
|---|---|---|---|
| What | the shader crate reports which root-constant dwords become an address; at submission those pages are admitted before the table is written | on a miss: give the missed pages copies, roll back what the piece wrote, run the same lists again, and only then go on to copy-back | admit to the end of the title's allocation instead of the end of 4 MiB; keep pages by last use |
| Reuses | the pager and its bounds (`gpu/paging.rs:1436`) | the verdict, the pager, the rollback (`gpu/watch.rs:858`), the table rewrite between submissions (`gpu/memory.rs:712-715`) | the pager; needs the extents of resource-model.md, section 2 |
| Guarantees | no miss for the page holding a root-constant address and the pages after it in its 4 MiB run, up to the first that cannot be or already is held, within the frame's admission limits (`gpu/paging.rs:1430-1470`) | for a piece proven *repeatable*, in a frame not already withholding, whose last round has no miss: that round computed with every page it reached present from its start, and what it wrote goes back as a run with no miss does today (**inferred**: outside the withholding window the rollback gives the copies title memory's bytes, which is what the first run started from, and GPU work is a function of its inputs) | nothing by itself: the first touch of memory nothing named is still a miss |
| Cannot | a pointer read from memory; an offset before the address or past the first stop; a page found earlier not to be title memory; more than 16 new runs or 256 pages in the frame; a page that cannot be held; a miss elsewhere in the frame, which still withholds everything | a piece not proven repeatable; a frame already withholding (a miss left unresolved in it or the frame before): the output is withheld as today; a page the title wrote meanwhile, which keeps the title's bytes whole, and a marked page partly in a root UAV's declared range (`gpu/memory.rs:99-101`); pages the pager does not admit, or admits later (not title memory, untracked, the per-frame limits, probes or table full) | one-shot work, on its own |
| Costs (none measured) | a new field of the translation, so every shader is translated once more; guards on pages never read | the piece's GPU time once more per round, towards the 250 ms warning and the 2 s reset (`gpu/host.rs:1540-1547`); first looks at pages under the submission lock | memory up to the pager's bound; a guard and a fault per held page the title writes (**doc**: `memory.md`, case G11) |

Not an option here: host buffers backed by the title's memory itself
(**doc**: `memory.md`, "Not candidates", G1).

### Proposed first: B, for repeatable pieces

Behind `VICTORY_MEMORY_REPLAY=1` (needs `VICTORY_MEMORY_PAGING=1`), off by
default. Then A, then C. Its guarantee alone does not need the pointer
before the program runs: the code describes the builder's misses as
record arrays and node pointers that later lists follow
(`gpu/memory.rs:344-349, 376-381`), a chain **inferred** to be over one
deep, its depth **unknown**. It changes no shader and no translation.

The unit is the piece, one `Host::execute`: every consecutive ready
segment of a title queue, with our upload list ahead
(`gpu/hold.rs:139-192`; `gpu/queue.rs:207-211`; `gpu/host.rs:1431-1439`).
The builder shares a list with other programs (`gpu/host.rs:1574-1576`),
so the proof and the cost are over all of it. **Repeatable** is proven
from the summaries of its lists (resource-model.md, section 4), derived
whenever the replay, the census or the scheduling record asks. All must
hold; a field that could not be filled fails (rule 10 there).

| # | Condition | Because a second run would otherwise repeat |
|---|---|---|
| 1 | every write, declared or marked, is to a host copy that takes title memory's bytes again exactly (a tracked copy through its tracker, `gpu/watch.rs:858`; an untracked one whole, once nothing is pending on it), to the list's own bitmap, or to its own scratch. A root UAV's write range is the rest of its copy, as in gpu-scheduling.md: in a program using title memory its writes past the declared 1 MiB go through the page table and are marked (`gpu/draw.rs:892-897`; `gpu/memory.rs:91-101`); elsewhere a root UAV that marks nothing (a copy with no mark entry, `gpu/shadow.rs:329-331`; a host root descriptor, `gpu/draw.rs:207-219`) fails the condition in the first version unless its copy ends inside the declared range | a write with no exact rollback (`gpu/shadow.rs:621-640`); bytes past 1 MiB that a rollback of the declared range leaves, read and written again by the second run |
| 2 | no render target, depth target or unordered texture; every texture named has the same read state first and last; no twin use | texels with no earlier image; states and twin sides already moved (`gpu/host.rs:915-925, 2146-2170`) |
| 3 | no `Copy` or `Run` step, no `Host` step without a declared range, no host write of a texture, no read-back | CPU work on title memory done before the first run (`gpu/host.rs:1094-1106, 1465-1475`) |
| 4 | no indirect execution, and no program that may write through any view of the heap (first version) | the work budget and the clamp (`gpu/host.rs:2449-2467`); writes that cannot be named (`shader/emit/mod.rs:491-496`) |

| Step | The round |
|---|---|
| 0 | It runs inside `Host::execute` after the wait, under the submission lock (`gpu/host.rs:1381-1383`) and before anything goes back (`gpu/host.rs:1645-1649`): the title sees nothing between rounds |
| 1 | Ask the lists whether they missed, as today. If none did: on as today the first time, to step 6 after a round |
| 2 | Go on as today, withheld, counted by reason, when: the switch is off; the piece is not repeatable; the frame is already withholding from another submission (the output would be withheld anyway, and the copies then hold withheld results a rollback would replace); the last round made no progress; or the rounds so far, their first looks at pages included, have taken 250 ms (**proposed**; the owner's to set) |
| 3 | Read the marks and give the missed pages copies through the pager, under its limits as they are: a region gets a probe and its page is named a round later; pages past the frame's limits or with a kept reason get none. *Progress* is a page admitted or a probe added. Today the marks are read inside the after-run closure together with the copy-back (`gpu/memory.rs:876-923`): reading them first is a change of its own, made before this one |
| 4 | Roll back: every range the piece declared or marked takes title memory's bytes again; snapshots and armed pages are reset; the lists' bitmaps are cleared |
| 5 | Write the tables, refresh the new copies and the rolled-back ranges, execute the same closed lists, wait. Back to step 1 |
| 6 | Before anything goes back: each marked page is asked whether the title wrote it since the copy took it (`gpu/shadow.rs:198-216`). If so, one more round after that page is retaken, within step 2's bounds; else the page keeps the title's bytes as today, counted, and the piece is counted as partial, not as resolved |

Only the last round's verdict reaches `note_submission`
(`gpu/memory.rs:335`): a miss a round resolved withholds nothing. Rounds
are not pointer hops: a region miss, a limit and a kept page each take
one. Each round has its own fence value. That a closed list may be
executed again once its first execution is complete is general knowledge
of Direct3D 12: **unknown** until a debug layer sees it.

### Tested here, and what the owner's run must show

Host tests, in the manner of `gpu/paging.rs:1219`. The proof is a
portable type; the round and the rollback are in Windows-only modules,
type-checked only: the model below tests the rule, not that code.

| Test (proposed) | Shows |
|---|---|
| The proof over synthetic summaries, by reason | an unknown field or an undeclared closure is never repeatable |
| A model run: a synthetic program follows a chain of pointers of depth `d` through synthetic title memory and writes results, once with every page present, once through miss, admit, roll back, run again; with a title writer on the same pages, a root UAV written past 1 MiB, a region miss before a page miss, and the frame's limits reached | title memory byte for byte the same over random chains and layouts; with no progress or a limit reached the result is today's (withheld), never a part; a kept page is reported as partial |
| The pager's bound tests, extended through rounds | the bound on host memory and the per-frame limits hold across rounds |
| Switch off | tables and decisions are what they were |

| The owner's run | Must show |
|---|---|
| 1. No switch, step 0's columns | which pipelines' writes are withheld and in how many frames each (`gpu/paging.rs:1529-1535` names each once today); of the pieces that missed, how many the proof would take and by which condition not (counts only). If none is one-shot, B has nothing to do and C is next |
| 2. `VICTORY_MEMORY_PAGING=1` | misses by reason with region misses apart from page misses, admissions put off (`later`, `full`), kept pages. Depth is not read from frames with new misses: probes, limits, new data and results dropped upstream of a miss (`gpu/memory.rs:363-374`) add frames too |
| 3. The replay, with the debug layer | no error on the second execution; rounds as a distribution by cause, which is where depth is read; refusals by reason; replays withheld anyway; partial ones; fewer withheld frames; the ray-traced scene captured; no crash now that the builder's writes reach title memory (**doc**: `progress.md`, the paging entry) |
| 4. With and without | its cost: `gpu_ms` and the frame-time columns |
| If the pieces that miss also render | they are not repeatable. A list cut at such a dispatch as at a wait (`gpu/list.rs:203-218`) does not help alone: a segment with no wait of its own joins the same piece (`gpu/hold.rs:139-192`). A piece boundary in the pump, as a signal makes one (`gpu/hold.rs:159-167`), is a design of its own behind its own switch, its extra submissions counted; or A leads |

### The paging's other limits

| Limit | Where | Direction (proposed) |
|---|---|---|
| Nothing at or above 2^40, though the runtime places memory up to 46 bits when asked | `gpu/paging.rs:239`; `hle/xmem.rs:79-86` | counted today; a wider table only if a run shows such memory (**unknown**) |
| A page held in part reads 0 past the copy, unmarked | `gpu/memory.rs:48-52` | extents: a copy rounded out to whole pages of its own allocation |
| An access is one lookup | `gpu/memory.rs:53-55` | the same; `cut` counts the exposure |
| A page is given up only when superseded or gone | **doc**: `memory.md`, G11 | last use needs a read mark for present pages: a translation change, later |

## Part 2. What may live where

By capability, never by model. A copy's host address is fixed when its
buffer is made (rule 7 of resource-model.md), so placement is decided
then; "moved" means a new copy made and the old one dying with its
holders, as a replaced host texture does (`gpu/resource.rs:1247-1249`).

| Kind (`memory.md` row) | Today | May also live | Never |
|---|---|---|---|
| Title memory (T1 to T3) | system memory, the title's | nowhere | discarded, trimmed or shrunk: the title can touch it again |
| Buffer copies, windows, view chunks, pages on demand (G1 to G3, G11) | system memory, CPU-mapped | CPU-visible video memory, where the device reports a GPU upload heap (`gpu/host.rs:3966-3977`), per kind, once a run shows that kind's CPU read volume, and under a cap ("Bounds"): such copies raise the use the texture pass evicts against (`gpu/resource.rs:1588-1601`) and are not themselves given up | on the card without a CPU mapping while the CPU reads them back |
| Lookup tables of a view heap (G6) | upload heap | with the copies they serve: the diagnostics switch leaves them behind and so under-measures (**doc**: `audit-2026-11-26.md`, A-P2) | |
| Host textures fed from title memory (V1) | the card, evicted by idle time and budget | unchanged | |
| Textures the host wrote; the newer side of a twin (V1, V2) | the card | unchanged | given up: nothing restores them (`gpu/resource.rs:1575-1581`) |
| Host structures (R2) | the card, under their own bound | unchanged | released while a reader's submission is incomplete (`gpu/rt.rs:70-74`) |
| Converted, recoverable data (C1, C3; the stream cache) | disk under the cache folder, some read whole into the heap | a mapped file (K1); the bounded cache of predictive-streaming.md | anything authoritative: the cache "may be deleted at any time" (`crates/victory-core/src/config.rs:18-22`) |

## Giving up and moving

A copy is given up or moved only when every row holds (**proposed**).
"No pending range" is not the test: the code leaves shader-written bytes
in a copy alone, with nothing pending, in the three cases of row 2.

| # | Condition | Where |
|---|---|---|
| 1 | **Reconstructible**: title memory alone restores it. For a texture, `evictable`. For a buffer copy the same rule: no shader has written it since title memory last matched it, kept as a *written* flag set by a `GpuWrite` step, by a list's marks and by view marks naming it, and cleared only when every page written went back and was taken | `gpu/resource.rs:1575-1581`; `gpu/shadow.rs:497-529` |
| 2 | Never while `withholding()` holds or a withheld range of it awaits its retake (`keep_writes` ends the pending range and the bytes stay for the frame's later submissions); never a copy a root UAV that marks nothing was bound to (it is written past the 1 MiB declared and copied back); never a copy with a page nothing is mapped at (copy-back leaves such pages and the tracker counts them as taken, so no refresh rewrites them) | `gpu/shadow.rs:457-459, 519-529`, `gpu/memory.rs:369-388`; `gpu/draw.rs:207-219, 932-934`; `gpu/shadow.rs:444, 496`, `gpu/watch.rs:19-21` |
| 3 | Its last use is complete (rule 1 of resource-model.md) | resource-model.md, section 6 |
| 4 | Not *named*: every holder count is zero but the page tables' own (tables are rewritten before the next submission reads them). A hold counts from the moment `find` returns: `find` drops the registry's lock before its caller records the host address and pushes the step, so steps and slots alone do not show it, and the lock alone does not close the race the texture pass accepts | resource-model.md, section 3; `gpu/memory.rs:712-715`; `gpu/shadow.rs:1113-1152`; `gpu/draw.rs:209-219`; `gpu/resource.rs:1682-1684` |
| 5 | A move needs all of the above too: while a list not yet reset holds the old copy, old and new could meet in one submission and a write through one not be seen through the other | `gpu/shadow.rs:1104-1109`; `gpu/paging.rs:53-57` |
| The pass | before a submission: idle copies first, then by *pressure* (resource-model.md) from its high mark to its low; the texture pass's budget is the card's and is not borrowed. Candidates, each through its case: windows, view chunks, pages on demand; a live buffer's copy only once a run shows idle ones worth making again | `gpu/host.rs:1389-1391`; `gpu/resource.rs:1588-1601` |
| Blocked | `VICTORY_MEMORY_EVICT`, until the holder counts exist: they are the tracking K8 calls missing | **doc**: `memory.md`, K8 |

## Pooling and suballocation

| Repeated allocation, or memory never returned | Where | Candidate |
|---|---|---|
| Loading buffers made and zero-filled per request | `hle/dstorage.rs:973-979`; **doc**: `memory.md` T5 | K3 |
| Refresh scratch made again by each worker thread | **doc**: `memory.md` G9 | K5 |
| Snapshot pool kept to 512 MiB with no decay | **doc**: `memory.md` G4 | K6 |
| Staging: the first 16 chunks never freed | `gpu/texture.rs:94-99`; **doc**: G5 | K7 |
| Windows never freed; view-copy chunks never removed | `gpu/shadow.rs:1159-1160, 1278-1372` | K8 |
| A host list per segment at its high-water mark | **doc**: `memory.md` G7 | K9 |
| A committed buffer per title buffer copy, rounded up to 64 KiB | `gpu/shadow.rs:1129-1132` | none yet: small copies from shared chunks, to be proposed with its case |
| Each pool has its own rule for "free again" | `gpu/texture.rs:94-99`; `gpu/placement.rs:7-11` | the last-use value of resource-model.md, section 6 |
| The candidates of `memory.md`: **none is approved here**; each still needs its written case and a measured before and after | K1, K2, K10: the disk tier. K3, K5: pooling (K3's bound is shared with streaming's read-ahead). K4, K11: neither | needs of the model: K6 an idle clock in frames; K7 the last-use value; K8 *named* and last use; K9 what a queue still holds after an early reset (`gpu/list.rs:275-280`) |

## Bounds

New long-lived structures (**proposed**), by the memory rule of
`../CLAUDE.md`; each has its case in `memory.md` before it is built.

| Structure | Owner in `account.rs` | Bound | Gauge | Lifetime |
|---|---|---|---|---|
| Pages on demand, also under B, A and C | `DemandPages` (exists) | the switch's megabytes: 256 by default, 4096 at most (`gpu/paging.rs:105-108`) | `own_demand_pages_mb` | until superseded or gone |
| Extents table | new | 16,384 extents of 16 bytes (256 KiB); past it extents are not recorded, counted, and C uses the 4 MiB run | new, in megabytes and entries | the run; only with `VICTORY_MEMORY_EXTENTS` |
| Stamps, holder counts, the written flag | none: words inside each copy's record | the copies' number | | the copy's |
| Copies in CPU-visible video memory | their kinds' owners | `VICTORY_MEMORY_PLACEMENT_MB`, by default an eighth of the reported local budget; past it new copies are made in system memory. A placed copy does not move (rule 7) | `own_copies_video_mb`, read beside `tex_remade` | the copy's |
| What waits in the release queue | its owners, counted while alive | what one waited submission names: empty after each completion | `release_queue_items` | to its fence value |

## Switches, instruments, tests

| Switch (proposed; off by default) | Behaviour | Fallback |
|---|---|---|
| `VICTORY_MEMORY_REPLAY=1` | option B | withholding, as today |
| `VICTORY_MEMORY_PREADMIT=1` | option A | the miss, then B or today |
| `VICTORY_MEMORY_EXTENTS=1` | option C, and the extents table | the end of the 4 MiB run |
| `VICTORY_RELEASE_BY_FENCE=1` | release by fence value (resource-model.md, section 6) | release when none is running |
| `VICTORY_MEMORY_EVICT=<kinds>` | giving up reconstructible copies of the kinds named (blocked on the holder counts) | they stay |
| `VICTORY_MEMORY_PLACEMENT=<kinds>` | those kinds in CPU-visible video memory where reported, under the cap | system memory |

| Columns (log text today, so `cargo xtask perf-summary` cannot compare them, `gpu/pacing.rs:184-189`) | When |
|---|---|
| `mem_missed_pages`, `mem_missed_regions`, `mem_admit_later`, `mem_kept_pages`, `mem_withheld_frames`, `mem_withheld_lists`, `mem_first_withheld_pipelines`, `mem_missed_repeatable` and `mem_missed_refused` by condition, `mem_demand_pages`, `mem_refresh_mb`, `mem_copy_back_mb`, `tex_given_up`, `tex_remade`, `tex_remade_soon` (counted and read by nothing, `gpu/resource.rs:1553-1556`); per kind of copy its count, bytes, bytes refreshed and bytes read back | step 0 |
| `replay_rounds` by cause, `replay_refused` by reason, `replay_withheld_anyway`, `replay_kept_pages` | with B |

The diagnostics feature already counts refresh, copy, snapshot, compare
and marked bytes (`gpu/copystat.rs:11-106`): promoted, not kept twice.

Tested here: every decision above as a pure function, a recorder racing
the giving-up pass among them. Only the owner's machine shows that a
device accepts a list twice and a GPU upload heap for these buffers,
every size and time, and that the picture is the same.
