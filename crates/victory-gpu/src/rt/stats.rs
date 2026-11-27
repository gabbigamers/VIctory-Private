//! What the backend did and what it refused, counted. Nothing it cannot do
//! is dropped quietly: every refusal and every fallback adds to a counter
//! here, and a run or a validation tool prints them.
//!
//! Counters are relaxed atomics, as the memory gauges are
//! (`crate::account`): counting takes no lock and never fails.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// One thing counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Count {
    /// Bottom and top levels built whole, the first time.
    BottomBuilds,
    TopBuilds,
    /// Structures refitted in place, and of those the refits made past the
    /// policy's refits in a row because the memory bound had no room for
    /// the rebuild.
    Refits,
    RefitsPastLimit,
    /// Structures built again, and of those by reason
    /// ([`super::update::Why`]; a first build has none).
    Rebuilds,
    RebuildsNotUpdatable,
    RebuildsCompacted,
    RebuildsShape,
    RebuildsContents,
    RebuildsRefitLimit,
    /// Structures copied into less memory, and the bytes that saved.
    Compactions,
    CompactionBytesSaved,
    /// Builds that asked for compaction and got no slot for the size, so
    /// stay as built; and compactions asked of a structure refitted since
    /// its build reported the size, which is then not known.
    CompactionsSkipped,
    CompactionsRefusedRefitted,
    /// Builds refused: a description D3D12 or this backend does not take,
    /// a state that forbids it, the memory budget, or the device.
    BuildsRefusedInvalid,
    BuildsRefusedState,
    BuildsRefusedBudget,
    BuildsRefusedDevice,
    /// Reads (bindings of a top level) refused by its state.
    ReadsRefused,
    /// Scratch or instance memory refused by its pool's bound.
    PoolRefusals,
    /// Structures given up, and buffers released after their last use.
    Retired,
    Released,
    /// Lists submitted, waits for them, and the microseconds waited.
    Submissions,
    Waits,
    WaitMicros,
    /// Regions that were not timed: no free timestamp pair. Regions cut by
    /// their list's submission, which have no time. Results dropped unread
    /// because more were kept than the backend holds.
    RegionsUntimed,
    RegionsCut,
    TimingsDropped,
}

impl Count {
    pub const ALL: [Count; 28] = [
        Count::BottomBuilds,
        Count::TopBuilds,
        Count::Refits,
        Count::RefitsPastLimit,
        Count::Rebuilds,
        Count::RebuildsNotUpdatable,
        Count::RebuildsCompacted,
        Count::RebuildsShape,
        Count::RebuildsContents,
        Count::RebuildsRefitLimit,
        Count::Compactions,
        Count::CompactionBytesSaved,
        Count::CompactionsSkipped,
        Count::CompactionsRefusedRefitted,
        Count::BuildsRefusedInvalid,
        Count::BuildsRefusedState,
        Count::BuildsRefusedBudget,
        Count::BuildsRefusedDevice,
        Count::ReadsRefused,
        Count::PoolRefusals,
        Count::Retired,
        Count::Released,
        Count::Submissions,
        Count::Waits,
        Count::WaitMicros,
        Count::RegionsUntimed,
        Count::RegionsCut,
        Count::TimingsDropped,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Count::BottomBuilds => "bottom_builds",
            Count::TopBuilds => "top_builds",
            Count::Refits => "refits",
            Count::RefitsPastLimit => "refits_past_limit",
            Count::Rebuilds => "rebuilds",
            Count::RebuildsNotUpdatable => "rebuilds_not_updatable",
            Count::RebuildsCompacted => "rebuilds_compacted",
            Count::RebuildsShape => "rebuilds_shape",
            Count::RebuildsContents => "rebuilds_contents",
            Count::RebuildsRefitLimit => "rebuilds_refit_limit",
            Count::Compactions => "compactions",
            Count::CompactionBytesSaved => "compaction_bytes_saved",
            Count::CompactionsSkipped => "compactions_skipped",
            Count::CompactionsRefusedRefitted => "compactions_refused_refitted",
            Count::BuildsRefusedInvalid => "builds_refused_invalid",
            Count::BuildsRefusedState => "builds_refused_state",
            Count::BuildsRefusedBudget => "builds_refused_budget",
            Count::BuildsRefusedDevice => "builds_refused_device",
            Count::ReadsRefused => "reads_refused",
            Count::PoolRefusals => "pool_refusals",
            Count::Retired => "retired",
            Count::Released => "released",
            Count::Submissions => "submissions",
            Count::Waits => "waits",
            Count::WaitMicros => "wait_us",
            Count::RegionsUntimed => "regions_untimed",
            Count::RegionsCut => "regions_cut",
            Count::TimingsDropped => "timings_dropped",
        }
    }

    /// The counter of rebuilds for `why`; a first build has none.
    pub const fn rebuild(why: super::update::Why) -> Option<Count> {
        use super::update::Why;
        match why {
            Why::NotBuilt => None,
            Why::NotUpdatable => Some(Count::RebuildsNotUpdatable),
            Why::Compacted => Some(Count::RebuildsCompacted),
            Why::ShapeChanged => Some(Count::RebuildsShape),
            Why::ContentsChanged => Some(Count::RebuildsContents),
            Why::RefitLimit => Some(Count::RebuildsRefitLimit),
        }
    }
}

