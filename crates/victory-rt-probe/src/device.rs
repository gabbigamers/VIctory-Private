//! What the run asks of a device: pipelines of the three shaders, host
//! structures built from the bytes of [`crate::inputs`], a dispatch over a
//! ray list, the hit records back, and what it holds in memory. Plain data
//! both ways, so that the run's logic is the same over Direct3D 12
//! (`crate::hardware`, Windows only) and over the stand-in the host tests
//! use.
//!
//! Every time a device reports is of one kind and stays apart from the
//! others: the card's own time of a marked region (timestamps, [`Gpu`] and
//! [`Traced::gpu_ms`]), the processor's time uploading and recording, and
//! the time the submitting thread was blocked until the card had run a
//! submission.

use std::time::Duration;

use victory_gpu::rt::desc::BuildFlags;
use victory_gpu::rt::select::Request;
use victory_shader::emit::rayquery::{Form, RayQueryShader};

use crate::inputs::{BoxInputs, TopInstance, TriangleInputs};
use crate::scene::Action;

/// A path that traces: which host structures, and which shader over them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    /// Host triangles; the device intersects (`Form::Triangles`).
    Native,
    /// One host box around each triangle; the shader intersects
    /// (`Form::Procedural`), reading the triangles from the triangle data.
    Hybrid,
}

impl Path {
    /// In the order a refused ray goes down: native, then hybrid.
    pub const ALL: [Path; 2] = [Path::Native, Path::Hybrid];

    /// The name in a result file.
    pub fn name(self) -> &'static str {
        match self {
            Path::Native => "native",
            Path::Hybrid => "hybrid",
        }
    }

    pub fn form(self) -> Form {
        match self {
            Path::Native => Form::Triangles,
            Path::Hybrid => Form::Procedural,
        }
    }

    /// Whether the path's shader writes front or back. The procedural
    /// shader writes kind 3 for every hit: its record has no place for
    /// the face.
    pub fn reports_face(self) -> bool {
        self == Path::Native
    }

    /// The path a ray this one refuses goes to, where one is built here.
    /// The hybrid's next is the software traversal on the GPU, which is
    /// not built.
    pub fn next(self) -> Option<Path> {
        match self {
            Path::Native => Some(Path::Hybrid),
            Path::Hybrid => None,
        }
    }
}

/// The name of a shader form in a result file and on the console.
pub fn form_name(form: Form) -> &'static str {
    match form {
        Form::Triangles => "triangles",
        Form::Procedural => "procedural",
        Form::Null => "null",
    }
}

/// One bottom level's bytes, for the path that reads them.
#[derive(Clone, Copy, Debug)]
pub enum Level<'a> {
    Triangles(&'a TriangleInputs),
    Boxes(&'a BoxInputs),
}

/// One path's structures as a frame leaves them, and what to do to get
/// there from the frame before.
#[derive(Clone, Debug)]
pub struct Structures<'a> {
    pub path: Path,
    /// Each bottom level in the scene's order with what is done to it.
    /// `Keep` hands nothing to the device; `Refit` asks the backend for an
    /// update in place, which it grants or turns into a build, and says
    /// which ([`Built`]); `Rebuild` is a build.
    pub bottoms: Vec<(Level<'a>, Action)>,
    pub instances: &'a [TopInstance],
    pub top_flags: BuildFlags,
    pub top: Action,
    /// The hybrid path's triangle data, where the frame changed it (and
    /// always with the first build): what the procedural shader reads.
    pub triangle_data: Option<&'a [u8]>,
}

/// The card's time of some regions of one kind in one submission.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gpu {
    /// The sum over the regions that have a time, milliseconds.
    pub ms: f64,
    /// Regions that have a time.
    pub timed: u32,
    /// Regions without one (no free query, a region cut by a submission,
    /// timestamps that make no interval). With any, `ms` is not the whole.
    pub untimed: u32,
}

