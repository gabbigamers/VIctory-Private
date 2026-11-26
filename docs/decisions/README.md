# Decision records

Source comments in this repository cite numbered design decisions
("decision 0007, step 3") and named measurement runs ("run evict4"). The
records and the run data are not in the repository and were not files at
any commit of its two branches (see "What the history shows" for what was
searched), and the owner does not have them (audit of 2026-11-26, finding
A-B5). This directory holds what can be recovered from the code and from
the history.

## What a decision record is here

A numbered text that a comment can cite instead of repeating a design's
reasons. The citing comments show that the originals had a number of four
digits, that two were divided into numbered steps (0003, 0007), that one
is cited by quoted phrases (0007) and one by "item" (0005). Nothing else
about their form is known.

Inferred: the quoted phrases are titles of sections of record 0007. The
comments give only the phrase after the number, as in `(decision 0007,
"Heap-indexed views")`.

The records below are **reconstructions**, written on 2026-11-26 from the
citing comments and the code around them, with dates from the commit
history. They are not the originals and do not claim to be. Each one
keeps four things apart:

* what a citing comment says, quoted with its file and line;
* what the cited code does, stated from reading it;
* what the commit history shows, with the commit named;
* interpretation, in sentences that start with "Inferred:".

Each record also lists what the original presumably held and cannot be
recovered. If an original turns up, it replaces the reconstruction of the
same number.

File and line numbers are as of commit `854470e`; `git show
854470e:<path>` shows a cited line whatever has moved since. Lines had
already moved when these files were finished, at commit `09efeea`:
`crates/victory-gpu/src/host.rs` had gained 19 lines after line 312 and 2
after line 3343, `docs/progress.md` had grown, and
`crates/victory-shader/src/emit/lower.rs` and `emit/mod.rs` had
uncommitted changes by other work. Each entry in a References table
therefore also names the item it is attached to, the tables of shortcuts
in 0003 and 0005 name the function, and `docs/progress.md` is cited by
heading.

## Index

