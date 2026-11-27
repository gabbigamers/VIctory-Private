# Memory

The record of where a run's memory goes and what is being done about it.
The goal (A5 in [progress.md](progress.md)): the title runs stably inside
16 GB of physical RAM with room left for Windows and what else runs, at the
picture quality the title asks for.

Rules for this track:

* **The same bytes in two places are not, by that fact, a duplicate.** The
  title's memory, a host copy in system RAM and a texture in video memory
  can each hold the same data and each be needed: by a different reader, at
  a different time, or as the only way back to the data when another copy
  is given up. A copy is redundant only when the case below shows it.
* **Nothing is traded for memory.** Not shader correctness, not the
  integrity of a texture, not the accuracy of geometry, not the reliability
  of world streaming. Lower texture resolution, dropped mips, smaller
  targets or skipped work are not optimisations here.
* **Measured before and after, and tested.** Every change made for memory
  has a run before it and a run after it in the ledger, peak and sustained,
  and regression tests that fail if the behaviour it relies on changes. A
  change with no measured gain is taken out again.
* **No paging file as a fix.** A larger paging file raises the commit limit
  and changes nothing about what must be in RAM. A run that only works
  with one has not met A5.
* Host independence as everywhere ([gpu-compatibility.md](gpu-compatibility.md)):
  budgets follow what the machine and the device report.

## Before an allocation is removed or consolidated

Each of these is answered in writing, from the code and from tests, before
any allocation is removed, shared, shrunk, freed earlier or made
reclaimable. An answer of "not known" stops the change until it is known.

1. **Ownership, lifetime, purpose.** Who makes it, who frees it and when,
   and what it is for. A copy made for one purpose may have picked up
   others.
2. **Every consumer.** Each CPU reader and writer (the title's code, our
   runtime, other threads) and each GPU reader and writer (draws,
   dispatches, copies, shaders that compute an address or a heap index
   themselves), found by search and not by recollection.
3. **Can the guest touch it again?** The title's memory is the title's: it
   may read back what it wrote, rewrite it in place, map the same pages at
   a second address, move them, or hand them to a later request. If the
   title can read or write a range again, its bytes must be there and
   current when it does.
4. **Coherency and synchronisation.** Which copy is the truth at each
   moment, how a write to one reaches the other, and what order the code
   relies on. Where both the title and the GPU write, both directions.
5. **Asynchronous execution.** A command list recorded now runs later, and
   the GPU may still be reading a resource after the call that used it has
   returned. Nothing is freed, moved or rewritten while a submitted list,
   a list held back for another queue, an in-flight copy-back or a
   presented frame can still refer to it. The lifetime that counts is the
   last use on the GPU, not the last call.
6. **The way back.** If the allocation is given up, how exactly its
   contents are restored when wanted again, from what source, at what
   cost, and what happens if that source has changed or gone in the
   meantime. Data that cannot be reconstructed is not given up.
7. **Identical rendering.** The frames are the same with and without the
   change: the shader probes and tests pass, captures of the reference
   scenes match, and no draw is dropped or delayed that was not before.

## How memory is reduced, in order of preference

1. **Remove what is demonstrably redundant.** A copy that the case above
   shows has no reader, no writer and no role in restoring anything.
2. **Reuse temporary buffers.** Scratch, staging and decode buffers made
   and dropped per request, or held at a high-water mark that one request
   set.
3. **Evict safely.** Give up what can be restored exactly, least recently
   used first, under a budget the machine reports, and only when it is not
   in use on the GPU (rule 5). Host textures are already given up this way
   against the video-memory budget.
4. **Do not hold caches in RAM that the system can hold for us.** A file
   read whole into private memory can be a mapping the system pages from
   the file.

Not done: reclaiming memory on the expectation that it will not be wanted
(discarding title pages, trimming the working set by force, shrinking
pools below what the title asked for). Where the answer to question 3 is
"it can", the memory stays.

## Tracking this needs

Reducing copies safely means knowing things about a range or a resource
that the runtime may not record today. Where a case needs one of these and
it is missing, the tracking is built first, with tests, as its own change:

* **Ownership**: which allocation a range belongs to, what the title asked
  for it (its flags, protection and memory type), and which host resources
  were made from it.
* **Residency**: whether a host copy exists, where it lives (system RAM,
  the card), and whether it is in use by a list that has not finished.
* **Dirty state**: which side wrote last and what has not yet reached the
  other, for writes by the title and for writes by the GPU, and for reads
  where a read is what makes a copy needed.

## What a memory change comes with

* The case above, written in the "Cases" section below, with every
  consumer cited as file and line.
* Regression tests: for the behaviour the change relies on (the lifetime,
  the ordering, the restore path), and for the picture where a test can
  show it. Logic tests run on any host; anything that needs a device is
  listed as unverified until it has run on one.
* A before and an after in the ledger: peak and sustained working set,
  peak commit, paging-file growth, lowest free RAM and video memory, from
  `cargo xtask perf-summary after.csv --against before.csv`, on the same
  scene, with nothing worse on the correctness or stability tracks.
* A way to turn it off for one run, so a suspected regression can be
  bisected to it.

## Four quantities

One number called "memory" hides four different things. They are recorded
separately everywhere in this project and are never added together.

| Quantity | What it is | What limits it | Where a run reports it |
|---|---|---|---|
| **Commit** | Memory the process has been promised, whether it has touched it or not | RAM plus paging files (the commit limit) | `commit_mb`, `peak_commit_mb`; machine: `system_commit_mb`, `commit_limit_mb` |
| **Working set** | The process's pages that are in physical RAM now | The machine's RAM. This is the quantity A5 is about | `working_set_mb`, `peak_working_set_mb`; machine: `physical_mb`, `physical_free_mb` |
| **Video memory** | On the card, or (shared) in system RAM that the GPU reads | The card's memory and the budget Windows gives the process | `video_mb`, `video_budget_mb`, `video_shared_mb` |
| **Paging file** | What the machine has had to move out of RAM | Disk; every use is time lost to paging | `pagefile_mb`, `pagefile_size_mb`; `page_faults_per_s` |

