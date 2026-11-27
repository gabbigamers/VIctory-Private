//! `cargo xtask rt-summary`: a result table of `victory rt-probe`
//! (docs/dxr-validation.md) as text, and two runs side by side.
//!
//! The table has `#` lines of `key=value`, a line of column names and a row
//! per scene class, frame and path. Columns are found by name, so a table
//! from an older or newer probe reads as far as it has the columns; an
//! empty field is "does not apply" or "not measured", never 0, and is shown
//! as `-`.
//!
//! What this prints is what the file holds, each measurement in its own
//! table: correctness counts, the card's time of the traversal dispatch
//! alone with the null shader's beside it, the card's time of builds and
//! refits, processor time, time blocked waiting for the card, video memory
//! by owner, system memory. Nothing is added to anything else, no ratio or
//! difference of two times is formed, and nothing is called faster or
//! slower: the probe's times are of synthetic scenes, select no path and
//! say nothing about a title. Its verdicts are `victory-rt`'s, under that
//! crate's policy and not yet the rule of docs/dxr-compatibility.md, so no
//! row is called a pass here either.
//!
//! With `--against`, the other run is taken as an earlier run of the same
//! seed on the same device and driver. What must then be equal is compared
//! and named where it is not: the counts of every row, and the hit records
//! in the files beside the two tables. Times are shown side by side without
//! a word on which is better. The comparison ends in one of three lines:
//! the two runs agree, they differ, or they were **not compared**. Agreement
//! is said only when every section of hit records the two tables' rows
//! have was found in both files and is the same; a file that is missing, a
//! section that is in neither file, and two runs with no hit record at all
//! are not agreement.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// How a table begins.
const MAGIC: &str = "# victory rt-probe results";

/// The tracing paths, in the order they are shown.
const TRACING: [&str; 2] = ["native", "hybrid"];

/// A defect, by `victory-rt`'s kinds.
const DEFECTS: [&str; 6] = ["missed", "phantom", "wrong_triangle", "wrong_face", "numeric", "malformed"];

/// Columns that count: two runs of one seed on one device and driver must
/// agree on each, row by row.
const COUNTS: &[&str] = &[
    "rays",
    "rays_listed",
    "refused_before",
    "refused_geometry",
    "refused_range",
    "unwritten",
    "refused_malformed",
    "refused_taken_by_next",
    "refused_unanswered",
    "judged",
    "clear",
    "ambiguous",
    "excluded",
    "wide",
    "matched",
    "permitted_primary",
    "permitted_other",
    "ids_only",
    "beyond_band",
    "missed",
    "phantom",
    "wrong_triangle",
    "wrong_face",
    "numeric",
    "malformed",
    "faces_not_reported",
    "model_defects",
    "model_identical",
    "model_divergent",
    "model_divergent_first_hit",
    "repeat_differing",
    "null_not_miss",
    "loop_last_differing",
];

/// Every column this summary reads; a test holds the probe's own list
/// against it.
#[cfg(test)]
const READ: &[&str] = &[
    "class",
    "frame",
    "path",
    "status",
    "note",
    "verdict",
    "timed_rays",
    "trace_samples",
    "trace_untimed",
    "trace_gpu_ms_min",
    "trace_gpu_ms_p50",
    "trace_gpu_ms_p95",
    "trace_gpu_ms_max",
    "null_gpu_ms_min",
    "null_gpu_ms_p50",
    "null_gpu_ms_p95",
    "null_gpu_ms_max",
    "builds_done",
    "refits_done",
    "build_gpu_ms",
    "refit_gpu_ms",
    "tlas_build_gpu_ms",
    "tlas_refit_gpu_ms",
    "build_untimed",
    "convert_cpu_ms",
    "upload_cpu_ms",
    "record_cpu_ms",
    "pipeline_cpu_ms",
    "build_wait_ms",
    "trace_wait_ms_p50",
    "readback_wait_ms",
    "own_rt_structures_bytes",
    "own_rt_scratch_bytes",
    "own_rt_buffers_bytes",
    "video_local_mb_before",
    "video_local_mb_after",
    "video_budget_mb",
    "commit_mb_before",
    "commit_mb_after",
    "working_set_mb_before",
    "working_set_mb_after",
    "loop_iterations",
    "iter_ms_min",
    "iter_ms_p50",
    "iter_ms_p95",
    "iter_ms_p99",
    "iter_ms_max",
    "model_refusal",
    "primitives",
];

/// A parsed table.
#[derive(Debug)]
pub struct Table {
    header: Vec<(String, String)>,
    columns: HashMap<String, usize>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines().enumerate().map(|(n, line)| (n + 1, line));
        match lines.next() {
            Some((_, first)) if first.starts_with(MAGIC) => {}
            _ => bail!("not a result table of victory rt-probe: its first line does not begin with \"{MAGIC}\""),
        }
        let mut header = Vec::new();
        let mut columns: Option<HashMap<String, usize>> = None;
        let mut rows = Vec::new();
        for (n, line) in lines {
            if line.trim().is_empty() {
                continue;
            }
            match (&columns, line.strip_prefix('#')) {
                (None, Some(rest)) => {
                    let (key, value) =
                        rest.trim_start().split_once('=').with_context(|| format!("line {n}: no key=value"))?;
                    header.push((key.trim().to_string(), value.trim().to_string()));
                }
                (Some(_), Some(_)) => bail!("line {n}: a header line among the rows"),
                (None, None) => {
                    columns = Some(line.split(',').enumerate().map(|(i, name)| (name.trim().to_string(), i)).collect());
                }
                (Some(columns), None) => {
                    let row: Vec<String> = line.split(',').map(|field| field.trim().to_string()).collect();
                    if row.len() != columns.len() {
                        bail!("line {n}: {} fields for {} columns", row.len(), columns.len());
                    }
                    rows.push(row);
                }
            }
        }
        let columns = columns.context("the table ends before its line of column names")?;
        for needed in ["class", "frame", "path", "status"] {
            if !columns.contains_key(needed) {
                bail!("the table has no `{needed}` column");
            }
        }
        Ok(Self { header, columns, rows })
    }

    /// The first header value of `key`.
    fn value(&self, key: &str) -> Option<&str> {
        self.header.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// The field of `row` in `column`; empty where the table has no such
    /// column.
    fn get(&self, row: usize, column: &str) -> &str {
        self.columns.get(column).map_or("", |&at| self.rows[row][at].as_str())
    }

    fn number(&self, row: usize, column: &str) -> Option<f64> {
        self.get(row, column).parse().ok()
    }

    fn count(&self, row: usize, column: &str) -> u64 {
        self.get(row, column).parse().unwrap_or(0)
    }

    /// The rows of `class` on `path`, in the file's order.
    fn rows_of(&self, class: &str, path: &str) -> Vec<usize> {
        (0..self.rows.len()).filter(|&r| self.get(r, "class") == class && self.get(r, "path") == path).collect()
    }

    /// The classes, in the order of their first rows.
    fn classes(&self) -> Vec<&str> {
        let mut classes: Vec<&str> = Vec::new();
        for row in 0..self.rows.len() {
            let class = self.get(row, "class");
            if !classes.contains(&class) {
                classes.push(class);
            }
        }
        classes
    }

    /// What names a row: class, frame and path.
    fn key(&self, row: usize) -> (String, String, String) {
        (self.get(row, "class").into(), self.get(row, "frame").into(), self.get(row, "path").into())
    }

    fn defects(&self, row: usize) -> u64 {
        DEFECTS.iter().map(|column| self.count(row, column)).sum()
    }
}

