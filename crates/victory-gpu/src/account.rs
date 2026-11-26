//! Memory we hold, counted where it is allocated and freed, by owner.
//!
//! The system's counters say how much a process has and not what it is for.
//! A [`Gauge`] per [`Owner`] holds the bytes alive now and the highest they
//! have been, so that a change made for memory has a measured before and
//! after (docs/memory.md). The performance log writes each gauge's highest
//! value in the period, which catches a buffer that lived for a moment
//! between two rows.
//!
//! A gauge counts what the code asked for. Whether those bytes are commit,
//! in RAM or on the card is the [`Owner`]'s kind, stated beside it; the
//! gauges of different kinds are not to be added up.
//!
//! Counting must not change behaviour: a gauge is two relaxed atomics, takes
//! no lock and never fails. A count that would go below zero stays at zero
//! and is noted ([`Gauge::underflows`]), since that means an allocation and
//! its release are not counted alike and the figure is not to be trusted.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Bytes alive, their highest, and how often they were added to.
pub struct Gauge {
    now: AtomicU64,
    /// The highest `now` since the start.
    peak: AtomicU64,
    /// The highest `now` since [`Gauge::take_period_peak`].
    period: AtomicU64,
    adds: AtomicU64,
    underflows: AtomicU64,
}

impl Gauge {
    pub const fn new() -> Self {
        Self {
            now: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            period: AtomicU64::new(0),
            adds: AtomicU64::new(0),
            underflows: AtomicU64::new(0),
        }
    }

    /// `bytes` more are alive.
    pub fn add(&self, bytes: u64) {
        let now = self.now.fetch_add(bytes, Relaxed).saturating_add(bytes);
        self.peak.fetch_max(now, Relaxed);
        self.period.fetch_max(now, Relaxed);
        self.adds.fetch_add(1, Relaxed);
    }

    /// `bytes` are no longer alive. More than are counted leaves zero and
    /// counts an underflow.
    pub fn sub(&self, bytes: u64) {
        let before = self
            .now
            .fetch_update(Relaxed, Relaxed, |now| Some(now.saturating_sub(bytes)))
            .unwrap_or_else(|now| now);
        if before < bytes {
            self.underflows.fetch_add(1, Relaxed);
        }
    }

    /// The bytes alive are `bytes`: for an owner whose size is known whole
    /// (a pool, a table) and not by its parts.
    pub fn set(&self, bytes: u64) {
        self.now.store(bytes, Relaxed);
        self.peak.fetch_max(bytes, Relaxed);
        self.period.fetch_max(bytes, Relaxed);
    }

    pub fn now(&self) -> u64 {
        self.now.load(Relaxed)
    }

    pub fn peak(&self) -> u64 {
        self.peak.load(Relaxed)
    }

    /// How often bytes were added: allocations, for an owner counted by its
    /// parts.
    pub fn adds(&self) -> u64 {
        self.adds.load(Relaxed)
    }

    /// Releases of more than was counted. Not zero means the figure is
    /// wrong.
    pub fn underflows(&self) -> u64 {
        self.underflows.load(Relaxed)
    }

    /// The highest value since the last call (or the start), and the period
    /// starts again from what is alive now.
    pub fn take_period_peak(&self) -> u64 {
        let now = self.now();
        self.period.swap(now, Relaxed).max(now)
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Self::new()
    }
}

/// What the memory is: which of the four quantities of docs/memory.md its
/// bytes are charged to. Whether touched pages are in RAM is the system's
/// to say for all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The process's private commit (the heap, `VirtualAlloc`).
    Private,
    /// A section backed by the paging file: the machine's commit, not the
    /// process's private commit.
    Section,
    /// Memory for the GPU in system RAM (upload heaps, custom heaps in the
    /// CPU's pool).
    GpuSystem,
    /// A count, not bytes.
    Count,
}

