//! The result file: the hits of one implementation for some classes, as
//! text, so that a run on a machine with a GPU can write what this crate
//! compares later or elsewhere. A header, then sections:
//!
//! ```text
//! victory-rt results 1
//! producer <one line of free text: which implementation, which machine>
//! section <class> seed <hex> frame <n> rays <count> case <fingerprint, hex>
//! <eight hex words of a hit>                      (one line per ray)
//! end <FNV-1a 64 of the section's hit words as little-endian bytes, hex>
//! ```
//!
//! The rays are not in the file: class and seed name them, and the
//! generator is deterministic. That it gave the writer the rays it gives
//! the reader is not taken on trust: each section carries the fingerprint
//! of the case its hits belong to ([`Case::fingerprint`]), and
//! [`Section::check`] compares it with the case regenerated here, so that
//! a generator that differs between two machines shows as that and not as
//! a list of defects.
//!
//! Numbers are digits only (hex ones in either case, no sign, no prefix).
//! A file that is cut short, has a line that does not parse, a checksum
//! that does not match or two sections for the same class, seed and frame
//! is refused whole; and [`ResultFile::to_text`] refuses to write what
//! [`ResultFile::parse`] would not read back as the same file.

use crate::generate::Case;
use crate::record::Hit;

pub const HEADER: &str = "victory-rt results 1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// A class name: not empty, no white space, no control character.
    pub class: String,
    pub seed: u64,
    pub frame: u32,
    /// [`Case::fingerprint`] of the case the hits were traced for.
    pub case: u64,
    pub hits: Vec<Hit>,
}

impl Section {
    /// The section for `hits`, one per ray of `case`, at `frame`.
    pub fn of(case: &Case, frame: u32, hits: Vec<Hit>) -> Section {
        Section {
            class: case.class.name().into(),
            seed: case.seed,
            frame,
            case: case.fingerprint(),
            hits,
        }
    }

