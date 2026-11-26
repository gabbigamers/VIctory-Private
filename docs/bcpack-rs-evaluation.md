# bcpack-rs: component-by-component evaluation

2026-11-26, corrected the same day after an independent verification
(what changed: the end of this document). A record of facts for the
owner's decision on audit items A-C15 and A-B6
([audit](audit-2026-11-26.md)); the rules it applies are the owner's:
evaluate each part of `bcpack-rs` on its own, reuse what has a verified
licence and origin, isolate what does not, bring in no unverified tables and
nothing that circumvents access controls. No legal advice: what licence
files, headers and commit messages say is quoted, nothing more.

## 1. Scope and method

* **Evaluated:** `/home/user/bcpack-rs` at `7b2b421` (2026-11-26), a full
  clone (28 commits, not shallow). Its manifest, README and file headers
  name `https://github.com/Ahmet-Dedeler/bcpack-rs`; the clone's `origin`
  is `https://github.com/gabbigamers/bcpack-rs`. That `7b2b421` is on the
  named upstream was not checked (no network use beyond crates.io).
* **VIctory side:** `crates/victory-formats/src/bcpack.rs` (1,612 lines)
  and `bcpack-tables.bin` (77,982 bytes) as committed at `fcb8bac`, read
  with `git show`. The checkout's HEAD has since moved to `2c90526`; both
  files are byte-identical there. The local history is shallow: 58 commits,
  cut at `552208c` (2026-11-25). Uncommitted files in the checkout
  (other work in progress) are not described or relied on. VIctory was not
  built in place.
* **Read:** every file of `bcpack-rs` outside `.git`, all 28 commit
  messages, and the history of `data/`, `src/`, `tools/`, `tests/vectors`,
  `tests/golden.txt`.
* **Built and run**, in copies under the session scratch directory
  (`scratchpad/bcpack-eval/`, `scratchpad/fix/`), nothing inside the
  reference checkout: `cargo fetch --locked` (37 crates from crates.io,
  nothing else downloaded), `cargo test --release --locked` with and
  without default features, `python3 -I python/bcpack.py selftest`,
  regeneration of the generated files, `bcpack bench`, a throughput harness
  (section 6), and a differential harness (`scratchpad/fix/h/diff`): a
  copy of the committed `bcpack.rs` and table file with one wrapper function
  appended, linked with a copy of `bcpack-rs` (sections 5 and 6).
* **Toolchain:** rustc 1.97.0, the default in the scratch directory.
  VIctory pins 1.98.1; it was used for one pair of head-to-head runs and
  nothing else (section 6).
* **Microsoft's Verilog:** a clone of `microsoft/silicon-rans` was already
  in the session scratch directory (HEAD
  `2f6827d8a4e6a78029c372f20d8b61f7cd408fc5`, clean; the commit `bcpack-rs`
  cites). It was read and used as input; this evaluation did not fetch it.
  Its `LICENSE` and the header of `mdu_minirans.v` are the MIT licence,
  "Copyright (c) Microsoft Corporation." The file has one module
  (`decoder`, lines 68-1224).
* **No game data** was present or used. Every statement about retail data
  below is `bcpack-rs`'s own. `bcpack-rs`'s `data/*.json` were read and, in
  scratch only, compiled into its own test and comparison binaries.
* **Words.** *Stated*: the project says so in the file and line given.
  *Verified*: read by us in a licence file, header or commit, or reproduced
  by a script named beside it. *Executed*: run in scratch on synthetic
  streams with the command given. A stated fact that needs retail data to
  check is never called verified.

## 2. Components

Licence, unless a row says otherwise: `Cargo.toml:5`
`license = "MIT OR Apache-2.0"`, README.md:11, and the first lines of
every `.rs`/`.py` file (lines 1-4, or 2-5 after a `#!` line in
`python/bcpack.py`, `tools/gen_tables.py`, `tools/import_tables.py`):
`SPDX-License-Identifier: MIT OR Apache-2.0`, "If you copy, vendor or port
this file, keep this header and credit bcpack-rs in your README."
`data/*.json`, `tests/golden.txt`, `tests/vectors/*.json` and the Markdown
files carry no header. `LICENSE-MIT` is the MIT text, "Copyright (c) 2026
Ahmet Dedeler". `LICENSE-APACHE` is the 15-line short notice that points to
the licence URL, not the full Apache-2.0 text. `THIRD_PARTY_NOTICES.md`
holds one notice: Microsoft's MIT text for silicon-rans. `AGENTS.md:13-20`
asks reusers to keep headers, copy the three licence files next to the
code, credit the project and prefer a dependency over copying.

History: three human authors (Ahmet Dedeler; `brainbudt`: `577bf3d`,
`23020cb`; `yefan911`: `0e49474`), no `Signed-off-by`, no contributor
agreement. Stated in the history and documents, not assessed: 15 of the 28
commits carry a `Co-Authored-By: Claude ...` trailer (among them `79adc29`,
`8b86a5e`, `c55c5fa`, `b1c3099`, `98eaf7f`); `docs/findings/decoder.md:3`
says "Agent `decoder`"; `tools/import_tables.py:15` says "the research
agents' fit outputs". The repository calls itself "Clean-room"
(`AGENTS.md:3`, `CITATION.cff:5`).

Classes: **A** the author's (or a contributor's) own work under the
repository licence; **B** derived from a named third party under a stated
compatible licence, notice present; **C** constants or data the author
derived from retail game data; **D** taken from another project or decoder;
**E** origin not stated or not determinable.

Decisions: **reuse** (may be brought into VIctory under the notices; where
nothing in VIctory needs it today the row says "no present use"),
**test-only** (reuse as a test-only reference), **isolate** (do not bring
in). Every isolate has its own reason; what would change each is in the
table after this one.

