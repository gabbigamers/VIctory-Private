//! `cargo xtask perf-summary`: a run's `VICTORY_PERF_LOG` as the figures the
//! ledgers in docs/progress.md hold, and two runs side by side.
//!
//! The log has a row per five-second period. A run is summarised over its
//! world periods (those drawing at least [`WORLD_DRAWS`] draws a frame; all
//! of them if none does), so loading and menus do not dilute it. Peaks are
//! taken over the whole run. Columns are found by name, so a log from an
//! older or newer build reads as far as it has the columns.
//!
//! Memory comes as four different things, kept apart here as in the log:
//! commit (promised, touched or not), the working set (in RAM), video
//! memory, and the paging file. The run fits a machine when its working set
//! does and the machine did not have to page; a commit figure alone says
//! neither.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};

/// A period that draws this much a frame is in the world.
const WORLD_DRAWS: f64 = 500.0;

/// A parsed log: column names and rows of values (`None` for an empty field).
#[derive(Debug)]
pub struct Log {
    columns: HashMap<String, usize>,
    rows: Vec<Vec<Option<f64>>>,
}

impl Log {
    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header = lines.next().context("the log is empty")?;
        let columns: HashMap<String, usize> =
            header.split(',').enumerate().map(|(i, name)| (name.trim().to_string(), i)).collect();
        if !columns.contains_key("seconds") {
            bail!("not a VICTORY_PERF_LOG file: no `seconds` column in its first line");
        }
        let mut rows = Vec::new();
        for (n, line) in lines.enumerate() {
            let row: Vec<Option<f64>> = line
                .split(',')
                .map(|field| match field.trim() {
                    "" => Ok(None),
                    value => value.parse().map(Some),
                })
                .collect::<std::result::Result<_, _>>()
                .with_context(|| format!("row {}: not a number", n + 1))?;
            // A run that was killed can leave its last row short.
            if row.len() == columns.len() {
                rows.push(row);
            }
        }
        if rows.is_empty() {
            bail!("the log has a header and no complete row: the run ended before its first period");
        }
        Ok(Self { columns, rows })
    }

    /// Column `name` of the given rows, where it has values.
    fn values(&self, name: &str, rows: &[usize]) -> Vec<f64> {
        let Some(&at) = self.columns.get(name) else { return Vec::new() };
        rows.iter().filter_map(|&r| self.rows[r][at]).collect()
    }

    fn all(&self) -> Vec<usize> {
        (0..self.rows.len()).collect()
    }

    /// The world's periods; every period if the run never got there.
    fn world(&self) -> (Vec<usize>, bool) {
        let Some(&at) = self.columns.get("draws_per_frame") else { return (self.all(), false) };
        let world: Vec<usize> =
            (0..self.rows.len()).filter(|&r| self.rows[r][at].is_some_and(|d| d >= WORLD_DRAWS)).collect();
        if world.is_empty() { (self.all(), false) } else { (world, true) }
    }
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 { values[mid] } else { (values[mid - 1] + values[mid]) / 2.0 })
}

fn max(values: Vec<f64>) -> Option<f64> {
    values.into_iter().reduce(f64::max)
}

fn min(values: Vec<f64>) -> Option<f64> {
    values.into_iter().reduce(f64::min)
}

/// Which way a figure is better.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Better {
    Lower,
    Higher,
    /// A fact of the machine or the run, not a result.
    Neither,
}

/// One figure of a run.
#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub group: &'static str,
    pub name: &'static str,
    pub unit: &'static str,
    pub value: Option<f64>,
    pub better: Better,
}

/// A run's figures, in the order they are printed.
pub struct Summary {
    pub periods: usize,
    pub world_periods: usize,
    pub reached_world: bool,
    pub seconds: Option<f64>,
    pub figures: Vec<Figure>,
}

