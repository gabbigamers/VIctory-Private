# 0007: The title's D3D12.X commands run on the host GPU

Status: Reconstructed on 2026-11-26 from source comments and code. The
original record is not in the repository; where this text goes beyond what
the citations say, it says so.

The title of this record is the reconstruction's. The original title is
not known. File and line numbers are as of commit `854470e`; of the
cited files, `host.rs` has gained lines since (see
[README.md](README.md)), and `git show 854470e:<path>` shows the cited
lines. Paths given without a crate are under `crates/victory-gpu/src/`.

This is the most cited of the missing records: 23 citations, 22 of them
in `crates/victory-gpu`. They cite it three ways: by a step number (only
step 3 is ever named), by a quoted phrase (nine phrases), and with no
part named.

Inferred: the quoted phrases are titles of sections of the record. The
comments give only the phrase after the number, as in `(decision 0007,
"Heap-indexed views")`. This record calls them section titles from here
on, on that reading.

## Context

The title records graphics commands through D3D12.X: "code inlined into
the title, which writes packets into the command list while its put
pointer is below a limit and calls the list's method otherwise. Our lists
keep the limits at zero, so every command arrives here as a call, to be
run on host D3D12. The title's GPU addresses are CPU addresses (console
memory is unified)" (`crates/victory-gpu/src/lib.rs:3-7`).

The problems the citing modules answer, each in its own header:

* the host GPU cannot read title memory, so buffers and textures need
  host copies that are kept current (`shadow.rs:1-11`, `texture.rs:1-6`);
* the title's shaders pick descriptors from the heap themselves, "so no
  draw names the views it reads" (`bindless.rs:2-4`);
* the title creates far more pipelines than it draws with, and making a
  host pipeline takes from milliseconds to seconds (`pipeline.rs:6-8`,
  `pipeline/make.rs:2-8`);
* the console allows uses of one texture that the host does not
  (`twin.rs:2-6`);
* the console's lists wait on memory values that lists on other queues
  write (`hold.rs:3-8`).

## Decision

Only what the citing comments state and the cited code implements.

**Cited without a step or a section.**

* "The host GPU: one D3D12 device and direct queue (decision 0007), and a
  window to present on" (`crates/victory-gpu/src/host.rs:1-2`). The
  crate makes one host command queue, of the direct type
  (`host.rs:221-222`).
* "Each list records onto a host command list as its methods are called
  (decision 0007); draw state lives in `crate::draw`"
  (`crates/victory-gpu/src/list.rs:8-9`). The list object keeps "the host
  list the calls record into (decision 0007), and the draw state the
  title set" after the console's own layout (`list.rs:114-116`).
* A queue's execute "Queues the lists' host lists behind what the queue
  holds, and runs what may run (`crate::hold`; decision 0007)"
  (`crates/victory-gpu/src/queue.rs:113-114`).
* "Textures also live on the host GPU (decision 0007), made on first use"
  (`crates/victory-gpu/src/resource.rs:781-782`).
* The runtime asks the host to write-watch title memory it reserves,
  because "The GPU side copies only the pages the title wrote since its
  last copy (`victory_gpu`, decision 0007); placeholders cannot be
  watched" (`crates/victory-hle/src/modules/xmem.rs:132-137`).
* Draws of kinds not run "dropped until their shaders translate (decision
  0007)" (`crates/victory-gpu/src/list.rs:1584-1586`; see "Where a citing
  comment and the code differ" below).

**Cited as step 3.** See "Steps".

**Cited by quoted phrase.** If the phrases are section titles, they are
the only names of the record's sections that survive.

