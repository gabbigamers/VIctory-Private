# Memory

The record of where a run's memory goes and what is being done about it.
The goal (A5 in [progress.md](progress.md)): the title runs stably inside
16 GB of physical RAM with room left for Windows and what else runs, at the
picture quality the title asks for.

Rules for this track:

* **Measured before and after.** Every change made for memory is entered in
  the ledger below with a run before it and a run after it, peak and
  sustained. A change with no measured gain is taken out again.
* **No picture traded for memory.** Lower texture resolution, dropped mips,
  smaller targets or skipped work are not optimisations here.
* **No paging file as a fix.** A larger paging file raises the commit limit
  and changes nothing about what must be in RAM. A run that only works
  with one has not met A5.
* Host independence as everywhere ([gpu-compatibility.md](gpu-compatibility.md)):
  budgets follow what the machine and the device report.

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

The question the inventory is to answer for each of these: is the second
copy needed while the first exists, and if not, which one can go without
changing what is drawn.

## Candidates

Not yet ranked by measurement. Each is entered with what it would remove,
which quantity that is, and what could go wrong; none is implemented
without a before and an after.

| # | Candidate | Removes | Quantity | Risk | Validated where |
|---|---|---|---|---|---|
| | | | | | to be filled from the inventory |
