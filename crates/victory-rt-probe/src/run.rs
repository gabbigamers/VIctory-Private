//! The run: every class, the null shader and each tracing path over each
//! frame, on a [`Device`]; the answers judged against the CPU's; a row
//! for each.
//!
//! Per class, with its rays mapped to what the shaders take and uploaded
//! as one list (`crate::rays`):
//!
//! 1. **The null shader**: one dispatch over the listed rays (every
//!    record must be a miss), then the timed dispatches, then the check
//!    (below). Its times stand beside each tracing path's.
//! 2. **Each tracing path** (native, then hybrid), one after the other so
//!    that the memory counted is one path's: per frame the scene is
//!    converted, the structures are built or refitted as the frame asks,
//!    one dispatch lists every ray once over poisoned records and its
//!    records are read back (the answers), then the timed dispatches list
//!    `timed_rays` records each, then the check. Then the loop mode, if
//!    asked for, with a check of its own. Then the path's structures are
//!    given up.
//! 3. **The verdicts**: each path's answers per frame against the CPU
//!    reference and the CPU model (`crate::adapter`), and its refusals
//!    counted against the next path's answers for the same rays.
//!
//! **The check.** A timed dispatch is a submission that holds the dispatch
//! and nothing else, so its records are not poisoned first, and what is in
//! the hit buffer after it may be what an earlier dispatch left. That it
//! did the work its time is reported for is therefore shown by one more
//! dispatch after the series: the same shader, structures and count of
//! records, over poisoned records, in no time of the file. Its records are
//! read back and compared with the answers, so a dispatch of `timed_rays`
//! records that leaves one unwritten, or writes another answer, is
//! counted. What it cannot show is that a timed dispatch, which differs
//! from it by the copy that poisons, did the same.
//!
//! A step that fails on the device fails its row, with what failed in the
//! row's note; the rows after it for that path and class are `not run`;
//! the run goes on with the next path and class. Nothing is retried, and
//! nothing a device did not do is filled in.

use std::time::{Duration, Instant};

use victory_shader::emit::rayquery::{self, Form, RayQueryShader};

use crate::adapter::{self, Case, Truth};
use crate::device::{Built, Device, Level, Memory, Path, Structures, Trace, form_name};
use crate::file::{Mismatch, Results, Table};
use crate::inputs::{self, BottomInputs, PathInputs};
use crate::plan::{self, ClassSize, Options};
use crate::rays::{self, Mapped};
use crate::records::{self, Answer};
use crate::row::{self, Row};
use crate::scene::{Actions, HitWords, Scene};
use crate::stats;

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1e3
}

/// Megabytes of 10^6 bytes, as the adapter's figures are given.
fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

fn median(values: &[f64]) -> Option<f64> {
    stats::figures(values).map(|f| f.p50)
}

/// The digest a DXIL container carries in its header (bytes 4 to 20).
pub fn container_hash(dxil: &[u8]) -> String {
    dxil.get(4..20).map(|digest| digest.iter().map(|b| format!("{b:02x}")).collect()).unwrap_or_default()
}

/// A series of timed dispatches: the card's time of each, and the time
/// blocked for each.
#[derive(Default)]
struct Series {
    gpu: Vec<Option<f64>>,
    waits: Vec<f64>,
}

fn timed_series(device: &mut dyn Device, trace: &Trace, options: &Options) -> Result<Series, String> {
    let mut series = Series::default();
    for _ in 0..options.discard + options.repeats {
        let traced = device.trace(trace)?;
        series.gpu.push(traced.gpu_ms);
        series.waits.push(ms(traced.wait));
    }
    Ok(series)
}

/// Writes a series' figures into `row`, the first `discard` left out; the
/// figures, where a dispatch kept has a time.
fn put_series(row: &mut Row, series: &Series, discard: u32) -> Option<stats::Figures> {
    let (kept, untimed) = stats::kept(&series.gpu, discard as usize);
    row.count("trace_samples", kept.len() as u64).count("trace_untimed", u64::from(untimed));
    let figures = stats::figures(&kept);
    if let Some(f) = &figures {
        row.number("trace_gpu_ms_min", Some(f.min))
            .number("trace_gpu_ms_p50", Some(f.p50))
            .number("trace_gpu_ms_p95", Some(f.p95))
            .number("trace_gpu_ms_max", Some(f.max));
    }
    row.number("trace_wait_ms_p50", median(series.waits.get(discard as usize..).unwrap_or(&[])));
    figures
}

fn put_memory(row: &mut Row, before: &Memory, after: &Memory) {
    row.count("own_rt_structures_bytes", after.structures)
        .count("own_rt_scratch_bytes", after.scratch)
        .count("own_rt_buffers_bytes", after.buffers)
        .number("video_local_mb_before", before.video_local_mb)
        .number("video_local_mb_after", after.video_local_mb)
        .number("video_budget_mb", after.video_budget_mb)
        .number("video_shared_mb_before", before.video_shared_mb)
        .number("video_shared_mb_after", after.video_shared_mb)
        .number("commit_mb_before", before.commit.map(mb))
        .number("commit_mb_after", after.commit.map(mb))
        .number("working_set_mb_before", before.working_set.map(mb))
        .number("working_set_mb_after", after.working_set.map(mb));
}

fn put_scene(row: &mut Row, scene: &Scene) {
    row.count("bottoms", scene.bottoms.len() as u64)
        .count("instances", scene.instances.len() as u64)
        .count("primitives", scene.primitives() as u64);
}

/// What a validation layer of the device said since it was last asked:
/// each message to the console, and how many with the first of them in the
/// row's note.
fn heard(device: &mut dyn Device, row: &mut Row, say: &mut dyn FnMut(&str)) {
    let messages = device.messages();
    let Some(first) = messages.first() else { return };
    let frame = if row.get("frame").is_empty() { String::new() } else { format!(" frame {}", row.get("frame")) };
    for message in &messages {
        say(&format!("validation layer, {} {}{frame}: {message}", row.get("class"), row.get("path")));
    }
    row.note(&format!("the validation layer said {} messages, the first: {first}", messages.len()));
}

fn put_rays(row: &mut Row, case: &Case, mapped: &Mapped) {
    row.count("rays", case.rays.len() as u64)
        .count("rays_listed", mapped.records.len() as u64)
        .count("skip_closest_hit_cleared", u64::from(mapped.skip_closest_hit_cleared))
        .count("mask_high_bits_cut", u64::from(mapped.mask_high_bits_cut))
        .count("refused_before", mapped.refused.len() as u64)
        .text("refused_before_reasons", &mapped.refusals_by_reason());
}

fn put_built(row: &mut Row, built: &Built) {
    row.count("builds_done", u64::from(built.builds))
        .count("refits_done", u64::from(built.refits))
        .number("build_gpu_ms", built.bottom_build.whole())
        .number("refit_gpu_ms", built.bottom_refit.whole())
        .number("tlas_build_gpu_ms", built.top_build.whole())
        .number("tlas_refit_gpu_ms", built.top_refit.whole())
        .count(
            "build_untimed",
            u64::from(
                built.bottom_build.untimed
                    + built.bottom_refit.untimed
                    + built.top_build.untimed
                    + built.top_refit.untimed,
            ),
        )
        .number("upload_cpu_ms", Some(ms(built.upload_cpu)))
        .number("record_cpu_ms", Some(ms(built.record_cpu)))
        .count("input_bytes", built.input_bytes)
        .number("build_wait_ms", Some(ms(built.wait)));
    for note in &built.notes {
        row.note(note);
    }
}

/// A scene as one path's inputs.
fn convert(path: Path, scene: &Scene) -> Result<PathInputs, String> {
    match path {
        Path::Native => PathInputs::triangles(scene),
        Path::Hybrid => PathInputs::boxes(scene),
    }
}

/// Hands the device what `steps` say of `next`.
fn build(
    device: &mut dyn Device,
    path: Path,
    next: &PathInputs,
    steps: &inputs::Steps,
) -> Result<Built, String> {
    let bottoms = next
        .bottoms
        .iter()
        .zip(&steps.bottoms)
        .map(|(bottom, action)| {
            let level = match bottom {
                BottomInputs::Triangles(t) => Level::Triangles(t),
                BottomInputs::Boxes(b) => Level::Boxes(b),
            };
            (level, *action)
        })
        .collect();
    device.build(&Structures {
        path,
        bottoms,
        instances: &next.instances,
        top_flags: next.top_flags,
        top: steps.top,
        triangle_data: next.triangle_data.as_deref().filter(|_| steps.triangle_data),
    })
}

/// What one path did over one class.
struct PathRun {
    /// A row per frame, the measurements in; the verdict's columns are
    /// filled when every path has run.
    rows: Vec<Row>,
    /// Per frame, the answers of the dispatch that listed every ray once,
    /// where it ran and its records came back.
    answers: Vec<Option<Vec<Answer>>>,
    loop_row: Option<Row>,
}

/// What a run keeps of the things that are the same for every class.
struct Shared<'a> {
    options: &'a Options,
    shaders: &'a [RayQueryShader; 3],
    /// Per shader: whether its pipeline was made, and the processor time
    /// it took until a row has reported it.
    pipelines: [Result<Option<Duration>, String>; 3],
}

fn shader_place(form: Form) -> usize {
    match form {
        Form::Null => 0,
        Form::Triangles => 1,
        Form::Procedural => 2,
    }
}

impl Shared<'_> {
    /// Marks `row` with the shader's container and, the first time, the
    /// pipeline's time. Where the pipeline was not made, why.
    fn pipeline(&mut self, form: Form, row: &mut Row) -> Result<(), String> {
        let at = shader_place(form);
        row.text("container_hash", &container_hash(&self.shaders[at].dxil));
        match &mut self.pipelines[at] {
            Ok(time) => {
                row.number("pipeline_cpu_ms", time.take().map(ms));
                Ok(())
            }
            Err(why) => Err(format!("the {} pipeline was not made: {why}", form_name(form))),
        }
    }
}

fn fail(row: &mut Row, step: &str, why: &str) {
    row.text("status", row::FAILED).note(&format!("{step}: {why}"));
}