What follows from the definitions, before any measurement:

* Commit can be far above RAM without anything being wrong: reserved-and-
  committed memory that is never touched costs no RAM. So a commit figure
  does not say whether a run fits 16 GB. The working set and the machine's
  free RAM do.
* Commit minus working set is memory that is either untouched or paged
  out. Which of the two it is shows in the paging file's growth over the
  run and in the page-fault rate.
* Whether Windows charges a process commit for video memory on the card is
  the operating system's behaviour and cannot be read from this code. A run
  settles it: compare commit with the sum of what the other counters own.

What each counter in the log does and does not count (from the inventory;
the operating-system behaviour in the first two is inferred, not measured):

* `commit_mb` is the process's *private* commit. The title's largest pool
  is held as sections backed by the paging file, which Windows charges to
  the machine's commit and not to a process's private commit. So
  `commit_mb` leaves that pool out, and `system_commit_mb` has it.
* `working_set_mb` is the whole working set: it includes section and
  mapped-file pages, and a page shown at two addresses may be counted at
  both. It is an upper figure for the process's RAM.
* For these two reasons no figure is derived by subtracting one of them
  from the other, and "does it fit" is read from the machine's side:
  `physical_free_mb`, the paging file's growth and the fault rate.
* `physical_free_mb` counts standby (file cache) pages as free, as Windows
  does. It shows RAM running out; it cannot show how much the file cache
  holds.
* `video_shared_mb` is every allocation the process has in system memory
  for the GPU, together: buffer copies, mapping windows, staging, lookup
  tables, list scratch. It cannot be read as any one of them.
* `uploads_mb` counts the title's own list upload blocks only, not
  Direct3D upload heaps. `host_textures_mb` is live host textures with
  depth twins, without the swap chain, scratch buffers, the upscaler
  library's textures or free space in the texture heaps.

## Baseline

| Date | Source | Machine | Build | Figure | Which quantity |
|---|---|---|---|---|---|
| before 2026-11-26 | owner-reported | target (16 GB RAM, RTX 5070 Ti 16 GB) | not recorded | about 27 GB "reported memory", at about 15 fps | **not known**: 27 GB cannot be the working set of a 16 GB machine, so it is commit or a tool's sum of several counters |
| 2026-11-25 | project-reported (README) | 32 GB RAM, RX 6700 XT 12 GB | not recorded | about 33 GB of commit, about 10 GB of video memory | commit; video |

The owner's figure establishes that the title runs on the target machine
with a commit of that order. It does not yet say how much of it was in RAM,
how much the machine paged, or what owned it. The first run with the
performance log (below) replaces it with all four quantities.

## Measuring

The procedure of "Measuring" in [progress.md](progress.md), with
`VICTORY_PERF_LOG` set. Then:

```
cargo xtask perf-summary run.csv
cargo xtask perf-summary after.csv --against before.csv
```

The first prints the run's figures: peaks over the whole run, and sustained
values as medians over the world's periods (so loading and menus do not
dilute them). The second prints each figure before and after and names what
got worse. Neither needs Windows or anything of a title's; the CSV holds
measurements only.

For each change made for memory, the same scene is run before and after at
the same commit apart from the change, and both summaries go into the
ledger. "Sustained working set" is the median working set over the world's
periods; "peak" is the highest the system recorded.

A run passes A5 when, over at least 30 minutes in the world: the working
set stays flat; the machine's free RAM does not fall to the point of
paging (the paging file does not grow and the fault rate stays low); and no
commit refusal is logged, with the paging file at the Windows default.
These are proposed thresholds until the first measured run shows what the
numbers look like.

## Ledger

Measured runs only. MB.

| Date | Commit | Change | Scene | Working set, peak | Working set, sustained | Process commit, peak | Paging file growth | Free RAM, lowest | Video, peak | Result |
|---|---|---|---|---|---|---|---|---|---|---|
| | | | | | | | | | | no measured run yet |

## What allocates

An inventory of every allocation of about 1 MB or more, or unbounded, in
three scopes, each read by one engineer and checked claim by claim by a
second (2026-11-26, commit `fcb8bac`; 67 claims: 40 confirmed, 27 corrected
in part, none refuted; 7 sites added by the checkers). **Nothing was run.**
Sizes are constants and formulas in the code; figures quoted from code
comments are marked as the code's own; whether pages are in RAM is
inference everywhere.

"Same bytes elsewhere" states a fact about storage. Whether that makes
either copy removable is a different question, answered only by a case.

### The title's memory (`victory-hle`)

Nothing is committed ahead of the title asking, and nothing enforces the
console's budget; the console mode changes only the figure reported back.