impl Gpu {
    pub fn add(&mut self, gpu_ms: Option<f64>) {
        match gpu_ms {
            Some(ms) => {
                self.ms += ms;
                self.timed += 1;
            }
            None => self.untimed += 1,
        }
    }

    /// The whole time, where every region has one and there is a region.
    pub fn whole(&self) -> Option<f64> {
        (self.timed > 0 && self.untimed == 0).then_some(self.ms)
    }
}

/// What bringing a path's structures to a frame did and cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Built {
    /// Processor time making upload memory and copying the inputs in.
    pub upload_cpu: Duration,
    /// Processor time describing the builds to the backend and recording
    /// them (the device's size queries among it), up to the submission.
    pub record_cpu: Duration,
    /// Time blocked from the submission until the card had run it.
    pub wait: Duration,
    /// The card's time, by what the backend did: bottom levels built,
    /// bottom levels refitted, the top level built, the top level
    /// refitted. Each structure is a region of its own, with its barriers.
    pub bottom_build: Gpu,
    pub bottom_refit: Gpu,
    pub top_build: Gpu,
    pub top_refit: Gpu,
    /// Structures built and refitted, as the backend decided.
    pub builds: u32,
    pub refits: u32,
    /// Bytes uploaded for this frame's builds.
    pub input_bytes: u64,
    /// Where the backend did other than asked (a refit turned into a
    /// build, with its reason), one line each.
    pub notes: Vec<String>,
}

/// One dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trace {
    pub form: Form,
    /// The path whose top level (and triangle data) is bound; `None` for
    /// the null form, which binds neither.
    pub path: Option<Path>,
    /// The rays listed: the first so many of the uploaded list.
    pub rays: u32,
    /// Before the dispatch, overwrite the hit records it is to write with
    /// [`POISON`], so that a record no thread wrote reads as that and not
    /// as what an earlier dispatch left there. The dispatches whose records
    /// are read are poisoned; the timed ones are not, so that a timed
    /// submission holds the dispatch and nothing else.
    pub poison: bool,
}

/// What a hit record holds where no thread wrote it since it was
/// poisoned, in each of its eight words. As a kind it has bit 31, which no
/// hit and no miss has, and it is not what a shader writes for a refusal:
/// such a record is counted as unwritten and judged as a record that is no
/// hit and no miss, a defect (`crate::records`). It is never a miss, never
/// a refusal, and never what an earlier dispatch left there.
pub const POISON: u32 = 0xA5C3_F0F0;

// No hit and no miss has bit 31 in its kind: the poison must.
const _: () = assert!(POISON & 0x8000_0000 != 0);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Traced {
    /// The card's time between a timestamp before the dispatch and one
    /// after it, milliseconds; `None` where it could not be timed.
    pub gpu_ms: Option<f64>,
    /// Processor time recording the dispatch, up to the submission.
    pub record_cpu: Duration,
    /// Time blocked from the submission until the card had run it.
    pub wait: Duration,
}

/// What is held now. The three owners are this process's own counts
/// (`victory_gpu::account`): bytes asked for, rounded as the backend
/// rounds them. The adapter's and the process's figures are the system's,
/// of the whole process, for a cross-check only.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Memory {
    /// `own_rt_structures_mb`'s gauge: structures' result memory, on the
    /// card.
    pub structures: u64,
    /// `own_rt_scratch_mb`'s: build scratch and the hit buffer, on the card.
    pub scratch: u64,
    /// `own_rt_buffers_mb`'s: uploads, instance descriptions, read-backs;
    /// system memory the card reads.
    pub buffers: u64,
    /// The adapter's figure for this process on the card, and its budget
    /// there; MB. `None` where the adapter does not say.
    pub video_local_mb: Option<f64>,
    pub video_budget_mb: Option<f64>,
    /// The same in system memory the card reads.
    pub video_shared_mb: Option<f64>,
    /// The process's private commit and working set, bytes; `None` where
    /// the system does not say.
    pub commit: Option<u64>,
    pub working_set: Option<u64>,
}

