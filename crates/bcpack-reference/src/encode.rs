// bcpack-rs by Ahmet Dedeler (https://github.com/Ahmet-Dedeler, https://ahmetdedeler.com)
// Source: https://github.com/Ahmet-Dedeler/bcpack-rs
// SPDX-License-Identifier: MIT OR Apache-2.0
// If you copy, vendor or port this file, keep this header and credit bcpack-rs in your README.
// Modified for VIctory (see ../README.md): `synthetic` and `synthetic_bc1`,
// which draw BC7 control codes from the default tables, are removed with the
// import they needed. Nothing else is changed.
//! Synthetic BCPack encoder, for tests and fixtures only.
//!
//! The hardware encoder is not public. This one produces streams that the
//! decoder accepts, which is enough to round-trip synthetic data:
//!
//! 1. each lane's symbols are split into flushed fragments; each fragment is an
//!    independent rANS stream (encoded in reverse from state `L`, 4-byte
//!    initial state, trailing zero bytes trimmed as the legal end states allow)
//!    behind a 2- or 3-byte fragment header;
//! 2. the per-lane byte streams are interleaved by running the decoder itself
//!    with a per-lane byte source and recording which lane takes each byte.
//!    The feeder only looks at lane states, so the recorded order is the order
//!    a decoder reading one shared stream will request.

use crate::rans::{self, ByteSource, DecodeError, Model, L};
use std::collections::VecDeque;

/// One fragment of a lane: symbols coded with model `q`, alphabet narrowed by `z`.
#[derive(Clone, Debug)]
pub struct Fragment {
    pub symbols: Vec<u16>,
    pub q: u8,
    pub z: u8,
}

/// Model a fragment header selects for a lane with `alphabet_bits`.
pub fn header_model(alphabet_bits: u8, z: u8) -> Option<u8> {
    let e = (alphabet_bits as i32 - z as i32) & 15;
    let eab = if e < 10 { e as u8 } else { 9 };
    if eab > 8 {
        None
    } else {
        Some(eab)
    }
}

fn header(z: u8, flush: bool, q: u8, len: usize) -> Result<Vec<u8>, String> {
    if !(1..=65535 + 257).contains(&len) {
        return Err(format!("fragment length {len} not in 1..={}", 65535 + 257));
    }
    let hdr3 = len > 256;
    let b0 = (z & 3) << 6 | (hdr3 as u8) << 5 | (flush as u8) << 4 | (q & 15);
    if hdr3 {
        let v = len - 257;
        Ok(vec![b0, v as u8, (v >> 8) as u8])
    } else {
        Ok(vec![b0, (len - 1) as u8])
    }
}

/// Encode one flushed fragment (header + payload).
pub fn encode_fragment(alphabet_bits: u8, frag: &Fragment) -> Result<Vec<u8>, String> {
    if alphabet_bits > 8 {
        return Err(format!("alphabet_bits {alphabet_bits} not in 0..=8"));
    }
    if frag.z > 3 {
        return Err(format!("fragment z {} not in 0..=3", frag.z));
    }
    if frag.q >= 16 {
        return Err(format!("fragment q {} not in 0..=15", frag.q));
    }
    let eab = header_model(alphabet_bits, frag.z).ok_or("z selects an undefined alphabet")?;
    let m: &Model = rans::model(eab, frag.q);
    if frag.symbols.is_empty() {
        return Err("empty fragment".into());
    }
    let mut x: u64 = L as u64;
    let mut rev: Vec<u8> = Vec::new();
    let mut emitted_before_last = 0usize;
    for (k, &s) in frag.symbols.iter().enumerate().rev() {
        if s as u32 >> (eab + 1) != 0 {
            return Err(format!("symbol {s} does not fit {} bits", eab + 1));
        }
        let (f, cf) = m.forward(s);
        if f == 0 {
            return Err(format!(
                "symbol {s} has zero frequency in model eab {eab} q {}",
                frag.q
            ));
        }
        let x_max = (f as u64) << 16;
        while x >= x_max {
            rev.push(x as u8);
            x >>= 8;
        }
        if k == frag.symbols.len() - 1 {
            emitted_before_last = rev.len();
        }
        x = ((x / f as u64) << 16) + (x % f as u64) + cf as u64;
    }
    // Bytes emitted while encoding the last symbol from x = L are zeros that the
    // decoder never needs to read: drop up to two (legal ends L>>8, L>>16).
    let mut ntrim = 0;
    while ntrim < 2 && ntrim < rev.len() && rev[ntrim] == 0 && emitted_before_last > ntrim {
        ntrim += 1;
    }
    let mut payload = (x as u32).to_be_bytes().to_vec();
    payload.extend(rev[ntrim..].iter().rev());
    let mut out = header(frag.z, true, frag.q, payload.len())?;
    out.extend(payload);
    Ok(out)
}

