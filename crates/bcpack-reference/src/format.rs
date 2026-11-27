// bcpack-rs by Ahmet Dedeler (https://github.com/Ahmet-Dedeler, https://ahmetdedeler.com)
// Source: https://github.com/Ahmet-Dedeler/bcpack-rs
// SPDX-License-Identifier: MIT OR Apache-2.0
// If you copy, vendor or port this file, keep this header and credit bcpack-rs in your README.
// Modified for VIctory (see ../README.md): the codec numbers of the store's
// chunk table (`Mode::from_codec`, `Mode::codec`) are removed. Nothing else is
// changed.
//! What the BCPack modes share: lane layouts, the two back references of a
//! block record and the dictionary that stands in for the blocks before the
//! chunk.
//!
//! Every lane emits one symbol per 8 unpacked bytes, so a 16-byte block (BC3,
//! BC5, BC6H, BC7) has two symbols per lane, called E (first) and O (second)
//! in the docs, and an 8-byte block (BC1, BC4) has one.
//!
//! A block record starts with two references:
//!
//! * the *index reference* `x`: lane 1 (8-byte blocks) or lane 1's E and O
//!   nibbles interleaved bit by bit, E on the even bits (16-byte blocks);
//! * the *endpoint reference* `(x + unzigzag(d)) mod 256`, with `d` formed
//!   the same way from lane 0.
//!
//! A reference `r` is a distance back in stream order: 0 means "predict the
//! zero state", `r <= i` means block `i - r` of this chunk, and `r > i`
//! reaches before the chunk into a fixed per-mode dictionary of 255 entries,
//! entry `r - i - 1` (entry 0 plays block -1, entry 254 block -255).

/// A BCPack mode (one per BCn family; BC2 has none).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mode {
    Bc1,
    Bc3,
    Bc4,
    Bc5,
    Bc6h,
    Bc7,
}

/// Verilog `alphabet_bits` (symbol width - 1) per lane.
pub const BC1_ALPHABET_BITS: [u8; 12] = [7, 7, 4, 5, 4, 4, 5, 4, 7, 7, 7, 7];
pub const BC3_ALPHABET_BITS: [u8; 12] = [3, 3, 7, 5, 5, 5, 5, 4, 5, 4, 7, 7];
pub const BC4_ALPHABET_BITS: [u8; 12] = [7, 7, 7, 7, 5, 5, 5, 5, 5, 5, 5, 5];
pub const BC5_ALPHABET_BITS: [u8; 12] = [3, 3, 7, 7, 5, 5, 5, 5, 5, 5, 5, 5];
pub const BC6H_ALPHABET_BITS: [u8; 16] = [3, 3, 7, 7, 7, 4, 4, 4, 5, 5, 5, 7, 7, 7, 7, 4];
pub const BC7_ALPHABET_BITS: [u8; 16] = [3, 3, 7, 7, 7, 7, 7, 6, 6, 4, 5, 7, 7, 7, 7, 4];

/// Entries of a mode's dictionary.
pub const DICTIONARY: usize = 255;

impl Mode {
    pub const ALL: [Mode; 6] = [
        Mode::Bc1,
        Mode::Bc3,
        Mode::Bc4,
        Mode::Bc5,
        Mode::Bc6h,
        Mode::Bc7,
    ];

    /// From a name such as `bc5` or `BC6H`.
    pub fn from_name(name: &str) -> Option<Mode> {
        Some(match name.to_ascii_lowercase().as_str() {
            "bc1" => Mode::Bc1,
            "bc3" => Mode::Bc3,
            "bc4" => Mode::Bc4,
            "bc5" => Mode::Bc5,
            "bc6h" | "bc6" => Mode::Bc6h,
            "bc7" => Mode::Bc7,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Bc1 => "bc1",
            Mode::Bc3 => "bc3",
            Mode::Bc4 => "bc4",
            Mode::Bc5 => "bc5",
            Mode::Bc6h => "bc6h",
            Mode::Bc7 => "bc7",
        }
    }

    /// Bytes of one unpacked block.
    pub fn block_bytes(self) -> usize {
        match self {
            Mode::Bc1 | Mode::Bc4 => 8,
            _ => 16,
        }
    }

    /// Decode the entropy layer of a chunk, given the number of BC blocks.
    pub fn decode_symbols(
        self,
        chunk: &[u8],
        blocks: usize,
    ) -> Result<crate::rans::LaneOutput, crate::rans::DecodeError> {
        let target = blocks
            .checked_mul(self.symbols_per_block())
            .ok_or_else(|| crate::rans::DecodeError::Invalid("symbol count overflow".into()))?;
        crate::rans::decode_slice(chunk, self.alphabet_bits(), target)
    }

    /// Symbols each lane emits per block (one per 8 unpacked bytes).
    pub fn symbols_per_block(self) -> usize {
        self.block_bytes() / 8
    }

    /// `alphabet_bits` of the mode's lanes (12 or 16 of them).
    pub fn alphabet_bits(self) -> &'static [u8] {
        match self {
            Mode::Bc1 => &BC1_ALPHABET_BITS,
            Mode::Bc3 => &BC3_ALPHABET_BITS,
            Mode::Bc4 => &BC4_ALPHABET_BITS,
            Mode::Bc5 => &BC5_ALPHABET_BITS,
            Mode::Bc6h => &BC6H_ALPHABET_BITS,
            Mode::Bc7 => &BC7_ALPHABET_BITS,
        }
    }
}

