//! The decoder against a second implementation: part of bcpack-rs, vendored
//! as `bcpack-reference` (its README says what was taken and what was
//! deliberately not). Everything here is synthetic: streams come from the
//! reference's fragment encoder or from the writer below, and the block stage
//! gets tables these tests make up, given to both sides. Nothing reads
//! `bcpack-tables.bin`.
//!
//! The entropy stage is compared symbol for symbol and byte for byte, on the
//! lanes of all six formats. The reference's lane is a port of Microsoft's
//! `mdu_minirans.v`. Where the two differed, that file's one-lane module was
//! simulated and asked: on one lane it sides with the reference in both
//! cases (the symbol count on one reading of its start signal; the kept byte
//! by decoded symbols), and says nothing about several lanes sharing a
//! stream (docs/bcpack-lane-corrections.md). The two cases are changed here
//! and kept as tests at the end of the entropy section.
//!
//! The block stage is compared for BC1 only. The reference's rebuild of BC3,
//! BC4, BC5 and BC7 is of undetermined origin and is not vendored
//! (docs/bcpack-rs-evaluation.md, section 2), and it has none for BC6H:
//! nothing here checks those block stages. Its BC1 rebuild is fitted, not
//! derived: the tests of it state what agrees and list what does not, without
//! taking a side.

use bcpack_reference::encode::{self, Fragment, Rng};
use bcpack_reference::format::Mode;
use bcpack_reference::rans::{self, ByteSource, DecodeError, LaneOutput};
use bcpack_reference::{bc1, tables};

use super::*;

const FORMATS: [(u8, Mode); 6] = [
    (1, Mode::Bc1),
    (3, Mode::Bc3),
    (4, Mode::Bc4),
    (5, Mode::Bc5),
    (6, Mode::Bc6h),
    (7, Mode::Bc7),
];

/// What the entropy stage made of a stream: the result, the records and the
/// number of bytes it took.
fn entropy(
    widths: &[u8],
    per: usize,
    blocks: usize,
    src: &[u8],
) -> (Result<(), Error>, Vec<u8>, usize) {
    let models = Models::builtin().unwrap();
    let mut records = vec![0u8; blocks * widths.len() * per];
    let mut taken = 0;
    let fetch = |_| {
        let b = src.get(taken).copied();
        taken += usize::from(b.is_some());
        b
    };
    let result = lanes(&models, widths, per, blocks, fetch, &mut records);
    (result, records, taken)
}

/// Records as the reference orders symbols: lane by lane.
fn lane_major(records: &[u8], lanes: usize, per: usize) -> Vec<u16> {
    let target = records.len() / lanes;
    let mut symbols = vec![0u16; records.len()];
    for (b, record) in records.chunks_exact(lanes * per).enumerate() {
        for (j, lane) in record.chunks_exact(per).enumerate() {
            for (k, &s) in lane.iter().enumerate() {
                symbols[j * target + b * per + k] = s.into();
            }
        }
    }
    symbols
}

/// Both of the reference's decoders and ours agree on `stream`: the same
/// symbols, and every byte of it taken.
fn assert_decodes_alike(widths: &[u8], per: usize, blocks: usize, stream: &[u8], what: &str) {
    let taken = assert_same_symbols(widths, per, blocks, stream, what);
    assert_eq!(taken, stream.len(), "{what}: bytes taken");
}

/// The same symbols from both. Returns the bytes ours took; the reference
/// takes all of `stream`.
fn assert_same_symbols(
    widths: &[u8],
    per: usize,
    blocks: usize,
    stream: &[u8],
    what: &str,
) -> usize {
    let target = blocks * per;
    let reference = rans::decode_slice(stream, widths, target);
    let reference = reference.unwrap_or_else(|e| panic!("{what}: the reference fails: {e}"));
    assert_eq!(
        reference.consumed,
        stream.len(),
        "{what}: the reference leaves bytes"
    );
    // Its cycle-level decoder, which the streams were interleaved with, is
    // slow: on the small streams only.
    if blocks <= 64 {
        let mut source = rans::SliceSource {
            data: stream,
            pos: 0,
        };
        let exact = rans::decode_lanes(&mut source, widths, target, stream.len()).unwrap();
        assert!(exact == reference, "{what}: the reference's two decoders");
    }
    let (result, records, taken) = entropy(widths, per, blocks, stream);
    assert_eq!(result, Ok(()), "{what}");
    assert!(
        lane_major(&records, widths.len(), per) == reference.symbols,
        "{what}: symbols differ"
    );
    taken
}

/// As [`assert_decodes_alike`] for streams whose fragments keep the zero
/// bytes after their last symbol. The reference reads every lane to the end
/// of its last fragment; this decoder stops at the last record, so of those
/// bytes it leaves the ones no lane had taken by then, at most two a lane.
fn assert_decodes_alike_but_for_last_bytes(
    widths: &[u8],
    per: usize,
    blocks: usize,
    stream: &[u8],
    what: &str,
) -> usize {
    let taken = assert_same_symbols(widths, per, blocks, stream, what);
    let left = &stream[taken..];
    assert!(
        left.len() <= 2 * widths.len() && left.iter().all(|&b| b == 0),
        "{what}: left {left:?}"
    );
    left.len()
}