| # | What | Kind | Size | Lifetime | Same bytes elsewhere | Counted |
|---|---|---|---|---|---|---|
| T1 | Physical pages: the pool streamed textures are decoded into and that is mapped into the GPU heap | section backed by the paging file | 64 KiB a page as the title asks; the code's comment records one request of 6.1 GiB | committed when mapped into a view, not when touched; a free only drops table entries, page numbers are never reused, each call makes a new section, so nothing is returned until every page and view of a section is gone | host textures and buffers uploaded from it. The title can read, remap and rewrite these pages, and shaders read title memory by address | no counter at all; not in `commit_mb` |
| T2 | The title's own pools (`XMemVirtualAlloc`) | private commit | exactly as asked, 4 KiB granularity | the title's own free really returns it | host copies made from these ranges | reservations at debug level only |
| T3 | The title's heap and stacks through Windows directly, and what host libraries it drives allocate | private commit | unknown from this code | the title's | none known | unseen |
| T4 | CPU-side memory standing in for each committed resource and each Direct3D heap the title makes | the title's own pool through its allocation hook; the process heap for heaps | the resource's console size; the heap's size | with the resource; zero-filled whole when made, so touched | the host resource holds the contents; the GPU layer reads and writes this memory back | diagnostics build only, and undercounted |
| T5 | Loading buffers: compressed source, intermediate, BCPack records (1.5 to 2 times the destination), fallback destination | process heap | per request | freed when the request ends; not pooled | two forms of one texture during a decode | only the compressed source counts against the 32 MiB in-flight limit |
| T6 | Start-up: whole-file buffers while the executable and DLLs are prepared | process heap | two copies of the file at once, three for a DLL; the code's comment gives 114 MB for the executable | transient; repeats for DLLs loaded during play | the prepared file on disk | none |
| T7 | Save data read and written whole | process heap | up to twice the blobs read (quota 256 MiB) | transient, rare | the title's buffer | none |

Smaller: the audio heap, bookkeeping tables, reservations and guard
ranges (address space only).

### The GPU layer in system memory (`victory-gpu`)

| # | What | Kind | Size | Lifetime | Same bytes elsewhere | Counted |
|---|---|---|---|---|---|---|
| G1 | A host copy of each title buffer a draw or a heap view uses | GPU memory in system RAM | the whole buffer rounded up to 64 KiB, whatever range is used | never released while the title holds the buffer; no eviction covers buffers; outlives the title's release while a list, a heap slot or a slot record holds it | the title's buffer. **Needed today**: the bytes are equal only for pages refreshed since the title's last write, and during a submission with shader writes the copy is the only holder of the new bytes until they are copied back | only inside `video_shared_mb` |
| G2 | Mapping windows for memory outside any buffer object | GPU memory in system RAM | 4 MiB at least each | never freed; unbounded in number; one stray root address makes one for the rest of the run | title memory | none |
| G3 | Copies for views off their buffer's element grid | GPU memory in system RAM | chunks of 16 MiB | chunks are never removed | the title's bytes; the enclosing buffer's copy there may be stale | none |
| G4 | Snapshots taken before a submission to find what shaders wrote | process heap | live: unbounded per submission; kept pool up to 512 MiB | no decay, no shrink | transient copies | none |
| G5 | Texture staging for uploads | upload heap (system RAM) | chunks of 32 MiB; up to 16 kept (512 MiB) once any submission needed them; larger buffers pooled to 64 MiB | the first 16 never freed | the texture in title memory and on the card | none |
| G6 | Per host view heap: stride table, mark table, title-memory page table, address table, slot vectors, and the descriptor heap itself | upload heap and process heap; the heap is runtime and driver memory | 8 MiB + 16 MiB + 16.8 MB + 16 B a slot + vectors, sized for 2^21 slots whatever the heap's own count; the code's comment gives over 100 MB of commit for the descriptor heap | with the heap | none | none |
| G7 | Per host list: command allocators, scratch, upload and readback buffers | runtime memory and upload heap | scratch chunks 64 KiB doubling to 4 MiB, never freed; one buffer per title copy | one host list per segment a title list has ever had, kept for the title list's life at its high-water mark | none | none |
| G8 | The title's list upload blocks | private commit | as asked | until the list's reset | none | `uploads_mb` |
| G9 | Refresh and detile scratch | process heap | the largest texture refreshed; kept per thread up to 32 MiB | worker threads end with each call, so theirs is allocated and zero-filled again each time | transient | none |
| G10 | Tables of views, placed-texture records, tracked pages | process heap | grows with what the title describes; views without a resource object are never dropped | the run | none | none |
| G11 | Copies of 64 KiB title pages a translated shader reached by address and no other copy held, and of the pages after them in their aligned 4 MiB (`VICTORY_MEMORY_PAGING=1`; off by default, then zero) | GPU memory in system RAM | one buffer of 4,194,304 bytes for each aligned 4 MiB of title memory with such a page; at most `max(1, floor(M / 4))` buffers for `M` megabytes asked (default 256: 64 buffers, 268.4 MB; at most 4,096: 4.29 GB) | a page from the submission that missed it (or the page before it) until another copy has it or the title gives the memory up; a buffer until the last copy in it is gone | title memory. **Needed while held**: during a submission the copy is the only holder of what shaders wrote there | `own_demand_pages_mb` |
| G12 | Per host view heap: which slots hold a view of a ray-tracing structure, and which structures those name (`descriptor/structures.rs`) | process heap | two maps, about 30 bytes an entry each; at most 2^14 slots a heap are followed (under 1 MiB), none until a title makes such a view. A slot past that is counted and the heap's list says it is not complete | with the host heap; an entry goes when its slot is written again or a table's slots are given back | none | `own_structure_slots` (entries, not bytes) |
| R1 | Build descriptions kept by the ray-tracing capture (`rtcapture`), only with `VICTORY_RT_CAPTURE` on | process heap | one entry a destination: at most 64 B a geometry and 72 B an instance; at most 65,536 entries and 64 MiB, more is refused and counted; 32 B for each address a structure was made at, at most 65,536 addresses (2 MiB), more is not kept and counted; up to 36 MiB more for the length of the call that reads the largest description allowed | an entry until the destination is described again, a copy or a failed call names it, or its memory is remapped; an address until its memory is remapped; both stay for structures the title stops using | the title's descriptions, which it may rewrite | `own_rt_capture_mb` |

### Video memory and caches (`victory-gpu`)

