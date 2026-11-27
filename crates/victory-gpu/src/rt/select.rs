//! Which path a ray-traced workload runs (docs/dxr-architecture.md,
//! "Selection"): **native** (host triangles traversed by inline
//! `RayQuery`), **hybrid** (host boxes, the title's own triangle test) or
//! **software** (the loop the translated program carries, walking the
//! title's structure; the reference, and what everything else falls back
//! to). Decided from what was asked for (`VICTORY_RT_BACKEND`,
//! `VICTORY_RT_CLASS`), what the device reports ([`HostCaps`]) and what
//! was recorded for it ([`super::record`]); never from its vendor or
//! model.
//!
//! | `VICTORY_RT_BACKEND` | A workload runs |
//! |---|---|
//! | unset, empty, `software` | software |
//! | `auto` | of native and hybrid, each with rows of the record that pass every gate ([`select_under`]), the one measured faster, the native one where neither was; software where neither passes |
//! | `native`, `hybrid` | that path where the device can run it and the workload has a variant for it, **not validated** and logged so at each use; software elsewhere. `VICTORY_RT_CLASS=<class>` keeps it to one workload class |
//! | `hardware` | as `native`: its older name, noted in the device report |
//! | anything else | software, noted in the device report |
//!
//! Software is the default and the end of every other row: a run never
//! loses its ray tracing to this switch, and a path the device cannot run,
//! or `auto` has no passing record for, is never selected.
//!
//! **What `auto` counts.** A verdict counts only if it was given under the
//! rule of equivalence, and by the version of the probe, that the build
//! accepts ([`Standard`]). The owner has not accepted the proposed rule
//! (docs/STATUS.md, "Waiting on the owner"), so this build accepts none
//! ([`ACCEPTED`]): under `auto` [`select`] gives every workload the
//! software path, whatever a record holds, and says why
//! ([`Reason::NoRule`]). The rule of selection itself is [`select_under`],
//! which takes what is accepted as an argument.
//!
//! **Nothing in a run calls [`select`] yet.** A run reads the switch for
//! the device report (`crate::caps::report`, through [`Switch::from_env`])
//! and for nothing else. [`selected`], which also logs what the switch
//! means once a process, is called only by the backend's own entry
//! (`Rt::open`), which nothing in a run calls. The counters ([`Tally`])
//! stay at zero.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use super::record::{Class, Digest, Form, Identity, Key, Record, Source, Tables, TitleKey, Times, Translator};
use crate::caps::HostCaps;

/// The environment variable that asks for a path.
pub const VARIABLE: &str = "VICTORY_RT_BACKEND";

/// The environment variable that keeps a forced path to one workload
/// class.
pub const CLASS_VARIABLE: &str = "VICTORY_RT_CLASS";

/// A way of running a workload, in the order of trial.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(usize)]
pub enum Path {
    /// Inline `RayQuery` over host triangles; what the code after the
    /// region reads is given equivalent values.
    Native,
    /// Inline `RayQuery` over one host box a triangle; each candidate runs
    /// the translation's emulation of the title's triangle test.
    Hybrid,
    /// The region as translated today, walking the title's own structure
    /// node by node: the reference, and what every refused case uses.
    Software,
}

impl Path {
    pub const ALL: [Path; 3] = [Path::Native, Path::Hybrid, Path::Software];

    /// The path's name: in the switch, the record and the log.
    pub const fn word(self) -> &'static str {
        match self {
            Path::Native => "native",
            Path::Hybrid => "hybrid",
            Path::Software => "software",
        }
    }
}

/// What `VICTORY_RT_BACKEND` asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Request {
    /// The software path for every workload: also what an unset variable
    /// means.
    #[default]
    Software,
    /// For each workload the fastest validated path.
    Auto,
    /// The native or the hybrid path, validated or not: for measuring.
    Native,
    Hybrid,
}

/// The older name of `native` in [`VARIABLE`].
const OLDER_NAME: &str = "hardware";

impl Request {
    /// The request a value of [`VARIABLE`] makes: `software`, `auto`,
    /// `native` or `hybrid` (and `hardware`, the older name of `native`),
    /// in any case, spaces around it ignored; unset or empty is software.
    /// Any other value is an error holding it, for the caller to report
    /// (software is then what runs).
    pub fn parse(value: Option<&str>) -> Result<Request, String> {
        let Some(value) = value else { return Ok(Request::Software) };
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "software" => Ok(Request::Software),
            "auto" => Ok(Request::Auto),
            "native" | OLDER_NAME => Ok(Request::Native),
            "hybrid" => Ok(Request::Hybrid),
            _ => Err(value.to_owned()),
        }
    }

    /// The path this request forces, if it forces one.
    pub const fn forced(self) -> Option<Path> {
        match self {
            Request::Native => Some(Path::Native),
            Request::Hybrid => Some(Path::Hybrid),
            Request::Software | Request::Auto => None,
        }
    }
}

/// Which workload classes a forced path is for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Narrow {
    /// Every class.
    #[default]
    All,
    /// The class whose hash begins with these digits (16 to 64 of them,
    /// in lower case): what a log line names a class by is enough.
    Class(String),
    /// `VICTORY_RT_CLASS` held something that names no class (kept, for
    /// the report). No path is forced then: forcing it for every class
    /// would be the opposite of what was asked.
    Unreadable(String),
}

impl Narrow {
    /// The narrowing a value of [`CLASS_VARIABLE`] that is not empty asks
    /// for.
    fn parse(text: &str) -> Narrow {
        let digits = text.to_ascii_lowercase();
        if (16..=64).contains(&digits.len()) && digits.bytes().all(|d| d.is_ascii_hexdigit()) {
            Narrow::Class(digits)
        } else {
            Narrow::Unreadable(text.to_owned())
        }
    }

    fn admits(&self, class: &Class) -> bool {
        match self {
            Narrow::All => true,
            Narrow::Class(digits) => class.hex().starts_with(digits.as_str()),
            Narrow::Unreadable(_) => false,
        }
    }
}

/// The two variables, read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switch {
    pub request: Request,
    /// The classes a forced path is for; [`Narrow::All`] where none is
    /// forced.
    pub narrow: Narrow,
    /// `hardware` was given: the older name of `native`.
    pub older_name: bool,
    /// A value of [`VARIABLE`] that is none of its names: software then.
    pub unknown: Option<String>,
    /// A value of [`CLASS_VARIABLE`] given where no path is forced: it
    /// narrows nothing.
    pub unused_class: Option<String>,
}

impl Switch {
    /// What values of [`VARIABLE`] and [`CLASS_VARIABLE`] ask for. The
    /// names are read in any case, spaces around them ignored; unset or
    /// empty is software and every class. A value that is not a name is
    /// kept for the report and never guessed at.
    pub fn parse(backend: Option<&str>, class: Option<&str>) -> Switch {
        let mut switch = Switch::default();
        match Request::parse(backend) {
            Ok(request) => {
                switch.request = request;
                switch.older_name = backend.is_some_and(|value| value.trim().eq_ignore_ascii_case(OLDER_NAME));
            }
            Err(unknown) => switch.unknown = Some(unknown),
        }
        match (class.map(str::trim).filter(|text| !text.is_empty()), switch.request.forced()) {
            (None, _) => {}
            (Some(text), Some(_)) => switch.narrow = Narrow::parse(text),
            (Some(text), None) => switch.unused_class = Some(text.to_owned()),
        }
        switch
    }

    /// What the two variables ask for, each read by its name through
    /// `get`: [`VARIABLE`] for the path, [`CLASS_VARIABLE`] for the class.
    pub fn read(get: impl Fn(&str) -> Option<String>) -> Switch {
        Switch::parse(get(VARIABLE).as_deref(), get(CLASS_VARIABLE).as_deref())
    }

    /// What the environment asks for.
    pub fn from_env() -> Switch {
        Switch::read(|name| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned()))
    }

    /// What the device report says beside its line: each value that was
    /// not taken as written.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self.older_name {
            notes.push(format!("{VARIABLE}={OLDER_NAME} is the older name of native: taken as native"));
        }
        if let Some(unknown) = &self.unknown {
            notes.push(format!("{VARIABLE}={unknown} is not software, auto, native or hybrid: taken as software"));
        }
        if let Narrow::Unreadable(text) = &self.narrow {
            notes.push(format!(
                "{CLASS_VARIABLE}={text} is not 16 to 64 hexadecimal digits of a workload class: no path is forced"
            ));
        }
        if let Some(text) = &self.unused_class {
            notes.push(format!(
                "{CLASS_VARIABLE}={text} narrows nothing: only a path forced by {VARIABLE}=native or hybrid is \
                 kept to a class"
            ));
        }
        notes
    }
}

/// One thing the native and the hybrid path need that a device does not
/// have. Both trace host structures by inline `RayQuery`, so both need
/// the same; the software path needs none of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lack {
    /// Ray tracing below tier 1.1 (the tier found, as [`HostCaps`] numbers
    /// it): inline `RayQuery` is a 1.1 feature.
    Tier(u32),
    /// No shader model 6.6: translated programs are 6.6 DXIL, and
    /// `RayQuery` needs 6.5.
    ShaderModel,
    /// The device does not answer as `ID3D12Device5`, the interface
    /// structures are sized through.
    Device,
}

impl std::fmt::Display for Lack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Lack::Tier(0) => write!(f, "no ray tracing"),
            Lack::Tier(tier) => write!(f, "ray tracing tier {}.{} (1.1 needed)", tier / 10, tier % 10),
            Lack::ShaderModel => write!(f, "no shader model 6.6"),
            Lack::Device => write!(f, "no ID3D12Device5"),
        }
    }
}

/// What the native and hybrid paths need and `caps` does not have, in a
/// fixed order; empty if the device can run them. Reads capabilities only.
pub fn lacks(caps: &HostCaps) -> Vec<Lack> {
    let mut lacks = Vec::new();
    if caps.raytracing_tier < 11 {
        lacks.push(Lack::Tier(caps.raytracing_tier));
    }
    if !caps.shader_model_6_6 {
        lacks.push(Lack::ShaderModel);
    }
    if !caps.raytracing_device {
        lacks.push(Lack::Device);
    }
    lacks
}

/// Whether [`lacks`] is empty.
fn able(caps: &HostCaps) -> bool {
    caps.raytracing_tier >= 11 && caps.shader_model_6_6 && caps.raytracing_device
}

/// What a verdict in the record must have been given under to count for
/// `auto`: a verdict given under other pass rules says nothing about
/// these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standard {
    /// The number of the rule of equivalence, its constants among it
    /// (docs/dxr-compatibility.md, "The rule"): what the probe's and the
    /// title's verdicts carry as their `rule`.
    pub rule: u32,
    /// The version of the probe (its scenes and pass rules): what the
    /// probe's verdict carries as its `version`.
    pub probe: u32,
}