#[test]
fn lane_layouts_are_the_references() {
    for (bc, mode) in FORMATS {
        let (widths, per) = layout(bc).unwrap();
        assert_eq!(widths, mode.alphabet_bits(), "BC{bc}");
        assert_eq!(per, mode.symbols_per_block(), "BC{bc}");
    }
    assert!(layout(2).is_none());
}

/// The frequencies are the vendored copy's, and the bucket starts computed
/// from them are Microsoft's base table.
#[test]
fn the_models_are_the_reference_tables() {
    assert!(models::FREQ_TABLE == tables::FREQ_TABLE);
    let models = Models::builtin().unwrap();
    for m in 0..MODELS {
        let (eab, q) = (m / 16, m % 16);
        for b in 0..eab + 2 {
            assert_eq!(models.freq[m][b], u32::from(tables::FREQ_TABLE[eab][q][b]));
            assert_eq!(
                models.start[m][b],
                u32::from(tables::BASE_TABLE[eab][q][b]),
                "model {m} bucket {b}"
            );
        }
    }
}

/// One decode: the symbol of the Verilog's reciprocal path and the state of
/// its forward path, for every model and every slot (slot 0, which no symbol
/// owns, included).
#[test]
fn a_payload_step_decodes_what_the_reference_does() {
    let models = Models::builtin().unwrap();
    for model in 0..MODELS as u8 {
        let reference = rans::model(model / 16, model % 16);
        for high in [0x100u32, 0xa5c3] {
            for slot in 0..1u32 << 16 {
                let x = high << 16 | slot;
                if x <= LOW {
                    continue;
                }
                let mut lane = Lane {
                    phase: 3,
                    model,
                    x,
                    left: 1,
                    owed: 1,
                    ..Default::default()
                };
                lane.step(&models).unwrap();
                let sym = reference.inverse_exact(slot as u16);
                assert_eq!(lane.out.map(u16::from), Some(sym), "model {model} x {x:#x}");
                assert_eq!(lane.x, reference.advance(x, sym), "model {model} x {x:#x}");
            }
        }
    }
}

/// Lanes of fragments as the reference's encoder writes them: each its own
/// rANS stream with the state reset after it.
fn flushed(widths: &[u8], target: usize, longest: usize, rng: &mut Rng) -> Vec<Vec<Fragment>> {
    widths
        .iter()
        .map(|&width| {
            let mut fragments = Vec::new();
            let mut left = target;
            while left > 0 {
                let n = left.min(1 + rng.below(longest as u64) as usize);
                let z = if rng.below(3) == 0 {
                    rng.below(4) as u8
                } else {
                    0
                };
                let q = rng.below(16) as u8;
                let symbols = (0..n)
                    .map(|_| encode::sample_symbol(rng, width - z, q))
                    .collect();
                fragments.push(Fragment { symbols, q, z });
                left -= n;
            }
            fragments
        })
        .collect()
}

/// Every format's lanes, from one block to thousands, with fragments from
/// one symbol to a whole lane, any model and any narrowing.
#[test]
fn streams_of_flushed_fragments_decode_alike() {
    for (bc, _) in FORMATS {
        let (widths, per) = layout(bc).unwrap();
        for blocks in [1, 2, 3, 7, 40, 300] {
            for longest in [1, 3, 16, 1000] {
                for seed in 1..=5 {
                    let mut rng = Rng(seed * 7919 + u64::from(bc));
                    let lanes = flushed(widths, blocks * per, longest, &mut rng);
                    let stream = encode::encode_lanes(widths, &lanes).unwrap();
                    let what =
                        format!("BC{bc}, {blocks} blocks, fragments to {longest}, seed {seed}");
                    assert_decodes_alike(widths, per, blocks, &stream, &what);
                }
            }
        }
        let mut rng = Rng(u64::from(bc));
        let lanes = flushed(widths, 6000 * per, 6000, &mut rng);
        let stream = encode::encode_lanes(widths, &lanes).unwrap();
        assert_decodes_alike(widths, per, 6000, &stream, &format!("BC{bc}, 6000 blocks"));
    }
}

/// Each of a lane's models (16 shapes at each of the four alphabets a header
/// can select) in a fragment of its own, on every lane of every format.
#[test]
fn every_model_and_narrowing_decodes_alike() {
    let mut used = [false; MODELS];
    for (bc, _) in FORMATS {
        let (widths, per) = layout(bc).unwrap();
        let mut rng = Rng(0x5eed + u64::from(bc));
        let lanes: Vec<Vec<Fragment>> = widths
            .iter()
            .map(|&width| {
                let mut fragments = Vec::new();
                for z in 0..4 {
                    for q in 0..16 {
                        used[usize::from(width - z) * 16 + usize::from(q)] = true;
                        let symbols = (0..6)
                            .map(|_| encode::sample_symbol(&mut rng, width - z, q))
                            .collect();
                        fragments.push(Fragment { symbols, q, z });
                    }
                }
                fragments
            })
            .collect();
        let stream = encode::encode_lanes(widths, &lanes).unwrap();
        assert_decodes_alike(widths, per, 4 * 16 * 6 / per, &stream, &format!("BC{bc}"));
    }
    assert!(used.iter().all(|&u| u), "a model no lane can select");
}

