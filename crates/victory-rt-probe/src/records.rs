//! The hit records a dispatch wrote, read back: which are answers, which
//! are refusals and which are neither, and what becomes of a ray a path
//! did not answer.
//!
//! A refusal is the record the shaders write for one
//! (`victory_shader::emit::rayquery`): a kind with `HIT_REFUSED` (bit 31)
//! and one or both of the two reasons, the other seven words 0. It is not
//! a hit and **not a miss**: the shader could not decide the ray. It is
//! counted by reason and its ray is for the next path (native, then hybrid,
//! then software). A ray refused before the dispatch (`crate::rays`) has
//! no record at all and goes the same way.
//!
//! **Any other record with bit 31 is no refusal.** No shader writes one, so
//! it is what the device left there: the poison of a record no thread
//! wrote (`crate::device::POISON`), or words that are no record. Such a
//! record is counted ([`Refusals::unwritten`], [`Refusals::malformed`]) and
//! **judged with the answers**, where it is a record that is no hit and no
//! miss: a defect. A row whose dispatch left records unwritten therefore
//! cannot pass on the records that were written.

use victory_shader::emit::rayquery::{HIT_BYTES, HIT_REFUSED, REFUSED_GEOMETRY, REFUSED_RANGE};

use crate::device::POISON;
use crate::scene::HitWords;

/// The records in `bytes`, eight words each; bytes past the last whole
/// record are left.
pub fn words(bytes: &[u8]) -> Vec<HitWords> {
    bytes
        .as_chunks::<{ HIT_BYTES as usize }>()
        .0
        .iter()
        .map(|record| {
            std::array::from_fn(|k| {
                u32::from_le_bytes([
                    record[4 * k],
                    record[4 * k + 1],
                    record[4 * k + 2],
                    record[4 * k + 3],
                ])
            })
        })
        .collect()
}

/// The kind of the record a result file holds for a ray that was refused
/// before the dispatch. No shader writes it: the probe does, so that a
/// file has one record per ray, and none of them reads as a miss. The
/// other seven words are 0.
pub const NOT_DISPATCHED: u32 = HIT_REFUSED | 0x4000_0000;

/// The reasons a shader gives with `HIT_REFUSED`.
const REASONS: u32 = REFUSED_GEOMETRY | REFUSED_RANGE;

/// Whether `record` is a refusal as the shaders write one: bit 31, one or
/// both of the two reasons and no other bit, and seven words of 0.
pub fn is_refusal(record: &HitWords) -> bool {
    let reasons = record[3] & !HIT_REFUSED;
    record[3] & HIT_REFUSED != 0
        && reasons != 0
        && reasons & !REASONS == 0
        && record.iter().enumerate().all(|(k, word)| k == 3 || *word == 0)
}

/// Whether `record` is as the poison left it: no thread wrote it.
pub fn is_unwritten(record: &HitWords) -> bool {
    *record == [POISON; 8]
}

/// What one ray got from a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The words the device left for the ray, where they are no refusal: a
    /// hit, a miss, or words that are neither (an unwritten record among
    /// them). All of them are judged.
    Record(HitWords),
    /// The shader refused the ray: the record, kind and all.
    Refused(HitWords),
    /// Refused before the dispatch: no record.
    NotDispatched,
}

impl Answer {
    /// What a record a device wrote is. Never [`Answer::NotDispatched`]:
    /// a device was given every ray it has a record for.
    pub fn of_device(record: HitWords) -> Answer {
        if is_refusal(&record) { Answer::Refused(record) } else { Answer::Record(record) }
    }

    /// What a record a result file holds is: as [`Answer::of_device`], and
    /// the probe's own record for a ray that was refused before the
    /// dispatch. (A device that wrote those very words would read back as
    /// that; in the run itself they are judged as the device's.)
    pub fn of(record: HitWords) -> Answer {
        if record == Answer::NotDispatched.words() { Answer::NotDispatched } else { Answer::of_device(record) }
    }