/// The null shader over one class: its row, and the figures of its timed
/// dispatches for the tracing rows to hold beside their own. `list` is the
/// class's ray list as it was uploaded: its bytes and the processor time
/// of the upload, which this row holds.
fn null_pass(
    device: &mut dyn Device,
    shared: &mut Shared,
    case: &Case,
    mapped: &Mapped,
    list: (u64, Duration),
) -> (Row, Option<stats::Figures>) {
    let options = shared.options;
    let mut row = Row::new(&case.class, case.seed, "", row::NULL);
    put_rays(&mut row, case, mapped);
    row.count("timed_rays", u64::from(options.timed_rays));
    row.count("input_bytes", list.0).number("upload_cpu_ms", Some(ms(list.1)));
    let listed = mapped.records.len() as u32;
    let mut figures = None;
    let ran = (|| -> Result<(), String> {
        shared.pipeline(Form::Null, &mut row)?;
        let not_miss = |bytes: &[u8]| records::words(bytes).iter().filter(|r| **r != [0; 8]).count() as u64;
        let once = Trace { form: Form::Null, path: None, rays: listed, poison: true };
        device.trace(&once).map_err(|e| format!("the dispatch over the listed rays: {e}"))?;
        let (bytes, wait) = device.hits(listed).map_err(|e| format!("reading the records back: {e}"))?;
        row.number("readback_wait_ms", Some(ms(wait)));
        let mut wrong = not_miss(&bytes);
        let timed = Trace { rays: options.timed_rays, poison: false, ..once };
        let series = timed_series(device, &timed, options).map_err(|e| format!("a timed dispatch: {e}"))?;
        figures = put_series(&mut row, &series, options.discard);
        // The check: what a timed dispatch lists, over poisoned records.
        let check = Trace { poison: true, ..timed };
        device.trace(&check).map_err(|e| format!("the dispatch that checks the timed ones: {e}"))?;
        let (bytes, _) = device
            .hits(options.timed_rays)
            .map_err(|e| format!("reading the checking dispatch's records back: {e}"))?;
        wrong += not_miss(&bytes);
        row.count("null_not_miss", wrong);
        Ok(())
    })();
    match ran {
        Ok(()) => {
            row.text("status", row::RAN);
        }
        Err(why) => fail(&mut row, "null shader", &why),
    }
    (row, figures)
}

/// One frame of one path: builds, the dispatch that lists every ray once,
/// its records, the timed dispatches. The answers, where they came back.
#[allow(clippy::too_many_arguments)]
fn frame_pass(
    device: &mut dyn Device,
    options: &Options,
    case: &Case,
    mapped: &Mapped,
    path: Path,
    frame: usize,
    built: &mut Option<PathInputs>,
    row: &mut Row,
    answers: &mut Option<Vec<Answer>>,
) -> Result<(), String> {
    let scene = &case.frames[frame].scene;
    let asked = &case.frames[frame].actions;
    let listed = mapped.records.len() as u32;

    let started = Instant::now();
    let next = convert(path, scene).map_err(|e| format!("converting the scene: {e}"))?;
    row.number("convert_cpu_ms", Some(ms(started.elapsed())));
    if path == Path::Hybrid {
        row.count("flat_boxes", u64::from(next.flat_boxes));
    }
    let (refits, builds) = asked.asked();
    row.count("refits_asked", u64::from(refits)).count("builds_asked", u64::from(builds));
    let steps = inputs::steps(built.as_ref(), &next, asked);
    for note in &steps.notes {
        row.note(note);
    }
    let done = build(device, path, &next, &steps).map_err(|e| format!("building the structures: {e}"))?;
    put_built(row, &done);
    *built = Some(next);

    let once = Trace { form: path.form(), path: Some(path), rays: listed, poison: true };
    device.trace(&once).map_err(|e| format!("the dispatch over the listed rays: {e}"))?;
    let (bytes, wait) = device.hits(listed).map_err(|e| format!("reading the records back: {e}"))?;
    row.number("readback_wait_ms", Some(ms(wait)));
    let first = records::words(&bytes);
    *answers = Some(records::answers(case.rays.len(), &mapped.source, &first)?);

    let timed = Trace { rays: options.timed_rays, poison: false, ..once };
    let series = timed_series(device, &timed, options).map_err(|e| format!("a timed dispatch: {e}"))?;
    put_series(row, &series, options.discard);
    // The check: what a timed dispatch lists, over poisoned records, so
    // that a record it leaves unwritten differs from the answer.
    let check = Trace { poison: true, ..timed };
    device.trace(&check).map_err(|e| format!("the dispatch that checks the timed ones: {e}"))?;
    let (bytes, _) = device
        .hits(options.timed_rays)
        .map_err(|e| format!("reading the checking dispatch's records back: {e}"))?;
    row.count("repeat_differing", u64::from(records::differing(&first, &records::words(&bytes))));
    Ok(())
}

/// The loop mode of one path over one class, from the state its last
/// frame left: iteration `i` brings the structures to frame `i` modulo
/// the frames (the return to the first frame builds what differs) and
/// lists every ray once.
#[allow(clippy::too_many_arguments)]
fn loop_pass(
    device: &mut dyn Device,
    options: &Options,
    case: &Case,
    mapped: &Mapped,
    path: Path,
    built: &mut Option<PathInputs>,
    first: &[Option<Vec<Answer>>],
    row: &mut Row,
) -> Result<(), String> {
    let frames = case.frames.len();
    let listed = mapped.records.len() as u32;
    let trace = Trace { form: path.form(), path: Some(path), rays: listed, poison: false };
    let (mut iterations, mut series) = (Vec::new(), Series::default());
    let (mut convert_ms, mut upload_ms, mut record_ms, mut build_wait) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut gpu: [Vec<f64>; 4] = Default::default();
    let (mut builds, mut refits, mut untimed, mut input_bytes) = (0u64, 0u64, 0u64, 0u64);
    let mut last = 0;
    for i in 0..options.loops as usize {
        let frame = i % frames;
        last = frame;
        let started = Instant::now();
        // A case of one frame changes nothing between iterations: there
        // is nothing to convert and nothing to build.
        if frames > 1 {
            let next = convert(path, &case.frames[frame].scene)
                .map_err(|e| format!("iteration {i}: converting the scene: {e}"))?;
            convert_ms.push(ms(started.elapsed()));
            let asked = match (frame, built.as_ref()) {
                (0, Some(built)) => inputs::actions_between(built, &next),
                (0, None) => Actions::build_all(next.bottoms.len()),
                _ => case.frames[frame].actions.clone(),
            };
            let steps = inputs::steps(built.as_ref(), &next, &asked);
            let done = build(device, path, &next, &steps)
                .map_err(|e| format!("iteration {i}: building the structures: {e}"))?;
            *built = Some(next);
            upload_ms.push(ms(done.upload_cpu));
            record_ms.push(ms(done.record_cpu));
            build_wait.push(ms(done.wait));
            for (kind, sum) in [done.bottom_build, done.bottom_refit, done.top_build, done.top_refit]
                .iter()
                .enumerate()
            {
                gpu[kind].extend(sum.whole());
                untimed += u64::from(sum.untimed);
            }
            builds += u64::from(done.builds);
            refits += u64::from(done.refits);
            input_bytes += done.input_bytes;
        }
        let traced = device.trace(&trace).map_err(|e| format!("iteration {i}: the dispatch: {e}"))?;
        iterations.push(ms(started.elapsed()));
        series.gpu.push(traced.gpu_ms);
        series.waits.push(ms(traced.wait));
    }
    row.count("loop_iterations", iterations.len() as u64).count("timed_rays", u64::from(listed));
    if let Some(f) = stats::figures(&iterations) {
        row.number("iter_ms_min", Some(f.min))
            .number("iter_ms_p50", Some(f.p50))
            .number("iter_ms_p95", Some(f.p95))
            .number("iter_ms_p99", Some(f.p99))
            .number("iter_ms_max", Some(f.max));
    }
    put_series(row, &series, 0);
    if frames > 1 {
        row.count("builds_done", builds)
            .count("refits_done", refits)
            .count("build_untimed", untimed)
            .count("input_bytes", input_bytes)
            .number("build_gpu_ms", median(&gpu[0]))
            .number("refit_gpu_ms", median(&gpu[1]))
            .number("tlas_build_gpu_ms", median(&gpu[2]))
            .number("tlas_refit_gpu_ms", median(&gpu[3]))
            .number("convert_cpu_ms", median(&convert_ms))
            .number("upload_cpu_ms", median(&upload_ms))
            .number("record_cpu_ms", median(&record_ms))
            .number("build_wait_ms", median(&build_wait));
    }
    // The check: one more dispatch over the structures as the last
    // iteration left them, over poisoned records and in no time of the
    // row; its records against the same frame's first ones.
    let check = Trace { poison: true, ..trace };
    device.trace(&check).map_err(|e| format!("the dispatch that checks the loop: {e}"))?;
    let (bytes, wait) =
        device.hits(listed).map_err(|e| format!("reading the checking dispatch's records back: {e}"))?;
    row.number("readback_wait_ms", Some(ms(wait)));
    let now = records::answers(case.rays.len(), &mapped.source, &records::words(&bytes))?;
    if let Some(before) = &first[last] {
        row.count("loop_last_differing", now.iter().zip(before).filter(|(a, b)| a != b).count() as u64);
    }
    Ok(())
}