/// A fragment's length byte counts from 1 and the two-byte form from 257:
/// payloads of 255 to 258 bytes on a lane, and one of tens of thousands.
#[test]
fn fragments_around_the_long_header_decode_alike() {
    let (widths, per) = layout(4).unwrap();
    let mut rng = Rng(77);
    let mut seen = [false; 4];
    for n in 200..300 {
        let lanes: Vec<Vec<Fragment>> = widths
            .iter()
            .map(|&width| {
                let symbols = (0..n)
                    .map(|_| encode::sample_symbol(&mut rng, width, 0))
                    .collect();
                vec![Fragment {
                    symbols,
                    q: 0,
                    z: 0,
                }]
            })
            .collect();
        for (lane, &width) in lanes.iter().zip(widths) {
            let bytes = encode::encode_fragment(width, &lane[0]).unwrap();
            let long = bytes[0] & 0x20 != 0;
            let payload = bytes.len() - if long { 3 } else { 2 };
            assert_eq!(long, payload > 256);
            if let Some(seen) = payload.checked_sub(255).and_then(|k| seen.get_mut(k)) {
                *seen = true;
            }
        }
        let stream = encode::encode_lanes(widths, &lanes).unwrap();
        assert_decodes_alike(widths, per, n, &stream, &format!("{n} symbols a fragment"));
    }
    assert_eq!(seen, [true; 4], "payloads of 255, 256, 257 and 258 bytes");
    let lanes = flushed(widths, 40_000, 40_000, &mut rng);
    let longest = lanes.iter().flatten().map(|f| f.symbols.len()).max();
    assert!(longest > Some(20_000));
    let stream = encode::encode_lanes(widths, &lanes).unwrap();
    assert_decodes_alike(widths, per, 40_000, &stream, "40000 blocks");
}

// A writer for what the reference's encoder does not write: fragments that
// hand the rANS state on to the next, and fragments that keep the bytes
// after their last symbol.

/// Symbols of a lane under one model.
struct Run {
    symbols: Vec<u16>,
    q: u8,
    z: u8,
}

#[derive(Clone, Copy)]
struct Shape {
    /// Runs of a lane share one rANS stream where the format lets a
    /// fragment end between them.
    carry: bool,
    /// A fragment that hands its state on may end inside the bytes the next
    /// symbol needs.
    split_reads: bool,
    /// Drop the zero bytes after a stream's last symbol, as the reference's
    /// encoder does.
    trim: bool,
    /// The state a stream ends in: 2^24 in a valid one.
    end: u32,
}

const PLAIN: Shape = Shape {
    carry: false,
    split_reads: false,
    trim: true,
    end: rans::L,
};

/// One rANS stream over `runs` as the decoder reads it (four state bytes,
/// then a byte whenever the state is below 2^24), and for each symbol the
/// bytes read when it is decoded and the state after it.
fn rans_stream(width: u8, runs: &[Run], end: u32) -> (Vec<u8>, Vec<(usize, u32)>) {
    let symbols: Vec<_> = runs
        .iter()
        .flat_map(|run| {
            run.symbols
                .iter()
                .map(|&s| (rans::model(width - run.z, run.q), s))
        })
        .collect();
    let mut x = u64::from(end);
    let mut reversed = Vec::new();
    for &(model, s) in symbols.iter().rev() {
        let (f, cf) = model.forward(s);
        assert!(f > 0, "a symbol the model cannot code");
        while x >= u64::from(f) << 16 {
            reversed.push(x as u8);
            x >>= 8;
        }
        x = ((x / u64::from(f)) << 16) + x % u64::from(f) + u64::from(cf);
    }
    let mut bytes = (x as u32).to_be_bytes().to_vec();
    bytes.extend(reversed.iter().rev());
    let (mut x, mut at) = (0u32, 0);
    let trace = symbols
        .iter()
        .map(|&(model, s)| {
            while x < rans::L {
                x = x << 8 | u32::from(bytes[at]);
                at += 1;
            }
            assert!(x > rans::L, "a state of exactly 2^24 decodes nothing");
            assert_eq!(model.inverse(x as u16), s);
            x = model.advance(x, s);
            (at, x)
        })
        .collect();
    (bytes, trace)
}

fn fragment_bytes(run: &Run, reset: bool, payload: &[u8]) -> Vec<u8> {
    assert!((1..=65792).contains(&payload.len()));
    let long = payload.len() > 256;
    let mut bytes = vec![run.z << 6 | u8::from(long) << 5 | u8::from(reset) << 4 | run.q];
    if long {
        bytes.extend(((payload.len() - 257) as u16).to_le_bytes());
    } else {
        bytes.push((payload.len() - 1) as u8);
    }
    bytes.extend(payload);
    bytes
}

