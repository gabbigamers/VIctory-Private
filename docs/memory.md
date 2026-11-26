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

An inventory of every significant allocation site by owner and kind, each
checked against the code by a second reader, is in progress (2026-11-26)
and is entered here when it completes. What the audit of 2026-11-26
established, as mechanisms and without sums
([audit-2026-11-26.md](audit-2026-11-26.md), "Memory"):

* The title's own memory is allocated as the title asks: 10 GiB + 3.5 GiB
  in Series X mode. On the console that memory is unified, so it holds the
  title's textures and geometry as well as its CPU data.
* Each title buffer a draw uses gets a whole-buffer committed host copy in
  system memory, and each texture a host texture in video memory. After an
  upload, the same bytes exist in the title's memory and on the host.
* Fixed costs in the GPU layer: a 512 MB snapshot pool, per view heap about
  41 MB of lookup tables, a 64 MB staging pool, mapping windows that are
  never freed.
* The pipeline library is read whole into memory and kept; a code comment
  gives about 1.7 GB for it on NVIDIA.
* Video memory follows the device's budget and is not the constraint.

These are mechanisms, not findings of waste. The inventory records, for
each site, the facts the case in "Before an allocation is removed or
consolidated" needs; whether any copy is redundant is decided there and
nowhere else.

## Candidates

Not yet ranked by measurement, and none approved. A candidate becomes a
change only through a case.

| # | Candidate | Kind (1 remove, 2 reuse, 3 evict, 4 map) | Quantity it would lower | Open questions of the case | Validated where |
|---|---|---|---|---|---|
| | | | | | to be filled from the inventory |

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