| # | What | Kind | Size | Lifetime | Same bytes elsewhere | Counted |
|---|---|---|---|---|---|---|
| V1 | Host textures | on the card: heaps of 64 MiB for plain textures up to 16 MiB, committed resources for the rest | the title's description in the host format | given up when idle for 300 frames and 120 s, and least recently used first above 90 % of the device's budget down to 85 %. Only textures fed from title memory are given up: ones the GPU wrote, and ones the title filled by a GPU copy, never are | the title's memory, for those fed from it. That copy is the way back after one is given up | `host_textures_mb`, `texture_heaps_mb` |
| V2 | Depth twins: a second texture per plane for depth written through unordered views | on the card | the depth texture's extent | with the depth texture; never given up | not the same values at all times: the code tracks which side is newer. **Needed** | inside `host_textures_mb` |
| V3 | Two host textures over one memory where the title views it block-compressed and plain, or a later view needs more uses | on the card | one more texture of the memory's size | while title textures hold the old one | the same memory in another format | none separately |
| V4 | Swap chain, scratch for twins, buffers for alias copies | on the card | small or transient | | | none |
| V5 | What the upscaler library makes on our device | on the card | not in this code | the library's | | none |
| C1 | The pipeline library file | process heap | the file read whole; the code's own figures: about 1.7 GB on NVIDIA, small on AMD | kept for the process: Direct3D needs the bytes to outlive the library. Hashed whole at start | the file on disk | size and load time logged |
| C2 | A save of the pipeline library | process heap | the whole serialised size again, on top of C1, under the library's lock | transient; runs when pipelines were stored, when the first use of any pipeline is recorded, and from idle workers every 20 s | C1 | none |
| C3 | Every shader container the title passes, and its translation | process heap | container plus DXIL each | a static map that only inserts; after translation the DXIL is read only when a pipeline is made | the shader cache on disk | none |
| C4 | Live pipeline state objects | driver memory | not in this code | while the title's pipeline or a list holds them | | counts only |
| R2 | Host ray-tracing structures of the experimental backend (`crate::rt`; nothing unless `VICTORY_RT_BACKEND` turns it on) | on the card: one committed buffer a structure | what the device asks for each, rounded up to 64 KiB; bounded together at a quarter of the device's budget (64 MiB to 2 GiB) or `VICTORY_RT_STRUCTURES_MB` | while the structure's owner holds it, then until every submission that may read it has run | none: built from geometry, not a copy of it | `own_rt_structures_mb` |
| R3 | That backend's other video memory: build scratch, its buffer for compacted sizes, and the output buffers a validation run's dispatches write | on the card | scratch in chunks of 4 MiB or one build's need, at most 256 MiB at once, 32 MiB kept idle; outputs as asked | scratch is reused once the submission that used a chunk has run, and idle chunks beyond what is kept are released; an output with its holder | transient | `own_rt_scratch_mb` |
| R4 | That backend's buffers in system memory: instance descriptions, geometry a validation run uploads, read-backs | upload and read-back heaps | instance chunks of 1 MiB, at most 64 MiB at once, 4 MiB kept idle; the rest as asked | instance chunks as R3's scratch; the others with their holder | instance descriptions are written from the caller's | `own_rt_buffers_mb` |

### What is counted now

Since 2026-11-26 the runtime counts what it allocates, by owner
(`crates/victory-gpu/src/account.rs`). Each owner is a column of the
performance log holding its highest value in the period, and
`cargo xtask perf-summary` reports each one's peak. **None of this has run:
the counting sits in Windows-only code and is type-checked only.** The
first run with the log is also the first check of these figures.

| Column | Row | What it counts | Not in it |
|---|---|---|---|
| `own_title_pages_mb` | T1 | pages the title holds by number, 64 KiB each | a free returns no memory until the section closes |
| `own_title_pages_mapped_mb` | T1 | distinct pages committed through any view, until their section closes | can exceed the column above after a free |
| `own_title_sections` | T1 | sections whose handle is open (a count) | |
| `own_loading_mb` | T5 | source, intermediate, BCPack records and fallback buffers of requests in flight | the decoder's small history tables |
| `own_buffer_copies_mb` | G1 | whole-buffer host copies | a copy kept alive only by a view heap slot after its record is gone: a lower figure |
| `own_windows_mb` | G2 | mapping windows | |
| `own_view_chunks_mb` | G3 | whole view-copy chunks | |
| `own_snapshots_mb` | G4 | snapshot buffers handed out, and pages saved when the title writes an armed page | |
| `own_snapshot_pool_mb` | G4 | snapshot buffers kept for reuse | |
| `own_staging_mb` | G5 | staging chunks and larger buffers, in use or kept | |
| `own_heap_tables_mb` | G6 | stride, mark, page and address tables of view heaps | the descriptor heaps themselves; a table a list still names after its heap is gone |
| `own_list_buffers_mb` | G7 | scratch, upload, readback, argument and budget buffers of host lists | command allocators and lists |
| `own_pipeline_library_mb` | C1 | the pipeline library's bytes held for the library | |
| `own_pipeline_save_mb` | C2 | the buffer a save serialises into, while it lives | |
| `own_shaders_mb` | C3 | container and DXIL bytes kept in the shader map; with each translation, the texts of what it asks of the host's waves; the count of programs by wave size and reason (`waves.rs`: at most 64 kinds, a few kilobytes, kept for the process) | the translations' other tables |
| `own_demand_pages_mb` | G11 | whole 4 MiB buffers of pages held on demand, from a buffer's first copy until its last copy is gone | the maps and counters beside them (process heap, bounded: see the case below) |
| `own_structure_slots` | G12 | host view heap slots that hold a view of a ray-tracing structure, as far as they are followed (a count) | slots past 2^14 a heap; the fixed tables of what was logged (static, 400 bytes) |
| `own_rt_capture_mb` | R1 | the capture registry's entries and the addresses structures were made at; 0 unless `VICTORY_RT_CAPTURE` is on | the registry's own map nodes; a description while it is read |
| `own_rt_structures_mb` | R2 | result buffers of host ray-tracing structures, live and waiting for their last reader | 0 unless the experimental backend is on |
| `own_rt_scratch_mb` | R3 | build scratch chunks, the compacted-size buffer, a validation run's output buffers | on the card: not to be added to the system-memory owners |
| `own_rt_buffers_mb` | R4 | instance-description chunks, uploads and read-backs of the backend | its query heap and command allocators |
| `private_working_set_mb` | | the process's working set without section and mapped-file pages | not given before Windows 10 22H2 |