| Phrase as quoted | Cited at | What the citing comment and code state |
|---|---|---|
| "Heap-indexed views" | `bindless.rs:1-2` | Every view the title writes into a shader-visible heap gets its host view when it is written; each heap keeps what its slots view that title memory feeds; a submission whose lists run heap-indexing shaders first brings all of that up to date (`bindless.rs:4-10`) |
| "Queues wait on memory values" | `hold.rs:1` | Our submissions run to completion when made, so a queue is a FIFO of list segments, signals and fence waits; a head runs once its waits hold; "Order within a queue never changes, only the interleaving across queues" (`hold.rs:8-15`) |
| "Pipeline cache" | `pipeline/library.rs:1-2` | Every host pipeline goes into the host's pipeline library under a hash of its host description, saved under the cache folder; the pipelines a run used are listed and handed to the workers at creation in the next run; the pre-build lists what it has put in the library (`pipeline/library.rs:4-20`) |
| "Pipelines made off the title's threads" | `pipeline/make.rs:7-8` | Host pipelines are made by workers; a first use queues the pipeline and waits within a budget per frame; past the budget the draw or dispatch is dropped (`pipeline/make.rs:1-15`) |
| "One host texture per texture memory" | `resource.rs:798-800`, `twin.rs:1-2` | Title textures at the same address with the same shape and element size share one host texture, each viewing it in its own format; a depth texture the title also writes through unordered access gets a writable twin, with copies where the use changes (`twin.rs:6-14`) |
| "Host textures made on first use" | `resource.rs:1475-1478` | A host texture is made the first time a list uses it, a heap slot views it, or a copy or upload reaches it; "Most of the title's textures are never used" |
| "Copies of title memory" | `shadow.rs:1-2`, `watch.rs:1-2` | A buffer a draw uses gets a persistent host buffer of its size on first use; the ranges each submission reads are brought up to date before it runs; ranges shaders write go back to title memory after it (`shadow.rs:2-11`) |
| "Change tracking" | `watch.rs:1-2` | Each host copy tracks the 64 KiB pages it has not taken since they changed; taking pages makes them read-only, and the first write faults into a handler that marks the block changed in every copy holding it, "So bringing a copy up to date costs what changed since, not how much memory it covers" (`watch.rs:2-12`) |
| "Texture memory" | `texture.rs:1-2` | Texture contents the title writes itself are detiled from console memory into a staging buffer when the content changed, and the staging buffer is copied into the host texture (`texture.rs:1-11`) |

Inferred: the section titles were added to the record over time as each
problem was met in a run. The grounds are that the citing headers give
their reasons as observations of the title running (pipeline counts, a
watchdog ending a run, lock waits) and cite named runs, and that in the
history each phrase is first cited by the commit that brought the code
it names, over three days: "Copies of title memory" and "Texture memory"
on 2026-11-23, "Pipeline cache", "Pipelines made off the title's
threads", "Heap-indexed views", "Change tracking" and "One host texture
per texture memory" on 2026-11-24, "Host textures made on first use" and
"Queues wait on memory values" on 2026-11-25 (commits and times in
[README.md](README.md), "What the history shows"). No comment says how
the record grew, and the record could as well have been written whole
and cited piece by piece.

## Consequences visible in the code

* Every submission is waited for (see the first shortcut below). The
  audit lists this as the first hypothesis for where a frame's time goes
  (A-P1); it is not measured.
* Each title buffer a draw uses has a whole-buffer host copy in system
  memory, and each used texture a host texture. The audit lists these as
  mechanisms of memory use (A-M2, A-M4), and [memory.md](../memory.md)
  rules that such copies are not assumed redundant.
* Draws and dispatches are dropped where a pipeline is not ready within
  the frame's budget, or was refused (audit A-C8). The frame line and the
  performance log count them.
* Host textures are given up against the device's video-memory budget and
  made again on use (`resource.rs:1527-1537`, `1613-1619`). The idle
  limits there cite runs `evict1` to `evict4`.
* Change tracking depends on the runtime: it reports mappings, aliases
  and its own writes (`watch.rs:8-10`, `23-31`), which is why the
  citation in `xmem.rs` exists.
* Inferred: the write-watch flag the runtime sets is used by the GPU
  layer to recognise the runtime's pools, not to read which pages
  changed. `watch.rs:382-389` asks the host for a write-watch answer with
  no reset and room for one address, to decide whether a range is the
  runtime's; changes are found by the read-only guards. The citing
  comment in `xmem.rs` does not say which mechanism it serves. The
  history shows it was written for an earlier one: when commit `2849477`
  added the comment, the header of `watch.rs` in the same commit said
  "The runtime reserves title GPU memory with write watching
  (`victory-hle` xmem), so the kernel keeps a dirty bit per 4 KiB page;
  asking for a range's written pages also clears them". Commit `c75fdba`
  (2026-11-24) replaced that header with the one cited today, which
  tracks changes with guards. The comment in `xmem.rs` is unchanged
  since `2849477`.