/// Why there is no device to run on. A run that ends here has run nothing
/// and writes no file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// `VICTORY_RT_BACKEND` asks for no path over host structures: what it
    /// holds, as the selection's own parser reads it
    /// (`victory_gpu::rt::select`). The probe opens the backend behind the
    /// same switch a run does.
    NotAsked(String),
    /// The Direct3D 12 debug layer was asked for and could not be turned
    /// on. The run is not made without it: it was asked for by name.
    NoDebugLayer(String),
    /// Direct3D 12 gave no device on the default adapter.
    NoDevice(String),
    /// The device lacks what inline ray queries need: what it reports, and
    /// what is missing.
    Lacks { adapter: String, lacks: Vec<String> },
    /// The backend did not open for another reason.
    Backend(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unavailable::NotAsked(holds) => write!(
                f,
                "the experimental host ray-tracing backend is off: {SWITCH} {holds}. The probe runs only with \
                 {SWITCH}=native, hybrid or auto set for this command (hardware is the older name of native), and \
                 measures both the native and the hybrid path under any of them"
            ),
            Unavailable::NoDebugLayer(why) => write!(
                f,
                "the Direct3D 12 debug layer was asked for (--debug-layer) and is not available: {why}. It comes \
                 with the optional Windows feature \"Graphics Tools\""
            ),
            Unavailable::NoDevice(why) => write!(f, "no Direct3D 12 device on the default adapter: {why}"),
            Unavailable::Lacks { adapter, lacks } => write!(
                f,
                "{adapter} cannot run inline ray queries: {} (ray tracing tier 1.1, shader model 6.6 and \
                 ID3D12Device5 are needed)",
                lacks.join(", ")
            ),
            Unavailable::Backend(why) => write!(f, "the host ray-tracing backend did not open: {why}"),
        }
    }
}

/// The switch the host ray-tracing backend is behind, for a run and for
/// the probe.
pub const SWITCH: &str = victory_gpu::rt::select::VARIABLE;

/// Whether the switch, holding `value`, lets the probe open the host
/// backend, read through the selection's own parser (`Request::parse`):
/// `native`, `hybrid` and `auto` do, and `hardware`, the older name of
/// `native`. Otherwise what the switch holds, for the line that says why
/// nothing ran. Unset is the software path, as in a run.
///
/// The request says whether the probe runs, never which path it runs: it
/// measures both under any of those values ([`opened`] says why).
/// `VICTORY_RT_CLASS` is not read: it narrows a path forced in a run.
pub fn asked(value: Option<&str>) -> Result<Request, String> {
    match Request::parse(value) {
        Ok(Request::Software) => Err(match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(_) => "is software".into(),
            None => "is not set, which is software".into(),
        }),
        Ok(request) => Ok(request),
        Err(unknown) => {
            Err(format!("holds {unknown:?}, which is not software, auto, native or hybrid and is read as software"))
        }
    }
}

/// What a result file's header says of the switch (`rt_backend_switch`):
/// what it asks for, as read (`hardware` is written `native`), and that
/// the probe ran both paths whatever it names. A forced path is a run's
/// matter, one workload at a time. The probe is a comparison: a ray the
/// native path refuses is judged by what the hybrid path answered for it
/// ([`Path::next`]), and the two paths' times are of one run, against
/// one null dispatch. It selects no path, under `auto` either.
pub fn opened(request: Request) -> String {
    let asks = match request {
        Request::Software => "software",
        Request::Auto => "auto",
        Request::Native => "native",
        Request::Hybrid => "hybrid",
    };
    format!(
        "{SWITCH}={asks}: the probe opened the host backend because the switch asks for a path over host \
         structures; it measures the native and the hybrid path both, whichever the switch names, and selects \
         neither"
    )
}