A release of more than was counted leaves the gauge at zero and is noted;
a run then logs which owners are wrong. What a run has to show before a
figure is believed: `own_loading_mb` is 0 whenever streaming is idle;
`own_snapshot_pool_mb` never exceeds 537 (512 MiB); `own_pipeline_save_mb`
is 0 except during a save; `own_title_pages_mb` rises by exactly 64 KiB a
page allocated; `own_demand_pages_mb` is 0 without `VICTORY_MEMORY_PAGING`
and with it never exceeds the figure the start-up line gives (268 at the
default); no owner is reported as miscounted. With the diagnostics
switch that puts buffer copies in video memory, the owners marked as GPU
memory in system RAM are on the card instead, and their figures are not
comparable with other runs.

Still not counted: T2 and T3 (the title's own pools and heap: a walk of the
address space would be needed), T4, T6 and T7, G10's tables, the per-list
written-pages bitmaps (up to about 17 MB pooled), the descriptor heaps,
the swap chain and what the upscaler library makes (V4, V5), and live
pipeline state objects (C4).

Also recorded: the runtime's commit-refusal message advises a larger
paging file (`xmem.rs`); there is no check of commit or RAM at start; the
title is told the process's private commit as its own memory use, which is
neither the title's use nor the whole process's.

## Candidates

Not ranked by measurement and **none approved**. Each becomes a change only
through a case, after a run has shown the size its counter reports.
Ordered by the preferred means.

| # | Candidate | Kind | Quantity it would lower | Open questions of the case | Validated where |
|---|---|---|---|---|---|
| K1 | C1: map the pipeline library file read-only instead of reading it into the heap | 4 map | commit by the file's length; resident pages become file-backed and droppable | does the device accept a mapped blob on every driver; the hash at start touches every page once; C2's save must not rewrite a file that is mapped | Windows, then a title run |
| K2 | C2: serialise a save into a mapped temporary file, and save only when pipelines were stored | 2 reuse | a transient private buffer of the library's size | ordering against workers storing pipelines; a crash mid-save must leave the old file | Windows |
| K3 | T5: bound and reuse loading buffers per worker; count all of them against the in-flight limit | 2 reuse | transient peaks while streaming | none for correctness if sizes and contents are unchanged; the limit must not stall streaming | Windows (the module is Windows-only) |
| K4 | T6: prepare the image in the buffer given and compare the published copy by length and hash | 2 reuse | start-up and late-DLL peaks | none found | any host (`prepare.rs`, `publish.rs` are portable) |
| K5 | G9: keep refresh scratch for worker threads | 2 reuse | allocation and zero-fill churn | none for correctness | Windows |
| K6 | G4: let the snapshot pool decay when unused | 3 evict | up to 512 MiB kept | none for correctness: pool buffers hold no live data | Windows |
| K7 | G5: release staging chunks idle for some submissions | 3 evict | up to 512 MiB kept | a chunk may still be read by a submitted list: last GPU use, not last call | Windows, then a title run |
| K8 | G2, G3: release windows and view chunks nothing refers to | 3 evict | unbounded growth over a long run | every holder (lists not yet reset, heap slots, slot records) must be found; needs residency tracking that does not exist | Windows, then a title run |
| K9 | G7: release host lists a title list no longer needs | 3 evict | per-list high-water memory | lists held by a queue after an early reset | Windows, then a title run |
| K10 | C3: drop translated DXIL once no pipeline is being made from it, re-reading it from the shader cache when one is | 3 evict | process heap by the code size of every shader seen | the cache file is the way back: it must exist and match; later pipelines reuse shaders | Windows, then a title run |
| K11 | G6: size the per-heap tables for the heap's own slots | 1 remove | tens of MB per view heap | the 2^21 bound is compiled into shaders (G3 in gpu-compatibility.md): the two change together | any host for the layout; a title run |

Not candidates, by the checkers' own reading of the rules above:

* **G1, the host copies of title buffers.** Needed today for the reasons in
  the table. Backing host buffers with the title's memory directly would
  remove them; one probe in a reference repository shows a committed
  region opened as a Direct3D heap on one NVIDIA card in a virtual machine,
  and nothing more than that. It is a design question for the GPU layer,
  not a memory optimisation, and is not scoped here.
* **V2, depth twins**, and the host textures of V1: each side is read.
* **T1 and T2, the title's memory.** The title can touch it again. Making
  uploaded pages discardable would need read tracking the guards do not
  have and an exact refill; a wrong refill is a corrupt texture. Ruled out
  under "Not done" above.
* Unbuffered file reads and exact-size default allocations: the checkers
  found no demonstrated gain for RAM (standby pages are already free to
  Windows; this title supplies its own allocator).

## Cases

One per change, written before the change is made. Template:

```
### <allocation> : <what is proposed>

Kind: remove / reuse / evict / map
1. Ownership, lifetime, purpose:
2. Consumers (CPU, GPU), each with file:line:
3. Can the guest read or write it again, and how do we know:
4. Coherency and synchronisation relied on:
5. Asynchronous use: what can still refer to it after the call returns:
6. The way back: restored from what, when, and if that source changed:
7. Identical rendering: which tests and captures show it:
Tracking needed that does not exist yet:
Regression tests added:
Switch to turn it off:
Before / after: (ledger rows)
Decision:
```