/// One tracing path over every frame of one class, then its loop, then
/// its structures given up.
fn path_pass(
    device: &mut dyn Device,
    shared: &mut Shared,
    case: &Case,
    mapped: &Mapped,
    path: Path,
    null: Option<&stats::Figures>,
    say: &mut dyn FnMut(&str),
) -> PathRun {
    let options = shared.options;
    let frames = case.frames.len();
    let mut run = PathRun { rows: Vec::new(), answers: vec![None; frames], loop_row: None };
    let before = device.memory();
    let mut built: Option<PathInputs> = None;
    let mut stopped: Option<usize> = None;
    for frame in 0..frames {
        let mut row = Row::new(&case.class, case.seed, &frame.to_string(), path.name());
        put_scene(&mut row, &case.frames[frame].scene);
        put_rays(&mut row, case, mapped);
        row.count("timed_rays", u64::from(options.timed_rays));
        if let Some(at) = stopped {
            row.text("status", row::NOT_RUN).note(&format!("frame {at} of this path failed"));
            run.rows.push(row);
            continue;
        }
        let ran = shared.pipeline(path.form(), &mut row).and_then(|()| {
            frame_pass(device, options, case, mapped, path, frame, &mut built, &mut row, &mut run.answers[frame])
        });
        put_memory(&mut row, &before, &device.memory());
        heard(device, &mut row, say);
        match ran {
            Ok(()) => {
                row.text("status", row::RAN);
                if let Some(null) = null {
                    row.number("null_gpu_ms_min", Some(null.min))
                        .number("null_gpu_ms_p50", Some(null.p50))
                        .number("null_gpu_ms_p95", Some(null.p95))
                        .number("null_gpu_ms_max", Some(null.max));
                }
            }
            Err(why) => {
                fail(&mut row, &format!("frame {frame}"), &why);
                stopped = Some(frame);
            }
        }
        run.rows.push(row);
    }
    if options.loops > 0 {
        let mut row = Row::new(&case.class, case.seed, "loop", path.name());
        put_rays(&mut row, case, mapped);
        row.text("container_hash", &container_hash(&shared.shaders[shader_place(path.form())].dxil));
        match stopped {
            Some(at) => {
                row.text("status", row::NOT_RUN).note(&format!("frame {at} of this path failed"));
            }
            None => {
                match loop_pass(device, options, case, mapped, path, &mut built, &run.answers, &mut row) {
                    Ok(()) => row.text("status", row::RAN),
                    Err(why) => row.text("status", row::FAILED).note(&why),
                };
                put_memory(&mut row, &before, &device.memory());
                heard(device, &mut row, say);
            }
        }
        run.loop_row = Some(row);
    }
    let released = device.release(path);
    let row = run.loop_row.as_mut().or(run.rows.last_mut()).expect("a case has a frame");
    if let Err(why) = released {
        row.note(&format!("giving the structures up: {why}"));
    }
    heard(device, row, say);
    run
}

/// Fills a row's verdict and refusal columns from a path's answers for a
/// frame; the defects it lists.
#[allow(clippy::too_many_arguments)]
fn judge(
    row: &mut Row,
    case: &Case,
    truth: &Truth,
    frame: usize,
    path: Path,
    answers: &[Answer],
    next: Option<&[Answer]>,
    mismatches: &mut Vec<Mismatch>,
) -> Result<(), String> {
    let refused = records::refusals(answers, next);
    row.count("refused_geometry", u64::from(refused.geometry))
        .count("refused_range", u64::from(refused.range))
        .count("unwritten", u64::from(refused.unwritten))
        .count("refused_malformed", u64::from(refused.malformed))
        .count("refused_taken_by_next", u64::from(refused.taken_by_next))
        .count("refused_unanswered", u64::from(refused.unanswered));
    let judged = adapter::judge(case, truth, frame, answers, path.reports_face())?;
    row.text("verdict", judged.verdict)
        .count("judged", u64::from(judged.judged))
        .count("clear", u64::from(judged.clear))
        .count("ambiguous", u64::from(judged.ambiguous))
        .count("excluded", u64::from(judged.excluded))
        .count("wide", u64::from(judged.wide))
        .text("not_clear_reasons", &judged.not_clear)
        .count("matched", u64::from(judged.matched))
        .count("permitted_primary", u64::from(judged.permitted_primary))
        .count("permitted_other", u64::from(judged.permitted_other))
        .count("ids_only", u64::from(judged.ids_only))
        .count("beyond_band", u64::from(judged.beyond_band))
        .count("missed", u64::from(judged.missed))
        .count("phantom", u64::from(judged.phantom))
        .count("wrong_triangle", u64::from(judged.wrong_triangle))
        .count("wrong_face", u64::from(judged.wrong_face))
        .count("numeric", u64::from(judged.numeric))
        .count("malformed", u64::from(judged.malformed))
        .number("worst_t_ratio", Some(judged.worst_t_ratio))
        .number("worst_uv_ratio", Some(judged.worst_uv_ratio));
    if !path.reports_face() {
        row.count("faces_not_reported", u64::from(judged.faces_not_reported));
    }
    match &judged.model {
        Some(model) => {
            row.count("model_defects", u64::from(model.defects))
                .count("model_identical", u64::from(model.identical))
                .count("model_divergent", u64::from(model.divergent))
                .count("model_divergent_first_hit", u64::from(model.divergent_first_hit));
        }
        None => {
            row.text("model_refusal", &judged.model_refusal);
        }
    }
    mismatches.extend(judged.defects.iter().map(|defect| Mismatch {
        class: case.class.clone(),
        seed: case.seed,
        frame: frame as u32,
        path: path.name(),
        ray: defect.ray,
        outcome: defect.outcome,
        ray_words: case.rays[defect.ray as usize],
        record: defect.record,
    }));
    Ok(())
}

/// A line for the console when a class is done: per path, what its rows
/// say.
fn class_line(class: &str, rows: &[Row]) -> String {
    let mut parts = Vec::new();
    for path in [row::NULL, row::NATIVE, row::HYBRID] {
        let of: Vec<&Row> = rows.iter().filter(|r| r.get("path") == path && r.get("frame") != "loop").collect();
        let ran = of.iter().filter(|r| r.get("status") == row::RAN).count();
        let sum = |column: &str| -> u64 { of.iter().filter_map(|r| r.get(column).parse::<u64>().ok()).sum() };
        let part = if path == row::NULL {
            format!("null {ran}/{} ran, {} records not a miss", of.len(), sum("null_not_miss"))
        } else {
            let defects = ["missed", "phantom", "wrong_triangle", "wrong_face", "numeric", "malformed"]
                .iter()
                .map(|c| sum(c))
                .sum::<u64>();
            let refused = sum("refused_taken_by_next") + sum("refused_unanswered");
            format!(
                "{path} {ran}/{} frames ran, {defects} defects ({} records unwritten), {refused} rays refused, {} \
                 records of the checking dispatch differing",
                of.len(),
                sum("unwritten"),
                sum("repeat_differing")
            )
        };
        parts.push(part);
    }
    format!("{class}: {}", parts.join("; "))
}

