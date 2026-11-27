//! The console's wave sizes on this host (docs/gpu-compatibility.md, G2):
//! which translated compute programs depend on them and why, counted, and
//! the switch that makes such programs carry the console's wave themselves.
//!
//! The console runs compute waves of 64 lanes (some programs: 32). A
//! program that reads other lanes or reads a lane mask as a number is
//! translated to declare that size (`victory_shader::emit::WaveUse`), and a
//! device whose waves cannot have it makes no pipeline: the program's
//! dispatches are left out. This module counts, per program and once each,
//! what was asked and why ([`note`]), so one run tells how many programs a
//! device loses and to which kind of code; the counts are logged when they
//! change and are columns of the performance log ([`COLUMNS`]). Compute
//! programs that do not translate at all are counted too
//! ([`note_untranslated`]), by their console wave and thread group: a
//! program the per-lane form refuses declares nothing, so without them a
//! run could show few lane-dependent programs while the ones that matter
//! fail earlier.
//!
//! `VICTORY_WAVE_EMULATION` ([`Emulation`]) asks the translator for the
//! other form, where a thread group of exactly one console wave is that
//! wave on any device. It is off unless set: it agrees with a model of the
//! console's wave on the translator's own test programs, and has not been
//! run on a GPU or seen by `dxil.dll` (Microsoft's validator outside
//! Windows accepts those tests' modules: `scripts/validate-dxil.sh`). What
//! is chosen follows the device's lane range, never its maker.
//!
//! The counting and the switch's reading are plain data and logic, tested
//! on any host; the calls into them are in Windows-only code.
//!
//! Memory: the census keeps at most [`MAX_KINDS`] kinds of program for the
//! run (a few kilobytes), counted with the shaders
//! (`crate::account::Owner::Shaders`, docs/memory.md C3).

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use victory_shader::emit::{WaveUse, emulate, wave_need, wave_needs_text};

use crate::caps::{CONSOLE_WAVES, HostCaps, runs_waves_of};

/// `VICTORY_WAVE_EMULATION`: which lane-dependent compute programs are
/// translated to carry the console's wave themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Emulation {
    /// None (unset, empty, `0`, `off`): each declares the console's wave
    /// size, as before the switch existed.
    Off,
    /// `1` or `auto`: those of a wave size the device cannot run.
    Unsupported,
    /// `32`, `64` or `all`: those of these sizes whatever the device runs
    /// (bits of `victory_shader::emit::emulate`), to compare the two forms
    /// on a device that runs both, or to translate ahead of a run.
    Sizes(u8),
}

/// The switch's value read; `None` for a value that is none of the above.
pub(crate) fn parse(value: Option<&str>) -> Option<Emulation> {
    Some(match value.map(str::trim).unwrap_or("") {
        "" | "0" | "off" => Emulation::Off,
        "1" | "auto" => Emulation::Unsupported,
        "32" => Emulation::Sizes(emulate::WAVE32),
        "64" => Emulation::Sizes(emulate::WAVE64),
        "all" => Emulation::Sizes(emulate::WAVE32 | emulate::WAVE64),
        _ => return None,
    })
}

/// The wave sizes to emulate under `mode` on a device with `caps` (`None`:
/// no device): bits of `victory_shader::emit::emulate`. Decided by the
/// device's lane range alone.
pub(crate) fn emulated_sizes(mode: Emulation, caps: Option<&HostCaps>) -> u8 {
    match mode {
        Emulation::Off => 0,
        Emulation::Sizes(bits) => bits,
        Emulation::Unsupported => caps.map_or(0, |caps| {
            let missing = CONSOLE_WAVES.into_iter().filter(|&lanes| !runs_waves_of(caps, lanes));
            missing.fold(0, |bits, lanes| bits | emulate::bit(lanes))
        }),
    }
}