    /// The record a result file holds for this answer.
    pub fn words(&self) -> HitWords {
        match self {
            Answer::Record(record) | Answer::Refused(record) => *record,
            Answer::NotDispatched => [0, 0, 0, NOT_DISPATCHED, 0, 0, 0, 0],
        }
    }

    /// The record to judge: of every ray the path did not refuse.
    pub fn answered(&self) -> Option<&HitWords> {
        match self {
            Answer::Record(record) => Some(record),
            Answer::Refused(_) | Answer::NotDispatched => None,
        }
    }

    /// Whether the path took the ray: a record to judge that is none of
    /// the records with bit 31 no shader writes. What is left unwritten is
    /// not taken.
    pub fn took(&self) -> bool {
        self.answered().is_some_and(|record| record[3] & HIT_REFUSED == 0)
    }
}

/// A path's refusals over one list of rays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refusals {
    /// Refused before the dispatch.
    pub before: u32,
    /// Records with `REFUSED_GEOMETRY` among their reasons.
    pub geometry: u32,
    /// Records with `REFUSED_RANGE` among their reasons.
    pub range: u32,
    /// Records no thread wrote: the poison, word for word. No refusal: they
    /// are judged, and are defects.
    pub unwritten: u32,
    /// Other records with bit 31 that are no refusal a shader writes: no
    /// reason, a reason bit this probe does not know, or other words that
    /// are not 0. No refusal either: judged, and defects.
    pub malformed: u32,
    /// Refused rays the next path did not refuse in its turn.
    pub taken_by_next: u32,
    /// Refused rays no built path took.
    pub unanswered: u32,
}

impl Refusals {
    /// Every ray this path did not answer.
    pub fn refused(&self, answers: &[Answer]) -> u32 {
        answers.iter().filter(|a| a.answered().is_none()).count() as u32
    }
}

/// One answer per ray from the records of a dispatch: record `k` is of ray
/// `source[k]`, and a ray with no record was refused before the dispatch.
/// An error where the dispatch's records are not one per listed ray.
pub fn answers(rays: usize, source: &[u32], records: &[HitWords]) -> Result<Vec<Answer>, String> {
    if records.len() != source.len() {
        return Err(format!("{} records read back for {} rays listed", records.len(), source.len()));
    }
    let mut answers = vec![Answer::NotDispatched; rays];
    for (&ray, &record) in source.iter().zip(records) {
        let slot = answers
            .get_mut(ray as usize)
            .ok_or_else(|| format!("a record for ray {ray} of {rays}"))?;
        *slot = Answer::of_device(record);
    }
    Ok(answers)
}

/// Counts `answers`' refusals by reason, the records with bit 31 that are
/// none, and what the next path made of the refused rays: `next` is its
/// answers for the same rays, or `None` where no next path is built.
pub fn refusals(answers: &[Answer], next: Option<&[Answer]>) -> Refusals {
    let mut counted = Refusals::default();
    for (ray, answer) in answers.iter().enumerate() {
        match answer {
            Answer::Record(record) => {
                if is_unwritten(record) {
                    counted.unwritten += 1;
                } else if record[3] & HIT_REFUSED != 0 {
                    counted.malformed += 1;
                }
                continue;
            }
            Answer::NotDispatched => counted.before += 1,
            Answer::Refused(record) => {
                counted.geometry += u32::from(record[3] & REFUSED_GEOMETRY != 0);
                counted.range += u32::from(record[3] & REFUSED_RANGE != 0);
            }
        }
        let taken = next.and_then(|next| next.get(ray)).is_some_and(Answer::took);
        if taken {
            counted.taken_by_next += 1;
        } else {
            counted.unanswered += 1;
        }
    }
    counted
}

