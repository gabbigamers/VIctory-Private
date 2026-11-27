//! GPU time of marked regions of a list, from timestamp queries: the
//! bookkeeping of which query belongs to which region and the arithmetic
//! on what is read back, with no device in it (`super::device` records the
//! queries and reads the values).
//!
//! A region is the commands recorded between its begin and its end. Its
//! time is the difference of two timestamps the GPU takes as it reaches
//! those points, in ticks of the queue's timestamp frequency. That is the
//! time the GPU's front end spent between them: on a GPU that runs
//! commands side by side, or defers work, it is not the cost of those
//! commands alone, and it says nothing of the CPU. It is one measurement to
//! read beside the others (the wait for a submission, wall time), not a
//! verdict.
//!
//! The time the CPU spends waiting for the GPU is measured where the wait
//! happens (`super::device`'s wait) and kept apart from these.
//!
//! A region lives in one list. One still open when its list is submitted
//! is cut there: its end is recorded (no query is resolved unwritten), but
//! it has no time, since what it was to measure went on in another list,
//! and it says that it was cut.

/// A region being timed: which list's, and its place among that list's
/// regions. It names nothing in any other list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    list: u64,
    at: u32,
}

/// One region's result.
#[derive(Clone, Debug, PartialEq)]
pub struct Timing {
    pub label: String,
    /// `None` if the region was cut or never ended, or its timestamps do
    /// not make an interval (the later is the smaller, or the frequency is
    /// 0).
    pub gpu_ms: Option<f64>,
    /// The region was still open when its list was submitted.
    pub cut: bool,
}

/// How a region stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Open,
    Ended,
    /// Ended by its list's submission, not by its caller.
    Cut,
}

/// The regions of one list. Region *i* uses queries 2*i* (its begin) and
/// 2*i*+1 (its end) of a heap of timestamp queries.
pub struct Regions {
    capacity: u32,
    /// Which list these are of: counted up by [`Regions::next_list`].
    list: u64,
    /// Each region's label and how it stands.
    regions: Vec<(String, End)>,
}

impl Regions {
    /// For a heap of `2 * capacity` queries.
    pub fn new(capacity: u32) -> Self {
        Regions { capacity, list: 0, regions: Vec::new() }
    }

    /// Begins a region: it and the query to record now. `None` when the
    /// heap has no pair left; the caller counts the region as untimed.
    pub fn begin(&mut self, label: &str) -> Option<(Region, u32)> {
        let at = self.regions.len() as u32;
        if at >= self.capacity {
            return None;
        }
        self.regions.push((label.to_owned(), End::Open));
        Some((Region { list: self.list, at }, 2 * at))
    }

    /// Ends a region: the query to record now. `None` for one that is
    /// ended or cut already, or is not of this list.
    pub fn end(&mut self, region: Region) -> Option<u32> {
        if region.list != self.list {
            return None;
        }
        let (_, end) = self.regions.get_mut(region.at as usize)?;
        if *end != End::Open {
            return None;
        }
        *end = End::Ended;
        Some(2 * region.at + 1)
    }

    /// The list is being submitted: cuts every region still open, oldest
    /// first, and gives the queries to record now, so that no query is
    /// resolved unwritten. The caller counts them.
    pub fn cut_open(&mut self) -> Vec<u32> {
        let mut queries = Vec::new();
        for (at, (_, end)) in (0u32..).zip(&mut self.regions) {
            if *end == End::Open {
                *end = End::Cut;
                queries.push(2 * at + 1);
            }
        }
        queries
    }

    /// The regions begun and neither ended nor cut.
    pub fn open(&self) -> usize {
        self.regions.iter().filter(|(_, end)| *end == End::Open).count()
    }