`shortcut:` comments that name what was left open. The citing files in
`crates/victory-gpu` hold 53 of them at `854470e` (`descriptor/host.rs`
10, `list.rs` 7, `resource.rs` 7, `shadow.rs` 5, `queue.rs` 4,
`bindless.rs` 3, `pipeline.rs` 3, `texture.rs` 3, `descriptor.rs` 2,
`draw.rs` 2, `host.rs` 2, `watch.rs` 2, `hold.rs` 1, `pipeline/make.rs`
1, `twin.rs` 1; `pipeline/host.rs` and `pipeline/library.rs` none), and
the one citing file outside it, `crates/victory-hle/src/modules/xmem.rs`,
holds 4 more, none of them attached to this citation. Those in a citing
module header of `crates/victory-gpu`, or attached to a citation:

| Where | What the shortcut leaves open |
|---|---|
| `host.rs:4-5` | "every submission is waited for, so CPU and GPU never overlap; overlap them with per-submission fences once more than clears run" |
| `pipeline.rs:12-16` | which stage combinations get host pipelines (see below: the list is out of date) |
| `pipeline/make.rs:17-19` | "the budget (half a second of waiting per title frame) is a guess; a pass the title runs only once and whose pipeline takes longer is lost" |
| `texture.rs:13` | "a change detiles the whole texture, not the pages written" (see below) |
| `twin.rs:16-20` | uses the lists do not name move nothing between a depth texture and its twin; a whole twin is copied for any use of either plane; console 24-bit depth gets no twin |
| `watch.rs:33-35` | a system call writing into a guarded block fails, and the known ones open their blocks first; memory freed and allocated again while a tracker holds it is not seen |
| `bindless.rs:306-312` | what shaders write through the heap's unordered access views does not go back to title memory (see below) |
| `descriptor/host.rs:156-164` | a host that caps its view heaps below the size translated shaders assume: indices past it fault, and descriptors past the host's limit are left out (G3 in [gpu-compatibility.md](../gpu-compatibility.md), audit A-C2) |
| `descriptor/host.rs:787-790` | views of textures the title placed itself are made when a draw reads them through a table, not when written |
| `hold.rs:230-232`, `queue.rs:187-189` | a safety valve forces waits nothing satisfies, "remove once runs show it never fires" |
| `list.rs:1132-1133` | no query runs on the host GPU; results are plausible values (audit A-C12) |
| `list.rs:1599-1600` | indirect signatures that change root arguments are dropped |

Where a citing comment and the code differ. Each was read in the code;
none could be run here, because the code is compiled for Windows only.

* `pipeline.rs:6-8` says "Host pipelines are made on a pipeline's first
  draw, not at creation". `pipeline/make.rs:1-2` says they are made by
  workers and not by the title thread that first records a draw, and
  pipelines an earlier run drew are started at the title's creation
  (`pipeline.rs:389`, `pipeline/make.rs:228-230`,
  `pipeline/library.rs:10-14`). The header describes an earlier state:
  commit `2849477` (2026-11-23) wrote the sentence, and commit `35b054e`
  (2026-11-24) brought the workers and the early start.
* `pipeline.rs:12-14` says mesh stages "keep their containers only".
  `pipeline/host.rs:410-431` makes mesh pipelines and refuses only stages
  other than vertex, mesh, pixel and compute. The audit already records
  this header as stale.
* `list.rs:1584-1586` says draws of the kinds "the host does not run yet
  (indirect, mesh), and dispatches" are "dropped until their shaders
  translate (decision 0007)". The comment is attached to the stub used
  when there is no host GPU (`#[cfg(not(windows))]`). In the Windows
  build, dispatches, mesh dispatches and indirect execution reach the
  draw state (`list.rs:1504-1535`, `1590-1600`). Read as a statement of
  the decision, it describes an earlier state.