/// The counters of one backend instance.
pub struct Stats {
    counts: [AtomicU64; Count::ALL.len()],
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

impl Stats {
    pub const fn new() -> Self {
        Stats { counts: [const { AtomicU64::new(0) }; Count::ALL.len()] }
    }

    pub fn add(&self, count: Count, n: u64) {
        self.counts[count as usize].fetch_add(n, Relaxed);
    }

    pub fn one(&self, count: Count) {
        self.add(count, 1);
    }

    pub fn get(&self, count: Count) -> u64 {
        self.counts[count as usize].load(Relaxed)
    }

    /// Every counter that is not zero, as `name=value` pairs in a fixed
    /// order: one log line.
    pub fn line(&self) -> String {
        let parts: Vec<String> = Count::ALL
            .into_iter()
            .filter(|&c| self.get(c) != 0)
            .map(|c| format!("{}={}", c.name(), self.get(c)))
            .collect();
        if parts.is_empty() { "nothing counted".to_owned() } else { parts.join(" ") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt::update::Why;

    #[test]
    fn every_counter_is_listed_where_it_is_numbered_and_has_its_own_name() {
        let mut names: Vec<&str> = Count::ALL.iter().map(|c| c.name()).collect();
        for (i, count) in Count::ALL.into_iter().enumerate() {
            assert_eq!(count as usize, i, "{count:?}");
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Count::ALL.len());
    }

    #[test]
    fn the_line_names_what_happened_and_leaves_out_what_did_not() {
        let stats = Stats::new();
        assert_eq!(stats.line(), "nothing counted");
        stats.one(Count::Refits);
        stats.add(Count::Refits, 2);
        stats.one(Count::BottomBuilds);
        stats.add(Count::WaitMicros, 1500);
        assert_eq!(stats.get(Count::Refits), 3);
        assert_eq!(stats.line(), "bottom_builds=1 refits=3 wait_us=1500");
    }

    #[test]
    fn each_reason_for_a_rebuild_has_a_counter_and_a_first_build_has_none() {
        assert_eq!(Count::rebuild(Why::NotBuilt), None);
        let reasons = [Why::NotUpdatable, Why::Compacted, Why::ShapeChanged, Why::ContentsChanged, Why::RefitLimit];
        let mut counters: Vec<usize> = reasons.iter().map(|&w| Count::rebuild(w).unwrap() as usize).collect();
        counters.dedup();
        assert_eq!(counters.len(), reasons.len());
    }
}