impl Summary {
    pub fn of(log: &Log) -> Self {
        let all = log.all();
        let (world, reached_world) = log.world();
        let mut figures = Vec::new();
        let mut put = |group, name, unit, value, better| figures.push(Figure { group, name, unit, value, better });
        use Better::{Higher, Lower, Neither};

        let w = |name: &str| log.values(name, &world);
        let a = |name: &str| log.values(name, &all);
        // Frames: what the world's periods were like.
        put("frames", "frame rate, median period", "fps", median(w("fps")), Higher);
        put("frames", "frame time, median", "ms", median(w("p50_ms")), Lower);
        put("frames", "frame time, 95th percentile", "ms", median(w("p95_ms")), Lower);
        put("frames", "frame time, 99th percentile", "ms", median(w("p99_ms")), Lower);
        put("frames", "longest frame", "ms", max(w("max_ms")), Lower);
        let late = {
            let (frames, share) = (w("frames"), w("late_share"));
            let total: f64 = frames.iter().sum();
            (frames.len() == share.len() && total > 0.0)
                .then(|| frames.iter().zip(&share).map(|(f, s)| f * s).sum::<f64>() / total)
        };
        put("frames", "late frames (over 50 ms)", "share", late, Lower);
        put("frames", "jitter, median period", "ms", median(w("jitter_ms")), Lower);
        put("frames", "preparing submissions, median", "ms", median(w("preparing_ms")), Lower);
        put("frames", "waiting for the GPU, median", "ms", median(w("gpu_ms")), Lower);
        put("frames", "dropped draws a frame, worst period", "", max(w("dropped_per_frame")), Lower);

        // In RAM: what 16 GB limits.
        let peak_ws = max([a("peak_working_set_mb"), a("working_set_mb")].concat());
        let (sustained_ws, sustained_commit) = (median(w("working_set_mb")), median(w("commit_mb")));
        put("in RAM", "working set, peak", "MB", peak_ws, Lower);
        put("in RAM", "working set, sustained (median, world)", "MB", sustained_ws, Lower);
        put("in RAM", "machine's RAM", "MB", max(a("physical_mb")), Neither);
        put("in RAM", "machine's free RAM, lowest", "MB", min(a("physical_free_mb")), Higher);
        put("in RAM", "page faults a second, median (world)", "", median(w("page_faults_per_s")), Lower);

        // Commit: promised, in RAM or not.
        let peak_commit = max([a("peak_commit_mb"), a("commit_mb")].concat());
        put("commit", "process commit, peak", "MB", peak_commit, Lower);
        put("commit", "process commit, sustained (median, world)", "MB", sustained_commit, Lower);
        put(
            "commit",
            "committed and not in RAM, sustained (paged out or never touched)",
            "MB",
            sustained_commit.zip(sustained_ws).map(|(c, ws)| (c - ws).max(0.0)),
            Lower,
        );
        put("commit", "machine commit, peak", "MB", max(a("system_commit_mb")), Lower);
        put("commit", "machine commit limit", "MB", min(a("commit_limit_mb")), Neither);

        // Paging files: the machine as a whole.
        let paging = a("pagefile_mb");
        put("paging file", "in use, peak (machine)", "MB", max(paging.clone()), Lower);
        put(
            "paging file",
            "growth over the run (machine)",
            "MB",
            paging.first().zip(max(paging.clone())).map(|(first, peak)| peak - first),
            Lower,
        );
        put("paging file", "size", "MB", max(a("pagefile_size_mb")), Neither);

        // Video memory.
        put("video", "on the card, peak", "MB", max(a("video_mb")), Lower);
        put("video", "budget on the card, lowest", "MB", min(a("video_budget_mb")), Neither);
        put("video", "in system RAM (shared), peak", "MB", max(a("video_shared_mb")), Lower);

        // By our own count.
        put("our count", "host textures, peak", "MB", max(a("host_textures_mb")), Lower);
        put("our count", "heaps of textures (used and free), peak", "MB", max(a("texture_heaps_mb")), Lower);
        put("our count", "list upload memory, peak", "MB", max(a("uploads_mb")), Lower);

        Self {
            periods: all.len(),
            world_periods: if reached_world { world.len() } else { 0 },
            reached_world,
            seconds: max(a("seconds")),
            figures,
        }
    }

    fn head(&self) -> String {
        let seconds = self.seconds.map_or("unknown".to_string(), |s| format!("{s:.0} s"));
        if self.reached_world {
            format!("{} periods over {seconds}, {} of them in the world", self.periods, self.world_periods)
        } else {
            format!("{} periods over {seconds}; the run never reached the world, so all periods are summarised", self.periods)
        }
    }
}

fn show(value: Option<f64>, unit: &str) -> String {
    match (value, unit) {
        (None, _) => "not recorded".to_string(),
        (Some(v), "share") => format!("{:.2} %", v * 100.0),
        (Some(v), "MB") => format!("{v:.0} MB"),
        (Some(v), "") => format!("{v:.1}"),
        (Some(v), unit) => format!("{v:.2} {unit}"),
    }
}

/// One run, as text.
pub fn render(summary: &Summary) -> String {
    let mut out = format!("{}\n", summary.head());
    let mut group = "";
    for f in &summary.figures {
        if f.group != group {
            group = f.group;
            let _ = writeln!(out, "\n{group}");
        }
        let _ = writeln!(out, "  {:<66} {}", f.name, show(f.value, f.unit));
    }
    out
}