* `resource.rs:781-782` says textures live on the host GPU and "buffers
  do not yet". Buffers have host copies, made by `crate::shadow` on first
  use (`shadow.rs:1-5`). The sentence is true only of the field it
  documents. It dates from commit `754619f` (2026-11-23), the first to
  cite the decision, which had no `shadow.rs`; that file came with
  `2849477`.
* `texture.rs:13` says a change detiles the whole texture.
  `texture.rs:572-585` reads, hashes and detiles "Only the subresources
  in the pages that changed" and cites run `integ2` for the cost of the
  whole-texture form. The shortcut was written by commit `2849477`
  (2026-11-23); the code under it was changed by `cc5f961` (2026-11-25).
* `bindless.rs:306-312` and `descriptor/host.rs:1277-1278` say what
  shaders write through unordered access views stays in the host copies
  and names "shaders marking the pages they write" as the upgrade. That
  upgrade is in the code: translated shaders mark written pages
  (`crates/victory-shader/src/emit/mod.rs:154-169`, "added 2026-11-25"), the
  cache translates with marks on (`shader_cache.rs:84-86`), and marked
  pages go back after each submission (`shadow.rs:891-900`,
  `host.rs:1579`). The shortcut in `bindless.rs` was written by commit
  `c75fdba` (2026-11-24); the marks came with `cd94d35` (2026-11-25).
* `host.rs:4-5` makes overlapping CPU and GPU conditional on "once more
  than clears run". Draws and dispatches run now, and every submission is
  still waited for (audit A-P1). The shortcut stands; its condition has
  passed. It was written by commit `754619f` (2026-11-23), whose message
  lists what the host ran then as "clears, barriers, copies, resolves
  and PresentX".

## Steps

| Step | Cited at | What the citing code does |
|---|---|---|
| 1 | not cited anywhere | unknown |
| 2 | not cited anywhere | unknown |
| 3 | `draw.rs:1` | draws on the host GPU: each draw brings the host list up to date with the host pipeline, our root signature, user data, host copies and texture states |
| 3 | `list.rs:1333` | the title's draw state, kept for its draws or set on the host list at once |
| 3 | `descriptor.rs:55-57` | shader-visible view and sampler heaps get a host heap with the same descriptor order on first use by a draw; slots written before wait |
| 3 | `descriptor/host.rs:1-2` | host descriptor heaps behind the title's heaps, slot for slot, because translated shaders index them at the title's positions |
| 3 | `pipeline.rs:8` | host pipelines made at a pipeline's first draw, not at creation |
| 3 | `pipeline/host.rs:1` | host pipelines for title pipelines, under a root signature of our own that the translated shaders' bindings define |
| 3 | `shadow.rs:1` | host copies of title buffers |

Whether the record had steps after 3 is unknown.

Inferred: step 3 was "draws", and steps 1 and 2 were what a draw needs
first: the host device with recording of the title's lists, then clears
and presentation. The grounds are that every step 3 citation is about
drawing, that the citations without a step are the device, the lists, the
queues and textures, and that the `host.rs` shortcut speaks of a time
when only clears ran. The history fits this and does not prove it: commit
`754619f` (2026-11-23 14:42) cited the decision four times with no step
(the device and queue in `host.rs`, the host list in `list.rs`, "Runs the
lists' host lists and waits for them (decision 0007)" in `queue.rs`, and
textures on the host in `resource.rs`) and had no `draw.rs`, its message
listing what ran on the host as "clears, barriers, copies, resolves and
PresentX"; commit `2849477` (2026-11-23 23:10) brought `draw.rs` and all
seven step 3 citations. No comment, at any commit, names steps 1 or 2.

## Unknown

* The original title, author and date of writing. The history gives the
  dates of the citations: the first were added by commit `754619f`
  (2026-11-23 14:42, the third commit of the repository), the last by
  `9cc37c5` (2026-11-25 14:20). Those commits are not in a shallow
  checkout cut at `552208c`; see "What the history shows" in
  [README.md](README.md) for how they were read. Inferred: the record
  existed in some form by 2026-11-23 and was still being cited as it grew
  two days later. No record was ever a file in the repository.
* What steps 1 and 2 were, and whether later steps existed.
* The full list of sections. Nine titles are quoted; sections never cited
  cannot be recovered.
