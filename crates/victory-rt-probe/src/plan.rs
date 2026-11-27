//! What a run does: its options, checked, and the dispatches and rays
//! they come to, counted before anything runs. The counts are what a run
//! says of its own length; no time is predicted from them.

use victory_shader::emit::rayquery::MAX_RAYS;

use crate::adapter;
use crate::device::Path;

/// The rays one timed dispatch lists where nothing else is asked: 2^20,
/// the count docs/dxr-validation.md gives its cost class.
pub const DEFAULT_TIMED_RAYS: u32 = 1 << 20;
/// Timed dispatches kept, and those run before them and left out.
pub const DEFAULT_REPEATS: u32 = 30;
pub const DEFAULT_DISCARD: u32 = 3;
/// The most of either a run takes: more is a typing error, and each
/// repeat is a submission.
pub const MOST_REPEATS: u32 = 10_000;
pub const MOST_LOOPS: u32 = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub seed: u64,
    /// Scene classes, in the order they run.
    pub classes: Vec<String>,
    /// Ray records one timed dispatch lists: the class's rays, begun again
    /// as often as it takes.
    pub timed_rays: u32,
    /// Timed dispatches kept per path and frame, after `discard` that are
    /// run and left out of the figures.
    pub repeats: u32,
    pub discard: u32,
    /// Iterations of the loop mode per class and path; 0 for none.
    pub loops: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            seed: adapter::default_seed(),
            classes: adapter::classes().into_iter().map(String::from).collect(),
            timed_rays: DEFAULT_TIMED_RAYS,
            repeats: DEFAULT_REPEATS,
            discard: DEFAULT_DISCARD,
            loops: 0,
        }
    }
}

/// A seed as typed: decimal, or hex after `0x`.
pub fn seed(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| format!("not a seed: {text:?} (a number, or hex after 0x)"))
}

/// Scene classes as typed: `all`, or names with commas between. A name
/// the generator does not have is an error that lists those it has; a
/// name twice is an error too, since a result file holds a class once.
pub fn classes(text: &str) -> Result<Vec<String>, String> {
    let known = adapter::classes();
    if text.trim() == "all" {
        return Ok(known.into_iter().map(String::from).collect());
    }
    let mut out: Vec<String> = Vec::new();
    for name in text.split(',').map(str::trim) {
        if !known.contains(&name) {
            return Err(format!("no scene class {name:?}; there are: all, {}", known.join(", ")));
        }
        if out.iter().any(|have| have == name) {
            return Err(format!("scene class {name} is named twice"));
        }
        out.push(name.to_string());
    }
    Ok(out)
}

impl Options {
    /// Whether a run can be made of them, and why not.
    pub fn check(&self) -> Result<(), String> {
        if self.classes.is_empty() {
            return Err("no scene class to run".into());
        }
        for (k, class) in self.classes.iter().enumerate() {
            if adapter::describe(class).is_none() {
                return Err(format!("no scene class {class:?}"));
            }
            if self.classes[..k].contains(class) {
                return Err(format!("scene class {class} is named twice"));
            }
        }
        if self.timed_rays == 0 || self.timed_rays > MAX_RAYS {
            return Err(format!(
                "{} rays in a timed dispatch: one dispatch lists 1 to {MAX_RAYS}",
                self.timed_rays
            ));
        }
        if self.repeats == 0 || self.repeats > MOST_REPEATS || self.discard > MOST_REPEATS {
            return Err(format!(
                "{} repeats kept after {} left out: at least 1 kept, at most {MOST_REPEATS} of either",
                self.repeats, self.discard
            ));
        }
        if self.loops > MOST_LOOPS {
            return Err(format!("{} loop iterations: at most {MOST_LOOPS}", self.loops));
        }
        Ok(())
    }
}

/// What one class brings to the counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassSize {
    pub frames: u32,
    /// Ray records in the list: the class's rays less those refused before
    /// the dispatch.
    pub listed: u32,
}