/// The wave sizes this run's translations emulate (the switch, resolved
/// once, with the device where the switch asks about it). Part of the
/// translation: the shader cache keys on it.
#[cfg(windows)]
pub(crate) fn emulated() -> u8 {
    static SIZES: OnceLock<u8> = OnceLock::new();
    *SIZES.get_or_init(|| {
        let value = std::env::var("VICTORY_WAVE_EMULATION").ok();
        let mode = parse(value.as_deref()).unwrap_or_else(|| {
            tracing::warn!(?value, "VICTORY_WAVE_EMULATION is none of 0, 1, auto, 32, 64, all: taken as 0");
            Emulation::Off
        });
        // Only `auto` needs the device, which it then makes if no one has.
        let caps = (mode == Emulation::Unsupported).then(|| crate::host_caps(false)).flatten();
        let sizes = emulated_sizes(mode, caps.as_ref());
        if mode != Emulation::Off {
            tracing::warn!(
                ?mode,
                wave32 = sizes & emulate::WAVE32 != 0,
                wave64 = sizes & emulate::WAVE64 != 0,
                "VICTORY_WAVE_EMULATION: lane-dependent compute programs of these wave sizes carry the \
                 console's wave in their thread group (experimental: tested against a model, not on a GPU)"
            );
        }
        sizes
    })
}

/// The device's answer about waves, once a device exists: wave intrinsics,
/// and the lane range.
static DEVICE: OnceLock<(bool, (u32, u32))> = OnceLock::new();

/// Records the device's waves, for [`note`], and settles the programs
/// counted before it answered.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn set_device(caps: &HostCaps) {
    let device = (caps.wave_ops, caps.wave_lanes);
    if DEVICE.set(device).is_ok() {
        for line in census(|c| c.resolve(device)) {
            log(&line);
        }
    }
}

/// Whether the device runs waves of `lanes` lanes; `None` before a device
/// has answered.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn device_runs(lanes: u32) -> Option<bool> {
    DEVICE.get().map(|&device| runs(device, lanes))
}

fn runs((ops, (min, max)): (bool, (u32, u32)), lanes: u32) -> bool {
    ops && (min..=max).contains(&lanes)
}

/// What a lane-dependent compute program got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Outcome {
    /// It declares a wave size this device runs.
    HostWaves,
    /// Its thread group carries the console's wave.
    Emulated,
    /// It declares a wave size this device does not run: no pipeline.
    Refused,
    /// It declares a wave size and no device has been asked yet.
    Unknown,
    /// It does not translate at all (whether it depends on the wave is not
    /// known): no pipeline.
    Untranslated,
}

/// A compute thread group against the console wave it runs in: only a
/// group of exactly one wave can carry the wave itself.
pub(crate) fn group(lanes: u32, threads: [u32; 3]) -> &'static str {
    let count = threads.iter().fold(1u64, |count, &n| count.saturating_mul(u64::from(n)));
    match count.cmp(&u64::from(lanes)) {
        std::cmp::Ordering::Equal => "one console wave",
        std::cmp::Ordering::Greater => "several console waves",
        std::cmp::Ordering::Less => "less than a console wave",
    }
}

/// One kind of program in the census.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Kind {
    outcome: Outcome,
    /// The wave size declared (0 for an emulated wave).
    size: u32,
    /// `victory_shader::emit::wave_need` bits.
    needs: u32,
    /// Why the program was not emulated although that was asked for. Of a
    /// program that does not translate: its thread group and whether its
    /// emulated form was asked for.
    refusal: String,
    /// Why the program has no per-lane form, where the emulated wave is
    /// its only translation.
    per_lane: String,
}

impl Kind {
    /// The bytes one kind takes in the census, about: the entry and its
    /// texts.
    fn bytes(&self) -> u64 {
        (size_of::<Kind>() + size_of::<u64>() + self.refusal.capacity() + self.per_lane.capacity()) as u64
    }
}

/// Distinct kinds kept; programs of further kinds are counted together.
/// (Sizes, reasons and refusals are few: this is a bound, not an estimate.)
const MAX_KINDS: usize = 64;

/// The performance log's columns, all counts of programs since the start:
/// by outcome, then the refused ones by each reason (a program with several
/// reasons counts under each).
pub(crate) const COLUMNS: usize = OUTCOMES + wave_need::NAMES.len();