### G11, pages of title memory held on demand : a new owner, off by default

Written 2026-11-27 with the change that adds it. The template is for
removing an allocation; this case adds one, so each question is answered
for the new copies, and the way they are given up again is the part the
template was written for. **Nothing here has run on a device**: the
bookkeeping and every decision that needs no device are tested on any host
(`crates/victory-gpu/src/paging.rs`), the Direct3D side is type-checked for
Windows only. File and line numbers are as of this change.

Kind: an addition (new copies); giving a page up is kind 3, evict what can
be restored exactly.

Why it exists. A translated shader that reads or writes title memory at an
address it computed finds the host copy through a page table
(`crates/victory-gpu/src/memory.rs`). A copy existed only for memory a
draw, a view or a root argument had named (`shadow.rs`, `find`). A page
reached only through a pointer the shader read had none: it read 0, its
writes were dropped, the miss was marked, and nothing ever made a copy for
it, so the same program missed it in every frame, and every miss keeps what
all programs using title memory wrote out of title memory for two frames
(`memory.rs:350`, `withholding`). With the switch, such a page gets a copy.

What it does not do. **Nothing runs again.** The list that missed keeps its
writes out of title memory as before and the copies drop them; the page is
there for the submissions that follow. A program the title runs every frame
reads it then. A program the title runs once (a build, an unpack in place)
that touches a page with no copy has lost its output for good, with the
switch as without, and every first touch of fresh memory is such a miss. So
the switch can only help work that is run again. Making pages present
before a dispatch runs (from the addresses its root arguments name), or
running the list again, would be needed for one-shot work; neither is done.
The lists whose writes were withheld are counted by pipeline in the misses
line, and each pipeline is named once in the log when it first happens, so
a run shows whether one-shot programs are among them.

Cost in time, not measured. Each held page is tracked like any copy's: the
change tracker makes the title's page read-only when the copy takes it, and
the title's next write there faults once and has the page copied again
before the next submission. The pages taken after a missed page are tracked
whether or not a shader ever reads them, so memory the title writes every
frame next to a structure costs a fault and 64 KiB a frame per page while
it is held. A first copy is 64 KiB a page, at most about 20 MB a frame.

Worst-case size, as a formula. With `M` the megabytes asked for
(`VICTORY_MEMORY_PAGING_MB`, default 256, cut to 4,096; a value that is not
a number above 0 is refused with a warning and the default used):

```
buffers  <= max(1, floor(min(M, 4096) / 4))    (paging.rs:151, limit_runs)
bytes    <= buffers x 4,194,304                (paging.rs:156, worst_case_bytes)
default:    64 x 4,194,304 = 268,435,456 bytes (256 MiB)
largest:  1024 x 4,194,304 = 4,294,967,296 bytes
switch off: 0
```

One buffer holds the pages on demand of one aligned 4 MiB of title memory
(a run), each at its own offset, so a buffer may hold one page or 64. The
bound holds because a page is taken only if its run has a buffer or fewer
buffers than the limit exist, and a buffer goes only with the last copy in
it (`paging.rs:675`, `Pager`; tests `a_run_is_free_only_when_its_last_copy_is_gone`,
`the_limit_bounds_the_host_memory`, which tries 16,000 steps of misses,
pages given up and copies going in random order). Per frame at most 16 new
buffers (64 MiB) and 256 pages, counting the pages taken with a missed page
after it in its run, and up to 63 more for the miss that reaches that
count (its run is finished). Beside the
buffers, in the process heap and each bounded: a map entry per held page
(at most 64 per buffer), what was found of at most 8,192 missed pages, 96
probed regions and at most 4,096 set aside, 256 kinds of miss and 32
pipelines already logged, and per view heap two descriptor slots for each
buffer. The page tables do not grow: they are of one size whatever they
name (G6).

1. Ownership, lifetime, purpose: owned by the paging state in
   `memory.rs:414`, `Paging`: the copies in its map of held pages, the
   buffers in its map of runs, counted by a `Held` on `Owner::DemandPages`
   per buffer (`memory.rs:394`, `Run`). A page's copy is made after the
   submission that missed it (`paging.rs:1380`, `note_misses`, from
   the list's after-run work in `memory.rs`, `bind`), only for a page that
   can be read, that the change tracker tracks whole and that no copy holds
   in whole or in part (`paging.rs:555`, `classify`;
   `shadow.rs:293`, `Shadow::page` refuses a page not tracked whole). The
   pages after it in its run are looked at in turn and given copies with it
   up to the first that cannot have one (`paging.rs:1436`, `admit`). It is
   given up (`memory.rs:616`, `give_up`) when a table names another copy for
   its page (`memory.rs:716`, `rewrite`) or when the frame-end check finds
   nothing of the page readable any more (`memory.rs:628`, `paging_frame`: 128
   held pages a frame, in turn). The copy then lives until every page table
   that named it is rewritten; its drop reports its page
   (`shadow.rs:768`), its place in the buffer is
   zeroed, and a buffer is freed with the last copy in it
   (`memory.rs:555`, `take_gone`). Purpose: the bytes shaders read and write at
   the page's addresses.
2. Consumers (CPU, GPU), each with file:line. CPU writers of the copy: the
   refresh before a submission, from title memory, of the pages the title
   wrote since the copy took them (`memory.rs:816`, `prepare`, into
   `shadow.rs:660`, `update_tracked`), and the zeroing of a place that holds
   no copy: when its copy is gone, and when the page is missed while the
   page before it is held, since a write that ran past that page's end (the
   miss) left bytes there (`memory.rs:478`, `clear`). CPU readers of the copy: the copy-back after
   a submission, which compares the page with title memory and writes title
   memory (`shadow.rs:198`, `copy_back_page`, called from `bind`'s
   after-run work in `memory.rs`). GPU readers and writers: translated
   shaders through the page table's raw views, one view pair per buffer per
   view heap (`crates/victory-shader/src/emit/lower/memory.rs`,
   `Lower::memory`; the pair is made in `rewrite`). No one else:
   `shadow::find` never returns such a copy, so no draw, no title view and
   no root view reads it, and it takes no bits of the views' written-pages
   bitmap.