/// The version of the synthetic probe in this tree (its scenes and pass
/// rules, and the columns of its result file): the one definition of that
/// number. The probe's crate takes it as its own
/// (`victory_rt_probe::PROBE_VERSION` is this constant; that crate depends
/// on this one, which cannot depend on it), its result file names it, and
/// a verdict of the probe in the record carries it as its `version`.
pub const PROBE_VERSION: u32 = 1;

/// The standard this build accepts verdicts under: **none**. The rule of
/// equivalence is a proposal the owner has not accepted, and until it is
/// `auto` selects nothing under it (docs/STATUS.md, "Waiting on the
/// owner"). So the default does not rest on nobody having written a
/// record: a record with every verdict passed selects nothing here.
/// Accepting the rule is setting this to its number and the probe's
/// version ([`PROBE_VERSION`]; a test holds the two equal), in the change
/// that records the acceptance.
pub const ACCEPTED: Option<Standard> = None;

/// What the switch means on one device: the line of the device report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub switch: Switch,
    /// What the device lacks for the native and hybrid paths (empty if
    /// nothing), whatever was asked for.
    pub lacks: Vec<Lack>,
    /// Why no validation record can count on this device in this build, if
    /// none can: [`Reason::NoRule`] (no rule of equivalence is accepted)
    /// or [`Reason::NoDriver`] (the adapter gave no driver version, so no
    /// record holds for it). `auto` then selects software for every
    /// workload; a forced path does not read the record.
    pub unrecorded: Option<Reason>,
}

impl Selection {
    /// Whether any workload may be given a native or hybrid path here: one
    /// is forced, or `auto` is asked and a validation record can count,
    /// and the device can run it. What each workload then gets is
    /// [`select`]'s to say.
    pub fn host_paths(&self) -> bool {
        let asked = match self.switch.request {
            Request::Software => false,
            Request::Auto => self.unrecorded.is_none(),
            Request::Native | Request::Hybrid => !matches!(self.switch.narrow, Narrow::Unreadable(_)),
        };
        asked && self.lacks.is_empty()
    }

    /// Whether a path was asked for by name and the device cannot run it.
    pub fn fell_back(&self) -> bool {
        self.switch.request.forced().is_some() && !self.lacks.is_empty()
    }

    /// One line for the log and the `gpu-info` report.
    pub fn describe(&self) -> String {
        let able = if self.lacks.is_empty() {
            "this device could run the native and hybrid paths".to_owned()
        } else {
            let lacks = self.lacks.iter().map(Lack::to_string).collect::<Vec<_>>().join(", ");
            format!("this device cannot run the native or the hybrid path: {lacks}")
        };
        let Some(path) = self.switch.request.forced().map(Path::word) else {
            return match (self.switch.request, self.unrecorded) {
                (Request::Auto, _) if !self.lacks.is_empty() => format!("software ({VARIABLE}=auto; {able})"),
                (Request::Auto, Some(Reason::NoDriver)) => format!(
                    "software ({VARIABLE}=auto; {able}, but its adapter gave no driver version, so no validation \
                     record holds for it and auto selects no path)"
                ),
                (Request::Auto, Some(_)) => format!(
                    "software ({VARIABLE}=auto; {able}, but this build accepts no rule of equivalence yet, so no \
                     validation record counts and auto selects no path)"
                ),
                (Request::Auto, None) => format!(
                    "auto ({VARIABLE}=auto): for each workload class the fastest of the native and hybrid paths \
                     with a passing validation record for this device and driver, software for every other"
                ),
                _ if self.switch.unknown.is_some() => format!(
                    "software ({VARIABLE} holds a value that is none of its names and is taken as software; {able})"
                ),
                _ => format!("software ({VARIABLE} is unset or software; {able})"),
            };
        };
        let refused = format!("software, though {VARIABLE}={path} asked for the {path} path");
        match &self.switch.narrow {
            _ if !self.lacks.is_empty() => format!("{refused}: {able}"),
            Narrow::Unreadable(_) => format!("{refused}: {CLASS_VARIABLE} names no class"),
            narrow => {
                let class = match narrow {
                    Narrow::Class(digits) => format!(" of class {digits}"),
                    _ => String::new(),
                };
                format!(
                    "{path} forced for measurement and NOT VALIDATED ({VARIABLE}={path}): on the workloads{class} \
                     that have a {path} variant, software for every other"
                )
            }
        }
    }
}

/// What `switch` means on a device with `caps`, in a build that accepts
/// verdicts under `accepted`.
pub fn device_under(accepted: Option<&Standard>, caps: &HostCaps, switch: Switch) -> Selection {
    let unrecorded = match accepted {
        None => Some(Reason::NoRule),
        Some(_) if caps.driver.is_none() => Some(Reason::NoDriver),
        Some(_) => None,
    };
    Selection { switch, lacks: lacks(caps), unrecorded }
}

/// What `switch` means on a device with `caps`, in this build
/// ([`ACCEPTED`]).
pub fn device(caps: &HostCaps, switch: Switch) -> Selection {
    device_under(ACCEPTED.as_ref(), caps, switch)
}

/// What the environment's switch means on a device with `caps`, with its
/// outcome logged once a process: as warnings a value that was not taken
/// as written, a path asked for by name where the device cannot run it,
/// and a forced path (it is not validated).
pub fn selected(caps: &HostCaps) -> Selection {
    let selection = device(caps, Switch::from_env());
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for note in selection.switch.notes() {
            tracing::warn!("ray tracing backend: {note}");
        }
        if selection.switch.request.forced().is_some() {
            tracing::warn!("ray tracing backend: {}", selection.describe());
        } else {
            tracing::info!("ray tracing backend: {}", selection.describe());
        }
    });
    selection
}

/// One path's variant of a workload: what its row in the record is keyed
/// by, and the hash of its container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variant {
    pub form: Form,
    pub source: Source,
    pub tables: Tables,
    pub container: Digest,
}

/// One recognised traversal region of one program, as the translator
/// describes it for selection: its class, the translator that made its
/// variants, the hash of the software container (the program as translated
/// today), and its variant for each other path, `None` where the path's
/// generator refused the region (the workload is then not eligible for
/// that path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Workload {
    pub class: Class,
    /// The translator, with its settings, that made the variants: what the
    /// probe's verdict on the class must have been given for.
    pub translator: Translator,
    pub software: Digest,
    pub native: Option<Variant>,
    pub hybrid: Option<Variant>,
}

impl Workload {
    /// The workload's variant for `path`; the software path has none (it
    /// is the program itself).
    pub fn variant(&self, path: Path) -> Option<&Variant> {
        match path {
            Path::Native => self.native.as_ref(),
            Path::Hybrid => self.hybrid.as_ref(),
            Path::Software => None,
        }
    }
}

/// Why a path was not taken for a workload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Reason {
    /// The device lacks what the path needs ([`lacks`]).
    Device,
    /// The workload has no variant for the path.
    NotEligible,
    /// The path is forced for another class ([`CLASS_VARIABLE`]).
    OtherClass,
    /// The path is forced with a class that could not be read: for none.
    ClassUnreadable,
    /// `auto`: the build accepts no rule of equivalence ([`ACCEPTED`]), so no
    /// verdict counts, whatever is recorded.
    NoRule,
    /// `auto`: the adapter gave no driver version. A driver update could not
    /// be told from none, so no record is kept for the device and none
    /// holds for it.
    NoDriver,
    /// There is no validation record.
    NoRecord,
    /// The record is for another device, or for this one with another
    /// driver.
    OtherDevice,
    /// The record has no row of either kind for the workload's class and
    /// the variant's form, structure source and side tables on this path.
    NotRecorded,
    /// Gate 1. The record has title rows for the class on this path and no
    /// verdict of the probe.
    ProbeNotRun,
    /// The probe's verdict is for the variant another translator made of
    /// the probe's program than the one that made this workload's.
    OtherTranslator,
    /// The probe's verdict was given under another rule of equivalence than
    /// the build accepts; or by another version of the probe.
    ProbeOtherRule,
    ProbeOtherVersion,
    /// The probe failed the class on this path.
    ProbeFailed,
    /// Gate 2. No container of the class was compared on the title on this
    /// path; or the row of this workload's containers holds times and no
    /// verdict.
    TitleNotCompared,
    /// Other containers of the class were compared on the title, and this
    /// workload's variant was not (another program of the class, or the
    /// same one before its translation changed).
    OtherContainer,
    /// The workload's variant was compared and timed against another
    /// software container than the workload's.
    OtherSoftware,
    /// The title's verdict was given under another rule of equivalence than
    /// the build accepts.
    TitleOtherRule,
    /// The title comparison failed; or passed over no dispatch of the class
    /// or no counted frame, which passes nothing.
    TitleFailed,
    TitleEmpty,
    /// Gate 3. The row holds no times measured on the title.
    NotTimed,
    /// The path's time on the title is not better than software's by more
    /// than the recorded spread.
    NotFaster,
    /// The native path passes every gate, and the hybrid, which does too,
    /// was measured faster ([`Times::faster_than`]).
    Slower,
    /// The hybrid path passes every gate and was not measured faster than
    /// the native path, which passes too and comes first in the order of
    /// trial.
    NativeFirst,
}

impl Reason {
    pub const ALL: [Reason; 24] = [
        Reason::Device,
        Reason::NotEligible,
        Reason::OtherClass,
        Reason::ClassUnreadable,
        Reason::NoRule,
        Reason::NoDriver,
        Reason::NoRecord,
        Reason::OtherDevice,
        Reason::NotRecorded,
        Reason::ProbeNotRun,
        Reason::OtherTranslator,
        Reason::ProbeOtherRule,
        Reason::ProbeOtherVersion,
        Reason::ProbeFailed,
        Reason::TitleNotCompared,
        Reason::OtherContainer,
        Reason::OtherSoftware,
        Reason::TitleOtherRule,
        Reason::TitleFailed,
        Reason::TitleEmpty,
        Reason::NotTimed,
        Reason::NotFaster,
        Reason::Slower,
        Reason::NativeFirst,
    ];

