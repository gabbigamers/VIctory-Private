//! Frame pacing: the time between presented frames as a distribution, not
//! an average. A second with twenty-nine frames of 20 ms and one of 420 ms
//! averages 30 fps and is not smooth; the percentiles, the late frames and
//! the jitter say so.
//!
//! [`Pacing`] collects the intervals of a report period and [`Summary`]
//! describes them. `VICTORY_PERF_LOG=<file.csv>` writes one [`Row`] per
//! period, with the process's and the card's memory beside the frame times,
//! for comparing runs (docs/progress.md, "Measuring").

/// The frame time of the target rate, 30 frames a second, in microseconds.
pub(crate) const TARGET_US: u64 = 33_333;

/// A frame is late when it took longer than this: a frame and a half, the
/// next refresh missed on a 60 Hz display.
pub(crate) const LATE_US: u64 = TARGET_US * 3 / 2;

/// The intervals between the frames presented in a report period.
#[derive(Default)]
pub(crate) struct Pacing {
    micros: Vec<u64>,
}

/// What a period's frames took. Times in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Summary {
    pub frames: usize,
    pub mean: f64,
    /// The median and the 95th and 99th percentiles (nearest rank).
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    /// The share of frames longer than [`LATE_US`], 0 to 1.
    pub late: f64,
    /// The mean difference between one frame's time and the next one's: 0
    /// for an even pace at any rate.
    pub jitter: f64,
}

impl Pacing {
    /// A frame presented `micros` after the one before it.
    pub(crate) fn record(&mut self, micros: u64) {
        self.micros.push(micros);
    }

    /// The period's summary, `None` without a frame; starts the next period.
    pub(crate) fn take(&mut self) -> Option<Summary> {
        let micros = std::mem::take(&mut self.micros);
        summarize(&micros)
    }
}

fn summarize(micros: &[u64]) -> Option<Summary> {
    let frames = micros.len();
    if frames == 0 {
        return None;
    }
    let ms = |us: u64| us as f64 / 1000.0;
    let mut sorted = micros.to_vec();
    sorted.sort_unstable();
    // Nearest rank: the smallest value with at least `percent` of the frames
    // at or below it.
    let percentile = |percent: usize| ms(sorted[(frames * percent).div_ceil(100).max(1) - 1]);
    let steps = micros.windows(2).map(|w| w[0].abs_diff(w[1])).sum::<u64>();
    Some(Summary {
        frames,
        mean: ms(micros.iter().sum::<u64>()) / frames as f64,
        p50: percentile(50),
        p95: percentile(95),
        p99: percentile(99),
        max: ms(sorted[frames - 1]),
        late: micros.iter().filter(|&&us| us > LATE_US).count() as f64 / frames as f64,
        jitter: if frames > 1 { ms(steps) / (frames - 1) as f64 } else { 0.0 },
    })
}

/// One period of a run as `VICTORY_PERF_LOG` records it. Memory in MB;
/// `None` where the host gave no answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Row {
    /// Seconds since the first presented frame.
    pub seconds: f64,
    /// Frames presented since the run started.
    pub frame: u64,
    pub pacing: Summary,
    pub draws: f64,
    /// Draws and dispatches a frame left out for want of a host pipeline:
    /// a picture with any is not a correct one.
    pub dropped: f64,
    /// Of a frame, the time preparing submissions and the time waiting for
    /// the GPU, in milliseconds.
    pub preparing: f64,
    pub gpu: f64,
    pub memory: Memory,
}

