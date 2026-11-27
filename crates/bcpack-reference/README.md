# bcpack-reference

Part of **bcpack-rs by Ahmet Dedeler**
(https://github.com/Ahmet-Dedeler/bcpack-rs), a decoder for Xbox Series
BCPack texture streams, vendored into VIctory as a second implementation
for differential tests of `victory-formats`' BCPack decoder
(`crates/victory-formats/src/bcpack/differential.rs`): the entropy stage of
all six formats and the block stage of BC1. Which parts are here follows
`docs/bcpack-rs-evaluation.md`; see "What was left out, and why".

* Upstream: https://github.com/Ahmet-Dedeler/bcpack-rs
* Commit: `7b2b42122c6b05787506f212e9a2570999d3f143` (upstream `master` on
  2026-11-26; crate version 0.2.0)
* Licence: MIT OR Apache-2.0, as upstream. `LICENSE-MIT`, `LICENSE-APACHE`
  and `THIRD_PARTY_NOTICES.md` beside this file are upstream's, unchanged.
  The rANS tables and the lane decoder carry Microsoft's MIT notice
  (`THIRD_PARTY_NOTICES.md`).

This crate is a dev-dependency of `victory-formats` and of nothing else. It
is not linked into any VIctory build
(`cargo tree -e normal -i bcpack-reference` shows no dependent). VIctory
itself is GPL-3.0-or-later; this directory keeps upstream's licence.

## What was taken

Line numbers are upstream's at the commit above.

| File | State |
|---|---|
| `src/rans.rs` | unchanged |
| `src/tables.rs` | unchanged |
| `src/format.rs` | changed, see below |
| `src/encode.rs` | changed, see below |
| `src/bc1.rs` | changed, see below |
| `src/lib.rs` | not upstream's: written here, it declares the modules |
| `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD_PARTY_NOTICES.md` | unchanged |

Not taken: `src/bcn.rs`, `src/bc7.rs` and `data/` (see the next section),
`src/lib.rs` (`decode_chunk` and `DecodedChunk` use the default tables and
those two modules), `src/main.rs`, `src/ttpf.rs`, `src/texture.rs`,
`python/`, `tools/`, `tests/`, `docs/`.

## What was left out, and why

### `src/bcn.rs` and `src/bc7.rs`

Upstream's block rebuild for BC3, BC4 and BC5 (`src/bcn.rs`) and for BC7
(`src/bc7.rs`) was deliberately not taken. `docs/bcpack-rs-evaluation.md`
(section 2, components 8 and 9, and "Isolated components and what would
change each") classes both as of undetermined origin: they share
non-trivial code with VIctory's own decoder
(`crates/victory-formats/src/bcpack.rs`, GPL-3.0-or-later), their headers
state them as the bcpack-rs author's under MIT OR Apache-2.0, and which was
written from which cannot be told from the two local histories. The owner's
rule is to isolate components of uncertain provenance and to test with the
verified ones. They would not be independent evidence either: that
document's section 5 records that their rules were matched to VIctory's
decoder, so agreement with them is consistency, not confirmation.

So this crate has no block stage for BC3, BC4, BC5 or BC7 (upstream has none
for BC6H), and no test of `victory-formats` compares those block stages with
anything from bcpack-rs. For those formats the reference checks the entropy
stage only.

What is here is what the same document classes as derived from Microsoft's
MIT-licensed Verilog (`tables.rs`, reproduced from it; the lane decoder of
`rans.rs`, a port of it) or as the bcpack-rs author's own code (the rest of
`rans.rs`, `format.rs`, `encode.rs`, `bc1.rs`): its components 1 to 5, 7 and
10. Two limits it records apply here. The lane widths in `format.rs`
(components 6a and 6b) have no verified origin; they equal VIctory's own, so
nothing new enters with them. `bc1.rs` is a second implementation whose rule
for lane 1 was written to match VIctory's reported output (component 7), so
agreement on BC1 blocks says the two projects read the format alike, not
that either is right.

`format.rs` keeps its helpers for the two-symbol lanes, the residuals and
the dictionary (`interleave`, `deinterleave`, `residual`, `residual_code`,
`reference_codes`, `Source`, `source`, `DICTIONARY`). In upstream's `src/`
no file but `bcn.rs`, `bc7.rs` and `format.rs` itself uses them, and nothing
here uses them outside that file. They are the file's own statement of the
format's rules, classed as the bcpack-rs author's code with no code shared
with VIctory's file (component 5), so the file was not cut down further.

### The codec numbers in `src/format.rs`

Upstream's `Mode::from_codec` and `Mode::codec` hold six numbers of the
store's chunk table. That document classes them as facts taken from
third-party format notes, for which bcpack-rs states no licence, and leaves
them out of the vendored `format.rs` (section 2, component 6c; section 7,
"What reuse means in practice"). They are removed here; nothing in this
repository used them.

### `data/`

Upstream's `data/*.json` hold the BC1 index codebook and window, the BC3,
BC4 and BC5 rank tables and dictionaries and the BC7 control codes and
dictionary. Upstream describes them as fitted to retail data and in part
corrected against a reference decoder it does not name. Their origin is not
established, so none of them is here: not as a file, not embedded, not as
constants. Upstream's `bc1.rs` embeds its two with `include_str!` and parses
them with `serde_json`; that is what the changes to `bc1.rs` below remove.
The tests that use this crate make up their own tables and give the same
ones to both implementations.

This crate has no dependency (upstream depends on `serde_json` for those
parsers).

## Every modification

Only removals, and one note under each changed file's header. No remaining
line is edited.

`src/encode.rs`
* a three-line note added after the header (line 4);
* line 20 removed: `use crate::{bc7, format::Mode};` (`bc7` is not in this
  crate);
* lines 216-289 removed: `synthetic_bc1` and `synthetic`, which draw BC7
  control codes from the default BC7 table through `bc7::Tables`.

`src/format.rs`
* a three-line note added after the header;
* lines 57-80 removed: `Mode::from_codec` and `Mode::codec`, the codec
  numbers of the store's chunk table.

`src/bc1.rs`
* a two-line note added after the header;
* line 30 removed: `use serde_json::Value;`;
* lines 37-41 removed: `DEFAULT_WINDOW_JSON`, `DEFAULT_QUAD_JSON`
  (`include_str!` of `data/bc1_window.json` and `data/bc1_quad_table.json`);
* lines 50-88 removed: `Window::from_json`, `Window::default_table`;
* lines 121-157 removed: `QuadTable::from_json`,
  `QuadTable::default_table`.

Module and item documentation in the vendored files still mentions the data
files, the removed functions and the modules that were not taken; it is
upstream's text and was left as it is.

`Cargo.toml` and `src/lib.rs` are written for this repository. The package
is named `bcpack-reference` (library `bcpack_reference`), edition 2021 as
upstream, without upstream's features, binary or dependencies. `src/lib.rs`
allows clippy's lints for the whole crate, so that upstream's code stays as
it is, and marks every module `#[rustfmt::skip]`, so that `cargo fmt` leaves
upstream's files alone (`cargo fmt -p bcpack-reference -- --check` reports
nothing).

## Updating

Copy the files again from a newer upstream commit, redo the removals above,
and record the commit here, in `crates/victory-formats/src/bcpack/models.rs`
(if `FREQ_TABLE` is copied again) and in the repository's
`THIRD_PARTY_NOTICES.md`. `src/bcn.rs` and `src/bc7.rs` stay out until the
question of their origin is answered (`docs/bcpack-rs-evaluation.md`, "Open
questions").