/// Who holds the memory. One gauge each; the letters and numbers are the
/// rows of "What allocates" in docs/memory.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Owner {
    /// T1: the title's physical pages allocated and not freed.
    TitlePages,
    /// T1: of those, the bytes shown in views (committed in their section).
    TitlePagesMapped,
    /// T1: sections alive.
    TitleSections,
    /// T5: loading buffers alive (source, intermediate, records, fallback).
    Loading,
    /// G1: host copies of title buffers.
    BufferCopies,
    /// G2: mapping windows for memory outside any buffer object.
    Windows,
    /// G3: chunks holding copies for views off their buffer's element grid.
    ViewChunks,
    /// G4: snapshots alive during a submission.
    Snapshots,
    /// G4: snapshot buffers kept for reuse.
    SnapshotPool,
    /// G5: texture staging, chunks and larger buffers, in use or kept.
    Staging,
    /// G6: the lookup tables of host view heaps (stride, mark, page and
    /// address tables), without the descriptor heaps themselves.
    HeapTables,
    /// G7: scratch, upload and readback buffers of host lists.
    ListBuffers,
    /// C1: the pipeline library's bytes held for the library.
    PipelineLibrary,
    /// C2: the buffer a save of the pipeline library serialises into.
    PipelineSave,
    /// C3: shader containers and their translations kept for the run.
    Shaders,
}

impl Owner {
    pub const ALL: [Owner; 15] = [
        Owner::TitlePages,
        Owner::TitlePagesMapped,
        Owner::TitleSections,
        Owner::Loading,
        Owner::BufferCopies,
        Owner::Windows,
        Owner::ViewChunks,
        Owner::Snapshots,
        Owner::SnapshotPool,
        Owner::Staging,
        Owner::HeapTables,
        Owner::ListBuffers,
        Owner::PipelineLibrary,
        Owner::PipelineSave,
        Owner::Shaders,
    ];

    /// The owner's column in the performance log.
    pub const fn column(self) -> &'static str {
        match self {
            Owner::TitlePages => "own_title_pages_mb",
            Owner::TitlePagesMapped => "own_title_pages_mapped_mb",
            Owner::TitleSections => "own_title_sections",
            Owner::Loading => "own_loading_mb",
            Owner::BufferCopies => "own_buffer_copies_mb",
            Owner::Windows => "own_windows_mb",
            Owner::ViewChunks => "own_view_chunks_mb",
            Owner::Snapshots => "own_snapshots_mb",
            Owner::SnapshotPool => "own_snapshot_pool_mb",
            Owner::Staging => "own_staging_mb",
            Owner::HeapTables => "own_heap_tables_mb",
            Owner::ListBuffers => "own_list_buffers_mb",
            Owner::PipelineLibrary => "own_pipeline_library_mb",
            Owner::PipelineSave => "own_pipeline_save_mb",
            Owner::Shaders => "own_shaders_mb",
        }
    }

    pub const fn kind(self) -> Kind {
        match self {
            Owner::TitlePages | Owner::TitlePagesMapped => Kind::Section,
            Owner::TitleSections => Kind::Count,
            Owner::Loading
            | Owner::Snapshots
            | Owner::SnapshotPool
            | Owner::PipelineLibrary
            | Owner::PipelineSave
            | Owner::Shaders => Kind::Private,
            Owner::BufferCopies
            | Owner::Windows
            | Owner::ViewChunks
            | Owner::Staging
            | Owner::HeapTables
            | Owner::ListBuffers => Kind::GpuSystem,
        }
    }
}

static GAUGES: [Gauge; Owner::ALL.len()] = [const { Gauge::new() }; Owner::ALL.len()];

/// The gauge of `owner`.
pub fn gauge(owner: Owner) -> &'static Gauge {
    &GAUGES[owner as usize]
}