/// Two runs: each figure before and after, the change, and whether that is
/// better or worse. A figure only one run has is shown without a verdict.
pub fn compare(before: &Summary, after: &Summary) -> String {
    let mut out = format!("before: {}\nafter:  {}\n", before.head(), after.head());
    let mut group = "";
    let mut worse = Vec::new();
    for (b, a) in before.figures.iter().zip(&after.figures) {
        debug_assert_eq!(b.name, a.name);
        if a.group != group {
            group = a.group;
            let _ = writeln!(out, "\n{group}");
        }
        let verdict = match (b.value, a.value, a.better) {
            (Some(x), Some(y), better) if better != Better::Neither => {
                let change = y - x;
                // Within a percent of the larger figure is no change.
                let noise = 0.01 * x.abs().max(y.abs());
                let word = if change.abs() <= noise {
                    "same"
                } else if (change < 0.0) == (better == Better::Lower) {
                    "better"
                } else {
                    worse.push(a.name);
                    "WORSE"
                };
                let shown = match a.unit {
                    "share" => format!("{:+.2} points", change * 100.0),
                    "MB" => format!("{change:+.0} MB"),
                    "" => format!("{change:+.1}"),
                    unit => format!("{change:+.2} {unit}"),
                };
                format!("{shown}  {word}")
            }
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "  {:<66} {:>14} -> {:>14}  {verdict}",
            a.name,
            show(b.value, b.unit),
            show(a.value, a.unit)
        );
    }
    let _ = match worse.as_slice() {
        [] => writeln!(out, "\nnothing is worse"),
        names => writeln!(out, "\nworse in {}: {}", names.len(), names.join("; ")),
    };
    out
}