/// A field as shown: `-` where it is empty.
fn shown(field: &str) -> &str {
    if field.is_empty() { "-" } else { field }
}

/// A time or a size with three decimals, `-` where there is none.
fn ms(value: Option<f64>) -> String {
    value.map_or("-".to_string(), |v| format!("{v:.3}"))
}

/// A frame as shown: the null shader's row has none.
fn frame(field: &str) -> &str {
    if field.is_empty() { "all" } else { field }
}

/// The first lines: which run this is, and how to read it.
fn head(table: &Table) -> String {
    let value = |key: &str| table.value(key).unwrap_or("not recorded");
    let mut out = String::new();
    let _ = writeln!(out, "victory rt-probe, probe version {}, seed {}", value("probe_version"), value("seed"));
    let _ = writeln!(
        out,
        "  device: {}; driver {}; ray tracing tier {}",
        value("adapter"),
        value("driver"),
        value("raytracing_tier")
    );
    let _ = writeln!(out, "  classes: {}", value("classes"));
    let _ = writeln!(
        out,
        "  a timed dispatch lists {} ray records; {} kept after {} left out; loop {}",
        value("timed_rays"),
        value("repeats"),
        value("discard"),
        value("loop")
    );
    let _ = writeln!(out, "  planned: {}", value("planned"));
    let _ = writeln!(out, "  rows: {}", value("rows"));
    let _ = writeln!(out, "  verdicts: {}", value("policy"));
    if let Some(layer) = table.value("d3d12_debug_layer").filter(|layer| layer.starts_with("on")) {
        let _ = writeln!(out, "  DEBUG LAYER {layer}");
    }
    if let Some(missing) = table.value("files_not_written") {
        let _ = writeln!(out, "  FILES NOT WRITTEN beside this table: {missing}");
    }
    out
}

/// Sums and counts over one class and path.
fn correctness(table: &Table, out: &mut String) {
    let _ = writeln!(out, "\ncorrectness, by class and path (frames together; the loop rows apart, below)");
    let _ = writeln!(
        out,
        "  {:<16} {:<7} {:>9} {:>8} {:>8} {:>6} {:>9} {:>8} {:>8} {:>9} {:>7} {:>9} {:>9}  verdicts on the rays judged (crate's policy)",
        "class", "path", "ran", "rays", "judged", "clear", "ambiguous", "excluded", "defects", "unwritten", "beyond", "refused", "differing"
    );
    for class in table.classes() {
        for path in TRACING {
            let rows: Vec<usize> =
                table.rows_of(class, path).into_iter().filter(|&r| table.get(r, "frame") != "loop").collect();
            if rows.is_empty() {
                continue;
            }
            let sum = |column: &str| -> u64 { rows.iter().map(|&r| table.count(r, column)).sum() };
            let ran = rows.iter().filter(|&&r| table.get(r, "status") == "ran").count();
            let mut verdicts: Vec<(&str, usize)> = Vec::new();
            for &row in &rows {
                let verdict = shown(table.get(row, "verdict"));
                match verdicts.iter_mut().find(|(v, _)| *v == verdict) {
                    Some((_, n)) => *n += 1,
                    None => verdicts.push((verdict, 1)),
                }
            }
            let verdicts: Vec<String> = verdicts.iter().map(|(v, n)| format!("{v} {n}")).collect();
            let refused = sum("refused_taken_by_next") + sum("refused_unanswered");
            let _ = writeln!(
                out,
                "  {:<16} {:<7} {:>9} {:>8} {:>8} {:>6} {:>9} {:>8} {:>8} {:>9} {:>7} {:>9} {:>9}  {}",
                class,
                path,
                format!("{ran}/{}", rows.len()),
                sum("rays"),
                sum("judged"),
                sum("clear"),
                sum("ambiguous"),
                sum("excluded"),
                rows.iter().map(|&r| table.defects(r)).sum::<u64>(),
                sum("unwritten"),
                sum("beyond_band"),
                refused,
                sum("repeat_differing"),
                verdicts.join(", ")
            );
        }
    }
    let _ = writeln!(
        out,
        "  (rays and judged are summed over frames; unwritten: records the dispatch left as they were poisoned, \
         each judged and a defect unless its ray is excluded; refused: rays the path refused, before or in the \
         dispatch, which no verdict speaks of; differing: records of the dispatch that checks the timed ones that \
         are not the first dispatch's, or are unwritten)"
    );
    let not_built = (0..table.rows.len()).filter(|&r| table.get(r, "status") == "not built").count();
    let software = table.value("path.software-gpu").unwrap_or("not recorded");
    let _ = writeln!(out, "  software-gpu: {software} ({not_built} rows say so and hold nothing else)");
}

/// The rows that ran on `paths`, loop rows left out unless `loops`.
fn ran(table: &Table, paths: &[&str], loops: bool) -> Vec<usize> {
    (0..table.rows.len())
        .filter(|&r| {
            paths.contains(&table.get(r, "path"))
                && table.get(r, "status") == "ran"
                && (table.get(r, "frame") == "loop") == loops
        })
        .collect()
}

fn traversal(table: &Table, out: &mut String) {
    let _ = writeln!(
        out,
        "\nGPU time of the traversal dispatch alone, ms: least, median, 95th percentile, greatest over the kept \
         repeats; the null shader's beside it"
    );
    let _ = writeln!(
        out,
        "  {:<16} {:>5} {:<7} {:>9} {:>7} {:>9} {:>9} {:>9} {:>9}   {:>9} {:>9} {:>9} {:>9}",
        "class", "frame", "path", "records", "samples", "min", "p50", "p95", "max", "null min", "null p50", "null p95", "null max"
    );
    for row in ran(table, &["null", "native", "hybrid"], false) {
        let n = |column: &str| ms(table.number(row, column));
        let _ = writeln!(
            out,
            "  {:<16} {:>5} {:<7} {:>9} {:>7} {:>9} {:>9} {:>9} {:>9}   {:>9} {:>9} {:>9} {:>9}",
            table.get(row, "class"),
            frame(table.get(row, "frame")),
            table.get(row, "path"),
            shown(table.get(row, "timed_rays")),
            shown(table.get(row, "trace_samples")),
            n("trace_gpu_ms_min"),
            n("trace_gpu_ms_p50"),
            n("trace_gpu_ms_p95"),
            n("trace_gpu_ms_max"),
            n("null_gpu_ms_min"),
            n("null_gpu_ms_p50"),
            n("null_gpu_ms_p95"),
            n("null_gpu_ms_max"),
        );
    }
}

fn builds(table: &Table, out: &mut String) {
    let _ = writeln!(out, "\nGPU time of builds and refits, ms, apart from traversal (each structure between timestamps of its own)");
    let _ = writeln!(
        out,
        "  {:<16} {:>5} {:<7} {:>10} {:>6} {:>7} {:>13} {:>13} {:>10} {:>10} {:>8}",
        "class", "frame", "path", "primitives", "built", "refits", "bottom builds", "bottom refits", "top build", "top refit", "untimed"
    );
    for row in ran(table, &TRACING, false) {
        let n = |column: &str| ms(table.number(row, column));
        let _ = writeln!(
            out,
            "  {:<16} {:>5} {:<7} {:>10} {:>6} {:>7} {:>13} {:>13} {:>10} {:>10} {:>8}",
            table.get(row, "class"),
            frame(table.get(row, "frame")),
            table.get(row, "path"),
            shown(table.get(row, "primitives")),
            shown(table.get(row, "builds_done")),
            shown(table.get(row, "refits_done")),
            n("build_gpu_ms"),
            n("refit_gpu_ms"),
            n("tlas_build_gpu_ms"),
            n("tlas_refit_gpu_ms"),
            shown(table.get(row, "build_untimed")),
        );
    }
}