/// Each owner's highest value in the period that ends now, in the log's
/// units (MB of 10^6 bytes; a count as it is), in [`Owner::ALL`]'s order.
/// Starts the next period.
pub fn take_period() -> [f64; Owner::ALL.len()] {
    Owner::ALL.map(|owner| {
        let value = gauge(owner).take_period_peak() as f64;
        if owner.kind() == Kind::Count { value } else { value / 1e6 }
    })
}

/// The owners whose releases ran ahead of their allocations, with how
/// often: their figures are wrong, and a run says so once at its end.
pub fn miscounted() -> Vec<(Owner, u64)> {
    Owner::ALL.into_iter().map(|o| (o, gauge(o).underflows())).filter(|&(_, n)| n > 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gauge_follows_what_is_alive_and_remembers_its_highest() {
        let g = Gauge::new();
        g.add(100);
        g.add(50);
        assert_eq!((g.now(), g.peak(), g.adds()), (150, 150, 2));
        g.sub(120);
        assert_eq!((g.now(), g.peak()), (30, 150));
        g.add(10);
        assert_eq!((g.now(), g.peak(), g.adds(), g.underflows()), (40, 150, 3, 0));
    }

    /// What the log is for: a buffer made and freed between two rows shows
    /// in the period it lived in, and not in the next.
    #[test]
    fn the_period_peak_catches_a_buffer_that_came_and_went() {
        let g = Gauge::new();
        g.add(10);
        g.add(1_000);
        g.sub(1_000);
        assert_eq!(g.take_period_peak(), 1_010);
        assert_eq!(g.take_period_peak(), 10, "the next period starts from what is alive");
        g.sub(10);
        assert_eq!(g.take_period_peak(), 10, "alive at its start");
        assert_eq!(g.take_period_peak(), 0);
        assert_eq!(g.peak(), 1_010, "the run's highest is kept");
    }

    #[test]
    fn a_release_of_more_than_was_counted_leaves_zero_and_is_noted() {
        let g = Gauge::new();
        g.add(5);
        g.sub(8);
        assert_eq!((g.now(), g.underflows()), (0, 1));
        g.sub(1);
        assert_eq!((g.now(), g.underflows()), (0, 2));
        g.add(3);
        g.sub(3);
        assert_eq!((g.now(), g.underflows()), (0, 2), "an exact release is not one");
    }

    #[test]
    fn a_gauge_set_whole_keeps_its_highest() {
        let g = Gauge::new();
        g.set(512);
        g.set(64);
        assert_eq!((g.now(), g.peak(), g.adds()), (64, 512, 0));
        assert_eq!(g.take_period_peak(), 512);
        assert_eq!(g.take_period_peak(), 64);
    }

    #[test]
    fn counting_from_many_threads_loses_nothing() {
        let g = Gauge::new();
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..10_000 {
                        g.add(3);
                        g.sub(3);
                    }
                    g.add(7);
                });
            }
        });
        assert_eq!((g.now(), g.adds(), g.underflows()), (56, 80_008, 0));
        assert!(g.peak() >= 56 && g.peak() <= 8 * 3 + 56, "{}", g.peak());
    }

    #[test]
    fn every_owner_has_its_own_gauge_and_column() {
        let mut columns: Vec<&str> = Owner::ALL.iter().map(|o| o.column()).collect();
        for (i, owner) in Owner::ALL.into_iter().enumerate() {
            assert_eq!(owner as usize, i, "{owner:?} is listed where it is numbered");
            assert!(owner.column().starts_with("own_"), "{owner:?}");
            let bytes = owner.kind() != Kind::Count;
            assert_eq!(owner.column().ends_with("_mb"), bytes, "{owner:?}: bytes are logged in MB");
        }
        columns.sort_unstable();
        columns.dedup();
        assert_eq!(columns.len(), Owner::ALL.len(), "no two owners share a column");
        assert!(!std::ptr::eq(gauge(Owner::BufferCopies), gauge(Owner::Windows)));
    }
}