3. Can the guest read or write it again, and how do we know: the copy is
   not the guest's memory and replaces none of it. The title reads and
   writes its own page as before. Its writes reach the copy through the
   change tracker at the next refresh (`watch.rs`; the copy's tracker is
   made by `track_copy` in `Shadow::page`). What shaders wrote reaches the
   title after the submission, before the bottom-of-pipe writes that tell
   the title the GPU is done (`host.rs`, the completion of `execute`).
4. Coherency and synchronisation relied on. Title memory is the truth
   except between a submission's start and its copy-back, when the copy
   holds what shaders wrote. Who may write the copy: shaders, the refresh,
   and the zeroing of an empty place; nothing else. The rules are those of
   every host copy and none is new: a page the title wrote since the copy
   took it keeps the title's bytes and loses the shaders'
   (`shadow.rs:198`, `copy_back_page`; now counted, in the misses
   line, where it was logged once); a submission that missed anything, and
   every list using title memory in that frame and the next, keeps its
   writes out of title memory and the copies drop them
   (`memory.rs:369`, `drop_withheld`; `memory.rs:383`, `retake_dropped`).
   New:
   * One place for a page's bytes. A page's copy stands back for every
     other copy the table would name for its page, whole or in part
     (`paging.rs:222`, `write_table`; tests
     `a_page_on_demand_stands_back_for_every_copy_that_has_its_page` and
     `pages_on_demand_change_only_entries_that_named_no_copy`, which shows
     over random sets of copies that a table with pages on demand differs
     from the one without only in entries that named no copy). So a title
     buffer's bytes are read and written at one place, the buffer's copy,
     by views and by address alike, and a root UAV's byte-for-byte
     copy-back finds what shaders wrote. Should a written page in a root
     UAV's range be a page on demand's all the same (the root UAV's copy
     had no slot in the table), it goes back by page and is counted
     (`paging.rs:421`, `take_marks`; test
     `a_root_uavs_page_goes_back_from_the_copy_the_table_named`).
   * The cost of that rule: a page that a small title buffer's copy holds
     in part gets no page on demand, and what lies in the rest of it reads
     0 and marks nothing, as without the switch (`memory.rs`'s header
     shortcut). Neither the miss counts nor paging see such memory. The
     pages a table names in part are counted when it is written and the
     frame line and the misses line give the figure.
   * Neighbouring pages lie side by side. Pages on demand of one aligned
     4 MiB lie in one buffer at their own offsets, so an access that starts
     in one and ends in the next (a shader's access is served by one
     lookup) reads and writes the right bytes when both are held, and the
     pages after a missed page are taken with it so that they are (test
     `neighbouring_pages_on_demand_lie_side_by_side`).
     shortcut, counted: where the next page is not beside a held page (the
     next 4 MiB, a page another copy holds, a page that is not tracked
     memory, a page not yet taken because the frame's count was reached),
     the part of an access that runs past the held page's end reads 0 and
     marks nothing. This is the shortcut every copy has at its end; the
     misses line gives the number of held pages where it applies. A write
     there marks the next page, so it goes back with that page, or is a
     counted miss whose bytes in the buffer are zeroed again.
   Locks: the paging state is taken inside the page tables' lock, never the
   other way, and a copy's drop takes neither (`memory.rs:496`, `GONE`).
5. Asynchronous use: what can still refer to it after the call returns. A
   submission runs to its end before the next starts (`host.rs`, `execute`
   waits for its fence), and a list recorded but not yet submitted names no
   copy: the table is written at submission (`memory.rs:816`, `prepare`).
   So what can refer to a copy given up is: the layout of each page table
   that named it, until that table's next rewrite; the views in each such
   heap, made null at that rewrite when no held page lies in the buffer any
   more; the batch of the submission that last refreshed it, dropped on the
   release thread. Each holds the copy itself (an `Arc`), the copy holds its
   buffer, and the buffer is counted until the last copy in it is gone. A
   page given up may be held again before its old copy is gone: the new copy
   lies at the same place, and the old one is named only by tables that are
   rewritten before any submission uses them. A view heap that is never used
   again keeps its last layout, and with it the copies that layout named:
   their buffers stay (the bound holds; the buffers usable shrink by that
   many).
6. The way back: restored from what, when, and if that source changed. From
   title memory. A page given up and reached again is a miss again
   (counted, its submission withheld) and gets a new copy, whose first
   refresh takes the whole page from title memory as it then is. Nothing a
   copy holds exists only there after its submission ended: what shaders
   wrote has gone back to title memory, or was withheld and dropped by the
   rule above.
7. Identical rendering: which tests and captures show it. With the switch
   off no copy is made, and two things differ from before, neither visible
   to a shader, each with its own switch:
   * a 256 MiB region a shader reached and no copy lies in gets an empty
     block in the page tables (at most 96 regions, only in blocks copies
     leave free; 32 KiB more to write per region whenever a table is
     written), so that its next miss names the page. Test
     `without_pages_on_demand_the_table_is_what_it_was` shows over random
     sets of copies that the table is, dword for dword, what the rule before
     this change gives, and that with probes every address finds the same
     view and offset and the host reads the same verdict (missed or not);
     `a_probe_makes_a_regions_misses_name_their_page` shows only the marked
     bit differs, and both bits are misses to the host
     (`paging.rs:343`, `mark`). `VICTORY_MEMORY_PROBES=0` turns the
     blocks off.
   * missed pages are looked at to count them by reason: a few system calls
     for at most 16 pages a frame, an answer kept for 30 to 59 frames and
     twice as long each time it is found again (up to 960), so a steady set
     of missed pages costs about a page a frame after its first seconds
     (test `a_steady_set_of_missed_pages_is_soon_asked_about_rarely`). **The
     cost of a call is not measured**: the repository's own notes put
     `VirtualQuery` in the runtime's big pools at 0.35 ms to milliseconds,
     so a frame that looks at 16 new stray pages may take several
     milliseconds longer. The misses line gives the pages looked at and the
     time it took; `VICTORY_MEMORY_MISS_REASONS=0` turns it off (misses are
     then counted as not looked at).
   With the switch on the frames are meant to differ: pages that read 0
   hold the title's bytes. **No capture exists**: this is the question the
   owner's run answers.

What a run shows, and what it does not.

* No "title memory misses" line while title-memory programs run (the
  existing line "title memory this frame") refutes the miss-and-withhold
  mechanism only. It does not show that shaders found all they reached:
  reads that return 0 without a mark exist (the rest of a page a copy holds
  in part, an access running past a copy's end, bytes past 512 MiB of a
  copy). The frame line's figure of pages named in part says how much
  memory is exposed to the first; whether shaders reach it is not counted.
* "in a 256 MiB region with no copy" is expected for the first frame or two
  a region is reached. If it persists, the line says how many such misses
  were in regions refused a block (all 96 in use by regions that may be
  title memory; also warned once) or set aside (every page looked at there
  was not title memory): no page there is named, looked at or given a copy.
* "copied but not named by the table" is expected with the switch on: a
  page given its copy for one list of a submission is missed by the
  submission's other lists too, since the table was written before.
* "tracked memory in no buffer object" and "in a title buffer with no copy"
  are the misses the switch gives copies. The first is decided by
  exclusion: the change tracker takes any mapped view that is neither an
  image nor plain private memory for the runtime's, so a stray address in a
  mapped view of something else is counted there too, and with the switch
  gets a copy and the tracker's guards (the exposure a 4 MiB window around
  a root argument has today, widened to pages shaders' pointers reach).
* With the switch on, ray-traced light appearing confirms the cause.
  **Its absence does not refute it**: if the misses line names lists whose
  writes were withheld (the builder's pipelines among them), one-shot work
  was lost at its first touch of each page and is not redone; and "refused
  (all N buffers in use)" above zero means the bound is too small.

Tracking needed that does not exist yet: which present pages a shader
reads (shaders mark writes and misses only), so pages cannot be given up
by last use, only when superseded or gone, and an access that runs past a
copy's end cannot be counted when it happens; and the extent of the title's
allocation around an address (the runtime knows it, the GPU layer does not
ask), so a structure is paged in from each missed page to the end of its
4 MiB and no further.

Regression tests added (any host, `crates/victory-gpu/src/paging.rs`, 43
tests): the table tests moved there from `memory.rs`, where they ran on
Windows only; for the table with pages on demand and probes
`a_page_on_demand_stands_back_for_every_copy_that_has_its_page`,
`pages_on_demand_change_only_entries_that_named_no_copy`,
`neighbouring_pages_on_demand_lie_side_by_side`,
`pages_on_demand_get_their_blocks_last`,
`a_probe_makes_a_regions_misses_name_their_page`,
`probes_take_only_the_blocks_copies_leave`,
`without_pages_on_demand_the_table_is_what_it_was`,
`bit_zero_is_above_the_table_only_once_the_lowest_region_has_a_block`; for
a list's marks `a_lists_marks_are_sorted_by_what_they_are`,
`a_root_uavs_page_goes_back_from_the_copy_the_table_named`,
`both_readings_of_a_bitmap_agree_on_a_miss`; for the bound
`pages_are_held_in_a_bounded_number_of_runs`,
`so_many_pages_and_new_runs_are_taken_in_a_frame`,
`a_run_is_free_only_when_its_last_copy_is_gone`,
`the_limit_bounds_the_host_memory`; for the decisions after a list ran,
against a stand-in for the system and the device,
`without_the_switch_misses_are_looked_at_and_counted_only`,
`a_missed_page_gets_a_copy_and_the_pages_after_it_in_its_run`,
`a_page_whose_run_has_no_buffer_left_stays_a_miss`,
`without_host_memory_nothing_is_held_and_nothing_tried_for_a_while`,
`a_page_the_tracker_refuses_is_not_held`,
`a_page_given_up_leaves_its_place_empty_and_its_run_its_buffer`,
`regions_only_strays_reach_make_way_for_others`,
`lists_whose_writes_were_withheld_are_counted_by_pipeline`; and the tests
of looking at pages, the counters and the log line. Needs a device and has
not run (`memory.rs`):
`a_missed_page_of_title_memory_gets_a_copy_for_the_next_submission` (two
neighbouring pages, a record read across them by one lookup, a write
through the second).

Switches: pages on demand are off unless `VICTORY_MEMORY_PAGING=1`.
`VICTORY_MEMORY_PAGING_MB=<megabytes>` sets the limit.
`VICTORY_MEMORY_PROBES=0` turns off the empty blocks (misses in regions
with no copy then stay counted by region, and no page there gets a copy).
`VICTORY_MEMORY_MISS_REASONS=0` turns off looking at missed pages (ignored,
with a warning, when pages on demand are on: a page is looked at before it
gets a copy).

Before / after: no measured run.

Decision: not approved as a default. In the tree behind the switch so that
one run can show whether the misses it addresses occur (the README's table
of switches says what the log shows, under `VICTORY_MEMORY_PAGING`).