fn processor(table: &Table, out: &mut String) {
    let _ = writeln!(
        out,
        "\nprocessor time, ms: turning the scene into build inputs, uploading them (null row: uploading the \
         class's ray list), describing and recording the builds, creating the pipeline (first use only)"
    );
    let _ = writeln!(
        out,
        "  {:<16} {:>5} {:<7} {:>10} {:>10} {:>10} {:>10}",
        "class", "frame", "path", "convert", "upload", "record", "pipeline"
    );
    for row in ran(table, &["null", "native", "hybrid"], false) {
        let n = |column: &str| ms(table.number(row, column));
        let _ = writeln!(
            out,
            "  {:<16} {:>5} {:<7} {:>10} {:>10} {:>10} {:>10}",
            table.get(row, "class"),
            frame(table.get(row, "frame")),
            table.get(row, "path"),
            n("convert_cpu_ms"),
            n("upload_cpu_ms"),
            n("record_cpu_ms"),
            n("pipeline_cpu_ms"),
        );
    }
    let _ = writeln!(
        out,
        "\nblocked waiting for the card, ms: after the builds' submission, after a timed dispatch's (median), for \
         the copy of the hit records"
    );
    let _ = writeln!(out, "  {:<16} {:>5} {:<7} {:>10} {:>10} {:>10}", "class", "frame", "path", "builds", "dispatch", "read-back");
    for row in ran(table, &["null", "native", "hybrid"], false) {
        let n = |column: &str| ms(table.number(row, column));
        let _ = writeln!(
            out,
            "  {:<16} {:>5} {:<7} {:>10} {:>10} {:>10}",
            table.get(row, "class"),
            frame(table.get(row, "frame")),
            table.get(row, "path"),
            n("build_wait_ms"),
            n("trace_wait_ms_p50"),
            n("readback_wait_ms"),
        );
    }
}

fn memory(table: &Table, out: &mut String) {
    let _ = writeln!(
        out,
        "\nvideo memory by owner after each frame, MB of 10^6 bytes by our own count; the adapter's figure for the \
         whole process beside it (cross-check only)"
    );
    let _ = writeln!(
        out,
        "  {:<16} {:>5} {:<7} {:>11} {:>9} {:>9}   {:>12} {:>12} {:>10}",
        "class", "frame", "path", "structures", "scratch", "buffers", "card before", "card after", "budget"
    );
    let own = |row: usize, column: &str| ms(table.number(row, column).map(|bytes| bytes / 1e6));
    for row in ran(table, &TRACING, false) {
        let n = |column: &str| ms(table.number(row, column));
        let _ = writeln!(
            out,
            "  {:<16} {:>5} {:<7} {:>11} {:>9} {:>9}   {:>12} {:>12} {:>10}",
            table.get(row, "class"),
            frame(table.get(row, "frame")),
            table.get(row, "path"),
            own(row, "own_rt_structures_bytes"),
            own(row, "own_rt_scratch_bytes"),
            own(row, "own_rt_buffers_bytes"),
            n("video_local_mb_before"),
            n("video_local_mb_after"),
            n("video_budget_mb"),
        );
    }
    let _ = writeln!(
        out,
        "  (structures and scratch are on the card; buffers are system memory the card reads, and are not to be \
         added to the other two)"
    );
    let _ = writeln!(out, "\nsystem memory of the process, MB, before the path's first build of the class and after each frame");
    let _ = writeln!(
        out,
        "  {:<16} {:>5} {:<7} {:>14} {:>13} {:>19} {:>18}",
        "class", "frame", "path", "commit before", "commit after", "working set before", "working set after"
    );
    for row in ran(table, &TRACING, false) {
        let n = |column: &str| ms(table.number(row, column));
        let _ = writeln!(
            out,
            "  {:<16} {:>5} {:<7} {:>14} {:>13} {:>19} {:>18}",
            table.get(row, "class"),
            frame(table.get(row, "frame")),
            table.get(row, "path"),
            n("commit_mb_before"),
            n("commit_mb_after"),
            n("working_set_mb_before"),
            n("working_set_mb_after"),
        );
    }
}

fn loops(table: &Table, out: &mut String) {
    let rows = ran(table, &TRACING, true);
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\nloop mode: wall time of one iteration on the submitting thread, ms (conversion, builds, their wait, the \
         dispatch and its wait)"
    );
    let _ = writeln!(
        out,
        "  {:<16} {:<7} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>10}",
        "class", "path", "iterations", "min", "p50", "p95", "p99", "max", "differing"
    );
    for row in rows {
        let n = |column: &str| ms(table.number(row, column));
        let _ = writeln!(
            out,
            "  {:<16} {:<7} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>10}",
            table.get(row, "class"),
            table.get(row, "path"),
            shown(table.get(row, "loop_iterations")),
            n("iter_ms_min"),
            n("iter_ms_p50"),
            n("iter_ms_p95"),
            n("iter_ms_p99"),
            n("iter_ms_max"),
            shown(table.get(row, "loop_last_differing")),
        );
    }
}

/// What a reader should look at first: each line is a row and what it says.
pub fn to_look_at(table: &Table) -> Vec<String> {
    let mut list = Vec::new();
    for row in 0..table.rows.len() {
        let (class, at, path) = table.key(row);
        let name = format!("{class}, frame {}, {path}", frame(&at));
        let mut say = |what: String| list.push(format!("{name}: {what}"));
        match table.get(row, "status") {
            "failed" => say(format!("failed: {}", shown(table.get(row, "note")))),
            "not run" => say(format!("not run: {}", shown(table.get(row, "note")))),
            _ => {}
        }
        let defects = table.defects(row);
        if defects > 0 {
            let kinds: Vec<String> = DEFECTS
                .iter()
                .filter(|column| table.count(row, column) > 0)
                .map(|column| format!("{column} {}", table.count(row, column)))
                .collect();
            say(format!("{defects} defects against the CPU reference ({})", kinds.join(", ")));
        }
        if let verdict @ ("fail" | "baseline-fail" | "useless") = table.get(row, "verdict") {
            say(format!("the crate's verdict is {verdict}"));
        }
        let mut counted = |column: &str, what: &str| {
            let n = table.count(row, column);
            if n > 0 {
                say(format!("{n} {what}"));
            }
        };
        counted("unwritten", "records the dispatch left unwritten (judged: each a defect unless its ray is excluded)");
        counted("refused_malformed", "records with kind bit 31 that are no refusal a shader writes (judged, as the unwritten ones)");
        counted(
            "repeat_differing",
            "records of the dispatch that checks the timed ones differ from the first dispatch's, or are unwritten",
        );
        counted(
            "loop_last_differing",
            "records of the dispatch that checks the loop differ from the frame's first dispatch",
        );
        counted("null_not_miss", "records of the null shader are not a miss (an unwritten record is one)");
        counted("refused_geometry", "rays refused in the dispatch: geometry of the other type");
        counted("refused_range", "rays refused in the dispatch: the shader's test left single precision");
        counted("refused_unanswered", "refused rays that no built path answered (the software traversal on the GPU is not built)");
        counted("beyond_band", "right answers with an error over 1/8 of the tolerance (no defect; reported)");
        counted("model_defects", "defects of the CPU model of the software traversal itself");
        counted("trace_untimed", "timed dispatches without a GPU time");
        counted("build_untimed", "build or refit regions without a GPU time");
        if !table.get(row, "model_refusal").is_empty() {
            say(format!("the CPU model has no answers: {}", table.get(row, "model_refusal")));
        }
        if table.get(row, "status") == "ran" && !table.get(row, "note").is_empty() {
            say(format!("note: {}", table.get(row, "note")));
        }
    }
    list
}