* Which alternatives were considered for each part. No citing comment
  names one as an alternative the record weighed. (Candidates the
  reconstruction can think of, as examples only: more than one host
  queue, mapping title memory into host resources instead of copying,
  host pipelines at creation, a per-draw copy of descriptors instead of
  heaps kept in the title's order.) One is known from the history to
  have been built and replaced, which is not the same as the record
  having weighed it: change tracking by the kernel's write watching (see
  "Consequences visible in the code").
* The measurements behind the choices. Comments give figures and name the
  runs they came from; the run data is not in the repository. Commit
  messages in the full history say more about most runs (see the tables
  of runs in [README.md](README.md)).
* Which host GPU the record was written against. The README says one
  machine was tested, with an AMD card; comments record observations on
  other cards (audit, "What the audit corrected"). The record's own
  assumptions about the host are not recoverable, and
  [gpu-compatibility.md](../gpu-compatibility.md), written on 2026-11-26,
  is the rule that applies now.
* What the record said about memory cost. The code comments record costs
  as they were met; [memory.md](../memory.md) is the rule that applies
  now.

## References

23 citations in code.

| Where | Item | Part cited | What the citing sentence says |
|---|---|---|---|
| `bindless.rs:1` | module header | "Heap-indexed views" | views shaders reach through the resource heap itself |
| `descriptor.rs:56` | field `Heap::host`, lines 55-57 | step 3 | a host heap with the same descriptor order, made when a draw first uses the heap |
| `descriptor/host.rs:1` | module header, lines 1-2 | step 3 | host descriptor heaps behind the title's shader-visible heaps |
| `draw.rs:1` | module header | step 3 | draws on the host GPU |
| `hold.rs:1` | module header | "Queues wait on memory values" | work a queue holds back |
| `host.rs:1` | module header | none | one D3D12 device and direct queue |
| `list.rs:9` | module header, lines 8-9 | none | each list records onto a host command list as its methods are called |
| `list.rs:116` | fields of `List`, lines 114-116 | none | the host list the calls record into |
| `list.rs:1333` | module `state` | step 3 | the title's draw state |
| `list.rs:1586` | stub `draw` for builds without a host GPU | none | draws dropped until their shaders translate |
| `pipeline.rs:8` | module header, lines 6-8 | step 3 | host pipelines made on first draw |
| `pipeline/host.rs:1` | module header | step 3 | host pipelines for title pipelines |
| `pipeline/library.rs:2` | module header, lines 1-2 | "Pipeline cache" | compiled host pipelines kept on disk, and the pipelines worth starting early |
| `pipeline/make.rs:7` | module header, lines 1-8 | "Pipelines made off the title's threads" | pipelines are made by workers |
| `queue.rs:114` | doc comment of `execute` | none | lists are queued behind what the queue holds |
| `resource.rs:781` | field `Resource::host` | none | textures also live on the host GPU |
| `resource.rs:798` | doc comment of `HostTexture` | "One host texture per texture memory" | title textures of one memory share a host texture |
| `resource.rs:1477` | doc comment of `Resource::host` | "Host textures made on first use" | a host texture is made the first time it is asked for |
| `shadow.rs:1` | module header | step 3 and "Copies of title memory" | host copies of title buffers |
| `texture.rs:1` | module header | "Texture memory" | texture contents the title writes itself |
| `twin.rs:1` | module header, lines 1-2 | "One host texture per texture memory" | depth textures the title also writes through unordered access |
| `watch.rs:1` | module header, lines 1-2 | "Copies of title memory" and "Change tracking" | which title memory changed since a host copy took it |
| `crates/victory-hle/src/modules/xmem.rs:133` | comment in `XMemVirtualAlloc`, lines 132-137 | none | title memory is write-watched for the GPU side's copies |

In `descriptor/host.rs` and `twin.rs` the word "decision" ends line 1 and
the number begins line 2, so a search for the two on one line misses
them. That is the difference between the audit's count of 39 citations
and the 41 found here.

`crates/victory-gpu/src/pipeline.rs` cites this decision and decision
[0003](0003-shader-translation.md) in the same paragraph.

Mentioned as missing in `docs/audit-2026-11-26.md:127` (A-B5) and in
`docs/progress.md` ("Needed from the owner", item 6).
