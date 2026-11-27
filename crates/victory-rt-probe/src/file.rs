//! The result files of a run. All are text, hold measurements and
//! synthetic data only, and are safe to share.
//!
//! * **The table** (`rt-probe.csv`): lines that begin with `#` hold the
//!   header, the first naming the format and each of the rest one
//!   `key=value`; then a line of column names and a row per scene class,
//!   frame and path ([`crate::row`]). An empty field is "does not apply"
//!   or "not measured", never 0.
//! * **The hit records**, one file per tracing path
//!   (`rt-probe.native.hits`, `rt-probe.hybrid.hits`): `victory-rt`'s own
//!   result file, written by that crate's writer, a section per class and
//!   frame with one record per ray. The format is that crate's as it is;
//!   what the probe adds is in the records: a kind with bit 31 is a
//!   refusal (the shader's, with its reason bits, or
//!   `crate::records::NOT_DISPATCHED` for a ray refused before the
//!   dispatch) or a record the dispatch left unwritten (the poison, eight
//!   times), which `victory-rt`'s own comparison would call malformed
//!   and [`crate::adapter::read_hits`] with `crate::records::Answer::of`
//!   reads as what it is. One file a path, because that format holds one
//!   producer and a class and frame once. A row of a tracing path has a
//!   section exactly when its refusal counts are filled, which is when
//!   its answers came back: `cargo xtask rt-summary` goes by that to know
//!   which sections two runs' files must both hold.
//! * **The mismatches** (`rt-probe-mismatch.csv`): the first defects of
//!   each class, frame and path, each with its ray and its record as hex
//!   words, enough to replay the ray in a test.
//!
//! A run is long and its files are written when it is done, so whether
//! they can be written is tried before it starts ([`Outputs::prepare`]),
//! and a file that cannot be written at the end does not lose the others
//! ([`write`]).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::adapter::{self, Case};
use crate::device;
use crate::row::{COLUMNS, Row};
use crate::scene::{HitWords, RayWords};

/// The first line of a table.
pub const MAGIC: &str = "# victory rt-probe results 1";

/// The table: its header and its rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    /// `key`, `value` in the order written. A key is a word of lower-case
    /// letters, digits, `_`, `.` and `-`; a value is one line.
    pub header: Vec<(String, String)>,
    pub rows: Vec<Row>,
}

fn one_line(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

fn is_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
}

impl Table {
    /// Adds a header line. A key that is no key is a mistake in this
    /// crate, and panics.
    pub fn set(&mut self, key: &str, value: &str) {
        assert!(is_key(key), "not a header key: {key:?}");
        self.header.push((key.to_string(), one_line(value)));
    }

    /// The first value of `key`.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.header.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("{MAGIC}\n");
        for (key, value) in &self.header {
            let _ = writeln!(out, "# {key}={value}");
        }
        let names: Vec<&str> = COLUMNS.iter().map(|(name, _)| *name).collect();
        let _ = writeln!(out, "{}", names.join(","));
        for row in &self.rows {
            let _ = writeln!(out, "{}", row.values().join(","));
        }
        out
    }

    /// Reads a table this version wrote: the same columns in the same
    /// order. Anything else is refused with the line it was found at.
    pub fn parse(text: &str) -> Result<Table, String> {
        let mut lines = text.lines().enumerate().map(|(n, line)| (n + 1, line));
        if lines.next().map(|(_, line)| line.trim_end()) != Some(MAGIC) {
            return Err(format!("not a result table: no \"{MAGIC}\" line"));
        }
        let mut table = Table::default();
        let mut columns = false;
        for (n, line) in lines {
            if let Some(rest) = line.strip_prefix('#') {
                if columns {
                    return Err(format!("line {n}: a header line among the rows"));
                }
                let (key, value) =
                    rest.trim_start().split_once('=').ok_or_else(|| format!("line {n}: no key=value"))?;
                if !is_key(key) {
                    return Err(format!("line {n}: not a header key: {key:?}"));
                }
                table.header.push((key.to_string(), value.to_string()));
            } else if !columns {
                let names: Vec<&str> = line.split(',').collect();
                if names.len() != COLUMNS.len() || names.iter().zip(COLUMNS).any(|(a, (b, _))| a != b) {
                    return Err(format!("line {n}: not this version's columns"));
                }
                columns = true;
            } else {
                let values: Vec<String> = line.split(',').map(String::from).collect();
                let fields = values.len();
                let row = Row::from_values(values)
                    .ok_or_else(|| format!("line {n}: {fields} fields for {} columns", COLUMNS.len()))?;
                table.rows.push(row);
            }
        }
        if !columns {
            return Err("the table ends before its line of column names".into());
        }
        Ok(table)
    }
}

/// One defect, with what replays it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub class: String,
    pub seed: u64,
    pub frame: u32,
    pub path: &'static str,
    pub ray: u32,
    /// `victory-rt`'s name for what was wrong.
    pub outcome: &'static str,
    pub ray_words: RayWords,
    pub record: HitWords,
}

