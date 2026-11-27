//! The validation record: what was demonstrated and measured about the
//! ray-tracing paths on one device with one driver
//! (docs/dxr-architecture.md, "Selection"). Under `auto` a native or hybrid
//! path is selected only from rows here ([`super::select::select`]); a
//! workload with no rows runs the software path.
//!
//! There are two kinds of row, because the two findings are about
//! different programs:
//!
//! * A **probe row** ([`ProbeRow`]) is the probe's verdict on one path of a
//!   workload class (an opaque hash the translator gives: what a traversal
//!   region means, never a title shader's name or hash) in one shader
//!   form, with one source of the host structure and one set of side tables
//!   ([`Key`]). The probe runs a synthetic program of its own for the class
//!   (docs/dxr-validation.md, "What is compared"), so its verdict is about
//!   the class and about the translator that made that program's variant,
//!   never about a title program's container. The row holds the
//!   translator's hash, which a workload's must equal, and the hash of the
//!   probe's own container, which is recorded and compared with nothing.
//!   One row a key and path: a later verdict takes the earlier one's place
//!   and the earlier one is handed back ([`Record::set_probe`]).
//! * A **title row** ([`TitleRow`]) is what the title comparison found
//!   about one container. Its key ([`TitleKey`]) is the class key, the
//!   path, the hash of the variant's container and the hash of the software
//!   container it was compared with and timed against. A class has a title
//!   row for each of its programs (and for each translation of one that was
//!   compared): a finding about one container never touches another's row.
//!
//! Each verdict says under which rule of equivalence it was given (a
//! number that stands for the rule's text and constants,
//! docs/dxr-compatibility.md, "The rule"), and the probe's says which
//! version of the probe gave it. The selection counts a verdict only if
//! those are the ones its build accepts ([`super::select::Standard`]).
//!
//! The device and driver identity ([`Identity`]) is an equality key and
//! nothing else: measured data holds for the device and driver it was
//! taken on, and nothing here or in the selection reads the vendor, the
//! device id or the driver version for any other purpose (rule 1 of
//! docs/gpu-compatibility.md).
//!
//! # On disk
//!
//! Text, one line a row, in the folder [`FOLDER`] of the cache folder,
//! where `shaders` and `pipelines` are. As each translator's shaders are in
//! a folder named by the start of its key (`crate::shader_cache`), each
//! device and driver's record is a file named by the start of its
//! identity's hash ([`file`]): another driver's file is never read, and
//! can be deleted.
//!
//! The first line names the format and its version, the second the device
//! and driver. A text that names another device counts as absent
//! ([`load`]). One that holds a second first line or a second `device`
//! line (two records put together) is refused whole ([`Error::Several`]):
//! rows taken on another device would otherwise be read as this one's. Two
//! lines for one key say two things about one subject: neither is matched,
//! and both are kept.
//!
//! A tool may write more into a row than the selection reads
//! ([`Record::set_probe_field`], [`Record::set_title_field`]: the probe's
//! levels, the largest coordinate it covered and the value of the
//! tolerance constant, a scene, a date, capture ids), and a later build
//! may write what this one does not know. Reading keeps both: a field of a
//! row it does not know is written back with the row, and a line it cannot
//! read is written back as it was and never matched ([`Record::unread`]).
//! (A field it does not know on the first line or on the `device` line is
//! read past and not written back.) A file of a later [`VERSION`] is
//! refused whole ([`Error::Newer`]): nothing is selected from a record
//! whose meaning this build cannot know.
//!
//! # Memory
//!
//! Bounded: [`MAX_BYTES`] of text and [`MAX_ROWS`] rows, more is refused.
//! A file is read through that bound, whatever the file system says its
//! size is ([`load`]). What is written back can be a few bytes longer than
//! what was read (a last line without its end, a device id written with
//! fewer than four digits), so a text at the bound may be read and then
//! not saved: [`save`] refuses it and leaves the file as it was. A record
//! a run keeps is counted by its holder ([`Held`], owner `RtRecord`,
//! docs/memory.md R6). No run keeps one yet.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path as FsPath, PathBuf};

use super::select::Path;
use crate::caps::HostCaps;

/// The version this build writes, and the latest it reads.
pub const VERSION: u32 = 1;

/// The most text a record may be, on disk and when written.
pub const MAX_BYTES: usize = 1 << 20;

/// The most rows a record may hold, of both kinds together, lines it
/// could not read among them.
pub const MAX_ROWS: usize = 1024;

/// The record's folder in the cache folder, where `shaders` and
/// `pipelines` are: what a run would ask its configuration for as it asks
/// for those (`Config::cache_subdir`).
pub const FOLDER: &str = "rt-record";

/// The most text the fields the selection does not read may take in one
/// row ([`Record::set_probe_field`], [`Record::set_title_field`]). With it
/// a record of [`MAX_ROWS`] of the longest rows this build writes is
/// within [`MAX_BYTES`].
pub const MAX_FIELDS: usize = 384;

/// The word that begins the first line of a record.
const MAGIC: &str = "victory-rt-record";

/// The hash of a container (the DXIL of one variant of one program),
/// compared for equality only. [`Digest::of`] is the one way it is made,
/// so that whatever writes a row and the run that looks for it name a
/// container alike.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest(pub [u8; 32]);

/// A workload class: the hash of what a recognised traversal region means
/// (docs/dxr-architecture.md, "The layer"). Opaque here.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Class(pub [u8; 32]);

/// The translator that made a variant, with the settings it translated
/// under: a hash its side gives of itself, opaque here and compared for
/// equality only (the key the shader cache names a translator's folder by,
/// `crate::shader_cache`, is such a hash). What the probe demonstrated for
/// one translator's variant of a class it did not demonstrate for
/// another's.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Translator(pub [u8; 32]);

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// The 32 bytes that 64 hexadecimal digits spell, in either case.
fn unhex(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (at, byte) in bytes.iter_mut().enumerate() {
        let digit = |d: u8| char::from(d).to_digit(16);
        *byte = (digit(digits[2 * at])? * 16 + digit(digits[2 * at + 1])?) as u8;
    }
    Some(bytes)
}

impl Digest {
    /// The hash of `container`'s bytes (BLAKE3, as the shader cache's keys
    /// are).
    pub fn of(container: &[u8]) -> Digest {
        Digest(*blake3::hash(container).as_bytes())
    }

    pub fn hex(&self) -> String {
        hex(&self.0)
    }
}

impl Class {
    /// All 64 digits: what the record holds.
    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// The first 16 digits: what a log line names a class by, and enough
    /// for `VICTORY_RT_CLASS`.
    pub fn short(&self) -> String {
        hex(&self.0[..8])
    }
}

impl Translator {
    pub fn hex(&self) -> String {
        hex(&self.0)
    }
}

impl std::fmt::Debug for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Digest({})", hex(&self.0[..8]))
    }
}

impl std::fmt::Debug for Class {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Class({})", self.short())
    }
}

impl std::fmt::Debug for Translator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Translator({})", hex(&self.0[..8]))
    }
}

/// The device and driver measured data was taken on. An equality key:
/// its parts are private, shown in a report and compared by [`same`], and
/// nothing may be decided from their values.
///
/// [`same`]: Identity::same
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    vendor: u32,
    device: u32,
    driver: Option<[u16; 4]>,
}

impl Identity {
    /// The identity of the device `caps` describes.
    pub fn of(caps: &HostCaps) -> Identity {
        Identity { vendor: caps.vendor, device: caps.device, driver: caps.driver }
    }

    /// Whether the adapter gave a driver version. Without one a driver
    /// update cannot be told from none, so nothing is recorded for the
    /// device and no record holds for it.
    pub fn known(&self) -> bool {
        self.driver.is_some()
    }

    /// Whether data taken on `other` holds for this device: the same
    /// device, and the same driver version, known on both sides.
    pub fn same(&self, other: &Identity) -> bool {
        self.known() && self == other
    }

    /// The fields of the record's `device` line.
    fn fields(&self) -> String {
        let driver = self.driver.map_or("unknown".to_owned(), |[a, b, c, d]| format!("{a}.{b}.{c}.{d}"));
        format!("vendor={:#06x} device={:#06x} driver={driver}", self.vendor, self.device)
    }

    fn parse(fields: &Fields<'_>) -> Option<Identity> {
        let id = |key| {
            let text: &str = fields.one(key)?;
            u32::from_str_radix(text.strip_prefix("0x")?, 16).ok()
        };
        let driver = match fields.one("driver")? {
            "unknown" => None,
            text => {
                let mut parts = text.split('.').map(|part| part.parse::<u16>().ok());
                let version = [parts.next()??, parts.next()??, parts.next()??, parts.next()??];
                if parts.next().is_some() {
                    return None;
                }
                Some(version)
            }
        };
        Some(Identity { vendor: id("vendor")?, device: id("device")?, driver })
    }
}

impl std::fmt::Display for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.fields())
    }
}

/// How a variant comes by its structure at a dispatch
/// (docs/dxr-architecture.md, "Selection", "Each dispatch"). The two are
/// different DXIL, so each has its own containers and its own rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Form {
    /// The structure is known from root arguments when the title records:
    /// the path's variant and the software one are both recorded, and
    /// submission gives one of them the title's counts.
    Dual,
    /// The structure is known only on the GPU: one variant holds the
    /// path's loop and the software loop and reads a table written at
    /// submission.
    BothLoops,
}

/// What the host structure a variant traces was built from
/// (docs/dxr-architecture.md, "Host structures").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// The build descriptions the title hands its builder.
    Descriptions,
    /// The buffers a rehydrate description names beside a packed
    /// structure.
    Packed,
    /// The guest's own nodes, decoded.
    Nodes,
}

/// The side tables a variant reads to give the code after a region what
/// the host does not return (docs/dxr-architecture.md, "Guest concepts and
/// their native equivalents"): a set, empty for most.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tables(u8);

impl Tables {
    pub const NONE: Tables = Tables(0);
    /// By instance index: the guest's instance record, or its address.
    pub const INSTANCE: Tables = Tables(1);
    /// By primitive: the guest node pointer.
    pub const NODE: Tables = Tables(2);
    /// By primitive: the guest triangle's id byte.
    pub const ID: Tables = Tables(4);