    /// The reason's name in a line of counters.
    pub const fn name(self) -> &'static str {
        match self {
            Reason::Device => "device",
            Reason::NotEligible => "not_eligible",
            Reason::OtherClass => "other_class",
            Reason::ClassUnreadable => "class_unreadable",
            Reason::NoRule => "no_rule",
            Reason::NoDriver => "no_driver",
            Reason::NoRecord => "no_record",
            Reason::OtherDevice => "other_device",
            Reason::NotRecorded => "not_recorded",
            Reason::ProbeNotRun => "probe_not_run",
            Reason::OtherTranslator => "other_translator",
            Reason::ProbeOtherRule => "probe_other_rule",
            Reason::ProbeOtherVersion => "probe_other_version",
            Reason::ProbeFailed => "probe_failed",
            Reason::TitleNotCompared => "title_not_compared",
            Reason::OtherContainer => "other_container",
            Reason::OtherSoftware => "other_software",
            Reason::TitleOtherRule => "title_other_rule",
            Reason::TitleFailed => "title_failed",
            Reason::TitleEmpty => "title_empty",
            Reason::NotTimed => "not_timed",
            Reason::NotFaster => "not_faster",
            Reason::Slower => "slower",
            Reason::NativeFirst => "native_first",
        }
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Reason::Device => "the device cannot run it",
            Reason::NotEligible => "the workload has no variant for it",
            Reason::OtherClass => "it is forced for another class",
            Reason::ClassUnreadable => "the class it is forced for could not be read",
            Reason::NoRule => "this build accepts no rule of equivalence, so no verdict counts",
            Reason::NoDriver => "the adapter gave no driver version, so no validation record holds for it",
            Reason::NoRecord => "there is no validation record",
            Reason::OtherDevice => "the validation record is for another device or driver",
            Reason::NotRecorded => "the validation record has no row for it",
            Reason::ProbeNotRun => "the probe has not run it",
            Reason::OtherTranslator => "the probe's verdict is for another translator's variant",
            Reason::ProbeOtherRule => "the probe's verdict was given under another rule than the one accepted",
            Reason::ProbeOtherVersion => "the probe's verdict was given by another version of the probe than the one accepted",
            Reason::ProbeFailed => "the probe failed it",
            Reason::TitleNotCompared => "it has not been compared on the title",
            Reason::OtherContainer => "it was compared on the title for other containers only",
            Reason::OtherSoftware => "it was compared on the title with another software container",
            Reason::TitleOtherRule => "the title's verdict was given under another rule than the one accepted",
            Reason::TitleFailed => "the title comparison failed it",
            Reason::TitleEmpty => "the title comparison compared no dispatch or counted no frame",
            Reason::NotTimed => "it has no times measured on the title",
            Reason::NotFaster => "it was not measured faster than software by more than the spread",
            Reason::Slower => "the other validated path was measured faster",
            Reason::NativeFirst => "it was not measured faster than the native path, which is tried first",
        })
    }
}

/// The path chosen for one workload, and why another was not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub path: Path,
    /// The path was forced by the switch: it is **not validated**.
    pub forced: bool,
    /// Why the native, and the hybrid, path was not taken: each a
    /// downgrade for the caller to count and log. `None` for the path
    /// taken, and for one that was not tried: under `software` neither is,
    /// under a forced path the other one is not, and under `auto` the
    /// hybrid is not looked to once the native path is taken, unless it
    /// passes every gate too ([`Reason::NativeFirst`]).
    pub native: Option<Reason>,
    pub hybrid: Option<Reason>,
}

impl Choice {
    /// Each path that was tried and not taken, with why.
    pub fn downgrades(&self) -> impl Iterator<Item = (Path, Reason)> {
        [(Path::Native, self.native), (Path::Hybrid, self.hybrid)]
            .into_iter()
            .filter_map(|(path, reason)| Some((path, reason?)))
    }
}

/// The three gates of `auto` for one path of one workload: the times it
/// was measured at on the title if it passes all of them, else the first
/// thing that keeps it from being selected.
///
/// Which hash each gate compares. Gate 1, the probe: none of a container.
/// The probe ran a program of its own, so its row is found by the class,
/// form, structure source, side tables and path, and must be for the
/// workload's translator. Gates 2 and 3, the title: the row is found by
/// the same key and by the hash of the workload's own variant and of its
/// own software container, both.
fn gates(
    accepted: Option<&Standard>,
    record: Option<&Record>,
    caps: &HostCaps,
    workload: &Workload,
    path: Path,
) -> Result<Times, Reason> {
    if !able(caps) {
        return Err(Reason::Device);
    }
    let variant = workload.variant(path).ok_or(Reason::NotEligible)?;
    let accepted = accepted.ok_or(Reason::NoRule)?;
    let device = Identity::of(caps);
    if !device.known() {
        return Err(Reason::NoDriver);
    }
    let record = record.ok_or(Reason::NoRecord)?;
    if !record.holds_for(&device) {
        return Err(Reason::OtherDevice);
    }
    let key = Key { class: workload.class, form: variant.form, source: variant.source, tables: variant.tables };
    // The probe: its verdict on the class, for this translator, under the
    // accepted rule and by the accepted version of the probe.
    let probe = match record.probe(&key, path) {
        Some(row) => row.probe,
        None if record.titles_of(&key, path).next().is_none() => return Err(Reason::NotRecorded),
        None => return Err(Reason::ProbeNotRun),
    };
    if probe.translator != workload.translator {
        return Err(Reason::OtherTranslator);
    }
    if probe.rule != accepted.rule {
        return Err(Reason::ProbeOtherRule);
    }
    if probe.version != accepted.probe {
        return Err(Reason::ProbeOtherVersion);
    }
    if !probe.passed {
        return Err(Reason::ProbeFailed);
    }
    // The title comparison: of this workload's variant with the software
    // container this workload would otherwise run.
    let of = TitleKey { key, path, container: variant.container, software: workload.software };
    let Some(row) = record.title(&of) else {
        let mut others = record.titles_of(&key, path).peekable();
        return Err(if others.peek().is_none() {
            Reason::TitleNotCompared
        } else if others.any(|(other, _)| other.container == variant.container) {
            Reason::OtherSoftware
        } else {
            Reason::OtherContainer
        });
    };
    let title = row.title.ok_or(Reason::TitleNotCompared)?;
    if title.rule != accepted.rule {
        return Err(Reason::TitleOtherRule);
    }
    if !title.passed {
        return Err(Reason::TitleFailed);
    }
    if title.dispatches == 0 || title.frames == 0 {
        return Err(Reason::TitleEmpty);
    }
    // The title's times.
    let times = row.times.ok_or(Reason::NotTimed)?;
    if !times.faster() {
        return Err(Reason::NotFaster);
    }
    Ok(times)
}

/// The path for `workload` on the device `caps` describes, under `switch`,
/// with `record` (the validation record at hand, if any) and in a build
/// that accepts verdicts given under `accepted`. A function of its
/// arguments alone.
///
/// * `software`: the software path.
/// * `auto`: a path is a candidate when it passes all three gates: the
///   probe passed the class for the workload's translator, the title
///   comparison passed the workload's own containers, and the title's
///   times are better than software's by more than the recorded spread
///   (each verdict under the accepted rule, the probe's by the accepted
///   version of the probe). Of two candidates the hybrid is taken only if
///   it was measured faster than the native path ([`Times::faster_than`]:
///   in runs that can be compared, by more than the spread); otherwise the
///   native path, the first in the order of trial. Software if there is no
///   candidate. With nothing accepted, a device that gave no driver
///   version, a record made on another device or with another driver, or
///   no rows for the workload's containers, there is none.
/// * `native`, `hybrid`: that path, validated or not, if the device can
///   run it, the class is the one the switch keeps it to and the workload
///   has a variant for it; else software. The record is not read.
///
/// Whatever is asked, a path the device lacks the capability for is never
/// chosen. Each path tried and not taken is in the result with its reason.
pub fn select_under(
    accepted: Option<&Standard>,
    record: Option<&Record>,
    caps: &HostCaps,
    switch: &Switch,
    workload: &Workload,
) -> Choice {
    let mut choice = Choice { path: Path::Software, forced: false, native: None, hybrid: None };
    if let Some(path) = switch.request.forced() {
        let refused = if !able(caps) {
            Some(Reason::Device)
        } else if let Narrow::Unreadable(_) = switch.narrow {
            Some(Reason::ClassUnreadable)
        } else if !switch.narrow.admits(&workload.class) {
            Some(Reason::OtherClass)
        } else {
            workload.variant(path).is_none().then_some(Reason::NotEligible)
        };
        match (refused, path) {
            (None, _) => (choice.path, choice.forced) = (path, true),
            (refused, Path::Native) => choice.native = refused,
            (refused, _) => choice.hybrid = refused,
        }
    } else if switch.request == Request::Auto {
        let native = gates(accepted, record, caps, workload, Path::Native);
        let hybrid = gates(accepted, record, caps, workload, Path::Hybrid);
        match (native, hybrid) {
            (Ok(native), Ok(hybrid)) if hybrid.faster_than(&native) => {
                (choice.path, choice.native) = (Path::Hybrid, Some(Reason::Slower));
            }
            (Ok(_), Ok(_)) => (choice.path, choice.hybrid) = (Path::Native, Some(Reason::NativeFirst)),
            (Ok(_), Err(_)) => choice.path = Path::Native,
            (Err(native), Ok(_)) => (choice.path, choice.native) = (Path::Hybrid, Some(native)),
            (Err(native), Err(hybrid)) => (choice.native, choice.hybrid) = (Some(native), Some(hybrid)),
        }
    }
    choice
}

/// [`select_under`] what this build accepts ([`ACCEPTED`]): the path a
/// workload is given here. Nothing is accepted yet, so under `auto` that
/// is the software path with [`Reason::NoRule`], whatever `record` holds.
pub fn select(record: Option<&Record>, caps: &HostCaps, switch: &Switch, workload: &Workload) -> Choice {
    select_under(ACCEPTED.as_ref(), record, caps, switch, workload)
}

/// How many workloads were given which path, and why another was not
/// taken: relaxed atomics, as the memory gauges are (`crate::account`), so
/// counting takes no lock and never fails.
pub struct Tally {
    /// By [`Path`]: the workloads given it, forced ones among them.
    selected: [AtomicU64; Path::ALL.len()],
    /// By [`Path`]: those of `selected` that were forced, not validated.
    forced: [AtomicU64; Path::ALL.len()],
    /// By [`Path`] and [`Reason`]: the times the path was not taken.
    downgrades: [[AtomicU64; Reason::ALL.len()]; Path::ALL.len()],
    changed: AtomicBool,
}

impl Default for Tally {
    fn default() -> Self {
        Self::new()
    }
}

impl Tally {
    pub const fn new() -> Self {
        Tally {
            selected: [const { AtomicU64::new(0) }; Path::ALL.len()],
            forced: [const { AtomicU64::new(0) }; Path::ALL.len()],
            downgrades: [const { [const { AtomicU64::new(0) }; Reason::ALL.len()] }; Path::ALL.len()],
            changed: AtomicBool::new(false),
        }
    }

    /// Counts one workload's choice.
    pub fn note(&self, choice: &Choice) {
        self.selected[choice.path as usize].fetch_add(1, Relaxed);
        if choice.forced {
            self.forced[choice.path as usize].fetch_add(1, Relaxed);
        }
        for (path, reason) in choice.downgrades() {
            self.downgrades[path as usize][reason as usize].fetch_add(1, Relaxed);
        }
        self.changed.store(true, Relaxed);
    }