| # | Component | What it is; algorithm | Origin as stated | Verified here | Class | Decision |
|---|---|---|---|---|---|---|
| 1 | `src/tables.rs` (919 lines) | 5 tables of 9 x 16 x 10 values: per-segment base, frequency and reciprocal constants of the 144 static rANS models | "Generated by tools/gen_tables.py from mdu_minirans.v in https://github.com/microsoft/silicon-rans (commit 2f6827d), MIT License" (`src/tables.rs:8-10`; notice `THIRD_PARTY_NOTICES.md:5-9`) | regenerated from the Verilog clone (twice, two scratch copies): all 5 x 1,440 values equal, checksum `346464df...` equal; the file differs only in line wrapping | **B**, reproducible | **reuse** (derivable from an MIT source; both notices) |
| 2a | `src/rans.rs:45-222, 560-670`: `Model`, `phase1` (the lane) | one lane as a clocked state machine: header bytes, renormalise, decode one symbol by segment compare and reciprocal multiply, per-lane symbol target, legal end states | "Each lane is a cycle-level port of the `decoder` module in Microsoft's MIT-licensed `silicon-rans` reference Verilog" (`rans.rs:7-8`); "clock-for-clock port of the probe agent's cycle-level simulator (itself a port of the `decoder` module ...)" (`docs/findings/decoder.md:9-10`; the `probe` branch is not in this clone) | the Verilog has the named module, states, header fields, `+1`/`+257` lengths, legal end states and per-lane `out_target`/`done` (`mdu_minirans.v:68, 908-913, 952-954, 1071, 1164, 1171, 1176-1216`); the port was not checked line by line and came through an intermediate simulator we cannot read. No shared text with VIctory's file beyond a `Display` signature | **B** (port; reproducible only as a re-port) | **test-only** now; product use only under option (ii), section 7 |
| 2b | `src/rans.rs:388-557`: `decode_lanes` (feeder, lockstep, end checks) | lanes served in lane order, 8 bytes a clock; symbols released one vector at a time; error unless every lane spends its byte budget and ends in a legal state | "Two parts are not in the published Verilog and were established on retail data" (`rans.rs:14-16`; the same at `BCPACK_SPEC.md:85`); "Xbox2Windows (R0Y00) and MarlomMedeiros' corpus-wide BCPack measurements (issue #15): fixed 8-byte rounds, demand-driven refills, the 2/3-byte length flag, lane-dependent constant models" (`README.md:135`) | the Verilog has one module and only the define `MDU_RANS_FEED_WINDOW 8` (`mdu_minirans.v:35`) for this part: the array around the lane is not published, so this is not derivable from the Verilog. The retail claim is not verifiable here. Executed: VIctory's lane loop returns the same symbols and consumes the same number of bytes on every encoder stream it accepts (section 5) | **A** (code); the rules it implements are stated as retail observations, partly credited to third-party measurements | **test-only**, as 2a |
| 3 | `src/rans.rs:672-1093` fast decoder (`decode_lanes_fast`, `decode_slice`) | same clock model with bit masks per lane, a burst path while all lanes are mid-payload, and a 64 KiB symbol table per model | same header; rewritten by `brainbudt`, commit `577bf3d` "rANS: speed up multi-lane slice decoder by 1.8-2.8x (#8)" | tests assert it equals #2 result for result, cycle for cycle, on valid and invalid inputs (`tests/roundtrip.rs:62-200`): pass. Speed-up measured 1.4-2.6x over #2 (section 6) | **B** (via 2a), with 2b's rules; a contributor's rewrite | **test-only**, as 2a |
| 4 | `src/rans.rs:1095-1122` unit test | reciprocal inverse = exact division for every model and slot | - | passes | A | reuse (test) |
| 5 | `src/format.rs` code | modes, references `(x + unzigzag(d)) mod 256`, dictionary addressing, zigzag and bit interleave helpers | author's; listed for commit `8b86a5e` (`bc345-bc7.md:8`) | read; unit test passes. No shared code form with VIctory's file (V:471-496 does the same with different code; no 12-token run in common outside the width arrays) | A | **test-only** (the encoder needs `Mode`) |
| 6a | `src/format.rs:37` BC1 lane widths | 12 widths | "Status: **exact**. Every lane of 15,000 chunks sampled across all 57 stores ... emits exactly `blocks` symbols, consumes exactly its byte budget and ends in a legal state; changing a lane width, the lane count or the symbol count breaks this." (`BCPACK_SPEC.md:49`) | equal to VIctory's own (V:266) | C | nothing new enters: equal to VIctory's |
| 6b | `src/format.rs:38-42` BC3, BC4, BC5, BC6H, BC7 lane widths | 12 or 16 widths each | no origin statement beyond "Facts used: lane widths and two symbols per lane for 16-byte blocks" (`bc345-bc7.md:21-22`) and, for BC6H, "every chunk in the corpus converges at the rANS layer" (`README.md:23`) | equal to VIctory's own (V:267-271) | E | nothing new enters: equal to VIctory's; an adapter can pass VIctory's arrays instead |
| 6c | `src/format.rs:58-79` codec numbers | 6 numbers of the store's chunk table | "Sources: SixNG format notes (`formats/05-ttpf.md`), paryzius' ttpf-viewer, Leonida `docs/textures.md`" (`BCPACK_SPEC.md:34`; values at :28); no licence for those sources is stated in `bcpack-rs` | read; a `formats/05-ttpf.md` listing these values is in the local `GTA6-Asset-Info`, whose `LICENSE` is the GPL version 3 text (audit: facts only) | D (facts from third-party notes) | **isolate**: left out of the vendored `format.rs`; no test needs them |
| 7 | `src/bc1.rs` | BC1 rebuild: endpoint fields = reference + zigzag residual; index = codebook pattern + index reference along the endpoint line | author's; rules from the research branches (`decoder.md:14-16`); first commit `79adc29`. The lane-1 rule was added in `98eaf7f`, whose test "now matches the reported VIctory output" | read. Shared forms with VIctory's file are small: three `copy_from_slice` lines present since `79adc29`, and the three 16-pixel `fold` helpers of `98eaf7f` (`bc1.rs:275-287` against V:625-627; VIctory's local history has that form at `552208c`, dated about five hours before `98eaf7f`). Embeds `data/bc1_*.json` (`bc1.rs:38-40`) and parses JSON | A (code); the lane-1 part was written to match VIctory's reported output | **test-only**, modified: without its data files (section 7) |
| 8 | `src/bcn.rs` | BC3/BC4/BC5 rebuild: state = reference state + residuals, pair and quadrant rank tables | header: author's. First commit `8b86a5e`, whose findings file says: "\"Reference\" is a third-party decoder used only to measure (never as a source of table values)" and "With the reference's tables converted in memory, the rebuild is byte-identical to the reference on every chunk" (`git show 8b86a5e:docs/findings/bc345-bc7.md`, lines 4-5 and 19). The reference is named VIctory 19 minutes later (`c55c5fa`) | shares structure with VIctory's file: `pack_alpha(e0, e1, ix, out)` / `pack_colour(ep, ix, out)` (`bcn.rs:435-456`) against `bc4_alpha(e0, e1, ix, out)` / `bc1_color(ep, ix, out)` (V:622-640), same parameter lists and shift expressions, two identical lines; 13 lines start a 12-token run that also occurs in V. Weaker than #9, same commit. Embeds `data/bc{3,4,5}.json` (`bcn.rs:252-254`). Executed: 0 differing blocks against VIctory (section 5) | **E**: stated A, but which of the two files was written first is not determinable from the two local histories | **isolate** |
| 9 | `src/bc7.rs` | BC7 rebuild: control code, 8-bit top-aligned endpoints, 4-bit top-aligned indices, block pack and unpack | as #8; anchor tables are the public BC7 ones | shares a run of statements with VIctory's file in identifier names and expression form, beyond the public tables: `bc7.rs:361-374` / V:682-696 (the same doc comment "Little-endian bit writer for one 128-bit block.", `struct Bits { v: u128, at: u32 }`, `fn put` with `if n > 0 && self.at < 128`); `:109` / V:741; `:115-120` / V:748-754; `:402` / V:866 (`let field = \|row: usize, col: usize, p: u8\|`); `:409-411` / V:876-878 (`let drop = ...`, `w.put(field(e / 2, 2 * ch + e % 2, p) >> drop, ... - drop)`); `:417, 421` / V:882, 883; `:432-441` / V:899-907; `:556, 564` / V:805, 817 (`let common = subsets.min(...)`, `if row < common { row } else { 0 }`). 33 lines start a 12-token run that also occurs in V (3 of them at the public tables). All of these are in the file's first commit `8b86a5e` (2026-11-24 05:24 +0300); VIctory's oldest local commit `552208c` (2026-11-25) already has them. Embeds `data/bc7.json` (`bc7.rs:204`). Executed: 0 differing blocks against VIctory | **E**, as #8: one was written from the other or both from a common text; direction not determinable here | **isolate** |
| 10 | `src/encode.rs` | synthetic encoder: reverse rANS per flushed fragment, interleave learned by replaying the decoder | "Synthetic BCPack encoder, for tests and fixtures only" (`encode.rs:5`) | read; round trips pass; no shared form with VIctory's file (3 short lines). Always sets the flush bit (`encode.rs:104`); BC7 control symbols are drawn from `data/bc7.json` through `bc7::Tables` (`encode.rs:20, 243`) | A | **test-only**, modified: the BC7 control list passed in |
| 11 | `src/lib.rs` | module list and `decode_chunk` dispatcher | author's | read: `decode_chunk` calls the embedded tables (`lib.rs:81, 91`) and the modules #8, #9 | A | **isolate as written** (not on origin: it cannot be used without #8, #9 and `data/`); a vendored subset has its own module list |
| 12 | `src/ttpf.rs:1-236` store reader | RDS5 chunk table and ETPF index parser | "Layout as documented by the SixNG format notes and paryzius' ttpf-viewer (whose Gfx10 tiling and mip-tail code this follows)" (`ttpf.rs:8-9`); "store/ETPF reader, Gfx10 mip chain and detiling (port of the harness `ttpf.py`, itself from ttpf-viewer)" (`decoder.md:17-18`; the `harness` branch is not in this clone) | read: plain header fields and zlib only; no cipher or key code anywhere in `src/`, `tools/`, `python/` (searched). So the access-control rule is not what keeps it out | **A or D, not determinable** (a port of a script we cannot read, "itself from" a project whose licence is not stated) | **isolate** |
| 13 | `src/ttpf.rs:238-436` Gfx10 tiling, mip chain | swizzle patterns, tile lookup, mip-tail placement | "port of ttpf-viewer src/tile.rs + src/layout.rs" (`ttpf.rs:239`), "at bafe9af" (`ttpf.rs:251`; `bc345-bc7.md:15`: `github.com/paryzius/ttpf-viewer`) | the repository states no licence for ttpf-viewer and has no notice for it; ttpf-viewer is not among the local repositories | **D**, licence not stated | **isolate** |
| 14 | `src/ttpf.rs:437-603` BC1 pixels, DDS, PNG, tests | BC1 palette, DDS header, PNG writer | author's | read; 2 unit tests pass | A | reuse; no present use (and it sits in one file with #12, #13) |
| 15 | `src/texture.rs` | mip 0 of a texture from a store | author's | read: built on #12 and #13 | A | **isolate as written** (cannot be used without #12, #13) |
| 16 | `src/main.rs` | CLI: decode, symbols, export, scan, bench | author's; `23020cb` by `brainbudt` | read; `bench` run (section 6); uses #11, #12 and the embedded tables | A | **isolate as written** (same dependencies); no present use |
| 17 | `python/bcpack.py` | pure-Python BC1 mirror of #2 and #7 | "cycle-level port of the `decoder` module in Microsoft's MIT-licensed silicon-rans Verilog" (`bcpack.py:17-18`) | `selftest` passes on the two vectors; by default it loads `data/bc1_*.json` (`bcpack.py:322-336`) | B (via 2a) | test-only; no present use |
| 18 | `python/bcpack_tables.py` | #1 for Python | generated, as #1 | regenerated: byte-identical | B, reproducible | reuse; no present use |
| 19 | `tools/gen_tables.py` | parses the five `assign ... = '{...}` tables from the Verilog, writes #1 and #18 with a SHA-256 | author's | run: reproduces #1 and #18 | A | **reuse** (or rewrite; it is 107 lines) |
| 20 | `tools/import_tables.py` | converts BC1 fit outputs to `data/bc1_*.json` | "Inputs are the research agents' fit outputs ... Only the fitted values and summary confidence numbers are copied; no per-texture data and no game bytes" (lines 15-17) | read. Covers only the two BC1 files; no script in the repository produces `data/bc{3,4,5,7}.json` | A | **isolate** (its one function is to write the class C BC1 files) |
| 21a | `tools/retail/crosscheck.py` | compares the Rust decoder with the research pipeline on the caller's texture pools | "Needs your own texture pools and the research branches checked out" (lines 6-7) | read; cannot run: it imports `ttpf`, `mdusim`, `endpoints` from branches that are not in this clone | A | **isolate** (unusable without the absent branches) |
| 21b | `tools/retail/mip_e2e.py` | scores decoded mip 0 against plain mip 1 | the same statement | read; cannot run: imports `ttpf`, `mipscore` from the absent `harness` branch | A | **isolate** (same reason) |
| 22 | `tests/roundtrip.rs` | 14 tests: table checksum, round trips, both decoders agree, padding, truncation, garbage, golden hashes | "Everything here is generated data; no game bytes" (line 6) | 14/14 pass. `default_tables_load` (369) and `bc1_lane1_index_reference` (464) load the fitted BC1 files | A | **reuse** the synthetic cases; not the two that read `data/` |
| 23 | `tests/modes.rs` | 9 tests: pack/unpack round trips, by-hand BC4 and BC7 blocks, shipped tables, synthetic streams, CLI | "synthetic checks only (no game bytes)" (line 5) | 9/9 pass; 5 of them load `data/` tables; all exercise #8 or #9 | A | no present use while #8, #9 are isolated; the by-hand block values are a model for VIctory's own tests |
| 24 | `tests/golden.txt` | 3 SHA-256 of decoded synthetic output | made by `golden_hashes` with `BCPACK_BLESS` from the repository's encoder and a formula-built window and codebook (`roundtrip.rs:41-59, 340-366`) | emptied and re-blessed in scratch (twice, two copies): byte-identical. The generator reads no `data/` file | A, **synthetic** | reuse (needs #7) |
| 25 | `tests/vectors/synthetic_11.json`, `_12.json` | two synthetic BC1 streams with symbols and output hash | "synthetic BCPack BC1 stream from src/encode.rs; bc1_sha256 uses the test tables of tests/roundtrip.rs" (the files' `about`); `roundtrip.rs:383-409` | deleted and re-blessed in scratch: both byte-identical | A, **synthetic** | reuse |
| 26 | `docs/BCPACK_SPEC.md` | BC1 format description with a status word per section | author's; cites SixNG notes, ttpf-viewer, Leonida (line 34) | read | A (text) | cited, not copied |
| 27a | `docs/findings/decoder.md` | how the BC1 decoder was built; retail results | author's ("Agent `decoder`"). Names five retail chunks by store and index | read; not reproducible here | A (text about retail data) | cited, not copied; the chunk identifiers are not repeated |
| 27b | `docs/findings/bc345-bc7.md` | how BC3/4/5/7 rules and tables were obtained | author's | read at HEAD and at `8b86a5e` | A (text about retail data) | cited, not copied |
| 28 | `README.md`, `AGENTS.md`, `CITATION.cff`, `Cargo.toml`, `Cargo.lock`, the three licence files | project metadata | author's | read | A | the licence files and credit line travel with any reused file |
| 29 | `data/*.json` | the constant tables, 11 tables in 6 files | section 3, one row each | section 3 | **C and D** | section 3, one decision each |

### Isolated components and what would change each

| Component | Reason | Evidence that would change it |
|---|---|---|
| #9 `bc7.rs` | shares non-trivial code with VIctory's GPL-3.0-or-later file; its header says it is the `bcpack-rs` author's under MIT OR Apache-2.0; which was first is unknown | in VIctory's full history: `git log -S'Little-endian bit writer for one 128-bit block' -- crates/victory-formats/src/bcpack.rs`, the date compared with 2026-11-24 05:24 +0300. If VIctory's is earlier, the module is VIctory's own code under another header, and a test against it proves nothing. If `bcpack-rs`'s is earlier, VIctory's file contains code that came from `bcpack-rs` and carries none of the header or credit `AGENTS.md:13-20` asks for; the module could then be vendored, but would still not be an independent check |
| #8 `bcn.rs` | the same question on weaker evidence (helper pair of the same shape), same first commit as #9 | the same search for `fn bc4_alpha`; and the answer for #9 |
| #6c codec numbers | six numbers whose stated source is third-party format notes; no decoder test uses them | a use for them in VIctory; they are facts the audit already allows citing |
| #11 `lib.rs`, #15 `texture.rs`, #16 `main.rs` | not origin: each calls isolated modules or the embedded tables | nothing needed; they follow the parts they call |
| #12 store reader | stated as a port of a script we cannot read, "itself from ttpf-viewer", whose licence is not stated. By content it reads only plain headers and zlib | the `harness` branch's `ttpf.py` and ttpf-viewer's licence file at `bafe9af`; and a use for an offline store reader in VIctory (none today) |
| #13 tiling port | derived from a project whose licence the repository does not state | ttpf-viewer's licence file at `bafe9af` and its notice. VIctory has its own layouts, so there is no present use either |
| #20 `import_tables.py` | writes only the class C BC1 files | a decision to accept those files (section 3) |
| #21a, #21b retail checks | cannot run: they import modules from branches absent from the repository | those branches. VIctory's policy already allows tests that take real files from environment variables, so the reason is not that they read retail data |
| #29, each table | section 3 | section 3 |

## 3. Data tables one by one

### Where the "reference decoder" leads

* `data/bc4.json:2`: "reference decoder tables (GPL project; values are
  constants of the format, see NOTE.md)". `bc3.json:2`, `bc5.json:2`,
  `bc7.json:2`: "reference decoder tables" (the field covers the whole
  file, the pair ranks and the 205 control codes included). No `NOTE.md`
  exists in any of the 28 commits.
* Commit `c55c5fa` (2026-11-24 05:43 +0300) names it: "These are the
  format's constant tables as a reference decoder (VIctory) carries them,
  converted to this repo's JSON layout. The entries solved from data earlier
  are unchanged and equal them." VIctory's manifest says
  `GPL-3.0-or-later`.
* Commit `8b86a5e`, 19 minutes earlier, already had the reference in use:
  its findings file says the rebuild was checked "With the reference's
  tables converted in memory" and scores the solved tables against the
  reference ("64/64 each", "205/205").
* Commit `b1c3099` (BC1 window) says "the format's constant BC1 dictionary
  as a reference decoder carries it" without a name. Its numbers reproduce
  against VIctory's file: 180 of the 254 fitted slots equal, 74 not, and
  all 255 shipped values equal (table below).
* Commit `98eaf7f`: the BC1 lane-1 rule was changed and the new test "now
  matches the reported VIctory output".
* Not found: which VIctory revision was read, how the table file was
  converted (no script; `tools/import_tables.py` handles BC1 fits only), and
  the fit scripts themselves (`tools/sem/fit_rgb.py`, `tools/bench`: on
  branches absent from this clone).
* VIctory does not record where its own tables came from (A-B6), and its
  local history is shallow: the commit that added the file is not in it.
* Other local repositories hold BCPack decoder files too (`SixNG`, whose
  `LICENSE` is the GPL version 3 text: `crates/sixng-core/src/bc*pack*.rs`;
  `rpf9-explorer/Core/Bcpack*.cs`; `gta6-explorer/gta6x/bc/bcpack`). Their
  tables were not compared here. No file or commit of `bcpack-rs` names any
  of them as a source of table values.

What this supports:

* **Not independent of VIctory, by the project's own statement and equal
  values:** all four dictionaries, the BC3 colour ranks, 80 BC7 control
  codes and 75 BC1 window slots.
* **Stated independent, not verifiable here:** the three pair rank tables
  and 205 BC7 control codes ("used only to measure (never as a source of
  table values)", `8b86a5e` findings). They were committed before the
  import, but that commit already reports their agreement with the
  reference, whose tables had been converted in memory; every value is
  equal, so the numbers cannot show independence either way.
* **Numerically shown independent:** only the BC1 codebook (4 entries
  differ from VIctory's) and the BC1 window fit (74 of 254 slots differed
  before `b1c3099`).

### The tables

Sections of VIctory's `bcpack-tables.bin` (77,982 bytes) by the layout in
`bcpack.rs:127-133`: models 0-2303; BC1 ranks 2304 (256 B); BC3 alpha ranks
2560 (64); BC3 colour ranks 2624 (256); BC4 ranks 2880 (64); BC5 ranks
2944 (64); BC7 control 3008 (512 u16); dictionaries BC1 4032 (5,610), BC3
9642 (10,200), BC4 19842 (4,590), BC5 24432 (9,180), BC6H 33612 (14,790),
BC7 48402 (29,580). Compared by a read-only script (counts and entry
numbers only); two such scripts written separately gave the same counts.

Decision codes: **I-C** isolate: derived from game data by the project's
account; `asset-policy.md` accepts one such file and "nothing else of this
kind". Changed by: the owner's answer to the audit's gate question and a
policy decision, or a licensed public source holding the values (none is
named in the repository). **I-D** isolate: VIctory's own values coming
back, of the same unrecorded origin. Changed by: a record of where
VIctory's tables came from (A-B6).

| Table (`bcpack-rs`) | Size | Origin as stated | Class | VIctory section; mapping | Result | Decision |
|---|---|---|---|---|---|---|
| model tables (`src/tables.rs`, not in `data/`) | 5 x 1,440 | Microsoft Verilog, MIT | B | models: `freq[16e+q][s]` = `FREQ_TABLE[e][q][s]`, `e` < 8 | 704/704 used entries equal, the 448 unused are zero; starts accumulated VIctory's way equal `BASE_TABLE` 704/704; (frequency, cumulative) equal for all 8,160 symbols of the 128 shared models. `bcpack-rs` has 16 more (9-bit alphabets) no layout uses | **reuse** |
| `bc1_quad_table.json` entries | 256 x 4 codes, plus per-entry fit statistics | "fitted (tools/sem/fit_rgb.py) on decoded retail-pool data with exact endpoints; symbols 0-3 measured exactly" (line 3). The file has one commit (`79adc29`); `BCPACK_SPEC.md:152` states it "has not been compared with a reference yet" | C | BC1 ranks: VIctory's 2-bit field `k` = line position of code `k` (codes 0,1,2,3 = positions 0,3,1,2) | **252 equal, 4 different: symbols 229, 235, 247, 248.** Each pair holds the same two patterns in swapped order. Both tables are bijections | I-C; also needs ground truth for the four symbols |
| `bc1_window.json` slots, 180 | 6 fields each | "fitted against mip-1 consistency on retail texture pools" (line 3) | C | BC1 dictionary, endpoint half: slot `s` = entry `255 - s` | equal 180/180 | I-C (nothing new: equal to VIctory's) |
| `bc1_window.json` slots, 75 (`"from": "reference"`) | 6 fields each | "the 74 that differed (kept as fitted_value) and slot 4 now hold the reference value" (line 3) | D (reference unnamed in this file; values equal VIctory's) | same | equal 75/75; none of the 74 kept `fitted_value`s equals VIctory. Slot 0 is null on purpose | I-D |
| (no counterpart) | - | - | - | BC1 dictionary, index half (255 x 16 B, 254 entries non-zero) | `bcpack-rs` reads zero there and counts the block as unfitted (`bc1.rs:245-246`) | - |
| `bc3.json` pairs | 64 x 2 | "substitution cipher: bigram likelihood ... against pair patterns counted on the plain mips of the same textures" (`bc345-bc7.md:33`), commit `8b86a5e` | C (independence of VIctory stated only) | BC3 alpha ranks: `[v & 7, v >> 4 & 7]` | 64/64 equal; predates `c55c5fa`, unchanged since | I-C (nothing new) |
| `bc3.json` quads | 256 x 4 | "could not be determined from retail data" (commit `c55c5fa`); statistical attempt "got 199 of 256 right" (`bc345-bc7.md:46-48`) | D (VIctory) | BC3 colour ranks, 2-bit fields | 256/256 equal; all null at `8b86a5e` | I-D |
| `bc3.json` dictionary | 255 x (8 + 32) | as above; fits got "61/428 entry halves exact" (`bc345-bc7.md:49-51`) | D (VIctory) | BC3 dictionary; endpoints reordered (VIctory colour first, `bcpack-rs` alpha first) | 255/255 endpoint halves, 255/255 index halves | I-D |
| `bc4.json` pairs | 64 x 2 | as BC3 pairs | C (stated only) | BC4 ranks | 64/64 equal; predates `c55c5fa` | I-C (nothing new) |
| `bc4.json` dictionary | 255 x (2 + 16) | `c55c5fa` | D (VIctory) | BC4 dictionary | 255/255, 255/255 | I-D |
| `bc5.json` pairs | 64 x 2 | as BC3 pairs | C (stated only) | BC5 ranks | 64/64 equal; predates `c55c5fa` | I-C (nothing new) |
| `bc5.json` dictionary | 255 x (4 + 32) | `c55c5fa` | D (VIctory) | BC5 dictionary | 255/255, 255/255 | I-D |
| `bc7.json` control, 205 (modes 1, 3-7) | 6 numbers each | "rank order ... of the 285 lane-15 pairs", anchor rule (`bc345-bc7.md:34`), `8b86a5e` | C (stated only) | BC7 control word: mode bits 0-2, rotation 3-4, index selection 5, subsets 6-7, partition 8-13 | 205/205 equal; all 205 present at `8b86a5e` and unchanged | I-C (nothing new) |
| `bc7.json` control, 80 (mode 0: 16, mode 2: 64) | 6 numbers each | "could not be determined from retail data" (`c55c5fa`) | D (VIctory) | same | 80/80 equal. VIctory has 285 non-zero codes, the same 285 | I-D |
| `bc7.json` dictionary | 255 x (24 + 32), `subsets` | `c55c5fa`; `"subsets": 1` set afterwards (`f5145dd`) | D (VIctory) | BC7 dictionary: endpoints = VIctory u16 >> 8 (low bytes all zero); indices = VIctory value at its width, top-aligned to 4 bits | 255/255 endpoints, 255/255 indices. VIctory's subset field is 0 in all 255 entries, `bcpack-rs` says 1: both make every row build on row 0 (`bcpack.rs:805-817`, `bc7.rs:556-564`) | I-D |
| (no counterpart) | - | - | - | BC6H dictionary (255 x 58 B) | `bcpack-rs` has no BC6H rebuild | - |

Derivable without retail data from a licensed source: the model tables
only. Every other table is, by the project's own account, fitted or solved
from retail texture pools or copied from VIctory; the repository names no
other source and does not say how the pools were obtained.

### The four BC1 symbols (A-C15)

Reproduced on the committed tables. What `bcpack-rs` says of those entries:

* "Fits on disjoint halves of the stores agree on 248/256 entries (99.76%
  of quadrant occurrences); the 8 that differ are 4 rare mirror pairs
  (229/235, 230/231, 247/248, 254/255)" (`docs/BCPACK_SPEC.md:142`).
* "The 4 uncertain codebook pairs (5.4). The index codebook has not been
  compared with a reference yet" (`docs/BCPACK_SPEC.md:152`).
* All four differing entries carry `stable_halves: false`; the file's
  `margin` field is 0.021 for 229 and 235 and about 1.6 for 247 and 248;
  their `share` fields sum to about 0.1 % of quadrants.

The other two flagged pairs (230/231, 254/255) are equal in both tables.
Neither project offers ground truth for the two that differ. Executed: on
81,920 synthetic BC1 blocks with lane 1 = 0, the two decoders with their
own tables differ on exactly the 4,948 blocks that use one of the four
symbols and on no other.

## 4. Dependencies

Licences as each crate's manifest declares them in the cargo registry after
`cargo fetch --locked`. "VIctory" is the committed `Cargo.lock` (101
packages).

| Crate | Version | Declared licence | Needed by | In VIctory's lock |
|---|---|---|---|---|
| serde_json | 1.0.154 | MIT OR Apache-2.0 | library (direct) | no |
| itoa | 1.0.19 | MIT OR Apache-2.0 | library (serde_json) | no |
| zmij | 1.0.23 | MIT | library (serde_json) | no |
| memchr | 2.8.3 | Unlicense OR MIT | library (serde_json) | 2.8.3 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | library (serde_json) | 1.0.229 |
| flate2 | 1.1.11 | MIT OR Apache-2.0 | `ttpf` (direct) | no |
| png | 0.18.1 | MIT OR Apache-2.0 | `ttpf` (direct) | no |
| crc32fast | 1.5.5 | MIT OR Apache-2.0 | `ttpf` | no |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 | `ttpf` (png) | no |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 | `ttpf` (flate2) | 0.9.1 |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 | `ttpf` (png) | no (0.9.1 only) |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | `ttpf` | 2.0.1 |
| simd-adler32 | 0.3.10 | MIT | `ttpf` | no |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | `ttpf` (png) | 2.13.2 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | `ttpf`, tests | 1.0.5 |
| rayon | 1.12.0 | MIT OR Apache-2.0 | `cli` (direct) | no |
| rayon-core | 1.13.0 | MIT OR Apache-2.0 | `cli` | no |
| crossbeam-deque | 0.8.8 | MIT OR Apache-2.0 | `cli` | no |
| crossbeam-epoch | 0.9.21 | MIT OR Apache-2.0 | `cli` | no |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | `cli` | no |
| either | 1.19.0 | MIT OR Apache-2.0 | `cli` | no |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | tests (direct, dev) | no |
| digest | 0.10.7 | MIT OR Apache-2.0 | tests | no |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | tests | no |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 | tests | no |
| generic-array | 0.14.7 | MIT | tests | no |
| typenum | 1.20.1 | MIT OR Apache-2.0 | tests | no |
| version_check | 0.9.5 | MIT/Apache-2.0 | tests (build) | no |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | tests | 0.3.1 only |
| libc | 0.2.191 | MIT OR Apache-2.0 | in the lock, not built on this host | 0.2.191 |
| serde, serde_derive | 1.0.229 | MIT OR Apache-2.0 | in the lock, not built | 1.0.229 |
| proc-macro2, quote | 1.0.107, 1.0.47 | MIT OR Apache-2.0 | in the lock, not built | same versions |
| syn | 3.0.10 | MIT OR Apache-2.0 | in the lock, not built | 3.0.10 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | in the lock, not built | 1.0.27 |
| zlib-rs | 0.6.8 | Zlib | in the lock, not built | no |

37 packages besides the crate itself (the lock file has 38 entries). None
is ruled out by its declared licence; the question for each is only whether
anything VIctory would take needs it.

With default features off (`cargo tree --no-default-features`) the library
needs `serde_json`, `itoa`, `memchr`, `serde_core` and `zmij`, and only to
parse the embedded `data/*.json` (`bc1.rs:30`, `bcn.rs:41`, `bc7.rs:45`).
The files proposed for test-only reuse (`rans.rs`, `tables.rs`,
`format.rs`, `encode.rs` once its `bc7` import is replaced by an argument)
use the standard library and nothing else: no new dependency enters
VIctory. `bc1.rs` adds `serde_json` and its three new crates unless its
JSON constructors are removed with the embedded files (its `Window` and
`QuadTable` can be built directly, as `tests/roundtrip.rs:41-59` does).
`rans.rs:497` holds the crate's one `unsafe` block in library code (an
unchecked symbol write in the per-lane decoder, guarded by a
`debug_assert`).

## 5. Comparison with VIctory's decoder

V = `crates/victory-formats/src/bcpack.rs` at `fcb8bac`; B = `bcpack-rs`
`src/`. "Executed" rows come from the scratch harness
(`cd scratchpad/fix/h/diff && cargo build --release --locked --offline &&
./target/release/vdiff && ./target/release/rebuild`): the committed V with
its shipped table file against B with its own tables, on streams from B's
encoder. "By reading" means from the two sources only.

| Feature | VIctory | bcpack-rs | Can output differ? |
|---|---|---|---|
| Formats, entropy layer | BC1, 3, 4, 5, 6H, 7 (V:265-272) | the same six, equal lane widths (B `format.rs:37-42`) | no |
| Formats, block rebuild | BC1, 3, 4, 5 (V:526-551), BC7 (V:757-845), BC6H (V:1057-1159) | BC1 (`bc1.rs:196`), BC3/4/5 (`bcn.rs:536`), BC7 (`bc7.rs:612`); BC6H refused (`lib.rs:69-73`) | - |
| Models | one frequency per bucket, starts accumulated, reciprocal `ceil(2^32/f)` (V:143-166, 361-368) | Verilog tables and its reciprocal path, or a symbol table (B `rans.rs:151-170, 96-101`) | no: equal for every symbol of every shared model (section 3) |
| Lane step, feeding, lockstep | one pass per round: step, fetch (8 a round, lane order), deliver (V:427-459) | two phases per clock as in the Verilog (B `rans.rs:459-523`) | executed, 8,400 streams made by B's encoder and accepted by B's decoders (6 layouts x 7 shapes x 200 seeds): wherever V returns a result (6,028) the symbols are equal and V consumes exactly the stream; V never returns different symbols. `bcpack-rs` also states 0 differing blocks against VIctory on 5,612 retail BC3/4/5/7 chunks (`c55c5fa`); not verified here |
| Symbol budget per lane | none: a lane whose fragment has ended asks for the next header whether or not it has symbols left (V:374-380, 434-439) | `out_target` per lane; a finished lane asks for nothing (B `rans.rs:623-664`), as in the Verilog, where a lane with no output remaining goes idle and sets `done` (`mdu_minirans.v:1193-1195, 1211-1214`, read) | **yes, executed.** V rejects 2,372 of those 8,400 streams ("the stream ends early or is corrupt"): a lane that has delivered its last symbol takes bytes meant for a lane still loading. By shape, errors per 200 streams and layout: 1 block, BC1 and BC4 200 (BC3/5/6H/7: 1); 2 blocks 1-8; 17 blocks with fragments of at most 4 symbols 103-148; 64 blocks/20: 87-102; 300/50: 42-73; 1,000/150: 36-43; 4,096/2,000: 0-2. With 64 zero bytes appended (6,000 streams): 4,023 equal, 1,930 errors, **47 decode with a wrong last block and no error**; no difference before the last block. VIctory's own test encoder appends a filler fragment per lane for this reason (V:1227-1241) |
| Fragment end without the flush bit | "a byte read in this step does not reach the carried state" (V:375) | the state continues across the fragment boundary (B `rans.rs:629-645`; `mdu_minirans.v:1177, 1183-1192`) | **yes, executed**, narrowly: one BC4 lane's payload split into an unflushed and a flushed fragment at each of 42 byte positions. B (both decoders) returns the original symbols for all 42; V for 39, different symbols for 2 and an error for 1: the three splits of 1 to 3 bytes, which end while the four state bytes are still being read. `bcpack-rs`'s encoder always flushes, so its streams never exercise this; whether retail streams contain such fragments is not stated anywhere |
| End-state check | none | every lane must end in 2^24, 2^16 or 2^8, else `IllegalEndState` (B `rans.rs:526-531`, `946-951`) | corrupt input: V can return blocks where B returns an error |
| Bytes after the stream | ignored (V:253-254) | must be zero, else `TrailingData` (`decode_slice`, B `rans.rs:1080-1093`); `decode_lanes_fast` itself does not check | accept or reject only |
| Stalled or short input | error after 4 idle rounds (V:461-464) | `NoProgress` after 64 clocks, `CycleLimit` (B `rans.rs:549-557`) | accept or reject only |
| Bad header | width minus narrowing, no range check; lanes are at least 4 bits wide so it cannot go negative (V:320-322) | narrowing below zero or an alphabet index above 8 is `BadHeader` (B `rans.rs:584-592`) | no for the six layouts |
| References, history; BC3, BC4, BC5, BC7 rebuild | 256-block ring, dictionary before block 0 (V:472-496); u16 endpoints and i16 indices at their own width for BC7 (V:68-78, 670-680) | the same addressing (B `format.rs:209-244`); u8 and 4-bit top-aligned BC7 state (B `bc7.rs:171-177`) | executed: **0 differing blocks** on 294,000 BC3, 298,000 BC4, 294,000 BC5 and 292,000 BC7 synthetic blocks, with 32,097 to 40,703 dictionary references per format. The tables are equal (section 3) and the rules were matched to VIctory (section 2, #8 and #9), so this is consistency, not independent confirmation |
| BC1 index reference before block 0 | reads the BC1 dictionary's index half (V:539) | reads zero, block counted unfitted (B `bc1.rs:241-247`) | **yes, executed**: with the encoder's random lane 1, 148,148 of 298,000 BC1 blocks differ. `bcpack-rs` says lane 1 is 0 in every retail chunk it measured (`BCPACK_SPEC.md:108`). With lane 1 = 0 and the four symbols excluded: 0 of 81,920 differ |
| BC1 codebook | section 3 | section 3 | **yes**: four symbols (4,948 of 4,948 blocks using them) |
| BC7 unknown control code | error (V:767-769) | block of zeros and a count; `decode_chunk` turns it into an error (B `bc7.rs:625-639`, `lib.rs:82-87`) | no through `decode_chunk` |
| Errors | `Error(&'static str)` (V:17-26) | `DecodeError` with six cases (B `rans.rs:243-256`); `expect` on the embedded JSON (B `bc1.rs:86, 157`, `bcn.rs:342`, `bc7.rs:306`) | - |
| Output | into the caller's buffer; `Vec<u8>` of records, 12-32 B a block (V:273-276) | owned `Vec`; `Vec<u16>` of symbols, 24-64 B a block, reserved fallibly (B `rans.rs:755-759`) | - |
| Threading | single-threaded pure function; called from the storage module's worker pool, tables parsed once (`victory-hle/src/modules/dstorage.rs:431-436, 894, 921-924`) | single-threaded per chunk; model tables built lazily in process-wide `OnceLock`s, 66 KiB per model used by the fast decoder and 128 KiB more per model used by the per-lane one (B `rans.rs:64-72, 133-146, 224-239`); threads only in the CLI | no |
| Tables | one binary file, size and "every model tiles 65536" checked (V:134-160); ranks and dictionaries unchecked | models generated with a checksum test; the rest JSON parsed and range-checked at first use | - |
| Tests | 10 run, 2 ignored (V:1163-1611). Exact blocks asserted for BC1 (2 blocks, values credited to `bcpack-rs`, V:1280-1283), BC4 (2), BC7 mode 6 (1); lane round trips for all six layouts; malformed input; `noise_never_panics`; table lookups against a scan | 28 run: 4 unit, 14 `roundtrip.rs`, 9 `modes.rs`, 1 doc test. Round trips through its encoder for all five rebuilt formats, by-hand BC1, BC4, BC7 blocks, pack/unpack inverses, both entropy decoders against each other, golden hashes, Python agreement | - |
| Size | one file, 1,612 lines (1,160 code, 452 tests) and a 78 KB table file | library 4,063 lines in 9 files besides 919 generated; CLI 463; tests 903; Python 499 + 840; 278 KB of JSON | - |

Two scratch experiments on a copy of V (not VIctory code, not a proposed
patch; `scratchpad/fix/h/diff2`, `diff3`):

* **A symbol budget is a small change.** Seven added lines in `lanes`
  (count the symbols delivered per lane; a lane back at a header with all
  of them delivered asks for nothing) make V equal B on all 8,400 streams
  and all 6,000 padded streams, consuming exactly the stream each time.
* **The unflushed case is not explained by the dropped byte alone.**
  Carrying the byte as well (one line) leaves 3 of 42 split points wrong
  (the one error becomes a third wrong result). The cause was not isolated.

Tests on either side prove self-consistency. Neither side's tests can show
that a table is right.

## 6. Performance

Machine: 4 cores, Intel Xeon 2.10 GHz, 15 GB, Linux 6.18, shared with
other work (load average 1.0-2.5 during the runs). rustc 1.97.0, the
default toolchain in the scratch directory, unless a row says 1.98.1;
`bcpack-rs`'s release profile (opt-level 3, thin LTO, one codegen unit).
Synthetic streams from `src/encode.rs` only. MB/s are megabytes of BC
output per second on one core, median of 40 runs, 16,384 blocks (65,536
blocks gave figures between 3 % higher and 12 % lower). The harness was run
three times; cells agree within about 10 %.

### bcpack-rs alone

Harness (ours, `examples/eval_bench.rs` in the scratch copy):
`cargo build --release --locked --example eval_bench &&
./target/release/examples/eval_bench 40`. Two stream shapes: *sampled*
(`encode::synthetic(mode, 7, blocks, 2000)`: fragments of up to 2,000
symbols, a random model each) and *uniform* (uniform symbols, one fragment
per lane, the shape of VIctory's own benchmark records).

| Stream | Decoder | BC1 | BC4 | BC5 | BC3 | BC7 | BC6H |
|---|---|---|---|---|---|---|---|
| sampled | per-lane exact (`decode_lanes`), entropy only | 26.3 | 27.2 | 27.8 | 27.3 | 15.7 | 16.9 |
| sampled | fast (`decode_lanes_fast`), entropy only | 47.5 | 50.7 | 50.1 | 48.2 | 38.5 | 36.6 |
| sampled | fast + rebuild (`decode_chunk`) | 41.2 | 38.7 | 41.4 | 41.5 | 23.9 | - |
| uniform | per-lane exact, entropy only | 49.1 | 47.3 | 48.5 | 49.8 | - | 35.1 |
| uniform | fast, entropy only | 121.6 | 120.6 | 124.0 | 124.6 | - | 50.1 |
| uniform | fast + rebuild | 85.2 | 73.6 | 80.6 | 78.3 | - | - |

In compressed input the BC1 figures are 19 (exact) and 35 MB/s (fast) on
sampled streams, 61 and 152 MB/s on uniform ones. The fast decoder is
1.8-2.6 times the exact one on the 12-lane formats and BC7, 1.4 times on
uniform BC6H; commit `577bf3d`'s title says 1.8-2.8.

The repository's own benchmark, `bcpack bench --format F chunk N 2` on
sampled chunks written to scratch (CPU time, fast + rebuild): BC1 48.5
(4,096 blocks) and 42.1 (65,536) MB/s; BC4 45.6 and 39.5; BC5 43.3 and
41.0; BC3 40.2 and 38.4; BC7 23.4 and 22.5. Its test suite has no ignored
benchmark. The README's "34-40 MB/s of BC1 output per core" is the
project's figure on retail chunks, before `577bf3d`.

### Head to head with VIctory (indicative)

One binary (`scratchpad/fix/h/diff`, `./target/release/h2h`), the same
bytes for all three decoders, outputs compared before timing, two runs per
toolchain; the ranges are over the formats and both runs. VIctory's side is
a copy of the committed file with its shipped tables, built outside its
workspace: indicative, not a measurement of VIctory's own build. Streams
are the first seed VIctory accepts (seed 7 for all).

| Entropy layer only | VIctory `lanes` | B per-lane | B fast | fast / VIctory |
|---|---|---|---|---|
| sampled, BC1/3/4/5, rustc 1.97.0 | 44.4-45.3 | 24.5-27.4 | 49.1-52.7 | 1.09-1.17 |
| sampled, BC1/3/4/5, rustc 1.98.1 | 40.7-42.7 | 25.8-28.0 | 50.0-53.1 | 1.17-1.30 |
| sampled, BC7, 1.97.0 / 1.98.1 | 26.0-26.2 / 24.0-24.5 | 14.3-15.1 / 14.9-15.3 | 37.3-38.0 / 37.2-37.8 | 1.43-1.45 / 1.54-1.55 |
| sampled, BC6H, 1.97.0 / 1.98.1 | 28.0-28.3 / 26.8-26.9 | 15.0-16.0 / 16.1-17.0 | 35.8-36.0 / 37.8-37.9 | 1.27-1.29 / 1.40-1.41 |
| uniform, BC1/3/4/5, 1.97.0 | 73.3-76.1 | 45.0-47.9 | 119.4-123.9 | 1.58-1.66 |
| uniform, BC1/3/4/5, 1.98.1 | 67.9-71.4 | 45.5-48.4 | 121.7-124.4 | 1.72-1.83 |
| uniform, BC6H, 1.97.0 / 1.98.1 | 53.0-53.7 / 48.0-49.4 | 32.9-33.2 / 32.8-33.0 | 50.0-50.2 / 48.4-50.1 | 0.93-0.95 / 0.98-1.04 |

Two per-lane cells (17.7 and 23.5) fell far outside their range in one run
each and are left out as load spikes.

* The per-lane exact decoder is roughly 0.55-0.7 times VIctory's present
  lane code: slower on every format and shape.
* The fast decoder is faster than VIctory by 1.1-1.3 times on sampled
  12-lane streams, 1.4-1.55 on BC7, 1.6-1.8 on uniform 12-lane streams, and
  level or slower on uniform BC6H.
* VIctory entropy plus rebuild: BC1 36.5-39.4 (sampled) and 60.4-62.4
  (uniform), BC4 37.8-39.4 and 57.5-59.5, BC7 18.7-19.3 (sampled); inside
  the "BC1 roughly 30-60 MB/s, BC7 15-20 MB/s" of its source comment
  (V:403-405). On the same streams `bcpack-rs`'s fast decoder plus its
  rebuild gave 38.7-41.5 (sampled 12-lane), 23.9 (BC7) and 73.6-85.2
  (uniform) in the other binary: level on sampled 12-lane streams, about
  1.2 times on BC7, 1.2-1.4 times on uniform ones. VIctory's BC6H rebuild
  returned an error on these random-symbol streams and was not timed.

**Not yet measured:** VIctory's own build under its pinned toolchain on an
idle machine. Its benchmark is the ignored test `bcpack_throughput`
(V:1554-1584: BC1, BC4, BC7, 16,384 blocks, uniform symbols, one fragment
per lane, 40 runs, entropy plus rebuild), run with `cargo test --release -p
victory-formats bcpack_throughput -- --ignored --nocapture`. The
like-for-like run is the harness above moved into a VIctory test: one
binary, streams from `encode.rs`, VIctory's rebuild tables on both sides,
outputs compared before timing, entropy layer and rebuild timed apart, both
stream shapes. It needs the symbol budget of section 5 first, or streams
must be picked by seed as here. Which stream shape retail data resembles is
not known to this evaluation.

## 7. Integration options

### The interface

`bcpack.rs` becomes a module of three files (`tables`, `entropy`,
`blocks`); the public `decode(bc, src, dst, tables)` and its caller
(`dstorage.rs:894`) do not change. The seam is the record buffer VIctory
already has between `lanes` (V:406) and the rebuild functions (V:281-286):

* `struct LaneLayout { widths: &'static [u8], per_block: usize }`: the six
  layouts of V:266-271 (VIctory's own arrays; the reference takes widths as
  an argument).
* `trait EntropyDecoder`, in `victory-formats/src/bcpack/entropy.rs`:
  * `fn decode(&self, layout: &LaneLayout, src: &[u8], blocks: usize,
    records: &mut [u8]) -> Result<EntropyEnd, Error>`: fills one record of
    `widths.len() * per_block` symbols per block, in VIctory's present
    order;
  * `fn name(&self) -> &'static str`, for logs and test output.
* `struct EntropyEnd { consumed: usize, end_states: [u32; 16] }`: what a
  strict caller checks (legal end states, zero padding) and a tolerant
  caller ignores.
* Implementations: `Stepwise` (today's code: the trait method wraps the
  closure-taking `lanes`, which stays as the inner function because
  VIctory's test interleaver calls it with per-lane queues, V:1230-1255);
  `Reference` and `ReferenceFast`, adapters over the vendored
  `decode_lanes` and `decode_lanes_fast` that copy lane-major `u16` symbols
  into block-major `u8` records. All six layouts are at most 8 bits wide,
  so nothing is lost. The adapters live behind `#[cfg(test)]` for option
  (i) and behind a cargo feature for option (ii).
* **Strictness.** Unmodified, the reference decoders are strict: they
  return `IllegalEndState` before any output (`rans.rs:526-531, 946-951`)
  and expose final states only on success. A tolerant `Reference` adapter
  needs a change in the vendored `rans.rs` (return the end states instead
  of the error). Only the zero-padding check is the caller's already
  (`decode_slice`).
* `fn decode_with(entropy: &dyn EntropyDecoder, bc, src, dst, tables)`;
  `decode` calls it with the default.

### What reuse means in practice

A cargo dependency on `bcpack-rs` (the audit's reuse table says "as a
dev-dependency") is not compatible with isolating `data/`: `bc1`, `bcn` and
`bc7` are unconditional modules (`lib.rs:36-38`) and embed all six JSON
files at compile time (`bc1.rs:38, 40`, `bcn.rs:252-254`, `bc7.rs:204`), so
any dependency on the crate, default features off included, compiles the
class C and D tables into VIctory's test binaries, and brings `bc7.rs` and
`bcn.rs` with it. `AGENTS.md:20` (prefer a dependency) cannot be followed
for this reason.

Reuse therefore means a **vendored, modified copy** of `rans.rs`,
`tables.rs`, `format.rs` and `encode.rs`, and of `bc1.rs` if a BC1 rebuild
reference is wanted: `format.rs` without `from_codec`/`codec` (its six
width arrays equal VIctory's own, so no new value enters); `encode.rs`
without its `bc7` import, the BC7 control symbols passed in (VIctory's own
285 codes); `bc1.rs` without the two
`include_str!` constants, `default_table` and the JSON constructors;
headers kept, the changes noted in each file, `LICENSE-MIT`,
`LICENSE-APACHE` and `THIRD_PARTY_NOTICES.md` beside the code, the credit
line, commit `7b2b421` recorded. Not `bcn.rs`, `bc7.rs`, `lib.rs` or
anything under `data/`.

### The options

| | (i) keep VIctory's decoder; `bcpack-rs` as test-only reference | (ii) entropy layer replaced by the port of Microsoft's lane decoder with `bcpack-rs`'s feeder, behind the trait | (iii) entropy and rebuild replaced for BC1/3/4/5/7; VIctory keeps BC6H |
|---|---|---|---|
| Reused | the vendored subset above, test-only; #1 generated from the Verilog | #1, #2a, #2b, #3 in the product; #10 for tests | all of (ii) plus #5, #7, #8, #9 in the product |
| Notices | file headers kept; the three licence files beside the code; credit line; commit recorded | the same, shipped with binaries | the same; and #8, #9 carry a header whose origin statement is in question (section 2) |
| Correctness | independent evidence for the entropy layer only: it already found that VIctory rejects streams the reference accepts and can return a wrong last block (section 5). For BC1 rebuild it is a second implementation with the lane-1 rule matched to VIctory. For BC3/4/5/7 it offers nothing independent: A-C15's untested BC3 and BC5 rebuild need by-hand blocks or VIctory's own block encoder | per-lane budgets and end-state checks arrive with the port; the short unflushed fragments are handled as the Verilog does. The feeder and lockstep are not Microsoft's: both projects hold them as retail observations | nothing beyond (ii): the rebuilds are equal on 1.18 M synthetic blocks; loses the BC1 index reference before block 0 unless the rebuild is changed |
| Speed | none | only the fast decoder is a candidate: the per-lane decoder is roughly 0.55-0.7 times VIctory's lane code. Fast: 1.1-1.3 times VIctory on sampled 12-lane streams, 1.4-1.55 on BC7, 1.6-1.8 on uniform 12-lane streams, level or slower on uniform BC6H | entropy plus rebuild: level on sampled 12-lane streams, 1.2 on BC7, 1.2-1.4 on uniform; and JSON parsing at first use |
| Maintainability | VIctory's 180-line lane code stays, plus a seven-line budget; one vendored test module to keep in step by hand | 1,100 lines replace 180; the fast path is intricate but has an oracle (#2) and agreement tests; one `unsafe` write in #2; a local change for tolerant end states | two table formats or a converter; `serde_json`, `itoa`, `zmij` enter the product; BC6H stays on the old path, so both designs remain |
| Tables needed | VIctory's own (or formula-built ones) on both sides; none from `data/` | models only: verified (B). Ranks and dictionaries stay VIctory's | models (verified) plus rank tables and dictionaries in `bcpack-rs`'s form: class C and D, **not verified**; they would be converted from VIctory's file, which is circular |
| Risk | low | a strict decoder may reject requests the tolerant one served (trailing bytes, end states): must start tolerant with strictness logged | the highest, for no evidence of benefit and with the origin question of #8, #9 open |
| Before it lands | a symbol budget in `lanes`: without it a differential test on encoder streams fails on 0-2 of 200 streams at 4,096 blocks and on most short-fragment streams. BC1 rebuild comparisons with lane 1 = 0 (the reference has no table for the index half) and VIctory's codebook on both sides. A decision on VIctory's filler-fragment test streams, which a Verilog-exact decoder does not accept as they are | all of (i) green; the short unflushed fragments decided against ground truth; a run on the owner's captures with the reference selected shows the same pictures; the head-to-head under the pinned toolchain | all of (ii); the history check for #8 and #9; every rebuild path equal on synthetic streams including dictionary references |

### Recommendation

1. **Option (i) now**, in the vendored, modified form described above, with
   the `EntropyDecoder` seam introduced at the same time so the reference
   decoders plug into tests at symbol level. No file from `data/`, no
   `bcn.rs`, no `bc7.rs`, no cargo dependency on the crate.
2. **Give `lanes` a symbol budget.** The differential shows the
   disagreement (2,372 of 8,400 encoder streams rejected; 47 of 6,000
   padded streams with a wrong last block and no error), the published lane
   goes idle once its output target is reached (`mdu_minirans.v:1193-1195,
   1211-1214`), and a seven-line change in a scratch copy removes all of
   it. That is VIctory's own fix to write and test; the experiment only
   shows its size. Whether retail streams ever trigger it is not known
   here (0-2 of 200 at 4,096 blocks with long fragments).
3. **Generate VIctory's model section** (the first 2,304 bytes) from the
   Microsoft tables with their notice, so that part of the policy
   exception has a recorded, reproducible origin (A-B6). The values are
   equal today, so decoding does not change.
4. **Do not take option (iii).** Its tables are unverified or VIctory's
   own, it has no BC6H, its BC3/4/5/7 modules share code with VIctory's
   file in an undetermined direction, and it shows no gain on sampled
   streams.
5. **Option (ii) is not supported by the present evidence.** Its two
   conditions were: VIctory's lane code wrong on streams the port accepts with
   no small fix, or the fast decoder at least about 1.5 times faster on
   both stream shapes while the performance log's `bcpack_workers` figure
   shows BCPack decode holding back texture streaming. The first fails on
   the size of the fix; the second is not met on sampled 12-lane streams
   (1.1-1.3) or BC6H. What would change this: retail streams shown to
   contain the short unflushed fragments VIctory mishandles and no small
   fix for them; or a head-to-head under 1.98.1 on an idle machine, on
   stream shapes shown to match retail data, at 1.5 times or more together
   with the performance-log evidence.
6. **Ask the history question for #8 and #9** (the `git log -S` of section
   2) whatever option is chosen: it also decides whether VIctory's own file
   owes `bcpack-rs` a notice.

### Open questions

* Is `7b2b421` on `Ahmet-Dedeler/bcpack-rs`? The local clone's origin is a
  different account.
* Which of `bcpack-rs`'s `bc7.rs`/`bcn.rs` and VIctory's `bcpack.rs` was
  written first? Not determinable from the two local histories.
* Which table is right at BC1 symbols 229/235 and 247/248? Needs ground
  truth neither repository has.
* Where did VIctory's rank tables, control codes and dictionaries come
  from (A-B6)? `bcpack-rs` now carries copies; it cannot answer this.
* Do retail streams contain unflushed fragments of 1 to 3 bytes, and was
  VIctory's rule at V:375 taken from an observation? Not recorded in
  either project. The cause of the three mismatches was not isolated here.
* Which synthetic stream shape resembles retail streams (fragment lengths,
  models per lane)? It decides which speed ratio applies.
* The licence of `paryzius/ttpf-viewer` (matters only if #12 or #13 is
  ever wanted).
* How the retail pools `bcpack-rs` measured on were obtained: not stated
  in that repository; the audit's gate question covers it.

## Corrections after verification

The first version of this document was checked independently; each finding
was re-checked against the sources and the scripts and measurements
concerned were run again. Changed: the class and decision of `bc7.rs` and
`bcn.rs` (shared code with VIctory's file; were A and test-only); the
independence of the pair ranks and 205 control codes (stated, not shown);
"dev-dependency" replaced by a vendored, modified subset; the two
entropy-layer differences now carry executed counts instead of "by
reading"; an indicative head-to-head added; rustc 1.97.0, not 1.98.1;
VIctory's lock has 101 packages, not 100; the lane-width and codec rows
split with their own origin statements; `rans.rs` split into the Verilog
port and the feeder; the interface text on strictness and the closure;
HEAD, commit trailers, the store reader's stated origin, header line
numbers. Not adopted: the finding's suggestion that the references and
dictionary code of `format.rs` shares code with VIctory; a token comparison
and a reading of both show different code for the same rule (row #5).
Added beyond the findings: the two scratch experiments of section 5, the
1.98.1 head-to-head runs, per-component reasons where rows had been
isolated or set aside as a group.