    const WORDS: [(Tables, &'static str); 3] =
        [(Tables::INSTANCE, "instance"), (Tables::NODE, "node"), (Tables::ID, "id")];

    /// Both sets together.
    pub const fn with(self, other: Tables) -> Tables {
        Tables(self.0 | other.0)
    }

    pub const fn has(self, other: Tables) -> bool {
        self.0 & other.0 == other.0
    }

    fn word(self) -> String {
        let words: Vec<&str> = Tables::WORDS.iter().filter(|(t, _)| self.has(*t)).map(|&(_, w)| w).collect();
        if words.is_empty() { "none".to_owned() } else { words.join("+") }
    }

    fn parse(text: &str) -> Option<Tables> {
        if text == "none" {
            return Some(Tables::NONE);
        }
        text.split('+').try_fold(Tables::NONE, |set, word| {
            let (table, _) = Tables::WORDS.iter().find(|(_, w)| *w == word)?;
            // A table named twice is not a set this build wrote.
            (!set.has(*table)).then(|| set.with(*table))
        })
    }
}

impl Form {
    const fn word(self) -> &'static str {
        match self {
            Form::Dual => "dual",
            Form::BothLoops => "both-loops",
        }
    }

    fn parse(text: &str) -> Option<Form> {
        [Form::Dual, Form::BothLoops].into_iter().find(|form| form.word() == text)
    }
}

impl Source {
    const fn word(self) -> &'static str {
        match self {
            Source::Descriptions => "descriptions",
            Source::Packed => "packed",
            Source::Nodes => "nodes",
        }
    }

    fn parse(text: &str) -> Option<Source> {
        [Source::Descriptions, Source::Packed, Source::Nodes].into_iter().find(|source| source.word() == text)
    }
}

/// What a row is for, beside its path: a workload class in one shader
/// form, with one structure source and one set of side tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub class: Class,
    pub form: Form,
    pub source: Source,
    pub tables: Tables,
}

/// The probe's verdict on one path of a [`Key`] (docs/dxr-validation.md,
/// "Rules"), with what it was given under and for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    pub passed: bool,
    /// The number of the rule of equivalence, its constants among it, the
    /// verdict was given under.
    pub rule: u32,
    /// The version of the probe that gave it: its scenes and pass rules.
    pub version: u32,
    /// The translator that made the variant of the probe's program. A
    /// workload made by another translator has no verdict here.
    pub translator: Translator,
    /// The production container of the probe's own program for the class:
    /// recorded so that a report can name what ran, and compared with
    /// nothing. A title program of the class has another container.
    pub container: Digest,
}

/// A probe row: the verdict, and the fields this build does not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeRow {
    pub probe: Probe,
    /// The fields this build does not know, as read (`key=value` words
    /// with a space between), written back after the ones it does.
    pub extra: String,
}

/// Which comparison on the title a title row is of: one path's variant of
/// one program, by its container, against the software container of the
/// same program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TitleKey {
    pub key: Key,
    pub path: Path,
    /// The variant's container: the one that was compared and timed.
    pub container: Digest,
    /// The software container it was compared with and timed against.
    pub software: Digest,
}

/// The title comparison's verdict on a container
/// (docs/dxr-validation.md, "After the probe, on the title"), with the
/// rule it was given under and the two counts that say it compared
/// anything: the class's dispatches that ran the path, and the frames that
/// counted. A verdict over none of either passes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Title {
    pub passed: bool,
    /// As [`Probe::rule`].
    pub rule: u32,
    pub dispatches: u64,
    pub frames: u64,
}

/// The times measured on the title in one comparison run, in microseconds
/// a counted frame: the path's median (its trace dispatches and the host
/// builds it adds) and the software path's (its trace dispatches), each
/// with its spread (the interquartile range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Times {
    pub median_us: u64,
    pub spread_us: u64,
    pub software_median_us: u64,
    pub software_spread_us: u64,
}

impl Times {
    /// Whether the path was measured faster than software by more than the
    /// larger of the two spreads.
    pub fn faster(&self) -> bool {
        let spread = self.spread_us.max(self.software_spread_us);
        self.median_us.checked_add(spread).is_some_and(|slowest| slowest < self.software_median_us)
    }

    /// Whether this path was measured faster than the path `other`'s times
    /// are of. The two were measured in two runs, so their medians are set
    /// against each other only where the runs can be: software's medians
    /// in the two differ by no more than the larger of its two spreads.
    /// Then this path is the faster if its median is below the other's by
    /// more than the larger of the two paths' spreads. Otherwise nothing
    /// was measured between the two.
    pub fn faster_than(&self, other: &Times) -> bool {
        let comparable = self.software_median_us.abs_diff(other.software_median_us)
            <= self.software_spread_us.max(other.software_spread_us);
        let spread = self.spread_us.max(other.spread_us);
        comparable && self.median_us.checked_add(spread).is_some_and(|slowest| slowest < other.median_us)
    }
}

/// A title row: the verdict, the times, either of which may be missing
/// (not both), and the fields this build does not read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleRow {
    pub title: Option<Title>,
    pub times: Option<Times>,
    /// As [`ProbeRow::extra`].
    pub extra: String,
}

/// The value of `name` among the fields of a row this build does not
/// read; the last, if the row has it twice.
fn field_of<'a>(extra: &'a str, name: &str) -> Option<&'a str> {
    let fields = extra.split(' ').filter_map(|word| word.split_once('='));
    fields.filter(|(key, _)| *key == name).map(|(_, value)| value).next_back()
}

impl ProbeRow {
    /// The value of `name` among the fields this build does not read
    /// ([`Record::set_probe_field`]); the last, if the row has it twice.
    pub fn field(&self, name: &str) -> Option<&str> {
        field_of(&self.extra, name)
    }
}

impl TitleRow {
    /// As [`ProbeRow::field`] ([`Record::set_title_field`]).
    pub fn field(&self, name: &str) -> Option<&str> {
        field_of(&self.extra, name)
    }
}

/// Why a record was not read, written or added to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The text does not begin as a record of some version does.
    NotARecord,
    /// The record is of a later version than this build reads.
    Newer { found: u32 },
    /// The second line is not a readable `device` line.
    NoDevice,
    /// A second first line or a second `device` line: more than one record
    /// in one text. Whose rows the later lines are cannot be said, so
    /// nothing is read from any of it.
    Several,
    /// More text than [`MAX_BYTES`], or more lines than [`MAX_ROWS`].
    TooLarge,
    /// The record holds [`MAX_ROWS`] rows: one more is not added.
    Full,
    /// The software path needs no validation and has no rows.
    Software,
    /// The device gave no driver version: nothing is recorded for it.
    NoDriver,
    /// A field was given for a row the record does not hold: a field goes
    /// with a verdict or with times.
    NoRow,
    /// Not a field a row takes: a name this build reads itself, a name or
    /// value with a space or none of it, or more text than [`MAX_FIELDS`]
    /// in one row.
    Field,
    /// The file or its folder could not be read or written.
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotARecord => write!(f, "not a validation record (no `{MAGIC} version=` line)"),
            Error::Newer { found } => write!(
                f,
                "a validation record of version {found}, later than the version {VERSION} this build reads: \
                 nothing is selected from it"
            ),
            Error::NoDevice => {
                write!(f, "a validation record whose second line does not say which device and driver it is for")
            }
            Error::Several => write!(
                f,
                "more than one validation record in one text (a second `{MAGIC}` or `device` line): nothing is \
                 selected from it"
            ),
            Error::TooLarge => {
                write!(f, "a validation record over its bound ({MAX_BYTES} bytes of text, {MAX_ROWS} rows)")
            }
            Error::Full => write!(f, "the validation record holds {MAX_ROWS} rows and takes no more"),
            Error::Software => write!(f, "the software path has no rows in the validation record"),
            Error::NoDriver => {
                write!(f, "the device gave no driver version: no validation record is kept for it")
            }
            Error::NoRow => write!(f, "the validation record has no such row to put a field in"),
            Error::Field => write!(
                f,
                "not a field the validation record takes beside its own (a word for a name the record does not \
                 use, a word for its value, {MAX_FIELDS} bytes a row at most)"
            ),
            Error::Io(error) => write!(f, "the validation record's file: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// The `key=value` fields of one line, after its first word.
struct Fields<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Fields<'a> {
    /// The fields of `rest`, or `None` if a word of it is not `key=value`.
    fn of(rest: std::str::SplitWhitespace<'a>) -> Option<Fields<'a>> {
        rest.map(|word| word.split_once('=').filter(|(key, _)| !key.is_empty())).collect::<Option<_>>().map(Fields)
    }

    /// The value of `key`, if the line has it exactly once.
    fn one(&self, key: &str) -> Option<&'a str> {
        let mut values = self.0.iter().filter(|(k, _)| *k == key).map(|&(_, v)| v);
        let value = values.next()?;
        values.next().is_none().then_some(value)
    }

    fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| *k == key)
    }

    /// The fields whose names are not in `known`, as they were read.
    fn others(&self, known: &[&str]) -> String {
        let unknown = self.0.iter().filter(|(key, _)| !known.contains(key));
        unknown.map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join(" ")
    }

    /// `key` present once and readable, absent, or (the outer `None`)
    /// unreadable.
    fn optional<T>(&self, key: &str, read: impl Fn(&str) -> Option<T>) -> Option<Option<T>> {
        if !self.has(key) {
            return Some(None);
        }
        read(self.one(key)?).map(Some)
    }
}

/// The first word of a probe row's line, and of a title row's.
const PROBE: &str = "probe";
const TITLE: &str = "title";

/// The fields of a probe row this build reads, in the order it writes
/// them; and those of a title row.
const PROBE_FIELDS: [&str; 10] =
    ["class", "path", "form", "source", "tables", "translator", "container", "verdict", "rule", "version"];
const TITLE_FIELDS: [&str; 15] = [
    "class",
    "path",
    "form",
    "source",
    "tables",
    "container",
    "software",
    "verdict",
    "rule",
    "dispatches",
    "frames",
    "median_us",
    "spread_us",
    "software_median_us",
    "software_spread_us",
];

fn verdict(text: &str) -> Option<bool> {
    match text {
        "passed" => Some(true),
        "failed" => Some(false),
        _ => None,
    }
}

const fn verdict_word(passed: bool) -> &'static str {
    if passed { "passed" } else { "failed" }
}

fn digest(text: &str) -> Option<Digest> {
    unhex(text).map(Digest)
}

/// The class key and the path every row's line names.
fn subject(fields: &Fields<'_>) -> Option<(Key, Path)> {
    let key = Key {
        class: Class(unhex(fields.one("class")?)?),
        form: Form::parse(fields.one("form")?)?,
        source: Source::parse(fields.one("source")?)?,
        tables: Tables::parse(fields.one("tables")?)?,
    };
    let named = fields.one("path")?;
    let path = [Path::Native, Path::Hybrid].into_iter().find(|path| path.word() == named)?;
    Some((key, path))
}