/// A lane's bytes: its runs as fragments in the given shape. A fragment may
/// hand its state on only where the state after its last symbol is below
/// 2^24, which is where the encoder, coming from the lane's end, puts out a
/// byte before that symbol. Elsewhere the rANS stream ends and the next run
/// starts another.
fn lane_bytes(width: u8, runs: &[Run], shape: Shape, rng: &mut Rng) -> Vec<u8> {
    let mut hands_on = vec![false; runs.len()];
    let mut x = u64::from(shape.end);
    for (k, run) in runs.iter().enumerate().rev() {
        let model = rans::model(width - run.z, run.q);
        for (i, &s) in run.symbols.iter().enumerate().rev() {
            let (f, cf) = model.forward(s);
            if i + 1 == run.symbols.len() && k + 1 < runs.len() {
                hands_on[k] = shape.carry && x >= u64::from(f) << 16;
                if !hands_on[k] {
                    x = rans::L.into();
                }
            }
            while x >= u64::from(f) << 16 {
                x >>= 8;
            }
            x = ((x / u64::from(f)) << 16) + x % u64::from(f) + u64::from(cf);
        }
    }
    let mut out = Vec::new();
    let mut first = 0;
    while first < runs.len() {
        let last = (first..runs.len()).find(|&k| !hands_on[k]).unwrap();
        let chain = &runs[first..=last];
        let end = if last + 1 == runs.len() {
            shape.end
        } else {
            rans::L
        };
        let (bytes, trace) = rans_stream(width, chain, end);
        let (mut from, mut symbols) = (0, 0);
        for (k, run) in chain.iter().enumerate() {
            symbols += run.symbols.len();
            let (read, state) = trace[symbols - 1];
            let (reset, to) = if k + 1 == chain.len() {
                let after = bytes.len() - read;
                (
                    true,
                    bytes.len() - if shape.trim { after.min(2) } else { 0 },
                )
            } else {
                assert!(state < rans::L);
                let wanted = trace[symbols].0 - read;
                let extra = if shape.split_reads {
                    rng.below(wanted as u64) as usize
                } else {
                    0
                };
                (false, read + extra)
            };
            out.extend(fragment_bytes(run, reset, &bytes[from..to]));
            from = to;
        }
        first = last + 1;
    }
    out
}

/// Lane byte queues that note which lane took each byte.
struct Queues {
    queues: Vec<std::collections::VecDeque<u8>>,
    order: Vec<u8>,
}

impl ByteSource for Queues {
    fn next(&mut self, lane: usize) -> Option<u8> {
        let b = self.queues[lane].pop_front()?;
        self.order.push(b);
        Some(b)
    }
}

/// The lanes' bytes as one stream in the order the reference's exact decoder
/// takes them, and what that decoder made of them.
fn interleave(
    widths: &[u8],
    target: usize,
    lanes: &[Vec<u8>],
) -> (Vec<u8>, Result<LaneOutput, DecodeError>) {
    let total = lanes.iter().map(Vec::len).sum();
    let mut source = Queues {
        queues: lanes.iter().map(|l| l.iter().copied().collect()).collect(),
        order: Vec::new(),
    };
    let decoded = rans::decode_lanes(&mut source, widths, target, total);
    (source.order, decoded)
}

fn runs(width: u8, target: usize, longest: usize, rng: &mut Rng) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut left = target;
    while left > 0 {
        let n = left.min(1 + rng.below(longest as u64) as usize);
        let z = if rng.below(3) == 0 {
            rng.below(4) as u8
        } else {
            0
        };
        let q = rng.below(16) as u8;
        let symbols = (0..n)
            .map(|_| encode::sample_symbol(rng, width - z, q))
            .collect();
        runs.push(Run { symbols, q, z });
        left -= n;
    }
    runs
}

/// A stream of random lanes in `shape`, and how many of its fragments hand
/// their state on.
fn shaped_stream(
    bc: u8,
    blocks: usize,
    longest: usize,
    shape: Shape,
    rng: &mut Rng,
) -> (Vec<u8>, usize) {
    let (widths, per) = layout(bc).unwrap();
    let lanes: Vec<Vec<u8>> = widths
        .iter()
        .map(|&width| lane_bytes(width, &runs(width, blocks * per, longest, rng), shape, rng))
        .collect();
    let mut carried = 0;
    for lane in &lanes {
        let mut at = 0;
        while at < lane.len() {
            carried += usize::from(lane[at] & 0x10 == 0);
            at += if lane[at] & 0x20 != 0 {
                260 + usize::from(u16::from_le_bytes([lane[at + 1], lane[at + 2]]))
            } else {
                3 + usize::from(lane[at + 1])
            };
        }
    }
    let (stream, decoded) = interleave(widths, blocks * per, &lanes);
    assert!(
        decoded.is_ok(),
        "the reference fails on a stream of the writer: {decoded:?}"
    );
    assert_eq!(stream.len(), lanes.iter().map(Vec::len).sum::<usize>());
    (stream, carried)
}

/// The writer's plain shape is the reference encoder's.
#[test]
fn the_writer_writes_what_the_reference_encoder_does() {
    let (widths, _) = layout(7).unwrap();
    for seed in 1..=20 {
        let lanes: Vec<Vec<Run>> = widths
            .iter()
            .map(|&w| runs(w, 60, 9, &mut Rng(seed + u64::from(w))))
            .collect();
        let fragments: Vec<Vec<Fragment>> = lanes
            .iter()
            .map(|lane| {
                lane.iter()
                    .map(|run| Fragment {
                        symbols: run.symbols.clone(),
                        q: run.q,
                        z: run.z,
                    })
                    .collect()
            })
            .collect();
        let ours: Vec<Vec<u8>> = widths
            .iter()
            .zip(&lanes)
            .map(|(&w, lane)| lane_bytes(w, lane, PLAIN, &mut Rng(1)))
            .collect();
        let (stream, decoded) = interleave(widths, 60, &ours);
        assert!(decoded.is_ok());
        assert_eq!(stream, encode::encode_lanes(widths, &fragments).unwrap());
    }
}