| Number | Reconstructed record | Citations in code | Cited by |
|---|---|---|---|
| 0001 | [Where title memory lies in the address space](0001-title-memory-address-range.md) | 1 | number only |
| 0003 | [Console shader code is translated to DXIL](0003-shader-translation.md) | 11 | number; steps 1 and 3 |
| 0005 | [Shaders reach title memory by address; the BVH instruction is emulated](0005-title-memory-and-bvh.md) | 6 | number; "the unified-memory item" |
| 0007 | [The title's D3D12.X commands run on the host GPU](0007-host-gpu.md) | 23 | number; step 3; nine quoted phrases |

41 citations in all, in `crates/` and `apps/`. `xtask/`, `README.md`,
`scripts/` and `tests/` cite none. Two documents name the four numbers
as missing and cite nothing else: `docs/audit-2026-11-26.md:127` (A-B5)
and `docs/progress.md` ("Needed from the owner", item 6).

The audit counted 39. The two it missed have the word "decision" at the
end of one line and the number at the start of the next
(`crates/victory-gpu/src/descriptor/host.rs:1-2`,
`crates/victory-gpu/src/twin.rs:1-2`).

The record titles are the reconstruction's. No original title is known.

## Numbers never cited

**0002, 0004 and 0006** are not cited anywhere in the repository: not in
code, not in a document, and not in any line or commit message of the
history (see the next section for what was searched). Nothing is known
about them: not their subjects, and not whether they were written,
withdrawn or superseded.

No number above 0007 is cited either. Whether the series went further is
unknown.

The search of the tree was for the word "decision", in any letter case,
followed by a number, on one line or across a line break, over `crates/`,
`apps/`, `xtask/`, `docs/`, `scripts/`, `tests/`, `README.md` and the
workspace manifest.

## What the history shows

The checkout these files were written in is a shallow clone: it is cut at
commit `552208c` (2026-11-25), whose parent `129a9ba` it does not hold,
and every citation is already present at `552208c`. The earlier history
was therefore read from a full clone of the origin's `main` branch, made
on 2026-11-26 outside the working tree: 692 commits from `f4504f6`
("Initial commit", 2026-11-23 01:15) to `63bcca4` (2026-11-26 00:27), 642
of them before `552208c`. The origin has two branches, `main` and the
branch this work is on, which adds the commits after `63bcca4`. Commit
names below `552208c` in this directory can be read only in a full clone.

**No record was ever a file.** On `main` 191 paths were ever added, and
none was ever deleted or renamed. Three are Markdown: `README.md`,
`docs/architecture.md` and `docs/asset-policy.md`. No path has
"decision", "adr", "design", "record", "notes" or "todo" in its name. The
branch this work is on adds documents under `docs/` and none of that
kind before this directory.

```
git log main --no-renames --diff-filter=A --name-only --format= | sort -u
git log main --no-renames --diff-filter=D --name-only --format=
git log main --reverse -i -S'ecision 0007' --format='%h %ad %s'
```

**No commit message cites a decision by number.** The search was the 692
messages of `main` and the messages of this branch, for "decision"
followed by digits.

**When each number, step and quoted phrase is first cited.** The commit is
the earliest in `git log --reverse` order whose change adds the text;
times are the commits' author times.

| What is cited | First cited in | Date |
|---|---|---|
| decision 0003; decision 0003, step 1 | `754619f` | 2026-11-23 14:42 |
| decision 0007 (no part named) | `754619f` | 2026-11-23 14:42 |
| decision 0001 | `2849477` | 2026-11-23 23:10 |
| decision 0003, step 3 | `2849477` | 2026-11-23 23:10 |
| decision 0007, step 3 | `2849477` | 2026-11-23 23:10 |
| decision 0007, "Copies of title memory" and "Texture memory" | `2849477` | 2026-11-23 23:10 |
| decision 0007, "Pipeline cache" and "Pipelines made off the title's threads" | `35b054e` | 2026-11-24 01:02 |
| decision 0005 (the BVH instruction, no part named) | `0b35b7f` | 2026-11-24 22:36 |
| decision 0005, "the unified-memory item" | `27c3dc9` | 2026-11-24 22:36 |
| decision 0007, "Heap-indexed views" and "Change tracking" | `c75fdba` | 2026-11-24 22:36 |
| decision 0007, "One host texture per texture memory" | `13bbd6f` | 2026-11-24 22:36 |
| decision 0007, "Host textures made on first use" | `a7381ed` | 2026-11-25 03:16 |
| decision 0007, "Queues wait on memory values" | `9cc37c5` | 2026-11-25 14:20 |

`754619f` is the third commit of the repository. No citation of a step 2,
of a step above 3, or of a decision number other than the four was ever
added on `main`. Every citation that was added is still in the tree, in
its first wording or a later one (the rewordings that say something are
in the [0007](0007-host-gpu.md) record), with one exception, which
follows.

**A third document that was never committed.** At `754619f` four comments
pointed to a `TODO.md` by section ("TODO.md, GPU", "TODO.md, formats"),
and one of them tied it to a decision: the header of
`crates/victory-gpu/src/pipeline.rs` said the console shader containers
were kept and not translated, "Translate through the cache here (TODO.md,
GPU: pipeline creation, decision 0003)". `2849477` replaced those four
comments. One reference remains in the tree,
`crates/victory-hle/src/modules/display.rs:61-62` ("see the presentation
items in `TODO.md`"), and one commit message (`5cb63d8`) quotes an entry
as being in "TODO.md since 2026-11-23". No `TODO.md` was ever a file in
the repository.

Inferred: the decision records and `TODO.md` were working notes kept
beside the repository by whoever wrote the code, from the first day of
the history. The grounds are that they are cited from the third commit
on and were never committed. Nothing in the history says where they were
kept.

## Named measurement runs

Comments give figures and name the run they came from. A name is a word
and a number; the comments never say what the word stands for, which
build or machine a run was taken on, or where its output went. **None of
the run data is in the repository**, at any commit. The figures below are
the comments' own. They are observations from runs with no recorded
commit or machine in the tree, so they are not entries for the ledgers in
[progress.md](../progress.md). Commit messages say more about most of
them; see "Runs in the history" below.

25 names are cited, 46 times on 42 lines, all in
`crates/victory-gpu/src/`. Paths in the table are relative to that
directory.

| Run | Cited at | What the comment says was observed |
|---|---|---|
| `b2` | `descriptor/host.rs:790` | a host texture and a staging copy for each of about 20,000 placed textures the title has views of "ran the host out of memory" |
| `c1` | `host.rs:1529`, `memory.rs:680` | results shaders computed from memory with no copy, written to title memory, broke the title's own data ("garbage pointers in title threads") |
| `c4` | `host.rs:1529` | cited with `c1` for the same observation |
| `drops1` | `memory.rs:313` | withholding only the lists that missed memory broke the title's own data: "an AABB's +inf/-inf in a pointer the title followed" |
| `evict1` | `host.rs:578`, `resource.rs:1616` | heap reads thrashed: a slot's read mark is reported once, so only its first read stamped the texture, and the run "gave up and made again hundreds of textures a frame" |
| `evict2` | `bindless.rs:248`, `descriptor.rs:212`, `resource.rs:1533` | bringing up every heap-viewed texture whenever its pages change re-uploaded about 4 GB in 30 to 46 world frames; title threads waited 1 to 6 s a frame on the heaps' lock while textures were made; an idle limit of 20 s alone cycled the textures of a cutscene's alternating shots in slow runs |
| `evict3` | `descriptor/host.rs:866`, `resource.rs:1535`, `texture.rs:564`, `watch.rs:839` | a per-texture map of slots cost about 25 microseconds a slot at 62,000 slots a frame; an idle limit of 30 frames gave up what menus and loading made before the world read it; 123,000 texture refreshes a frame; textures over withheld ranges uploaded again for nothing, 19 % of the uploads |
| `evict4` | `resource.rs:1530` | of the textures given up after 20 s, nine in ten were read again within a minute |
| `evict5` | `bindless.rs:221`, `descriptor.rs:228`, `descriptor/host.rs:795`, `texture.rs:142`, `texture.rs:167`, `texture.rs:518` | in a new shot: reading title memory, detiling and making staging buffers took half the submitting thread, with title threads waiting on the lock; making upload buffers took 10 % of it; making committed textures one by one took 7 %; reading and detiling cost about 6 ms of frame a MB on one thread |
| `evict6` | `texture.rs:382` | one barrier per texture between copies kept the GPU from overlapping them; new shots copy hundreds of textures |
| `integ2` | `texture.rs:41`, `texture.rs:574` | staging buffers kept for each texture's life took 2.6 to 2.8 GB of system memory; a whole re-read and detile of each streamed texture cost up to 430 MB a frame |
| `light13` | `host.rs:1535`, `host.rs:4237` | another program wrote "the imposter transforms" in the same list as the BVH builder; its writes have to go back when the builder's are withheld |
| `light19` | `indirect.rs:359` | with `light21`: the title's GPU heap was "moved to unordered access under the input assembler", against the debug layer's rule |
| `light21` | `indirect.rs:359`, `twin.rs:600` | as `light19`; and a prelude's copy into a depth twin "started from the states the list leaves" |
| `noevict1` | `bindless.rs:248`, `descriptor.rs:212` | cited with `evict2` for the re-upload volume and the lock waits |
| `perf1` | `host.rs:3961` | releasing a D3D12 resource is a kernel call, about 5 % of a world frame on the title's submitting thread |
| `perf3` | `descriptor/host.rs:694`, `frame.rs:103` | a world frame looks at about 700,000 heap slots, "~75 ms" (the comment gives the figure beside the change from two locks per slot to one lock of each kind for them all, and does not say which form it was measured on); waiting for the next frame tick cost the cutscene's frames 0 to 33 ms each, 4 % of the frame thread |
| `perf4` | `bindless.rs:59`, `watch.rs:477` | merged ranges rebuilt whole after every change cost 13 to 27 ms a frame; each page with nothing mapped cost a dozen failing kernel reads, 7 to 30 ms a frame |
| `perf7` | `draw.rs:2889`, `texture.rs:373` | 149 textures uploaded 3,195 times in a frame, 17 ms of GPU time; the same textures up to 24 times a frame |
| `perf15` | `watch.rs:837` | guards opened page by page cost about 15 % of the submitting thread at each frame end |
| `placed1` | `texture.rs:710` | a fresh buffer for each texture cost a zero-fill and a page fault per 4 KiB: 129,000 faults in a frame |
| `shards20` | `memory.rs:333`, `memory.rs:1085` | "garbage BVH nodes over meshlet buffers" until the frame's end, from what a submission that missed memory left in the host copies |
| `textures8` | `texture.rs:95` | a fresh 12.7 MB of upload buffers a frame cost about 5 ms while the menu movie rewrote the same textures |
| `textures9` | `memory.rs:347`, `shadow.rs:1561` | results computed from memory with no copy fed a later frame: "a box16 node's f16 bounds over a title object" |
| `w10` | `descriptor/host.rs:1803` | dated 2026-11-24, the only run with a date: null buffer views in slots with no view "did not stop the world's low-address page faults" |

Seven of these citations are in doc comments of tests that reproduce the
run's shape with synthetic data (`host.rs:578`, `host.rs:4237`,
`indirect.rs:359`, `twin.rs:600`, `memory.rs:1085`, `draw.rs:2889`,
`shadow.rs:1561`): the tests exist, the runs they were drawn from do
not.

