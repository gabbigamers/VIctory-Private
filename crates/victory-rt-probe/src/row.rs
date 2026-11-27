//! A row of the result file: one scene class, frame and path, with every
//! measurement in a column of its own. The columns and their definitions
//! are here, and docs/dxr-validation.md holds the same table (a test
//! keeps the two the same, names and definitions, in the same order).
//!
//! A column that does not apply to a row, or that could not be measured,
//! is empty: never 0.

/// The paths a file names, in the order of their rows within a frame.
pub const NATIVE: &str = "native";
pub const HYBRID: &str = "hybrid";
/// The null shader: a dispatch that traces nothing.
pub const NULL: &str = "null";
/// The software traversal as a shader over console-format nodes. No such
/// shader is built here: its rows say so and hold nothing else.
pub const SOFTWARE_GPU: &str = "software-gpu";

/// What a row's `status` can be.
pub const RAN: &str = "ran";
pub const FAILED: &str = "failed";
pub const NOT_BUILT: &str = "not built";
pub const NOT_RUN: &str = "not run";

/// Every column, in the file's order, with what it holds.
pub const COLUMNS: &[(&str, &str)] = &[
    ("class", "the scene class, by victory-rt's name"),
    ("seed", "the seed the class's scene and rays were generated from, hex"),
    ("frame", "the frame of the class's case, from 0; empty in the null shader's row, which reads no scene; `loop` in a loop row"),
    ("path", "`native` (host triangles, the triangles shader), `hybrid` (one host box a triangle, the procedural shader), `null` (the null shader), `software-gpu` (not built)"),
    ("status", "`ran`; `failed` (a step on the device failed and `note` says which: the columns of that step and of the steps after it are empty, those of the steps before it are kept, the verdict among them where the answers had come back, and the memory columns hold what was held when it failed); `not built` (the path has no shader here); `not run` (the class has no ray to list, or an earlier frame of the path failed: `note` says which)"),
    ("note", "what failed, where the backend or the probe did other than the frame asked, and what a validation layer said, if one was on"),
    ("container_hash", "the digest in the header of the DXIL container the row's pipeline was made from, 32 hex digits"),
    ("bottoms", "bottom-level structures of the frame's scene"),
    ("instances", "instances in its top level"),
    ("primitives", "triangles over its bottom levels, each counted once"),
    ("flat_boxes", "hybrid: boxes with no extent on at least one axis (a triangle in an axis plane)"),
    ("rays", "rays of the class"),
    ("rays_listed", "ray records in the list a dispatch reads: `rays` less `refused_before`"),
    ("timed_rays", "ray records one timed dispatch lists: the list's rays, begun again as often as it takes"),
    ("skip_closest_hit_cleared", "rays whose SKIP_CLOSEST_HIT_SHADER flag was cleared on the CPU (an inline query runs no such shader)"),
    ("mask_high_bits_cut", "rays whose instance mask had bits above the low eight, which were cut on the CPU"),
    ("refused_before", "rays the shaders cannot take as DXR defines them, refused on the CPU and in no list"),
    ("refused_before_reasons", "those by reason, `reason: count` with `;` between"),
    ("refused_geometry", "records the path's shader wrote as refused (kind bit 31) with reason 1: geometry of the other type"),
    ("refused_range", "the same with reason 2: the procedural shader's test left single precision"),
    ("unwritten", "records of the dispatch that listed each ray once that still hold the poison written before it: no thread wrote them. No refusal: each is judged, as a record that is no hit and no miss"),
    ("refused_malformed", "other records with kind bit 31 that are no refusal a shader writes (no reason, a reason bit this probe does not know, or other words that are not zero). No refusal: each is judged, as a record that is no hit and no miss"),
    ("refused_taken_by_next", "rays this path refused, before or in the dispatch, that the next path gave a hit or a miss (native: the hybrid)"),
    ("refused_unanswered", "rays this path refused that no built path took (the hybrid's next path, software on the GPU, is not built)"),
    ("verdict", "victory-rt's verdict on the rays judged: `pass`, `fallback`, `fail`, `baseline-fail`, `useless`. It says nothing of the rays the path refused, which are counted in the columns before this one. Under the crate's policy, not yet the rule"),
    ("judged", "rays judged: every ray the path has a record for that is no refusal, unwritten records among them"),
    ("clear", "of those, rays with one permitted result"),
    ("ambiguous", "rays with several"),
    ("excluded", "rays counted and not judged (an unstable candidate, or over 16 results)"),
    ("wide", "clear or ambiguous rays with a result whose tolerance is too wide to compare numbers by"),
    ("not_clear_reasons", "the rays that are not clear by the reference's reasons, `reason count` with `;` between"),
    ("matched", "clear rays answered rightly"),
    ("permitted_primary", "ambiguous rays answered as the reference itself answers"),
    ("permitted_other", "ambiguous rays answered with another permitted result"),
    ("ids_only", "right answers compared by ids and face alone (a wide result)"),
    ("beyond_band", "right answers whose error is over 1/8 of the tolerance: no defect, reported"),
    ("missed", "defects: a miss where a hit is required"),
    ("phantom", "defects: a hit where none is permitted"),
    ("wrong_triangle", "defects: a hit on a triangle that is not permitted"),
    ("wrong_face", "defects: the right triangle, the other face"),
    ("numeric", "defects: t, u or v outside the tolerance"),
    ("malformed", "defects: a record that is no hit and no miss (an unwritten record is one, unless its ray is excluded)"),
    ("worst_t_ratio", "the largest error of t among compared hits, as a share of its tolerance"),
    ("worst_uv_ratio", "the same for u and v"),
    ("faces_not_reported", "hybrid: hits judged with the face of the permitted result they name, since the procedural shader reports none"),
    ("model_defects", "defects of the CPU model of the software traversal against the same reference on the same rays; empty where the model refused the frame"),
    ("model_identical", "rays the path and the model answer rightly and word for word alike"),
    ("model_divergent", "rays both answer rightly with different results, first-hit rays apart"),
    ("model_divergent_first_hit", "the same on first-hit rays"),
    ("model_refusal", "why the CPU model has no answers for the frame"),
    ("repeat_differing", "hit records of the checking dispatch (one more dispatch after the timed ones, of the `timed_rays` records a timed one lists, over poisoned records and in no time of the row) that differ, word for word, from the dispatch that listed each ray once; a record it left unwritten differs"),
    ("null_not_miss", "null row: records that are not eight zero words, of its dispatch over the listed rays and of its checking dispatch; a record left unwritten is one"),
    ("trace_samples", "timed dispatches kept that have a GPU time"),
    ("trace_untimed", "timed dispatches kept that have none"),
    ("trace_gpu_ms_min", "GPU time between a timestamp before and one after the dispatch alone (`timed_rays` records, the production container, a queue doing nothing else, a submission that holds nothing else), over the kept repeats: the least"),
    ("trace_gpu_ms_p50", "the median"),
    ("trace_gpu_ms_p95", "the 95th percentile by nearest rank"),
    ("trace_gpu_ms_max", "the greatest"),
    ("null_gpu_ms_min", "tracing rows: the null shader's `trace_gpu_ms_min` for the same class and `timed_rays`, beside"),
    ("null_gpu_ms_p50", "its median"),
    ("null_gpu_ms_p95", "its 95th percentile"),
    ("null_gpu_ms_max", "its greatest"),
    ("builds_asked", "structures the frame asks to be built, bottom levels and the top level together"),
    ("refits_asked", "structures it asks to be refitted"),
    ("builds_done", "structures the backend built"),
    ("refits_done", "structures the backend refitted in place"),
    ("build_gpu_ms", "GPU time of the frame's bottom-level builds, each between timestamps of its own with its barrier, summed; empty where one has no time"),
    ("refit_gpu_ms", "the same for bottom-level refits"),
    ("tlas_build_gpu_ms", "the same for the top level where it was built"),
    ("tlas_refit_gpu_ms", "the same for the top level where it was refitted"),
    ("build_untimed", "build and refit regions without a time"),
    ("convert_cpu_ms", "processor time turning the frame's scene into the bytes a build reads (vertices, indices, boxes, triangle data, instances)"),
    ("upload_cpu_ms", "processor time making upload memory and copying those bytes in; null row: the same for the class's ray list"),
    ("record_cpu_ms", "processor time describing the builds to the backend and recording them, the device's size queries and making each new structure's memory among it"),
    ("pipeline_cpu_ms", "processor time creating the path's pipeline, the driver's compile among it; in the path's first row of a class only the first time"),
    ("input_bytes", "bytes uploaded for the frame's builds (a refit uploads positions or boxes alone); null row: the bytes of the class's ray list"),
    ("build_wait_ms", "time the submitting thread was blocked from the builds' submission until the card had run it"),
    ("trace_wait_ms_p50", "the same for a timed dispatch's submission, median over the kept repeats"),
    ("readback_wait_ms", "the same for the copy that brings the hit records back"),
    ("own_rt_structures_bytes", "the account gauge of structure memory (on the card) after the frame's builds and dispatches"),
    ("own_rt_scratch_bytes", "the gauge of build scratch and the hit buffer (on the card) then"),
    ("own_rt_buffers_bytes", "the gauge of uploads, instance descriptions and read-backs (system memory the card reads) then"),
    ("video_local_mb_before", "the adapter's figure for this process on the card before the path's first build of the class; cross-check only"),
    ("video_local_mb_after", "the same after the frame's builds and dispatches"),
    ("video_budget_mb", "the adapter's budget for this process on the card then"),
    ("video_shared_mb_before", "the adapter's figure in system memory the card reads, before"),
    ("video_shared_mb_after", "and after"),
    ("commit_mb_before", "the process's private commit before the path's first build of the class"),
    ("commit_mb_after", "and after the frame's builds and dispatches"),
    ("working_set_mb_before", "the process's working set before"),
    ("working_set_mb_after", "and after"),
    ("loop_iterations", "loop row: iterations run"),
    ("iter_ms_min", "loop row: wall time on the submitting thread of one iteration, from the start of its conversion through its builds' submission and wait to the end of the wait for its dispatch: the least"),
    ("iter_ms_p50", "the median"),
    ("iter_ms_p95", "the 95th percentile"),
    ("iter_ms_p99", "the 99th percentile"),
    ("iter_ms_max", "the greatest"),
    ("loop_last_differing", "loop row: hit records of the loop's checking dispatch (one more dispatch after the last iteration, over poisoned records and in no time of the row) that differ from those of the same frame's first dispatch"),
];