/// How the figures are to be read; printed last, in every summary.
const READING: &str = "\
how to read this
  No row is a pass under the rule of docs/dxr-compatibility.md: the verdicts are victory-rt's, under that
  crate's policy, and the rule is a proposal the comparison is not yet brought into line with.
  The times are of synthetic scenes on this device and driver. They select no path and say nothing about
  the title: a probe scene is not a frame of it.
";

/// One run, as text.
pub fn render(table: &Table) -> String {
    let mut out = head(table);
    correctness(table, &mut out);
    traversal(table, &mut out);
    builds(table, &mut out);
    processor(table, &mut out);
    memory(table, &mut out);
    loops(table, &mut out);
    let list = to_look_at(table);
    let _ = writeln!(out, "\nto look at ({})", list.len());
    for line in &list {
        let _ = writeln!(out, "  {line}");
    }
    if list.is_empty() {
        let _ = writeln!(
            out,
            "  nothing: no failed row, no defect, no unwritten record, no refusal, no record that differs between \
             dispatches"
        );
    }
    let _ = write!(out, "\n{READING}");
    out
}

/// The sections of a hit-record file (`victory-rt`'s result file): what
/// names each (class, seed, frame, the case's fingerprint) and its record
/// lines.
fn hit_sections(text: &str) -> Result<Vec<(String, Vec<&str>)>> {
    if !text.lines().next().is_some_and(|first| first.starts_with("victory-rt results")) {
        bail!("not a file of hit records: its first line does not begin with \"victory-rt results\"");
    }
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut open = false;
    for (n, line) in text.lines().enumerate().skip(2) {
        if let Some(name) = line.strip_prefix("section ") {
            sections.push((name.trim().to_string(), Vec::new()));
            open = true;
        } else if line.starts_with("end ") {
            open = false;
        } else if open {
            sections.last_mut().expect("a section is open").1.push(line.trim());
        } else if n == 2 && line.starts_with("rule k ") {
            // Version 2 of the format: the third line is the constant of
            // the rule's tolerance as the writing build has it. No record.
        } else if !line.trim().is_empty() {
            bail!("line {}: a record outside a section", n + 1);
        }
    }
    if open {
        bail!("the file ends inside a section");
    }
    Ok(sections)
}

/// The class and the frame a section's name holds: its first word, and
/// the word after `frame`.
fn section_key(name: &str) -> (String, String) {
    let mut words = name.split_whitespace();
    let class = words.next().unwrap_or("").to_string();
    let frame = words.skip_while(|word| *word != "frame").nth(1).unwrap_or("").to_string();
    (class, frame)
}

/// What the comparison of two hit-record files that were both read gave.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HitsCompared {
    /// Class and frame of each section that holds the same records in both.
    pub identical: Vec<(String, String)>,
    /// The records in those sections.
    pub records: u64,
    /// Class, frame and a line for each section that does not, or that is
    /// in one file only.
    pub differing: Vec<(String, String, String)>,
}

/// The hit records of one path in two runs.
#[derive(Debug, PartialEq, Eq)]
pub enum Hits {
    /// Neither run has a file for the path: the probe writes none for a
    /// path it has no records of.
    NoFiles,
    /// Both files were read.
    Compared(HitsCompared),
    /// A file is missing or is no hit-record file: why.
    NotCompared(String),
}

/// Two hit-record files: which sections hold the same records, and which
/// do not.
pub fn compare_hits(before: &str, after: &str) -> Result<HitsCompared> {
    let (before, after) = (hit_sections(before)?, hit_sections(after)?);
    let mut compared = HitsCompared::default();
    let mut differ = |name: &str, what: String| {
        let (class, frame) = section_key(name);
        compared.differing.push((class, frame, format!("{name}: {what}")));
    };
    let mut identical = Vec::new();
    for (name, records) in &after {
        match before.iter().find(|(other, _)| other == name) {
            None => differ(name, "not in the earlier run".into()),
            Some((_, earlier)) if earlier == records => identical.push((section_key(name), records.len() as u64)),
            Some((_, earlier)) => {
                let n = earlier.iter().zip(records).filter(|(a, b)| a != b).count()
                    + earlier.len().abs_diff(records.len());
                differ(name, format!("{n} of {} records differ", records.len()));
            }
        }
    }
    for (name, _) in &before {
        if !after.iter().any(|(other, _)| other == name) {
            differ(name, "not in this run".into());
        }
    }
    compared.records = identical.iter().map(|(_, records)| records).sum();
    compared.identical = identical.into_iter().map(|(key, _)| key).collect();
    Ok(compared)
}

/// Class and frame of the rows of `path` whose records the probe kept: it
/// fills a row's refusal counts exactly when it kept the row's records,
/// and those are the sections of the path's hit-record file.
fn sections_of_rows(table: &Table, path: &str) -> Vec<(String, String)> {
    (0..table.rows.len())
        .filter(|&r| {
            table.get(r, "path") == path
                && table.get(r, "frame") != "loop"
                && !table.get(r, "refused_unanswered").is_empty()
        })
        .map(|r| (table.get(r, "class").to_string(), table.get(r, "frame").to_string()))
        .collect()
}

