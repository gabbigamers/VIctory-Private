# 0001: Where title memory lies in the address space

Status: Reconstructed on 2026-11-26 from source comments and code. The
original record is not in the repository; where this text goes beyond what
the citations say, it says so.

The title of this record is the reconstruction's. The original title is
not known. File and line numbers are as of commit `854470e`.

This record is short because the decision is cited once, at the end of one
doc comment, with no step, section or item named.

## Context

`xmem.dll` is the console's memory interface, served here from host memory
(`crates/victory-hle/src/modules/xmem.rs:1-4`). The same header says "The
GPU side works from CPU addresses (unified memory)" and that "what
constrains addresses (alignment, 2 MiB pages, address-bit limits) is
honoured".

The problem the cited comment names: a title asks for memory with flags
that limit which addresses it may get, and code in the title depends on
the result. The comment gives one dependency by name: the console's
ray-tracing structures must lie where the console keeps title memory, and
"Grand Theft Auto VI's BVH builder refuses one above".

## Decision

Cited once, as the last sentence of the doc comment of `address_range`
(`crates/victory-hle/src/modules/xmem.rs:71-77`, "Decision 0001."). What
that comment states and the function implements:

* Memory asked for as 32-bit graphics memory is placed in the low 4 GiB.
* All other title memory is placed from 4 GiB upwards. The comment gives
  the reason for the lower bound: the low 4 GiB is "kept for 32-bit
  requests, which the title's big pools would otherwise crowd out".
* The upper bound is an address width taken "from the field at bit 12" of
  the flags, "up to 46 down to 40 bits", "or 40 bits without one". The
  code computes it as 47 minus the field for a field of 1 to 7, and 40
  for a field of 0 (`xmem.rs:78-85`).
* The reason given for the default of 40 bits: "the console keeps title
  memory there, and its ray-tracing structures must be", and "the title
  asks for its GPU heap with no field".

`XMemVirtualAlloc` passes the range to the host as address requirements
for every reservation at an address of our choice (`xmem.rs:94-131`).

## Consequences visible in the code

* A 32-bit request that finds no room below 4 GiB is retried after guard
  reservations there make way (`xmem.rs:157-161`).
* `shortcut:` at `xmem.rs:312-315`: `XMemAllocDefault` serves ordinary
  requests from the host heap, whose "addresses lie above 2^40, unlike the
  console's"; the comment records that nothing seen builds ray-tracing
  structures in that memory and names the upgrade ("Sub-allocate from a
  reservation below 2^40 if one does"). The 40-bit bound is therefore
  applied to `XMemVirtualAlloc` reservations and not to every allocation
  the title makes.
* A test in the same file (`virtual_alloc_meets_console_constraints`,
  `xmem.rs:1117-1129`) asks for 2 MiB pages with the field set to 7 and
  asserts the address is below 2^40. The module is compiled for Windows
  only, so the test runs only there.
* Inferred: the 40-bit default is what lets the shader side cover title
  memory with a fixed table. The page table of decision 0005 has entries
  "per 64 KiB page below 2^40" (`crates/victory-gpu/src/memory.rs:4`),
  and addresses at or above 2^40 read as missing
  (`crates/victory-shader/src/emit/mod.rs:132-133`). No comment links the
  two decisions; the link is the shared bound.

No comment was found that contradicts the code here.

## Steps

No step of this decision is cited anywhere. Whether the record had steps
is unknown.

## Unknown

* The original title, author and date of writing. The history gives the
  date of the citation: commit `2849477` (2026-11-23 23:10) added it, in
  the wording it has now, and that commit's message lists "title memory
  placed below 2^40" among its changes. That commit is not in a shallow
  checkout cut at `552208c`; see "What the history shows" in
  [README.md](README.md) for how it was read. Inferred: the record
  existed in some form by then. No record was ever a file in the
  repository.
* How the meaning of the field at bit 12 and the console's 40-bit default
  were established. The comment states them as facts and names no source.
* Which alternatives were considered. No comment names one. (Two the
  reconstruction can think of, as examples only: placing all memory below
  2^40 regardless of the field, or not honouring the field at all.)
* Whether the record covered more than the address range. Other behaviour
  in `xmem.rs` (page sizes, placeholders for 64 KiB pages, physical-page
  mapping, what `XMemVirtualQuery` reports) cites no decision, so nothing
  ties it to this one.
* Any measurement or failing run behind "refuses one above".

## References

One citation in code.

| Where | Item | What the citing sentence says |
|---|---|---|
| `crates/victory-hle/src/modules/xmem.rs:77` | doc comment of `address_range`, lines 71-77 | The address range each kind of request is given, and why the default upper bound is 40 bits; ends "Decision 0001." |

Mentioned as missing in `docs/audit-2026-11-26.md:127` (A-B5) and in
`docs/progress.md` ("Needed from the owner", item 6).