/// Two 4-bit symbols as one byte: `even` on bits 0, 2, 4, 6 and `odd` on bits
/// 1, 3, 5, 7.
#[inline]
pub fn interleave(even: u16, odd: u16) -> u8 {
    let mut out = 0u8;
    for k in 0..4 {
        out |= ((even >> k & 1) as u8) << (2 * k);
        out |= ((odd >> k & 1) as u8) << (2 * k + 1);
    }
    out
}

/// A byte split back into its even-bit and odd-bit nibbles (inverse of
/// [`interleave`]).
#[inline]
pub fn deinterleave(b: u8) -> (u16, u16) {
    let (mut even, mut odd) = (0u16, 0u16);
    for k in 0..4 {
        even |= ((b >> (2 * k) & 1) as u16) << k;
        odd |= ((b >> (2 * k + 1) & 1) as u16) << k;
    }
    (even, odd)
}

/// Zigzag code -> signed value: 0, 1, 2, 3, 4 -> 0, -1, +1, -2, +2.
#[inline]
pub fn unzigzag(v: u16) -> i32 {
    let v = v as i32;
    if v & 1 != 0 {
        -((v + 1) >> 1)
    } else {
        v >> 1
    }
}

/// Signed value -> zigzag code (inverse of [`unzigzag`]).
#[inline]
pub fn zigzag(v: i32) -> u16 {
    if v < 0 {
        (-2 * v - 1) as u16
    } else {
        (2 * v) as u16
    }
}

/// Zigzag residual as an unsigned value modulo `2^bits`.
#[inline]
pub fn residual(v: u16, bits: u32) -> u8 {
    (unzigzag(v) & ((1i32 << bits) - 1)) as u8
}

/// The zigzag code of the residual that takes `from` to `to` modulo `2^bits`,
/// choosing the representative nearest zero.
#[inline]
pub fn residual_code(from: u8, to: u8, bits: u32) -> u16 {
    let m = 1i32 << bits;
    let mut d = (to as i32 - from as i32).rem_euclid(m);
    if d >= m / 2 {
        d -= m;
    }
    zigzag(d)
}

/// The record's two references `(endpoint, index)` from the lane-0 value `d`
/// and the lane-1 value `x` (each already one byte).
#[inline]
pub fn references(d: u8, x: u8) -> (usize, usize) {
    let delta = unzigzag(d as u16);
    (((x as i32 + delta) & 0xFF) as usize, x as usize)
}

/// Lane-0 and lane-1 values that encode the references `(endpoint, index)`.
#[inline]
pub fn reference_codes(endpoint: usize, index: usize) -> (u8, u8) {
    (
        residual_code(index as u8, endpoint as u8, 8) as u8,
        index as u8,
    )
}

/// Where a reference of block `i` points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Reference 0: the zero state.
    Zero,
    /// A block of this chunk (its index).
    Block(usize),
    /// A dictionary entry (0..=254).
    Dictionary(usize),
}

/// Resolve reference `r` (0..=255) of block `i`.
#[inline]
pub fn source(i: usize, r: usize) -> Source {
    if r == 0 {
        Source::Zero
    } else if r <= i {
        Source::Block(i - r)
    } else {
        Source::Dictionary(r - i - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_invert() {
        for b in 0..=255u8 {
            let (e, o) = deinterleave(b);
            assert_eq!(interleave(e, o), b);
        }
        for v in -300..300 {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        for bits in [2u32, 3, 5, 6, 8] {
            let m = 1u32 << bits;
            for a in 0..m {
                for b in 0..m {
                    let c = residual_code(a as u8, b as u8, bits);
                    assert!(c < m as u16, "code fits the lane");
                    assert_eq!((a + residual(c, bits) as u32) % m, b);
                }
            }
        }
        for ep in 0..256 {
            for ix in 0..256 {
                let (d, x) = reference_codes(ep, ix);
                assert_eq!(references(d, x), (ep, ix));
            }
        }
        assert_eq!(source(5, 0), Source::Zero);
        assert_eq!(source(5, 5), Source::Block(0));
        assert_eq!(source(5, 6), Source::Dictionary(0));
        assert_eq!(source(0, 255), Source::Dictionary(254));
    }
}