/// What a run comes to, if every step of it runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub classes: u32,
    /// Frames over the classes; each is built once per path.
    pub frames: u64,
    /// Dispatches of every kind, each a submission of its own.
    pub dispatches: u64,
    /// Of those, the null shader's, which trace nothing.
    pub null_dispatches: u64,
    /// Of those, timed and kept, and timed and left out.
    pub timed_kept: u64,
    pub timed_discarded: u64,
    /// Of those, the dispatches that check: one after each series of timed
    /// dispatches and one after each loop, over poisoned records, in no
    /// time of the file.
    pub checks: u64,
    /// Ray records listed by the tracing dispatches, repeats counted each
    /// time: rays traced.
    pub rays_traced: u64,
    /// Records the null shader's dispatches list.
    pub null_records: u64,
    /// The bytes of the largest ray list and hit buffer a class uploads
    /// and writes.
    pub ray_list_bytes: u64,
    pub hit_buffer_bytes: u64,
}

/// The records a class's ray list holds: enough for the pass that lists
/// every ray once and for a timed dispatch.
pub fn list_records(listed: u32, timed_rays: u32) -> u32 {
    listed.max(timed_rays)
}

/// The counts of a run of `options` over classes of these sizes. Per
/// class: the null shader once (one dispatch over the rays, the timed
/// ones, and one more that lists what a timed one lists, over poisoned
/// records, to check that such a dispatch writes every record); each
/// tracing path the same once a frame; in loop mode one dispatch over the
/// rays per iteration and path, and one more after the last to check. A
/// class with no ray to list dispatches nothing.
pub fn counts(options: &Options, sizes: &[ClassSize]) -> Counts {
    let mut counts = Counts { classes: sizes.len() as u32, ..Counts::default() };
    let paths = Path::ALL.len() as u64;
    let (kept, discarded) = (u64::from(options.repeats), u64::from(options.discard));
    let timed = u64::from(options.timed_rays);
    let loops = u64::from(options.loops);
    // A loop's check: once a path, where there is a loop.
    let loop_checks = if loops > 0 { paths } else { 0 };
    for size in sizes {
        let (frames, listed) = (u64::from(size.frames), u64::from(size.listed));
        counts.frames += frames;
        if listed == 0 {
            continue;
        }
        let passes = paths * frames;
        counts.null_dispatches += 2 + kept + discarded;
        counts.dispatches += (1 + passes) * (2 + kept + discarded) + paths * loops + loop_checks;
        counts.timed_kept += (1 + passes) * kept;
        counts.timed_discarded += (1 + passes) * discarded;
        counts.checks += 1 + passes + loop_checks;
        counts.rays_traced +=
            passes * (listed + (kept + discarded + 1) * timed) + (paths * loops + loop_checks) * listed;
        counts.null_records += listed + (kept + discarded + 1) * timed;
        let records = u64::from(list_records(size.listed, options.timed_rays));
        counts.ray_list_bytes = counts.ray_list_bytes.max(48 * records);
        counts.hit_buffer_bytes = counts.hit_buffer_bytes.max(32 * records);
    }
    counts
}