/// Fragments that keep the zero bytes after their last symbol: the lane
/// reads them, to a state of exactly 2^24, before its next header.
#[test]
fn fragments_with_their_last_bytes_decode_alike() {
    let shape = Shape {
        trim: false,
        ..PLAIN
    };
    let (mut left, mut streams) = (0, 0);
    for (bc, _) in FORMATS {
        let (widths, per) = layout(bc).unwrap();
        for blocks in [1, 2, 5, 60, 400] {
            for longest in [1, 2, 7, 500] {
                for seed in 1..=3 {
                    let mut rng = Rng(seed * 104_729 + u64::from(bc));
                    let (stream, _) = shaped_stream(bc, blocks, longest, shape, &mut rng);
                    let what =
                        format!("BC{bc}, {blocks} blocks, fragments to {longest}, seed {seed}");
                    left += assert_decodes_alike_but_for_last_bytes(
                        widths, per, blocks, &stream, &what,
                    );
                    streams += 1;
                }
            }
        }
    }
    assert!(
        left > 0 && left < streams,
        "{left} bytes left in {streams} streams"
    );
}

/// Fragments that hand the state on (the reset bit clear): the next header
/// changes the model and the lane goes on in the same rANS stream. Some end
/// with the read that lets their last symbol decode, some one byte into the
/// two the next symbol needs.
#[test]
fn fragments_that_hand_the_state_on_decode_alike() {
    let mut carried = [0; 2];
    for (split_reads, carried) in [false, true].into_iter().zip(&mut carried) {
        for trim in [true, false] {
            let shape = Shape {
                carry: true,
                split_reads,
                trim,
                ..PLAIN
            };
            for (bc, _) in FORMATS {
                let (widths, per) = layout(bc).unwrap();
                for blocks in [2, 5, 60, 400] {
                    for longest in [1, 2, 7, 80] {
                        for seed in 1..=2 {
                            let mut rng = Rng(seed * 15_485_863 + u64::from(bc));
                            let (stream, n) = shaped_stream(bc, blocks, longest, shape, &mut rng);
                            *carried += n;
                            let what = format!(
                                "BC{bc}, {blocks} blocks, fragments to {longest}, seed {seed}"
                            );
                            if trim {
                                assert_decodes_alike(widths, per, blocks, &stream, &what);
                            } else {
                                assert_decodes_alike_but_for_last_bytes(
                                    widths, per, blocks, &stream, &what,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        carried.iter().all(|&n| n > 10_000),
        "fragments that hand the state on: {carried:?}"
    );
}

/// A lane that has delivered its last symbol and read its fragment takes no
/// more input (the Verilog's `output_remaining` and `done`). Here every lane
/// of a one-block BC4 stream has one fragment of one symbol, 0, under model
/// 0. Lanes 0 to 7 are served first, eight bytes a round, and finish before
/// lanes 8 to 11 have had a byte. The decoder used to read the next header
/// on a finished lane, so lanes 0 to 7 took the other lanes' bytes and the
/// stream was refused.
#[test]
fn a_lane_that_has_delivered_its_symbols_takes_no_more_input() {
    let (widths, per) = layout(4).unwrap();
    let zero = || Fragment {
        symbols: vec![0],
        q: 0,
        z: 0,
    };
    let lanes: Vec<Vec<Fragment>> = widths.iter().map(|_| vec![zero()]).collect();
    let stream = encode::encode_lanes(widths, &lanes).unwrap();
    // A fragment is a header (reset, model 0), its length less one, and the
    // four bytes of the state symbol 0 decodes from: 01 01 00 02 on the
    // 8-bit lanes 0 to 3, 40 10 00 11 on the 6-bit lanes 4 to 11.
    #[rustfmt::skip]
    let expected: [u8; 72] = [
        0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 3, 3, 3, 3, 3, 3, 3, 3,
        1, 1, 1, 1, 0x40, 0x40, 0x40, 0x40, 1, 1, 1, 1, 0x10, 0x10, 0x10, 0x10,
        0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 2, 0x11, 0x11, 0x11, 0x11,
        0x10, 0x10, 0x10, 0x10, 3, 3, 3, 3,
        0x40, 0x40, 0x40, 0x40, 0x10, 0x10, 0x10, 0x10, 0, 0, 0, 0, 0x11, 0x11, 0x11, 0x11,
    ];
    assert_eq!(stream, expected);
    assert_decodes_alike(widths, per, 1, &stream, "one symbol a lane");
    let (_, records, _) = entropy(widths, per, 1, &stream);
    assert_eq!(records, [0; 12]);
}

/// A fragment that hands its state on and ends with a read keeps the byte
/// it read, and ends one step later (the Verilog's PROCESSING state stores
/// the state with the input whatever comes next, and passes through
/// DRAINING when that state is not 2^24). Lane 0 of a two-block BC1 stream
/// codes symbols 255 and 0 under model 15 of the 8-bit alphabet, whose
/// symbol 255 has frequency 1: after it the state is two bytes short, the
/// first fragment ends after one of them and the second holds the other.
/// The decoder used to drop the first and to ask for the next header a step
/// early.
#[test]
fn a_fragment_that_ends_on_a_read_keeps_the_byte() {
    let (widths, per) = layout(1).unwrap();
    let run = |symbols: &[u16], q| Run {
        symbols: symbols.to_vec(),
        q,
        z: 0,
    };
    let lane0 = [run(&[255], 15), run(&[0], 15)];
    assert_eq!(rans::model(widths[0], 15).forward(255), (1, 0xffff));
    let (bytes, trace) = rans_stream(widths[0], &lane0, rans::L);
    assert_eq!(bytes, [1, 1, 0xff, 0xff, 1, 1]);
    assert_eq!(trace, [(4, 0x101), (6, rans::L)]);
    let mut lanes = vec![
        [
            fragment_bytes(&lane0[0], false, &bytes[..5]),
            fragment_bytes(&lane0[1], true, &bytes[5..]),
        ]
        .concat(),
    ];
    for &width in &widths[1..] {
        lanes.push(lane_bytes(width, &[run(&[0, 0], 0)], PLAIN, &mut Rng(1)));
    }
    let (stream, decoded) = interleave(widths, 2, &lanes);
    assert_eq!(decoded.unwrap().lane(0), [255, 0]);
    assert_decodes_alike(widths, per, 2, &stream, "a fragment that ends on a read");
}

/// A fragment that hands on a state of exactly 2^24 while its lane owes
/// symbols: at that state a lane neither reads nor decodes (the Verilog
/// waits there for good), and both refuse the stream. Before the correction
/// above this decoder went on from the state before the fragment's last
/// byte and decoded the next fragment.
#[test]
fn a_state_of_exactly_2_24_handed_on_stalls_both() {
    let (widths, per) = layout(4).unwrap();
    let run = |symbols: &[u16]| Run {
        symbols: symbols.to_vec(),
        q: 0,
        z: 0,
    };
    let (first, second) = (run(&[1]), run(&[2]));
    let (bytes, trace) = rans_stream(widths[0], std::slice::from_ref(&first), rans::L);
    assert_eq!((bytes.len(), trace[0]), (5, (4, rans::L >> 8)));
    let (more, _) = rans_stream(widths[0], std::slice::from_ref(&second), rans::L);
    let mut lanes = vec![
        [
            fragment_bytes(&first, false, &bytes),
            fragment_bytes(&second, true, &more),
        ]
        .concat(),
    ];
    for &width in &widths[1..] {
        lanes.push(lane_bytes(width, &[run(&[1, 2])], PLAIN, &mut Rng(1)));
    }
    let (stream, decoded) = interleave(widths, 2, &lanes);
    assert!(
        matches!(decoded, Err(DecodeError::NoProgress { .. })),
        "{decoded:?}"
    );
    let (result, _, _) = entropy(widths, per, 2, &stream);
    assert_eq!(result, Err(Error("the stream ends early or is corrupt")));
}

/// What the reference refuses and this decoder accepts. Neither is changed
/// here: the decoder has no notion of where a stream ends.
#[test]
fn what_the_reference_checks_and_this_decoder_does_not() {
    let (widths, per) = layout(5).unwrap();
    let blocks = 30;
    let target = blocks * per;
    let mut rng = Rng(4242);
    let (stream, _) = shaped_stream(5, blocks, 9, PLAIN, &mut rng);
    let good = rans::decode_slice(&stream, widths, target).unwrap();

    // Bytes after the stream that are not zero padding.
    let mut longer = stream.clone();
    longer.extend([0, 7]);
    let refused = rans::decode_slice(&longer, widths, target);
    assert!(
        matches!(refused, Err(DecodeError::TrailingData { .. })),
        "{refused:?}"
    );
    let (result, records, taken) = entropy(widths, per, blocks, &longer);
    assert_eq!((result, taken), (Ok(()), stream.len()));
    assert!(lane_major(&records, widths.len(), per) == good.symbols);

    // A lane whose last fragment ends in a state other than 2^24, 2^16 or
    // 2^8: the reference reports the end state, or, where the lane's last
    // symbol leaves it above 2^24 with nothing to read, waits for good as
    // the Verilog would. Every symbol still decodes.
    let mut lanes: Vec<Vec<u8>> = Vec::new();
    for (j, &width) in widths.iter().enumerate() {
        let end = if j == 3 { rans::L + 5 } else { rans::L };
        let shape = Shape {
            end,
            trim: false,
            ..PLAIN
        };
        lanes.push(lane_bytes(
            width,
            &runs(width, target, 9, &mut rng),
            shape,
            &mut rng,
        ));
    }
    let (stream, refused) = interleave(widths, target, &lanes);
    let expected = [
        DecodeError::IllegalEndState { lanes: vec![3] },
        DecodeError::NoProgress {
            consumed: stream.len(),
        },
    ];
    assert!(
        refused.as_ref().is_err_and(|e| expected.contains(e)),
        "{refused:?}"
    );
    let (result, _, taken) = entropy(widths, per, blocks, &stream);
    assert_eq!((result, taken), (Ok(()), stream.len()));

    // The zero bytes after a lane's last symbol cut off: the reference waits
    // for them, this decoder has every record and stops.
    let lanes: Vec<Vec<u8>> = widths
        .iter()
        .map(|&width| {
            lane_bytes(
                width,
                &[Run {
                    symbols: vec![1; target],
                    q: 0,
                    z: 0,
                }],
                Shape {
                    trim: false,
                    ..PLAIN
                },
                &mut rng,
            )
        })
        .collect();
    let (stream, decoded) = interleave(widths, target, &lanes);
    let decoded = decoded.unwrap();
    let cut = &stream[..stream.len() - 1];
    let refused = rans::decode_slice(cut, widths, target);
    assert!(
        matches!(refused, Err(DecodeError::NoProgress { .. })),
        "{refused:?}"
    );
    let (result, records, _) = entropy(widths, per, blocks, cut);
    assert_eq!(result, Ok(()));
    assert!(lane_major(&records, widths.len(), per) == decoded.symbols);
}

// The block stage, for BC1: the reference's rebuild of the other formats is
// not vendored. Its BC1 rules were fitted to data, so agreement here says the
// two projects read the format alike, not that either is right; what differs
// is listed in the tests and nothing is changed for it.

/// Random records: every lane's symbol anywhere in its alphabet, and one
/// record in four without references.
fn random_records(bc: u8, blocks: usize, rng: &mut Rng) -> Vec<u8> {
    let (widths, per) = layout(bc).unwrap();
    let mut records = Vec::new();
    for _ in 0..blocks {
        let plain = rng.below(4) == 0;
        for (j, &width) in widths.iter().enumerate() {
            for _ in 0..per {
                let s = rng.below(2 << width) as u8;
                records.push(if plain && j < 2 { 0 } else { s });
            }
        }
    }
    records
}

/// Records as the reference's rebuild takes them.
fn lane_output(bc: u8, records: &[u8]) -> LaneOutput {
    let (widths, per) = layout(bc).unwrap();
    LaneOutput {
        lanes: widths.len(),
        target: records.len() / widths.len(),
        symbols: lane_major(records, widths.len(), per),
        consumed: 0,
        cycles: 0,
        stats: Vec::new(),
    }
}

fn rebuilt(bc: u8, records: &[u8], constants: &Constants) -> Result<Vec<u8>, Error> {
    let (widths, per) = layout(bc).unwrap();
    let stride = widths.len() * per;
    let mut blocks = vec![0; records.len() / stride * 8 * per];
    rebuild(bc, records, stride, &mut blocks, 8 * per, constants)?;
    Ok(blocks)
}

/// Constants with nothing in them: the tests fill in what a format reads.
fn no_constants() -> Constants {
    Constants::parse(&vec![0; CONSTANTS_SIZE]).unwrap()
}

/// How often the references of `records` name nothing, an earlier block and
/// the dictionary.
fn reference_kinds(bc: u8, records: &[u8]) -> [usize; 3] {
    let (widths, per) = layout(bc).unwrap();
    let mut kinds = [0; 3];
    for (i, s) in records.chunks_exact(widths.len() * per).enumerate() {
        let (ep, ix) = references(s, per == 2);
        for r in [ep, ix] {
            kinds[if r == 0 {
                0
            } else if r <= i {
                1
            } else {
                2
            }] += 1;
        }
    }
    kinds
}

/// A made-up 2-bit rank table: a symbol's four residuals, as one byte with
/// the first lowest and as a list.
fn made_up_quads(rng: &mut Rng) -> ([u8; 256], [[u8; 4]; 256]) {
    let quads = [(); 256].map(|()| [(); 4].map(|()| rng.below(4) as u8));
    (
        quads.map(|q| q[0] | q[1] << 2 | q[2] << 4 | q[3] << 6),
        quads,
    )
}

/// A made-up dictionary of a plain format: elements of the widths `w`.
fn made_up_dictionary(w: &Widths, rng: &mut Rng) -> Vec<Plain> {
    (0..DICTIONARY)
        .map(|_| Plain {
            ep: w.ep.map(|bits| rng.below(1 << bits) as u8),
            ix: w.ix.map(|bits| rng.below(1 << bits) as u8),
        })
        .collect()
}

/// Made-up tables for BC1. The reference keeps the index codebook as BC1
/// codes and the endpoint dictionary as a window whose slot 255 is entry 0;
/// it has no index dictionary.
fn made_up_bc1(rng: &mut Rng) -> (Constants, bc1::Window, bc1::QuadTable) {
    let mut ours = no_constants();
    let (rank, quads) = made_up_quads(rng);
    ours.rank1 = rank;
    ours.bc1 = made_up_dictionary(&BC1, rng);
    let mut window = bc1::Window {
        slots: [[0; 6]; 256],
        fitted: [true; 256],
    };
    for (entry, slot) in ours.bc1.iter().zip(window.slots.iter_mut().rev()) {
        slot.copy_from_slice(&entry.ep[..6]);
    }
    let codes = quads.map(|q| q.map(|position| BC1_CODE[usize::from(position)] as u8));
    (ours, window, bc1::QuadTable::from_codes(codes))
}

/// BC1 blocks whose index reference, followed back, reaches before the
/// first block. The reference has no table for those; it counts these blocks
/// and the ones whose endpoints refer to them.
fn bc1_reaches_the_index_dictionary(records: &[u8]) -> Vec<bool> {
    let mut reaches: Vec<bool> = Vec::new();
    for (i, s) in records.as_chunks::<12>().0.iter().enumerate() {
        let (_, ix) = references(s, false);
        reaches.push(ix > i || (ix != 0 && reaches[i - ix]));
    }
    reaches
}

/// BC1 with an index dictionary of zeros, which is what the reference reads
/// before the first block: the same blocks throughout.
#[test]
fn bc1_blocks_rebuild_alike() {
    let mut kinds = [0; 3];
    for seed in 1..=8 {
        let mut rng = Rng(seed * 37);
        let (mut ours, window, quads) = made_up_bc1(&mut rng);
        for entry in &mut ours.bc1 {
            entry.ix = [0; 32];
        }
        for blocks in [1, 2, 40, 700] {
            let records = random_records(1, blocks, &mut rng);
            let reference = bc1::assemble(&lane_output(1, &records), &window, &quads);
            let reaching = bc1_reaches_the_index_dictionary(&records)
                .iter()
                .filter(|&&r| r)
                .count();
            assert!(reference.unfitted_blocks >= reaching);
            assert!(
                rebuilt(1, &records, &ours).unwrap() == reference.blocks,
                "seed {seed}"
            );
            kinds = [0, 1, 2].map(|k| kinds[k] + reference_kinds(1, &records)[k]);
        }
    }
    assert!(kinds.iter().all(|&n| n > 500), "references: {kinds:?}");
}

/// A known difference. This decoder has a BC1 index dictionary; the
/// reference reads zeros before the first block and counts the blocks that
/// depend on them. With a dictionary that is not all zeros the two agree on
/// every other block and on the endpoints of every block, and differ in the
/// index words of blocks the reference counted.
#[test]
fn bc1_index_references_before_the_first_block_differ() {
    let mut rng = Rng(41);
    let (ours, window, quads) = made_up_bc1(&mut rng);
    let records = random_records(1, 600, &mut rng);
    let reference = bc1::assemble(&lane_output(1, &records), &window, &quads);
    let blocks = rebuilt(1, &records, &ours).unwrap();
    let reaches = bc1_reaches_the_index_dictionary(&records);
    let mut differing = 0;
    let (ours, theirs) = (
        blocks.as_chunks::<8>().0,
        reference.blocks.as_chunks::<8>().0,
    );
    for ((ours, theirs), reaches) in ours.iter().zip(theirs).zip(&reaches) {
        assert_eq!(ours[..4], theirs[..4]);
        assert!(*reaches || ours == theirs);
        differing += usize::from(ours != theirs);
    }
    let reaching = reaches.iter().filter(|&&r| r).count();
    assert!(differing > 100 && differing <= reaching && reaching <= reference.unfitted_blocks);
}

/// The whole decoder against the reference's, with Microsoft's models.
/// Records of every format go through the reference's encoder and both
/// entropy stages. For BC1, whose block stage the reference has here, the
/// stream is decoded to blocks by both, with made-up constants.
#[test]
fn streams_decode_alike_and_bc1_streams_to_the_same_blocks() {
    for (bc, _) in FORMATS {
        let (widths, per) = layout(bc).unwrap();
        for seed in 1..=4 {
            let mut rng = Rng(seed * 53 + u64::from(bc));
            let blocks = 1 + rng.below(500) as usize;
            let bc1 = (bc == 1).then(|| {
                let (mut ours, window, quads) = made_up_bc1(&mut rng);
                ours.bc1.iter_mut().for_each(|entry| entry.ix = [0; 32]);
                (ours, window, quads)
            });
            let records = random_records(bc, blocks, &mut rng);
            let symbols = lane_major(&records, widths.len(), per);
            let lanes: Vec<Vec<Fragment>> = symbols
                .chunks_exact(blocks * per)
                .map(|lane| {
                    lane.chunks(1 + rng.below(200) as usize)
                        .map(|symbols| Fragment {
                            symbols: symbols.to_vec(),
                            q: 0,
                            z: 0,
                        })
                        .collect()
                })
                .collect();
            let stream = encode::encode_lanes(widths, &lanes).unwrap();
            let what = format!("BC{bc}, seed {seed}");
            assert_decodes_alike(widths, per, blocks, &stream, &what);
            let Some((constants, window, quads)) = bc1 else {
                continue;
            };
            let reference = bc1::decode(&stream, blocks, &window, &quads).unwrap();
            let tables = Tables {
                models: Models::builtin().unwrap(),
                constants,
            };
            let mut ours = vec![0; blocks * 8 * per];
            decode(bc, &stream, &mut ours, &tables).unwrap();
            assert!(ours == reference.blocks, "{what}");
            assert!(ours == rebuilt(bc, &records, &tables.constants).unwrap());
        }
    }
}