/// How many records of a later dispatch over the same list are not the
/// first dispatch's, word for word: `later` holds the list as often as it
/// was repeated (`crate::rays::list`), and each repeat is compared with
/// `first`. A device and driver must give the same records for the same
/// rays in the same structure. A record of `later` that no thread wrote
/// counts whatever `first` holds: two dispatches that both left a record
/// unwritten have not agreed on it.
pub fn differing(first: &[HitWords], later: &[HitWords]) -> u32 {
    if first.is_empty() {
        return later.len() as u32;
    }
    later.iter().zip(first.iter().cycle()).filter(|(a, b)| a != b || is_unwritten(a)).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIT: HitWords = [0x4000_0000, 0x3E80_0000, 0x3E80_0000, 1, 2, 77, 0, 9];
    const MISS: HitWords = [0; 8];

    fn refused(reasons: u32) -> HitWords {
        [0, 0, 0, HIT_REFUSED | reasons, 0, 0, 0, 0]
    }

    #[test]
    fn records_are_eight_little_endian_words() {
        let mut bytes: Vec<u8> = HIT.iter().flat_map(|w| w.to_le_bytes()).collect();
        bytes.extend_from_slice(&[0; 32]);
        bytes.extend_from_slice(&[1, 2, 3]);
        assert_eq!(words(&bytes), [HIT, MISS]);
    }

    #[test]
    fn a_refusal_is_neither_a_hit_nor_a_miss() {
        assert_eq!(Answer::of(MISS), Answer::Record(MISS));
        assert_eq!(Answer::of(HIT).answered(), Some(&HIT));
        let record = refused(REFUSED_GEOMETRY);
        assert_eq!(Answer::of(record), Answer::Refused(record));
        assert_eq!(Answer::of(record).answered(), None);
        assert_eq!(Answer::NotDispatched.answered(), None);
        // What a file holds for each reads back as what it was.
        for answer in [Answer::Record(HIT), Answer::Record(MISS), Answer::Refused(record), Answer::NotDispatched] {
            assert_eq!(Answer::of(answer.words()), answer);
            assert!(answer == Answer::Record(MISS) || answer.words() != MISS);
        }
    }

    /// Only what a shader writes for a refusal is one. Everything else with
    /// bit 31 is a record to judge: it takes no ray out of the comparison.
    #[test]
    fn a_record_with_bit_31_that_no_shader_writes_is_no_refusal() {
        for reasons in [REFUSED_GEOMETRY, REFUSED_RANGE, REFUSED_GEOMETRY | REFUSED_RANGE] {
            assert!(is_refusal(&refused(reasons)), "{reasons}");
        }
        let mut dirty = refused(REFUSED_GEOMETRY);
        dirty[7] = 1;
        let no_refusal = [[POISON; 8], refused(0), refused(REFUSED_RANGE | 0x40), refused(4), dirty, HIT, MISS];
        for record in no_refusal {
            assert!(!is_refusal(&record), "{record:08x?}");
            assert_eq!(Answer::of_device(record), Answer::Record(record));
            assert_eq!(Answer::of_device(record).answered(), Some(&record), "it is judged");
        }
        assert!(is_unwritten(&[POISON; 8]) && !is_unwritten(&dirty) && !is_unwritten(&MISS));
        let mut partly = [POISON; 8];
        partly[0] = 0;
        assert!(!is_unwritten(&partly) && !is_refusal(&partly));
        // A device was given every ray it has a record for: the probe's own
        // record for a ray that was not dispatched is none of a device's.
        let own = Answer::NotDispatched.words();
        assert_eq!(Answer::of_device(own), Answer::Record(own));
        assert_eq!(Answer::of(own), Answer::NotDispatched);
    }

    #[test]
    fn records_go_to_their_rays_and_a_ray_without_one_was_not_dispatched() {
        // Five rays; rays 1 and 3 were refused before the dispatch.
        let got = answers(5, &[0, 2, 4], &[HIT, refused(REFUSED_RANGE), MISS]).unwrap();
        assert_eq!(
            got,
            [
                Answer::Record(HIT),
                Answer::NotDispatched,
                Answer::Refused(refused(REFUSED_RANGE)),
                Answer::NotDispatched,
                Answer::Record(MISS),
            ]
        );
        assert!(answers(5, &[0, 2, 4], &[HIT, MISS]).is_err());
        assert!(answers(2, &[0, 2], &[HIT, MISS]).is_err());
        // An unwritten record is its ray's record, to be judged.
        let got = answers(2, &[0, 1], &[[POISON; 8], MISS]).unwrap();
        assert_eq!(got, [Answer::Record([POISON; 8]), Answer::Record(MISS)]);
    }

    #[test]
    fn refusals_are_counted_by_reason_and_by_what_the_next_path_did() {
        let mut dirty = refused(REFUSED_GEOMETRY);
        dirty[0] = 1;
        let own: Vec<Answer> = [
            HIT,
            Answer::NotDispatched.words(),
            refused(REFUSED_GEOMETRY),
            refused(REFUSED_RANGE),
            refused(REFUSED_GEOMETRY | REFUSED_RANGE),
            // Four records with bit 31 that are no refusal: no reason, a
            // reason nobody knows, other words set, and the poison.
            refused(0),
            refused(REFUSED_RANGE | 0x40),
            dirty,
            [POISON; 8],
            MISS,
        ]
        .into_iter()
        .map(Answer::of)
        .collect();
        // The next path answers rays 2 and 3, refuses 4 itself, and was
        // never given ray 1.
        let next: Vec<Answer> = [
            HIT,
            Answer::NotDispatched.words(),
            HIT,
            MISS,
            refused(REFUSED_RANGE),
            HIT,
            HIT,
            HIT,
            HIT,
            MISS,
        ]
        .into_iter()
        .map(Answer::of)
        .collect();
        // A next path that left a refused ray's record unwritten did not
        // take it.
        let mut unwritten = next.clone();
        unwritten[2] = Answer::of([POISON; 8]);
        let counted = refusals(&own, Some(&unwritten));
        assert_eq!((counted.taken_by_next, counted.unanswered), (1, 3));
        assert!(!unwritten[2].took() && next[2].took() && !next[4].took() && !next[1].took());
        let counted = refusals(&own, Some(&next));
        assert_eq!(
            counted,
            Refusals {
                before: 1,
                geometry: 2,
                range: 2,
                unwritten: 1,
                malformed: 3,
                taken_by_next: 2,
                unanswered: 2,
            }
        );
        // Four rays were refused; the four records that are no refusal are
        // answers to judge, and are for no next path.
        assert_eq!(counted.refused(&own), 4);
        assert_eq!(own.iter().filter(|a| a.answered().is_some()).count(), 6);
        // With no next path built, nobody takes them.
        let alone = refusals(&own, None);
        assert_eq!((alone.taken_by_next, alone.unanswered), (0, 4));
        assert_eq!(refusals(&[Answer::Record(HIT), Answer::Record(MISS)], None), Refusals::default());
    }

    #[test]
    fn a_later_dispatch_is_compared_with_the_first_repeat_by_repeat() {
        let first = [HIT, MISS, HIT];
        let same = [HIT, MISS, HIT, HIT, MISS, HIT, HIT];
        assert_eq!(differing(&first, &same), 0);
        let mut other = same;
        other[4] = HIT;
        other[6][0] ^= 1;
        assert_eq!(differing(&first, &other), 2);
        assert_eq!(differing(&[], &same), 7);
        assert_eq!(differing(&first, &[]), 0);
        // A record the later dispatch did not write is counted, also where
        // the first did not write it either.
        let unwritten = [HIT, [POISON; 8], HIT];
        assert_eq!(differing(&first, &unwritten), 1);
        assert_eq!(differing(&unwritten, &unwritten), 1);
    }
}