const MISMATCH_COLUMNS: &str = "class,seed,frame,path,ray,outcome,ray_words,record";

fn hex(words: &[u32]) -> String {
    words.iter().map(|w| format!("{w:08x}")).collect::<Vec<_>>().join(" ")
}

/// The mismatch file: its column line, then a line a defect. With no
/// defect listed it is the column line alone.
pub fn mismatches_text(list: &[Mismatch]) -> String {
    let mut out = format!("{MISMATCH_COLUMNS}\n");
    for m in list {
        let _ = writeln!(
            out,
            "{},{:x},{},{},{},{},{},{}",
            m.class,
            m.seed,
            m.frame,
            m.path,
            m.ray,
            m.outcome,
            hex(&m.ray_words),
            hex(&m.record)
        );
    }
    out
}

/// Everything a run found.
pub struct Results {
    pub table: Table,
    pub cases: Vec<Case>,
    /// The records of each tracing dispatch that listed every ray once:
    /// the path, the case's place in `cases`, the frame, and one record
    /// per ray of the case (refusals among them).
    pub records: Vec<(device::Path, usize, u32, Vec<HitWords>)>,
    pub mismatches: Vec<Mismatch>,
}

impl Results {
    /// The hit records of `path` as a `victory-rt` result file; `None`
    /// where the path has no records.
    pub fn hits_text(&self, path: device::Path) -> Result<Option<String>, String> {
        let sections: Vec<(&Case, u32, &[HitWords])> = self
            .records
            .iter()
            .filter(|(of, ..)| *of == path)
            .map(|(_, case, frame, records)| (&self.cases[*case], *frame, records.as_slice()))
            .collect();
        if sections.is_empty() {
            return Ok(None);
        }
        let producer = format!(
            "victory rt-probe {}, path {}, {}",
            crate::PROBE_VERSION,
            path.name(),
            self.table.value("adapter").unwrap_or("an unnamed device")
        );
        adapter::hits_text(&one_line(&producer), &sections).map(Some)
    }
}

/// Where a run's files go, from the table's path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outputs {
    pub table: PathBuf,
    pub hits: [(device::Path, PathBuf); 2],
    pub mismatches: PathBuf,
}

impl Outputs {
    /// Beside `table` and named after it: `rt-probe.csv` gives
    /// `rt-probe.native.hits`, `rt-probe.hybrid.hits` and
    /// `rt-probe-mismatch.csv`.
    pub fn beside(table: &Path) -> Outputs {
        let stem = table.file_stem().map_or_else(|| "rt-probe".into(), |s| s.to_string_lossy().into_owned());
        let named = |name: String| table.with_file_name(name);
        Outputs {
            table: table.to_path_buf(),
            hits: device::Path::ALL.map(|path| (path, named(format!("{stem}.{}.hits", path.name())))),
            mismatches: named(format!("{stem}-mismatch.csv")),
        }
    }

    pub fn all(&self) -> Vec<&Path> {
        let mut all = vec![self.table.as_path()];
        all.extend(self.hits.iter().map(|(_, path)| path.as_path()));
        all.push(&self.mismatches);
        all
    }

    /// Makes the folder the files go to and tries each of the four names,
    /// before a run: an error says which cannot be written and why. Nothing
    /// an earlier run left is changed (a file that exists is opened for
    /// writing and closed again), and nothing new is left behind (a name
    /// that is free is created and removed again), so that a run that then
    /// does not start has written no file.
    pub fn prepare(&self) -> Result<(), String> {
        if let Some(folder) = self.table.parent().filter(|folder| !folder.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder).map_err(|e| format!("creating the folder {}: {e}", folder.display()))?;
        }
        for path in self.all() {
            let at = path.display();
            match std::fs::metadata(path) {
                Ok(found) if found.is_dir() => {
                    return Err(format!("{at} is a folder: a result file cannot have its name"));
                }
                Ok(_) => {
                    let opened = std::fs::OpenOptions::new().write(true).open(path);
                    opened.map_err(|e| format!("{at} is there and cannot be written over: {e}"))?;
                }
                Err(_) => {
                    let made = std::fs::OpenOptions::new().write(true).create_new(true).open(path);
                    made.map_err(|e| format!("{at} cannot be created: {e}"))?;
                    std::fs::remove_file(path)
                        .map_err(|e| format!("{at} was created to try the name and cannot be removed: {e}"))?;
                }
            }
        }
        Ok(())
    }
}

/// The header key of a table that is on disk without all of its files: the
/// files that could not be written, each with why.
pub const NOT_WRITTEN: &str = "files_not_written";

/// What [`write`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    /// The files written, with their sizes, in the order written.
    pub files: Vec<(PathBuf, u64)>,
    /// A line for each file that could not be written, and for an earlier
    /// run's file that could not be removed.
    pub failed: Vec<String>,
    /// The table's text, where the table itself could not be written: the
    /// caller's to put where it can still be read.
    pub table_lost: Option<String>,
}