    /// The queries to resolve: from the first to the last one recorded.
    pub fn queries(&self) -> u32 {
        2 * self.regions.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Each region's time from the resolved `stamps` (one a query, from
    /// query 0) and the queue's ticks a second.
    pub fn resolve(&self, stamps: &[u64], frequency: u64) -> Vec<Timing> {
        self.regions
            .iter()
            .enumerate()
            .map(|(i, (label, end))| {
                let pair = stamps.get(2 * i).zip(stamps.get(2 * i + 1));
                let ended = *end == End::Ended;
                let gpu_ms = pair.filter(|_| ended).and_then(|(&begin, &end)| milliseconds(begin, end, frequency));
                Timing { label: label.clone(), gpu_ms, cut: *end == End::Cut }
            })
            .collect()
    }

    /// The list is done with (submitted, or dropped): its regions, and
    /// `self` empty for the next list, whose regions no handle of this one
    /// names.
    pub fn next_list(&mut self) -> Regions {
        let next = Regions { capacity: self.capacity, list: self.list + 1, regions: Vec::new() };
        std::mem::replace(self, next)
    }
}

/// Results waiting to be read, at most `capacity` of them: beyond that the
/// oldest go, and are counted, so that a caller that never reads them does
/// not hold every region of a run.
pub struct Results {
    capacity: usize,
    kept: std::collections::VecDeque<Timing>,
}

impl Results {
    pub fn new(capacity: usize) -> Self {
        Results { capacity, kept: std::collections::VecDeque::new() }
    }

    /// Adds a list's results; how many of the oldest were dropped for them.
    pub fn add(&mut self, timings: Vec<Timing>) -> usize {
        self.kept.extend(timings);
        let over = self.kept.len().saturating_sub(self.capacity);
        self.kept.drain(..over);
        over
    }

    /// Everything kept, oldest first; nothing is kept after.
    pub fn take(&mut self) -> Vec<Timing> {
        std::mem::take(&mut self.kept).into()
    }