/// One probe row from the fields of its line, or `None` if a field this
/// build reads is missing, repeated or says something it does not
/// understand: the line is then kept as it is and never matched. A verdict
/// that does not say what it was given under is such a line.
fn probe_row(fields: &Fields<'_>) -> Option<((Key, Path), ProbeRow)> {
    let subject = subject(fields)?;
    let probe = Probe {
        passed: verdict(fields.one("verdict")?)?,
        rule: fields.one("rule")?.parse().ok()?,
        version: fields.one("version")?.parse().ok()?,
        translator: Translator(unhex(fields.one("translator")?)?),
        container: digest(fields.one("container")?)?,
    };
    Some((subject, ProbeRow { probe, extra: fields.others(&PROBE_FIELDS) }))
}

/// One title row from the fields of its line, or `None` as for
/// [`probe_row`]. A verdict comes with its rule and both counts, times all
/// four together, and a row has a verdict or times.
fn title_row(fields: &Fields<'_>) -> Option<(TitleKey, TitleRow)> {
    let number = |text: &str| text.parse::<u64>().ok();
    let (key, path) = subject(fields)?;
    let of = TitleKey {
        key,
        path,
        container: digest(fields.one("container")?)?,
        software: digest(fields.one("software")?)?,
    };
    let rule = fields.optional("rule", |text| text.parse::<u32>().ok())?;
    let (dispatches, frames) = (fields.optional("dispatches", number)?, fields.optional("frames", number)?);
    let title = match (fields.optional("verdict", verdict)?, rule, dispatches, frames) {
        (Some(passed), Some(rule), Some(dispatches), Some(frames)) => Some(Title { passed, rule, dispatches, frames }),
        (None, None, None, None) => None,
        _ => return None,
    };
    let times = ["median_us", "spread_us", "software_median_us", "software_spread_us"];
    let [median, spread, software_median, software_spread] = times.map(|key| fields.optional(key, number));
    let times = match (median?, spread?, software_median?, software_spread?) {
        (Some(median_us), Some(spread_us), Some(software_median_us), Some(software_spread_us)) => {
            Some(Times { median_us, spread_us, software_median_us, software_spread_us })
        }
        (None, None, None, None) => None,
        _ => return None,
    };
    if title.is_none() && times.is_none() {
        return None;
    }
    Some((of, TitleRow { title, times, extra: fields.others(&TITLE_FIELDS) }))
}

/// Puts `name=value` among the fields `extra` holds, in place of a field
/// of that name if there is one.
fn put_field(extra: &mut String, name: &str, value: &str) -> Result<(), Error> {
    let word = |text: &str| !text.is_empty() && !text.contains(char::is_whitespace);
    let known = PROBE_FIELDS.contains(&name) || TITLE_FIELDS.contains(&name);
    if !word(name) || !word(value) || name.contains('=') || known {
        return Err(Error::Field);
    }
    let others = extra.split(' ').filter(|word| word.split_once('=').is_some_and(|(key, _)| key != name));
    let fields = others.map(str::to_owned).chain([format!("{name}={value}")]).collect::<Vec<_>>().join(" ");
    if fields.len() > MAX_FIELDS {
        return Err(Error::Field);
    }
    *extra = fields;
    Ok(())
}

/// Takes the row `row` that `line` gave for `key` while a record is read.
/// A key that a line before it gave already has two lines that say two
/// things: neither is matched, and both are kept as they were (`seen`
/// holds the first line of each key that has one line so far, and nothing
/// for a key with more).
fn take_row<'a, K: Ord + Copy, R>(
    rows: &mut BTreeMap<K, R>,
    seen: &mut BTreeMap<K, Option<&'a str>>,
    unread: &mut Vec<String>,
    (key, row): (K, R),
    line: &'a str,
) {
    match seen.insert(key, None) {
        None => {
            rows.insert(key, row);
            seen.insert(key, Some(line));
        }
        Some(first) => {
            if let Some(first) = first {
                rows.remove(&key);
                unread.push(first.to_owned());
            }
            unread.push(line.to_owned());
        }
    }
}

/// The validation record of one device and driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    identity: Identity,
    probes: BTreeMap<(Key, Path), ProbeRow>,
    titles: BTreeMap<TitleKey, TitleRow>,
    unread: Vec<String>,
}

impl Record {
    /// An empty record for the device and driver `identity`.
    pub fn new(identity: Identity) -> Record {
        Record { identity, probes: BTreeMap::new(), titles: BTreeMap::new(), unread: Vec::new() }
    }

    /// The device and driver the record was made on.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Whether the record's data holds for `device`
    /// ([`Identity::same`]). One made on another device, or with another
    /// driver, counts as absent.
    pub fn holds_for(&self, device: &Identity) -> bool {
        device.same(&self.identity)
    }

    /// The probe's row for `path` of `key`.
    pub fn probe(&self, key: &Key, path: Path) -> Option<&ProbeRow> {
        self.probes.get(&(*key, path))
    }

    /// The title row of the comparison `of`.
    pub fn title(&self, of: &TitleKey) -> Option<&TitleRow> {
        self.titles.get(of)
    }

    /// Every title row for `path` of `key`, whatever its containers, in a
    /// fixed order.
    pub fn titles_of<'a>(
        &'a self,
        key: &Key,
        path: Path,
    ) -> impl Iterator<Item = (&'a TitleKey, &'a TitleRow)> + use<'a> {
        let end = |byte| TitleKey { key: *key, path, container: Digest([byte; 32]), software: Digest([byte; 32]) };
        self.titles.range(end(0x00)..=end(0xff))
    }

    /// Every probe row, in a fixed order.
    pub fn probes(&self) -> impl Iterator<Item = (&Key, Path, &ProbeRow)> {
        self.probes.iter().map(|((key, path), row)| (key, *path, row))
    }

    /// Every title row, in a fixed order.
    pub fn titles(&self) -> impl Iterator<Item = (&TitleKey, &TitleRow)> {
        self.titles.iter()
    }

    /// The rows read or set, of both kinds (not the lines that could not
    /// be read).
    pub fn len(&self) -> usize {
        self.probes.len() + self.titles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The lines that were not understood when the record was read: kept
    /// to be written back, and matched by nothing.
    pub fn unread(&self) -> &[String] {
        &self.unread
    }

    /// Whether one more row may be added.
    fn room(&self) -> Result<(), Error> {
        if self.len() + self.unread.len() >= MAX_ROWS { Err(Error::Full) } else { Ok(()) }
    }

    /// Records the probe's verdict on `path` of `key`. It takes the place
    /// of the verdict there, if there is one, with that one's fields: the
    /// row that went is returned, for the caller to say what was replaced
    /// (a verdict for an earlier translator, an earlier run's).
    pub fn set_probe(&mut self, key: Key, path: Path, probe: Probe) -> Result<Option<ProbeRow>, Error> {
        if path == Path::Software {
            return Err(Error::Software);
        }
        if !self.probes.contains_key(&(key, path)) {
            self.room()?;
        }
        Ok(self.probes.insert((key, path), ProbeRow { probe, extra: String::new() }))
    }

    /// The title row of `of`, made if it is not there.
    fn title_row(&mut self, of: TitleKey) -> Result<&mut TitleRow, Error> {
        if of.path == Path::Software {
            return Err(Error::Software);
        }
        if !self.titles.contains_key(&of) {
            self.room()?;
        }
        Ok(self.titles.entry(of).or_default())
    }

    /// Records the title comparison's verdict on the container of `of`
    /// against its software container, in that comparison's own row: rows
    /// of other containers stay as they are. Returns the verdict it took
    /// the place of, if the row had one.
    pub fn set_title(&mut self, of: TitleKey, title: Title) -> Result<Option<Title>, Error> {
        Ok(self.title_row(of)?.title.replace(title))
    }

    /// Records the times measured on the title for the container of `of`
    /// and for its software container in the same run. Returns the times
    /// they took the place of, if the row had some.
    pub fn set_times(&mut self, of: TitleKey, times: Times) -> Result<Option<Times>, Error> {
        Ok(self.title_row(of)?.times.replace(times))
    }

    /// Records `name=value` in the probe's row for `path` of `key`:
    /// something found with the verdict that the selection does not read
    /// (the probe's levels, the largest coordinate it covered, the value
    /// of the tolerance constant, a scene, a date). It is kept with the
    /// row and written back, in place of a field of that name if the row
    /// has one. The row must be there ([`Error::NoRow`]).
    pub fn set_probe_field(&mut self, key: Key, path: Path, name: &str, value: &str) -> Result<(), Error> {
        let row = self.probes.get_mut(&(key, path)).ok_or(Error::NoRow)?;
        put_field(&mut row.extra, name, value)
    }

    /// As [`Record::set_probe_field`], in the title row of `of` (capture
    /// ids, a date).
    pub fn set_title_field(&mut self, of: TitleKey, name: &str, value: &str) -> Result<(), Error> {
        let row = self.titles.get_mut(&of).ok_or(Error::NoRow)?;
        put_field(&mut row.extra, name, value)
    }

    /// Takes the probe's row for `path` of `key` out of the record and
    /// returns it.
    pub fn forget_probe(&mut self, key: &Key, path: Path) -> Option<ProbeRow> {
        self.probes.remove(&(*key, path))
    }

    /// Takes the title row of `of` out of the record and returns it: how
    /// a tool makes room for rows by giving up those of containers no
    /// translation makes any more, saying which.
    pub fn forget_title(&mut self, of: &TitleKey) -> Option<TitleRow> {
        self.titles.remove(of)
    }

    /// About the bytes the record takes in memory: its rows at their size,
    /// and the text of what was kept without being understood. Under
    /// 1.5 MiB for any record [`Record::parse`] takes: [`MAX_ROWS`] rows
    /// and lines, and no more of that text than [`MAX_BYTES`].
    pub fn bytes(&self) -> u64 {
        let probes = self.probes.len() * size_of::<((Key, Path), ProbeRow)>();
        let titles = self.titles.len() * size_of::<(TitleKey, TitleRow)>();
        let probe_extra: usize = self.probes.values().map(|row| row.extra.len()).sum();
        let title_extra: usize = self.titles.values().map(|row| row.extra.len()).sum();
        let unread: usize = self.unread.iter().map(|line| size_of::<String>() + line.len()).sum();
        (size_of::<Record>() + probes + titles + probe_extra + title_extra + unread) as u64
    }

    /// The record as text: what [`Record::parse`] reads back equal.
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        fn subject(text: &mut String, kind: &str, key: &Key, path: Path) {
            let _ = write!(
                text,
                "{kind} class={} path={} form={} source={} tables={}",
                key.class.hex(),
                path.word(),
                key.form.word(),
                key.source.word(),
                key.tables.word()
            );
        }
        fn end(text: &mut String, extra: &str) {
            if !extra.is_empty() {
                let _ = write!(text, " {extra}");
            }
            text.push('\n');
        }
        let mut text = format!("{MAGIC} version={VERSION}\ndevice {}\n", self.identity.fields());
        for ((key, path), row) in &self.probes {
            subject(&mut text, PROBE, key, *path);
            let probe = &row.probe;
            let _ = write!(
                text,
                " translator={} container={} verdict={} rule={} version={}",
                probe.translator.hex(),
                probe.container.hex(),
                verdict_word(probe.passed),
                probe.rule,
                probe.version
            );
            end(&mut text, &row.extra);
        }
        for (of, row) in &self.titles {
            subject(&mut text, TITLE, &of.key, of.path);
            let _ = write!(text, " container={} software={}", of.container.hex(), of.software.hex());
            if let Some(title) = row.title {
                let _ = write!(
                    text,
                    " verdict={} rule={} dispatches={} frames={}",
                    verdict_word(title.passed),
                    title.rule,
                    title.dispatches,
                    title.frames
                );
            }
            if let Some(times) = row.times {
                let _ = write!(
                    text,
                    " median_us={} spread_us={} software_median_us={} software_spread_us={}",
                    times.median_us, times.spread_us, times.software_median_us, times.software_spread_us
                );
            }
            end(&mut text, &row.extra);
        }
        for line in &self.unread {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// The record `text` holds. Unknown fields and lines are kept, and two
    /// lines for one key are kept and not matched (the module text); a
    /// later version, a second line that is not the identity, a second
    /// record in the text and a record over its bound are errors, and the
    /// caller then has no record.
    pub fn parse(text: &str) -> Result<Record, Error> {
        if text.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
        let mut header = lines.next().ok_or(Error::NotARecord)?.split_whitespace();
        if header.next() != Some(MAGIC) {
            return Err(Error::NotARecord);
        }
        let version = Fields::of(header).and_then(|fields| fields.one("version")?.parse::<u32>().ok());
        match version.ok_or(Error::NotARecord)? {
            0 => return Err(Error::NotARecord),
            found if found > VERSION => return Err(Error::Newer { found }),
            _ => {}
        }
        // The identity is the line after: every row below it is this
        // device's, and no later line may say otherwise.
        let mut device = lines.next().ok_or(Error::NoDevice)?.split_whitespace();
        if device.next() != Some("device") {
            return Err(Error::NoDevice);
        }
        let identity = Fields::of(device).and_then(|fields| Identity::parse(&fields)).ok_or(Error::NoDevice)?;
        let mut record = Record::new(identity);
        let (mut probes_seen, mut titles_seen) = (BTreeMap::new(), BTreeMap::new());
        for line in lines {
            let mut words = line.split_whitespace();
            let (kind, fields) = (words.next(), Fields::of(words));
            if kind == Some(MAGIC) || kind == Some("device") {
                return Err(Error::Several);
            }
            let read = match (kind, &fields) {
                (Some(PROBE), Some(fields)) => probe_row(fields)
                    .map(|row| take_row(&mut record.probes, &mut probes_seen, &mut record.unread, row, line)),
                (Some(TITLE), Some(fields)) => title_row(fields)
                    .map(|row| take_row(&mut record.titles, &mut titles_seen, &mut record.unread, row, line)),
                _ => None,
            };
            if read.is_none() {
                record.unread.push(line.to_owned());
            }
            if record.len() + record.unread.len() > MAX_ROWS {
                return Err(Error::TooLarge);
            }
        }
        Ok(record)
    }
}

/// The file in `dir` that holds the record of `device`, named by the start
/// of the identity's hash; `None` for a device that gave no driver version
/// ([`Identity::known`]).
pub fn file(dir: &FsPath, device: &Identity) -> Option<PathBuf> {
    let name = blake3::hash(device.fields().as_bytes()).to_hex();
    device.known().then(|| dir.join(format!("{}.txt", &name[..16])))
}

/// The text `from` holds, read through the record's bound: one byte past
/// [`MAX_BYTES`] at most is asked for, whatever a file system says the
/// size is and however much there is to read, and a text that long is
/// refused.
fn read_text(from: impl Read) -> Result<String, Error> {
    let mut bytes = Vec::new();
    let read = from.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes);
    read.map_err(|error| Error::Io(error.to_string()))?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| Error::NotARecord)
}

