//! Generated rays as what the shaders of `victory_shader::emit::rayquery`
//! take, and the list a dispatch reads.
//!
//! A generated ray is a D3D12 ray as `TraceRay` knows it. Two of its words
//! ask for more than an inline query has, and are adapted here, on the
//! CPU, before a record is written (each adaptation counted):
//!
//! * **`SKIP_CLOSEST_HIT_SHADER`** (0x08) is cleared. An inline query runs
//!   no closest-hit shader, so the answer the reference gives does not
//!   depend on the flag; `TraceRayInline` does not take it.
//! * **The instance mask** is cut to its low eight bits, which is all D3D12
//!   reads of it.
//!
//! What the shaders still cannot trace as DXR defines it
//! (`rayquery::ray_refusal`: a NaN, an infinite origin or direction, a
//! direction of zero length, `tmin` negative or above `tmax`, flags outside
//! the inline set or in a combination DXR calls undefined) is **refused
//! before the dispatch**: such a ray is in no list a device reads, is
//! counted with its reason, and is for the next path.

use victory_shader::emit::rayquery::{RAY_BYTES, ray_refusal};

use crate::scene::RayWords;

/// `D3D12_RAY_FLAG_SKIP_CLOSEST_HIT_SHADER`.
pub const SKIP_CLOSEST_HIT_SHADER: u32 = 0x08;

/// A class's rays as the shaders take them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapped {
    /// The records a dispatch lists, in the order of the rays they are of.
    pub records: Vec<RayWords>,
    /// For each record, the ray it is of.
    pub source: Vec<u32>,
    /// The rays no record was written for, each with its reason.
    pub refused: Vec<(u32, &'static str)>,
    /// Rays whose `SKIP_CLOSEST_HIT_SHADER` flag was cleared.
    pub skip_closest_hit_cleared: u32,
    /// Rays whose mask had bits above the low eight, which were cut.
    pub mask_high_bits_cut: u32,
}

/// One ray as the shaders take it: the record, and whether each of the two
/// adaptations changed it.
pub fn adapt(ray: &RayWords) -> (RayWords, bool, bool) {
    let mut record = *ray;
    record[8] &= 0xFF;
    record[9] &= !SKIP_CLOSEST_HIT_SHADER;
    // The two words no shader reads.
    (record[10], record[11]) = (0, 0);
    (record, ray[9] != record[9], ray[8] != record[8])
}

pub fn map(rays: &[RayWords]) -> Mapped {
    let mut mapped = Mapped::default();
    for (index, ray) in rays.iter().enumerate() {
        let (record, flag, mask) = adapt(ray);
        mapped.skip_closest_hit_cleared += u32::from(flag);
        mapped.mask_high_bits_cut += u32::from(mask);
        match ray_refusal(&record) {
            Some(reason) => mapped.refused.push((index as u32, reason)),
            None => {
                mapped.records.push(record);
                mapped.source.push(index as u32);
            }
        }
    }
    mapped
}