/// Text as a field holds it: one line, no comma.
fn field(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            ',' => ';',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// A time or a ratio as a field: enough digits to read back as it was
/// measured to a nanosecond of a millisecond.
fn decimal(value: f64) -> String {
    format!("{value:.6}")
}

/// One row: a value per column, empty where there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    values: Vec<String>,
}

fn place(column: &str) -> usize {
    COLUMNS
        .iter()
        .position(|(name, _)| *name == column)
        .unwrap_or_else(|| panic!("no column {column}"))
}

impl Row {
    pub fn new(class: &str, seed: u64, frame: &str, path: &str) -> Row {
        let mut row = Row { values: vec![String::new(); COLUMNS.len()] };
        row.text("class", class);
        row.text("seed", &format!("{seed:x}"));
        row.text("frame", frame);
        row.text("path", path);
        row
    }

    /// Sets a column. A column that is not in [`COLUMNS`] is a mistake in
    /// this crate, and panics.
    pub fn text(&mut self, column: &str, value: &str) -> &mut Row {
        self.values[place(column)] = field(value);
        self
    }

    pub fn count(&mut self, column: &str, value: u64) -> &mut Row {
        self.text(column, &value.to_string())
    }

    /// Milliseconds, megabytes or a ratio; not set where there is none or
    /// it is no number.
    pub fn number(&mut self, column: &str, value: Option<f64>) -> &mut Row {
        if let Some(value) = value.filter(|v| v.is_finite()) {
            self.text(column, &decimal(value));
        }
        self
    }