/// The record kept in `dir` for `device`: `None` if there is no file for
/// it, or the file says it is for another device or driver. A file that
/// is there and is not read is an error, for the caller to say.
pub fn load(dir: &FsPath, device: &Identity) -> Result<Option<Record>, Error> {
    let Some(path) = file(dir, device) else { return Ok(None) };
    let named = |error: Error| match error {
        Error::Io(error) => Error::Io(format!("{}: {error}", path.display())),
        other => other,
    };
    let text = match std::fs::File::open(&path) {
        Ok(file) => read_text(file).map_err(named)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(named(Error::Io(error.to_string()))),
    };
    let record = Record::parse(&text)?;
    if !record.holds_for(device) {
        // The name is this device's and the content another's: a file
        // copied or renamed. Said, since nothing else would show why a
        // record that is there selects nothing.
        tracing::warn!(
            "ray tracing: the validation record {} is for another device or driver ({}): it counts as absent",
            path.display(),
            record.identity()
        );
        return Ok(None);
    }
    Ok(Some(record))
}

/// Writes `record` to its file in `dir` (made if it is not there), whole
/// and under another name first, so that a reader finds the old record or
/// the new one. A record whose text is over [`MAX_BYTES`] is refused and
/// nothing is written. Returns the file.
pub fn save(dir: &FsPath, record: &Record) -> Result<PathBuf, Error> {
    let path = file(dir, record.identity()).ok_or(Error::NoDriver)?;
    let text = record.to_text();
    if text.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    let io = |error: std::io::Error| Error::Io(format!("{}: {error}", path.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    let partial = path.with_extension(format!("{}.partial", std::process::id()));
    std::fs::write(&partial, text).map_err(io)?;
    std::fs::rename(&partial, &path).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        io(error)
    })?;
    Ok(path)
}

/// A record kept for a run, its bytes counted while it is kept
/// (`crate::account::Owner::RtRecord`).
pub struct Held {
    record: Record,
    _bytes: crate::account::Held,
}

impl Held {
    /// `record`, kept and counted from now until this is dropped.
    pub fn new(record: Record) -> Held {
        let bytes = crate::account::Held::new(crate::account::Owner::RtRecord, record.bytes());
        Held { record, _bytes: bytes }
    }

    /// The record of `device` in `dir`, kept and counted, if there is one.
    /// A file that is there and is not read is said as a warning, with
    /// what follows from it: there is then no record.
    pub fn open(dir: &FsPath, device: &Identity) -> Option<Held> {
        match load(dir, device) {
            Ok(record) => record.map(Held::new),
            Err(error) => {
                tracing::warn!(
                    "ray tracing: {error} ({}): there is no validation record, so `auto` selects the software \
                     path for every workload",
                    dir.display()
                );
                None
            }
        }
    }

    pub fn record(&self) -> &Record {
        &self.record
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(driver: [u16; 4]) -> Identity {
        Identity::of(&HostCaps { vendor: 0x10de, device: 0x2c05, driver: Some(driver), ..Default::default() })
    }

    fn here() -> Identity {
        device([32, 0, 15, 7000])
    }

    fn key(class: u8) -> Key {
        Key { class: Class([class; 32]), form: Form::Dual, source: Source::Descriptions, tables: Tables::NONE }
    }

    const SOFTWARE: Digest = Digest([0x50; 32]);
    const TIMES: Times = Times { median_us: 400, spread_us: 30, software_median_us: 900, software_spread_us: 45 };
    const PASSED: Probe = Probe {
        passed: true,
        rule: 1,
        version: 3,
        translator: Translator([0x77; 32]),
        container: Digest([0xb0; 32]),
    };
    const COMPARED: Title = Title { passed: true, rule: 1, dispatches: 1200, frames: 600 };

    /// The comparison of the native variant of a program of class `class`,
    /// whose container is `container`, with that program's software
    /// container.
    fn of(class: u8, container: u8) -> TitleKey {
        TitleKey { key: key(class), path: Path::Native, container: Digest([container; 32]), software: SOFTWARE }
    }

    /// The first two lines of a record made here.
    fn head() -> String {
        format!("victory-rt-record version=1\ndevice {}\n", here())
    }

    /// A record with every part a row can have, and rows that have few:
    /// two probe rows and two title rows.
    fn full() -> Record {
        let mut record = Record::new(here());
        record.set_probe(key(1), Path::Native, PASSED).unwrap();
        record.set_title(of(1, 0xc0), COMPARED).unwrap();
        record.set_times(of(1, 0xc0), TIMES).unwrap();
        let failed = Probe { passed: false, container: Digest([0xb1; 32]), ..PASSED };
        record.set_probe(key(1), Path::Hybrid, failed).unwrap();
        let other = Key {
            form: Form::BothLoops,
            source: Source::Nodes,
            tables: Tables::INSTANCE.with(Tables::ID),
            ..key(2)
        };
        let timed = TitleKey { key: other, path: Path::Hybrid, container: Digest([0xc2; 32]), software: SOFTWARE };
        record.set_times(timed, TIMES).unwrap();
        record
    }

    #[test]
    fn a_record_reads_back_as_it_was_written() {
        let record = full();
        let text = record.to_text();
        assert_eq!(Record::parse(&text), Ok(record.clone()));
        assert_eq!(Record::parse(&text).unwrap().to_text(), text, "and writes the same text again");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "victory-rt-record version=1");
        assert_eq!(lines[1], "device vendor=0x10de device=0x2c05 driver=32.0.15.7000");
        let (class, translator) = ("01".repeat(32), "77".repeat(32));
        assert_eq!(
            lines[2],
            format!(
                "probe class={class} path=native form=dual source=descriptions tables=none \
                 translator={translator} container={} verdict=passed rule=1 version=3",
                "b0".repeat(32)
            )
        );
        assert_eq!(
            lines[3],
            format!(
                "probe class={class} path=hybrid form=dual source=descriptions tables=none \
                 translator={translator} container={} verdict=failed rule=1 version=3",
                "b1".repeat(32)
            )
        );
        assert_eq!(
            lines[4],
            format!(
                "title class={class} path=native form=dual source=descriptions tables=none container={} \
                 software={} verdict=passed rule=1 dispatches=1200 frames=600 median_us=400 spread_us=30 \
                 software_median_us=900 software_spread_us=45",
                "c0".repeat(32),
                "50".repeat(32)
            )
        );
        assert!(lines[5].starts_with(&format!("title class={} path=hybrid form=both-loops ", "02".repeat(32))));
        assert!(lines[5].contains(" source=nodes tables=instance+id container="), "{}", lines[5]);
        assert!(!lines[5].contains("verdict=") && lines[5].ends_with(" software_spread_us=45"), "{}", lines[5]);
        assert_eq!(lines.len(), 6);
        assert_eq!((record.len(), record.probes().count(), record.titles().count()), (4, 2, 2));
        assert_eq!(Record::parse(&Record::new(here()).to_text()), Ok(Record::new(here())), "an empty one too");
    }