    /// Whether the section is of `case` as generated here: its class and
    /// seed, a frame it has, one hit per ray, and the same fingerprint.
    pub fn check(&self, case: &Case) -> Result<(), String> {
        let at = format!(
            "section {} seed {:x} frame {}",
            self.class, self.seed, self.frame
        );
        if self.class != case.class.name() || self.seed != case.seed {
            return Err(format!(
                "{at}: compared with {} seed {:x}",
                case.class.name(),
                case.seed
            ));
        }
        let fingerprint = case.fingerprint();
        if self.case != fingerprint {
            return Err(format!(
                "{at}: written for a case with fingerprint {:016x}; generated here it is {fingerprint:016x}. The generator differs between the two machines, and the hits cannot be compared",
                self.case
            ));
        }
        if self.frame as usize >= case.frame_count() {
            return Err(format!("{at}: the case has {} frames", case.frame_count()));
        }
        if self.hits.len() != case.rays.len() {
            return Err(format!(
                "{at}: {} hits for {} rays",
                self.hits.len(),
                case.rays.len()
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultFile {
    /// One line of free text: no control character, no white space at
    /// either end.
    pub producer: String,
    pub sections: Vec<Section>,
}

fn checksum(hits: &[Hit]) -> u64 {
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    for byte in hits.iter().flat_map(Hit::to_bytes) {
        h = (h ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// A number of `radix` 10 or 16 written in digits alone: the standard
/// parsers also take a sign.
fn digits(text: &str, radix: u32) -> Option<u64> {
    if text.is_empty() || !text.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u64::from_str_radix(text, radix).ok()
}

fn class_is_a_word(class: &str) -> bool {
    !class.is_empty() && !class.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl ResultFile {
    pub fn section(&self, class: &str, seed: u64, frame: u32) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| s.class == class && s.seed == seed && s.frame == frame)
    }

    /// The first section that has the class, seed and frame of an earlier
    /// one.
    fn repeated(&self) -> Option<&Section> {
        self.sections.iter().enumerate().find_map(|(k, s)| {
            self.sections[..k]
                .iter()
                .any(|e| e.class == s.class && e.seed == s.seed && e.frame == s.frame)
                .then_some(s)
        })
    }

    /// The file as text. Refuses a producer or a class name the format
    /// cannot hold and a repeated section, instead of writing a file that
    /// reads back as another or not at all.
    pub fn to_text(&self) -> Result<String, String> {
        use std::fmt::Write;
        let p = &self.producer;
        if p.chars().any(char::is_control) || p.trim() != p {
            return Err(format!(
                "a producer the file cannot hold in one line: {p:?}"
            ));
        }
        if let Some(s) = self.sections.iter().find(|s| !class_is_a_word(&s.class)) {
            return Err(format!("a class name that is not one word: {:?}", s.class));
        }
        if let Some(s) = self.repeated() {
            return Err(format!(
                "two sections for {} seed {:x} frame {}",
                s.class, s.seed, s.frame
            ));
        }
        let mut out = String::new();
        let _ = writeln!(out, "{HEADER}\nproducer {p}");
        for s in &self.sections {
            let _ = writeln!(
                out,
                "section {} seed {:x} frame {} rays {} case {:016x}",
                s.class,
                s.seed,
                s.frame,
                s.hits.len(),
                s.case
            );
            for h in &s.hits {
                let w = h.to_words().map(|w| format!("{w:08x}"));
                let _ = writeln!(out, "{}", w.join(" "));
            }
            let _ = writeln!(out, "end {:016x}", checksum(&s.hits));
        }
        Ok(out)
    }

    pub fn parse(text: &str) -> Result<ResultFile, String> {
        let mut lines = text.lines().enumerate().map(|(n, l)| (n + 1, l.trim_end()));
        let bad = |n: usize, what: &str| format!("line {n}: {what}");
        match lines.next() {
            Some((_, HEADER)) => {}
            _ => return Err(format!("not a result file: no \"{HEADER}\" line")),
        }
        let producer = match lines.next() {
            Some((_, l)) if l == "producer" || l.starts_with("producer ") => {
                l[8..].trim().to_string()
            }
            _ => return Err(bad(2, "no producer line")),
        };
        let mut file = ResultFile {
            producer,
            sections: Vec::new(),
        };
        while let Some((n, line)) = lines.next() {
            let f: Vec<&str> = line.split(' ').collect();
            let &[
                "section",
                class,
                "seed",
                seed,
                "frame",
                frame,
                "rays",
                rays,
                "case",
                case,
            ] = &f[..]
            else {
                return Err(bad(n, "not a section line"));
            };
            if !class_is_a_word(class) {
                return Err(bad(n, "a section without a class"));
            }
            let seed = digits(seed, 16).ok_or_else(|| bad(n, "a seed that is not hex"))?;
            let frame = digits(frame, 10)
                .and_then(|f| u32::try_from(f).ok())
                .ok_or_else(|| bad(n, "a frame that is not a number"))?;
            let rays = digits(rays, 10)
                .and_then(|r| usize::try_from(r).ok())
                .ok_or_else(|| bad(n, "a ray count that is not a number"))?;
            let case =
                digits(case, 16).ok_or_else(|| bad(n, "a case fingerprint that is not hex"))?;
            if file.section(class, seed, frame).is_some() {
                return Err(bad(n, "a second section for this class, seed and frame"));
            }
            let mut hits = Vec::new();
            for _ in 0..rays {
                let (n, line) = lines.next().ok_or("the file ends inside a section")?;
                let words: Vec<&str> = line.split(' ').collect();
                if words.len() != Hit::WORDS || words.iter().any(|w| w.len() != 8) {
                    return Err(bad(n, "not eight words of eight hex digits"));
                }
                let mut w = [0u32; 8];
                for (k, word) in words.iter().enumerate() {
                    // Eight hex digits fit a word.
                    w[k] = digits(word, 16).ok_or_else(|| bad(n, "a word that is not hex"))? as u32;
                }
                hits.push(Hit::from_words(&w));
            }
            let (n, end) = lines
                .next()
                .ok_or("the file ends before a section's end line")?;
            let sum = end
                .strip_prefix("end ")
                .and_then(|s| digits(s, 16))
                .ok_or_else(|| bad(n, "not an end line"))?;
            if sum != checksum(&hits) {
                return Err(bad(n, "the section's checksum does not match its hits"));
            }
            file.sections.push(Section {
                class: class.to_string(),
                seed,
                frame,
                case,
                hits,
            });
        }
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::{Class, DEFAULT_SEED, generate};
    use crate::rng::Rng;

    fn sample() -> ResultFile {
        let mut r = Rng::new(3);
        let mut hits = |n: usize| -> Vec<Hit> {
            (0..n)
                .map(|k| {
                    if k % 5 == 0 {
                        return Hit::MISS;
                    }
                    Hit::from_words(&std::array::from_fn(|_| r.next_u32()))
                })
                .collect()
        };
        ResultFile {
            producer: "software traversal, CPU model".into(),
            sections: vec![
                Section {
                    class: "static-soup".into(),
                    seed: 0xFEED_0001_0002_0003,
                    frame: 0,
                    case: 0x0123_4567_89AB_CDEF,
                    hits: hits(37),
                },
                Section {
                    class: "empty".into(),
                    seed: 0,
                    frame: 3,
                    case: 7,
                    hits: Vec::new(),
                },
                Section {
                    class: "dynamic-refit".into(),
                    seed: 9,
                    frame: 2,
                    case: u64::MAX,
                    hits: hits(4),
                },
            ],
        }
    }

    #[test]
    fn a_file_reads_back_word_for_word() {
        let file = sample();
        let text = file.to_text().unwrap();
        assert!(text.starts_with("victory-rt results 1\nproducer software traversal, CPU model\n"));
        assert!(text.contains(
            "section static-soup seed feed000100020003 frame 0 rays 37 case 0123456789abcdef\n"
        ));
        let back = ResultFile::parse(&text).unwrap();
        // NaN payloads and negative zeros included: the records are words.
        assert_eq!(back, file);
        assert_eq!(back.section("dynamic-refit", 9, 2).unwrap().hits.len(), 4);
        assert!(back.section("dynamic-refit", 9, 1).is_none());
        // Windows line ends are the same file, and so are capital digits.
        assert_eq!(
            ResultFile::parse(&text.replace('\n', "\r\n")).unwrap(),
            file
        );
        let capitals = text.replace("feed000100020003", "FEED000100020003");
        assert_eq!(ResultFile::parse(&capitals).unwrap(), file);
    }

    #[test]
    fn a_damaged_file_is_refused() {
        let text = sample().to_text().unwrap();
        assert!(ResultFile::parse("").is_err());
        assert!(ResultFile::parse(&text.replacen("results 1", "results 2", 1)).is_err());
        // Cut short, inside a section and before its end line.
        let lines: Vec<&str> = text.lines().collect();
        assert!(ResultFile::parse(&lines[..10].join("\n")).is_err());
        assert!(ResultFile::parse(&lines[..40].join("\n")).is_err());
        assert!(ResultFile::parse(&lines[..41].join("\n")).is_ok());
        // One digit of one hit changed.
        let at = text.find("\n00000000").unwrap() + 1;
        let mut changed = text.clone();
        changed.replace_range(at..at + 1, "1");
        assert!(
            ResultFile::parse(&changed)
                .unwrap_err()
                .contains("checksum")
        );
        // A hit line with a word missing, and a count that lies.
        let short = text.replacen(" 00000000\n", "\n", 1);
        assert!(ResultFile::parse(&short).is_err());
        assert!(ResultFile::parse(&text.replacen("rays 37", "rays 36", 1)).is_err());
        // A section line of the form before the fingerprint.
        let old = text.replacen(" case 0123456789abcdef", "", 1);
        assert_eq!(
            ResultFile::parse(&old).unwrap_err(),
            "line 3: not a section line"
        );
    }

    #[test]
    fn numbers_are_digits_and_nothing_else() {
        let text = sample().to_text().unwrap();
        // The standard parsers take a sign; a word of a hit, the seed, the
        // frame, the count, the fingerprint and the checksum do not.
        let refused = |from: &str, to: &str, what: &str| {
            assert!(text.contains(from), "{from}");
            let e = ResultFile::parse(&text.replacen(from, to, 1)).unwrap_err();
            assert!(e.contains(what), "{from} -> {to}: {e}");
        };
        refused("\n00000000 ", "\n+0000000 ", "a word that is not hex");
        refused("\n00000000 ", "\n0x000000 ", "a word that is not hex");
        refused("seed 9 ", "seed +9 ", "a seed that is not hex");
        refused("seed 9 ", "seed 0x9 ", "a seed that is not hex");
        refused("frame 2 ", "frame +2 ", "a frame that is not a number");
        refused(
            "frame 2 ",
            "frame 4294967296 ",
            "a frame that is not a number",
        );
        refused("rays 4 ", "rays +4 ", "a ray count that is not a number");
        refused("case 0000000000000007", "case +000000000000007", "not hex");
        refused("case 0000000000000007", "case 10000000000000007", "not hex");
        let end = text.rfind("end ").unwrap();
        let mut signed = text.clone();
        signed.replace_range(end + 4..end + 5, "+");
        assert!(
            ResultFile::parse(&signed)
                .unwrap_err()
                .contains("not an end line")
        );
    }

    #[test]
    fn what_would_not_read_back_is_not_written() {
        // A class name with a space in it would write a section line the
        // parser refuses.
        let mut file = sample();
        file.sections[0].class = "static soup".into();
        assert!(file.to_text().unwrap_err().contains("not one word"));
        file.sections[0].class = String::new();
        assert!(file.to_text().is_err());
        // A producer of two lines, or with a space at its end, would read
        // back as another.
        for producer in ["two\nlines", "tab\there", " led", "trailed "] {
            let mut file = sample();
            file.producer = producer.into();
            assert!(file.to_text().is_err(), "{producer:?}");
        }
        let mut file = sample();
        file.producer = String::new();
        assert_eq!(ResultFile::parse(&file.to_text().unwrap()).unwrap(), file);
        // Two sections for one class, seed and frame: neither written nor
        // read, whatever their hits. `section` could only return one.
        let mut file = sample();
        let mut again = file.sections[2].clone();
        again.hits.pop();
        file.sections.push(again);
        assert!(file.to_text().unwrap_err().contains("two sections"));
        let text = sample().to_text().unwrap();
        let from = text.find("section dynamic-refit").unwrap();
        // The file has 49 lines; the section line that repeats is the next.
        let twice = format!("{text}{}", &text[from..]);
        assert_eq!(text.lines().count(), 49);
        assert_eq!(
            ResultFile::parse(&twice).unwrap_err(),
            "line 50: a second section for this class, seed and frame"
        );
        // The same class at another frame or seed is another section.
        let other = text[from..].replacen("frame 2", "frame 1", 1);
        assert_eq!(
            ResultFile::parse(&format!("{text}{other}"))
                .unwrap()
                .sections
                .len(),
            4
        );
    }

    #[test]
    fn a_section_is_checked_against_the_case_generated_here() {
        let case = generate(Class::DynamicRefit, DEFAULT_SEED);
        let hits = vec![Hit::MISS; case.rays.len()];
        let section = Section::of(&case, 3, hits.clone());
        assert_eq!(
            (section.class.as_str(), section.seed, section.case),
            ("dynamic-refit", DEFAULT_SEED, case.fingerprint())
        );
        assert_eq!(section.check(&case), Ok(()));
        // Through the file and back.
        let file = ResultFile {
            producer: "test".into(),
            sections: vec![section.clone()],
        };
        let back = ResultFile::parse(&file.to_text().unwrap()).unwrap();
        let read = back.section("dynamic-refit", DEFAULT_SEED, 3).unwrap();
        assert_eq!(read.check(&case), Ok(()));
        // A generator that gave the writer other numbers: one bit of one
        // vertex is enough, and it is named as what it is.
        let mut elsewhere = case.clone();
        let x = &mut elsewhere.scene.bottoms[0].geometries[0].vertices[0][0];
        *x = x.next_up();
        let e = Section::of(&elsewhere, 3, hits.clone())
            .check(&case)
            .unwrap_err();
        assert!(e.contains("The generator differs"), "{e}");
        // Another class, another seed, a frame the case does not have, a
        // hit too few.
        let other = generate(Class::StaticSoup, DEFAULT_SEED);
        assert!(section.check(&other).is_err());
        assert!(section.check(&generate(Class::DynamicRefit, 1)).is_err());
        assert!(Section::of(&case, 4, hits.clone()).check(&case).is_err());
        let mut short = section;
        short.hits.pop();
        assert!(short.check(&case).unwrap_err().contains("hits for"));
    }
}