/// The columns by outcome, before the ones by reason.
const OUTCOMES: usize = 4;

/// The columns' names, in order.
pub(crate) fn header() -> String {
    let reasons = [
        "swizzle",
        "permute",
        "count_below",
        "lane_read",
        "dpp",
        "mask_read",
        "mask_made",
        "shared_memory",
        "section",
        "lane_write",
        "restored",
    ];
    let mut names = vec![
        "wave_programs_host".to_owned(),
        "wave_programs_emulated".to_owned(),
        "wave_programs_refused".to_owned(),
        "wave_programs_untranslated".to_owned(),
    ];
    names.extend(reasons.map(|r| format!("wave_refused_{r}")));
    debug_assert_eq!(names.len(), COLUMNS);
    names.join(",")
}

/// Programs by what they ask of the host's waves.
#[derive(Debug, Default)]
pub(crate) struct Census {
    kinds: BTreeMap<Kind, u64>,
    /// Programs of kinds past [`MAX_KINDS`].
    other: u64,
    columns: [u64; COLUMNS],
    changed: bool,
}

impl Census {
    /// Counts one translated compute program. `device`: wave intrinsics
    /// and the lane range, if a device has answered. Returns a line for the
    /// log when the program's kind is new.
    pub(crate) fn note(&mut self, wave: &WaveUse, device: Option<(bool, (u32, u32))>) -> Option<String> {
        if wave.size == 0 && !wave.emulated {
            // It asks nothing of the host's waves.
            return None;
        }
        let outcome = match device {
            _ if wave.emulated => Outcome::Emulated,
            None => Outcome::Unknown,
            Some(device) if runs(device, wave.size) => Outcome::HostWaves,
            Some(_) => Outcome::Refused,
        };
        let kind = Kind {
            outcome,
            size: wave.size,
            needs: wave.needs,
            refusal: wave.refusal.clone(),
            per_lane: wave.per_lane.clone(),
        };
        self.count(kind, 1, device)
    }

    /// Counts one compute program that does not translate: its console
    /// wave's `lanes`, its thread group, and whether its emulated form was
    /// asked for (and so refused too). Returns a line for the log when the
    /// kind is new.
    pub(crate) fn note_untranslated(&mut self, lanes: u32, threads: [u32; 3], asked: bool) -> Option<String> {
        let group = group(lanes, threads);
        let form = match (asked, group) {
            (true, "one console wave") => "the emulated form was asked for and is refused too",
            (true, _) => "the emulated form was asked for, which such a group cannot have",
            (false, "one console wave") => {
                "the emulated form was not asked for (`victory translate` says what it comes to)"
            }
            (false, _) => "such a group cannot have the emulated form",
        };
        let kind = Kind {
            outcome: Outcome::Untranslated,
            size: lanes,
            needs: 0,
            refusal: format!("thread group of {group}; {form}"),
            per_lane: String::new(),
        };
        self.count(kind, 1, None)
    }

    /// Adds `programs` of `kind`; the kind's line if it is new.
    fn count(&mut self, kind: Kind, programs: u64, device: Option<(bool, (u32, u32))>) -> Option<String> {
        self.changed = true;
        match kind.outcome {
            Outcome::HostWaves => self.columns[0] += programs,
            Outcome::Emulated => self.columns[1] += programs,
            Outcome::Refused => {
                self.columns[2] += programs;
                for bit in 0..wave_need::NAMES.len() {
                    self.columns[OUTCOMES + bit] += programs * u64::from(kind.needs >> bit & 1);
                }
            }
            Outcome::Untranslated => self.columns[3] += programs,
            Outcome::Unknown => {}
        }
        if let Some(count) = self.kinds.get_mut(&kind) {
            *count += programs;
            return None;
        }
        if self.kinds.len() >= MAX_KINDS {
            self.other += programs;
            return None;
        }
        let line = describe(&kind, device);
        self.kinds.insert(kind, programs);
        Some(line)
    }