/// Runs `options` on `device`. `say` gets a line now and then for the
/// console. An error is what stops the whole run before or between
/// classes (options that make no run, a case the CPU cannot generate or
/// judge); what fails on the device fails rows.
pub fn run(device: &mut dyn Device, options: &Options, say: &mut dyn FnMut(&str)) -> Result<Results, String> {
    options.check()?;
    let shaders = [Form::Null, Form::Triangles, Form::Procedural].map(rayquery::build);
    let mut cases = Vec::new();
    for class in &options.classes {
        cases.push(adapter::case(class, options.seed)?);
    }
    let mapped: Vec<Mapped> = cases.iter().map(|case| rays::map(&case.rays)).collect();
    let sizes: Vec<ClassSize> = cases
        .iter()
        .zip(&mapped)
        .map(|(case, mapped)| ClassSize { frames: case.frames.len() as u32, listed: mapped.records.len() as u32 })
        .collect();
    let planned = plan::counts(options, &sizes);
    say(&format!("this run: {}", planned.line()));

    let mut table = Table::default();
    table.set("probe_version", &crate::PROBE_VERSION.to_string());
    table.set("policy", adapter::POLICY);
    table.set(
        "reading",
        "nothing has selected a path from this file and no row is a pass under the rule of \
         docs/dxr-compatibility.md; the times are of synthetic scenes, select nothing and say nothing about a title",
    );
    table.set("k", "not computed: the tolerance is the crate's (docs/dxr-validation-reference.md)");
    table.set("seed", &format!("{:x}", options.seed));
    table.set("classes", &options.classes.join(" "));
    table.set("timed_rays", &options.timed_rays.to_string());
    table.set("repeats", &options.repeats.to_string());
    table.set("discard", &options.discard.to_string());
    table.set("loop", &options.loops.to_string());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    table.set("unix_time", &now.to_string());
    for (key, value) in device.identity() {
        table.set(&key, &value);
    }
    table.set("planned", &planned.line());
    table.set(
        &format!("path.{}", row::SOFTWARE_GPU),
        "not built: no shader walks console-format nodes here; the model_* columns compare with its CPU model",
    );

    let mut shared = Shared {
        options,
        shaders: &shaders,
        pipelines: [Ok(None), Ok(None), Ok(None)],
    };
    for shader in &shaders {
        let name = form_name(shader.form);
        let made = device.pipeline(shader);
        let line = match &made {
            Ok(time) => format!("made in {:.3} ms of processor time", ms(*time)),
            Err(why) => format!("not made: {why}"),
        };
        let messages = device.messages();
        let line = match messages.first() {
            Some(first) => format!("{line}; the validation layer said {} messages, the first: {first}", messages.len()),
            None => line,
        };
        say(&format!("pipeline, {name} shader: {line}"));
        for message in &messages {
            say(&format!("validation layer, {name} pipeline: {message}"));
        }
        table.set(
            &format!("container.{name}"),
            &format!("{} ({} bytes)", container_hash(&shader.dxil), shader.dxil.len()),
        );
        table.set(&format!("pipeline.{name}"), &line);
        shared.pipelines[shader_place(shader.form)] = made.map(Some);
    }

    let mut records: Vec<(Path, usize, u32, Vec<HitWords>)> = Vec::new();
    let mut mismatches = Vec::new();
    for (at, (case, mapped)) in cases.iter().zip(&mapped).enumerate() {
        say(&format!("{}: the CPU reference and the CPU model, {} rays", case.class, case.rays.len()));
        let truth = adapter::truth(case)?;
        let frames = case.frames.len();
        let listed = mapped.records.len() as u32;
        let mut rows: Vec<Row> = Vec::new();

        // Whether anything can be dispatched for the class at all.
        let ready = if listed == 0 {
            Err((row::NOT_RUN, "every ray of the class is refused before the dispatch".to_string()))
        } else {
            let list = rays::list(&mapped.records, plan::list_records(listed, options.timed_rays));
            device
                .rays(&list)
                .map(|took| (list.len() as u64, took))
                .map_err(|e| (row::FAILED, format!("uploading the ray list: {e}")))
        };
        let (mut null_row, null) = match &ready {
            Ok(list) => null_pass(device, &mut shared, case, mapped, *list),
            Err((status, why)) => {
                let mut row = Row::new(&case.class, case.seed, "", row::NULL);
                put_rays(&mut row, case, mapped);
                row.text("status", status).note(why);
                (row, None)
            }
        };
        heard(device, &mut null_row, say);
        rows.push(null_row);

        let mut runs: Vec<PathRun> = Vec::new();
        for path in Path::ALL {
            runs.push(match &ready {
                Ok(_) => path_pass(device, &mut shared, case, mapped, path, null.as_ref(), say),
                Err((status, why)) => {
                    let rows = (0..frames)
                        .map(|frame| {
                            let mut row = Row::new(&case.class, case.seed, &frame.to_string(), path.name());
                            put_scene(&mut row, &case.frames[frame].scene);
                            put_rays(&mut row, case, mapped);
                            row.text("status", status).note(why);
                            row
                        })
                        .collect();
                    PathRun { rows, answers: vec![None; frames], loop_row: None }
                }
            });
        }

        // A path that never dispatched still refused every ray it was not
        // given: the rays refused before the dispatch are counted in its
        // rows (`refused_before`), and nothing of it is judged.
        for frame in 0..frames {
            for (p, path) in Path::ALL.into_iter().enumerate() {
                let next = path
                    .next()
                    .and_then(|next| Path::ALL.iter().position(|p| *p == next))
                    .and_then(|next| runs[next].answers[frame].clone());
                let Some(answers) = runs[p].answers[frame].clone() else { continue };
                let row = &mut runs[p].rows[frame];
                if let Err(why) = judge(row, case, &truth, frame, path, &answers, next.as_deref(), &mut mismatches)
                {
                    row.note(&format!("the comparison: {why}"));
                }
                records.push((path, at, frame as u32, answers.iter().map(Answer::words).collect()));
            }
        }
        for frame in 0..frames {
            for run in &runs {
                rows.push(run.rows[frame].clone());
            }
            let mut row = Row::new(&case.class, case.seed, &frame.to_string(), row::SOFTWARE_GPU);
            row.text("status", row::NOT_BUILT);
            rows.push(row);
        }
        rows.extend(runs.iter_mut().filter_map(|run| run.loop_row.take()));
        say(&class_line(&case.class, &rows));
        table.rows.extend(rows);
    }
    table.set("backend_counters", &device.counters());
    let count = |status: &str| table.rows.iter().filter(|r| r.get("status") == status).count();
    table.set(
        "rows",
        &format!(
            "{} ran, {} failed, {} not run, {} not built",
            count(row::RAN),
            count(row::FAILED),
            count(row::NOT_RUN),
            count(row::NOT_BUILT)
        ),
    );
    Ok(Results { table, cases, records, mismatches })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Traced;
    use crate::standin::StandIn;

    /// A short run: the timed dispatches list fewer records than a class
    /// has rays, and few of them are made.
    fn short(classes: &[&str]) -> Options {
        Options {
            classes: classes.iter().map(|c| c.to_string()).collect(),
            timed_rays: 96,
            repeats: 2,
            discard: 1,
            ..Options::default()
        }
    }

    fn go(device: &mut StandIn, options: &Options) -> Results {
        let mut said = Vec::new();
        let results = run(device, options, &mut |line| said.push(line.to_string())).unwrap();
        assert!(said[0].starts_with("this run: "), "{said:?}");
        results
    }

    fn rows<'a>(results: &'a Results, class: &str, path: &str) -> Vec<&'a Row> {
        results.table.rows.iter().filter(|r| r.get("class") == class && r.get("path") == path).collect()
    }

    fn number(row: &Row, column: &str) -> u64 {
        row.get(column).parse().unwrap_or_else(|_| panic!("{column} is {:?}", row.get(column)))
    }

    fn defects(row: &Row) -> u64 {
        ["missed", "phantom", "wrong_triangle", "wrong_face", "numeric", "malformed"]
            .iter()
            .map(|column| number(row, column))
            .sum()
    }

    /// What the probe hands a device describes the scene the CPU reference
    /// traces: a brute-force device that reads the build inputs as D3D12
    /// would, and the triangle data as the procedural shader does, has no
    /// defect in any class, on either path, in any frame. This is about
    /// the conversion and the run, not about any device.
    #[test]
    fn every_class_runs_on_a_brute_force_device_without_a_defect() {
        let classes = adapter::classes();
        let options = short(&classes);
        let mut device = StandIn::default();
        let results = go(&mut device, &options);
        assert_eq!(results.table.value("policy"), Some(adapter::POLICY));
        assert!(results.table.value("reading").unwrap().contains("select nothing"));
        assert!(results.table.value("path.software-gpu").unwrap().starts_with("not built"));
        let mut frames = 0;
        for class in &classes {
            let null = rows(&results, class, row::NULL);
            assert_eq!(null.len(), 1);
            assert_eq!((null[0].get("status"), number(null[0], "null_not_miss")), (row::RAN, 0), "{class}");
            assert_eq!(number(null[0], "trace_samples"), 2);
            for path in Path::ALL {
                let of = rows(&results, class, path.name());
                assert!(!of.is_empty());
                frames += of.len();
                for row in of {
                    let at = format!("{class} frame {} {}: {:?}", row.get("frame"), path.name(), row.get("note"));
                    assert_eq!(row.get("status"), row::RAN, "{at}");
                    assert_eq!(defects(row), 0, "{at}: {:?}", results.mismatches.first());
                    assert!(matches!(row.get("verdict"), "pass" | "fallback"), "{at}: {}", row.get("verdict"));
                    assert_eq!(number(row, "repeat_differing"), 0, "{at}");
                    assert_eq!(number(row, "judged") + number(row, "refused_before"), number(row, "rays"), "{at}");
                    assert_eq!(number(row, "refused_geometry") + number(row, "refused_range"), 0, "{at}");
                    assert_eq!(number(row, "unwritten") + number(row, "refused_malformed"), 0, "{at}");
                    assert_eq!(row.get("model_refusal"), "", "{at}");
                    assert_eq!(number(row, "model_defects"), 0, "{at}");
                    assert_eq!(row.get("container_hash").len(), 32, "{at}");
                    assert_eq!(number(row, "trace_samples"), 2, "{at}");
                    assert!(!row.get("null_gpu_ms_p50").is_empty(), "{at}");
                    // The stand-in refits whatever it is asked to: what was
                    // done is what the steps asked for.
                    assert_eq!(
                        number(row, "builds_done") + number(row, "refits_done") > 0,
                        row.get("frame") == "0" || number(row, "builds_asked") + number(row, "refits_asked") > 0,
                        "{at}"
                    );
                }
            }
            // The software traversal as a shader: named, and not built.
            let software = rows(&results, class, row::SOFTWARE_GPU);
            assert!(!software.is_empty());
            for row in software {
                assert_eq!(row.get("status"), row::NOT_BUILT);
                assert!(row.values()[5..].iter().all(String::is_empty), "nothing is made up for it");
            }
        }
        assert!(results.mismatches.is_empty());
        assert_eq!(results.records.len(), frames, "one list of records a path and frame");
        // The run did what it said it would.
        let planned = results.table.value("planned").unwrap();
        assert!(planned.contains(&format!("; {} dispatches (", device.dispatches)), "{planned}; {}", device.dispatches);
        assert_eq!(
            results.table.value("rows").unwrap(),
            format!("{} ran, 0 failed, 0 not run, {} not built", frames + classes.len(), frames / 2)
        );
        // Every path's structures were given up.
        assert_eq!(device.log.iter().filter(|l| l.starts_with("release ")).count(), 2 * classes.len());
        assert_eq!(device.memory().structures, 0);
    }

    #[test]
    fn a_frame_is_built_or_refitted_as_the_scene_says() {
        let mut device = StandIn::default();
        let results = go(&mut device, &short(&["dynamic-refit", "dynamic-rebuild"]));
        let refit = rows(&results, "dynamic-refit", row::NATIVE);
        assert_eq!(refit.len(), 4);
        // Three bottom levels and the top level built; then two refitted
        // and the top level with them, one kept.
        assert_eq!((number(refit[0], "builds_done"), number(refit[0], "refits_done")), (4, 0));
        for row in &refit[1..] {
            assert_eq!((number(row, "builds_done"), number(row, "refits_done")), (0, 3), "{:?}", row.get("note"));
            assert_eq!((number(row, "builds_asked"), number(row, "refits_asked")), (0, 3));
            assert_eq!(row.get("note"), "");
            assert!(!row.get("refit_gpu_ms").is_empty() && !row.get("tlas_refit_gpu_ms").is_empty());
            assert_eq!(row.get("build_gpu_ms"), "", "nothing was built: empty, not 0");
        }
        let log: Vec<&String> = device.log.iter().filter(|l| l.starts_with("build native")).collect();
        assert_eq!(log[1], "build native bottoms [Keep, Refit, Refit] top Refit data false");
        // The hybrid uploads its triangle data again when vertices move.
        let log: Vec<&String> = device.log.iter().filter(|l| l.starts_with("build hybrid")).collect();
        assert_eq!(log[0], "build hybrid bottoms [Rebuild, Rebuild, Rebuild] top Rebuild data true");
        assert_eq!(log[1], "build hybrid bottoms [Keep, Refit, Refit] top Refit data true");
        // The rebuild class builds.
        let rebuild = rows(&results, "dynamic-rebuild", row::HYBRID);
        assert_eq!(rebuild.len(), 4);
        assert!(rebuild[1..].iter().all(|row| number(row, "builds_done") > 0));
        assert!(rebuild.iter().all(|row| number(row, "flat_boxes") <= number(row, "primitives")));
    }

    #[test]
    fn a_ray_the_native_shader_refuses_is_counted_and_answered_by_the_hybrid() {
        let mut device = StandIn { native_refuses_every: Some(7), ..StandIn::default() };
        let results = go(&mut device, &short(&["static-soup"]));
        let native = rows(&results, "static-soup", row::NATIVE)[0];
        let refused = 2000_u64.div_ceil(7);
        assert_eq!(number(native, "refused_geometry"), refused);
        assert_eq!((number(native, "refused_taken_by_next"), number(native, "refused_unanswered")), (refused, 0));
        // Not judged, and so never a miss: no defect comes of them.
        assert_eq!((number(native, "judged"), defects(native)), (2000 - refused, 0));
        let hybrid = rows(&results, "static-soup", row::HYBRID)[0];
        assert_eq!((number(hybrid, "judged"), number(hybrid, "refused_range")), (2000, 0));
        // The records a file holds keep the refusals as refusals.
        let text = results.hits_text(Path::Native).unwrap().unwrap();
        let back = adapter::read_hits(&text).unwrap();
        let kept = back[0].records.iter().filter(|r| matches!(Answer::of(**r), Answer::Refused(_))).count();
        assert_eq!(kept as u64, refused);

        // The hybrid refuses too: where both do, nothing built answers,
        // since the software traversal on the GPU is not built.
        let mut device =
            StandIn { native_refuses_every: Some(7), hybrid_refuses_every: Some(5), ..StandIn::default() };
        let results = go(&mut device, &short(&["static-soup"]));
        let native = rows(&results, "static-soup", row::NATIVE)[0];
        let both = 2000_u64.div_ceil(35);
        assert_eq!(number(native, "refused_unanswered"), both);
        assert_eq!(number(native, "refused_taken_by_next"), refused - both);
        let hybrid = rows(&results, "static-soup", row::HYBRID)[0];
        assert_eq!((number(hybrid, "refused_range"), number(hybrid, "refused_unanswered")), (400, 400));
        assert_eq!((number(hybrid, "refused_taken_by_next"), defects(hybrid)), (0, 0));
    }

    #[test]
    fn rays_refused_before_the_dispatch_are_in_no_list_and_counted_with_their_reasons() {
        let mut device = StandIn::default();
        let results = go(&mut device, &short(&["masks-and-flags", "precision"]));
        let mut refused_somewhere = 0;
        for class in ["masks-and-flags", "precision"] {
            for path in [row::NATIVE, row::HYBRID] {
                let row = rows(&results, class, path)[0];
                let before = number(row, "refused_before");
                refused_somewhere += before;
                assert_eq!(number(row, "rays_listed") + before, number(row, "rays"));
                assert_eq!(before == 0, row.get("refused_before_reasons").is_empty());
                // Neither path was given them, and no built path takes them.
                assert_eq!(number(row, "refused_unanswered"), before, "{class} {path}");
                assert_eq!(number(row, "judged"), number(row, "rays_listed"));
            }
        }
        // Whether a class has such rays is the generator's to say; where
        // it has, the dispatches listed fewer records than the class has
        // rays, and the log says how many.
        let listed = number(rows(&results, "masks-and-flags", row::NATIVE)[0], "rays_listed");
        assert!(device.log.contains(&format!("trace Triangles {listed} poisoned")), "{refused_somewhere}");
    }

    #[test]
    fn what_fails_on_the_device_fails_its_rows_and_the_run_goes_on() {
        // No pipeline for the triangles shader: the native rows fail with
        // the reason, the hybrid's run.
        let mut device = StandIn { no_pipeline: Some(Form::Triangles), ..StandIn::default() };
        let results = go(&mut device, &short(&["dynamic-refit"]));
        let native = rows(&results, "dynamic-refit", row::NATIVE);
        assert_eq!(native[0].get("status"), row::FAILED);
        assert!(native[0].get("note").contains("the triangles pipeline was not made"), "{}", native[0].get("note"));
        assert!(native[1..].iter().all(|r| r.get("status") == row::NOT_RUN));
        assert!(native.iter().all(|r| r.get("verdict").is_empty() && r.get("judged").is_empty()));
        assert!(results.table.value("pipeline.triangles").unwrap().starts_with("not made"));
        assert!(results.hits_text(Path::Native).unwrap().is_none(), "no records, no file");
        let hybrid = rows(&results, "dynamic-refit", row::HYBRID);
        assert!(hybrid.iter().all(|r| r.get("status") == row::RAN && defects(r) == 0));
        assert_eq!(results.table.value("rows").unwrap(), "5 ran, 1 failed, 3 not run, 4 not built");

        // The third build of the run fails: the native path's frame 2.
        let mut device = StandIn { fail_build: Some(2), ..StandIn::default() };
        let results = go(&mut device, &short(&["dynamic-refit"]));
        let native = rows(&results, "dynamic-refit", row::NATIVE);
        let status: Vec<&str> = native.iter().map(|r| r.get("status")).collect();
        assert_eq!(status, [row::RAN, row::RAN, row::FAILED, row::NOT_RUN]);
        assert!(native[2].get("note").contains("frame 2: building the structures"), "{}", native[2].get("note"));
        assert_eq!(native[2].get("trace_gpu_ms_p50"), "", "what did not run has no time");
        assert_eq!(native[3].get("note"), "frame 2 of this path failed");
        assert!(rows(&results, "dynamic-refit", row::HYBRID).iter().all(|r| r.get("status") == row::RAN));
        assert!(device.log.contains(&"release native".to_string()));
    }

    #[test]
    fn a_stale_structure_shows_as_defects_and_a_dispatch_that_writes_nothing_as_no_answer() {
        // A refit that is granted and not done: the frames after the first
        // trace the old vertices.
        let mut device = StandIn { stale_refits: true, ..StandIn::default() };
        let results = go(&mut device, &short(&["dynamic-refit"]));
        for path in [row::NATIVE, row::HYBRID] {
            let of = rows(&results, "dynamic-refit", path);
            assert_eq!((of[0].get("verdict"), defects(of[0])), ("pass", 0), "{path}");
            assert!(of[1..].iter().all(|r| r.get("verdict") == "fail" && defects(r) > 0), "{path}");
        }
        // The mismatch file holds the first of them, each with its ray.
        assert!(!results.mismatches.is_empty());
        assert!(results.mismatches.iter().all(|m| m.frame > 0));
        let listed = results.mismatches.iter().filter(|m| m.path == row::NATIVE && m.frame == 1).count();
        let native = rows(&results, "dynamic-refit", row::NATIVE)[1];
        assert_eq!(listed as u64, defects(native).min(adapter::DEFECTS_LISTED as u64));
        let first = &results.mismatches[0];
        assert_eq!(first.ray_words, results.cases[0].rays[first.ray as usize]);

        // Tracing shaders that write nothing: every record is the poison,
        // which is no hit, no miss and no refusal. Each is judged, and is
        // a defect; the checking dispatch's records are unwritten too.
        let mut device = StandIn { writes_nothing: true, ..StandIn::default() };
        let results = go(&mut device, &short(&["static-soup"]));
        for path in [row::NATIVE, row::HYBRID] {
            let row = rows(&results, "static-soup", path)[0];
            assert_eq!((number(row, "unwritten"), number(row, "refused_malformed")), (2000, 0));
            assert_eq!((number(row, "judged"), row.get("verdict")), (2000, "fail"));
            assert_eq!(number(row, "malformed") + number(row, "excluded"), 2000);
            assert_eq!(number(row, "missed"), 0, "an unwritten record is not a miss");
            assert_eq!((number(row, "refused_taken_by_next"), number(row, "refused_unanswered")), (0, 0));
            assert_eq!(number(row, "repeat_differing"), 96, "what the check did not write is counted");
        }
    }

    /// A dispatch that leaves some of its records unwritten cannot pass on
    /// the others: the unwritten ones stay among the rays judged, as
    /// defects, in the row, in the mismatch list and in the hit file.
    #[test]
    fn a_dispatch_that_leaves_records_unwritten_does_not_pass_on_the_rest() {
        let mut device = StandIn { leaves_unwritten_every: Some(3), ..StandIn::default() };
        let results = go(&mut device, &short(&["static-soup", "dynamic-refit"]));
        for (class, rays) in [("static-soup", 2000_u64), ("dynamic-refit", 1200)] {
            for path in Path::ALL {
                for row in rows(&results, class, path.name()) {
                    let at = format!("{class} {} frame {}", path.name(), row.get("frame"));
                    let unwritten = rays.div_ceil(3);
                    assert_eq!(row.get("status"), row::RAN, "{at}");
                    assert_eq!(number(row, "unwritten"), unwritten, "{at}");
                    assert_eq!(number(row, "judged"), rays, "{at}: none is taken out of the comparison");
                    assert_eq!(row.get("verdict"), "fail", "{at}");
                    // Each is a defect, but for those of rays the reference
                    // excludes, which nothing is judged on.
                    let malformed = number(row, "malformed");
                    assert!(malformed > 0 && malformed <= unwritten, "{at}: {malformed}");
                    assert!(malformed + number(row, "excluded") >= unwritten, "{at}: {malformed}");
                    assert_eq!(defects(row), malformed, "{at}: the written records are right");
                    assert_eq!((number(row, "refused_geometry"), number(row, "refused_range")), (0, 0), "{at}");
                    assert_eq!((number(row, "refused_taken_by_next"), number(row, "refused_unanswered")), (0, 0));
                    // The check lists 96 records and leaves every third
                    // unwritten too.
                    assert_eq!(number(row, "repeat_differing"), 32, "{at}");
                }
            }
        }
        let listed: Vec<&Mismatch> = results.mismatches.iter().filter(|m| m.class == "static-soup").collect();
        assert!(!listed.is_empty());
        assert!(listed.iter().all(|m| m.outcome == "malformed" && m.record == [crate::device::POISON; 8]));
        let text = results.hits_text(Path::Hybrid).unwrap().unwrap();
        let back = adapter::read_hits(&text).unwrap();
        let kept = back[0].records.iter().filter(|r| records::is_unwritten(r)).count();
        assert_eq!(kept as u64, 2000_u64.div_ceil(3), "the file holds them as they were read");
    }

    /// The times of a row are of dispatches that list `timed_rays` records.
    /// That such a dispatch writes every record it lists is shown by the
    /// checking dispatch: a device that writes the first 2,000 records of a
    /// dispatch and no more answers every ray of a class of 2,000 rightly,
    /// and its dispatches of 4,096 records leave 2,096 unwritten.
    #[test]
    fn the_checking_dispatch_shows_a_timed_size_dispatch_that_does_not_write_every_record() {
        let mut device = StandIn { writes_at_most: Some(2000), ..StandIn::default() };
        let options = Options { timed_rays: 4096, ..short(&["static-soup"]) };
        let results = go(&mut device, &options);
        let null = rows(&results, "static-soup", row::NULL)[0];
        assert_eq!((null.get("status"), number(null, "null_not_miss")), (row::RAN, 2096));
        for path in [row::NATIVE, row::HYBRID] {
            let row = rows(&results, "static-soup", path)[0];
            assert_eq!((row.get("verdict"), defects(row), number(row, "unwritten")), ("pass", 0, 0), "{path}");
            assert_eq!(number(row, "repeat_differing"), 2096, "{path}");
        }
        // With a device that writes what it lists, nothing differs.
        let mut device = StandIn::default();
        let results = go(&mut device, &options);
        for path in [row::NULL, row::NATIVE, row::HYBRID] {
            let row = rows(&results, "static-soup", path)[0];
            let column = if path == row::NULL { "null_not_miss" } else { "repeat_differing" };
            assert_eq!(number(row, column), 0, "{path}");
        }
        // And what it was asked: per shader, one dispatch over the 2,000
        // listed rays, poisoned; three timed ones (one left out, two kept)
        // of exactly `timed_rays` records, not poisoned; one check of as
        // many, poisoned.
        for form in ["Null", "Triangles", "Procedural"] {
            let of: Vec<&String> = device.log.iter().filter(|l| l.starts_with(&format!("trace {form} "))).collect();
            let expected = [
                format!("trace {form} 2000 poisoned"),
                format!("trace {form} 4096"),
                format!("trace {form} 4096"),
                format!("trace {form} 4096"),
                format!("trace {form} 4096 poisoned"),
            ];
            assert_eq!(of, expected.iter().collect::<Vec<_>>());
        }
    }

    /// A row that fails after its answers came back keeps what came before
    /// the step that failed: the verdict and the counts, and its records
    /// in the hit file. The step that failed and those after it have no
    /// column.
    #[test]
    fn a_row_that_fails_after_its_answers_keeps_them_and_nothing_of_the_failed_step() {
        // With `short`, a pass is five dispatches: the null shader's are 0
        // to 4, the native path's frame 0 has 5 (the answers), 6 to 8
        // (timed) and 9 (the check). The second timed one fails.
        let mut device = StandIn { fail_dispatch: Some(7), ..StandIn::default() };
        let results = go(&mut device, &short(&["dynamic-refit"]));
        let native = rows(&results, "dynamic-refit", row::NATIVE);
        assert_eq!(native[0].get("status"), row::FAILED);
        assert!(native[0].get("note").contains("frame 0: a timed dispatch"), "{}", native[0].get("note"));
        assert_eq!((native[0].get("verdict"), number(native[0], "judged"), defects(native[0])), ("pass", 1200, 0));
        assert!(!native[0].get("builds_done").is_empty() && !native[0].get("readback_wait_ms").is_empty());
        let of_later_steps =
            ["trace_samples", "trace_gpu_ms_p50", "trace_wait_ms_p50", "repeat_differing", "null_gpu_ms_p50"];
        for column in of_later_steps {
            assert_eq!(native[0].get(column), "", "{column}");
        }
        // What was held when it failed is read all the same.
        assert!(!native[0].get("own_rt_structures_bytes").is_empty() && !native[0].get("commit_mb_after").is_empty());
        assert_eq!(native[1].get("own_rt_structures_bytes"), "", "a row that did not run has no figure");
        assert!(native[1..].iter().all(|r| r.get("status") == row::NOT_RUN && r.get("verdict").is_empty()));
        let text = results.hits_text(Path::Native).unwrap().unwrap();
        assert_eq!(adapter::read_hits(&text).unwrap().len(), 1, "frame 0's records are in the file");
        has_records_where_the_refusal_counts_are_filled(&results);
    }

    /// What `cargo xtask rt-summary` goes by when it compares two runs: a
    /// row of a tracing path has a section in the path's hit file exactly
    /// when its refusal counts are filled.
    fn has_records_where_the_refusal_counts_are_filled(results: &Results) {
        for row in &results.table.rows {
            let Some(path) = Path::ALL.into_iter().find(|p| p.name() == row.get("path")) else { continue };
            let Ok(frame) = row.get("frame").parse::<u32>() else {
                assert_eq!(row.get("refused_unanswered"), "", "a loop row has no section");
                continue;
            };
            let case = results.cases.iter().position(|c| c.class == row.get("class")).unwrap();
            let kept = results.records.iter().any(|(p, c, f, _)| (*p, *c, *f) == (path, case, frame));
            assert_eq!(kept, !row.get("refused_unanswered").is_empty(), "{} {} {frame}", row.get("class"), path.name());
        }
    }

    fn fixed(value: f64) -> String {
        format!("{value:.6}")
    }

    fn millis(time: Duration) -> String {
        fixed(time.as_secs_f64() * 1e3)
    }

    /// The least, the median, the 95th percentile by nearest rank and the
    /// greatest of 40 samples or of 5, picked by hand from the sorted ones.
    fn four(samples: &[f64]) -> [String; 4] {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let (p50, p95) = match sorted.len() {
            40 => ((sorted[19] + sorted[20]) / 2.0, sorted[37]),
            5 => (sorted[2], sorted[4]),
            other => panic!("{other} samples"),
        };
        [sorted[0], p50, p95, sorted[sorted.len() - 1]].map(fixed)
    }

    fn middle(samples: &[f64]) -> String {
        four(samples)[1].clone()
    }

    /// Holds every column of `row` from `trace_samples` on against
    /// `expected`: a column named there has that value, any other is
    /// empty. The columns in `clock` are this process's own wall time, of
    /// which only that there is one can be said.
    fn measured(row: &Row, expected: &[(&str, String)], clock: &[&str]) {
        let first = row::COLUMNS.iter().position(|(name, _)| *name == "trace_samples").unwrap();
        for (name, _) in &row::COLUMNS[first..] {
            let at = format!("{} {} {}: {name}", row.get("class"), row.get("path"), row.get("frame"));
            if clock.contains(name) {
                assert!(row.get(name).parse::<f64>().is_ok_and(|v| v >= 0.0), "{at} is {:?}", row.get(name));
                continue;
            }
            let wanted = expected.iter().find(|(column, _)| column == name).map_or("", |(_, value)| value.as_str());
            assert_eq!(row.get(name), wanted, "{at}");
        }
        for (column, _) in expected {
            assert!(row::COLUMNS[first..].iter().any(|(name, _)| name == column), "{column} is no measurement column");
        }
    }

    /// The columns that hold what the device says it holds: the three
    /// gauges after, the adapter's and the process's figures before and
    /// after.
    fn memory_columns(before: &Memory, after: &Memory) -> Vec<(&'static str, String)> {
        vec![
            ("own_rt_structures_bytes", after.structures.to_string()),
            ("own_rt_scratch_bytes", after.scratch.to_string()),
            ("own_rt_buffers_bytes", after.buffers.to_string()),
            ("video_local_mb_before", fixed(before.video_local_mb.unwrap())),
            ("video_local_mb_after", fixed(after.video_local_mb.unwrap())),
            ("video_budget_mb", fixed(after.video_budget_mb.unwrap())),
            ("video_shared_mb_before", fixed(before.video_shared_mb.unwrap())),
            ("video_shared_mb_after", fixed(after.video_shared_mb.unwrap())),
            ("commit_mb_before", fixed(before.commit.unwrap() as f64 / 1e6)),
            ("commit_mb_after", fixed(after.commit.unwrap() as f64 / 1e6)),
            ("working_set_mb_before", fixed(before.working_set.unwrap() as f64 / 1e6)),
            ("working_set_mb_after", fixed(after.working_set.unwrap() as f64 / 1e6)),
        ]
    }

    /// Each measurement reaches its own column and no other. The stand-in
    /// reports another figure for every quantity and at every call and
    /// keeps what it reported; every measurement column of a null row, of
    /// rows that built and rows that refitted on both paths, and of a loop
    /// row is held against exactly the figures the device gave for it.
    #[test]
    fn every_measurement_reaches_its_own_column() {
        let mut device = StandIn::default();
        let options =
            Options { timed_rays: 96, repeats: 40, discard: 3, loops: 5, ..short(&["dynamic-refit"]) };
        let results = go(&mut device, &options);
        let (frames, per_pass, listed) = (4, 2 + 3 + 40, 1200);
        // What the device was asked, in its own order: the null shader's
        // pass; then per path four passes (a frame each), five loop
        // iterations and the loop's check.
        let of = |form: Form| -> Vec<(Trace, Traced)> {
            device.traced.iter().filter(|(trace, _)| trace.form == form).copied().collect()
        };
        let pass = |all: &[(Trace, Traced)], k: usize| all[per_pass * k..per_pass * (k + 1)].to_vec();
        // A pass: the dispatch over the listed rays, the timed ones (all
        // of `timed_rays` records, none poisoned), the check.
        let timed = |pass: &[(Trace, Traced)]| -> Vec<Traced> {
            assert_eq!((pass[0].0.rays, pass[0].0.poison), (listed, true));
            assert_eq!((pass[per_pass - 1].0.rays, pass[per_pass - 1].0.poison), (96, true));
            assert!(pass[1..per_pass - 1].iter().all(|(trace, _)| trace.rays == 96 && !trace.poison));
            pass[1..per_pass - 1].iter().map(|(_, traced)| *traced).collect()
        };
        // The figures of a series: the first three are left out.
        let series = |traced: &[Traced]| -> Vec<(&'static str, String)> {
            let kept = &traced[traced.len() - 40..];
            let gpu: Vec<f64> = kept.iter().map(|t| t.gpu_ms.unwrap()).collect();
            let waits: Vec<f64> = kept.iter().map(|t| t.wait.as_secs_f64() * 1e3).collect();
            let [min, p50, p95, max] = four(&gpu);
            assert!(min != p50 && p50 != p95 && p95 != max, "the stand-in's times tell the four apart");
            vec![
                ("trace_samples", "40".into()),
                ("trace_untimed", "0".into()),
                ("trace_gpu_ms_min", min),
                ("trace_gpu_ms_p50", p50),
                ("trace_gpu_ms_p95", p95),
                ("trace_gpu_ms_max", max),
                ("trace_wait_ms_p50", middle(&waits)),
            ]
        };

        // The null row: its series, the first read-back's wait, its
        // pipeline, and the ray list's upload.
        let null_series = series(&timed(&pass(&of(Form::Null), 0)));
        let null = rows(&results, "dynamic-refit", row::NULL)[0];
        assert_eq!(device.uploaded.len(), 1);
        let mut expected = null_series.clone();
        expected.extend([
            ("readback_wait_ms", millis(device.read[0].1)),
            ("pipeline_cpu_ms", millis(crate::standin::pipeline_time(Form::Null))),
            ("upload_cpu_ms", millis(device.uploaded[0].1)),
            ("input_bytes", (48 * u64::from(listed)).to_string()),
        ]);
        assert_eq!(device.uploaded[0].0, 48 * u64::from(listed));
        measured(null, &expected, &[]);
        assert_eq!((number(null, "timed_rays"), number(null, "null_not_miss")), (96, 0));

        // The calls, in the order the run makes them: per path its frames,
        // then its loop.
        let per_path_builds = frames + 5;
        let per_path_memories = 1 + frames + 1;
        let per_path_reads = 2 * frames + 1;
        for (p, path) in Path::ALL.into_iter().enumerate() {
            let all = of(path.form());
            let of_path = rows(&results, "dynamic-refit", path.name());
            assert_eq!(of_path.len(), frames + 1);
            let before = &device.memories[per_path_memories * p];
            for (frame, row) in of_path.iter().copied().enumerate().take(frames) {
                let (at, built) = &device.built[per_path_builds * p + frame];
                assert_eq!(*at, path);
                let mut expected = series(&timed(&pass(&all, frame)));
                // The null shader's figures beside: its own columns' values.
                for (column, value) in &null_series[2..6] {
                    let beside = column.replace("trace_gpu_ms", "null_gpu_ms");
                    let name = row::COLUMNS.iter().find(|(name, _)| *name == beside).unwrap().0;
                    expected.push((name, value.clone()));
                }
                let (refits, builds) = results.cases[0].frames[frame].actions.asked();
                expected.extend([
                    ("builds_asked", builds.to_string()),
                    ("refits_asked", refits.to_string()),
                    ("builds_done", built.builds.to_string()),
                    ("refits_done", built.refits.to_string()),
                    ("build_untimed", "0".into()),
                    ("upload_cpu_ms", millis(built.upload_cpu)),
                    ("record_cpu_ms", millis(built.record_cpu)),
                    ("build_wait_ms", millis(built.wait)),
                    ("input_bytes", built.input_bytes.to_string()),
                    ("readback_wait_ms", millis(device.read[2 + per_path_reads * p + 2 * frame].1)),
                ]);
                // The card's time by kind, each where the frame did it.
                for (column, gpu) in [
                    ("build_gpu_ms", built.bottom_build),
                    ("refit_gpu_ms", built.bottom_refit),
                    ("tlas_build_gpu_ms", built.top_build),
                    ("tlas_refit_gpu_ms", built.top_refit),
                ] {
                    expected.extend(gpu.whole().map(|ms| (column, fixed(ms))));
                }
                if frame == 0 {
                    expected.push(("pipeline_cpu_ms", millis(crate::standin::pipeline_time(path.form()))));
                    assert_eq!((built.builds, built.refits), (4, 0));
                    assert!(built.bottom_build.whole().is_some() && built.top_build.whole().is_some());
                } else {
                    assert_eq!((built.builds, built.refits), (0, 3));
                    assert!(built.bottom_refit.whole().is_some() && built.top_refit.whole().is_some());
                }
                expected.extend(memory_columns(before, &device.memories[per_path_memories * p + 1 + frame]));
                measured(row, &expected, &["convert_cpu_ms"]);
                assert_eq!(number(row, "timed_rays"), 96);
            }
            // A refit uploads what moves; the hybrid's triangle data goes
            // with it.
            let (first, second) = (&device.built[per_path_builds * p].1, &device.built[per_path_builds * p + 1].1);
            assert!(second.input_bytes > 0 && second.input_bytes < first.input_bytes, "{}", path.name());

            // The loop row: five dispatches over the listed rays, none
            // left out; the builds' figures are medians over the five
            // iterations, the counts and bytes are sums.
            let looped = of_path[frames];
            assert_eq!(looped.get("frame"), "loop");
            let iterations = &all[per_pass * frames..];
            assert_eq!(iterations.len(), 5 + 1, "five iterations and the check");
            assert!(iterations[..5].iter().all(|(trace, _)| trace.rays == listed && !trace.poison));
            assert_eq!((iterations[5].0.rays, iterations[5].0.poison), (listed, true));
            let gpu: Vec<f64> = iterations[..5].iter().map(|(_, t)| t.gpu_ms.unwrap()).collect();
            let waits: Vec<f64> = iterations[..5].iter().map(|(_, t)| t.wait.as_secs_f64() * 1e3).collect();
            let [min, p50, p95, max] = four(&gpu);
            let builds: Vec<&Built> =
                device.built[per_path_builds * p + frames..per_path_builds * (p + 1)].iter().map(|(_, b)| b).collect();
            let median = |pick: &dyn Fn(&Built) -> Option<f64>| -> Option<String> {
                let mut values: Vec<f64> = builds.iter().filter_map(|b| pick(b)).collect();
                values.sort_by(f64::total_cmp);
                match values.len() {
                    0 => None,
                    n if n % 2 == 1 => Some(fixed(values[n / 2])),
                    n => Some(fixed((values[n / 2 - 1] + values[n / 2]) / 2.0)),
                }
            };
            let time = |pick: &dyn Fn(&Built) -> Duration| median(&|b| Some(pick(b).as_secs_f64() * 1e3)).unwrap();
            let mut expected = vec![
                ("loop_iterations", "5".to_string()),
                ("loop_last_differing", "0".into()),
                ("trace_samples", "5".into()),
                ("trace_untimed", "0".into()),
                ("trace_gpu_ms_min", min),
                ("trace_gpu_ms_p50", p50),
                ("trace_gpu_ms_p95", p95),
                ("trace_gpu_ms_max", max),
                ("trace_wait_ms_p50", middle(&waits)),
                ("builds_done", builds.iter().map(|b| b.builds).sum::<u32>().to_string()),
                ("refits_done", builds.iter().map(|b| b.refits).sum::<u32>().to_string()),
                ("build_untimed", "0".into()),
                ("input_bytes", builds.iter().map(|b| b.input_bytes).sum::<u64>().to_string()),
                ("upload_cpu_ms", time(&|b| b.upload_cpu)),
                ("record_cpu_ms", time(&|b| b.record_cpu)),
                ("build_wait_ms", time(&|b| b.wait)),
                ("readback_wait_ms", millis(device.read[2 + per_path_reads * p + 2 * frames].1)),
            ];
            for (column, pick) in [
                ("build_gpu_ms", (|b: &Built| b.bottom_build.whole()) as fn(&Built) -> Option<f64>),
                ("refit_gpu_ms", |b| b.bottom_refit.whole()),
                ("tlas_build_gpu_ms", |b| b.top_build.whole()),
                ("tlas_refit_gpu_ms", |b| b.top_refit.whole()),
            ] {
                expected.extend(median(&pick).map(|value| (column, value)));
            }
            let has = |column: &str| expected.iter().any(|(c, _)| *c == column);
            assert!(has("build_gpu_ms") && has("refit_gpu_ms"), "the loop built and refitted");
            expected.extend(memory_columns(before, &device.memories[per_path_memories * p + 1 + frames]));
            let clock = ["convert_cpu_ms", "iter_ms_min", "iter_ms_p50", "iter_ms_p95", "iter_ms_p99", "iter_ms_max"];
            measured(looped, &expected, &clock);
            assert_eq!(number(looped, "timed_rays"), u64::from(listed), "a loop's dispatch lists every ray once");
            let wall = |column: &str| looped.get(column).parse::<f64>().unwrap();
            assert!(wall("iter_ms_min") <= wall("iter_ms_p50") && wall("iter_ms_p50") <= wall("iter_ms_p95"));
            assert!(wall("iter_ms_p95") <= wall("iter_ms_p99") && wall("iter_ms_p99") <= wall("iter_ms_max"));
        }
        // Every call the device recorded was held against a row.
        assert_eq!(device.built.len(), 2 * per_path_builds);
        assert_eq!(device.memories.len(), 2 * per_path_memories);
        assert_eq!(device.read.len(), 2 + 2 * per_path_reads);
        // The null shader's pass; per path a pass a frame, five iterations
        // and the loop's check.
        assert_eq!(device.traced.len(), per_pass + 2 * (per_pass * frames + 5 + 1));
        has_records_where_the_refusal_counts_are_filled(&results);
    }

    /// With a validation layer on, what it says goes to the console as it
    /// comes and into the note of the row it was said in.
    #[test]
    fn what_a_validation_layer_says_is_printed_and_noted() {
        let mut device = StandIn { complains: true, ..StandIn::default() };
        let mut said = Vec::new();
        let results = run(&mut device, &short(&["static-soup"]), &mut |line| said.push(line.to_string())).unwrap();
        let has = |line: &str| said.iter().any(|l| l == line);
        assert!(has("validation layer, null pipeline: the Null pipeline: a message"), "{said:?}");
        assert!(has("validation layer, static-soup native frame 0: build 0: a message, with a comma"), "{said:?}");
        assert!(has("validation layer, static-soup hybrid frame 0: build 1: another"), "{said:?}");
        let made = results.table.value("pipeline.triangles").unwrap();
        assert!(made.ends_with("the validation layer said 1 messages, the first: the Triangles pipeline: a message"));
        let native = rows(&results, "static-soup", row::NATIVE)[0];
        assert_eq!(native.get("status"), row::RAN);
        assert_eq!(
            native.get("note"),
            "the validation layer said 2 messages; the first: build 0: a message; with a comma"
        );
        // Without one nothing is said and no note is made.
        let results = go(&mut StandIn::default(), &short(&["static-soup"]));
        assert_eq!(rows(&results, "static-soup", row::NATIVE)[0].get("note"), "");
    }

    #[test]
    fn repeats_that_differ_and_dispatches_without_a_time_are_counted() {
        let mut device = StandIn { flickers: true, untimed_every: Some(4), ..StandIn::default() };
        let options = Options { repeats: 6, discard: 2, ..short(&["static-soup"]) };
        let results = go(&mut device, &options);
        for path in [row::NATIVE, row::HYBRID] {
            let row = rows(&results, "static-soup", path)[0];
            assert_eq!(number(row, "repeat_differing"), 1, "{path}");
            assert_eq!(number(row, "trace_samples") + number(row, "trace_untimed"), 6);
            assert!(number(row, "trace_untimed") >= 1);
            let figure = |column: &str| row.get(column).parse::<f64>().unwrap();
            assert!(figure("trace_gpu_ms_min") <= figure("trace_gpu_ms_p50"));
            assert!(figure("trace_gpu_ms_p50") <= figure("trace_gpu_ms_p95"));
            assert!(figure("trace_gpu_ms_p95") <= figure("trace_gpu_ms_max"));
        }
        let null = rows(&results, "static-soup", row::NULL)[0];
        assert_eq!(number(null, "null_not_miss"), 0, "the null shader does not flicker");
    }

    #[test]
    fn the_loop_mode_walks_the_frames_again_and_ends_on_the_records_it_began_with() {
        let mut device = StandIn::default();
        let options = Options { loops: 6, ..short(&["dynamic-refit", "static-soup"]) };
        let results = go(&mut device, &options);
        for path in [row::NATIVE, row::HYBRID] {
            let moving = rows(&results, "dynamic-refit", path);
            let looped = moving.last().unwrap();
            assert_eq!((looped.get("frame"), looped.get("status")), ("loop", row::RAN), "{:?}", looped.get("note"));
            assert_eq!((number(looped, "loop_iterations"), number(looped, "loop_last_differing")), (6, 0));
            // Six iterations from the last frame's state: frame 0 again
            // (what differs is built), then 1, 2, 3 as the case says, then
            // 0 and 1.
            assert_eq!(number(looped, "builds_done"), 2 * 3);
            assert_eq!(number(looped, "refits_done"), 4 * 3);
            assert!(!looped.get("iter_ms_p99").is_empty() && !looped.get("convert_cpu_ms").is_empty());
            // A case of one frame has nothing to build in a loop.
            let still = rows(&results, "static-soup", path);
            let looped = still.last().unwrap();
            assert_eq!((looped.get("frame"), number(looped, "loop_iterations")), ("loop", 6));
            assert_eq!((looped.get("builds_done"), looped.get("build_gpu_ms")), ("", ""));
            assert_eq!(number(looped, "loop_last_differing"), 0);
        }
        let planned = results.table.value("planned").unwrap();
        assert!(planned.contains(&format!("; {} dispatches (", device.dispatches)), "{planned}; {}", device.dispatches);
    }

    #[test]
    fn a_runs_files_are_written_and_read_back() {
        let mut device = StandIn { native_refuses_every: Some(50), ..StandIn::default() };
        let results = go(&mut device, &short(&["dynamic-refit", "alpha-tested"]));
        let dir = std::env::temp_dir().join(format!("victory-rt-probe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let outputs = crate::file::Outputs::beside(&dir.join("rt-probe.csv"));
        // A file an earlier run left for a path that has no records now
        // would be read as this run's: none is left for a path with
        // records either, it is written over.
        std::fs::write(&outputs.hits[0].1, "an earlier run's").unwrap();
        assert_eq!(outputs.prepare(), Ok(()));
        assert_eq!(std::fs::read_to_string(&outputs.hits[0].1).unwrap(), "an earlier run's", "trying changes nothing");
        assert!(!outputs.table.exists() && !outputs.mismatches.exists(), "and leaves nothing behind");
        let written = crate::file::write(&results, &outputs);
        assert_eq!((written.files.len(), &written.failed, &written.table_lost), (4, &vec![], &None));
        assert_eq!(written.files.last().unwrap().0, outputs.table, "the table last");
        let table = Table::parse(&std::fs::read_to_string(&outputs.table).unwrap()).unwrap();
        assert_eq!(table, results.table);
        assert_eq!(table.value(crate::file::NOT_WRITTEN), None);
        for (path, file) in &outputs.hits {
            let sections = adapter::read_hits(&std::fs::read_to_string(file).unwrap()).unwrap();
            assert_eq!(sections.len(), 5, "{}", path.name());
            assert_eq!(sections[4].class, "alpha-tested");
            assert_eq!(sections[0].records.len(), results.cases[0].rays.len());
        }
        let mismatches = std::fs::read_to_string(&outputs.mismatches).unwrap();
        assert_eq!(mismatches.lines().count(), 1, "the column line alone: {mismatches}");
        std::fs::remove_dir_all(&dir).unwrap();
        has_records_where_the_refusal_counts_are_filled(&results);
    }

    /// Whether the files can be written is found out before a run, and a
    /// file that cannot be written at its end does not lose the others.
    #[test]
    fn a_place_that_cannot_be_written_is_found_before_the_run_and_loses_no_other_file_after_it() {
        let dir = std::env::temp_dir().join(format!("victory-rt-probe-test-places-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The folder is made, down to the last part of its path.
        let outputs = crate::file::Outputs::beside(&dir.join("deep").join("er").join("rt-probe.csv"));
        assert_eq!(outputs.prepare(), Ok(()));
        assert!(dir.join("deep").join("er").is_dir());
        assert_eq!(std::fs::read_dir(dir.join("deep").join("er")).unwrap().count(), 0, "no file is left there");
        // A name that is taken by a folder is refused, before any run.
        std::fs::create_dir(&outputs.hits[0].1).unwrap();
        let refused = outputs.prepare().unwrap_err();
        assert!(refused.contains("rt-probe.native.hits is a folder"), "{refused}");
        // A folder that cannot be made, because a file has its name.
        std::fs::write(dir.join("a-file"), "x").unwrap();
        let under = crate::file::Outputs::beside(&dir.join("a-file").join("rt-probe.csv"));
        assert!(under.prepare().unwrap_err().starts_with("creating the folder "));

        // The same at the end of a run, had it come about in between: the
        // native records cannot be written; everything else is, and the
        // table says what is missing beside it.
        let results = go(&mut StandIn::default(), &short(&["static-soup"]));
        let written = crate::file::write(&results, &outputs);
        assert_eq!(written.failed.len(), 1);
        assert!(written.failed[0].contains("rt-probe.native.hits: "), "{:?}", written.failed);
        let names: Vec<&std::path::Path> = written.files.iter().map(|(path, _)| path.as_path()).collect();
        assert_eq!(names, [outputs.hits[1].1.as_path(), outputs.mismatches.as_path(), outputs.table.as_path()]);
        assert_eq!(written.table_lost, None);
        let table = Table::parse(&std::fs::read_to_string(&outputs.table).unwrap()).unwrap();
        assert_eq!(table.value(crate::file::NOT_WRITTEN), Some(written.failed[0].as_str()));
        assert_eq!(table.rows, results.table.rows);
        assert_eq!(adapter::read_hits(&std::fs::read_to_string(&outputs.hits[1].1).unwrap()).unwrap().len(), 1);

        // The table itself cannot be written: its text is handed back, so
        // that the caller can still put it somewhere.
        std::fs::remove_file(&outputs.table).unwrap();
        std::fs::create_dir(&outputs.table).unwrap();
        let written = crate::file::write(&results, &outputs);
        assert_eq!(written.failed.len(), 2);
        let lost = Table::parse(written.table_lost.as_deref().unwrap()).unwrap();
        assert_eq!(lost.rows, results.table.rows);
        assert!(lost.value(crate::file::NOT_WRITTEN).unwrap().contains("rt-probe.native.hits"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The length of a default run, as docs/dxr-validation.md gives it to
    /// the owner: counted here from the generated classes, at each seed the
    /// owner is asked to run.
    #[test]
    fn a_default_run_is_as_long_as_the_specification_says() {
        let spec = include_str!("../../../docs/dxr-validation.md");
        for seed in [1, 2, adapter::default_seed()] {
            let options = Options { seed, ..Options::default() };
            let (mut sizes, mut rays, mut asked) = (Vec::new(), 0, (0, 0));
            for class in &options.classes {
                let case = adapter::case(class, seed).unwrap();
                let listed = rays::map(&case.rays).records.len() as u32;
                assert_eq!(listed as usize, case.rays.len(), "{class}: the generator makes no ray D3D12 forbids");
                rays += case.rays.len() * case.frames.len();
                for frame in &case.frames {
                    let (refits, builds) = frame.actions.asked();
                    asked = (asked.0 + builds, asked.1 + refits);
                }
                sizes.push(ClassSize { frames: case.frames.len() as u32, listed });
            }
            let line = plan::counts(&options, &sizes).line();
            assert!(spec.contains(&format!("\n> {line}\n")), "seed {seed:x}: {line}");
            assert_eq!((rays, asked), (23_635, (37, 9)), "seed {seed:x}");
            let looped = plan::counts(&Options { loops: 600, ..options }, &sizes);
            assert_eq!((looped.dispatches, looped.rays_traced), (13_490, 1_160_652_828));
        }
    }

    #[test]
    fn options_that_make_no_run_stop_it_before_the_device_is_asked_anything() {
        let mut device = StandIn::default();
        let options = Options { repeats: 0, ..short(&["static-soup"]) };
        assert!(run(&mut device, &options, &mut |_| {}).is_err());
        assert!(device.log.is_empty());
    }

    #[test]
    fn a_containers_hash_is_the_digest_in_its_header() {
        let shader = rayquery::build(Form::Null);
        let hash = container_hash(&shader.dxil);
        assert_eq!(hash.len(), 32);
        assert_eq!(&shader.dxil[..4], b"DXBC");
        assert_eq!(u8::from_str_radix(&hash[..2], 16).unwrap(), shader.dxil[4]);
        assert_eq!(container_hash(&[1, 2, 3]), "");
    }
}