/// Two runs: the header where it differs, the counts row by row, the hit
/// records section by section, and the times side by side. `hits` is, per
/// tracing path, what became of the two hit-record files. The last line
/// before the reading says the runs agree only when every section the
/// tables' rows have was compared and is the same.
pub fn compare(before: &Table, after: &Table, hits: &[(String, Hits)]) -> String {
    let mut out = String::new();
    let _ = write!(out, "the earlier run\n{}\nthis run\n{}", head(before), head(after));
    let _ = writeln!(out, "\nthe two headers, where they differ");
    let mut keys: Vec<&str> = Vec::new();
    for (key, _) in before.header.iter().chain(&after.header) {
        if !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    let mut differ = 0;
    for key in keys {
        // When a run was made is not a difference between two runs.
        if key == "unix_time" || before.value(key) == after.value(key) {
            continue;
        }
        differ += 1;
        let (b, a) = (before.value(key).unwrap_or("not recorded"), after.value(key).unwrap_or("not recorded"));
        let _ = writeln!(out, "  {key}: {b}  ->  {a}");
    }
    if differ == 0 {
        let _ = writeln!(out, "  nothing differs");
    }
    let same_seed = before.value("seed") == after.value("seed");
    let same_device = ["adapter", "driver"].iter().all(|key| before.value(key) == after.value(key));
    if !same_seed {
        let _ = writeln!(
            out,
            "\nThe two runs have different seeds: other scenes and other rays. Their counts and records are not \
             expected to be equal, and are not compared."
        );
        let _ = write!(out, "\n{READING}");
        return out;
    }
    if !same_device {
        let _ = writeln!(
            out,
            "\nThe two runs are of different devices or drivers. Equal records are promised for one device and \
             driver only: what differs below is a finding about the two, not a failure of either."
        );
    }

    let _ = writeln!(out, "\ncounts, row by row (the same seed must give the same counts on one device and driver)");
    let mut unequal = 0;
    let mut rows = 0;
    for row in 0..after.rows.len() {
        let key = after.key(row);
        let Some(other) = (0..before.rows.len()).find(|&r| before.key(r) == key) else {
            let _ = writeln!(out, "  {}, frame {}, {}: not in the earlier run", key.0, frame(&key.1), key.2);
            unequal += 1;
            continue;
        };
        rows += 1;
        let mut differing: Vec<String> = ["status", "verdict"]
            .iter()
            .chain(COUNTS)
            .filter(|column| before.get(other, column) != after.get(row, column))
            .map(|column| {
                format!("{column} {} -> {}", shown(before.get(other, column)), shown(after.get(row, column)))
            })
            .collect();
        if !differing.is_empty() {
            unequal += 1;
            differing.truncate(8);
            let _ = writeln!(out, "  {}, frame {}, {}: {}", key.0, frame(&key.1), key.2, differing.join("; "));
        }
    }
    for row in 0..before.rows.len() {
        let key = before.key(row);
        if !(0..after.rows.len()).any(|r| after.key(r) == key) {
            let _ = writeln!(out, "  {}, frame {}, {}: not in this run", key.0, frame(&key.1), key.2);
            unequal += 1;
        }
    }
    let _ = writeln!(out, "  {rows} rows are in both runs; {unequal} rows differ");

    let _ = writeln!(out, "\nhit records, section by section (a class and frame of one path)");
    // Sections compared and the same, their records, sections that differ,
    // and sections the rows have that were not compared.
    let (mut same, mut records, mut records_differ, mut not_compared) = (0usize, 0u64, 0usize, 0usize);
    for (path, hits) in hits {
        // What the two tables' rows say the files hold.
        let mut expected = sections_of_rows(before, path);
        for key in sections_of_rows(after, path) {
            if !expected.contains(&key) {
                expected.push(key);
            }
        }
        match hits {
            Hits::NoFiles if expected.is_empty() => {
                let _ = writeln!(out, "  {path}: neither run has hit records of this path");
            }
            Hits::NoFiles => {
                not_compared += expected.len();
                let _ = writeln!(
                    out,
                    "  {path}: NOT COMPARED: neither run's file is there, and the rows have {} sections",
                    expected.len()
                );
            }
            Hits::NotCompared(why) => {
                // Something was to be compared, whatever the rows say.
                not_compared += expected.len().max(1);
                let _ = writeln!(out, "  {path}: NOT COMPARED: {why}");
            }
            Hits::Compared(compared) => {
                let _ = writeln!(
                    out,
                    "  {path}: {} sections identical ({} records), {} differing",
                    compared.identical.len(),
                    compared.records,
                    compared.differing.len()
                );
                same += compared.identical.len();
                records += compared.records;
                records_differ += compared.differing.len();
                for (_, _, line) in &compared.differing {
                    let _ = writeln!(out, "    {line}");
                }
                let found = |key: &(String, String)| {
                    compared.identical.contains(key) || compared.differing.iter().any(|(c, f, _)| (c, f) == (&key.0, &key.1))
                };
                for key in expected.iter().filter(|key| !found(key)) {
                    not_compared += 1;
                    let _ = writeln!(out, "    {}, frame {}: NOT COMPARED: the rows have it and neither file does", key.0, key.1);
                }
            }
        }
    }

    let _ = writeln!(
        out,
        "\nGPU time of the traversal dispatch alone, median, ms: the earlier run, this run (side by side; neither \
         is called better)"
    );
    let _ = writeln!(out, "  {:<16} {:>5} {:<7} {:>12} {:>12}", "class", "frame", "path", "earlier", "this");
    for row in ran(after, &["null", "native", "hybrid"], false) {
        let key = after.key(row);
        let earlier = (0..before.rows.len())
            .find(|&r| before.key(r) == key)
            .and_then(|r| before.number(r, "trace_gpu_ms_p50"));
        let _ = writeln!(
            out,
            "  {:<16} {:>5} {:<7} {:>12} {:>12}",
            key.0,
            frame(&key.1),
            key.2,
            ms(earlier),
            ms(after.number(row, "trace_gpu_ms_p50"))
        );
    }

    let _ = if unequal > 0 || records_differ > 0 {
        let beside = match not_compared {
            0 => String::new(),
            n => format!("; {n} more sections were not compared"),
        };
        writeln!(
            out,
            "\nTHE TWO RUNS DIFFER: {unequal} rows with other counts, {records_differ} sections of hit records \
             that differ{beside}"
        )
    } else if not_compared > 0 {
        writeln!(
            out,
            "\nNOT COMPARED: {not_compared} sections of hit records were not compared ({same} were, and are \
             identical). The counts are equal in every row; that alone is no agreement of the two runs"
        )
    } else if same == 0 {
        writeln!(
            out,
            "\nNOT COMPARED: neither run has a hit record to compare. The counts are equal in every row; that alone \
             is no agreement of the two runs"
        )
    } else {
        writeln!(
            out,
            "\nthe two runs agree: equal counts in all {rows} rows, and all {same} sections of hit records \
             ({records} records) are identical"
        )
    };
    let _ = write!(out, "\n{READING}");
    out
}

/// The hit-record file of `path` beside a table: `rt-probe.csv` has
/// `rt-probe.native.hits`.
fn hits_file(table: &Path, path: &str) -> PathBuf {
    let stem = table.file_stem().map_or_else(|| "rt-probe".into(), |s| s.to_string_lossy().into_owned());
    table.with_file_name(format!("{stem}.{path}.hits"))
}

/// The command: one table summarised, or `against` and then this one
/// compared.
pub fn run(file: &Path, against: Option<&Path>) -> Result<()> {
    let read = |path: &Path| -> Result<Table> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Table::parse(&text).with_context(|| format!("{}", path.display()))
    };
    let after = read(file)?;
    let Some(against) = against else {
        print!("{}", render(&after));
        return Ok(());
    };
    let before = read(against)?;
    let hits: Vec<(String, Hits)> =
        TRACING.iter().map(|path| (path.to_string(), read_hits(against, file, path))).collect();
    print!("{}", compare(&before, &after, &hits));
    Ok(())
}