    /// The device has answered: the programs counted before it did are
    /// counted as what they turn out to be. Returns the lines of the kinds
    /// that are new.
    pub(crate) fn resolve(&mut self, device: (bool, (u32, u32))) -> Vec<String> {
        let unknown: Vec<Kind> =
            self.kinds.keys().filter(|k| k.outcome == Outcome::Unknown).cloned().collect();
        let mut lines = Vec::new();
        for kind in unknown {
            let programs = self.kinds.remove(&kind).unwrap_or(0);
            let outcome = if runs(device, kind.size) { Outcome::HostWaves } else { Outcome::Refused };
            lines.extend(self.count(Kind { outcome, ..kind }, programs, Some(device)));
        }
        lines
    }

    /// The columns of [`header`].
    pub(crate) fn columns(&self) -> [u64; COLUMNS] {
        self.columns
    }

    /// The bytes the kinds kept take, about (at most [`MAX_KINDS`] of
    /// them).
    pub(crate) fn bytes(&self) -> u64 {
        self.kinds.keys().map(Kind::bytes).sum()
    }

    /// Every kind with its count, one a line, if a program was counted
    /// since the last call.
    pub(crate) fn take_changed(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.changed) {
            return None;
        }
        let mut lines: Vec<String> =
            self.kinds.iter().map(|(kind, count)| format!("{count} x {}", describe(kind, None))).collect();
        if self.other > 0 {
            lines.push(format!("{} x of kinds past the {MAX_KINDS} kept apart", self.other));
        }
        Some(lines.join("\n"))
    }
}

/// A kind in words.
fn describe(kind: &Kind, device: Option<(bool, (u32, u32))>) -> String {
    let why = wave_needs_text(kind.needs);
    let waves = match device {
        Some((true, (min, max))) => format!(" (device waves: {min} to {max} lanes)"),
        Some((false, _)) => " (the device has no wave intrinsics)".to_owned(),
        None => String::new(),
    };
    let asked = if kind.refusal.is_empty() {
        String::new()
    } else {
        format!("; not emulated: {}", kind.refusal)
    };
    match kind.outcome {
        Outcome::Emulated if !kind.per_lane.is_empty() => format!(
            "compute programs carrying the console's wave themselves (emulated): {why}; they have no per-lane \
             form: {}",
            kind.per_lane
        ),
        Outcome::Emulated => format!("compute programs carrying the console's wave themselves (emulated): {why}"),
        Outcome::Untranslated => format!(
            "compute programs of {}-lane console waves that do not translate (whether they depend on the wave \
             is not known): NO HOST PIPELINE, their dispatches are left out: {}",
            kind.size, kind.refusal
        ),
        Outcome::HostWaves => {
            format!("compute programs declaring {}-lane waves, which the device runs{waves}: {why}{asked}", kind.size)
        }
        Outcome::Refused => format!(
            "compute programs declaring {}-lane waves, which the device does not run{waves}: NO HOST PIPELINE, \
             their dispatches are left out: {why}{asked}",
            kind.size
        ),
        Outcome::Unknown => {
            format!("compute programs declaring {}-lane waves, translated before a device was asked: {why}{asked}", kind.size)
        }
    }
}

/// The run's census, and its bytes as counted with the shaders: kept for
/// the process, bounded by [`MAX_KINDS`].
static CENSUS: Mutex<Option<(Census, crate::account::Held)>> = Mutex::new(None);

fn census<R>(f: impl FnOnce(&mut Census) -> R) -> R {
    let mut guard = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
    let (census, held) = guard
        .get_or_insert_with(|| (Census::default(), crate::account::Held::new(crate::account::Owner::Shaders, 0)));
    let r = f(census);
    held.resize(census.bytes());
    r
}

/// Counts a compute program just translated (once per program: the caller
/// translates each container once) and logs its kind the first time.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn note(wave: &WaveUse) {
    let device = DEVICE.get().copied();
    if let Some(line) = census(|c| c.note(wave, device)) {
        log(&line);
    }
}

