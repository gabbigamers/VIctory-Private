// Part of bcpack-rs by Ahmet Dedeler (https://github.com/Ahmet-Dedeler, https://ahmetdedeler.com)
// Source: https://github.com/Ahmet-Dedeler/bcpack-rs
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// This file is not upstream's `src/lib.rs`: it only declares the vendored
// modules. Upstream's `decode_chunk` and `DecodedChunk`, which use the default
// tables and the modules `bcn` and `bc7`, are left out. See ../README.md for
// what was taken, what was changed and what was deliberately not taken.
//! Part of bcpack-rs, vendored as a second implementation of BCPack for the
//! differential tests in `victory-formats`: the entropy stage of all six
//! formats and the block stage of BC1.
//!
//! * [`tables`]: the rANS model tables of Microsoft's `mdu_minirans.v`
//!   (MIT, see `THIRD_PARTY_NOTICES.md`), unmodified;
//! * [`rans`]: the cycle-level multi-lane decoder, unmodified;
//! * [`format`]: lane layouts, references, zigzag helpers, without the codec
//!   numbers of the store's chunk table;
//! * [`encode`]: the synthetic fragment encoder and interleaver;
//! * [`bc1`]: the BC1 block rebuild, without any table values. Tables are
//!   built by the caller.
//!
//! Upstream's `bcn` (BC3, BC4, BC5) and `bc7` block rebuilds are deliberately
//! not here: their origin is undetermined (`../README.md`). No data file of
//! bcpack-rs is here either, embedded or loaded.

// Upstream's code is kept as it is; its lints are upstream's, and so is its
// formatting: `rustfmt::skip` on a module keeps `cargo fmt` out of its file.
#![allow(clippy::all)]

#[rustfmt::skip]
pub mod bc1;
#[rustfmt::skip]
pub mod encode;
#[rustfmt::skip]
pub mod format;
#[rustfmt::skip]
pub mod rans;
#[rustfmt::skip]
pub mod tables;