    #[test]
    fn every_word_of_a_key_reads_back() {
        for form in [Form::Dual, Form::BothLoops] {
            assert_eq!(Form::parse(form.word()), Some(form));
        }
        for source in [Source::Descriptions, Source::Packed, Source::Nodes] {
            assert_eq!(Source::parse(source.word()), Some(source));
        }
        for bits in 0..8 {
            assert_eq!(Tables::parse(&Tables(bits).word()), Some(Tables(bits)), "{bits}");
        }
        assert_eq!(Tables::NODE.with(Tables::ID).word(), "node+id");
        for text in ["", "node+node", "none+id", "boxes", "node+"] {
            assert_eq!(Tables::parse(text), None, "{text:?}");
        }
        assert_eq!(unhex(&"aB".repeat(32)), Some([0xab; 32]));
        for text in ["ab".repeat(31), "ab".repeat(33), "g0".repeat(32), format!("+{}", "0".repeat(63))] {
            assert_eq!(unhex(&text), None, "{text:?}");
        }
        assert_eq!(Class([0x1f; 32]).short(), "1f".repeat(8));
        assert_eq!(Translator([0x2e; 32]).hex(), "2e".repeat(32));
        // A container is named by its bytes, the same wherever it is hashed.
        assert_eq!(Digest::of(b"a container"), Digest::of(b"a container"));
        assert_ne!(Digest::of(b"a container"), Digest::of(b"another"));
        assert_eq!(Digest::of(b"").hex(), blake3::hash(b"").to_hex().as_str());
        assert_eq!(unhex(&Digest::of(b"a container").hex()), Some(Digest::of(b"a container").0));
    }

    /// What a later build of the same version adds is kept: a field with
    /// its row, a line as it was. Neither is lost when this build writes
    /// the record again, and neither is matched by anything.
    #[test]
    fn unknown_fields_and_lines_are_kept_and_written_back() {
        let mut text = full().to_text();
        let verdict = " verdict=passed rule=1 version=3";
        assert_eq!(text.matches(verdict).count(), 1, "the native probe row");
        text = text.replacen(verdict, " m_max=4096.5 verdict=passed rule=1 version=3 level3=agree", 1);
        text = text.replacen(" median_us=400", " capture=17 median_us=400", 1);
        text.push_str("summary rows=3 note=later\n# a comment\nprobe class=short path=native\nrow class=x\n\n   \n");
        let record = Record::parse(&text).unwrap();
        assert_eq!(record.len(), 4);
        let row = record.probe(&key(1), Path::Native).unwrap();
        assert_eq!(row.extra, "m_max=4096.5 level3=agree");
        assert_eq!(row.probe, PASSED, "the known fields beside them are read");
        let row = record.title(&of(1, 0xc0)).unwrap();
        assert_eq!((row.extra.as_str(), row.title, row.times), ("capture=17", Some(COMPARED), Some(TIMES)));
        assert_eq!(
            record.unread(),
            ["summary rows=3 note=later", "# a comment", "probe class=short path=native", "row class=x"]
        );

        let again = record.to_text();
        assert!(again.contains(" version=3 m_max=4096.5 level3=agree\n"), "{again}");
        assert!(again.contains(" software_spread_us=45 capture=17\n"), "{again}");
        let kept = "summary rows=3 note=later\n# a comment\nprobe class=short path=native\nrow class=x\n";
        assert!(again.ends_with(kept), "{again}");
        assert_eq!(Record::parse(&again), Ok(record.clone()));
        assert!(record.bytes() > full().bytes(), "what is kept is counted");

        // The first line and the `device` line may carry fields this build
        // does not know.
        let header = full().to_text().replacen("version=1", "version=1 probe=7", 1);
        assert_eq!(Record::parse(&header), Ok(full()));
        let device = full().to_text().replacen("driver=32.0.15.7000", "driver=32.0.15.7000 os=26100", 1);
        assert_eq!(Record::parse(&device), Ok(full()));
    }

    /// A row is matched only if every field this build reads is one it
    /// understands: a guess at a verdict, at what it was given under or at
    /// a key would select a path from something nobody recorded.
    #[test]
    fn a_row_with_a_field_this_build_does_not_understand_is_kept_unread() {
        let whole = full().to_text();
        let (probe, title) = (whole.lines().nth(2).unwrap(), whole.lines().nth(4).unwrap());
        let class = "01".repeat(32);
        let unread = |line: &str, from: &str, to: &str| {
            assert!(line.contains(from), "{from:?}");
            let changed = line.replacen(from, to, 1);
            let record = Record::parse(&format!("{}{changed}\n", head())).unwrap_or_else(|e| panic!("{from:?}: {e}"));
            assert_eq!((record.len(), record.unread()), (0, [changed.clone()].as_slice()), "{from:?} to {to:?}");
        };
        for (from, to) in [
            (" verdict=passed", " verdict=inconclusive"),
            (" verdict=passed", ""),
            (" path=native", " path=software"),
            (" path=native", " path=compute"),
            (" path=native", ""),
            (" form=dual", " form=triple"),
            (" source=descriptions", " source=elsewhere"),
            (" tables=none", " tables=boxes"),
            // A verdict that does not say under which rule, or by which
            // version of the probe, it was given.
            (" rule=1", ""),
            (" rule=1", " rule=one"),
            (" rule=1", " rule=-1"),
            (" version=3", ""),
            (" version=3", " version=three"),
            (" translator=77", " translator=7"),
            (" translator=", " translater="),
            (" container=", " container=00"),
            (" container=", " contained="),
            (" form=dual", ""),
            (" form=dual", " form=dual form=dual"),
            (" form=dual", " form"),
            (" form=dual", " =dual"),
            (class.as_str(), "0g"),
            // A probe's line is not a title's.
            ("probe class", "title class"),
        ] {
            unread(probe, from, to);
        }
        let all = " verdict=passed rule=1 dispatches=1200 frames=600 median_us=400 spread_us=30 \
                   software_median_us=900 software_spread_us=45";
        for (from, to) in [
            (" verdict=passed", " verdict=mostly"),
            // The rule and the counts without a verdict; a verdict without
            // one of them.
            (" verdict=passed", ""),
            (" rule=1", ""),
            (" dispatches=1200", ""),
            (" frames=600", ""),
            (" frames=600", " frames=-1"),
            (" median_us=400", ""),
            (" median_us=400", " median_us=0.4"),
            (" spread_us=30", " spread_us=30 spread_us=31"),
            (" software=50", " softwar=50"),
            (" software=50", " software=5"),
            (" container=c0", " container=c"),
            (" path=native", " path=software"),
            // Neither a verdict nor times: the row says nothing.
            (all, ""),
            ("title class", "probe class"),
        ] {
            unread(title, from, to);
        }
        // What a title row may be without: its times, or its verdict.
        let times = " median_us=400 spread_us=30 software_median_us=900 software_spread_us=45";
        let record = Record::parse(&format!("{}{}\n", head(), title.replacen(times, "", 1))).unwrap();
        let row = TitleRow { title: Some(COMPARED), times: None, extra: String::new() };
        assert_eq!(record.title(&of(1, 0xc0)), Some(&row));
        let verdict = " verdict=passed rule=1 dispatches=1200 frames=600";
        let record = Record::parse(&format!("{}{}\n", head(), title.replacen(verdict, "", 1))).unwrap();
        let row = TitleRow { title: None, times: Some(TIMES), extra: String::new() };
        assert_eq!(record.title(&of(1, 0xc0)), Some(&row));
    }

    #[test]
    fn a_later_version_is_refused_and_says_so() {
        let later = full().to_text().replacen("version=1", "version=2", 1);
        assert_eq!(Record::parse(&later), Err(Error::Newer { found: 2 }));
        let said = Error::Newer { found: 2 }.to_string();
        assert!(said.contains("version 2") && said.contains("nothing is selected from it"), "{said}");
        assert_eq!(Record::parse(&full().to_text().replacen("version=1", "version=4000000000", 1)), {
            Err(Error::Newer { found: 4_000_000_000 })
        });
    }

    #[test]
    fn what_is_no_record_is_an_error_and_never_an_empty_record() {
        let whole = full().to_text();
        for text in [
            String::new(),
            "\n\n".to_owned(),
            "something else\n".to_owned(),
            whole.replacen("victory-rt-record", "victory-record", 1),
            whole.replacen(" version=1", "", 1),
            whole.replacen("version=1", "version=one", 1),
            whole.replacen("version=1", "version=0", 1),
            whole.replacen("version=1", "version=1 version=1", 1),
            whole.replacen("version=1", "version", 1),
        ] {
            assert_eq!(Record::parse(&text), Err(Error::NotARecord), "{text:?}");
        }
        for (from, to) in [
            ("device vendor=", "devise vendor="),
            ("vendor=0x10de", "vendor=10de"),
            ("vendor=0x10de ", ""),
            ("driver=32.0.15.7000", "driver=32.0.15"),
            ("driver=32.0.15.7000", "driver=32.0.15.7000.1"),
            ("driver=32.0.15.7000", "driver=32.0.15.70000"),
            ("driver=32.0.15.7000", "driver=latest"),
            ("driver=32.0.15.7000", "driver=32.0.15.7000 driver=32.0.15.7000"),
        ] {
            assert!(whole.contains(from));
            assert_eq!(Record::parse(&whole.replacen(from, to, 1)), Err(Error::NoDevice), "{to:?}");
        }
        // The identity is the second line and no other: a row before it
        // would be nobody's.
        let lines: Vec<&str> = whole.lines().collect();
        let row_first = format!("{}\n{}\n{}\n", lines[0], lines[2], lines[1]);
        assert_eq!(Record::parse(&row_first), Err(Error::NoDevice));
        assert_eq!(Record::parse("victory-rt-record version=1\n"), Err(Error::NoDevice));
        // Blank lines and the other line ending are not part of the text.
        let spaced = format!("\n  {}", whole.replace('\n', "\r\n\r\n"));
        assert_eq!(Record::parse(&spaced), Ok(full()));
    }