    /// The workloads given `path`; and those of them it was forced for.
    pub fn selected(&self, path: Path) -> u64 {
        self.selected[path as usize].load(Relaxed)
    }

    pub fn forced(&self, path: Path) -> u64 {
        self.forced[path as usize].load(Relaxed)
    }

    /// The times `path` was not taken for `reason`.
    pub fn downgraded(&self, path: Path, reason: Reason) -> u64 {
        self.downgrades[path as usize][reason as usize].load(Relaxed)
    }

    /// Every count as `name=value` pairs in a fixed order, those at zero
    /// left out but for the three paths: one log line.
    pub fn line(&self) -> String {
        let pairs = |count: &dyn Fn(Path) -> u64| {
            Path::ALL
                .into_iter()
                .filter(|&path| count(path) != 0)
                .map(|path| format!("{}={}", path.word(), count(path)))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut line = Path::ALL.map(|path| format!("{}={}", path.word(), self.selected(path))).join(" ");
        let forced = pairs(&|path| self.forced(path));
        if !forced.is_empty() {
            line += &format!("; forced and NOT VALIDATED: {forced}");
        }
        for path in [Path::Native, Path::Hybrid] {
            let reasons: Vec<String> = Reason::ALL
                .into_iter()
                .filter(|&reason| self.downgraded(path, reason) != 0)
                .map(|reason| format!("{}={}", reason.name(), self.downgraded(path, reason)))
                .collect();
            if !reasons.is_empty() {
                line += &format!("; {} not taken: {}", path.word(), reasons.join(" "));
            }
        }
        line
    }

    /// [`Tally::line`], if a choice was counted since the last call.
    pub fn take_changed(&self) -> Option<String> {
        self.changed.swap(false, Relaxed).then(|| self.line())
    }
}

/// The run's counts.
static TALLY: Tally = Tally::new();

/// The run's counts of [`choose`]'s choices.
pub fn tally() -> &'static Tally {
    &TALLY
}

/// The run's counts as one line, if a workload was chosen for since the
/// last call: what the frame meter logs beside its five-second lines
/// (`host.rs`). Nothing chooses in a run yet, so nothing is logged.
pub fn take_changed() -> Option<String> {
    TALLY.take_changed()
}

