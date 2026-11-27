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

/// Bytes counted for as long as this lives: added to a gauge when made and
/// taken from it when dropped. Kept as a field of whatever owns the memory,
/// so the count cannot outlive the allocation or be released twice, on any
/// path, without the owner needing a `Drop` of its own.
pub struct Held {
    gauge: &'static Gauge,
    bytes: u64,
}

impl Held {
    /// `bytes` held by `owner` from now until this is dropped.
    pub fn new(owner: Owner, bytes: u64) -> Self {
        Self::on(gauge(owner), bytes)
    }

    /// As [`Held::new`], on a gauge given.
    pub fn on(gauge: &'static Gauge, bytes: u64) -> Self {
        gauge.add(bytes);
        Self { gauge, bytes }
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The memory this stands for is now `bytes` long (it grew or shrank in
    /// place).
    pub fn resize(&mut self, bytes: u64) {
        if bytes >= self.bytes {
            self.gauge.add(bytes - self.bytes);
        } else {
            self.gauge.sub(self.bytes - bytes);
        }
        self.bytes = bytes;
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.gauge.sub(self.bytes);
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
    /// CPU's pool). Not so in a diagnostics run with
    /// `VICTORY_COPIES_IN_VIDEO_MEMORY` on: buffer copies, windows, view
    /// chunks, pages held on demand and the argument and budget buffers of
    /// lists are then on the card, and still counted under their owners.
    GpuSystem,
    /// Video memory: default heaps, on the card of a device that has one
    /// (an adapter without its own memory keeps them in system RAM, which
    /// the adapter's report of its memory says, not this counter).
    Video,
    /// A count, not bytes.
    Count,
}

/// Who holds the memory. One gauge each; the letters and numbers are the
/// rows of "What allocates" in docs/memory.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Owner {
    /// T1: the title's physical pages allocated and not freed: 64 KiB for
    /// each entry of the page table. What the title holds by number, not
    /// what is committed: a free returns nothing until its section closes.
    TitlePages,
    /// T1: the bytes committed in the title's sections: each page once, from
    /// the first time a view shows it (the title's, or ours for a loading
    /// write) until its section's handle is closed. Pages the title has
    /// freed stay counted until then, so this can be more than
    /// `TitlePages`. Under Wine a section is committed whole when made and
    /// counted so: an upper figure there, since such a section costs nothing
    /// until touched. Not seen: a loading view of ours still mapped when the
    /// handle closes keeps the section's memory for as long as it lasts.
    TitlePagesMapped,
    /// T1: the title's sections whose handle is open: one per allocation of
    /// physical pages, closed when no page and no title view of it is left.
    TitleSections,
    /// T5: loading buffers alive: the source read from a file, the
    /// intermediate between zlib and BCPack, the BCPack decoder's records
    /// (at the size its layout gives, for the length of the decode call)
    /// and the fallback destination. Not the decoder's history tables
    /// (under 32 KiB a request), the page lists of a request, or the BCPack
    /// tables parsed once and kept for the run.
    Loading,
    /// G1: host copies of whole title buffers, each at the size asked for
    /// (the buffer rounded up to 64 KiB), while the copy's record lives: the
    /// title holds the buffer, or a list or a slot record still holds the
    /// copy. A lower bound: a view heap slot that still views a buffer
    /// after that keeps the host buffer alive, uncounted, until the slot is
    /// rewritten.
    BufferCopies,
    /// G2: mapping windows for memory outside any buffer object. Never
    /// freed, so this only grows.
    Windows,
    /// G3: chunks holding copies for views off their buffer's element grid:
    /// whole chunks, which are never removed, not the runs in use in them.
    ViewChunks,
    /// G4: the bytes kept from before a submission to tell what its shaders
    /// wrote, while they are kept. Each snapshot buffer handed out and not
    /// yet back in the pool, at the capacity it was handed out with; and
    /// 64 KiB for each page saved when the title writes it while shaders
    /// may (`crate::watch`), until the page is taken again, handed to its
    /// copy-back or disarmed. Not the 64 KiB buffers a copy-back reads
    /// into.
    Snapshots,
    /// G4: snapshot buffers kept for reuse, by capacity: what the pool holds
    /// between submissions.
    SnapshotPool,
    /// G5: texture staging, chunks and larger buffers, in use or kept, while
    /// the staging pools have them. A buffer dropped from a pool that the
    /// upload list still names lives on, uncounted, until that list's reset.
    Staging,
    /// G6: the lookup tables of host view heaps (stride, mark, page and
    /// address tables), without the descriptor heaps themselves and without
    /// the heaps' slot vectors (process heap). Not here either: the two
    /// bitmaps of pages written through views and of slots read, which
    /// belong to no heap. Each table is counted while its heap has it: one
    /// that a list still names after its heap is released lives on,
    /// uncounted, until that list's reset.
    HeapTables,
    /// G7: scratch chunks, upload and readback buffers, and the argument and
    /// budget buffers of host lists. Not the command allocators (runtime
    /// memory), the written-pages bitmap of a list that uses title memory,
    /// or the timestamp buffer of a timing run.
    ListBuffers,
    /// C1: the pipeline library's bytes held for the library, from when the
    /// library is opened from them (not while the file is hashed just
    /// before).
    PipelineLibrary,
    /// C2: the buffer a save of the pipeline library serialises into, until
    /// the save has written it.
    PipelineSave,
    /// C3: the bytes of the shader containers kept for the run and of the
    /// DXIL of each one translated, with the texts of what it asks of the
    /// host's waves; and the count of programs by wave size
    /// (`crate::waves`, bounded). Not the translations' other tables or
    /// the map itself.
    Shaders,
    /// G11 (last here so that the log's earlier columns stay where they
    /// were): the buffers holding copies of 64 KiB title pages that
    /// translated shaders reached by address and no other copy held
    /// (`crate::memory`, with `VICTORY_MEMORY_PAGING`): one whole buffer of
    /// 4 MiB for each aligned 4 MiB of title memory with such a page, from
    /// its first page's copy until the last copy in it is gone. Zero in a
    /// run without the switch.
    DemandPages,
    /// G12 (after G11, for the same reason): slots of host view heaps that
    /// hold a view of a ray-tracing structure, as far as they are followed
    /// (`crate::descriptor::structures::Live`): an entry in each of two
    /// maps on the process heap, about 30 bytes each, at most 2^14 slots a
    /// heap. A count, since the bytes are few and the number is what a run
    /// is read for.
    StructureSlots,
    /// R1: the build descriptions the ray-tracing capture keeps, one for
    /// each destination (`crate::rtcapture`): each entry with its
    /// geometry or instance descriptions, while the registry has it; and
    /// 32 B for each address it knows a structure was made at. Zero
    /// unless `VICTORY_RT_CAPTURE` is on; at most the registry's bound
    /// (64 MiB) and 2 MiB of addresses. Not the registry's own map nodes,
    /// or what a description takes while it is being read (36 MiB at
    /// most, for the call).
    RtCapture,
    /// R2: host ray-tracing structures of the experimental backend
    /// (`crate::rt`): each result buffer at the size it was made with (what
    /// the device asked for, rounded up to 64 KiB), from when it is made
    /// until it is released: while its structure lives, and after that until
    /// every submission that may read it has run. Nothing without the
    /// backend.
    RtStructures,
    /// R3: the video memory of `crate::rt` that is no structure: its build
    /// scratch chunks, in use or kept for reuse, its buffer for compacted
    /// sizes, and the output buffers a validation run's dispatches write.
    RtScratch,
    /// R4: the system-memory buffers of `crate::rt`: chunks of instance
    /// descriptions, geometry it uploads itself (a validation run's; a
    /// title's geometry is its buffer copies), and its read-back buffers
    /// (compacted sizes, timestamps, results read back). Not its query heap
    /// or command allocators (runtime memory), and nothing in video memory
    /// (R3).
    RtBuffers,
}

impl Owner {
    pub const ALL: [Owner; 21] = [
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
        Owner::DemandPages,
        Owner::StructureSlots,
        Owner::RtCapture,
        Owner::RtStructures,
        Owner::RtScratch,
        Owner::RtBuffers,
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
            Owner::DemandPages => "own_demand_pages_mb",
            Owner::StructureSlots => "own_structure_slots",
            Owner::RtCapture => "own_rt_capture_mb",
            Owner::RtStructures => "own_rt_structures_mb",
            Owner::RtScratch => "own_rt_scratch_mb",
            Owner::RtBuffers => "own_rt_buffers_mb",
        }
    }

    pub const fn kind(self) -> Kind {
        match self {
            Owner::TitlePages | Owner::TitlePagesMapped => Kind::Section,
            Owner::TitleSections | Owner::StructureSlots => Kind::Count,
            Owner::Loading
            | Owner::Snapshots
            | Owner::SnapshotPool
            | Owner::PipelineLibrary
            | Owner::PipelineSave
            | Owner::Shaders
            | Owner::RtCapture => Kind::Private,
            Owner::BufferCopies
            | Owner::Windows
            | Owner::ViewChunks
            | Owner::Staging
            | Owner::HeapTables
            | Owner::ListBuffers
            | Owner::DemandPages
            | Owner::RtBuffers => Kind::GpuSystem,
            Owner::RtStructures | Owner::RtScratch => Kind::Video,
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
/// often: their figures are wrong, and a run says so when it first finds
/// each (a warning beside the frame meter's five-second lines).
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
    fn held_bytes_are_counted_while_they_live_and_once() {
        static G: Gauge = Gauge::new();
        let a = Held::on(&G, 100);
        let mut b = Held::on(&G, 30);
        assert_eq!((G.now(), a.bytes(), b.bytes()), (130, 100, 30));
        b.resize(50);
        assert_eq!(G.now(), 150);
        b.resize(10);
        assert_eq!((G.now(), b.bytes()), (110, 10));
        // Moved, not counted again; dropped where it ends up.
        let moved = vec![a];
        assert_eq!(G.now(), 110);
        drop(moved);
        assert_eq!(G.now(), 10);
        drop(b);
        assert_eq!((G.now(), G.peak(), G.underflows()), (0, 150, 0));
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