    /// Two records put together in one text. Read as one, the rows taken
    /// on the other device would count for this one: it is refused whole.
    #[test]
    fn a_text_that_holds_two_records_is_refused() {
        let failed = Probe { passed: false, ..PASSED };
        let mut ours = Record::new(here());
        ours.set_probe(key(1), Path::Native, failed).unwrap();
        let mut theirs = Record::new(device([9, 9, 9, 9]));
        theirs.set_probe(key(1), Path::Native, PASSED).unwrap();
        theirs.set_title(of(1, 0xc0), COMPARED).unwrap();
        theirs.set_times(of(1, 0xc0), TIMES).unwrap();
        let (ours, theirs) = (ours.to_text(), theirs.to_text());
        let their_lines: Vec<&str> = theirs.lines().collect();
        let rows = their_lines[2..].join("\n");
        for text in [
            format!("{ours}{theirs}"),
            format!("{theirs}{ours}"),
            // A second first line alone, a second `device` line alone, and
            // one that cannot be read.
            format!("{ours}{}\n{rows}\n", their_lines[0]),
            format!("{ours}{}\n{rows}\n", their_lines[1]),
            format!("{ours}device of another kind\n{rows}\n"),
            format!("{ours}{rows}\n{}\n", their_lines[1]),
            format!("{ours}{ours}"),
        ] {
            assert_eq!(Record::parse(&text), Err(Error::Several), "{text}");
        }
        let said = Error::Several.to_string();
        assert!(said.contains("more than one validation record") && said.contains("nothing is selected"), "{said}");
        // The words elsewhere in a line begin no record.
        let noted = format!("{ours}note device victory-rt-record\n");
        assert_eq!(Record::parse(&noted).map(|record| record.unread().len()), Ok(1));

        // And from a file: no record, said.
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join(FOLDER);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(file(&dir, &here()).unwrap(), format!("{ours}{theirs}")).unwrap();
        assert_eq!(load(&dir, &here()), Err(Error::Several));
        assert!(Held::open(&dir, &here()).is_none());
    }

    /// Two lines for one key say two things about one subject. Neither is
    /// matched, whichever comes first and whatever each says, and both are
    /// kept.
    #[test]
    fn a_key_given_by_two_lines_is_matched_by_neither() {
        let line = |set: &dyn Fn(&mut Record)| {
            let mut record = Record::new(here());
            set(&mut record);
            record.to_text().lines().nth(2).unwrap().to_owned()
        };
        let probe = |path: Path, probe: Probe| {
            line(&|record| {
                record.set_probe(key(1), path, probe).unwrap();
            })
        };
        let passed = probe(Path::Native, PASSED);
        let failed = probe(Path::Native, Probe { passed: false, ..PASSED });
        let hybrid = probe(Path::Hybrid, PASSED);
        for (first, second) in [(&passed, &failed), (&failed, &passed), (&passed, &passed)] {
            let record = Record::parse(&format!("{}{first}\n{second}\n", head())).unwrap();
            assert_eq!(record.probe(&key(1), Path::Native), None);
            assert_eq!((record.len(), record.unread()), (0, [first.clone(), second.clone()].as_slice()));
            let again = record.to_text();
            assert!(again.ends_with(&format!("{first}\n{second}\n")), "{again}");
            assert_eq!(Record::parse(&again), Ok(record), "kept, and read the same again");
        }
        // A third line for the key is kept too; the other path of the key
        // is another key, given once, and is read.
        let text = format!("{}{passed}\n{hybrid}\n{failed}\n{passed}\n", head());
        let record = Record::parse(&text).unwrap();
        assert_eq!(record.unread(), [passed.clone(), failed.clone(), passed.clone()]);
        assert_eq!((record.len(), record.probe(&key(1), Path::Hybrid).map(|row| row.probe)), (1, Some(PASSED)));
        assert_eq!(Record::parse(&record.to_text()), Ok(record));

        // Title rows: the key is the comparison, containers among it.
        let title = |of: TitleKey, title: Title| {
            line(&|record| {
                record.set_title(of, title).unwrap();
            })
        };
        let a_passed = title(of(1, 0xc0), COMPARED);
        let a_failed = title(of(1, 0xc0), Title { passed: false, ..COMPARED });
        let b = title(of(1, 0xc1), COMPARED);
        for (first, second) in [(&a_passed, &a_failed), (&a_failed, &a_passed)] {
            let record = Record::parse(&format!("{}{first}\n{b}\n{second}\n", head())).unwrap();
            assert_eq!(record.title(&of(1, 0xc0)), None);
            assert_eq!(record.title(&of(1, 0xc1)).and_then(|row| row.title), Some(COMPARED), "another container");
            assert_eq!((record.len(), record.unread()), (1, [first.clone(), second.clone()].as_slice()));
        }
        // Lines kept this way count towards the bound as the rows did.
        let mut many = head();
        for _ in 0..=MAX_ROWS {
            many.push_str(&passed);
            many.push('\n');
        }
        assert_eq!(Record::parse(&many), Err(Error::TooLarge));
    }

    /// The identity is an equality key: another device, another driver
    /// version, or a driver version that is not known, and the record
    /// does not hold.
    #[test]
    fn a_record_holds_only_for_the_device_and_driver_it_was_made_on() {
        let record = full();
        assert!(record.holds_for(&here()));
        assert!(!record.holds_for(&device([32, 0, 15, 7001])), "a driver update");
        assert!(!record.holds_for(&device([31, 0, 15, 7000])));
        let caps = HostCaps { vendor: 0x10de, device: 0x2c05, driver: Some([32, 0, 15, 7000]), ..Default::default() };
        assert!(record.holds_for(&Identity::of(&caps)));
        assert!(!record.holds_for(&Identity::of(&HostCaps { device: 0x2c02, ..caps.clone() })));
        assert!(!record.holds_for(&Identity::of(&HostCaps { vendor: 0x1002, ..caps.clone() })));
        assert!(!record.holds_for(&Identity::of(&HostCaps { driver: None, ..caps.clone() })));
        // What a report may say of the device changes nothing.
        let renamed = HostCaps { name: "another name".into(), dedicated_mb: 1, software: true, ..caps.clone() };
        assert!(record.holds_for(&Identity::of(&renamed)));

        // Not known on either side is not "the same".
        let unknown = Identity::of(&HostCaps { driver: None, ..caps });
        assert!(!unknown.known() && !unknown.same(&unknown));
        let made_without = Record::parse(&Record::new(unknown.clone()).to_text()).unwrap();
        assert_eq!(made_without.identity(), &unknown, "`driver=unknown` reads back");
        assert!(!made_without.holds_for(&unknown) && !made_without.holds_for(&here()));
    }

    /// A class is shared by programs, each with containers of its own. What
    /// the title comparison finds about one container goes into that
    /// comparison's row and leaves every other row as it was; nothing is
    /// replaced without being handed back.
    #[test]
    fn a_finding_about_one_container_leaves_the_rows_of_others() {
        let mut record = Record::new(here());
        let failed = Title { passed: false, dispatches: 7, ..COMPARED };
        // Two programs of one class: two variants, two software containers.
        let (a, b) = (of(1, 0xc0), TitleKey { software: Digest([0x51; 32]), ..of(1, 0xd0) });
        assert_eq!(record.set_title(a, COMPARED), Ok(None));
        assert_eq!(record.set_times(a, TIMES), Ok(None));
        assert_eq!(record.set_title(b, failed), Ok(None));
        let row_a = TitleRow { title: Some(COMPARED), times: Some(TIMES), extra: String::new() };
        assert_eq!(record.title(&a), Some(&row_a), "the second program's finding left the first's");
        assert_eq!(record.title(&b), Some(&TitleRow { title: Some(failed), times: None, extra: String::new() }));
        // The first program translated again, its variant the same and
        // its software container another: one more row.
        let again = TitleKey { software: Digest([0x52; 32]), ..a };
        assert_eq!(record.set_times(again, TIMES), Ok(None));
        assert_eq!((record.len(), record.title(&a)), (3, Some(&row_a)));
        let found: Vec<TitleKey> = record.titles_of(&key(1), Path::Native).map(|(of, _)| *of).collect();
        assert_eq!(found, [a, again, b], "every comparison of the class's native variants");
        assert_eq!(record.titles_of(&key(1), Path::Hybrid).count(), 0);
        assert_eq!(record.titles_of(&key(2), Path::Native).count(), 0);
        assert_eq!(record.titles_of(&Key { form: Form::BothLoops, ..key(1) }, Path::Native).count(), 0);
        // And the same variant on the other path is another comparison.
        assert_eq!(record.set_title(TitleKey { path: Path::Hybrid, ..a }, failed), Ok(None));
        assert_eq!(record.titles_of(&key(1), Path::Hybrid).count(), 1);
        assert_eq!(record.title(&a), Some(&row_a));

        // A finding for the same comparison takes the earlier one's place,
        // which is handed back; the other part of the row stays.
        let slower = Times { median_us: 500, ..TIMES };
        assert_eq!(record.set_title(a, failed), Ok(Some(COMPARED)));
        assert_eq!(record.set_times(a, slower), Ok(Some(TIMES)));
        assert_eq!(record.title(&a), Some(&TitleRow { title: Some(failed), times: Some(slower), extra: String::new() }));

        // The probe's verdict is about the class and a translator: one row
        // a key and path, whatever containers the title's programs have.
        assert_eq!(record.set_probe(key(1), Path::Native, PASSED), Ok(None));
        record.set_probe_field(key(1), Path::Native, "m_max", "4096").unwrap();
        let later = Probe { passed: false, translator: Translator([0x78; 32]), container: Digest([0xb9; 32]), ..PASSED };
        let went = record.set_probe(key(1), Path::Native, later);
        assert_eq!(went, Ok(Some(ProbeRow { probe: PASSED, extra: "m_max=4096".to_owned() })), "handed back whole");
        let row = ProbeRow { probe: later, extra: String::new() };
        assert_eq!(record.probe(&key(1), Path::Native), Some(&row), "the earlier verdict's fields went with it");
        assert_eq!(record.probe(&key(1), Path::Hybrid), None);
        assert_eq!((record.len(), record.titles().count()), (5, 4), "and the title rows are as they were");
        assert_eq!(Record::parse(&record.to_text()), Ok(record.clone()));

        // A row is given up only by name, and handed back.
        assert_eq!(record.forget_title(&b).map(|row| row.title), Some(Some(failed)));
        assert_eq!(record.forget_title(&b), None);
        assert_eq!(record.forget_probe(&key(1), Path::Native), Some(row));
        assert_eq!(record.forget_probe(&key(1), Path::Native), None);
        assert_eq!((record.len(), record.title(&a).is_some(), record.title(&again).is_some()), (3, true, true));
    }