/// [`asked`] of what the environment holds: the request, or why nothing
/// is to be opened.
pub fn asked_by_environment() -> Result<Request, Unavailable> {
    let value = std::env::var_os(SWITCH).map(|value| value.to_string_lossy().into_owned());
    asked(value.as_deref()).map_err(Unavailable::NotAsked)
}

pub trait Device {
    /// What the device says of itself, for the file's header: `key`,
    /// `value` pairs (adapter, driver, tier). For the report only: nothing
    /// in a run is decided by them.
    fn identity(&self) -> Vec<(String, String)>;

    /// Makes the pipeline of `shader`; the processor time it took, the
    /// driver's compile among it.
    fn pipeline(&mut self, shader: &RayQueryShader) -> Result<Duration, String>;

    /// Takes `list` (ray records, `crate::rays::list`) as the rays every
    /// later dispatch reads, and makes room for one hit record per ray.
    /// The processor time of the upload.
    fn rays(&mut self, list: &[u8]) -> Result<Duration, String>;

    /// Brings a path's structures to a frame, submits and waits.
    fn build(&mut self, structures: &Structures) -> Result<Built, String>;

    /// One dispatch in a submission of its own, waited for.
    fn trace(&mut self, trace: &Trace) -> Result<Traced, String>;

    /// The first `records` hit records as the last dispatch left them, and
    /// the time blocked for the copy.
    fn hits(&mut self, records: u32) -> Result<(Vec<u8>, Duration), String>;

    /// Gives up a path's structures and inputs, and waits until their
    /// memory is released.
    fn release(&mut self, path: Path) -> Result<(), String>;

    fn memory(&mut self) -> Memory;

    /// The backend's own counters, in a line: what it did and refused.
    fn counters(&self) -> String;

    /// What a validation layer of the device said since this was last
    /// asked, a message a line; nothing where no layer is on, which is
    /// every run that measures.
    fn messages(&mut self) -> Vec<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sum_of_regions_is_whole_only_when_each_has_a_time() {
        let mut gpu = Gpu::default();
        assert_eq!(gpu.whole(), None, "no region");
        gpu.add(Some(1.5));
        gpu.add(Some(0.25));
        assert_eq!(gpu.whole(), Some(1.75));
        gpu.add(None);
        assert_eq!((gpu.whole(), gpu.timed, gpu.untimed), (None, 2, 1));
    }

    #[test]
    fn a_refused_ray_goes_from_native_to_hybrid_and_no_further_here() {
        assert_eq!(Path::Native.next(), Some(Path::Hybrid));
        assert_eq!(Path::Hybrid.next(), None);
        assert_eq!(Path::ALL.map(Path::name), ["native", "hybrid"]);
        assert!(Path::Native.reports_face() && !Path::Hybrid.reports_face());
        assert_eq!(Path::Hybrid.form(), Form::Procedural);
    }

    #[test]
    fn a_device_that_cannot_run_the_probe_is_named_with_what_it_lacks() {
        let line = Unavailable::Lacks {
            adapter: "Some Adapter".into(),
            lacks: vec!["ray tracing tier 1.0 (1.1 needed)".into(), "it reports ray tracing tier: 1.0".into()],
        }
        .to_string();
        assert_eq!(
            line,
            "Some Adapter cannot run inline ray queries: ray tracing tier 1.0 (1.1 needed), it reports ray tracing \
             tier: 1.0 (ray tracing tier 1.1, shader model 6.6 and ID3D12Device5 are needed)"
        );
        assert!(Unavailable::NoDevice("E_FAIL".into()).to_string().starts_with("no Direct3D 12 device"));
        assert!(Unavailable::Backend("no fence".into()).to_string().contains("did not open: no fence"));
        assert!(Unavailable::NoDebugLayer("E_FAIL".into()).to_string().contains("--debug-layer"));
    }