/// Counts a compute program that does not translate ([`Outcome::Untranslated`])
/// and logs its kind the first time.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn note_untranslated(lanes: u32, threads: [u32; 3], asked: bool) {
    if let Some(line) = census(|c| c.note_untranslated(lanes, threads, asked)) {
        log(&line);
    }
}

/// A kind's line, the first time it is counted.
fn log(line: &str) {
    if line.contains("NO HOST PIPELINE") {
        tracing::warn!("wave sizes: {line} (logged once; counts follow)");
    } else {
        tracing::info!("wave sizes: {line} (logged once; counts follow)");
    }
}

/// [`Census::columns`] of the run so far.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn columns() -> [u64; COLUMNS] {
    census(|c| c.columns())
}

/// [`Census::take_changed`] of the run.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn take_changed() -> Option<String> {
    census(|c| c.take_changed())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(lanes: (u32, u32)) -> HostCaps {
        HostCaps { wave_ops: true, wave_lanes: lanes, ..Default::default() }
    }

    fn asks(size: u32, needs: u32) -> WaveUse {
        WaveUse { size, needs, ..Default::default() }
    }

    #[test]
    fn the_switch_is_off_unless_it_says_otherwise() {
        for off in [None, Some(""), Some("0"), Some(" off ")] {
            assert_eq!(parse(off), Some(Emulation::Off), "{off:?}");
        }
        assert_eq!(parse(Some("1")), Some(Emulation::Unsupported));
        assert_eq!(parse(Some("auto")), Some(Emulation::Unsupported));
        assert_eq!(parse(Some("64")), Some(Emulation::Sizes(emulate::WAVE64)));
        assert_eq!(parse(Some("32")), Some(Emulation::Sizes(emulate::WAVE32)));
        assert_eq!(parse(Some("all")), Some(Emulation::Sizes(3)));
        for unknown in ["yes", "16", "64,32", "nvidia"] {
            assert_eq!(parse(Some(unknown)), None, "{unknown}");
        }
    }

    /// What is emulated follows the lanes the device reports and nothing
    /// else about it.
    #[test]
    fn auto_emulates_the_wave_sizes_the_device_cannot_run() {
        let auto = Emulation::Unsupported;
        assert_eq!(emulated_sizes(auto, Some(&caps((32, 32)))), emulate::WAVE64);
        assert_eq!(emulated_sizes(auto, Some(&caps((32, 64)))), 0);
        assert_eq!(emulated_sizes(auto, Some(&caps((4, 16)))), emulate::WAVE32 | emulate::WAVE64);
        assert_eq!(emulated_sizes(auto, Some(&caps((64, 64)))), emulate::WAVE32);
        assert_eq!(emulated_sizes(auto, Some(&caps((8, 128)))), 0);
        // No wave intrinsics: no wave of any size.
        let none = HostCaps { wave_ops: false, wave_lanes: (32, 64), ..Default::default() };
        assert_eq!(emulated_sizes(auto, Some(&none)), 3);
        // No device: nothing is known to be missing.
        assert_eq!(emulated_sizes(auto, None), 0);
        // The same lanes under any maker's name.
        for vendor in [0x10de, 0x1002, 0x8086, 0] {
            let named = HostCaps { vendor, name: format!("{vendor:#x}"), ..caps((32, 32)) };
            assert_eq!(emulated_sizes(auto, Some(&named)), emulate::WAVE64);
        }
        // Named sizes are taken as named; off is off.
        assert_eq!(emulated_sizes(Emulation::Sizes(3), Some(&caps((32, 64)))), 3);
        assert_eq!(emulated_sizes(Emulation::Sizes(2), None), 2);
        assert_eq!(emulated_sizes(Emulation::Off, Some(&caps((32, 32)))), 0);
    }

    #[test]
    fn programs_are_counted_by_what_they_ask_and_what_they_get() {
        let mut census = Census::default();
        let narrow = Some((true, (32, 32)));
        // Nothing asked: not counted.
        assert_eq!(census.note(&WaveUse::default(), narrow), None);
        assert_eq!(census.take_changed(), None);
        let masks = wave_need::MASK_READ | wave_need::MASK_MADE;
        // The first of a kind gives its line; the next of it only counts.
        let first = census.note(&asks(64, masks), narrow).unwrap();
        assert_eq!(
            first,
            "compute programs declaring 64-lane waves, which the device does not run (device waves: 32 to 32 \
             lanes): NO HOST PIPELINE, their dispatches are left out: mask read as a number + mask made from a \
             number"
        );
        assert_eq!(census.note(&asks(64, masks), narrow), None);
        assert!(census.note(&asks(64, wave_need::DPP), narrow).is_some());
        assert!(census.note(&asks(32, wave_need::DPP), narrow).unwrap().contains("which the device runs"));
        let emulated = WaveUse { needs: masks, emulated: true, ..Default::default() };
        assert_eq!(
            census.note(&emulated, narrow).unwrap(),
            "compute programs carrying the console's wave themselves (emulated): mask read as a number + mask made \
             from a number"
        );
        let several = WaveUse {
            refusal: "an emulated wave in a thread group of several waves".into(),
            ..asks(64, wave_need::SECTION)
        };
        assert!(census.note(&several, narrow).unwrap().ends_with(
            "whole-wave section; not emulated: an emulated wave in a thread group of several waves"
        ));
        // host, emulated, refused, untranslated; then the refused by
        // reason: two with both mask reasons, one with DPP, one whole-wave
        // section.
        assert_eq!(census.columns(), [1, 1, 4, 0, 0, 0, 0, 0, 1, 2, 2, 0, 1, 0, 0]);
        let text = census.take_changed().unwrap();
        assert_eq!(text.lines().count(), 5);
        assert!(text.lines().any(|l| l.starts_with("2 x compute programs declaring 64-lane waves")), "{text}");
        assert_eq!(census.take_changed(), None, "nothing counted since");
        // Before a device answers, a program's fate is not known.
        let mut early = Census::default();
        assert!(early.note(&asks(64, masks), None).unwrap().contains("before a device was asked"));
        assert_eq!(early.columns(), [0; COLUMNS]);
        // A device without wave intrinsics runs no declared size.
        assert!(early.note(&asks(32, masks), Some((false, (32, 64)))).unwrap().contains("no wave intrinsics"));
        assert_eq!(early.columns()[2], 1);
    }

    #[test]
    fn programs_counted_before_the_device_answered_are_settled_when_it_does() {
        let mut census = Census::default();
        let masks = wave_need::MASK_READ;
        for _ in 0..3 {
            census.note(&asks(64, masks), None);
        }
        census.note(&asks(32, masks), None);
        assert_eq!(census.columns(), [0; COLUMNS]);
        let lines = census.resolve((true, (32, 32)));
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().any(|l| l.contains("64-lane waves, which the device does not run")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("32-lane waves, which the device runs")), "{lines:?}");
        // One program for the device's waves, three refused for theirs.
        assert_eq!(census.columns()[..3], [1, 0, 3]);
        assert_eq!(census.columns()[OUTCOMES + 5], 3, "by reason: masks read as numbers");
        assert!(!census.take_changed().unwrap().contains("before a device was asked"));
        assert!(census.resolve((true, (32, 32))).is_empty(), "nothing is left to settle");
    }

    #[test]
    fn the_census_is_bounded() {
        let mut census = Census::default();
        for needs in 1..=200 {
            census.note(&asks(64, needs), Some((true, (32, 32))));
        }
        assert_eq!(census.kinds.len(), MAX_KINDS);
        assert_eq!(census.other, 200 - MAX_KINDS as u64);
        assert_eq!(census.columns()[2], 200, "every program is counted, kept apart or not");
        assert!(census.take_changed().unwrap().ends_with("136 x of kinds past the 64 kept apart"));
    }

    #[test]
    fn the_columns_have_their_names() {
        let header = header();
        assert_eq!(header.split(',').count(), COLUMNS);
        assert!(header.starts_with(
            "wave_programs_host,wave_programs_emulated,wave_programs_refused,wave_programs_untranslated,\
             wave_refused_swizzle"
        ));
        assert!(header.ends_with("wave_refused_section,wave_refused_lane_write,wave_refused_restored"));
        assert_eq!(wave_need::NAMES.len(), 11, "a reason added to the translator gets a column here");
    }

    #[test]
    fn programs_that_do_not_translate_are_counted_by_their_group() {
        assert_eq!(group(64, [64, 1, 1]), "one console wave");
        assert_eq!(group(64, [8, 8, 1]), "one console wave");
        assert_eq!(group(64, [8, 8, 2]), "several console waves");
        assert_eq!(group(64, [32, 1, 1]), "less than a console wave");
        assert_eq!(group(32, [32, 1, 1]), "one console wave");
        assert_eq!(group(32, [u32::MAX, u32::MAX, 2]), "several console waves");
        let mut census = Census::default();
        assert_eq!(
            census.note_untranslated(64, [64, 1, 1], false).unwrap(),
            "compute programs of 64-lane console waves that do not translate (whether they depend on the wave is \
             not known): NO HOST PIPELINE, their dispatches are left out: thread group of one console wave; the \
             emulated form was not asked for (`victory translate` says what it comes to)"
        );
        assert_eq!(census.note_untranslated(64, [8, 8, 1], false), None, "the same kind");
        assert!(
            census
                .note_untranslated(64, [64, 1, 1], true)
                .unwrap()
                .ends_with("one console wave; the emulated form was asked for and is refused too")
        );
        assert!(
            census
                .note_untranslated(64, [16, 16, 1], true)
                .unwrap()
                .ends_with("several console waves; the emulated form was asked for, which such a group cannot have")
        );
        assert!(
            census
                .note_untranslated(32, [8, 1, 1], false)
                .unwrap()
                .ends_with("less than a console wave; such a group cannot have the emulated form")
        );
        // Their own column; no reason is known for them.
        let mut columns = [0; COLUMNS];
        columns[3] = 5;
        assert_eq!(census.columns(), columns);
        // A device's answer changes nothing for them.
        assert!(census.resolve((true, (32, 32))).is_empty());
        assert_eq!(census.columns(), columns);
        assert_eq!(census.take_changed().unwrap().lines().count(), 4);
    }

    #[test]
    fn an_emulated_program_without_a_per_lane_form_says_so() {
        let mut census = Census::default();
        let only = WaveUse {
            needs: wave_need::MASK_READ,
            emulated: true,
            per_lane: "a lane-mask join of an untracked mask".into(),
            ..Default::default()
        };
        assert_eq!(
            census.note(&only, None).unwrap(),
            "compute programs carrying the console's wave themselves (emulated): mask read as a number; they have \
             no per-lane form: a lane-mask join of an untracked mask"
        );
        // Another kind than the programs emulated in place of a wave size.
        let either = WaveUse { needs: wave_need::MASK_READ, emulated: true, ..Default::default() };
        assert!(census.note(&either, None).is_some());
        assert_eq!(census.columns()[1], 2);
    }

    #[test]
    fn the_census_takes_a_few_kilobytes_at_most() {
        let mut census = Census::default();
        assert_eq!(census.bytes(), 0);
        let reason = "an emulated wave in a thread group of several waves";
        for needs in 1..=200 {
            let wave = WaveUse { refusal: reason.into(), per_lane: reason.into(), ..asks(64, needs) };
            census.note(&wave, Some((true, (32, 32))));
        }
        assert_eq!(census.kinds.len(), MAX_KINDS);
        let most = (MAX_KINDS * (size_of::<Kind>() + 8 + 2 * reason.len())) as u64;
        assert_eq!(census.bytes(), most);
        assert!(most < 16 << 10, "{most}");
        // Settling the kinds a device had not answered for adds none.
        let mut early = Census::default();
        for needs in 1..=200 {
            early.note(&asks(64, needs), None);
        }
        let before = early.bytes();
        early.resolve((true, (32, 32)));
        assert_eq!(early.bytes(), before);
    }
}