    /// What a tool finds with a verdict and the selection does not read
    /// goes into the row as a field of its own.
    #[test]
    fn a_field_the_selection_does_not_read_is_kept_with_its_row() {
        let mut record = full();
        record.set_probe_field(key(1), Path::Native, "m_max", "4096").unwrap();
        record.set_probe_field(key(1), Path::Native, "scene", "a=b").unwrap();
        record.set_probe_field(key(1), Path::Native, "m_max", "16777216").unwrap();
        let row = record.probe(&key(1), Path::Native).unwrap();
        assert_eq!(row.extra, "scene=a=b m_max=16777216", "the later value takes the earlier one's place");
        assert_eq!((row.field("m_max"), row.field("scene"), row.field("date")), (Some("16777216"), Some("a=b"), None));
        assert_eq!(row.probe, PASSED, "what the row held stays");
        record.set_title_field(of(1, 0xc0), "capture", "17").unwrap();
        let row = record.title(&of(1, 0xc0)).unwrap();
        assert_eq!((row.field("capture"), row.title, row.times), (Some("17"), Some(COMPARED), Some(TIMES)));
        let read = Record::parse(&record.to_text()).unwrap();
        assert_eq!(read, record);
        assert_eq!(read.probe(&key(1), Path::Native).unwrap().field("m_max"), Some("16777216"));

        // Not a field: a name the record reads itself in either kind of
        // row, or not one word.
        for (name, value) in [
            ("verdict", "passed"),
            ("container", "00"),
            ("translator", "00"),
            ("version", "4"),
            ("rule", "2"),
            ("software", "00"),
            ("software_spread_us", "1"),
            ("", "1"),
            ("m max", "1"),
            ("m=max", "1"),
            ("m_max", ""),
            ("m_max", "1 2"),
            ("m_max", "1\n"),
        ] {
            let refused = record.set_probe_field(key(1), Path::Native, name, value);
            assert_eq!(refused, Err(Error::Field), "{name:?}={value:?}");
            assert_eq!(record.set_title_field(of(1, 0xc0), name, value), Err(Error::Field), "{name:?}={value:?}");
        }
        assert_eq!(record, read, "a refusal changes nothing");
        // Bounded a row: the fields together, to the byte.
        let long = "9".repeat(MAX_FIELDS);
        assert_eq!(record.set_probe_field(key(1), Path::Native, "date", &long), Err(Error::Field));
        assert_eq!(record.set_title_field(of(1, 0xc0), "date", &long), Err(Error::Field));
        record.set_probe_field(key(1), Path::Hybrid, "d", &"9".repeat(MAX_FIELDS - 2)).unwrap();
        assert_eq!(record.probe(&key(1), Path::Hybrid).unwrap().extra.len(), MAX_FIELDS);
        assert_eq!(record.set_probe_field(key(1), Path::Hybrid, "e", "1"), Err(Error::Field));
        record.set_probe_field(key(1), Path::Hybrid, "d", "1").unwrap();
        record.set_probe_field(key(1), Path::Hybrid, "e", "1").unwrap();
        // A field goes with a verdict or with times: it makes no row.
        let rows = record.len();
        assert_eq!(record.set_probe_field(key(9), Path::Native, "date", "1"), Err(Error::NoRow));
        assert_eq!(record.set_probe_field(key(1), Path::Software, "date", "1"), Err(Error::NoRow));
        assert_eq!(record.set_title_field(of(1, 0xdd), "date", "1"), Err(Error::NoRow));
        assert_eq!(record.len(), rows);
        // A field read from a file, where a name can come twice.
        let twice = ProbeRow { probe: PASSED, extra: "a=1 b=2 a=3".to_owned() };
        assert_eq!((twice.field("a"), twice.field("b"), twice.field("")), (Some("3"), Some("2"), None));
    }

    #[test]
    fn the_software_path_has_no_rows() {
        let mut record = Record::new(here());
        assert_eq!(record.set_probe(key(1), Path::Software, PASSED), Err(Error::Software));
        let software = TitleKey { path: Path::Software, ..of(1, 0xc0) };
        assert_eq!(record.set_title(software, COMPARED), Err(Error::Software));
        assert_eq!(record.set_times(software, TIMES), Err(Error::Software));
        assert!(record.is_empty());
        assert_eq!((record.probe(&key(1), Path::Software), record.title(&software)), (None, None));
    }

    /// The bound: rows are refused past it when set, a text past it when
    /// read; a record full of the longest rows this build writes, each
    /// with all the fields a row may carry, is within the bound on text;
    /// in memory it is under half a megabyte without those fields.
    #[test]
    fn a_record_is_bounded_in_rows_and_in_text() {
        let mut record = Record::new(here());
        let class = |i: usize| Class(std::array::from_fn(|at| (i >> (8 * (at % 2))) as u8));
        let longest = |i: usize| TitleKey {
            key: Key {
                class: class(i),
                form: Form::BothLoops,
                source: Source::Descriptions,
                tables: Tables::INSTANCE.with(Tables::NODE).with(Tables::ID),
            },
            path: Path::Native,
            container: Digest([1; 32]),
            software: Digest([5; 32]),
        };
        let title = Title { passed: true, rule: u32::MAX, dispatches: u64::MAX, frames: u64::MAX };
        let times = Times {
            median_us: u64::MAX,
            spread_us: u64::MAX,
            software_median_us: u64::MAX,
            software_spread_us: u64::MAX,
        };
        for i in 0..MAX_ROWS {
            assert_eq!(record.set_title(longest(i), title), Ok(None));
            assert_eq!(record.set_times(longest(i), times), Ok(None));
        }
        assert_eq!(record.len(), MAX_ROWS);
        assert_eq!(record.set_title(longest(MAX_ROWS), title), Err(Error::Full));
        assert_eq!(record.set_times(longest(MAX_ROWS), times), Err(Error::Full));
        assert_eq!(record.set_probe(key(0), Path::Native, PASSED), Err(Error::Full));
        assert_eq!(record.set_probe(key(0), Path::Software, PASSED), Err(Error::Software));
        assert_eq!(record.len(), MAX_ROWS, "a refusal adds nothing");
        assert_eq!(record.set_title(longest(7), title), Ok(Some(title)), "a row it has is written again");

        let text = record.to_text();
        assert!(text.len() <= MAX_BYTES, "a full record of the longest rows is {} bytes", text.len());
        assert_eq!(Record::parse(&text).map(|r| r.len()), Ok(MAX_ROWS));
        assert!(record.bytes() < 512 << 10, "{} bytes", record.bytes());
        assert!(record.bytes() >= (MAX_ROWS * size_of::<TitleRow>()) as u64);
        // A row in memory, as docs/memory.md (R6) gives it: about 150
        // bytes for the probe's, about 200 for a title comparison's.
        let (probe_row, title_row) = (size_of::<((Key, Path), ProbeRow)>(), size_of::<(TitleKey, TitleRow)>());
        assert!((128..=160).contains(&probe_row) && (176..=208).contains(&title_row), "{probe_row} {title_row}");
        let a_line_more = format!("{text}one more line\n");
        assert!(a_line_more.len() <= MAX_BYTES, "refused for its lines, not its bytes");
        assert_eq!(Record::parse(&a_line_more), Err(Error::TooLarge));
        let padded = format!("{}{}", full().to_text(), "#".repeat(MAX_BYTES));
        assert_eq!(Record::parse(&padded), Err(Error::TooLarge));

        // With every row carrying all the text its fields may take, the
        // record is still one this build can write and read.
        for i in 0..MAX_ROWS {
            record.set_title_field(longest(i), "f", &"9".repeat(MAX_FIELDS - 2)).unwrap();
        }
        let text = record.to_text();
        assert!(text.len() <= MAX_BYTES, "with all its fields a full record is {} bytes", text.len());
        assert_eq!(Record::parse(&text), Ok(record.clone()));
        assert!(record.bytes() < 1536 << 10, "{} bytes", record.bytes());
        // A probe row is the shorter kind, so the same holds of any mix.
        let mut probe = Record::new(here());
        let version = Probe { rule: u32::MAX, version: u32::MAX, ..PASSED };
        probe.set_probe(longest(0).key, Path::Native, version).unwrap();
        assert!(probe.to_text().lines().nth(2).unwrap().len() < text.lines().nth(2).unwrap().len() - MAX_FIELDS);

        // Giving a row up makes room for one, of either kind.
        assert!(record.forget_title(&longest(8)).is_some());
        assert_eq!(record.set_probe(key(0), Path::Native, PASSED), Ok(None));
        assert_eq!(record.set_probe(key(0), Path::Hybrid, PASSED), Err(Error::Full));

        // Lines kept unread take the place of rows.
        let mut unread = Record::parse(&format!("{}later a=b\n", full().to_text())).unwrap();
        assert_eq!((unread.len(), unread.unread().len()), (4, 1));
        for i in 0..MAX_ROWS - 5 {
            let key = Key { class: class(i + 1000), ..key(0) };
            assert_eq!(unread.set_probe(key, Path::Hybrid, PASSED), Ok(None));
        }
        let another = Key { class: class(60_000), ..key(0) };
        assert_eq!(unread.set_probe(another, Path::Hybrid, PASSED), Err(Error::Full));
        assert_eq!(unread.set_title(of(3, 0xc0), COMPARED), Err(Error::Full));
        assert_eq!(Record::parse(&unread.to_text()).map(|r| r.len()), Ok(MAX_ROWS - 1));
    }

    #[test]
    fn the_file_is_the_devices_in_the_records_folder() {
        let dir = FsPath::new("cache").join(FOLDER);
        assert_eq!(dir, FsPath::new("cache").join("rt-record"));
        let (a, b) = (file(&dir, &here()).unwrap(), file(&dir, &device([32, 0, 15, 7001])).unwrap());
        assert_ne!(a, b, "a driver's record is a file of its own");
        assert_eq!(a.parent(), Some(dir.as_path()));
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(name.len() == 20 && name.ends_with(".txt") && name[..16].bytes().all(|b| b.is_ascii_hexdigit()), "{name}");
        assert_eq!(file(&dir, &here()), Some(a), "the same device and driver, the same file");
        let no_driver = Identity::of(&HostCaps { vendor: 0x10de, device: 0x2c05, ..Default::default() });
        assert_eq!(file(&dir, &no_driver), None);
    }