Inferred: the word in a name is the subject the run was made for
(eviction of host textures, performance, lighting, textures), and the
number counts runs on that subject. No comment says so.

Inferred: the numbering implies further runs that neither a comment nor
a commit message names (for example `light8` to `light12`). This rests
on the reading above.

### Runs in the history

From the full clone described under "What the history shows". The first
column of commits is the earliest whose change adds the run's name to
code; the second lists every commit on `main` whose message names the
run. All are dated 2026-11-25 except where a date is given.

| Run | First cited in code | Commits whose message names it |
|---|---|---|
| `b2` | `c75fdba` (2026-11-24) | none |
| `c1` | `c75fdba` (2026-11-24) | none |
| `c4` | `c75fdba` (2026-11-24) | none |
| `drops1` | `9337b35` | `9337b35` |
| `evict1` | `69d1e8e` | `69d1e8e` |
| `evict2` | `69d1e8e` | `69d1e8e`, `4959d34`, `cea278c` |
| `evict3` | `658a0d1` | `658a0d1`, `ed25c0e`, `c6e9500` |
| `evict4` | `69d1e8e` | `69d1e8e` |
| `evict5` | `42ea09f` | `42ea09f`, `56f75f8`, `8a3d975`, `ab2e2b2` |
| `evict6` | `ea81289` | `69d1e8e`, `8a3d975`, `ab2e2b2` |
| `noevict1` | `4959d34` | `69d1e8e`, `4959d34`, `cea278c` |
| `integ2` | `fdf34ab` | `fdf34ab`, `cc5f961` |
| `light13` | `0a5be3c` | `0a5be3c` |
| `light19` | `315cea1` | `9cc37c5`, `315cea1` |
| `light21` | `315cea1` | `9cc37c5`, `315cea1` |
| `perf1` | `3e10052` | `1365fe1`, `1953339`, `3e10052`, `f1c55a8` |
| `perf3` | `f809521` | `280b150`, `f809521`, `cc0e153`, `ed887f2`, `9c179ea` |
| `perf4` | `2922141` | `2922141`, `2a4938e` |
| `perf7` | `23a4b62` | `23a4b62` |
| `perf15` | `36ca299` | `ed10c27`, `36ca299` |
| `placed1` | `8f2a6f1` | `11e9a54`, `8f2a6f1` |
| `shards20` | `ec9e750` | `ec9e750` |
| `textures8` | `fdf34ab` | `d75491e`, `5c4c685`, `fdf34ab` |
| `textures9` | `38994e5` | `38994e5` |
| `w10` | `35b054e` (2026-11-24) | none |