/// The command: one log summarised, or `against` and then this one compared.
pub fn run(log: &std::path::Path, against: Option<&std::path::Path>) -> Result<()> {
    let read = |path: &std::path::Path| -> Result<Summary> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Summary::of(&Log::parse(&text).with_context(|| format!("{}", path.display()))?))
    };
    let after = read(log)?;
    match against {
        Some(before) => print!("{}", compare(&read(before)?, &after)),
        None => print!("{}", render(&after)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "seconds,frame,frames,fps,p50_ms,p95_ms,p99_ms,max_ms,late_share,jitter_ms,draws_per_frame,\
                          dropped_per_frame,preparing_ms,gpu_ms,commit_mb,working_set_mb,peak_commit_mb,\
                          peak_working_set_mb,page_faults_per_s,physical_mb,physical_free_mb,pagefile_mb,\
                          pagefile_size_mb,video_mb,video_budget_mb";

    /// A run: two loading periods, then world periods of (fps, working set,
    /// commit, paging file in use).
    fn run_of(world: &[(f64, f64, f64, f64)]) -> String {
        let mut text = format!("{HEADER}\n");
        for (i, ws) in [(0, 2000.0), (1, 6000.0)] {
            let _ = writeln!(
                text,
                "{},{},150,30,33,34,35,40,0,1,10,0,1,1,8000,{ws},8000,{ws},100,16000,9000,1000,24000,500,15000",
                5 * (i + 1),
                150 * (i + 1)
            );
        }
        for (i, &(fps, ws, commit, paging)) in world.iter().enumerate() {
            let ms = 1000.0 / fps;
            let _ = writeln!(
                text,
                "{},{},{},{fps},{ms},{},{},{},0.1,2,5000,0.5,{},{},{commit},{ws},{commit},{ws},50,16000,{},{paging},24000,9000,15000",
                15 + 5 * i,
                450 + 75 * i,
                fps * 5.0,
                ms * 1.5,
                ms * 2.0,
                ms * 3.0,
                ms * 0.4,
                ms * 0.5,
                16000.0 - ws - 2000.0,
            );
        }
        text
    }

    fn figure(summary: &Summary, name: &str) -> Option<f64> {
        summary.figures.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name}")).value
    }

    #[test]
    fn a_run_is_summarised_over_its_world_periods_and_peaks_over_all_of_it() {
        let text = run_of(&[(15.0, 11000.0, 27000.0, 9000.0), (10.0, 12000.0, 26000.0, 9500.0), (20.0, 11500.0, 27500.0, 9200.0)]);
        let s = Summary::of(&Log::parse(&text).unwrap());
        assert_eq!((s.periods, s.world_periods, s.reached_world), (5, 3, true));
        assert_eq!(figure(&s, "frame rate, median period"), Some(15.0), "loading at 30 fps is left out");
        assert_eq!(figure(&s, "working set, sustained (median, world)"), Some(11500.0));
        assert_eq!(figure(&s, "working set, peak"), Some(12000.0));
        assert_eq!(figure(&s, "process commit, peak"), Some(27500.0));
        assert_eq!(figure(&s, "process commit, sustained (median, world)"), Some(27000.0));
        assert_eq!(
            figure(&s, "committed and not in RAM, sustained (paged out or never touched)"),
            Some(15500.0),
            "27 GB of commit with 11.5 GB of it in RAM"
        );
        assert_eq!(figure(&s, "machine's free RAM, lowest"), Some(2000.0));
        assert_eq!(figure(&s, "in use, peak (machine)"), Some(9500.0));
        assert_eq!(figure(&s, "growth over the run (machine)"), Some(8500.0), "from the first period's 1000");
        assert_eq!(figure(&s, "longest frame"), Some(300.0), "three times 100 ms at 10 fps");
        assert_eq!(figure(&s, "dropped draws a frame, worst period"), Some(0.5));
        // Late share is weighted by frames: every world period has 10 %.
        assert!((figure(&s, "late frames (over 50 ms)").unwrap() - 0.1).abs() < 1e-12);
    }

    #[test]
    fn a_run_that_never_reached_the_world_is_summarised_whole_and_says_so() {
        let s = Summary::of(&Log::parse(&run_of(&[])).unwrap());
        assert_eq!((s.periods, s.world_periods, s.reached_world), (2, 0, false));
        assert_eq!(figure(&s, "working set, sustained (median, world)"), Some(4000.0));
        assert!(render(&s).contains("never reached the world"));
    }

    #[test]
    fn columns_a_log_lacks_are_not_recorded_and_older_logs_still_read() {
        let old = "seconds,frame,frames,fps,draws_per_frame,commit_mb,working_set_mb\n5,150,150,30,6000,20000,9000\n";
        let s = Summary::of(&Log::parse(old).unwrap());
        assert_eq!(figure(&s, "process commit, peak"), Some(20000.0), "from the commit column alone");
        assert_eq!(figure(&s, "in use, peak (machine)"), None);
        assert!(render(&s).contains("not recorded"));
        // An empty field is unknown, not zero.
        let gaps = "seconds,draws_per_frame,commit_mb,video_mb\n5,6000,,9000\n10,6000,21000,\n";
        let s = Summary::of(&Log::parse(gaps).unwrap());
        assert_eq!(figure(&s, "process commit, peak"), Some(21000.0));
        assert_eq!(figure(&s, "on the card, peak"), Some(9000.0));
    }

    #[test]
    fn what_is_not_a_log_is_refused_with_a_reason() {
        assert!(Log::parse("").is_err());
        assert!(Log::parse("a,b\n1,2\n").unwrap_err().to_string().contains("seconds"));
        assert!(Log::parse("seconds,fps\n").unwrap_err().to_string().contains("no complete row"));
        assert!(Log::parse("seconds,fps\n5,fast\n").is_err());
        // A last row cut short by a crash is dropped, the rest kept.
        let cut = Log::parse("seconds,fps\n5,30\n10\n").unwrap();
        assert_eq!(cut.rows.len(), 1);
    }

    #[test]
    fn a_comparison_names_what_got_better_and_what_got_worse() {
        let before = Summary::of(&Log::parse(&run_of(&[(15.0, 12000.0, 27000.0, 9000.0); 3])).unwrap());
        // Less in RAM and less commit, the same rate, but more paging.
        let after = Summary::of(&Log::parse(&run_of(&[(15.0, 9000.0, 20000.0, 11000.0); 3])).unwrap());
        let text = compare(&before, &after);
        let line = |name: &str| text.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name}\n{text}"));
        assert!(line("working set, sustained").ends_with("-3000 MB  better"), "{text}");
        assert!(line("process commit, peak").ends_with("-7000 MB  better"));
        assert!(line("frame rate, median period").ends_with("+0.00 fps  same"));
        assert!(line("in use, peak (machine)").ends_with("+2000 MB  WORSE"));
        assert!(line("machine's free RAM, lowest").ends_with("+3000 MB  better"), "more free RAM is better");
        // A fact of the machine gets no verdict.
        assert!(line("machine's RAM").trim_end().ends_with("16000 MB"));
        assert!(text.contains("worse in 2: in use, peak (machine); growth over the run (machine)"), "{text}");
        assert!(compare(&before, &before).contains("nothing is worse"));
    }

    #[test]
    fn medians_and_extremes() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!((max(vec![1.0, 3.0, 2.0]), min(vec![1.0, 3.0, 2.0])), (Some(3.0), Some(1.0)));
    }
}