impl Counts {
    /// The counts in a line, as a run prints them before it starts.
    pub fn line(&self) -> String {
        format!(
            "{} classes, {} frames; {} dispatches ({} of the null shader; {} timed and kept, {} timed and left \
             out, {} that check them and are in no time); {} rays traced; a ray list of up to {} bytes and a hit \
             buffer of up to {} bytes",
            self.classes,
            self.frames,
            self.dispatches,
            self.null_dispatches,
            self.timed_kept,
            self.timed_discarded,
            self.checks,
            self.rays_traced,
            self.ray_list_bytes,
            self.hit_buffer_bytes
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_and_classes_are_read_as_typed() {
        assert_eq!(seed("1"), Ok(1));
        assert_eq!(seed(" 0x5EED20261127 "), Ok(0x5EED_2026_1127));
        assert!(seed("-1").is_err() && seed("0xZZ").is_err() && seed("").is_err());
        assert_eq!(classes("all").unwrap().len(), 10);
        assert_eq!(classes("precision, static-soup").unwrap(), ["precision", "static-soup"]);
        assert!(classes("soup").unwrap_err().contains("static-soup"));
        assert!(classes("precision,precision").unwrap_err().contains("twice"));
        assert!(classes("").is_err());
    }

    #[test]
    fn options_that_make_no_run_are_refused_with_a_reason() {
        let good = Options::default();
        assert_eq!(good.check(), Ok(()));
        assert_eq!((good.timed_rays, good.repeats, good.discard, good.loops), (1 << 20, 30, 3, 0));
        let bad = |change: fn(&mut Options)| {
            let mut options = Options::default();
            change(&mut options);
            options.check().unwrap_err()
        };
        assert!(bad(|o| o.classes.clear()).contains("no scene class"));
        assert!(bad(|o| o.classes.push("no-such".into())).contains("no-such"));
        assert!(bad(|o| o.classes.push("precision".into())).contains("twice"));
        assert!(bad(|o| o.timed_rays = 0).contains("rays"));
        assert!(bad(|o| o.timed_rays = MAX_RAYS + 1).contains("4194240"));
        assert!(bad(|o| o.repeats = 0).contains("repeats"));
        assert!(bad(|o| o.loops = MOST_LOOPS + 1).contains("loop"));
        let most = Options { timed_rays: MAX_RAYS, ..Options::default() };
        assert_eq!(most.check(), Ok(()));
    }

    #[test]
    fn a_run_is_counted_before_it_starts() {
        // One class of one frame and 2,000 rays, at the defaults: the null
        // shader and two paths, each one dispatch over the rays, 33 timed
        // ones and one that checks them.
        let options = Options::default();
        let one = counts(&options, &[ClassSize { frames: 1, listed: 2000 }]);
        assert_eq!((one.dispatches, one.null_dispatches), (3 * 35, 35));
        assert_eq!((one.timed_kept, one.timed_discarded, one.checks), (90, 9, 3));
        assert_eq!(one.rays_traced, 2 * (2000 + 34 * (1 << 20)));
        assert_eq!(one.null_records, 2000 + 34 * (1 << 20));
        assert_eq!((one.ray_list_bytes, one.hit_buffer_bytes), (48 << 20, 32 << 20));
        // Four frames, and a loop of 10: each frame is traced per path,
        // each iteration is a dispatch over the rays per path, and each
        // path's loop is checked by one more.
        let looped = Options { loops: 10, timed_rays: 500, repeats: 2, discard: 1, ..Options::default() };
        let four = counts(&looped, &[ClassSize { frames: 4, listed: 1200 }]);
        assert_eq!(four.dispatches, (1 + 8) * 5 + 20 + 2);
        assert_eq!(four.checks, 1 + 8 + 2);
        assert_eq!(four.rays_traced, 8 * (1200 + 4 * 500) + 22 * 1200);
        assert_eq!(four.ray_list_bytes, 48 * 1200, "the list holds every ray once at least");
        // Every dispatch is of one kind: the null shader's and the two
        // paths' first dispatches, the timed ones, the checks, the loop's.
        assert_eq!(four.dispatches, 9 + four.timed_kept + four.timed_discarded + four.checks + 20);
        // A class with nothing to list dispatches nothing, and is counted
        // as a class.
        let none = counts(&options, &[ClassSize { frames: 3, listed: 0 }]);
        assert_eq!((none.classes, none.frames, none.dispatches, none.rays_traced), (1, 3, 0, 0));
        assert!(one.line().starts_with(
            "1 classes, 1 frames; 105 dispatches (35 of the null shader; 90 timed and kept, 9 timed and left out, 3 \
             that check them and are in no time); "
        ));
    }
}