    pub fn len(&self) -> usize {
        self.kept.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }
}

/// The milliseconds between two timestamps of a queue with `frequency`
/// ticks a second; `None` if they are not an interval.
pub fn milliseconds(begin: u64, end: u64, frequency: u64) -> Option<f64> {
    if frequency == 0 || end < begin {
        return None;
    }
    Some((end - begin) as f64 * 1e3 / frequency as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_region_has_a_pair_of_queries() {
        let mut regions = Regions::new(4);
        let (build, q) = regions.begin("build").unwrap();
        assert_eq!(q, 0);
        assert_eq!(regions.end(build), Some(1));
        let (trace, q) = regions.begin("trace").unwrap();
        assert_eq!((q, regions.queries()), (2, 4));
        assert_eq!(regions.end(trace), Some(3));
        assert_eq!(regions.end(trace), None, "a region ends once");
        assert_eq!(regions.end(Region { list: 0, at: 9 }), None);
    }

    #[test]
    fn regions_may_nest_and_overlap() {
        let mut regions = Regions::new(4);
        let (outer, _) = regions.begin("frame").unwrap();
        let (inner, _) = regions.begin("build").unwrap();
        assert_eq!(regions.open(), 2);
        regions.end(inner);
        assert_eq!(regions.open(), 1);
        regions.end(outer);
        assert_eq!(regions.open(), 0);
        assert!(regions.cut_open().is_empty(), "nothing left to cut");
        let times = regions.resolve(&[1_000, 9_000, 2_000, 5_000], 1_000_000);
        assert_eq!(times[0], Timing { label: "frame".into(), gpu_ms: Some(8.0), cut: false });
        assert_eq!(times[1], Timing { label: "build".into(), gpu_ms: Some(3.0), cut: false });
    }

    /// A region open when its list is submitted is cut: its end query is
    /// recorded, it has no time (the rest of what it was to measure is in
    /// another list), and it says so.
    #[test]
    fn a_region_open_at_submission_is_cut_and_has_no_time() {
        let mut regions = Regions::new(4);
        let (done, _) = regions.begin("done").unwrap();
        let (outer, _) = regions.begin("spans the submit").unwrap();
        let (inner, _) = regions.begin("inner").unwrap();
        regions.end(done);
        assert_eq!(regions.cut_open(), [3, 5], "the end queries of the two left open, oldest first");
        assert_eq!(regions.open(), 0);
        assert!(regions.cut_open().is_empty(), "once");
        assert_eq!(regions.end(outer), None, "a cut region is not ended again");
        assert_eq!(regions.end(inner), None);
        let times = regions.resolve(&[10, 20, 30, 90, 40, 90], 1_000);
        assert_eq!(times[0], Timing { label: "done".into(), gpu_ms: Some(10.0), cut: false });
        assert_eq!(times[1], Timing { label: "spans the submit".into(), gpu_ms: None, cut: true });
        assert_eq!(times[2], Timing { label: "inner".into(), gpu_ms: None, cut: true });
    }

    /// A handle of one list names nothing in the next: ending it there
    /// records no query and ends no other region.
    #[test]
    fn a_region_of_one_list_is_not_a_region_of_the_next() {
        let mut regions = Regions::new(4);
        let (outer, _) = regions.begin("outer").unwrap();
        assert_eq!(regions.cut_open(), [1]);
        let first = regions.next_list();
        assert!(regions.is_empty() && !first.is_empty());
        let (inner, query) = regions.begin("inner").unwrap();
        assert_eq!(query, 0, "the same queries are used again");
        assert_ne!(inner, outer);
        assert_eq!(regions.end(outer), None, "the old handle ends nothing here");
        assert_eq!(regions.open(), 1);
        assert_eq!(regions.end(inner), Some(1));
        // And after a list that was dropped.
        let (dropped, _) = regions.begin("dropped").unwrap();
        drop(regions.next_list());
        regions.begin("third").unwrap();
        assert_eq!(regions.end(dropped), None);
        assert!(first.resolve(&[5, 6], 1_000)[0].cut);
    }

    #[test]
    fn results_not_read_are_bounded_and_the_dropped_ones_counted() {
        let timing = |n: u32| Timing { label: n.to_string(), gpu_ms: Some(f64::from(n)), cut: false };
        let mut results = Results::new(3);
        assert_eq!(results.add(vec![timing(1), timing(2)]), 0);
        assert_eq!(results.add(vec![timing(3), timing(4), timing(5)]), 2, "the two oldest went");
        assert_eq!(results.len(), 3);
        assert_eq!(results.take(), [timing(3), timing(4), timing(5)]);
        assert!(results.is_empty());
        assert_eq!(results.add((0..10).map(timing).collect()), 7, "more than it holds at once");
        assert_eq!(results.take(), [timing(7), timing(8), timing(9)]);
        assert_eq!(Results::new(0).add(vec![timing(1)]), 1);
    }

    #[test]
    fn a_full_heap_times_no_further_region() {
        let mut regions = Regions::new(2);
        assert!(regions.begin("a").is_some());
        assert!(regions.begin("b").is_some());
        assert_eq!(regions.begin("c"), None);
        assert_eq!(regions.queries(), 4);
        drop(regions.next_list());
        assert!(regions.is_empty());
        assert_eq!(regions.begin("d").map(|(_, q)| q), Some(0), "the next list starts over");
    }

    #[test]
    fn what_is_not_an_interval_has_no_time() {
        assert_eq!(milliseconds(100, 350, 1_000), Some(250.0));
        assert_eq!(milliseconds(7, 7, 10), Some(0.0));
        assert_eq!(milliseconds(8, 7, 10), None, "the later timestamp is the smaller");
        assert_eq!(milliseconds(1, 2, 0), None, "no frequency");
        let mut regions = Regions::new(4);
        let (ended, _) = regions.begin("ended").unwrap();
        regions.begin("left open").unwrap();
        regions.begin("not read back").unwrap();
        regions.end(ended);
        let times = regions.resolve(&[10, 20, 30, 40], 1_000);
        assert_eq!(times.iter().map(|t| t.gpu_ms).collect::<Vec<_>>(), [Some(10.0), None, None]);
        assert!(times.iter().all(|t| !t.cut), "never ended is not cut: that is the submission's doing");
        assert_eq!(times[2].label, "not read back");
    }
}