/// [`select`] for a run: the choice is counted in the run's [`tally`], a
/// forced path is logged as not validated at each use, and each path not
/// taken is logged with its reason at debug level. To be called once for
/// a workload when its variant is made, not at each dispatch.
pub fn choose(record: Option<&Record>, caps: &HostCaps, switch: &Switch, workload: &Workload) -> Choice {
    let choice = select(record, caps, switch, workload);
    TALLY.note(&choice);
    let class = workload.class.short();
    if choice.forced {
        tracing::warn!(
            "ray tracing: workload class {class} runs the {path} path, forced by {VARIABLE}={path} for measurement: \
             NOT VALIDATED on this device and driver",
            path = choice.path.word()
        );
    }
    for (path, reason) in choice.downgrades() {
        tracing::debug!(
            "ray tracing: workload class {class} runs the {} path, not the {} path: {reason}",
            choice.path.word(),
            path.word()
        );
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::super::record::{Probe, Title};
    use super::*;

    /// A device with everything the native and hybrid paths need, by
    /// capability, and a driver version for the record to be keyed by.
    fn able_device() -> HostCaps {
        HostCaps {
            raytracing_tier: 11,
            shader_model_6_6: true,
            raytracing_device: true,
            vendor: 0x10de,
            device: 0x2c05,
            driver: Some([32, 0, 15, 7000]),
            ..Default::default()
        }
    }

    fn switch(backend: &str) -> Switch {
        Switch::parse(Some(backend), None)
    }

    /// A standard for these tests: what a build would accept once the rule
    /// is. (This build accepts none: [`ACCEPTED`].)
    const STANDARD: Standard = Standard { rule: 2, probe: 3 };

    const CLASS: Class = Class([0xa1; 32]);
    const TRANSLATOR: Translator = Translator([0x77; 32]);
    const SOFTWARE: Digest = Digest([0x50; 32]);
    const NATIVE: Variant =
        Variant { form: Form::Dual, source: Source::Descriptions, tables: Tables::NONE, container: Digest([0xc0; 32]) };
    const HYBRID: Variant =
        Variant { form: Form::Dual, source: Source::Nodes, tables: Tables::NONE, container: Digest([0xc1; 32]) };

    /// A workload with a variant for both paths: one title program.
    fn workload() -> Workload {
        Workload { class: CLASS, translator: TRANSLATOR, software: SOFTWARE, native: Some(NATIVE), hybrid: Some(HYBRID) }
    }

    fn key(variant: &Variant) -> Key {
        Key { class: CLASS, form: variant.form, source: variant.source, tables: variant.tables }
    }

    /// The title comparison of `workload`'s variant for `path` with its
    /// software container.
    fn of(workload: &Workload, path: Path) -> TitleKey {
        let variant = workload.variant(path).unwrap();
        TitleKey { key: key(variant), path, container: variant.container, software: workload.software }
    }

    /// The probe's verdict: passed, for [`TRANSLATOR`], under [`STANDARD`],
    /// on the container of the probe's own program, which is no title
    /// program's.
    const PASSED: Probe =
        Probe { passed: true, rule: 2, version: 3, translator: TRANSLATOR, container: Digest([0xb0; 32]) };
    const COMPARED: Title = Title { passed: true, rule: 2, dispatches: 1200, frames: 600 };

    fn times(median_us: u64) -> Times {
        Times { median_us, spread_us: 30, software_median_us: 900, software_spread_us: 45 }
    }

    /// Records everything the three gates ask for about `path` of
    /// [`workload`], measured at `median_us` against software's 900.
    fn pass(record: &mut Record, path: Path, median_us: u64) {
        let workload = workload();
        record.set_probe(key(workload.variant(path).unwrap()), path, PASSED).unwrap();
        record.set_title(of(&workload, path), COMPARED).unwrap();
        record.set_times(of(&workload, path), times(median_us)).unwrap();
    }

    /// A record for [`able_device`] with nothing in it.
    fn empty() -> Record {
        Record::new(Identity::of(&able_device()))
    }

    /// A record in which `path` passes every gate.
    fn passing(path: Path) -> Record {
        let mut record = empty();
        pass(&mut record, path, 400);
        record
    }

    /// The rule of selection with [`STANDARD`] accepted.
    fn under(record: Option<&Record>, caps: &HostCaps, switch: &Switch, workload: &Workload) -> Choice {
        select_under(Some(&STANDARD), record, caps, switch, workload)
    }

    fn auto(record: Option<&Record>) -> Choice {
        under(record, &able_device(), &switch("auto"), &workload())
    }

    fn software(native: Reason, hybrid: Reason) -> Choice {
        Choice { path: Path::Software, forced: false, native: Some(native), hybrid: Some(hybrid) }
    }

    #[test]
    fn software_is_what_an_unset_variable_means() {
        assert_eq!(Switch::parse(None, None), Switch::default());
        assert_eq!(Switch::parse(Some(""), Some("")), Switch::default());
        assert_eq!(Switch::default().request, Request::Software);
        assert_eq!(switch("software"), Switch::default());
        assert!(Switch::default().notes().is_empty());
        for accepted in [None, Some(&STANDARD)] {
            let on_device = device_under(accepted, &able_device(), Switch::default());
            assert!(!on_device.host_paths(), "a capable device does not turn a path on");
            assert!(!on_device.fell_back());
            // Not even with a record in which both paths pass every gate.
            let mut record = passing(Path::Native);
            pass(&mut record, Path::Hybrid, 300);
            let choice = select_under(accepted, Some(&record), &able_device(), &Switch::default(), &workload());
            assert_eq!(choice, Choice { path: Path::Software, forced: false, native: None, hybrid: None });
            assert_eq!(choice.downgrades().count(), 0, "nothing was tried, so nothing was downgraded");
        }
    }

    #[test]
    fn the_names_are_read_in_any_case() {
        for (text, request) in [
            ("software", Request::Software),
            ("Native", Request::Native),
            (" AUTO ", Request::Auto),
            ("hybrid\n", Request::Hybrid),
            ("HYBRID", Request::Hybrid),
        ] {
            let read = switch(text);
            assert_eq!((read.request, read.notes().len()), (request, 0), "{text:?}");
            assert_eq!(Request::parse(Some(text)), Ok(request), "{text:?}");
        }
        assert_eq!(Request::Native.forced(), Some(Path::Native));
        assert_eq!(Request::Hybrid.forced(), Some(Path::Hybrid));
        assert_eq!((Request::Auto.forced(), Request::Software.forced()), (None, None));
        // The request alone, in the shape it had before the paths were
        // three: a value that is no name is handed back.
        assert_eq!((Request::parse(None), Request::parse(Some(" "))), (Ok(Request::Software), Ok(Request::Software)));
        assert_eq!(Request::parse(Some(" Hardware ")), Ok(Request::Native));
        assert_eq!(Request::parse(Some(" dxr ")), Err(" dxr ".to_owned()));
    }

    /// What was written against the earlier switch still works: `hardware`
    /// asks for the native path, and the report says it was renamed.
    #[test]
    fn hardware_is_the_older_name_of_native() {
        for text in ["hardware", "Hardware", " HARDWARE "] {
            let read = switch(text);
            assert_eq!((read.request, read.older_name, &read.unknown), (Request::Native, true, &None), "{text:?}");
            assert_eq!(read.notes(), ["VICTORY_RT_BACKEND=hardware is the older name of native: taken as native"]);
            let choice = select(None, &able_device(), &read, &workload());
            assert_eq!(choice, select(None, &able_device(), &switch("native"), &workload()));
            assert_eq!((choice.path, choice.forced), (Path::Native, true));
        }
        assert!(!switch("native").older_name && !switch("hardwares").older_name);
    }

    /// A value that is not one of the names is handed back, not guessed
    /// at: `1` and `on` are how the on/off switches are set, and are not
    /// a path. Every workload then runs software, whatever is recorded.
    #[test]
    fn an_unknown_value_is_software_and_is_noted() {
        let mut record = passing(Path::Native);
        pass(&mut record, Path::Hybrid, 300);
        for text in ["1", "on", "dxr", "hard ware", "software,native", "0", "natif", "native hybrid"] {
            let read = switch(text);
            assert_eq!((read.request, read.unknown.as_deref()), (Request::Software, Some(text)), "{text:?}");
            assert_eq!(
                read.notes(),
                [format!("VICTORY_RT_BACKEND={text} is not software, auto, native or hybrid: taken as software")]
            );
            let choice = under(Some(&record), &able_device(), &read, &workload());
            assert_eq!(choice, Choice { path: Path::Software, forced: false, native: None, hybrid: None });
            let on_device = device_under(Some(&STANDARD), &able_device(), read);
            assert!(!on_device.host_paths());
            // The line does not say the variable is unset or software
            // beside a note that it holds something else.
            assert_eq!(
                on_device.describe(),
                "software (VICTORY_RT_BACKEND holds a value that is none of its names and is taken as software; \
                 this device could run the native and hybrid paths)"
            );
        }
    }

    #[test]
    fn a_class_is_read_only_where_a_path_is_forced() {
        let class = CLASS.hex();
        for digits in [&class[..16], &class[..], &class[..40], class.to_uppercase().as_str()] {
            let read = Switch::parse(Some("native"), Some(digits));
            assert_eq!(read.narrow, Narrow::Class(digits.to_ascii_lowercase()), "{digits}");
            assert!(read.notes().is_empty());
        }
        assert_eq!(Switch::parse(Some("hybrid"), Some(" \t")).narrow, Narrow::All);
        assert_eq!(Switch::parse(Some("hybrid"), None).narrow, Narrow::All);
        // Too short, too long, not digits: nothing is forced, and it says so.
        for text in [&class[..15], "a1a1a1a1a1a1a1a1 a1", "the-first-class!", "0x1234567890abcdef", &format!("{class}0")] {
            let read = Switch::parse(Some("native"), Some(text));
            assert_eq!(read.narrow, Narrow::Unreadable(text.to_owned()), "{text:?}");
            let note = format!(
                "VICTORY_RT_CLASS={text} is not 16 to 64 hexadecimal digits of a workload class: no path is forced"
            );
            assert_eq!(read.notes(), [note]);
        }
        // Under software or auto it narrows nothing, readable or not.
        for (backend, text) in [("auto", &class[..16]), ("software", "nonsense"), ("dxr", &class[..16])] {
            let read = Switch::parse(Some(backend), Some(text));
            assert_eq!((&read.narrow, read.unused_class.as_deref()), (&Narrow::All, Some(text)), "{backend}");
            let said = read.notes().join("\n");
            assert!(said.contains(&format!("VICTORY_RT_CLASS={text} narrows nothing")), "{said}");
        }
        assert_eq!(Switch::parse(Some("dxr"), Some("x")).notes().len(), 2, "each value not taken as written");
    }

    /// Each of the two variables is read by its own name: the path from
    /// one, the class from the other.
    #[test]
    fn the_switch_is_read_from_both_variables_each_by_its_name() {
        assert_eq!((VARIABLE, CLASS_VARIABLE), ("VICTORY_RT_BACKEND", "VICTORY_RT_CLASS"));
        let class = CLASS.hex();
        let held = |backend: Option<&str>, class: Option<&str>| {
            let (backend, class) = (backend.map(str::to_owned), class.map(str::to_owned));
            move |name: &str| match name {
                "VICTORY_RT_BACKEND" => backend.clone(),
                "VICTORY_RT_CLASS" => class.clone(),
                other => panic!("{other} is not a variable of the switch"),
            }
        };
        let read = Switch::read(held(Some("hybrid"), Some(&class[..16])));
        assert_eq!((read.request, &read.narrow), (Request::Hybrid, &Narrow::Class(class[..16].to_owned())));
        assert_eq!(read, Switch::parse(Some("hybrid"), Some(&class[..16])));
        // The other way round they would say something else.
        let turned = Switch::read(held(Some(&class[..16]), Some("hybrid")));
        assert_eq!((turned.request, turned.unknown.as_deref()), (Request::Software, Some(&class[..16])));
        assert_eq!(Switch::read(held(None, None)), Switch::default());
        assert_eq!(Switch::read(held(Some("auto"), None)).request, Request::Auto);
        assert_eq!(Switch::read(held(None, Some("x"))).unused_class.as_deref(), Some("x"));
    }

    #[test]
    fn each_missing_capability_is_named_and_leaves_software_only() {
        let mut record = passing(Path::Native);
        pass(&mut record, Path::Hybrid, 300);
        for (caps, lack) in [
            (HostCaps { raytracing_tier: 10, ..able_device() }, Lack::Tier(10)),
            (HostCaps { raytracing_tier: 0, ..able_device() }, Lack::Tier(0)),
            (HostCaps { shader_model_6_6: false, ..able_device() }, Lack::ShaderModel),
            (HostCaps { raytracing_device: false, ..able_device() }, Lack::Device),
        ] {
            assert_eq!(lacks(&caps), [lack]);
            assert!(!able(&caps));
            for backend in ["native", "hybrid", "auto", "hardware"] {
                let on_device = device_under(Some(&STANDARD), &caps, switch(backend));
                assert!(!on_device.host_paths(), "{lack:?} {backend}");
                assert_eq!(on_device.fell_back(), backend != "auto", "only a request by name is refused");
                assert!(on_device.describe().starts_with("software"), "{}", on_device.describe());
                assert!(on_device.describe().contains(&lack.to_string()), "{}", on_device.describe());
                // The record passes every gate, and still the device decides.
                let choice = under(Some(&record), &caps, &switch(backend), &workload());
                assert_eq!((choice.path, choice.forced), (Path::Software, false), "{lack:?} {backend}");
                let expected = match backend {
                    "auto" => vec![(Path::Native, Reason::Device), (Path::Hybrid, Reason::Device)],
                    "hybrid" => vec![(Path::Hybrid, Reason::Device)],
                    _ => vec![(Path::Native, Reason::Device)],
                };
                assert_eq!(choice.downgrades().collect::<Vec<_>>(), expected, "{lack:?} {backend}");
                // With nothing accepted the device is still the first
                // thing said.
                let choice = select(Some(&record), &caps, &switch(backend), &workload());
                assert_eq!(choice.downgrades().collect::<Vec<_>>(), expected, "{lack:?} {backend}");
            }
        }
        assert_eq!(lacks(&HostCaps::default()), [Lack::Tier(0), Lack::ShaderModel, Lack::Device]);
        assert!(lacks(&able_device()).is_empty() && able(&able_device()));
        let later = HostCaps { raytracing_tier: 12, ..able_device() };
        assert!(able(&later) && lacks(&later).is_empty(), "a higher tier than the one needed is enough");
        assert_eq!(auto(Some(&record)).path, under(Some(&record), &later, &switch("auto"), &workload()).path);
        assert_eq!(auto(Some(&record)).path, Path::Hybrid);
    }

    /// The probe's version is one number: what a build accepts verdicts of
    /// the probe by, once it accepts any, is the version of the probe in
    /// the tree, which the probe's crate takes from here. Setting
    /// [`ACCEPTED`] to another probe's number fails this.
    #[test]
    fn the_probe_a_build_accepts_verdicts_of_is_the_probe_in_the_tree() {
        if let Some(accepted) = ACCEPTED {
            assert_eq!(accepted.probe, PROBE_VERSION, "ACCEPTED names another version of the probe than the tree's");
        }
    }

    /// The default in force (docs/STATUS.md): the rule of equivalence is
    /// not accepted, and `auto` selects nothing under it. That is a
    /// constant of the build, not the absence of a record: a record in
    /// which every verdict says passed selects nothing here. When the
    /// owner accepts the rule this test changes with [`ACCEPTED`].
    #[test]
    fn this_build_accepts_no_rule_so_auto_selects_nothing_whatever_is_recorded() {
        assert_eq!(ACCEPTED, None);
        let mut record = passing(Path::Native);
        pass(&mut record, Path::Hybrid, 300);
        assert_eq!(auto(Some(&record)).path, Path::Hybrid, "the same record, were a rule accepted");
        let choice = select(Some(&record), &able_device(), &switch("auto"), &workload());
        assert_eq!(choice, software(Reason::NoRule, Reason::NoRule));
        assert_eq!(choice, select_under(None, Some(&record), &able_device(), &switch("auto"), &workload()));
        // Verdicts that name whatever rule and version, the zeros a text
        // written by hand might hold among them.
        for (rule, version) in [(0, 0), (1, 1), (u32::MAX, u32::MAX)] {
            let mut record = empty();
            let workload = workload();
            record.set_probe(key(&NATIVE), Path::Native, Probe { rule, version, ..PASSED }).unwrap();
            record.set_title(of(&workload, Path::Native), Title { rule, ..COMPARED }).unwrap();
            record.set_times(of(&workload, Path::Native), times(1)).unwrap();
            let choice = select(Some(&record), &able_device(), &switch("auto"), &workload);
            assert_eq!(choice, software(Reason::NoRule, Reason::NoRule), "rule {rule}, probe {version}");
        }
        assert_eq!(select(None, &able_device(), &switch("auto"), &workload()), software(Reason::NoRule, Reason::NoRule));

        // The report's line promises no selection, and the backend's own
        // entry has no path to open for.
        let on_device = device(&able_device(), switch("auto"));
        assert_eq!(on_device, device_under(None, &able_device(), switch("auto")));
        assert_eq!(on_device.unrecorded, Some(Reason::NoRule));
        assert!(!on_device.host_paths() && !on_device.fell_back());
        assert_eq!(
            on_device.describe(),
            "software (VICTORY_RT_BACKEND=auto; this device could run the native and hybrid paths, but this build \
             accepts no rule of equivalence yet, so no validation record counts and auto selects no path)"
        );
        // A forced path reads no record and needs no rule: it is not
        // validated, and says so.
        for backend in ["native", "hybrid"] {
            let choice = select(None, &able_device(), &switch(backend), &workload());
            assert!(choice.forced && choice.downgrades().count() == 0, "{backend}");
            assert!(device(&able_device(), switch(backend)).host_paths(), "{backend}");
        }
    }

    // `auto`: every way a path is kept from being selected.

    #[test]
    fn auto_without_a_record_is_software() {
        assert_eq!(auto(None), software(Reason::NoRecord, Reason::NoRecord));
        assert_eq!(auto(Some(&empty())), software(Reason::NotRecorded, Reason::NotRecorded));
        // Rows for one path say nothing of the other.
        let choice = auto(Some(&passing(Path::Hybrid)));
        assert_eq!(choice, Choice { path: Path::Hybrid, forced: false, native: Some(Reason::NotRecorded), hybrid: None });
    }

    #[test]
    fn auto_takes_a_path_whose_rows_pass_all_three_gates() {
        let choice = auto(Some(&passing(Path::Native)));
        assert_eq!(choice, Choice { path: Path::Native, forced: false, native: None, hybrid: None });
        assert_eq!(choice.downgrades().count(), 0, "the first path in the order of trial was taken");
    }

    /// The identity is an equality key: the same rows made on another
    /// device, or on this one before a driver update, select nothing.
    #[test]
    fn a_record_for_another_driver_or_device_counts_as_absent() {
        let record = passing(Path::Native);
        assert_eq!(auto(Some(&record)).path, Path::Native);
        for other in [
            HostCaps { driver: Some([32, 0, 15, 7001]), ..able_device() },
            HostCaps { device: 0x2c02, ..able_device() },
            HostCaps { vendor: 0x1002, ..able_device() },
        ] {
            let choice = under(Some(&record), &other, &switch("auto"), &workload());
            assert_eq!(choice, software(Reason::OtherDevice, Reason::OtherDevice), "{other:?}");
        }
    }

    /// A device whose adapter gives no driver version has no record: one
    /// made for it would hold after a driver update too. The reason and
    /// the report's line say that, and not that the record is another
    /// device's.
    #[test]
    fn a_device_that_gives_no_driver_version_has_no_record_and_says_so() {
        let no_driver = HostCaps { driver: None, ..able_device() };
        let said = "the adapter gave no driver version, so no validation record holds for it";
        assert_eq!(Reason::NoDriver.to_string(), said);
        // Whatever record is at hand: this device's with its driver
        // version, one made without a version, or none.
        let mut made_without = Record::new(Identity::of(&no_driver));
        pass(&mut made_without, Path::Native, 400);
        for record in [Some(&passing(Path::Native)), Some(&made_without), None] {
            let choice = under(record, &no_driver, &switch("auto"), &workload());
            assert_eq!(choice, software(Reason::NoDriver, Reason::NoDriver));
        }
        let on_device = device_under(Some(&STANDARD), &no_driver, switch("auto"));
        assert_eq!(on_device.unrecorded, Some(Reason::NoDriver));
        assert!(!on_device.host_paths());
        assert_eq!(
            on_device.describe(),
            "software (VICTORY_RT_BACKEND=auto; this device could run the native and hybrid paths, but its adapter \
             gave no driver version, so no validation record holds for it and auto selects no path)"
        );
        // A forced path reads no record: it runs here as anywhere.
        let forced = under(None, &no_driver, &switch("native"), &workload());
        assert_eq!((forced.path, forced.forced), (Path::Native, true));
        assert!(device_under(Some(&STANDARD), &no_driver, switch("native")).host_paths());
        // And what the device lacks is said before what its adapter omits.
        let unable = HostCaps { raytracing_tier: 10, ..no_driver };
        assert_eq!(under(None, &unable, &switch("auto"), &workload()), software(Reason::Device, Reason::Device));
    }

    /// The rule of `crate::caps`: who made the device decides nothing. The
    /// same capabilities and the same rows, recorded on each device, give
    /// the same choice on every one of them.
    #[test]
    fn the_choice_does_not_depend_on_who_made_the_device() {
        let makers = [(0x10de, 0x2c05), (0x1002, 0x73df), (0x8086, 0x56a0), (0x1414, 0x8c), (0, 0)];
        for backend in ["software", "auto", "native", "hybrid"] {
            for tier in [11, 10] {
                let mut choices = Vec::new();
                for (vendor, id) in makers {
                    let caps = HostCaps {
                        vendor,
                        device: id,
                        name: format!("{vendor:#x}"),
                        driver: Some([vendor as u16, 0, 15, 1]),
                        software: vendor == 0x1414,
                        dedicated_mb: u64::from(id),
                        raytracing_tier: tier,
                        ..able_device()
                    };
                    let mut record = Record::new(Identity::of(&caps));
                    pass(&mut record, Path::Native, 400);
                    pass(&mut record, Path::Hybrid, 300);
                    choices.push(under(Some(&record), &caps, &switch(backend), &workload()));
                    let unnamed = HostCaps { raytracing_tier: tier, ..able_device() };
                    for accepted in [None, Some(&STANDARD)] {
                        let here = device_under(accepted, &caps, switch(backend));
                        assert_eq!(here, device_under(accepted, &unnamed, switch(backend)));
                    }
                }
                assert!(choices.iter().all(|choice| choice == &choices[0]), "{backend} {tier}: {choices:?}");
                let taken = match (backend, tier) {
                    ("auto", 11) => Path::Hybrid,
                    ("native", 11) => Path::Native,
                    ("hybrid", 11) => Path::Hybrid,
                    _ => Path::Software,
                };
                assert_eq!(choices[0].path, taken, "{backend} {tier}");
            }
        }
    }

    /// The probe runs a synthetic program of its own for a class, and the
    /// title comparison runs the title's: two containers. The probe's
    /// verdict counts for every program of the class this translator made;
    /// the title's verdict and times only for the containers compared.
    #[test]
    fn the_probes_verdict_is_the_classes_and_the_titles_is_the_containers() {
        let first = workload();
        assert_ne!(PASSED.container, NATIVE.container, "the probe's program is not the title's");
        let mut record = empty();
        // The probe, then the title comparison of the first program.
        assert_eq!(record.set_probe(key(&NATIVE), Path::Native, PASSED), Ok(None));
        assert_eq!(auto(Some(&record)).native, Some(Reason::TitleNotCompared));
        assert_eq!(record.set_title(of(&first, Path::Native), COMPARED), Ok(None));
        assert_eq!(record.set_times(of(&first, Path::Native), times(400)), Ok(None));
        let native = Choice { path: Path::Native, forced: false, native: None, hybrid: None };
        assert_eq!(auto(Some(&record)), Choice { hybrid: None, ..native }, "all three gates, from two programs");

        // A second program with a loop of the same class: other
        // containers. It has the probe's verdict and no comparison yet.
        let second_variant = Variant { container: Digest([0xd0; 32]), ..NATIVE };
        let second = Workload { software: Digest([0x51; 32]), native: Some(second_variant), hybrid: None, ..first };
        let ask = |record: &Record, workload: &Workload| under(Some(record), &able_device(), &switch("auto"), workload);
        assert_eq!(ask(&record, &second), software(Reason::OtherContainer, Reason::NotEligible));
        // Compared and timed, it is selected, and the first program still is.
        record.set_title(of(&second, Path::Native), COMPARED).unwrap();
        record.set_times(of(&second, Path::Native), times(500)).unwrap();
        assert_eq!(ask(&record, &second).path, Path::Native);
        assert_eq!(ask(&record, &first), native, "validating a second program took nothing from the first");
        // A failed comparison of the second leaves the first selected.
        record.set_title(of(&second, Path::Native), Title { passed: false, ..COMPARED }).unwrap();
        assert_eq!(ask(&record, &second).native, Some(Reason::TitleFailed));
        assert_eq!(ask(&record, &first), native);
        assert_eq!(record.len(), 3, "one probe row, a title row for each program");

        // The other order: the title comparison first, the probe after.
        let mut record = empty();
        record.set_title(of(&first, Path::Native), COMPARED).unwrap();
        record.set_times(of(&first, Path::Native), times(400)).unwrap();
        assert_eq!(auto(Some(&record)).native, Some(Reason::ProbeNotRun));
        record.set_probe(key(&NATIVE), Path::Native, PASSED).unwrap();
        assert_eq!(auto(Some(&record)), native);
    }

    #[test]
    fn a_comparison_of_other_containers_counts_as_absent() {
        let record = passing(Path::Native);
        let retranslated = Variant { container: Digest([0xee; 32]), ..NATIVE };
        let changed = Workload { native: Some(retranslated), ..workload() };
        let choice = under(Some(&record), &able_device(), &switch("auto"), &changed);
        assert_eq!(choice, software(Reason::OtherContainer, Reason::NotRecorded));

        // The software container the path was compared and timed against.
        let changed = Workload { software: Digest([0xef; 32]), ..workload() };
        let choice = under(Some(&record), &able_device(), &switch("auto"), &changed);
        assert_eq!(choice.native, Some(Reason::OtherSoftware));
        assert_eq!(choice.path, Path::Software);
        // Both changed: no comparison of this variant at all.
        let changed = Workload { software: Digest([0xef; 32]), native: Some(retranslated), ..workload() };
        assert_eq!(under(Some(&record), &able_device(), &switch("auto"), &changed).native, Some(Reason::OtherContainer));

        // Another form, structure source or set of side tables has rows of
        // its own, of the probe and of the title.
        for other in [
            Variant { form: Form::BothLoops, ..NATIVE },
            Variant { source: Source::Packed, ..NATIVE },
            Variant { tables: Tables::NODE, ..NATIVE },
        ] {
            let changed = Workload { native: Some(other), ..workload() };
            let choice = under(Some(&record), &able_device(), &switch("auto"), &changed);
            assert_eq!(choice.native, Some(Reason::NotRecorded), "{other:?}");
        }
        // Another class has no rows.
        let another = Workload { class: Class([0xa2; 32]), ..workload() };
        let choice = under(Some(&record), &able_device(), &switch("auto"), &another);
        assert_eq!(choice.native, Some(Reason::NotRecorded));
    }

    /// The probe's verdict was given for one translator's variant. After a
    /// change of the translator it says nothing, though the class is the
    /// same and a title container may be too.
    #[test]
    fn a_probe_verdict_for_another_translator_selects_nothing() {
        let record = passing(Path::Native);
        let rebuilt = Workload { translator: Translator([0x78; 32]), ..workload() };
        let choice = under(Some(&record), &able_device(), &switch("auto"), &rebuilt);
        assert_eq!(choice, software(Reason::OtherTranslator, Reason::NotRecorded));
        // The probe run again with that translator: selected again, the
        // title rows of the unchanged containers still counting.
        let mut record = record;
        let again = Probe { translator: Translator([0x78; 32]), ..PASSED };
        let went = record.set_probe(key(&NATIVE), Path::Native, again).unwrap();
        assert_eq!(went.map(|row| row.probe), Some(PASSED), "the earlier verdict is handed back");
        assert_eq!(under(Some(&record), &able_device(), &switch("auto"), &rebuilt).path, Path::Native);
        assert_eq!(auto(Some(&record)).native, Some(Reason::OtherTranslator), "and the older build has none");
    }

    /// A verdict counts only under the rule the build accepts, and the
    /// probe's only from the version of the probe it accepts: one given
    /// under other pass rules says nothing about these.
    #[test]
    fn a_verdict_under_another_rule_or_from_another_probe_selects_nothing() {
        let workload = workload();
        let with = |probe: Probe, title: Title| {
            let mut record = empty();
            record.set_probe(key(&NATIVE), Path::Native, probe).unwrap();
            record.set_title(of(&workload, Path::Native), title).unwrap();
            record.set_times(of(&workload, Path::Native), times(400)).unwrap();
            auto(Some(&record)).native
        };
        assert_eq!((STANDARD.rule, STANDARD.probe), (PASSED.rule, PASSED.version));
        assert_eq!(with(PASSED, COMPARED), None, "both under the standard: selected");
        for rule in [0, 1, 3, u32::MAX] {
            assert_eq!(with(Probe { rule, ..PASSED }, COMPARED), Some(Reason::ProbeOtherRule), "{rule}");
            assert_eq!(with(PASSED, Title { rule, ..COMPARED }), Some(Reason::TitleOtherRule), "{rule}");
        }
        for version in [0, 2, 4, u32::MAX] {
            assert_eq!(with(Probe { version, ..PASSED }, COMPARED), Some(Reason::ProbeOtherVersion), "{version}");
        }
        // The rule's number is not the probe's version, nor the other way.
        let crossed = Probe { rule: STANDARD.probe, version: STANDARD.rule, ..PASSED };
        assert_eq!(with(crossed, COMPARED), Some(Reason::ProbeOtherRule));
        // A failed verdict under another rule is not a failure under this one.
        let failed = Probe { passed: false, rule: 1, ..PASSED };
        assert_eq!(with(failed, COMPARED), Some(Reason::ProbeOtherRule));
        // Another standard accepted: the same record, the other answer.
        let later = Standard { rule: 3, probe: 3 };
        let record = passing(Path::Native);
        let choice = select_under(Some(&later), Some(&record), &able_device(), &switch("auto"), &workload);
        assert_eq!(choice.native, Some(Reason::ProbeOtherRule));
    }

    /// The first gate.
    #[test]
    fn a_probe_that_failed_or_did_not_run_selects_nothing() {
        let workload = workload();
        let mut record = empty();
        // The title's verdict and times alone: the probe has not run.
        record.set_title(of(&workload, Path::Native), COMPARED).unwrap();
        record.set_times(of(&workload, Path::Native), times(400)).unwrap();
        assert_eq!(auto(Some(&record)), software(Reason::ProbeNotRun, Reason::NotRecorded));

        let failed = Probe { passed: false, ..PASSED };
        record.set_probe(key(&NATIVE), Path::Native, failed).unwrap();
        assert_eq!(auto(Some(&record)), software(Reason::ProbeFailed, Reason::NotRecorded));
        // The probe's verdict on the other path is not this path's.
        record.set_probe(key(&NATIVE), Path::Hybrid, PASSED).unwrap();
        assert_eq!(auto(Some(&record)).native, Some(Reason::ProbeFailed));

        record.set_probe(key(&NATIVE), Path::Native, PASSED).unwrap();
        assert_eq!(auto(Some(&record)).path, Path::Native, "with the probe passed, every gate is");
    }

    /// The second gate.
    #[test]
    fn a_passed_probe_without_a_passing_title_verdict_selects_nothing() {
        let (workload, mut record) = (workload(), empty());
        let native = of(&workload, Path::Native);
        record.set_probe(key(&NATIVE), Path::Native, PASSED).unwrap();
        assert_eq!(auto(Some(&record)), software(Reason::TitleNotCompared, Reason::NotRecorded));

        // Times without a verdict are no verdict.
        record.set_times(native, times(400)).unwrap();
        assert_eq!(auto(Some(&record)).native, Some(Reason::TitleNotCompared));

        let failed = Title { passed: false, ..COMPARED };
        record.set_title(native, failed).unwrap();
        assert_eq!(auto(Some(&record)).native, Some(Reason::TitleFailed));

        // A verdict over nothing passes nothing.
        for empty in [Title { dispatches: 0, ..COMPARED }, Title { frames: 0, ..COMPARED }] {
            record.set_title(native, empty).unwrap();
            assert_eq!(auto(Some(&record)).native, Some(Reason::TitleEmpty), "{empty:?}");
        }
        record.set_title(native, Title { dispatches: 1, frames: 1, ..COMPARED }).unwrap();
        assert_eq!(auto(Some(&record)).path, Path::Native, "one dispatch and one frame are something");

        record.set_title(native, COMPARED).unwrap();
        assert_eq!(auto(Some(&record)).path, Path::Native);
    }

    /// The third gate.
    #[test]
    fn a_title_time_not_better_than_softwares_selects_nothing() {
        let (workload, mut record) = (workload(), empty());
        let native = of(&workload, Path::Native);
        record.set_probe(key(&NATIVE), Path::Native, PASSED).unwrap();
        record.set_title(native, COMPARED).unwrap();
        assert_eq!(auto(Some(&record)), software(Reason::NotTimed, Reason::NotRecorded));

        // Software: 900 with a spread of 45. Better by 45 is not better
        // by more than the spread; slower and equal are not better at all.
        for median_us in [855, 856, 900, 901, 5000] {
            record.set_times(native, times(median_us)).unwrap();
            assert_eq!(auto(Some(&record)), software(Reason::NotFaster, Reason::NotRecorded), "{median_us}");
        }
        // The path's own spread, where it is the larger.
        let noisy = Times { median_us: 800, spread_us: 100, software_median_us: 900, software_spread_us: 45 };
        record.set_times(native, noisy).unwrap();
        assert_eq!(auto(Some(&record)).native, Some(Reason::NotFaster));

        record.set_times(native, times(854)).unwrap();
        assert_eq!(auto(Some(&record)).path, Path::Native, "better by 46");
    }

    /// Of two validated paths the hybrid is taken only where it was
    /// measured faster than the native path; else the native path, first
    /// in the order of trial; when one fails a gate, the other; software
    /// last.
    #[test]
    fn auto_takes_the_hybrid_only_where_it_was_measured_faster_than_the_native_path() {
        let both = |native_us, hybrid_us| {
            let mut record = empty();
            pass(&mut record, Path::Native, native_us);
            pass(&mut record, Path::Hybrid, hybrid_us);
            auto(Some(&record))
        };
        let native = Choice { path: Path::Native, forced: false, native: None, hybrid: Some(Reason::NativeFirst) };
        let hybrid = Choice { path: Path::Hybrid, forced: false, native: Some(Reason::Slower), hybrid: None };
        assert_eq!(both(400, 500), native);
        assert_eq!(both(500, 400), hybrid);
        assert_eq!(both(400, 400), native, "measured equal: the first in the order of trial");
        // The paths' spread is 30: a hybrid better by that or less was
        // not measured faster.
        assert_eq!(both(430, 400), native);
        assert_eq!(both(431, 400), hybrid);
        assert_eq!(both(400, 399), native);
        // The native path fails a gate: the hybrid is next, and the
        // reason is the native path's own.
        assert_eq!(
            both(880, 400),
            Choice { path: Path::Hybrid, forced: false, native: Some(Reason::NotFaster), hybrid: None }
        );
        // The hybrid fails one: the native path is taken and nothing was
        // downgraded.
        assert_eq!(both(400, 880), Choice { path: Path::Native, forced: false, native: None, hybrid: None });
        assert_eq!(both(880, 880), software(Reason::NotFaster, Reason::NotFaster));

        // The two paths were timed in two runs. Where software took
        // another time in the hybrid's run than in the native path's, the
        // two medians are of different scenes and say nothing of each
        // other: 390 against a software of 420 is not faster than 400
        // against a software of 2000.
        let runs = |native: Times, hybrid: Times| {
            let (workload, mut record) = (workload(), empty());
            pass(&mut record, Path::Native, 400);
            pass(&mut record, Path::Hybrid, 400);
            record.set_times(of(&workload, Path::Native), native).unwrap();
            record.set_times(of(&workload, Path::Hybrid), hybrid).unwrap();
            auto(Some(&record))
        };
        let run = |median_us, spread_us, software_median_us| Times {
            median_us,
            spread_us,
            software_median_us,
            software_spread_us: 0,
        };
        assert_eq!(runs(run(400, 0, 2000), run(390, 0, 420)), native);
        assert_eq!(runs(run(400, 0, 2000), run(390, 0, 2000)), hybrid, "the same run of software: comparable");
        // And a hair's difference inside a wide spread decides nothing.
        assert_eq!(runs(run(400, 150, 2000), run(399, 150, 2000)), native);
        assert_eq!(runs(run(400, 150, 2000), run(249, 150, 2000)), hybrid);
    }

    #[test]
    fn auto_gives_a_workload_without_a_variant_the_other_path_or_software() {
        let mut record = passing(Path::Native);
        pass(&mut record, Path::Hybrid, 300);
        let run = |workload: &Workload| under(Some(&record), &able_device(), &switch("auto"), workload);
        let no_native = Workload { native: None, ..workload() };
        assert_eq!(
            run(&no_native),
            Choice { path: Path::Hybrid, forced: false, native: Some(Reason::NotEligible), hybrid: None }
        );
        let no_hybrid = Workload { hybrid: None, ..workload() };
        assert_eq!(run(&no_hybrid), Choice { path: Path::Native, forced: false, native: None, hybrid: None });
        let neither = Workload { native: None, hybrid: None, ..workload() };
        assert_eq!(run(&neither), software(Reason::NotEligible, Reason::NotEligible));
        assert_eq!(workload().variant(Path::Software), None);
    }

    // The forced paths.

    #[test]
    fn a_forced_path_runs_unvalidated_on_eligible_workloads_only() {
        for (backend, path, other) in [("native", Path::Native, Path::Hybrid), ("hybrid", Path::Hybrid, Path::Native)] {
            // No record, a failing record, no accepted rule, a workload of
            // another translator: none of it is read.
            let mut failing = empty();
            let variant = *workload().variant(path).unwrap();
            failing.set_probe(key(&variant), path, Probe { passed: false, ..PASSED }).unwrap();
            let rebuilt = Workload { translator: Translator([0x78; 32]), ..workload() };
            for record in [None, Some(&failing), Some(&passing(other))] {
                for accepted in [None, Some(&STANDARD)] {
                    for workload in [workload(), rebuilt] {
                        let choice = select_under(accepted, record, &able_device(), &switch(backend), &workload);
                        assert_eq!(choice, Choice { path, forced: true, native: None, hybrid: None }, "{backend}");
                    }
                }
            }
            // A workload with no variant for the path is not eligible,
            // and the other path is not tried in its place.
            let ineligible = match path {
                Path::Native => Workload { native: None, ..workload() },
                _ => Workload { hybrid: None, ..workload() },
            };
            let choice = under(Some(&passing(other)), &able_device(), &switch(backend), &ineligible);
            assert_eq!((choice.path, choice.forced), (Path::Software, false), "{backend}");
            assert_eq!(choice.downgrades().collect::<Vec<_>>(), [(path, Reason::NotEligible)], "{backend}");
        }
    }

    #[test]
    fn a_forced_path_is_kept_to_the_class_named() {
        let class = CLASS.hex();
        let another = Workload { class: Class([0xa2; 32]), ..workload() };
        for digits in [&class[..16], &class[..]] {
            let narrowed = Switch::parse(Some("native"), Some(digits));
            let choice = select(None, &able_device(), &narrowed, &workload());
            assert_eq!(choice, Choice { path: Path::Native, forced: true, native: None, hybrid: None });
            let choice = select(None, &able_device(), &narrowed, &another);
            assert_eq!(choice, Choice { path: Path::Software, forced: false, native: Some(Reason::OtherClass), hybrid: None });
        }
        // A class that could not be read forces nothing, for any class.
        let unreadable = Switch::parse(Some("hybrid"), Some("the class"));
        for workload in [workload(), another] {
            let choice = select(None, &able_device(), &unreadable, &workload);
            assert_eq!(
                choice,
                Choice { path: Path::Software, forced: false, native: None, hybrid: Some(Reason::ClassUnreadable) }
            );
        }
        assert!(!device_under(Some(&STANDARD), &able_device(), unreadable).host_paths());
        // Under auto the class narrows nothing.
        let record = passing(Path::Native);
        let unused = Switch::parse(Some("auto"), Some(&Class([0xa2; 32]).hex()));
        assert_eq!(under(Some(&record), &able_device(), &unused, &workload()).path, Path::Native);
    }

    /// The digits name the class whose hash they begin: a class that only
    /// holds them further on is another class.
    #[test]
    fn the_class_digits_are_the_beginning_of_the_hash() {
        let mut bytes = [0x11; 32];
        bytes[8..16].copy_from_slice(&[0xab; 8]);
        let (named, holds_them_later) = (Class([0xab; 32]), Class(bytes));
        let digits = "ab".repeat(8);
        assert!(holds_them_later.hex().contains(&digits) && !holds_them_later.hex().starts_with(&digits));
        let narrowed = Switch::parse(Some("native"), Some(&digits));
        let choice = |class| select(None, &able_device(), &narrowed, &Workload { class, ..workload() });
        assert_eq!((choice(named).path, choice(named).forced), (Path::Native, true));
        assert_eq!(
            choice(holds_them_later),
            Choice { path: Path::Software, forced: false, native: Some(Reason::OtherClass), hybrid: None }
        );
        // More digits than a class shares with the ones given: another class.
        let longer = Switch::parse(Some("native"), Some(&format!("{digits}00")));
        assert_eq!(select(None, &able_device(), &longer, &Workload { class: named, ..workload() }).path, Path::Software);
    }

    #[test]
    fn the_line_says_what_was_asked_and_what_the_device_can_do() {
        let line = |caps: &HostCaps, switch: Switch| device_under(Some(&STANDARD), caps, switch).describe();
        let able = able_device();
        assert_eq!(
            line(&able, Switch::default()),
            "software (VICTORY_RT_BACKEND is unset or software; this device could run the native and hybrid paths)"
        );
        assert_eq!(
            line(&able, switch("auto")),
            "auto (VICTORY_RT_BACKEND=auto): for each workload class the fastest of the native and hybrid paths \
             with a passing validation record for this device and driver, software for every other"
        );
        assert_eq!(
            line(&able, switch("native")),
            "native forced for measurement and NOT VALIDATED (VICTORY_RT_BACKEND=native): on the workloads that \
             have a native variant, software for every other"
        );
        assert_eq!(line(&able, switch("hardware")), line(&able, switch("native")), "the older name reads as the new");
        assert_eq!(
            line(&able, Switch::parse(Some("hybrid"), Some("A1A1A1A1A1A1A1A1"))),
            "hybrid forced for measurement and NOT VALIDATED (VICTORY_RT_BACKEND=hybrid): on the workloads of \
             class a1a1a1a1a1a1a1a1 that have a hybrid variant, software for every other"
        );
        assert_eq!(
            line(&able, Switch::parse(Some("hybrid"), Some("first"))),
            "software, though VICTORY_RT_BACKEND=hybrid asked for the hybrid path: VICTORY_RT_CLASS names no class"
        );
        let old = HostCaps { raytracing_tier: 10, ..able_device() };
        assert_eq!(
            line(&old, switch("native")),
            "software, though VICTORY_RT_BACKEND=native asked for the native path: this device cannot run the \
             native or the hybrid path: ray tracing tier 1.0 (1.1 needed)"
        );
        assert_eq!(
            line(&HostCaps { raytracing_device: false, ..able_device() }, switch("auto")),
            "software (VICTORY_RT_BACKEND=auto; this device cannot run the native or the hybrid path: no \
             ID3D12Device5)"
        );
        assert_eq!(
            line(&HostCaps::default(), Switch::default()),
            "software (VICTORY_RT_BACKEND is unset or software; this device cannot run the native or the hybrid \
             path: no ray tracing, no shader model 6.6, no ID3D12Device5)"
        );
        // What the device lacks is said the same with nothing accepted;
        // the lines of a forced path and of an unset switch do not depend
        // on what is accepted.
        for (caps, asked) in [(&old, "native"), (&old, "auto"), (&able, "native"), (&able, ""), (&able, "dxr")] {
            assert_eq!(device(caps, switch(asked)).describe(), line(caps, switch(asked)), "{asked}");
        }
        for backend in ["auto", "native", "hybrid", "hardware"] {
            let on_device = device_under(Some(&STANDARD), &able, switch(backend));
            assert!(on_device.host_paths() && !on_device.fell_back(), "{backend}");
            assert_eq!(on_device.unrecorded, None);
        }
    }

    // The counters.

    #[test]
    fn every_path_and_reason_is_listed_where_it_is_numbered_and_has_its_own_name() {
        for (i, path) in Path::ALL.into_iter().enumerate() {
            assert_eq!(path as usize, i, "{path:?}");
        }
        assert_eq!(Path::ALL.map(Path::word), ["native", "hybrid", "software"]);
        assert!(Path::Native < Path::Hybrid && Path::Hybrid < Path::Software, "the order of trial");
        let mut names: Vec<&str> = Reason::ALL.iter().map(|r| r.name()).collect();
        let mut texts: Vec<String> = Reason::ALL.iter().map(Reason::to_string).collect();
        for (i, reason) in Reason::ALL.into_iter().enumerate() {
            assert_eq!(reason as usize, i, "{reason:?}");
            assert!(reason.name().bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{reason:?}");
        }
        names.sort_unstable();
        names.dedup();
        texts.sort_unstable();
        texts.dedup();
        assert_eq!((names.len(), texts.len()), (Reason::ALL.len(), Reason::ALL.len()));
        assert_eq!(Reason::ALL.len(), 24);
        // The counters take no memory but their own: under 1 KiB of statics.
        assert!(size_of::<Tally>() < 1024, "{}", size_of::<Tally>());
    }

    #[test]
    fn the_tally_counts_each_choice_by_path_and_each_downgrade_by_reason() {
        let tally = Tally::new();
        assert_eq!(tally.line(), "native=0 hybrid=0 software=0");
        assert_eq!(tally.take_changed(), None, "nothing chosen, nothing to say");

        let mut record = passing(Path::Native);
        tally.note(&auto(Some(&record)));
        tally.note(&auto(None));
        tally.note(&auto(None));
        tally.note(&auto(Some(&empty())));
        pass(&mut record, Path::Hybrid, 300);
        tally.note(&auto(Some(&record)));
        tally.note(&select(None, &able_device(), &switch("native"), &workload()));
        tally.note(&select(None, &able_device(), &switch("native"), &Workload { native: None, ..workload() }));
        tally.note(&select(None, &able_device(), &Switch::default(), &workload()));

        assert_eq!(
            (tally.selected(Path::Native), tally.selected(Path::Hybrid), tally.selected(Path::Software)),
            (2, 1, 5)
        );
        assert_eq!((tally.forced(Path::Native), tally.forced(Path::Hybrid), tally.forced(Path::Software)), (1, 0, 0));
        assert_eq!(tally.downgraded(Path::Native, Reason::NoRecord), 2);
        assert_eq!(tally.downgraded(Path::Hybrid, Reason::NoRecord), 2);
        assert_eq!(tally.downgraded(Path::Native, Reason::Slower), 1);
        assert_eq!(tally.downgraded(Path::Native, Reason::NotEligible), 1);
        assert_eq!(tally.downgraded(Path::Hybrid, Reason::NotEligible), 0);
        let line = "native=2 hybrid=1 software=5; forced and NOT VALIDATED: native=1; native not taken: \
                    not_eligible=1 no_record=2 not_recorded=1 slower=1; hybrid not taken: no_record=2 not_recorded=1";
        assert_eq!(tally.line(), line);
        assert_eq!(tally.take_changed().as_deref(), Some(line));
        assert_eq!(tally.take_changed(), None, "said once, until another choice is counted");
        tally.note(&auto(None));
        assert!(tally.take_changed().is_some_and(|line| line.starts_with("native=2 hybrid=1 software=6;")));
        // The hybrid passed over for the native path is a count of its own.
        pass(&mut record, Path::Hybrid, 400);
        tally.note(&auto(Some(&record)));
        assert_eq!(tally.downgraded(Path::Hybrid, Reason::NativeFirst), 1);
        assert!(tally.line().ends_with("; hybrid not taken: no_record=3 not_recorded=1 native_first=1"), "{}", tally.line());

        // Every downgrade a choice returns is one count, and no other.
        let total: u64 = [Path::Native, Path::Hybrid]
            .into_iter()
            .flat_map(|path| Reason::ALL.map(|reason| tally.downgraded(path, reason)))
            .sum();
        assert_eq!(total, [0, 2, 2, 2, 1, 0, 1, 0, 2, 1].iter().sum::<u64>(), "by choice, in the order counted");
        assert!(Reason::ALL.into_iter().all(|reason| tally.downgraded(Path::Software, reason) == 0));
    }

    /// The run's counts, through [`choose`]. No other test calls it, and
    /// nothing in a run does yet. It chooses as this build does: under
    /// `auto` nothing is accepted, so nothing is selected.
    #[test]
    fn a_choice_made_for_a_run_is_counted_in_the_runs_tally() {
        assert_eq!(take_changed(), None, "nothing chosen for yet");
        let forced = choose(None, &able_device(), &switch("hybrid"), &workload());
        assert_eq!((forced.path, forced.forced), (Path::Hybrid, true));
        let mut record = passing(Path::Native);
        pass(&mut record, Path::Hybrid, 300);
        let fell = choose(Some(&record), &able_device(), &switch("auto"), &workload());
        assert_eq!(fell, software(Reason::NoRule, Reason::NoRule));
        assert_eq!(
            take_changed().as_deref(),
            Some(
                "native=0 hybrid=1 software=1; forced and NOT VALIDATED: hybrid=1; native not taken: no_rule=1; \
                 hybrid not taken: no_rule=1"
            )
        );
        assert_eq!(take_changed(), None);
        assert_eq!((tally().selected(Path::Hybrid), tally().forced(Path::Hybrid)), (1, 1));
    }
}
