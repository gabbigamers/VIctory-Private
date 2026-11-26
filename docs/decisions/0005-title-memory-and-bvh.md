# 0005: Shaders reach title memory by address; the BVH instruction is emulated

Status: Reconstructed on 2026-11-26 from source comments and code. The
original record is not in the repository; where this text goes beyond what
the citations say, it says so.

The title of this record is the reconstruction's. The original title and
subject are not known: five of the six citations name one part of the
record ("the unified-memory item", "title memory") and the sixth names no
part. File and line numbers are as of commit `854470e`; `emit/lower.rs`
and `emit/mod.rs` had uncommitted changes by other work when this was
written, so their line numbers have moved since (the doc comment of
`Lower::memory` begins at line 4641 in the commit; `git show
854470e:<path>` shows the cited lines).

## Context

The console's memory is unified: "The title's GPU addresses are CPU
addresses" (`crates/victory-gpu/src/lib.rs:7`). A console shader can
therefore read and write memory at an address it computes from data: a
pointer in its root constants, a pointer it loaded from memory, or
arithmetic on such values (`crates/victory-shader/src/resource.rs:41-44`).
The host's memory is not unified, and a host shader has no instruction
that reads a raw address.

The second problem the citations show: the console GPU has an instruction
that tests one ray against one node of a bounding volume hierarchy the
console wrote into memory (`image_bvh_intersect_ray` and its 64-bit form).
The host has no such instruction, and the nodes are in title memory.

## Decision

Only what the six citing comments state and the cited code implements.

**The unified-memory item.**

* "Title memory at a raw GPU address (decision 0005: the unified-memory
  item). Every access to an address the program computes goes through
  here: scalar and global loads, stores and atomics, the BVH node fetch,
  and in a program that uses title memory its root SRVs and UAVs"
  (doc comment of `Lower::memory`,
  `crates/victory-shader/src/emit/lower.rs:4641-4645`).
* The address is not traced to a root parameter: "lowering takes the
  address from the instruction's own operands (decision 0005, title
  memory)" (doc comment of `MEMORY`,
  `crates/victory-shader/src/resource.rs:41-44`).
* The shader's side of the contract: "**Title memory** (added 2026-11-24,
  decision 0005's unified-memory item): accesses at addresses the program
  computes (pointers in root constants or loaded from memory, BVH nodes)
  go through a page table" (`crates/victory-shader/src/emit/mod.rs:122-143`).
  The table has 4096 first-level entries for address bits 28 to 39 and
  blocks of 4096 entries for 64 KiB pages; an entry names the heap index
  of a raw view of the host memory holding the page and the low dword of
  the title address that memory starts at. A second root view takes a bit
  for each page a shader writes and for each page or 256 MiB region it
  reached that no copy holds. "Reads there give 0, writes are dropped."
* The host's side: "Title memory at raw GPU addresses (decision 0005, the
  unified-memory item; the contract is `victory_shader::emit`'s "Title
  memory")" (`crates/victory-gpu/src/memory.rs:1-2`). The copies the table
  names "are the ones draws and dispatches use through views, so what one
  shader writes through a view another reads at its address, within a
  submission too" (`memory.rs:7-9`). After a submission each marked page
  goes back into title memory (`memory.rs:17-20`).
* The tests: "Title memory at addresses the program computes (decision
  0005, the unified-memory item), run on the host GPU and WARP with the
  test host serving the contract"
  (`crates/victory-shader/src/emit/memory_tests.rs:1-3`). The programs are
  synthetic.

**The BVH instruction.**

* "`image_bvh_intersect_ray` and `image_bvh64_intersect_ray` emulated
  (decision 0005): one ray against one node of a BVH the console wrote,
  the result in the hardware's encoding, so the program's own traversal
  runs unchanged" (`crates/victory-shader/src/emit/lower/bvh.rs:1-4`).
  The header names its sources: AMD's RDNA2 instruction set guide, AMD's
  GPURT and Mesa RADV (`bvh.rs:4-8`).

Inferred: the record was a list of items about running the title's ray
tracing as the title's own code, of which unified memory was one. The
grounds are the phrase "the unified-memory item", which implies other
items; the BVH emulation citing the same number; and the audit's reading
of the code that "The title's own builder runs as translated compute"
with "no host acceleration-structure code" (audit, A-C7). No citing
comment states the record's subject.

