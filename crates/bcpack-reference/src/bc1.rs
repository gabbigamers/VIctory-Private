// bcpack-rs by Ahmet Dedeler (https://github.com/Ahmet-Dedeler, https://ahmetdedeler.com)
// Source: https://github.com/Ahmet-Dedeler/bcpack-rs
// SPDX-License-Identifier: MIT OR Apache-2.0
// If you copy, vendor or port this file, keep this header and credit bcpack-rs in your README.
// Modified for VIctory (see ../README.md): the embedded data files, their
// JSON parsers and the default tables are removed. Nothing else is changed.
//! BC1 semantics: lane symbols -> BC1 blocks.
//!
//! Lane layout of a BC1 chunk (12 lanes, each with one symbol per block):
//!
//! | lane | width | meaning |
//! |---|---|---|
//! | 0 | 8 | endpoint delta `unzigzag(v)`, added to the index reference |
//! | 1 | 8 | index reference `x` (blocks back, 0 = none; 0 in the retail data measured so far) |
//! | 2-7 | 5,6,5,5,6,5 | zigzag residuals of R0 G0 B0 R1 G1 B1 |
//! | 8-11 | 8 | 2x2 index quadrants TL, TR, BL, BR (codebook symbols) |
//!
//! The two references are those of [`crate::format::references`]: the index
//! reference is `x` blocks back and the endpoint reference is
//! `d = (x + unzigzag(lane 0)) mod 256` blocks back (0 = the zero state).
//! Endpoint fields of block i = fields of block `i - d` (or zero when d = 0)
//! plus the residual, modulo 2^width. An endpoint reference before block 0
//! reads slot `(i - d) mod 256` of a fixed preset window ([`Window`]). The
//! index residuals come from the four quadrant patterns of a fixed codebook
//! ([`QuadTable`]); each pixel's residual is added, along the endpoint line
//! and modulo 4, to the same pixel of block `i - x`. An index reference
//! before block 0 has no known table here: it reads zero and the block is
//! counted in [`Bc1Output::unfitted_blocks`]. Blocks come out in stream order, which is the stored Gfx10
//! tile order of the texture.

use crate::rans::{self, DecodeError, LaneOutput};

/// Verilog `alphabet_bits` (symbol width - 1) of the 12 BC1 lanes.
pub const BC1_ALPHABET_BITS: [u8; 12] = [7, 7, 4, 5, 4, 4, 5, 4, 7, 7, 7, 7];
/// Bit widths of the endpoint fields R0 G0 B0 R1 G1 B1 (lanes 2-7).
pub const ENDPOINT_WIDTHS: [u32; 6] = [5, 6, 5, 5, 6, 5];

/// The 256-slot preset endpoint window read by references before block 0.
#[derive(Clone, Debug)]
pub struct Window {
    pub slots: [[u8; 6]; 256],
    pub fitted: [bool; 256],
}

impl Window {
    pub fn fitted_count(&self) -> usize {
        self.fitted.iter().filter(|&&f| f).count()
    }
}

/// Index codebook: for each quadrant (TL, TR, BL, BR) and symbol, the bits that
/// symbol contributes to the 32-bit BC1 index word (pixel (x, y) at bit 2*(4y+x)).
#[derive(Clone, Debug)]
pub struct QuadTable {
    pub codes: [[u8; 4]; 256],
    pub masks: [[u32; 256]; 4],
}

/// (x, y) of the top-left pixel of quadrants TL, TR, BL, BR.
const QUAD_ORIGIN: [(u32, u32); 4] = [(0, 0), (2, 0), (0, 2), (2, 2)];
/// Pixel offsets of a quadrant's four codes, in table order TL TR BL BR.
const GROUP_PIXEL: [(u32, u32); 4] = [(0, 0), (1, 0), (0, 1), (1, 1)];

impl QuadTable {
    pub fn from_codes(codes: [[u8; 4]; 256]) -> QuadTable {
        let mut masks = [[0u32; 256]; 4];
        for (q, &(ox, oy)) in QUAD_ORIGIN.iter().enumerate() {
            for s in 0..256 {
                let mut w = 0u32;
                for (k, &(dx, dy)) in GROUP_PIXEL.iter().enumerate() {
                    let (x, y) = (ox + dx, oy + dy);
                    w |= (codes[s][k] as u32 & 3) << (2 * (4 * y + x));
                }
                masks[q][s] = w;
            }
        }
        QuadTable { codes, masks }
    }
}

/// Lane 0 symbol -> back-reference distance (`unzigzag(v) mod 256`).
#[inline]
pub fn back_distance(v: u16) -> usize {
    let v = v as usize & 0xFF;
    if v & 1 == 0 {
        v >> 1
    } else {
        255 - (v >> 1)
    }
}

/// Zigzag: 0, 1, 2, 3, 4 -> 0, -1, +1, -2, +2.
#[inline]
pub fn unzigzag(v: u16) -> i32 {
    let v = v as i32;
    if v & 1 != 0 {
        -((v + 1) >> 1)
    } else {
        v >> 1
    }
}