/// Writes a run's files. The hit records go first and the table last, so
/// that a table on disk has its records beside it, or says which of them
/// is missing: a file that cannot be written does not stop the others, and
/// the table then holds a [`NOT_WRITTEN`] line that names it. A path with
/// no records gets no file, and one left by an earlier run under the same
/// name is removed, so that it cannot be read as this run's.
pub fn write(results: &Results, outputs: &Outputs) -> Written {
    let mut written = Written::default();
    let put = |written: &mut Written, path: &Path, text: &str| match std::fs::write(path, text) {
        Ok(()) => written.files.push((path.to_path_buf(), text.len() as u64)),
        Err(e) => written.failed.push(format!("{}: {e}", path.display())),
    };
    for (path, file) in &outputs.hits {
        match results.hits_text(*path) {
            Ok(Some(text)) => put(&mut written, file, &text),
            Ok(None) => match std::fs::remove_file(file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => written.failed.push(format!(
                    "{}: an earlier run's, which this run has no records for and could not remove: {e}",
                    file.display()
                )),
            },
            Err(why) => written.failed.push(format!("{}: the records are not a file: {why}", file.display())),
        }
    }
    put(&mut written, &outputs.mismatches, &mismatches_text(&results.mismatches));
    let text = if written.failed.is_empty() {
        results.table.to_text()
    } else {
        let mut table = results.table.clone();
        table.set(NOT_WRITTEN, &written.failed.join("; "));
        table.to_text()
    };
    let before = written.failed.len();
    put(&mut written, &outputs.table, &text);
    if written.failed.len() > before {
        written.table_lost = Some(text);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row::{NATIVE, RAN};

    fn table() -> Table {
        let mut table = Table::default();
        table.set("probe_version", "1");
        table.set("adapter", "A card, with a comma\nand a second line");
        table.set("pipeline.triangles", "made in 12.5 ms = fine");
        let mut row = Row::new("static-soup", 0x5EED, "0", NATIVE);
        row.text("status", RAN).count("judged", 2000).number("trace_gpu_ms_p50", Some(0.25));
        row.note("one, two");
        table.rows.push(row);
        table.rows.push(Row::new("static-soup", 0x5EED, "", crate::row::NULL));
        table
    }

    #[test]
    fn a_table_reads_back_as_it_was_written() {
        let table = table();
        let text = table.to_text();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(MAGIC));
        assert_eq!(lines.next(), Some("# probe_version=1"));
        assert_eq!(lines.next(), Some("# adapter=A card, with a comma and a second line"));
        assert!(text.contains("\nclass,seed,frame,path,status,note,"));
        let back = Table::parse(&text).unwrap();
        assert_eq!(back, table);
        assert_eq!(back.value("pipeline.triangles"), Some("made in 12.5 ms = fine"));
        assert_eq!(back.value("no-such"), None);
        assert_eq!(back.rows[0].get("note"), "one; two");
        assert_eq!(back.rows[1].get("frame"), "");
    }

    #[test]
    fn what_is_not_this_versions_table_is_refused_with_its_line() {
        let text = table().to_text();
        assert!(Table::parse("").unwrap_err().contains("not a result table"));
        assert!(Table::parse(&text.replacen("results 1", "results 2", 1)).is_err());
        let no_columns: String = text.lines().take(3).map(|l| format!("{l}\n")).collect();
        assert!(Table::parse(&no_columns).unwrap_err().contains("column names"));
        assert!(Table::parse(&text.replacen("class,seed", "seed,class", 1)).unwrap_err().contains("line 5"));
        assert!(Table::parse(&format!("{text}a,b,c\n")).unwrap_err().contains("3 fields"));
        assert!(Table::parse(&format!("{text}# late=1\n")).unwrap_err().contains("among the rows"));
        assert!(Table::parse(&text.replacen("# probe_version=1", "# probe version", 1)).is_err());
    }

    #[test]
    fn a_mismatch_holds_its_ray_and_its_record_as_hex_words() {
        let one = Mismatch {
            class: "precision".into(),
            seed: 0x5EED,
            frame: 2,
            path: NATIVE,
            ray: 17,
            outcome: "missed",
            ray_words: std::array::from_fn(|k| k as u32),
            record: [0; 8],
        };
        let text = mismatches_text(&[one]);
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(MISMATCH_COLUMNS));
        let line = lines.next().unwrap();
        assert!(line.starts_with("precision,5eed,2,native,17,missed,00000000 00000001 "), "{line}");
        assert!(line.ends_with(",00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000"));
        assert_eq!(line.split(',').count(), 8);
        assert_eq!(mismatches_text(&[]), format!("{MISMATCH_COLUMNS}\n"));
    }

    #[test]
    fn the_files_of_a_run_are_named_after_its_table() {
        let outputs = Outputs::beside(Path::new("out/rt-probe-2.csv"));
        let names: Vec<String> = outputs.all().iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(
            names,
            [
                "out/rt-probe-2.csv",
                "out/rt-probe-2.native.hits",
                "out/rt-probe-2.hybrid.hits",
                "out/rt-probe-2-mismatch.csv"
            ]
        );
    }
}