    /// Adds to the row's note, `; ` between its parts.
    pub fn note(&mut self, text: &str) -> &mut Row {
        let at = place("note");
        let text = field(text);
        if !text.is_empty() {
            if !self.values[at].is_empty() {
                self.values[at].push_str("; ");
            }
            self.values[at].push_str(&text);
        }
        self
    }

    pub fn get(&self, column: &str) -> &str {
        &self.values[place(column)]
    }

    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// A row from fields read out of a file with the same columns.
    pub fn from_values(values: Vec<String>) -> Option<Row> {
        (values.len() == COLUMNS.len()).then_some(Row { values })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_holds_a_value_per_column_and_nothing_where_there_is_none() {
        let mut row = Row::new("static-soup", 0x5EED, "0", NATIVE);
        row.count("judged", 2000).number("trace_gpu_ms_p50", Some(0.5)).number("trace_gpu_ms_max", None);
        row.number("worst_t_ratio", Some(f64::NAN));
        assert_eq!(row.get("class"), "static-soup");
        assert_eq!(row.get("seed"), "5eed");
        assert_eq!((row.get("judged"), row.get("trace_gpu_ms_p50")), ("2000", "0.500000"));
        assert_eq!(row.get("trace_gpu_ms_max"), "", "not measured is empty, never 0");
        assert_eq!(row.get("worst_t_ratio"), "");
        assert_eq!(row.values().len(), COLUMNS.len());
        assert_eq!(Row::from_values(row.values().to_vec()), Some(row));
        assert_eq!(Row::from_values(vec![String::new(); 3]), None);
    }

    #[test]
    fn text_is_kept_to_one_field() {
        let mut row = Row::new("precision", 1, "", NULL);
        row.note("pipeline: the driver said no,\nin two lines").note("").note("second");
        assert_eq!(row.get("note"), "pipeline: the driver said no; in two lines; second");
        assert!(!row.values().iter().any(|v| v.contains(',') || v.contains('\n')));
    }

    #[test]
    fn column_names_are_words_and_none_is_there_twice() {
        for (k, (name, definition)) in COLUMNS.iter().enumerate() {
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{name}");
            assert!(!definition.is_empty() && !definition.contains('\n'));
            assert!(!COLUMNS[..k].iter().any(|(other, _)| other == name), "{name} twice");
        }
    }

    /// The specification's column table is this file's: the same columns
    /// in the same order, each with the same definition, word for word.
    #[test]
    fn the_specification_defines_every_column_as_this_file_does() {
        let spec = include_str!("../../../docs/dxr-validation.md");
        let start = spec.find("### The columns of `rt-probe.csv`").expect("the column section");
        let section = &spec[start..];
        let section = &section[..section[4..].find("\n## ").map_or(section.len(), |at| at + 4)];
        let mut defined: Vec<(&str, &str)> = Vec::new();
        for line in section.lines().filter(|l| l.starts_with("| `")) {
            // `| `name` | definition |`: the definition is everything
            // between the second bar and the last.
            let rest = line.strip_prefix("| `").expect("filtered above");
            let (name, rest) = rest.split_once("` | ").unwrap_or_else(|| panic!("no name in: {line}"));
            let definition = rest.strip_suffix(" |").unwrap_or_else(|| panic!("no closing bar in: {line}"));
            defined.push((name, definition));
        }
        for (name, definition) in COLUMNS {
            assert!(!definition.contains('|'), "{name}: a bar would end the document's cell early");
        }
        for (k, (name, definition)) in COLUMNS.iter().enumerate() {
            let spec = defined.get(k).unwrap_or_else(|| panic!("docs/dxr-validation.md does not define {name}"));
            assert_eq!(spec.0, *name, "column {k} of the document's table");
            assert_eq!(spec.1, *definition, "docs/dxr-validation.md defines {name} in other words");
        }
        assert_eq!(defined.len(), COLUMNS.len(), "the specification defines a column no row has: {:?}", defined.last());
    }
}