/// Endpoints and index words assembled from decoded lanes.
#[derive(Clone, Debug)]
pub struct Bc1Output {
    /// BC1 blocks, 8 bytes each (c0, c1 little endian, then the index word).
    pub blocks: Vec<u8>,
    /// Blocks whose reference chain reaches an unfitted window slot (their
    /// endpoints are probably wrong until the window table is complete).
    pub unfitted_blocks: usize,
    /// Number of references that pointed before block 0.
    pub prestart_refs: usize,
}

/// Turn the 12 decoded BC1 lanes into BC1 blocks.
pub fn assemble(lanes: &LaneOutput, window: &Window, quads: &QuadTable) -> Bc1Output {
    let n = lanes.target;
    let l0 = lanes.lane(0);
    let l1 = lanes.lane(1);
    let res: [&[u16]; 6] = [
        lanes.lane(2),
        lanes.lane(3),
        lanes.lane(4),
        lanes.lane(5),
        lanes.lane(6),
        lanes.lane(7),
    ];
    let idx: [&[u16]; 4] = [lanes.lane(8), lanes.lane(9), lanes.lane(10), lanes.lane(11)];
    // References reach back at most 255 blocks; keep a 256-slot history ring.
    let mut fields = [[0u8; 6]; 256];
    let mut tainted = [false; 256];
    let mut out = vec![0u8; n * 8];
    let mut unfitted = 0usize;
    let mut prestart = 0usize;
    let mut lines = [0u32; 256];
    for i in 0..n {
        let (d, x) = crate::format::references(l0[i] as u8, l1[i] as u8);
        let (pred, t) = if d == 0 {
            ([0u8; 6], false)
        } else if d <= i {
            (fields[(i - d) & 255], tainted[(i - d) & 255])
        } else {
            prestart += 1;
            let slot = (i as isize - d as isize).rem_euclid(256) as usize;
            (window.slots[slot], !window.fitted[slot])
        };
        let mut e = [0u8; 6];
        for k in 0..6 {
            let m = (1i32 << ENDPOINT_WIDTHS[k]) - 1;
            e[k] = ((pred[k] as i32 + unzigzag(res[k][i])) & m) as u8;
        }
        fields[i & 255] = e;
        tainted[i & 255] = t;
        unfitted += t as usize;
        let c0 = (e[0] as u16) << 11 | (e[1] as u16) << 5 | e[2] as u16;
        let c1 = (e[3] as u16) << 11 | (e[4] as u16) << 5 | e[5] as u16;
        let residual = quads.masks[0][idx[0][i] as usize & 0xFF]
            | quads.masks[1][idx[1][i] as usize & 0xFF]
            | quads.masks[2][idx[2][i] as usize & 0xFF]
            | quads.masks[3][idx[3][i] as usize & 0xFF];
        let (ref_line, ref_t) = if x == 0 {
            (0, false)
        } else if x <= i {
            (lines[(i - x) & 255], tainted[(i - x) & 255])
        } else {
            (0, true)
        };
        let line = add_lines(codes_to_lines(residual), ref_line);
        lines[i & 255] = line;
        if ref_t && !t {
            tainted[i & 255] = true;
            unfitted += 1;
        }
        let w = lines_to_codes(line);
        let o = &mut out[i * 8..i * 8 + 8];
        o[0..2].copy_from_slice(&c0.to_le_bytes());
        o[2..4].copy_from_slice(&c1.to_le_bytes());
        o[4..8].copy_from_slice(&w.to_le_bytes());
    }
    Bc1Output {
        blocks: out,
        unfitted_blocks: unfitted,
        prestart_refs: prestart,
    }
}

/// Position of each BC1 code along the endpoint line (c0, 1/3, 2/3, c1):
/// codes 0, 2, 3, 1 are line positions 0, 1, 2, 3.
const CODE_TO_LINE: [u32; 4] = [0, 3, 1, 2];
const LINE_TO_CODE: [u32; 4] = [0, 2, 3, 1];

/// A BC1 index word (16 2-bit codes) -> the same pixels as line positions.
#[inline]
fn codes_to_lines(w: u32) -> u32 {
    (0..16).fold(0, |acc, p| acc | CODE_TO_LINE[(w >> (2 * p) & 3) as usize] << (2 * p))
}

/// Line positions -> a BC1 index word.
#[inline]
fn lines_to_codes(l: u32) -> u32 {
    (0..16).fold(0, |acc, p| acc | LINE_TO_CODE[(l >> (2 * p) & 3) as usize] << (2 * p))
}

/// Per-pixel sum of two line-position words, modulo 4.
#[inline]
fn add_lines(a: u32, b: u32) -> u32 {
    (0..16).fold(0, |acc, p| acc | ((a >> (2 * p)).wrapping_add(b >> (2 * p)) & 3) << (2 * p))
}

/// Decode the 12 lane symbol streams of a BCPack BC1 chunk.
pub fn decode_symbols(chunk: &[u8], blocks: usize) -> Result<LaneOutput, DecodeError> {
    rans::decode_slice(chunk, &BC1_ALPHABET_BITS, blocks)
}

/// BCPack BC1 chunk bytes + block count -> BC1 blocks (stream / Gfx10 order).
pub fn decode(
    chunk: &[u8],
    blocks: usize,
    window: &Window,
    quads: &QuadTable,
) -> Result<Bc1Output, DecodeError> {
    let lanes = decode_symbols(chunk, blocks)?;
    Ok(assemble(&lanes, window, quads))
}