struct PerLane {
    queues: Vec<VecDeque<u8>>,
    order: Vec<usize>,
}

impl ByteSource for PerLane {
    fn next(&mut self, lane: usize) -> Option<u8> {
        let b = self.queues[lane].pop_front();
        if b.is_some() {
            self.order.push(lane);
        }
        b
    }
}

/// Interleave per-lane byte streams into the single stream the decoder reads.
pub fn interleave(
    alphabet_bits: &[u8],
    target: usize,
    lane_bytes: &[Vec<u8>],
) -> Result<Vec<u8>, DecodeError> {
    if lane_bytes.len() != alphabet_bits.len() {
        return Err(DecodeError::Invalid(format!(
            "lane_bytes count {} != alphabet_bits count {}",
            lane_bytes.len(),
            alphabet_bits.len()
        )));
    }
    let total: usize = lane_bytes.iter().map(|v| v.len()).sum();
    let mut src = PerLane {
        queues: lane_bytes
            .iter()
            .map(|v| v.iter().copied().collect())
            .collect(),
        order: vec![],
    };
    rans::decode_lanes(&mut src, alphabet_bits, target, total)?;
    if src.queues.iter().any(|q| !q.is_empty()) {
        return Err(DecodeError::Invalid("a lane left bytes unread".into()));
    }
    let mut cursors = vec![0usize; lane_bytes.len()];
    Ok(src
        .order
        .iter()
        .map(|&l| {
            let b = lane_bytes[l][cursors[l]];
            cursors[l] += 1;
            b
        })
        .collect())
}

/// Encode whole lanes (each a list of fragments) into one interleaved stream.
pub fn encode_lanes(alphabet_bits: &[u8], lanes: &[Vec<Fragment>]) -> Result<Vec<u8>, String> {
    if lanes.is_empty() {
        return Err("lanes must not be empty".into());
    }
    if lanes.len() != alphabet_bits.len() {
        return Err(format!(
            "lanes count {} != alphabet_bits count {}",
            lanes.len(),
            alphabet_bits.len()
        ));
    }
    let target = lanes[0].iter().map(|f| f.symbols.len()).sum::<usize>();
    let mut bytes = Vec::new();
    for (i, frags) in lanes.iter().enumerate() {
        if frags.iter().map(|f| f.symbols.len()).sum::<usize>() != target {
            return Err(format!("lane {i} has a different symbol count"));
        }
        let mut v = Vec::new();
        for f in frags {
            v.extend(encode_fragment(alphabet_bits[i], f)?);
        }
        bytes.push(v);
    }
    interleave(alphabet_bits, target, &bytes).map_err(|e| e.to_string())
}

/// Tiny deterministic PRNG (xorshift64*), so tests need no extra crates.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// Draw a symbol roughly from model (eab, q): pick a cf uniformly and invert it,
/// which samples the model's own distribution.
pub fn sample_symbol(rng: &mut Rng, eab: u8, q: u8) -> u16 {
    let m = rans::model(eab, q);
    loop {
        let cf = 1 + rng.below(0xFFFF) as u16;
        let s = m.inverse_exact(cf);
        if s as u32 >> (eab + 1) == 0 && m.forward(s).0 > 0 {
            return s;
        }
    }
}