The messages were not transcribed here. They are the nearest thing to run
notes that exists: several give a run's length, the frames looked at, the
build compared with and the figures before and after, and some name the
card (for example `11e9a54`, which names run `shots2`, has figures
"Measured on the RX 6700 XT"). They are still one author's summary of a
run, not its data.

Thirty more runs are named in commit messages on `main` and by no comment
in the tree: `drops2`, `fps0`, `integ3`, `light2`, `light3`, `light5`,
`light7`, `light17`, `light18`, `light20`, `mk2`, `mk3`, `perf2`, `perf5`,
`perf13`, `scratch1`, `shards2`, `shards3`, `shards5`, `shards6`,
`shards21`, `shards22`, `shots1`, `shots2`, `textures1`, `textures2`,
`textures3`, `textures5`, `textures6`, `textures7`. One of them was once
cited in code: `textures2`, added by `52d14b1` and removed by `69d1e8e`.

The names were found in messages by pattern (a family word from the list
above followed by a number, or any word and number after "run" or
"runs"), so a run named in another form could have been missed.

## Decisions recorded in documents that exist

These are in force and are the place to look first. They are pointers,
not part of the old series, and they are not renumbered into it.

| Document | What it decides |
|---|---|
| [asset-policy.md](../asset-policy.md) | Rule 1: nothing from the game is stored in the repository. "The one exception" is one file of constant tables, kept by the project owner's decision dated 2026-11-23 and entered in `xtask/src/policy.rs`. "Access controls": no decryption, key material or reuse of a title's cipher routines |
| [gpu-compatibility.md](../gpu-compatibility.md) | The layer is NVIDIA-first and works across GPU generations: nine rules (capabilities and not names, guest semantics independent of the host, and the rest), what the translation needs of a host, and the host dependencies still in the code (G1 to G11) |
| [progress.md](../progress.md), "Order of work" | Correctness before speed; no frame generation; measured, not assumed. The same document holds the rules that do not change between sessions, the acceptance criteria A1 to A7 with the 16 GB RAM target, and the validation tiers |
| [progress.md](../progress.md), "Correctness", milestone C7, and "Session log", 2026-11-26, "Owner's decisions on the loop limit" | The owner's decisions of 2026-11-26: a translated loop cut short by the iteration limit is a correctness failure to investigate, never an accepted way to render; stops are counted and logged by shader and stage; a stopped lane ends in a defined, stage-appropriate way with no output left undefined; the tests that run full-length loops stay in the default test run. This is the rule for the `LOOP_LIMIT` shortcut listed in [0003](0003-shader-translation.md) |
| [progress.md](../progress.md), "Session log", 2026-11-26, "Owner's decision on the BCPack decoder" | VIctory's decoder is kept; the parts of `bcpack-rs` whose origin is independently verified are used for testing; its data files stay out. The evaluation behind it is [bcpack-rs-evaluation.md](../bcpack-rs-evaluation.md) |
| [memory.md](../memory.md) | The same bytes in two places are not a duplicate until a written case shows it; the seven questions of a case; the order of preferred means; the four quantities measured apart. Its case template ends in a "Decision:" line; no case has been written yet |
| [architecture.md](../architecture.md) | The import resolution order, and summaries of the host GPU and memory rules above |
| [audit-2026-11-26.md](../audit-2026-11-26.md), "Reuse" | Which outside sources may be used and under what licence, and which may not |

A reconstructed record describes what the code and its comments said when
they were read. It does not override any of these documents.