impl Mapped {
    /// The refusals by reason, most frequent first, as `reason: count`
    /// joined by `; `. Empty when nothing was refused.
    pub fn refusals_by_reason(&self) -> String {
        let mut counts: Vec<(&str, u32)> = Vec::new();
        for (_, reason) in &self.refused {
            match counts.iter_mut().find(|(r, _)| r == reason) {
                Some((_, n)) => *n += 1,
                None => counts.push((reason, 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let parts: Vec<String> = counts.iter().map(|(r, n)| format!("{r}: {n}")).collect();
        parts.join("; ")
    }
}

/// The bytes of a ray buffer of `count` records: `records` in order, begun
/// again from the first as often as it takes. A dispatch that lists the
/// first `records.len()` of them traces each ray once; one that lists all
/// `count` is the timed dispatch, the same rays as often as they fit.
/// Empty when there is no record to repeat.
pub fn list(records: &[RayWords], count: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(count as usize * RAY_BYTES as usize);
    if records.is_empty() {
        return out;
    }
    for record in records.iter().cycle().take(count as usize) {
        for word in record {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ray(mask: u32, flags: u32) -> RayWords {
        let f = [1.0f32, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0, 100.0].map(f32::to_bits);
        [f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7], mask, flags, 0xDEAD, 0xBEEF]
    }

    #[test]
    fn the_two_adaptations_change_only_their_bits_and_are_counted() {
        let rays = [
            ray(0xFF, 0),
            // Skip the closest-hit shader, with a cull flag that stays.
            ray(0x01, 0x08 | 0x10),
            // Bits above the low eight.
            ray(0xFFFF_FF80, 0x04),
            ray(0x1_00, 0x08),
        ];
        let mapped = map(&rays);
        assert_eq!(mapped.source, [0, 1, 2, 3]);
        assert!(mapped.refused.is_empty());
        assert_eq!((mapped.skip_closest_hit_cleared, mapped.mask_high_bits_cut), (2, 2));
        assert_eq!(mapped.records[0][..10], rays[0][..10]);
        assert_eq!((mapped.records[1][8], mapped.records[1][9]), (0x01, 0x10));
        assert_eq!((mapped.records[2][8], mapped.records[2][9]), (0x80, 0x04));
        assert_eq!((mapped.records[3][8], mapped.records[3][9]), (0x00, 0x00));
        // The words no shader reads are written as zero; the rest is the
        // ray's, bit for bit.
        for (record, ray) in mapped.records.iter().zip(&rays) {
            assert_eq!(record[..8], ray[..8]);
            assert_eq!(record[10..], [0, 0]);
        }
    }

    #[test]
    fn what_the_shaders_cannot_trace_is_in_no_list_and_keeps_its_reason() {
        let mut nan = ray(0xFF, 0);
        nan[1] = f32::NAN.to_bits();
        let mut still = ray(0xFF, 0);
        (still[4], still[5], still[6]) = (0, 0, 0);
        let mut backwards = ray(0xFF, 0);
        backwards[3] = 200.0f32.to_bits();
        let rays = [
            ray(0xFF, 0),
            nan,
            // Both faces culled; a flag D3D12 does not define.
            ray(0xFF, 0x30),
            ray(0xFF, 0x400),
            still,
            ray(0x0F, 0x100),
            backwards,
            // Still refused once its closest-hit flag is cleared.
            ray(0xFF, 0x08 | 0x30),
        ];
        let mapped = map(&rays);
        assert_eq!(mapped.source, [0, 5]);
        assert_eq!(mapped.records.len(), 2);
        let refused: Vec<u32> = mapped.refused.iter().map(|(ray, _)| *ray).collect();
        assert_eq!(refused, [1, 2, 3, 4, 6, 7]);
        assert_eq!(mapped.refused[0].1, "a NaN in the ray");
        assert_eq!(mapped.refused[3].1, "a direction of zero length");
        assert_eq!(mapped.skip_closest_hit_cleared, 1);
        assert_eq!(
            mapped.refusals_by_reason(),
            "both faces culled: 2; a NaN in the ray: 1; a direction of zero length: 1; \
             ray flags outside DXR 1.1's inline set: 1; tmin negative or above tmax: 1"
        );
        assert_eq!(map(&rays[..1]).refusals_by_reason(), "");
    }

    #[test]
    fn a_list_repeats_its_records_in_order() {
        let records = [ray(1, 0), ray(2, 0), ray(3, 0)];
        let bytes = list(&records, 7);
        assert_eq!(bytes.len(), 7 * 48);
        let mask = |k: usize| u32::from_le_bytes(bytes[48 * k + 32..48 * k + 36].try_into().unwrap());
        assert_eq!((0..7).map(mask).collect::<Vec<_>>(), [1, 2, 3, 1, 2, 3, 1]);
        assert_eq!(list(&records, 2).len(), 96);
        assert!(list(&[], 5).is_empty());
    }
}