    #[test]
    fn a_record_is_kept_on_disk_for_its_device_and_driver_only() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join(FOLDER);
        assert_eq!(load(&dir, &here()), Ok(None), "no folder yet");

        let record = full();
        let path = save(&dir, &record).unwrap();
        assert_eq!(Some(path.clone()), file(&dir, &here()));
        assert_eq!(load(&dir, &here()), Ok(Some(record.clone())));
        let files = || std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files(), 1, "nothing is left beside it");

        // After a driver update there is no record, and the old one stays
        // for the old driver.
        let updated = device([32, 0, 15, 7100]);
        assert_eq!(load(&dir, &updated), Ok(None));
        let mut second = Record::new(updated.clone());
        second.set_probe(key(9), Path::Hybrid, PASSED).unwrap();
        save(&dir, &second).unwrap();
        assert_eq!(load(&dir, &updated), Ok(Some(second.clone())));
        assert_eq!(load(&dir, &here()), Ok(Some(record.clone())));
        assert_eq!(files(), 2);

        // A file under this device's name that says it is another's (a
        // copy, a rename) counts as absent.
        std::fs::copy(file(&dir, &updated).unwrap(), &path).unwrap();
        assert_eq!(load(&dir, &here()), Ok(None));

        // A save replaces the file whole.
        save(&dir, &record).unwrap();
        assert_eq!(load(&dir, &here()), Ok(Some(record)));
        assert_eq!(files(), 2);

        let no_driver = Identity::of(&HostCaps { vendor: 0x10de, device: 0x2c05, ..Default::default() });
        assert_eq!(load(&dir, &no_driver), Ok(None));
        assert_eq!(save(&dir, &Record::new(no_driver)), Err(Error::NoDriver));
        assert_eq!(files(), 2);
    }

    #[test]
    fn a_file_that_is_not_read_is_an_error_and_gives_no_record() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join(FOLDER);
        std::fs::create_dir_all(&dir).unwrap();
        let path = file(&dir, &here()).unwrap();

        std::fs::write(&path, full().to_text().replacen("version=1", "version=9", 1)).unwrap();
        assert_eq!(load(&dir, &here()), Err(Error::Newer { found: 9 }));
        assert!(Held::open(&dir, &here()).is_none(), "said, and no record");

        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(load(&dir, &here()), Err(Error::NotARecord));
        std::fs::write(&path, "").unwrap();
        assert_eq!(load(&dir, &here()), Err(Error::NotARecord));

        std::fs::write(&path, vec![b'#'; MAX_BYTES + 1]).unwrap();
        assert_eq!(load(&dir, &here()), Err(Error::TooLarge));

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let io = load(&dir, &here());
        assert!(matches!(&io, Err(Error::Io(said)) if said.contains(&*path.to_string_lossy())), "{io:?}");
        let io = save(&dir, &full());
        assert!(matches!(io, Err(Error::Io(_))), "{io:?}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "the partial file is not left behind");
    }

    /// A file is read through the bound: however much there is to read,
    /// and whatever a file system says the size is, one byte past the
    /// bound is the most that is asked for.
    #[test]
    fn a_file_is_read_through_the_bound_and_no_further() {
        /// Text without end, which counts what it was asked for and fails
        /// a reader that goes on well past the bound.
        struct Endless {
            asked: usize,
        }
        impl Read for Endless {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.asked > 4 * MAX_BYTES {
                    return Err(std::io::Error::other("read on past the bound"));
                }
                buffer.fill(b'#');
                self.asked += buffer.len();
                Ok(buffer.len())
            }
        }
        let mut endless = Endless { asked: 0 };
        assert_eq!(read_text(&mut endless), Err(Error::TooLarge));
        assert_eq!(endless.asked, MAX_BYTES + 1);

        assert_eq!(read_text(&vec![b'#'; MAX_BYTES][..]).map(|text| text.len()), Ok(MAX_BYTES));
        assert_eq!(read_text(&vec![b'#'; MAX_BYTES + 1][..]), Err(Error::TooLarge));
        assert_eq!(read_text(&[0xffu8, 0xfe][..]), Err(Error::NotARecord));
        assert_eq!(read_text(&b""[..]), Ok(String::new()));
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("the disk went"))
            }
        }
        assert_eq!(read_text(Broken), Err(Error::Io("the disk went".to_owned())));
    }

    /// A record over the bound is not written, and what is on disk stays.
    /// That reaches a text read at the bound: written back it can be a
    /// byte longer (here the end of its last line), and is then refused.
    #[test]
    fn a_record_over_the_bound_is_not_saved_and_the_file_stays() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join(FOLDER);
        std::fs::create_dir_all(&dir).unwrap();
        let path = file(&dir, &here()).unwrap();
        let mut text = full().to_text();
        text.push('#');
        text.push_str(&"x".repeat(MAX_BYTES - text.len()));
        assert!(text.len() == MAX_BYTES && !text.ends_with('\n'));
        std::fs::write(&path, &text).unwrap();

        let mut record = load(&dir, &here()).unwrap().expect("a text at the bound is read");
        assert_eq!((record.len(), record.unread().len()), (4, 1));
        assert_eq!(record.to_text().len(), MAX_BYTES + 1, "the last line gets its end");
        assert_eq!(save(&dir, &record), Err(Error::TooLarge));
        // And a record that grew past the bound in memory.
        record.set_probe(key(7), Path::Native, PASSED).unwrap();
        assert_eq!(save(&dir, &record), Err(Error::TooLarge));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "the file stays as it was");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "and nothing is left beside it");

        // A byte more on disk is not read.
        std::fs::write(&path, format!("{text}\n")).unwrap();
        assert_eq!(load(&dir, &here()), Err(Error::TooLarge));
        // Without the line that filled it the record is saved again.
        let record = Record::parse(&full().to_text()).unwrap();
        assert_eq!(save(&dir, &record), Ok(path));
        assert_eq!(load(&dir, &here()), Ok(Some(full())));
    }

    /// A record a run keeps is counted under its owner for as long as it
    /// is kept. (The gauge is the process's: nothing else in these tests
    /// adds to it, and a read of it is compared with what this test put
    /// there.)
    #[test]
    fn a_record_kept_for_a_run_is_counted_while_it_is_kept() {
        let gauge = crate::account::gauge(crate::account::Owner::RtRecord);
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join(FOLDER);
        assert!(Held::open(&dir, &here()).is_none(), "nothing there: nothing kept");
        assert_eq!(gauge.now(), 0);
        save(&dir, &full()).unwrap();
        let held = Held::open(&dir, &here()).unwrap();
        assert_eq!(held.record(), &full());
        assert_eq!(gauge.now(), full().bytes());
        assert!(full().bytes() > (2 * size_of::<ProbeRow>() + 2 * size_of::<TitleRow>()) as u64);
        drop(held);
        assert_eq!((gauge.now(), gauge.underflows()), (0, 0));
        assert!(Held::open(&dir, &device([1, 2, 3, 4])).is_none(), "another driver: nothing kept");
        assert_eq!(gauge.now(), 0);
    }

    #[test]
    fn the_time_gate_is_strict_and_takes_the_larger_spread() {
        let times = |median_us, spread_us, software_median_us, software_spread_us| Times {
            median_us,
            spread_us,
            software_median_us,
            software_spread_us,
        };
        assert!(times(400, 30, 900, 45).faster());
        assert!(times(854, 30, 900, 45).faster(), "better by 46, more than 45");
        assert!(!times(855, 30, 900, 45).faster(), "better by exactly the spread");
        assert!(!times(855, 45, 900, 30).faster(), "whichever side the larger spread is on");
        assert!(times(854, 45, 900, 30).faster());
        assert!(!times(900, 0, 900, 0).faster(), "equal is not faster");
        assert!(times(899, 0, 900, 0).faster());
        assert!(!times(1200, 0, 900, 0).faster());
        assert!(!times(u64::MAX, 1, u64::MAX, 0).faster(), "a sum past the type is not faster");
        assert!(!times(0, 0, 0, 0).faster());
    }

    /// Two paths were timed in two runs. One is the faster of the two only
    /// where the runs can be set against each other (software took the
    /// same in both, within its spread) and the difference is more than
    /// the paths' spread.
    #[test]
    fn one_path_is_faster_than_another_only_between_comparable_runs_and_past_the_spread() {
        let times = |median_us, spread_us, software_median_us, software_spread_us| Times {
            median_us,
            spread_us,
            software_median_us,
            software_spread_us,
        };
        let native = times(400, 30, 900, 45);
        assert!(times(300, 30, 900, 45).faster_than(&native));
        assert!(times(369, 30, 900, 45).faster_than(&native), "better by 31, more than 30");
        assert!(!times(370, 30, 900, 45).faster_than(&native), "better by exactly the spread");
        assert!(!times(369, 31, 900, 45).faster_than(&native), "the larger spread, on this side");
        assert!(!times(369, 30, 900, 45).faster_than(&times(400, 31, 900, 45)), "or on the other");
        assert!(!native.faster_than(&native) && !times(500, 30, 900, 45).faster_than(&native));
        assert!(times(399, 0, 900, 0).faster_than(&times(400, 0, 900, 0)));
        // A hair's difference inside a wide spread decides nothing.
        assert!(!times(399, 150, 900, 0).faster_than(&times(400, 150, 900, 0)));

        // Software's medians in the two runs: apart by its larger spread
        // at most, on either side.
        assert!(times(300, 30, 945, 45).faster_than(&native), "apart by exactly the spread");
        assert!(!times(300, 30, 946, 45).faster_than(&native));
        assert!(times(300, 30, 855, 10).faster_than(&native), "the larger of software's two spreads");
        assert!(!times(300, 30, 854, 10).faster_than(&native));
        assert!(!times(300, 30, 946, 10).faster_than(&times(400, 30, 900, 10)));
        // Runs of two different scenes: 390 against a software of 420 is
        // not faster than 400 against a software of 2000.
        assert!(!times(390, 0, 420, 0).faster_than(&times(400, 0, 2000, 0)));
        assert!(!times(u64::MAX, 1, 0, 0).faster_than(&times(u64::MAX, 0, 0, 0)), "a sum past the type");
    }
}