    /// The probe is behind the switch a run is behind, read through the
    /// selection's parser: with it unset, or holding anything but
    /// `native`, `hybrid`, `auto` or the older name `hardware`, nothing is
    /// opened.
    #[test]
    fn the_backend_is_opened_only_where_the_switch_asks_for_it() {
        assert_eq!(SWITCH, "VICTORY_RT_BACKEND");
        assert_eq!(asked(Some("native")), Ok(Request::Native));
        assert_eq!(asked(Some("hybrid")), Ok(Request::Hybrid));
        assert_eq!(asked(Some("hardware")), Ok(Request::Native), "the older name of native");
        assert_eq!(asked(Some(" Auto ")), Ok(Request::Auto));
        assert_eq!(asked(None).unwrap_err(), "is not set, which is software");
        assert_eq!(asked(Some("")).unwrap_err(), "is not set, which is software");
        assert_eq!(asked(Some("software")).unwrap_err(), "is software");
        // What the selection's parser does not name is software, as in a
        // run, and the line holds the value.
        for other in ["1", "on", "native,hybrid", "both"] {
            assert_eq!(
                asked(Some(other)).unwrap_err(),
                format!("holds {other:?}, which is not software, auto, native or hybrid and is read as software")
            );
        }
        // The probe reads the switch as the selection does: every value
        // that asks a run for a path over host structures opens it, and no
        // other.
        for value in [None, Some(""), Some("software"), Some("auto"), Some("native"), Some("hybrid"), Some("hardware")]
        {
            let run = victory_gpu::rt::select::Switch::parse(value, None).request;
            assert_eq!(asked(value).ok(), Some(run).filter(|r| *r != Request::Software), "{value:?}");
        }
        let line = Unavailable::NotAsked(asked(None).unwrap_err()).to_string();
        assert_eq!(
            line,
            "the experimental host ray-tracing backend is off: VICTORY_RT_BACKEND is not set, which is software. \
             The probe runs only with VICTORY_RT_BACKEND=native, hybrid or auto set for this command (hardware is \
             the older name of native), and measures both the native and the hybrid path under any of them"
        );
    }

    /// Whichever value opened the backend, the file says the probe ran
    /// both paths: the switch's value limits the run to none of them.
    #[test]
    fn the_file_says_what_the_switch_asked_and_that_both_paths_ran() {
        for (value, word) in [("native", "native"), ("hybrid", "hybrid"), ("hardware", "native"), ("auto", "auto")] {
            let line = opened(asked(Some(value)).unwrap());
            assert!(line.starts_with(&format!("VICTORY_RT_BACKEND={word}: ")), "{line}");
            assert!(
                line.ends_with("the native and the hybrid path both, whichever the switch names, and selects neither")
            );
        }
        assert_eq!(Path::ALL, [Path::Native, Path::Hybrid], "the paths a run measures, under every value");
    }

    /// One number for the probe's version: this crate's is the one
    /// `victory-gpu` defines beside what the selection accepts verdicts of
    /// the probe by, and a result file names that number.
    #[test]
    fn the_probes_version_is_the_one_the_selection_knows() {
        assert_eq!(crate::PROBE_VERSION, victory_gpu::rt::select::PROBE_VERSION);
    }

    #[test]
    fn a_poisoned_record_is_unwritten_and_is_judged_not_refused() {
        use crate::records::{Answer, is_refusal, is_unwritten, refusals};
        assert!(is_unwritten(&[POISON; 8]) && !is_refusal(&[POISON; 8]));
        let answer = Answer::of_device([POISON; 8]);
        assert_eq!(answer.answered(), Some(&[POISON; 8]), "it stays among the records that are judged");
        assert!(!answer.took());
        let counted = refusals(&[answer], None);
        assert_eq!((counted.unwritten, counted.malformed, counted.geometry, counted.range), (1, 0, 0, 0));
        assert_eq!((counted.taken_by_next, counted.unanswered), (0, 0), "no refusal: it is for no next path");
    }
}