/// Where memory stands at the end of a period, in MB; `None` where the host
/// gave no answer. Four different things, never to be added up or mistaken
/// for one another:
///
/// * **commit** is what the process has been promised, touched or not. It
///   counts against RAM plus paging files, and a figure of tens of GB says
///   nothing by itself about the RAM in use;
/// * the **working set** is what is in physical RAM now. This is what a
///   machine's RAM limits;
/// * **video** memory is on the card, or (shared) in system RAM that the
///   GPU reads;
/// * the **paging file** in use is what the machine as a whole has had to
///   put out of RAM.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Memory {
    /// Our process's private commit now, and its highest since the start.
    pub commit: Option<f64>,
    pub peak_commit: Option<f64>,
    /// Our process's working set now, and its highest since the start.
    pub working_set: Option<f64>,
    pub peak_working_set: Option<f64>,
    /// Page faults a second over the period, soft ones included: a rate
    /// that stays high while the working set is flat is the machine paging.
    pub page_faults: Option<f64>,
    /// The machine's commit charge and its limit (RAM and paging files).
    pub system_commit: Option<f64>,
    pub commit_limit: Option<f64>,
    /// The machine's RAM, and what of it is free for use now.
    pub physical: Option<f64>,
    pub physical_free: Option<f64>,
    /// The machine's paging files: in use now, and their size.
    pub pagefile: Option<f64>,
    pub pagefile_size: Option<f64>,
    /// Video memory on the card: used and the budget the system gives us.
    pub video: Option<f64>,
    pub video_budget: Option<f64>,
    /// Video memory in system RAM (shared): used.
    pub video_shared: Option<f64>,
    /// What we hold ourselves, by our own count: host textures, the heaps
    /// they are placed in (used and free), and list upload memory.
    pub host_textures: Option<f64>,
    pub texture_heaps: Option<f64>,
    pub uploads: Option<f64>,
}

impl Memory {
    /// The columns, in [`MEMORY_COLUMNS`]' order.
    fn values(&self) -> [Option<f64>; 17] {
        [
            self.commit,
            self.working_set,
            self.system_commit,
            self.commit_limit,
            self.video,
            self.video_budget,
            self.video_shared,
            self.peak_commit,
            self.peak_working_set,
            self.page_faults,
            self.physical,
            self.physical_free,
            self.pagefile,
            self.pagefile_size,
            self.host_textures,
            self.texture_heaps,
            self.uploads,
        ]
    }
}

const FRAME_COLUMNS: &str = "seconds,frame,frames,fps,mean_ms,p50_ms,p95_ms,p99_ms,max_ms,late_share,jitter_ms,\
draws_per_frame,dropped_per_frame,preparing_ms,gpu_ms";

const MEMORY_COLUMNS: &str = "commit_mb,working_set_mb,system_commit_mb,commit_limit_mb,video_mb,video_budget_mb,\
video_shared_mb,peak_commit_mb,peak_working_set_mb,page_faults_per_s,physical_mb,physical_free_mb,pagefile_mb,\
pagefile_size_mb,host_textures_mb,texture_heaps_mb,uploads_mb";

/// The first line of the file: [`Row::line`]'s columns. Readers find
/// columns by name (`cargo xtask perf-summary`), so columns are only added.
pub(crate) fn header() -> String {
    format!("{FRAME_COLUMNS},{MEMORY_COLUMNS}")
}

impl Row {
    /// The row as a line of comma-separated values (no newline); an unknown
    /// value is an empty field.
    pub(crate) fn line(&self) -> String {
        let p = &self.pacing;
        let fps = if p.mean > 0.0 { 1000.0 / p.mean } else { 0.0 };
        let known = self.memory.values().map(|v| v.map_or(String::new(), |v| format!("{v:.0}"))).join(",");
        format!(
            "{:.1},{},{},{fps:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.4},{:.2},{:.0},{:.1},{:.2},{:.2},{known}",
            self.seconds,
            self.frame,
            p.frames,
            p.mean,
            p.p50,
            p.p95,
            p.p99,
            p.max,
            p.late,
            p.jitter,
            self.draws,
            self.dropped,
            self.preparing,
            self.gpu,
        )
    }
}

/// Whether `VICTORY_PERF_LOG` names a file.
pub(crate) fn logging() -> bool {
    path().is_some()
}

fn path() -> Option<&'static std::ffi::OsString> {
    static PATH: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| std::env::var_os("VICTORY_PERF_LOG").filter(|p| !p.is_empty())).as_ref()
}

