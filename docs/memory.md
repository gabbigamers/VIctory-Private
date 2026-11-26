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

### What no line reports

The measured "before" that every change needs does not exist yet for most
of the above. Counters to add first, each small:

* T1: pages allocated, freed, mapped, and sections alive (the first counter
  the checkers name);
* G1 to G5 and G7: bytes by kind for system-memory GPU allocations, so
  `video_shared_mb` can be split by owner;
* G4, G5: the kept pools' sizes and high-water marks;
* C1 to C3: the blob's size as a column, the save buffer's size and count,
  the bytes held in the shader map;
* T5: bytes in flight for all loading buffers, with the high-water mark;
* the process's private working set beside the total, so section pages can
  be told apart.

Also recorded: the runtime's commit-refusal message advises a larger
paging file (`xmem.rs`); there is no check of commit or RAM at start; the
title is told the process's private commit as its own memory use, which is
neither the title's use nor the whole process's.

## Candidates

Not ranked by measurement and **none approved**. Each becomes a change only
through a case, after its counter exists and a run has shown the size.
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

No case has been written yet.