/// The hit-record files of `path` beside the two tables, read and
/// compared. A file that is not there is no agreement: with neither there
/// the rows decide whether anything was to be compared, with one there
/// nothing was.
fn read_hits(against: &Path, file: &Path, path: &str) -> Hits {
    let (earlier, this) = (hits_file(against, path), hits_file(file, path));
    let text = |file: &Path| -> Result<Option<String>, String> {
        match std::fs::read_to_string(file) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", file.display())),
        }
    };
    match (text(&earlier), text(&this)) {
        (Err(why), _) | (_, Err(why)) => Hits::NotCompared(why),
        (Ok(None), Ok(None)) => Hits::NoFiles,
        (Ok(None), Ok(Some(_))) => Hits::NotCompared(format!("{}: there is no such file", earlier.display())),
        (Ok(Some(_)), Ok(None)) => Hits::NotCompared(format!("{}: there is no such file", this.display())),
        (Ok(Some(a)), Ok(Some(b))) => match compare_hits(&a, &b) {
            Ok(compared) => Hits::Compared(compared),
            Err(e) => Hits::NotCompared(format!("{e:#}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_rt_probe::file::Table as Written;
    use victory_rt_probe::row::{self, Row};

    /// A tracing row that ran, as the probe writes one.
    fn traced(class: &str, at: &str, path: &str, missed: u64, p50: f64) -> Row {
        let mut row = Row::new(class, 0x5EED, at, path);
        row.text("status", row::RAN)
            .text("verdict", if missed > 0 { "fail" } else { "pass" })
            .count("rays", 2000)
            .count("rays_listed", 2000)
            .count("judged", 2000)
            .count("clear", 1990)
            .count("ambiguous", 10)
            .count("excluded", 0)
            .count("missed", missed)
            .count("phantom", 0)
            .count("repeat_differing", 0)
            .count("refused_taken_by_next", 0)
            .count("refused_unanswered", 0)
            .count("timed_rays", 1 << 20)
            .count("trace_samples", 30)
            .count("primitives", 5000)
            .count("builds_done", 2)
            .count("own_rt_structures_bytes", 3_000_000)
            .number("trace_gpu_ms_min", Some(p50 - 0.25))
            .number("trace_gpu_ms_p50", Some(p50))
            .number("trace_gpu_ms_p95", Some(p50 + 0.5))
            .number("trace_gpu_ms_max", Some(p50 + 2.0))
            .number("null_gpu_ms_p50", Some(0.125))
            .number("build_gpu_ms", Some(0.75))
            .number("commit_mb_before", Some(500.0))
            .number("commit_mb_after", Some(510.0));
        row
    }

    /// A table as the probe writes it: a null row, a class of two frames
    /// on both paths, the rows of the path that is not built.
    fn written(missed_native: u64, hybrid_p50: f64) -> String {
        let mut table = Written::default();
        table.set("probe_version", "1");
        table.set("policy", "victory-rt's comparison: under the crate's policy, not yet the rule");
        table.set("seed", "5eed");
        table.set("adapter", "A card");
        table.set("driver", "1.2.3.4");
        table.set("raytracing_tier", "1.1");
        table.set("unix_time", &format!("{}", 1000 + missed_native));
        table.set("path.software-gpu", "not built: no shader walks console-format nodes here");
        let mut null = Row::new("dynamic-refit", 0x5EED, "", row::NULL);
        null.text("status", row::RAN).count("null_not_miss", 0).number("trace_gpu_ms_p50", Some(0.125));
        table.rows.push(null);
        for at in ["0", "1"] {
            table.rows.push(traced("dynamic-refit", at, row::NATIVE, if at == "1" { missed_native } else { 0 }, 1.5));
            table.rows.push(traced("dynamic-refit", at, row::HYBRID, 0, hybrid_p50));
            let mut software = Row::new("dynamic-refit", 0x5EED, at, row::SOFTWARE_GPU);
            software.text("status", row::NOT_BUILT);
            table.rows.push(software);
        }
        table.to_text()
    }

    #[test]
    fn the_summary_reads_what_the_probe_writes() {
        // Every column read here is one the probe has.
        for column in READ.iter().chain(COUNTS).chain(&DEFECTS) {
            assert!(row::COLUMNS.iter().any(|(name, _)| name == column), "the probe has no column {column}");
        }
        let table = Table::parse(&written(0, 4.0)).unwrap();
        assert_eq!(table.rows.len(), 7);
        assert_eq!(table.classes(), ["dynamic-refit"]);
        assert_eq!(table.value("adapter"), Some("A card"));
        let text = render(&table);
        let line = |start: &str| {
            text.lines().find(|l| l.trim_start().starts_with(start)).unwrap_or_else(|| panic!("{start}\n{text}"))
        };
        assert!(text.starts_with("victory rt-probe, probe version 1, seed 5eed\n"), "{text}");
        assert!(line("device:").contains("A card; driver 1.2.3.4; ray tracing tier 1.1"));
        // Two frames summed: 4,000 rays judged, no defect, both passes.
        let native = line("dynamic-refit    native");
        assert!(native.contains("2/2") && native.contains("4000") && native.ends_with("pass 2"), "{native}");
        assert!(line("software-gpu:").contains("not built") && line("software-gpu:").contains("2 rows"));
        // A time is shown as measured; what was not measured is a dash.
        let timed: Vec<&str> = text.lines().filter(|l| l.contains("1048576")).collect();
        assert_eq!(timed.len(), 4, "{text}");
        assert!(timed[0].contains("1.250") && timed[0].contains("1.500") && timed[0].contains("3.500"), "{}", timed[0]);
        assert!(timed[0].contains("0.125"), "the null shader's median beside it: {}", timed[0]);
        assert!(text.contains("to look at (0)\n  nothing: no failed row"), "{text}");
        assert!(text.contains("No row is a pass under the rule"));
        assert!(text.contains("They select no path and say nothing about"));
        // No word of comparison between paths or runs.
        for word in ["faster", "slower", "speed", "better", "worse"] {
            assert!(!text.to_lowercase().contains(word), "{word}");
        }
    }

    #[test]
    fn what_needs_a_look_is_listed_row_by_row() {
        let mut text = written(3, 4.0);
        // A failed row and a row whose repeats differ, as the probe would
        // write them.
        let mut failed = Row::new("precision", 0x5EED, "0", row::HYBRID);
        failed.text("status", row::FAILED).note("frame 0: building the structures: refused");
        let mut flickers = traced("precision", "0", row::NATIVE, 0, 2.0);
        flickers.count("repeat_differing", 5).count("refused_geometry", 7).count("refused_unanswered", 7);
        flickers.count("trace_untimed", 1);
        for row in [failed, flickers] {
            text.push_str(&row.values().join(","));
            text.push('\n');
        }
        let table = Table::parse(&text).unwrap();
        let list = to_look_at(&table);
        let has = |part: &str| list.iter().any(|l| l.contains(part));
        assert!(has("dynamic-refit, frame 1, native: 3 defects against the CPU reference (missed 3)"), "{list:?}");
        assert!(has("dynamic-refit, frame 1, native: the crate's verdict is fail"));
        assert!(has("precision, frame 0, hybrid: failed: frame 0: building the structures: refused"));
        assert!(has("precision, frame 0, native: 5 records of the dispatch that checks the timed ones differ"));
        assert!(has("precision, frame 0, native: 7 rays refused in the dispatch: geometry of the other type"));
        assert!(has("precision, frame 0, native: 7 refused rays that no built path answered"));
        assert!(has("precision, frame 0, native: 1 timed dispatches without a GPU time"));
        assert_eq!(list.len(), 7, "{list:?}");
        let text = render(&table);
        assert!(text.contains("to look at (7)"));
        let native = text.lines().find(|l| l.contains("dynamic-refit    native")).unwrap();
        assert!(native.ends_with("pass 1, fail 1"), "{native}");
    }

    #[test]
    fn what_is_not_a_table_is_refused_with_a_reason() {
        assert!(Table::parse("").unwrap_err().to_string().contains("not a result table"));
        assert!(Table::parse("seconds,fps\n1,2\n").is_err());
        let text = written(0, 4.0);
        let header_only: String = text.lines().take(4).map(|l| format!("{l}\n")).collect();
        assert!(Table::parse(&header_only).unwrap_err().to_string().contains("column names"));
        assert!(Table::parse(&format!("{text}a,b\n")).unwrap_err().to_string().contains("2 fields"));
        assert!(Table::parse(&format!("{text}# late=1\n")).unwrap_err().to_string().contains("among the rows"));
        // A table of another probe version with other columns reads as far
        // as it has them.
        let other = "# victory rt-probe results 9\n# seed=1\nclass,frame,path,status,new_column\nsoup,0,native,ran,7\n";
        let table = Table::parse(other).unwrap();
        assert_eq!((table.get(0, "new_column"), table.get(0, "verdict"), table.count(0, "missed")), ("7", "", 0));
        assert!(render(&table).contains("soup"));
        assert!(Table::parse("# victory rt-probe results 9\nclass,frame,path\n").unwrap_err().to_string().contains("status"));
    }

    fn hits(seed: &str, records: &[&str]) -> String {
        let mut text = "victory-rt results 1\nproducer a test\n".to_string();
        for at in 0..2 {
            let _ = writeln!(
                text,
                "section dynamic-refit seed {seed} frame {at} rays {} case 00000000000000aa",
                records.len()
            );
            for record in records {
                let _ = writeln!(text, "{record}");
            }
            let _ = writeln!(text, "end 0123456789abcdef");
        }
        text
    }

    const RECORD: &str = "3f800000 00000000 00000000 00000001 00000000 00000007 00000000 00000002";
    const MISS: &str = "00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000";

    fn key(frame: &str) -> (String, String) {
        ("dynamic-refit".to_string(), frame.to_string())
    }

    /// Both paths' files compared and the same: what `written`'s rows have.
    fn all_identical() -> Vec<(String, Hits)> {
        let same = || compare_hits(&hits("5eed", &[RECORD, MISS]), &hits("5eed", &[RECORD, MISS])).unwrap();
        vec![("native".to_string(), Hits::Compared(same())), ("hybrid".to_string(), Hits::Compared(same()))]
    }

    #[test]
    fn two_runs_of_one_seed_are_compared_by_counts_and_by_records() {
        let (before, after) = (Table::parse(&written(0, 4.0)).unwrap(), Table::parse(&written(0, 9.0)).unwrap());
        let same = compare_hits(&hits("5eed", &[RECORD, MISS]), &hits("5eed", &[RECORD, MISS])).unwrap();
        assert_eq!(same, HitsCompared { identical: vec![key("0"), key("1")], records: 4, differing: vec![] });
        let text = compare(&before, &after, &all_identical());
        assert!(text.contains("the two headers, where they differ\n  nothing differs"), "{text}");
        assert!(text.contains("7 rows are in both runs; 0 rows differ"), "{text}");
        assert!(text.contains("native: 2 sections identical (4 records), 0 differing"));
        assert!(text.contains("hybrid: 2 sections identical (4 records), 0 differing"));
        assert!(
            text.contains(
                "\nthe two runs agree: equal counts in all 7 rows, and all 4 sections of hit records (8 records) are \
                 identical\n"
            ),
            "{text}"
        );
        assert!(!text.contains("NOT COMPARED") && !text.contains("DIFFER"));
        // The times stand side by side, and nothing is said of them.
        let hybrid = text.lines().find(|l| l.contains("hybrid") && l.contains("9.000")).unwrap();
        assert!(hybrid.contains("4.000"), "{hybrid}");
        for word in ["faster", "slower", "better", "worse", "WORSE"] {
            assert!(!text.replace("neither is called better", "").contains(word), "{word}\n{text}");
        }

        // The later run lost three hits in one frame, and a record differs.
        let after = Table::parse(&written(3, 4.0)).unwrap();
        let differing = compare_hits(&hits("5eed", &[RECORD, MISS]), &hits("5eed", &[MISS, MISS])).unwrap();
        assert_eq!((differing.identical.len(), differing.records), (0, 0));
        assert_eq!(
            differing.differing[0],
            (
                "dynamic-refit".to_string(),
                "0".to_string(),
                "dynamic-refit seed 5eed frame 0 rays 2 case 00000000000000aa: 1 of 2 records differ".to_string()
            )
        );
        let mut files = all_identical();
        files[0].1 = Hits::Compared(differing);
        let text = compare(&before, &after, &files);
        assert!(text.contains("dynamic-refit, frame 1, native: verdict pass -> fail; missed 0 -> 3"), "{text}");
        assert!(text.contains("7 rows are in both runs; 1 rows differ"));
        assert!(text.contains("THE TWO RUNS DIFFER: 1 rows with other counts, 2 sections of hit records that differ\n"));
        assert!(!text.contains("the two runs agree"));
        // When each run was made is not a difference.
        assert!(!text.contains("unix_time"));
    }

    /// Agreement is said of records that were compared, and of nothing
    /// else: a file that is missing, a section in neither file, and runs
    /// without a hit record each end in NOT COMPARED.
    #[test]
    fn two_runs_whose_hit_records_were_not_all_compared_are_not_said_to_agree() {
        let (before, after) = (Table::parse(&written(0, 4.0)).unwrap(), Table::parse(&written(0, 4.0)).unwrap());
        let ends = |files: &[(String, Hits)]| -> String {
            let text = compare(&before, &after, files);
            assert!(text.contains("7 rows are in both runs; 0 rows differ"), "{text}");
            assert!(!text.contains("the two runs agree"), "{text}");
            assert!(!text.contains("DIFFER"), "{text}");
            text
        };
        // One path's file could not be read.
        let mut files = all_identical();
        files[1].1 = Hits::NotCompared("b.hybrid.hits: there is no such file".into());
        let text = ends(&files);
        assert!(text.contains("hybrid: NOT COMPARED: b.hybrid.hits: there is no such file"), "{text}");
        assert!(
            text.contains("\nNOT COMPARED: 2 sections of hit records were not compared (2 were, and are identical)."),
            "{text}"
        );
        // No file of either path in either run, where the rows have records.
        let none = vec![("native".to_string(), Hits::NoFiles), ("hybrid".to_string(), Hits::NoFiles)];
        let text = ends(&none);
        assert!(text.contains("native: NOT COMPARED: neither run's file is there, and the rows have 2 sections"), "{text}");
        assert!(text.contains("\nNOT COMPARED: 4 sections of hit records were not compared (0 were"), "{text}");
        // Files that were read and hold no section, or not every section.
        let header = "victory-rt results 1\nproducer a test\n";
        let empty = compare_hits(header, header).unwrap();
        assert_eq!(empty, HitsCompared::default());
        let one: String = hits("5eed", &[RECORD]).lines().take(5).map(|l| format!("{l}\n")).collect();
        let first_only = compare_hits(&one, &one).unwrap();
        assert_eq!(first_only.identical, [key("0")]);
        let cut = vec![("native".to_string(), Hits::Compared(empty)), ("hybrid".to_string(), Hits::Compared(first_only))];
        let text = ends(&cut);
        assert!(text.contains("native: 0 sections identical (0 records), 0 differing"), "{text}");
        assert!(text.contains("    dynamic-refit, frame 1: NOT COMPARED: the rows have it and neither file does"), "{text}");
        assert!(text.contains("\nNOT COMPARED: 3 sections of hit records were not compared (1 were"), "{text}");

        // Two runs in which no row has records (neither made a pipeline):
        // equal counts, and nothing that says the runs agree.
        let failed = |text: String| -> Table {
            let mut failed = Written::parse(&text).unwrap();
            for row in &mut failed.rows {
                if matches!(row.get("path"), "native" | "hybrid") {
                    let mut bare = Row::new(row.get("class"), 0x5EED, row.get("frame"), row.get("path"));
                    bare.text("status", row::FAILED).note("the pipeline was not made");
                    *row = bare;
                }
            }
            Table::parse(&failed.to_text()).unwrap()
        };
        let (before, after) = (failed(written(0, 4.0)), failed(written(0, 4.0)));
        let text = compare(&before, &after, &none);
        assert!(text.contains("native: neither run has hit records of this path"), "{text}");
        assert!(text.contains("\nNOT COMPARED: neither run has a hit record to compare."), "{text}");
        assert!(!text.contains("the two runs agree"), "{text}");
        // A difference is said first, and what was not compared beside it.
        let mut files = all_identical();
        files[1].1 = Hits::NoFiles;
        let after = Table::parse(&written(3, 4.0)).unwrap();
        let text = compare(&Table::parse(&written(0, 4.0)).unwrap(), &after, &files);
        assert!(
            text.contains(
                "THE TWO RUNS DIFFER: 1 rows with other counts, 0 sections of hit records that differ; 2 more \
                 sections were not compared"
            ),
            "{text}"
        );
    }

    /// The files beside two tables, as the command reads them.
    #[test]
    fn hit_files_that_are_missing_or_no_hit_files_are_not_compared() {
        let dir = std::env::temp_dir().join(format!("victory-rt-summary-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.csv"), dir.join("b.csv"));
        assert_eq!(read_hits(&a, &b, "native"), Hits::NoFiles);
        std::fs::write(dir.join("a.native.hits"), hits("5eed", &[RECORD, MISS])).unwrap();
        match read_hits(&a, &b, "native") {
            Hits::NotCompared(why) => assert!(why.ends_with("b.native.hits: there is no such file"), "{why}"),
            other => panic!("{other:?}"),
        }
        match read_hits(&b, &a, "native") {
            Hits::NotCompared(why) => assert!(why.ends_with("b.native.hits: there is no such file"), "{why}"),
            other => panic!("{other:?}"),
        }
        std::fs::write(dir.join("b.native.hits"), "something else\n").unwrap();
        match read_hits(&a, &b, "native") {
            Hits::NotCompared(why) => assert!(why.contains("not a file of hit records"), "{why}"),
            other => panic!("{other:?}"),
        }
        std::fs::write(dir.join("b.native.hits"), hits("5eed", &[RECORD, MISS])).unwrap();
        match read_hits(&a, &b, "native") {
            Hits::Compared(compared) => assert_eq!((compared.identical.len(), compared.records), (2, 4)),
            other => panic!("{other:?}"),
        }
        assert_eq!(read_hits(&a, &b, "hybrid"), Hits::NoFiles);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn runs_of_other_seeds_or_devices_are_said_to_be_that() {
        let before = Table::parse(&written(0, 4.0)).unwrap();
        let other_seed = Table::parse(&written(0, 4.0).replace("# seed=5eed", "# seed=2")).unwrap();
        let text = compare(&before, &other_seed, &[]);
        assert!(text.contains("seed: 5eed  ->  2"), "{text}");
        assert!(text.contains("different seeds") && !text.contains("counts, row by row"), "{text}");
        let other_driver = Table::parse(&written(0, 4.0).replace("# driver=1.2.3.4", "# driver=1.2.3.5")).unwrap();
        let text = compare(&before, &other_driver, &[]);
        assert!(text.contains("driver: 1.2.3.4  ->  1.2.3.5"));
        assert!(text.contains("different devices or drivers") && text.contains("counts, row by row"), "{text}");
    }

    #[test]
    fn hit_files_with_other_sections_or_cut_short_say_so() {
        let two = hits("5eed", &[RECORD]);
        let one: String = two.lines().take(5).map(|l| format!("{l}\n")).collect();
        let compared = compare_hits(&two, &one).unwrap();
        assert_eq!((&compared.identical, compared.records), (&vec![key("0")], 1));
        assert_eq!(
            compared.differing,
            [(
                "dynamic-refit".to_string(),
                "1".to_string(),
                "dynamic-refit seed 5eed frame 1 rays 1 case 00000000000000aa: not in this run".to_string()
            )]
        );
        let compared = compare_hits(&one, &two).unwrap();
        assert!(compared.differing[0].2.ends_with("not in the earlier run"));
        let cut: String = two.lines().take(4).map(|l| format!("{l}\n")).collect();
        assert!(compare_hits(&cut, &two).is_err());
        assert!(compare_hits("", &two).is_err() && compare_hits(&two, "a table, say\n").is_err());
        assert_eq!(section_key("static-soup seed 1 frame 12 rays 5 case 00"), ("static-soup".into(), "12".into()));
        assert_eq!(hits_file(Path::new("a/rt-probe-b.csv"), "native"), Path::new("a/rt-probe-b.native.hits"));
    }

    /// The hit files the probe writes are in the form read here.
    #[test]
    fn the_probes_own_hit_file_is_read() {
        use victory_rt_probe::adapter;
        let case = adapter::case("static-soup", 1).unwrap();
        let misses = vec![[0u32; 8]; case.rays.len()];
        let text = adapter::hits_text("a test", &[(&case, 0, &misses)]).unwrap();
        let sections = hit_sections(&text).unwrap();
        assert_eq!(sections.len(), 1);
        assert!(sections[0].0.starts_with("static-soup seed 1 frame 0 rays "), "{}", sections[0].0);
        assert_eq!(sections[0].1.len(), case.rays.len());
        let compared = compare_hits(&text, &text).unwrap();
        assert_eq!(compared.identical, [("static-soup".to_string(), "0".to_string())]);
        assert_eq!((compared.records, compared.differing.len()), (case.rays.len() as u64, 0));
        // The file is version 2 of the format, with the rule's constant on
        // its third line; that line is no record and belongs to no section.
        let head = "victory-rt results 2\nproducer a test\nrule k 8\n";
        assert!(text.starts_with(head), "{}", &text[..text.len().min(120)]);
        // A file of version 1, as the probe wrote before that line was
        // there, holds the same sections and compares as identical.
        let old = text.replacen(head, "victory-rt results 1\nproducer a test\n", 1);
        assert_eq!(hit_sections(&old).unwrap(), sections);
        let compared = compare_hits(&old, &text).unwrap();
        assert_eq!((compared.records, compared.differing.len()), (case.rays.len() as u64, 0));
        // And what the probe wrote, the probe's own reader reads back.
        let back = adapter::read_hits(&text).unwrap();
        assert_eq!((back.len(), back[0].records.as_slice()), (1, misses.as_slice()));
        // The line anywhere else is still a record outside a section.
        let late = format!("{text}rule k 8\n");
        assert!(compare_hits(&late, &text).unwrap_err().to_string().contains("a record outside a section"));
    }

    /// What the comparison of two runs goes by to know which sections the
    /// hit-record files hold: the rows of a real run of the probe's own
    /// writers, read as this summary reads them.
    #[test]
    fn the_rows_that_have_records_are_known_from_the_table() {
        let table = Table::parse(&written(0, 4.0)).unwrap();
        assert_eq!(sections_of_rows(&table, "native"), [key("0"), key("1")]);
        assert_eq!(sections_of_rows(&table, "hybrid"), [key("0"), key("1")]);
        assert!(sections_of_rows(&table, "null").is_empty() && sections_of_rows(&table, "software-gpu").is_empty());
        assert!(row::COLUMNS.iter().any(|(name, _)| *name == "refused_unanswered"));
    }
}