/// Appends `row` to the `VICTORY_PERF_LOG` file, made anew by the run's
/// first row, under [`header`]. Written a row at a time, so a run that ends
/// in a crash keeps what it measured.
pub(crate) fn log(row: &Row) {
    static FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
    let Some(path) = path() else { return };
    let mut file = FILE.lock().unwrap_or_else(|e| e.into_inner());
    if file.is_none() {
        *file = create(std::path::Path::new(path))
            .inspect_err(|e| tracing::warn!("VICTORY_PERF_LOG: {}: {e}", path.to_string_lossy()))
            .ok();
    }
    if let Some(file) = file.as_mut() {
        let _ = append(file, row);
    }
}

fn create(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    writeln!(file, "{}", header())?;
    Ok(file)
}

fn append(mut to: impl std::io::Write, row: &Row) -> std::io::Result<()> {
    writeln!(to, "{}", row.line())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(micros: &[u64]) -> Summary {
        let mut pacing = Pacing::default();
        for &us in micros {
            pacing.record(us);
        }
        pacing.take().unwrap()
    }

    #[test]
    fn an_even_30_fps_has_no_late_frames_and_no_jitter() {
        let s = of(&[TARGET_US; 150]);
        assert_eq!(s.frames, 150);
        for ms in [s.mean, s.p50, s.p95, s.p99, s.max] {
            assert!((ms - 33.333).abs() < 1e-9, "{ms}");
        }
        assert_eq!((s.late, s.jitter), (0.0, 0.0));
    }

    /// The case an average hides: 30 frames in a second, one of them a hitch.
    #[test]
    fn a_hitch_shows_in_the_tail_where_the_mean_hides_it() {
        let mut micros = vec![20_000; 29];
        micros.push(420_000);
        let s = of(&micros);
        assert!((s.mean - 33.333).abs() < 0.001, "30 fps on average: {}", s.mean);
        assert_eq!((s.p50, s.p95), (20.0, 20.0));
        assert_eq!((s.p99, s.max), (420.0, 420.0));
        assert!((s.late - 1.0 / 30.0).abs() < 1e-12);
        assert!((s.jitter - 400.0 / 29.0).abs() < 1e-9, "one step of 400 ms: {}", s.jitter);
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let micros: Vec<u64> = (1..=100).map(|ms| ms * 1000).collect();
        let s = of(&micros);
        assert_eq!((s.p50, s.p95, s.p99, s.max), (50.0, 95.0, 99.0, 100.0));
        // In any order.
        let reversed: Vec<u64> = micros.iter().rev().copied().collect();
        let r = of(&reversed);
        assert_eq!((r.p50, r.p95, r.p99, r.max), (50.0, 95.0, 99.0, 100.0));
        assert_eq!(of(&[7_000]).p99, 7.0, "one frame is every percentile");
    }

    #[test]
    fn late_is_past_a_frame_and_a_half() {
        assert_eq!(LATE_US, 49_999);
        assert_eq!(of(&[LATE_US, LATE_US]).late, 0.0, "at the limit is on time");
        assert_eq!(of(&[LATE_US + 1, TARGET_US]).late, 0.5);
    }

    /// Alternating 16.7 and 50 ms averages 30 fps too; every step is uneven.
    #[test]
    fn jitter_tells_an_uneven_pace_from_an_even_one() {
        let uneven: Vec<u64> = (0..100).map(|i| if i % 2 == 0 { 16_667 } else { 50_000 }).collect();
        let s = of(&uneven);
        assert!((s.mean - 33.3335).abs() < 0.001);
        assert!((s.jitter - 33.333).abs() < 0.001, "{}", s.jitter);
        assert_eq!(s.late, 0.5);
    }

    #[test]
    fn a_period_without_frames_has_no_summary_and_taking_starts_a_new_one() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.take(), None);
        pacing.record(10_000);
        assert_eq!(pacing.take().unwrap().frames, 1);
        assert_eq!(pacing.take(), None);
    }

    #[test]
    fn the_file_starts_with_the_header_and_gains_a_line_per_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf.csv");
        let row = Row {
            seconds: 5.0,
            frame: 150,
            pacing: of(&[TARGET_US; 150]),
            draws: 0.0,
            dropped: 0.0,
            preparing: 0.0,
            gpu: 0.0,
            memory: Memory::default(),
        };
        let mut file = create(&path).unwrap();
        append(&mut file, &row).unwrap();
        append(&mut file, &row).unwrap();
        drop(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines, [header().as_str(), &row.line(), &row.line()]);
    }

    #[test]
    fn a_row_has_the_headers_columns_and_leaves_unknown_values_empty() {
        let mut row = Row {
            seconds: 12.34,
            frame: 370,
            pacing: of(&[TARGET_US; 150]),
            draws: 5123.4,
            dropped: 0.25,
            preparing: 12.345,
            gpu: 6.5,
            memory: Memory {
                commit: Some(20_480.4),
                peak_commit: Some(21_000.0),
                working_set: Some(9_000.0),
                peak_working_set: Some(9_500.0),
                page_faults: Some(1_234.4),
                system_commit: Some(28_000.0),
                commit_limit: Some(40_000.0),
                physical: Some(16_300.0),
                physical_free: Some(2_100.0),
                pagefile: Some(3_000.0),
                pagefile_size: Some(24_000.0),
                video: Some(9_500.0),
                video_budget: Some(15_000.0),
                video_shared: Some(120.0),
                host_textures: Some(8_000.0),
                texture_heaps: Some(8_600.0),
                uploads: Some(64.0),
            },
        };
        let (line, columns) = (row.line(), header());
        assert_eq!(line.split(',').count(), columns.split(',').count());
        assert_eq!(
            line,
            "12.3,370,150,30.00,33.33,33.33,33.33,33.33,33.33,0.0000,0.00,5123,0.2,12.35,6.50,\
             20480,9000,28000,40000,9500,15000,120,21000,9500,1234,16300,2100,3000,24000,8000,8600,64"
        );
        (row.memory.commit, row.memory.video_budget, row.memory.uploads) = (None, None, None);
        let line = row.line();
        assert_eq!(line.split(',').count(), columns.split(',').count());
        assert!(line.contains(",6.50,,9000,28000,40000,9500,,120,"), "{line}");
        assert!(line.ends_with(",8600,"), "{line}");
    }

    /// Each value under the column that names it: the reader goes by name.
    #[test]
    fn every_memory_value_sits_under_its_own_column() {
        let memory = Memory {
            commit: Some(1.0),
            working_set: Some(2.0),
            system_commit: Some(3.0),
            commit_limit: Some(4.0),
            video: Some(5.0),
            video_budget: Some(6.0),
            video_shared: Some(7.0),
            peak_commit: Some(8.0),
            peak_working_set: Some(9.0),
            page_faults: Some(10.0),
            physical: Some(11.0),
            physical_free: Some(12.0),
            pagefile: Some(13.0),
            pagefile_size: Some(14.0),
            host_textures: Some(15.0),
            texture_heaps: Some(16.0),
            uploads: Some(17.0),
        };
        let names: Vec<&str> = MEMORY_COLUMNS.split(',').collect();
        assert_eq!(names.len(), memory.values().len());
        let at = |name: &str| memory.values()[names.iter().position(|n| *n == name).unwrap()];
        for (name, value) in [
            ("commit_mb", 1.0),
            ("working_set_mb", 2.0),
            ("system_commit_mb", 3.0),
            ("commit_limit_mb", 4.0),
            ("video_mb", 5.0),
            ("video_budget_mb", 6.0),
            ("video_shared_mb", 7.0),
            ("peak_commit_mb", 8.0),
            ("peak_working_set_mb", 9.0),
            ("page_faults_per_s", 10.0),
            ("physical_mb", 11.0),
            ("physical_free_mb", 12.0),
            ("pagefile_mb", 13.0),
            ("pagefile_size_mb", 14.0),
            ("host_textures_mb", 15.0),
            ("texture_heaps_mb", 16.0),
            ("uploads_mb", 17.0),
        ] {
            assert_eq!(at(name), Some(value), "{name}");
        }
        // The columns the first version of the file had keep their places.
        assert!(header().starts_with(
            "seconds,frame,frames,fps,mean_ms,p50_ms,p95_ms,p99_ms,max_ms,late_share,jitter_ms,\
             draws_per_frame,dropped_per_frame,preparing_ms,gpu_ms,commit_mb,working_set_mb,\
             system_commit_mb,commit_limit_mb,video_mb,video_budget_mb,video_shared_mb,"
        ));
    }
}