Inferred: emulating the instruction instead of using the host's ray
tracing was the decided approach. The comment gives the effect ("the
program's own traversal runs unchanged") and not a comparison with
alternatives.

## Consequences visible in the code

* What shaders compute from memory no copy holds is not the console's
  result. Such writes are kept out of title memory, and so is what every
  program using title memory wrote while any list misses, for that frame
  and the next (`crates/victory-gpu/src/memory.rs:308-315`, `678-682`).
  The comments cite the runs that led to each rule (`c1`, `drops1`,
  `shards20`, `textures9`; see the table of runs in
  [README.md](README.md)). The audit lists this withholding as one of
  three candidate causes for ray-tracing structures never appearing
  (A-C7); none has been shown by a run.
* In a program that uses title memory, root SRVs and UAVs are title memory
  too, read through the page table instead of root views
  (`emit/mod.rs:138-143`).
* The cost per submission is stated in the header: a change check of every
  host copy, copies of the pages the title wrote, a compare of each page
  shaders wrote, the table's rewrite when copies came or went, and a
  256 KiB bitmap per list (`memory.rs:22-25`). The audit lists the page
  table among the lookup tables shaders read across the bus (A-P2) and
  the two-level walk per computed address as emitted-code cost (A-P7).
  Neither is measured.
* Inferred: the table's reach of 2^40 depends on title memory being
  placed below 2^40, which is decision
  [0001](0001-title-memory-address-range.md). No comment links the two.

`shortcut:` comments in the citing files that name what was left open:

| Where | What the shortcut leaves open |
|---|---|
| `crates/victory-gpu/src/memory.rs:30-31` | "a page two copies share without either holding all of it is the larger one's" |
| `crates/victory-gpu/src/memory.rs:32-33` | a table names copies in at most `CAPACITY` (512) regions of 256 MiB; past that the smallest copies read as missing |
| `crates/victory-gpu/src/memory.rs:85-86` | a marked page partly in a precisely declared range loses what shaders wrote in its other part |
| `crates/victory-shader/src/emit/lower.rs:4656-4657` (fn `Lower::memory`) | "one lookup per access, so its bytes are taken to lie in one host copy of title memory" |
| `crates/victory-shader/src/emit/lower/bvh.rs:127-128` | the BVH's root address is taken to be 256-byte aligned |

Limits the BVH header states without the word shortcut: the form that
packs both directions as f16 is refused; instance and procedural nodes
return four invalid words; the descriptor is accepted only with box
sorting, no growth, no bound and barycentrics; and "The numerators' scale
is the emulation's own (the hardware's is not public); programs use the
ratios and the denominator's sign" (`bvh.rs:10-21`, `46-47`).

No citing comment was found that contradicts the code. One statement was
not checked against a device: that the emulation matches a CPU reference
"word for word" is what `emit/bvh_tests.rs:1-7` tests on a host GPU and on
WARP, which this reconstruction could not run.

## Steps

No step of this decision is cited anywhere. The record is cited by item
instead:

| Item | Cited at | What the citing code does |
|---|---|---|
| "the unified-memory item" (also "title memory") | `memory.rs:1`, `emit/lower.rs:4641`, `emit/memory_tests.rs:1`, `emit/mod.rs:122`, `resource.rs:44` | the page table, its lowering, its host side and its tests |
| none named | `emit/lower/bvh.rs:2` | the BVH intersection emulation |
| other items | not cited anywhere | unknown |

## Unknown

* The original title, author, subject and date of writing. The one date
  in a citing comment is that the shader side was "added 2026-11-24".
  The history agrees: commit `0b35b7f` added the BVH citation and commit
  `27c3dc9` the five that name the unified-memory item, both on
  2026-11-24 at 22:36, and nothing cited the number before. Those
  commits are not in a shallow checkout cut at `552208c`; see "What the
  history shows" in [README.md](README.md) for how they were read.
  Inferred: the record existed in some form by 2026-11-24. No record was
  ever a file in the repository.
* The other items of the list, and their number.
* Which alternatives were considered. No citing comment names one.
  (Candidates the reconstruction can think of, as examples only: host
  ray-tracing acceleration structures built from the title's data, one
  large host buffer mirroring all title memory, sparse or reserved
  resources, or refusing such programs.)
* Why 64 KiB pages and a two-level table were chosen. `watch.rs:41-44`
  gives a reason for the 64 KiB unit of change tracking (the console's
  page; finer units cost more than they saved) but cites decision 0007
  for its module, not this one.
* What the record said about results computed from memory with no copy.
  The withholding rules in the code cite runs, not the decision.
* Any measurements.

## References

Six citations in code.

| Where | Item | What the citing sentence says |
|---|---|---|
| `crates/victory-gpu/src/memory.rs:1` | module header, lines 1-2 | title memory at raw GPU addresses; the contract is the shader crate's "Title memory" |
| `crates/victory-shader/src/emit/lower.rs:4641` | doc comment of `Lower::memory` | every access at a computed address goes through this function |
| `crates/victory-shader/src/emit/lower/bvh.rs:2` | module header, lines 1-4 | the two BVH intersection instructions are emulated |
| `crates/victory-shader/src/emit/memory_tests.rs:1` | module header, lines 1-3 | tests of title memory at computed addresses on a host GPU and WARP |
| `crates/victory-shader/src/emit/mod.rs:122` | module header, "Title memory", lines 122-143 | the page-table contract between shader and host |
| `crates/victory-shader/src/resource.rs:44` | doc comment of `MEMORY`, lines 41-44 | lowering takes the address from the instruction's own operands |

Mentioned as missing in `docs/audit-2026-11-26.md:127` (A-B5) and in
`docs/progress.md` ("Needed from the owner", item 6).
