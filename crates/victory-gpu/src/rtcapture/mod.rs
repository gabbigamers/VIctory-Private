//! Recording what the title asks its ray-tracing builder to build
//! (`VICTORY_RT_CAPTURE`, off by default; docs/dxr-build-capture.md).
//!
//! The title's acceleration structures are built by its own middleware
//! (`XboxBvhBuilder.dll`), which the title calls with D3D12's build
//! descriptions and which then runs compute programs through our device.
//! Nothing of ours is called with a description, so the calls are taken
//! where the title's imports of that DLL are bound (`victory_hle`'s
//! `rtcapture`), and passed on unchanged. This module is what those taps
//! record into: [`desc`] reads a description out of title memory,
//! [`registry`] keeps the last one for each destination, and [`Capture`]
//! counts and logs.
//!
//! Nothing is built here and nothing the title or the builder sees is
//! changed; an experimental host ray-tracing backend would read the
//! registry. With the switch unset there is no capture at all:
//! [`capture`] is `None` and the taps are not installed.
//!
//! A tap counts its call on the way in ([`Capture::entered`]) and shows
//! the call to the capture once the builder has returned, with what it
//! returned: a call the builder throws out of, or answers with a failure,
//! records nothing.
//!
//! What a record is worth: the registry knows a destination only from
//! calls it could read. A batched build (its description is a console
//! structure whose layout we do not have), a refused description or a
//! copy that is not applied leaves destinations written without the
//! registry knowing; all are counted ([`Counts::unseen`]), and while that
//! count is not zero the registry is not the whole truth. Nor is it while
//! calls have not returned ([`Counts::unreturned`]): what a build that
//! threw did before it threw is not known.

pub mod desc;
pub mod registry;

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

use desc::{
    Description, GeometryKind, IndexFormat, Level, Limits, Read, Refusal, build_flags,
    dxr_vertex_format, instance_flags,
};
use registry::{Bounds, CopyRefusal, Origin, Registry};

use crate::account::{Gauge, Held, Owner, gauge};

/// The switch: any value but `0` records; `dump` also logs the first bytes
/// of the first batched build descriptions, whose layout is not known.
pub const SWITCH: &str = "VICTORY_RT_CAPTURE";

/// The most descriptions one unpacking call may bring.
const BATCH_LIMIT: u32 = 1 << 16;
/// How many records of each level are logged one by one before only the
/// totals are.
const DETAILED: u32 = 16;
/// How many distinct notes (a refusal with its value, a format, a mode)
/// are logged; more are counted ([`Counts::notes_dropped`]).
const NOTES: usize = 256;
/// How often the totals are logged while calls keep arriving. They are
/// also logged when the number of calls reaches a power of two, when calls
/// have stopped for a [`TICK`], and where the process ends ([`flush`]).
const TOTALS_EVERY: Duration = Duration::from_secs(10);
/// How often the timer looks whether there are totals to log.
const TICK: Duration = Duration::from_secs(2);
/// How many batched calls `dump` logs the first bytes of, and how many
/// bytes.
const DUMPS: u32 = 4;
const DUMP_BYTES: usize = 0x80;
/// What is counted for each address in the set of addresses a structure
/// was made at: the address and its share of the set's nodes at their
/// emptiest.
const MADE_BYTES: u64 = 32;
/// The longest vertex or box stride taken as plausible; a longer one is
/// counted ([`Counts::odd_strides`]).
const STRIDE_MOST: u64 = 0x1000;

/// `D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT`, the one
/// mode the builder is read as doing work for (xbox-hle-lab,
/// docs/GHIDRA-BVH.md, section 4, which calls mode 1 a clone; PC's header
/// calls it compaction, and either way the result is the source's
/// geometry at the destination). In any other mode nothing is known of
/// what the destination holds afterwards.
const COPY_APPLIED: u32 = 1;

/// Which of a copy's two addresses is written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyOrder {
    /// As `ID3D12GraphicsCommandList4::CopyRaytracingAccelerationStructure`
    /// on PC: destination, then source.
    DestinationFirst,
    /// As xbox-hle-lab's notes write the builder's method: source, then
    /// destination.
    SourceFirst,
}

/// The order of a copy's two addresses in the builder's method: not
/// decided. The two sources above disagree and neither is a declaration
/// of the method, so no copy is applied to the registry until a title run
/// shows the order ([`Counts::copies_first_sure`]); then it is set here.
pub const COPY_ORDER: Option<CopyOrder> = None;

/// What has been seen since the start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Calls that reached a tap and were passed to the builder, and those
    /// the builder returned from. Every count below is of calls that
    /// returned.
    pub entered: u64,
    pub returned: u64,
    /// Calls of the single build, each with one description.
    pub build_calls: u64,
    /// Calls of the batched build and the descriptions they brought, none
    /// of which is read.
    pub batched_calls: u64,
    pub batched_descriptions: u64,
    /// Calls that unpack shipped structures, and their descriptions.
    pub rehydrate_calls: u64,
    pub rehydrate_descriptions: u64,
    /// Of those, the calls the builder answered with a failure, and their
    /// descriptions: none of them recorded.
    pub failed_calls: u64,
    pub failed_descriptions: u64,
    pub copy_calls: u64,
    /// Size queries; those for a top level; those naming a console stream
    /// description (geometry in a form the public structures do not have).
    pub size_queries: u64,
    pub size_queries_top: u64,
    pub size_queries_with_stream: u64,

    /// Descriptions read, by level: all of them kept, but for those the
    /// registry had no room for (`full`).
    pub recorded_top: u64,
    pub recorded_bottom: u64,
    /// Of those: updates of an earlier structure, and ones with the
    /// console's packed flag.
    pub updates: u64,
    pub packed: u64,
    /// Descriptions not read ([`Refusal`]).
    pub refused: u64,
    /// Records the registry had no room for.
    pub full: u64,
    /// Structures written where the registry does not know: batched
    /// builds, refused or failed descriptions whose destination could not
    /// be read either, and copies that were not applied. Not zero means
    /// the registry does not describe every structure there is.
    pub unseen: u64,
    /// Of `unseen`, the copies.
    pub unseen_copies: u64,

    /// Over the recorded bottom levels.
    pub triangle_geometries: u64,
    pub box_geometries: u64,
    pub indices: u64,
    pub vertices: u64,
    pub boxes: u64,
    /// Triangle geometries whose vertex format DXR 1.1 does not take.
    pub other_vertex_formats: u64,
    /// Triangle geometries by index format: none, 16 bits, 32 bits.
    pub index_none: u64,
    pub index_u16: u64,
    pub index_u32: u64,
    /// Geometries that do not look like geometry, which is what a console
    /// layout other than PC's would show as. Triangle sets whose corners
    /// (indices, or vertices without an index buffer) are not a multiple
    /// of three; sets with vertices whose stride is zero, or with boxes
    /// whose stride is shorter than a box, or with either longer than
    /// [`STRIDE_MOST`]; sets with vertices, indices or boxes counted and
    /// their buffer at null, or an index buffer or count without an index
    /// format.
    pub odd_counts: u64,
    pub odd_strides: u64,
    pub odd_buffers: u64,
    /// Over the recorded top levels: instances, those without a bottom
    /// level, and those with flags outside DXR 1.1's.
    pub instances: u64,
    pub null_instances: u64,
    pub other_instance_flags: u64,
    /// Distinct bottom levels named by each top level when recorded, and
    /// of those the ones the registry had no bottom level for.
    pub references: u64,
    pub references_unknown: u64,
    pub postbuild: u64,

    /// Copies whose description went from the source to the destination:
    /// none while [`COPY_ORDER`] is not decided.
    pub copies_applied: u64,
    /// Copies in the applied mode that were not applied because the order
    /// of their addresses is not decided: both addresses were forgotten.
    pub copies_undecided: u64,
    /// Copies in a mode other than the one applied: what the registry had
    /// for where they wrote was forgotten.
    pub copies_other_mode: u64,
    /// At which of a copy's two addresses a structure was known to have
    /// been made before it (by a description read, or by an earlier copy
    /// in the applied mode). A copy's source is a structure, so over a run
    /// the argument that is the source is the one known whenever only one
    /// is.
    pub copies_first_known: u64,
    pub copies_second_known: u64,
    pub copies_both_known: u64,
    pub copies_neither_known: u64,
    /// Of the copies with one address known, those made while every
    /// structure made so far was at an address that was read (no batched
    /// build, no description refused or failed without its destination).
    /// Only these decide the order: with a structure made at an address
    /// not read, the one known can be the destination.
    pub copies_first_sure: u64,
    pub copies_second_sure: u64,
    /// Addresses the set of structures made had no room for; not zero
    /// means the six counts above are not reliable.
    pub made_dropped: u64,
    /// Entries forgotten: their memory was mapped or unmapped, or a build,
    /// a failed call or a copy there was not read into the registry.
    pub removed_by_remap: u64,
    pub removed_unread: u64,
    pub notes_dropped: u64,
}

impl Counts {
    /// Calls the builder has not returned from: it threw (the title
    /// catches that), or the call is still running. Nothing is recorded
    /// for them, and what the registry has for their destinations is as
    /// it was before the call.
    pub fn unreturned(&self) -> u64 {
        self.entered.saturating_sub(self.returned)
    }

    /// Whether every structure made so far, copies aside, was made at an
    /// address that was read.
    fn all_placed(&self) -> bool {
        self.unseen == self.unseen_copies && self.made_dropped == 0
    }
}

/// A way a geometry does not look like geometry ([`Counts::odd_counts`]
/// and the two after it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Odd {
    Count,
    Stride,
    Buffer,
}

/// A thing logged once per distinct value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Note {
    Refused(Via, Refusal),
    VertexFormat(u32),
    InstanceFlags(u8),
    /// A copy by its mode, which of its addresses were known, and whether
    /// that decides ([`Counts::copies_first_sure`]).
    Copy(u32, [bool; 2], bool),
    CopyMode(u32),
    CopyUndecided,
    CopyUnknown,
    Batched,
    Stream,
    Full,
    MadeFull,
    Packed,
    BatchTooLong(u32),
    /// What an unpacking call returned, when not zero.
    Result(i32),
    Odd(Odd),
}

/// Which call a description came with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Via {
    Build,
    Rehydrate,
    SizeQuery,
}

impl Via {
    const fn name(self) -> &'static str {
        match self {
            Via::Build => "build",
            Via::Rehydrate => "rehydrate",
            Via::SizeQuery => "size query",
        }
    }
}

struct State {
    registry: Registry,
    counts: Counts,
    notes: BTreeSet<Note>,
    /// Records of each level (top, bottom) logged one by one so far.
    detailed: [u32; 2],
    dumped: u32,
    /// Addresses a structure is known to have been made at and whose
    /// memory has not been remapped since: the destination of each
    /// description read (kept or not), and both addresses of each copy in
    /// the applied mode. Only for telling which argument of a copy is the
    /// source; unlike the registry it does not forget an address a copy
    /// names. At most the registry's bound of entries.
    made: BTreeSet<u64>,
    made_held: Held,
    /// When the totals were last logged, and the counts they showed.
    logged: Option<(Instant, Counts)>,
    /// The counts the timer last saw.
    ticked: Counts,
}

impl State {
    /// Whether `note` is new and may be logged.
    fn first(&mut self, note: Note) -> bool {
        if self.notes.contains(&note) {
            return false;
        }
        if self.notes.len() >= NOTES {
            self.counts.notes_dropped += 1;
            return false;
        }
        self.notes.insert(note)
    }

    /// A structure was made at `at`.
    fn made_at(&mut self, at: u64, most: usize) {
        if self.made.contains(&at) {
            return;
        }
        if self.made.len() >= most {
            self.counts.made_dropped += 1;
            if self.first(Note::MadeFull) {
                tracing::warn!(
                    most,
                    "ray-tracing capture: more addresses with structures than are kept; which argument of a copy is its source is no longer counted reliably"
                );
            }
            return;
        }
        self.made.insert(at);
        self.made_held.resize(self.made.len() as u64 * MADE_BYTES);
    }

    /// Forgets the entry at `at`, which no longer describes what is there.
    fn forget(&mut self, at: u64) {
        let removed = self.registry.remove(at);
        self.counts.removed_unread += u64::from(removed);
    }

    /// The build of the description at `at` is not known to the registry:
    /// forgets what it had for the destination, or counts a structure
    /// made nobody knows where. The destination, if it could be read.
    fn forget_destination(&mut self, read: &mut Read<'_>, at: u64) -> Option<u64> {
        let mut destination = [0; 8];
        let known = at != 0 && read(at, &mut destination);
        match u64::from_le_bytes(destination) {
            destination if known && destination != 0 => {
                self.forget(destination);
                Some(destination)
            }
            _ => {
                self.counts.unseen += 1;
                None
            }
        }
    }
}

/// The recorder: a registry, counts and a log.
pub struct Capture {
    limits: Limits,
    dump: bool,
    copy_order: Option<CopyOrder>,
    /// The most addresses kept in `State::made`.
    made_most: usize,
    state: Mutex<State>,
}

impl Capture {
    /// A recorder of its own, as tests make; a run has one ([`capture`]),
    /// with [`COPY_ORDER`].
    pub fn new(
        limits: Limits,
        bounds: Bounds,
        gauge: &'static Gauge,
        dump: bool,
        copy_order: Option<CopyOrder>,
    ) -> Self {
        Capture {
            limits,
            dump,
            copy_order,
            made_most: bounds.entries,
            state: Mutex::new(State {
                registry: Registry::new(bounds, gauge),
                counts: Counts::default(),
                notes: BTreeSet::new(),
                detailed: [0; 2],
                dumped: 0,
                made: BTreeSet::new(),
                made_held: Held::on(gauge, 0),
                logged: None,
                ticked: Counts::default(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn counts(&self) -> Counts {
        self.state().counts
    }

    /// Reads the registry under its lock. Not to be held across a call
    /// back into the capture.
    pub fn with_registry<R>(&self, f: impl FnOnce(&Registry) -> R) -> R {
        f(&self.state().registry)
    }

    /// A call is about to be passed to the builder. The tap says so
    /// before, and shows the call (one of the five methods below) after:
    /// a call the builder throws out of is counted here and nowhere else.
    pub fn entered(&self) {
        self.state().counts.entered += 1;
    }

    /// The single build, done: one description at `description`, and
    /// `postbuild_count` post-build descriptions at `postbuild`, recorded
    /// on `list`.
    pub fn build(
        &self,
        read: &mut Read<'_>,
        list: u64,
        description: u64,
        postbuild_count: u32,
        postbuild: u64,
    ) {
        let mut state = self.state();
        state.counts.returned += 1;
        state.counts.build_calls += 1;
        self.one(
            &mut state,
            read,
            Via::Build,
            list,
            description,
            (postbuild_count, postbuild),
        );
        self.after(&mut state);
    }

    /// An unpacking call, done: `count` descriptions in an array at
    /// `descriptions`, and what the builder returned (an `HRESULT`). After
    /// a failure nothing is recorded, and what the registry had for the
    /// destinations named is forgotten: how far the builder got is not
    /// known.
    pub fn rehydrate(
        &self,
        read: &mut Read<'_>,
        list: u64,
        count: u32,
        descriptions: u64,
        result: i32,
    ) {
        let mut state = self.state();
        let state = &mut *state;
        state.counts.returned += 1;
        state.counts.rehydrate_calls += 1;
        state.counts.rehydrate_descriptions += u64::from(count);
        let failed = result < 0;
        if failed {
            state.counts.failed_calls += 1;
            state.counts.failed_descriptions += u64::from(count);
        }
        if result != 0 && state.first(Note::Result(result)) {
            let result = format_args!("{result:#010x}");
            if failed {
                tracing::warn!(
                    %result,
                    count,
                    "ray-tracing capture: the builder failed an unpacking call; its descriptions are not recorded and their destinations are forgotten"
                );
            } else {
                tracing::info!(
                    %result,
                    "ray-tracing capture: an unpacking call answered with a success other than 0; recorded as done"
                );
            }
        }
        if count > BATCH_LIMIT {
            // Where they would have gone is not known either.
            state.counts.refused += u64::from(count);
            state.counts.unseen += u64::from(count);
            if state.first(Note::BatchTooLong(count)) {
                tracing::warn!(
                    count,
                    limit = BATCH_LIMIT,
                    "ray-tracing capture: an unpacking call with more descriptions than are read; none recorded"
                );
            }
        } else {
            for i in 0..u64::from(count) {
                // An address that wraps reads as null and is refused as such.
                let at = descriptions.checked_add(i * desc::DESC_BYTES).unwrap_or(0);
                let at = if descriptions == 0 { 0 } else { at };
                if failed {
                    state.forget_destination(read, at);
                } else {
                    self.one(state, read, Via::Rehydrate, list, at, (0, 0));
                }
            }
        }
        self.after(state);
    }

    /// The batched build, done: `count` console batch descriptions at
    /// `descriptions`. Their layout is not public and not known here, so
    /// none is read and each is a build the registry does not see.
    pub fn batched(&self, read: &mut Read<'_>, list: u64, count: u32, descriptions: u64) {
        let mut state = self.state();
        state.counts.returned += 1;
        state.counts.batched_calls += 1;
        state.counts.batched_descriptions += u64::from(count);
        state.counts.unseen += u64::from(count);
        if state.first(Note::Batched) {
            tracing::warn!(
                count,
                list = format_args!("{list:#x}"),
                "ray-tracing capture: batched builds are not read (console description, layout not known); the registry misses them"
            );
        }
        if self.dump && state.dumped < DUMPS && descriptions != 0 {
            state.dumped += 1;
            self.dump_batched(read, count, descriptions);
        }
        self.after(&mut state);
    }

    /// For finding the batch description's layout from one run: its first
    /// bytes as 32-bit words, and whether they read as a public
    /// description. Logged only; nothing is recorded from it.
    fn dump_batched(&self, read: &mut Read<'_>, count: u32, descriptions: u64) {
        let mut bytes = [0u8; DUMP_BYTES];
        let mut len = DUMP_BYTES;
        // Shorter if the memory ends early.
        while len > 0 && !read(descriptions, &mut bytes[..len]) {
            len /= 2;
        }
        let words: Vec<String> = bytes[..len]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| format!("{:08x}", u32::from_le_bytes(*w)))
            .collect();
        let trial = match desc::parse(read, descriptions, self.limits) {
            Ok(d) => format!("reads as a public description: {}", summary(&d)),
            Err(refusal) => format!("does not read as a public description: {refusal}"),
        };
        tracing::info!(
            count,
            at = format_args!("{descriptions:#x}"),
            words = words.join(" "),
            %trial,
            "ray-tracing capture: batch description, first bytes"
        );
    }

    /// A size query, done: the inputs at `inputs`, and the console's
    /// stream description at `stream` or null.
    pub fn size_query(&self, read: &mut Read<'_>, inputs: u64, stream: u64) {
        let mut state = self.state();
        state.counts.returned += 1;
        state.counts.size_queries += 1;
        if stream != 0 {
            state.counts.size_queries_with_stream += 1;
            if state.first(Note::Stream) {
                tracing::warn!(
                    "ray-tracing capture: a size query names a console stream description: geometry may arrive in a form the public description does not have"
                );
            }
        }
        match desc::parse_inputs_head(read, inputs) {
            Ok(head) => state.counts.size_queries_top += u64::from(head.top),
            Err(refusal) => {
                if state.first(Note::Refused(Via::SizeQuery, refusal)) {
                    tracing::warn!(%refusal, "ray-tracing capture: size query not read");
                }
            }
        }
        self.after(&mut state);
    }

    /// A copy between two addresses in `mode`, done.
    ///
    /// Which address is written to is the capture's `copy_order`. While
    /// that is not decided the copy is not applied: the registry forgets
    /// both addresses, since one of them no longer holds what its entry
    /// says and it is not known which. Which address the registry knows
    /// does not decide it for one copy: the destination can be an address
    /// built at before, and the source a structure made by a call that
    /// was not read. Over a run it does ([`Counts::copies_first_sure`]),
    /// so that is counted, and logged once for each distinct case.
    ///
    /// With the order decided, a copy in the applied mode gives the
    /// destination the source's description; in any other mode what the
    /// destination holds afterwards is not known, and its entry goes.
    pub fn copy(&self, list: u64, first: u64, second: u64, mode: u32) {
        let mut state = self.state();
        let state = &mut *state;
        state.counts.returned += 1;
        state.counts.copy_calls += 1;
        let applied = mode == COPY_APPLIED;

        let known = [first, second].map(|at| state.made.contains(&at));
        let sure = state.counts.all_placed();
        let counts = &mut state.counts;
        match known {
            [true, false] => {
                counts.copies_first_known += 1;
                counts.copies_first_sure += u64::from(sure);
            }
            [false, true] => {
                counts.copies_second_known += 1;
                counts.copies_second_sure += u64::from(sure);
            }
            [true, true] => counts.copies_both_known += 1,
            [false, false] => counts.copies_neither_known += 1,
        }
        if state.first(Note::Copy(mode, known, sure)) {
            tracing::info!(
                mode,
                first = format_args!("{first:#x}"),
                second = format_args!("{second:#x}"),
                first_known = known[0],
                second_known = known[1],
                every_structure_placed = sure,
                "ray-tracing capture: a copy, by which of its addresses a structure was known at"
            );
        }
        if applied {
            // Whichever is the source, both hold a structure now.
            state.made_at(first, self.made_most);
            state.made_at(second, self.made_most);
        } else {
            state.counts.copies_other_mode += 1;
            if state.first(Note::CopyMode(mode)) {
                tracing::warn!(
                    mode,
                    "ray-tracing capture: a copy in a mode that is not applied to the registry; what it wrote over is forgotten"
                );
            }
        }

        let (source, destination) = match self.copy_order {
            Some(CopyOrder::DestinationFirst) => (second, first),
            Some(CopyOrder::SourceFirst) => (first, second),
            None => {
                state.counts.copies_undecided += u64::from(applied);
                state.forget(first);
                if second != first {
                    state.forget(second);
                }
                state.counts.unseen += 1;
                state.counts.unseen_copies += 1;
                if state.first(Note::CopyUndecided) {
                    tracing::warn!(
                        first = format_args!("{first:#x}"),
                        second = format_args!("{second:#x}"),
                        "ray-tracing capture: copies are not applied: which argument is the source is not decided; both addresses are forgotten"
                    );
                }
                return self.after(state);
            }
        };
        if !applied {
            state.forget(destination);
            state.counts.unseen += 1;
            state.counts.unseen_copies += 1;
            return self.after(state);
        }
        match state.registry.copy(source, destination, list) {
            Ok(_) => state.counts.copies_applied += 1,
            Err(CopyRefusal::Full(_)) => {
                state.counts.full += 1;
                if state.first(Note::Full) {
                    tracing::warn!(
                        "ray-tracing capture: the registry is full; records are refused"
                    );
                }
            }
            Err(CopyRefusal::UnknownSource { removed }) => {
                state.counts.removed_unread += u64::from(removed);
                state.counts.unseen += 1;
                state.counts.unseen_copies += 1;
                if state.first(Note::CopyUnknown) {
                    tracing::warn!(
                        source = format_args!("{source:#x}"),
                        "ray-tracing capture: a copy of a structure the registry does not know"
                    );
                }
            }
            Err(CopyRefusal::Same) => {}
        }
        self.after(state);
    }

    /// The runtime mapped or unmapped `[at, at + len)`: structures there
    /// are gone.
    pub fn memory_remapped(&self, at: u64, len: u64) {
        let mut state = self.state();
        let removed = state.registry.remove_range(at, len);
        state.counts.removed_by_remap += removed as u64;
        let gone: Vec<u64> = state
            .made
            .range(at..at.saturating_add(len))
            .copied()
            .collect();
        if !gone.is_empty() {
            for at in gone {
                state.made.remove(&at);
            }
            let bytes = state.made.len() as u64 * MADE_BYTES;
            state.made_held.resize(bytes);
        }
    }

    /// Reads and records one description.
    fn one(
        &self,
        state: &mut State,
        read: &mut Read<'_>,
        via: Via,
        list: u64,
        at: u64,
        (postbuild_count, postbuild): (u32, u64),
    ) {
        let parsed = desc::parse(read, at, self.limits).and_then(|description| {
            let postbuild = desc::parse_postbuild(read, postbuild, postbuild_count, self.limits)?;
            Ok((description, postbuild))
        });
        let (description, postbuild) = match parsed {
            Ok(parsed) => parsed,
            Err(refusal) => {
                state.counts.refused += 1;
                // The build happened all the same: what the registry had
                // for its destination is no longer true.
                if let Some(destination) = state.forget_destination(read, at) {
                    state.made_at(destination, self.made_most);
                }
                if state.first(Note::Refused(via, refusal)) {
                    tracing::warn!(
                        via = via.name(),
                        %refusal,
                        description = format_args!("{at:#x}"),
                        "ray-tracing capture: description refused; left to the title's builder alone"
                    );
                }
                return;
            }
        };
        self.tally(state, &description, postbuild.len());
        let origin = match via {
            Via::Rehydrate => Origin::Rehydrate,
            _ => Origin::Build,
        };
        let (destination, top) = (description.destination, description.is_top());
        state.made_at(destination, self.made_most);
        let line = (state.detailed[usize::from(!top)] < DETAILED).then(|| summary(&description));
        let recorded = match state.registry.record(description, postbuild, origin, list) {
            Ok(recorded) => recorded,
            Err(_) => {
                state.counts.full += 1;
                if state.first(Note::Full) {
                    tracing::warn!(
                        "ray-tracing capture: the registry is full; records are refused"
                    );
                }
                return;
            }
        };
        let references = state.registry.resolve(destination).unwrap_or_default();
        let (known, unknown) = (references.known.len(), references.unknown.len());
        state.counts.references += (known + unknown) as u64;
        state.counts.references_unknown += unknown as u64;
        if let Some(line) = line {
            state.detailed[usize::from(!top)] += 1;
            tracing::info!(
                via = via.name(),
                destination = format_args!("{destination:#x}"),
                list = format_args!("{list:#x}"),
                generation = recorded.generation,
                bottom_levels_known = known,
                bottom_levels_unknown = unknown,
                "ray-tracing capture: {}",
                line
            );
        }
    }

    /// Adds a description that was read to the counts, and logs when
    /// first seen: each vertex format, each set of instance flags DXR 1.1
    /// does not have, the console's packed flag, and each way a geometry
    /// does not look like one ([`Odd`]) with the geometry it was seen in.
    fn tally(&self, state: &mut State, description: &Description, postbuild: usize) {
        let counts = &mut state.counts;
        counts.postbuild += postbuild as u64;
        counts.updates += u64::from(description.is_update());
        let packed = description.flags & build_flags::CONSOLE_DEHYDRATE != 0;
        counts.packed += u64::from(packed);
        let mut formats = Vec::new();
        let mut flags = 0;
        // The first geometry showing each oddity.
        let mut odd: [Option<usize>; 3] = [None; 3];
        match &description.level {
            Level::Top(top) => {
                counts.recorded_top += 1;
                counts.instances += top.instances.len() as u64;
                for instance in &top.instances {
                    counts.null_instances += u64::from(instance.bottom_level == 0);
                    let other = instance.flags & !instance_flags::KNOWN;
                    counts.other_instance_flags += u64::from(other != 0);
                    flags |= other;
                }
            }
            Level::Bottom(bottom) => {
                counts.recorded_bottom += 1;
                for (n, geometry) in bottom.geometries.iter().enumerate() {
                    let oddities = oddities(&geometry.kind);
                    counts.odd_counts += u64::from(oddities[0]);
                    counts.odd_strides += u64::from(oddities[1]);
                    counts.odd_buffers += u64::from(oddities[2]);
                    for (first, is) in odd.iter_mut().zip(oddities) {
                        if is {
                            first.get_or_insert(n);
                        }
                    }
                    match geometry.kind {
                        GeometryKind::Triangles(t) => {
                            counts.triangle_geometries += 1;
                            counts.indices += u64::from(t.index_count);
                            counts.vertices += u64::from(t.vertex_count);
                            match t.index_format {
                                IndexFormat::None => counts.index_none += 1,
                                IndexFormat::U16 => counts.index_u16 += 1,
                                IndexFormat::U32 => counts.index_u32 += 1,
                            }
                            if !dxr_vertex_format(t.vertex_format) {
                                counts.other_vertex_formats += 1;
                            }
                            if !formats.contains(&t.vertex_format) {
                                formats.push(t.vertex_format);
                            }
                        }
                        GeometryKind::Boxes(b) => {
                            counts.box_geometries += 1;
                            counts.boxes = counts.boxes.saturating_add(b.count);
                        }
                    }
                }
            }
        }
        for format in formats {
            if state.first(Note::VertexFormat(format)) {
                tracing::info!(
                    format,
                    dxr = dxr_vertex_format(format),
                    "ray-tracing capture: a vertex format"
                );
            }
        }
        if flags != 0 && state.first(Note::InstanceFlags(flags)) {
            tracing::warn!(
                flags = format_args!("{flags:#x}"),
                "ray-tracing capture: instance flags DXR 1.1 does not have"
            );
        }
        if packed && state.first(Note::Packed) {
            tracing::info!(
                flags = format_args!("{:#x}", description.flags),
                "ray-tracing capture: a description with the console's packed flag"
            );
        }
        let kinds = [
            (Odd::Count, "corners not a multiple of three"),
            (Odd::Stride, "a stride of zero, too short or too long"),
            (Odd::Buffer, "a buffer at null or without its format"),
        ];
        for ((kind, what), first) in kinds.into_iter().zip(odd) {
            let (Some(n), Level::Bottom(bottom)) = (first, &description.level) else {
                continue;
            };
            if state.first(Note::Odd(kind)) {
                tracing::warn!(
                    destination = format_args!("{:#x}", description.destination),
                    geometry = n,
                    as_read = ?bottom.geometries[n].kind,
                    "ray-tracing capture: a geometry that does not look like one ({what}); the console's layout may not be PC's"
                );
            }
        }
    }

    /// After each call shown: the totals, when none were logged for
    /// [`TOTALS_EVERY`] or the calls have reached a power of two (so that a
    /// burst of calls, however short, leaves totals of at least half of
    /// it).
    fn after(&self, state: &mut State) {
        let due = state
            .logged
            .is_none_or(|(at, _)| at.elapsed() >= TOTALS_EVERY);
        if due || state.counts.returned.is_power_of_two() {
            self.totals(state);
        }
    }

    /// For the timer, every [`TICK`]: the totals, if they are not the ones
    /// last logged and the calls have stopped since the last tick or
    /// [`TOTALS_EVERY`] has passed. A run that stops calling, or only
    /// copies and queries, still ends with its totals.
    pub fn tick(&self) {
        let mut state = self.state();
        let quiet = state.counts == state.ticked;
        state.ticked = state.counts;
        let due = state
            .logged
            .is_none_or(|(at, _)| at.elapsed() >= TOTALS_EVERY);
        if quiet || due {
            self.totals_if_new(&mut state);
        }
    }

    /// Where the process ends: the totals, if they are not the ones last
    /// logged. Does not wait for the lock, which the thread that ends the
    /// process may hold; then nothing is logged. Whether it logged.
    pub fn flush(&self) -> bool {
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        self.totals_if_new(&mut state)
    }

    /// The totals, unless nothing has been counted since the last (or at
    /// all). Whether they were logged.
    fn totals_if_new(&self, state: &mut State) -> bool {
        let last = state.logged.map_or(Counts::default(), |(_, counts)| counts);
        if last == state.counts {
            return false;
        }
        self.totals(state);
        true
    }

    fn totals(&self, state: &mut State) {
        state.logged = Some((Instant::now(), state.counts));
        let c = &state.counts;
        tracing::info!(
            calls = format_args!("{} entered, {} not returned", c.entered, c.unreturned()),
            builds = c.build_calls,
            batched = format_args!(
                "{} calls, {} descriptions",
                c.batched_calls, c.batched_descriptions
            ),
            rehydrated = format_args!(
                "{} calls, {} descriptions ({} calls failed, {} descriptions)",
                c.rehydrate_calls, c.rehydrate_descriptions, c.failed_calls, c.failed_descriptions
            ),
            copies = format_args!(
                "{} ({} applied, {} undecided, {} in another mode; first/second/both/neither known {}/{}/{}/{}; first/second known with every structure placed {}/{})",
                c.copy_calls,
                c.copies_applied,
                c.copies_undecided,
                c.copies_other_mode,
                c.copies_first_known,
                c.copies_second_known,
                c.copies_both_known,
                c.copies_neither_known,
                c.copies_first_sure,
                c.copies_second_sure
            ),
            size_queries = format_args!(
                "{} ({} top level, {} with a stream)",
                c.size_queries, c.size_queries_top, c.size_queries_with_stream
            ),
            recorded = format_args!(
                "{} top, {} bottom ({} updates, {} packed)",
                c.recorded_top, c.recorded_bottom, c.updates, c.packed
            ),
            refused = c.refused,
            unseen = format_args!("{} ({} by copies)", c.unseen, c.unseen_copies),
            full = c.full,
            geometry = format_args!(
                "{} triangle sets ({} indices, {} vertices, {} in formats DXR does not take; index format none/16/32 {}/{}/{}), {} box sets ({} boxes); odd counts/strides/buffers {}/{}/{}",
                c.triangle_geometries,
                c.indices,
                c.vertices,
                c.other_vertex_formats,
                c.index_none,
                c.index_u16,
                c.index_u32,
                c.box_geometries,
                c.boxes,
                c.odd_counts,
                c.odd_strides,
                c.odd_buffers
            ),
            instances = format_args!(
                "{} ({} null, {} with other flags), bottom levels named {} ({} not known)",
                c.instances,
                c.null_instances,
                c.other_instance_flags,
                c.references,
                c.references_unknown
            ),
            registry = format_args!(
                "{} entries, {} KiB; removed {} by remapping, {} unread; structures known at {} addresses ({} not kept)",
                state.registry.len(),
                state.registry.bytes() >> 10,
                c.removed_by_remap,
                c.removed_unread,
                state.made.len(),
                c.made_dropped
            ),
            "ray-tracing capture: totals"
        );
    }
}

/// Which ways `kind` does not look like geometry: its count, its stride,
/// its buffers ([`Counts::odd_counts`] and the two after it).
fn oddities(kind: &GeometryKind) -> [bool; 3] {
    match *kind {
        GeometryKind::Triangles(t) => {
            let indexed = t.index_format != IndexFormat::None;
            let corners = if indexed {
                t.index_count
            } else {
                t.vertex_count
            };
            [
                !corners.is_multiple_of(3),
                t.vertex_count != 0 && (t.vertex_stride == 0 || t.vertex_stride > STRIDE_MOST),
                (t.vertex_count != 0 && t.vertex_buffer == 0)
                    || (indexed && t.index_count != 0 && t.index_buffer == 0)
                    || (!indexed && (t.index_count != 0 || t.index_buffer != 0)),
            ]
        }
        // A box is six floats; a stride of zero repeats one box, as D3D12
        // allows.
        GeometryKind::Boxes(b) => [
            false,
            b.count != 0 && b.stride != 0 && !(24..=STRIDE_MOST).contains(&b.stride),
            b.count != 0 && b.address == 0,
        ],
    }
}

/// One recorded description in a line.
fn summary(description: &Description) -> String {
    let flags = description.flags;
    let update = if description.is_update() {
        ", update"
    } else {
        ""
    };
    match &description.level {
        Level::Top(top) => {
            let null = top.instances.iter().filter(|i| i.bottom_level == 0).count();
            format!(
                "top level, flags {flags:#x}{update}, {} instances ({null} null) as {:?} at {:#x}",
                top.instances.len(),
                top.layout,
                top.address
            )
        }
        Level::Bottom(bottom) => {
            let (mut sets, mut indices, mut vertices, mut boxes) = (0, 0u64, 0u64, 0u64);
            for geometry in &bottom.geometries {
                match geometry.kind {
                    GeometryKind::Triangles(t) => {
                        sets += 1;
                        indices += u64::from(t.index_count);
                        vertices += u64::from(t.vertex_count);
                    }
                    GeometryKind::Boxes(b) => boxes = boxes.saturating_add(b.count),
                }
            }
            format!(
                "bottom level, flags {flags:#x}{update}, {} geometries as {:?}: {sets} triangle sets ({indices} indices, {vertices} vertices), {boxes} boxes",
                bottom.geometries.len(),
                bottom.layout
            )
        }
    }
}

/// This run's capture, once something has asked for it ([`capture`]).
static CAPTURE: OnceLock<Option<Capture>> = OnceLock::new();

/// This run's capture; `None` unless [`SWITCH`] is on.
pub fn capture() -> Option<&'static Capture> {
    CAPTURE
        .get_or_init(|| {
            if !crate::switch::on(SWITCH) {
                return None;
            }
            let dump = std::env::var_os(SWITCH).is_some_and(|v| v == "dump");
            let (limits, bounds) = (Limits::DEFAULT, Bounds::DEFAULT);
            tracing::info!(
                dump,
                instances = limits.instances,
                geometries = limits.geometries,
                entries = bounds.entries,
                megabytes = bounds.bytes >> 20,
                copy_order = ?COPY_ORDER,
                "ray-tracing capture: on; build descriptions are recorded, nothing else changes"
            );
            // The totals do not wait for a call: a run whose calls come in
            // one burst, or stop, still logs where it ended. The thread
            // waits here until the capture is made.
            let timer = std::thread::Builder::new()
                .name("victory rt capture".into())
                .spawn(|| {
                    loop {
                        std::thread::sleep(TICK);
                        if let Some(capture) = capture() {
                            capture.tick();
                        }
                    }
                });
            if let Err(error) = timer {
                tracing::warn!(
                    %error,
                    "ray-tracing capture: no timer thread; totals are logged only as calls arrive and at exit"
                );
            }
            Some(Capture::new(
                limits,
                bounds,
                gauge(Owner::RtCapture),
                dump,
                COPY_ORDER,
            ))
        })
        .as_ref()
}

/// Whether [`SWITCH`] is on.
pub fn enabled() -> bool {
    capture().is_some()
}

/// The process is about to end (the title exits, aborts, or an exception
/// nobody handled is being reported): logs the totals if they are not the
/// ones last logged. Nothing unless the capture is on and has been asked
/// for: this neither reads the switch nor makes the capture, so it is safe
/// wherever the process ends.
pub fn flush() {
    if let Some(Some(capture)) = CAPTURE.get() {
        capture.flush();
    }
}

/// A reader of title memory for [`Capture`]'s calls: the title's addresses
/// are ours. The read is the kernel's, which fails for a page that is not
/// mapped instead of faulting, and passes pages our guards made read-only.
/// Without a host (anything but Windows) there is no title and nothing
/// reads.
pub fn title_memory() -> impl FnMut(u64, &mut [u8]) -> bool {
    |address, out| {
        #[cfg(windows)]
        {
            usize::try_from(address).is_ok_and(|at| crate::host::read_into(at as *const u8, out))
        }
        #[cfg(not(windows))]
        {
            let _ = (address, out);
            false
        }
    }
}

/// The runtime mapped or unmapped title memory (`crate::watch`).
#[cfg(windows)]
pub(crate) fn memory_remapped(at: usize, len: usize) {
    if let Some(capture) = capture() {
        capture.memory_remapped(at as u64, len as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::desc::image::*;
    use super::desc::{Boxes, Instance, Triangles, What};
    use super::*;

    const DEST: u64 = 0x30_0000_0000;

    fn with(bounds: Bounds, order: Option<CopyOrder>) -> (Capture, &'static Gauge) {
        let gauge = Box::leak(Box::new(Gauge::new()));
        (
            Capture::new(Limits::DEFAULT, bounds, gauge, true, order),
            gauge,
        )
    }

    /// A capture as a run has it: the order of a copy's addresses not
    /// decided.
    fn new() -> Capture {
        with(Bounds::DEFAULT, COPY_ORDER).0
    }

    fn ordered(order: CopyOrder) -> Capture {
        with(Bounds::DEFAULT, Some(order)).0
    }

    /// Puts a bottom level of `geometries` triangle sets for `destination`
    /// at `at` (its array right after it).
    fn put_bottom(memory: &mut Memory, at: u64, destination: u64, geometries: u32, flags: u32) {
        let array = at + 0x100;
        memory.put(
            at,
            desc(destination, (1, flags, geometries, 0), array, 0, 0x1_0000),
        );
        memory.put(
            array,
            (0..geometries)
                .flat_map(|n| triangles(1, &some_triangles(n)))
                .collect(),
        );
    }

    fn put_top(memory: &mut Memory, at: u64, destination: u64, bottoms: &[u64]) {
        let array = at + 0x100;
        memory.put(
            at,
            desc(
                destination,
                (0, 0, bottoms.len() as u32, 0),
                array,
                0,
                0x1_0000,
            ),
        );
        let instances = bottoms
            .iter()
            .enumerate()
            .map(|(n, &b)| some_instance(n as u32, b));
        memory.put(array, instances.flat_map(|i| instance_image(&i)).collect());
    }

    /// How many geometries the bottom level recorded at `at` has, if one
    /// is.
    fn geometries(c: &Capture, at: u64) -> Option<usize> {
        c.with_registry(|r| match &r.get(at)?.description.level {
            Level::Bottom(bottom) => Some(bottom.geometries.len()),
            Level::Top(_) => None,
        })
    }

    #[test]
    fn the_order_of_a_copys_addresses_is_not_decided() {
        // No copy is applied in a run until this is set from evidence.
        assert_eq!(COPY_ORDER, None);
    }

    #[test]
    fn builds_are_recorded_counted_and_resolved() {
        let c = new();
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 3, build_flags::ALLOW_COMPACTION);
        put_bottom(&mut memory, 0x2000, DEST + 0x1000, 1, 0);
        put_top(
            &mut memory,
            0x3000,
            DEST + 0x8000,
            &[DEST, DEST + 0x1000, 0, DEST + 0x5000, DEST],
        );
        memory.put(0x4000, words(&[(8, 0x9000), (4, 0), (4, 0)]));
        let read = &mut memory.reader();
        c.build(read, 0x11, 0x1000, 1, 0x4000);
        c.build(read, 0x11, 0x2000, 0, 0);
        c.build(read, 0x22, 0x3000, 0, 0);

        let counts = c.counts();
        assert_eq!(
            (
                counts.build_calls,
                counts.recorded_bottom,
                counts.recorded_top
            ),
            (3, 2, 1)
        );
        assert_eq!(counts.returned, 3);
        assert_eq!((counts.refused, counts.unseen, counts.full), (0, 0, 0));
        assert_eq!((counts.triangle_geometries, counts.box_geometries), (4, 0));
        // some_triangles(0..3) and (0): 300 + 303 + 0, and 300.
        assert_eq!(
            (counts.indices, counts.vertices),
            (903, 100 + 101 + 102 + 100)
        );
        // 16 bits twice, then 32 bits, then none.
        assert_eq!(
            (counts.index_u16, counts.index_u32, counts.index_none),
            (2, 1, 1)
        );
        assert_eq!(
            (counts.odd_counts, counts.odd_strides, counts.odd_buffers),
            (0, 0, 0)
        );
        assert_eq!(counts.other_vertex_formats, 0);
        assert_eq!((counts.instances, counts.null_instances), (5, 1));
        assert_eq!(counts.other_instance_flags, 0);
        assert_eq!((counts.references, counts.references_unknown), (3, 1));
        assert_eq!(counts.postbuild, 1);
        c.with_registry(|r| {
            assert_eq!(r.len(), 3);
            let bottom = r.get(DEST).unwrap();
            assert_eq!(
                (bottom.list, bottom.origin, bottom.generation),
                (0x11, Origin::Build, 0)
            );
            assert_eq!(bottom.postbuild.len(), 1);
            assert_eq!(bottom.postbuild[0].destination, 0x9000);
            let references = r.resolve(DEST + 0x8000).unwrap();
            assert_eq!(references.known, [(DEST, 1), (DEST + 0x1000, 2)]);
            assert_eq!(references.unknown, [DEST + 0x5000]);
        });

        // Built again: the same entry, a generation later.
        c.build(read, 0x33, 0x1000, 0, 0);
        c.with_registry(|r| {
            let bottom = r.get(DEST).unwrap();
            assert_eq!(
                (bottom.list, bottom.generation, bottom.sequence),
                (0x33, 1, 4)
            );
            assert!(bottom.postbuild.is_empty());
        });
    }

    #[test]
    fn a_refused_build_forgets_its_destination_and_is_counted() {
        let c = new();
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 2, 0);
        // The same destination, with a flag nobody knows.
        put_bottom(&mut memory, 0x2000, DEST, 2, 0x4000_0000);
        // Not readable at all.
        let read = &mut memory.reader();
        c.build(read, 1, 0x1000, 0, 0);
        assert!(c.with_registry(|r| r.get(DEST).is_some()));
        c.build(read, 1, 0x2000, 0, 0);
        let counts = c.counts();
        assert_eq!(
            (counts.refused, counts.removed_unread, counts.unseen),
            (1, 1, 0)
        );
        assert!(c.with_registry(|r| r.is_empty()));

        c.build(read, 1, 0x7777_0000, 0, 0);
        c.build(read, 1, 0, 0, 0);
        let counts = c.counts();
        assert_eq!(
            (counts.refused, counts.removed_unread, counts.unseen),
            (3, 1, 2)
        );
        assert_eq!(counts.recorded_bottom, 1);

        // Post-build descriptions that cannot be read refuse the build too.
        c.build(read, 1, 0x1000, 2, 0x8888_0000);
        let counts = c.counts();
        assert_eq!((counts.refused, counts.recorded_bottom), (4, 1));
        assert!(c.with_registry(|r| r.is_empty()));
    }

    #[test]
    fn an_unpacking_call_brings_an_array_of_descriptions() {
        let c = new();
        let mut memory = Memory::default();
        // Three descriptions in a row; the middle one names boxes with a
        // flag that refuses it.
        let geometry = 0x5000;
        let mut array = desc(
            DEST,
            (1, build_flags::CONSOLE_DEHYDRATE, 1, 0),
            geometry,
            DEST + 0x100,
            0,
        );
        array.extend(desc(
            DEST + 0x1000,
            (1, 0, 1, 0),
            geometry + 0x100,
            DEST + 0x1100,
            0,
        ));
        array.extend(desc(
            DEST + 0x2000,
            (1, 0, 1, 0),
            geometry,
            DEST + 0x2100,
            0,
        ));
        memory.put(0x1000, array);
        memory.put(geometry, triangles(1, &some_triangles(1)));
        memory.put(
            geometry + 0x100,
            boxes(
                8,
                &Boxes {
                    count: 4,
                    address: 0x6000,
                    stride: 24,
                },
            ),
        );
        let read = &mut memory.reader();
        // A success that is not zero is a success.
        c.rehydrate(read, 0x44, 3, 0x1000, 1);
        let counts = c.counts();
        assert_eq!(
            (counts.rehydrate_calls, counts.rehydrate_descriptions),
            (1, 3)
        );
        assert_eq!(
            (counts.recorded_bottom, counts.refused, counts.packed),
            (2, 1, 1)
        );
        assert_eq!((counts.failed_calls, counts.failed_descriptions), (0, 0));
        c.with_registry(|r| {
            let destinations: Vec<u64> = r.iter().map(|(at, _)| at).collect();
            assert_eq!(destinations, [DEST, DEST + 0x2000]);
            let entry = r.get(DEST).unwrap();
            assert_eq!((entry.origin, entry.list), (Origin::Rehydrate, 0x44));
            assert_eq!(entry.description.source, DEST + 0x100);
        });

        // Null, and more than are read: counted, nothing recorded.
        c.rehydrate(read, 0, 2, 0, 0);
        c.rehydrate(read, 0, BATCH_LIMIT + 1, 0x1000, 0);
        let counts = c.counts();
        assert_eq!(counts.rehydrate_calls, 3);
        assert_eq!(counts.refused, 1 + 2 + u64::from(BATCH_LIMIT) + 1);
        assert_eq!(counts.unseen, 2 + u64::from(BATCH_LIMIT) + 1);
        assert_eq!(counts.recorded_bottom, 2);
    }

    #[test]
    fn an_unpacking_call_of_exactly_the_most_descriptions_is_read() {
        let c = new();
        let mut memory = Memory::default();
        // Empty bottom levels, each for a destination of its own.
        let array: Vec<u8> = (0..u64::from(BATCH_LIMIT))
            .flat_map(|n| desc(DEST + n * 0x100, (1, 0, 0, 0), 0, 0, 0))
            .collect();
        memory.put(0x1000, array);
        c.rehydrate(&mut memory.reader(), 1, BATCH_LIMIT, 0x1000, 0);
        let counts = c.counts();
        assert_eq!(counts.recorded_bottom, u64::from(BATCH_LIMIT));
        assert_eq!((counts.refused, counts.unseen, counts.full), (0, 0, 0));
        assert_eq!(c.with_registry(|r| r.len()), BATCH_LIMIT as usize);
    }

    #[test]
    fn a_failed_unpacking_call_records_nothing_and_forgets_its_destinations() {
        let c = new();
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 2, 0);
        put_bottom(&mut memory, 0x2000, DEST + 0x1000, 1, 0);
        // Two descriptions in a row for the two destinations above, then
        // one whose memory ends before its destination.
        let mut array = desc(DEST, (1, 0, 0, 0), 0, 0, 0);
        array.extend(desc(DEST + 0x1000, (1, 0, 0, 0), 0, 0, 0));
        memory.put(0x3000, array);
        let read = &mut memory.reader();
        c.build(read, 1, 0x1000, 0, 0);
        c.build(read, 1, 0x2000, 0, 0);
        c.rehydrate(read, 1, 0, 0, 0);
        assert_eq!(c.with_registry(|r| r.len()), 2);

        // E_INVALIDARG for three descriptions: the two entries go, and the
        // third's destination is not known.
        let invalid = 0x8007_0057_u32 as i32;
        c.rehydrate(read, 9, 3, 0x3000, invalid);
        let counts = c.counts();
        assert_eq!(
            (counts.rehydrate_calls, counts.rehydrate_descriptions),
            (2, 3)
        );
        assert_eq!((counts.failed_calls, counts.failed_descriptions), (1, 3));
        assert_eq!(
            (
                counts.recorded_bottom,
                counts.refused,
                counts.removed_unread
            ),
            (2, 0, 2)
        );
        assert_eq!(counts.unseen, 1);
        assert!(c.with_registry(|r| r.is_empty()));

        // Failed with more descriptions than are read: counted as failed
        // and as unseen, and still nothing read.
        c.rehydrate(read, 9, BATCH_LIMIT + 1, 0x3000, invalid);
        let counts = c.counts();
        assert_eq!(
            (counts.failed_calls, counts.failed_descriptions),
            (2, 3 + u64::from(BATCH_LIMIT) + 1)
        );
        assert_eq!(counts.unseen, 1 + u64::from(BATCH_LIMIT) + 1);
    }

    #[test]
    fn a_call_that_does_not_return_is_counted_and_records_nothing() {
        let c = new();
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 2, 0);
        let read = &mut memory.reader();
        c.entered();
        c.build(read, 1, 0x1000, 0, 0);
        assert_eq!(c.counts().unreturned(), 0);
        // The builder throws out of the next two: the tap never shows them.
        c.entered();
        c.entered();
        let counts = c.counts();
        assert_eq!((counts.entered, counts.returned), (3, 1));
        assert_eq!(counts.unreturned(), 2);
        assert_eq!(counts.build_calls, 1);
        // What was there before is as it was.
        assert_eq!(geometries(&c, DEST), Some(2));
    }

    #[test]
    fn batched_builds_are_counted_as_unseen_and_never_recorded() {
        let c = new();
        let mut memory = Memory::default();
        // Even bytes that happen to read as a description are only logged.
        put_bottom(&mut memory, 0x1000, DEST, 1, 0);
        let read = &mut memory.reader();
        c.batched(read, 5, 7, 0x1000);
        c.batched(read, 5, 2, 0);
        let counts = c.counts();
        assert_eq!(
            (
                counts.batched_calls,
                counts.batched_descriptions,
                counts.unseen
            ),
            (2, 9, 9)
        );
        assert_eq!(
            (counts.recorded_bottom, counts.recorded_top, counts.refused),
            (0, 0, 0)
        );
        assert!(c.with_registry(|r| r.is_empty()));

        // Without `dump`, nothing is read at all.
        let gauge = Box::leak(Box::new(Gauge::new()));
        let quiet = Capture::new(Limits::DEFAULT, Bounds::DEFAULT, gauge, false, None);
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 1, 0);
        quiet.batched(&mut memory.reader(), 5, 7, 0x1000);
        assert!(memory.reads.is_empty());
    }

    /// The review's case: the registry knows the destination and not the
    /// source (built by a call that is not read). Taking the known address
    /// for the source would hand the old description to the source's
    /// address and keep the overwritten one.
    #[test]
    fn an_undecided_copy_is_not_applied_and_both_addresses_are_forgotten() {
        let c = new();
        let (a, b, d) = (DEST, DEST + 0x1000, DEST + 0x2000);
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, b, 2, 0);
        put_bottom(&mut memory, 0x2000, a, 5, 0);
        put_bottom(&mut memory, 0x3000, d, 7, 0);
        let read = &mut memory.reader();
        c.build(read, 1, 0x1000, 0, 0);
        // `a` is built by a batched call, which is not read.
        c.batched(read, 1, 1, 0);
        // PC's order: to `b`, from `a`.
        c.copy(1, b, a, COPY_APPLIED);
        let counts = c.counts();
        assert_eq!((counts.copies_applied, counts.copies_undecided), (0, 1));
        assert_eq!(
            (counts.copies_first_known, counts.copies_second_known),
            (1, 0)
        );
        // Known on the wrong side, and not counted as deciding: a
        // structure had been made where nothing was read.
        assert_eq!(
            (counts.copies_first_sure, counts.copies_second_sure),
            (0, 0)
        );
        assert_eq!((counts.unseen, counts.unseen_copies), (2, 1));
        assert_eq!(counts.removed_unread, 1);
        assert!(c.with_registry(|r| r.is_empty()));

        // Both known: neither description moves to the other address.
        c.build(read, 1, 0x2000, 0, 0);
        c.build(read, 1, 0x3000, 0, 0);
        c.copy(1, a, d, COPY_APPLIED);
        let counts = c.counts();
        assert_eq!((counts.copies_applied, counts.copies_undecided), (0, 2));
        assert_eq!(counts.copies_both_known, 1);
        assert_eq!((counts.unseen, counts.unseen_copies), (3, 2));
        assert_eq!((geometries(&c, a), geometries(&c, d)), (None, None));
        assert_eq!(counts.copy_calls, 2);
    }

    #[test]
    fn which_address_of_a_copy_is_known_is_counted_whatever_the_registry_forgot() {
        let c = new();
        let (a, b, d, e) = (DEST, DEST + 0x1000, DEST + 0x2000, DEST + 0x3000);
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, a, 1, 0);
        put_bottom(&mut memory, 0x2000, d, 1, 0);
        let read = &mut memory.reader();
        c.build(read, 1, 0x1000, 0, 0);
        c.build(read, 1, 0x2000, 0, 0);
        // To a new address from `a`; then again from `a`, whose entry the
        // first copy took from the registry, to an address built at before.
        c.copy(1, b, a, COPY_APPLIED);
        assert_eq!(geometries(&c, a), None);
        c.copy(1, d, a, COPY_APPLIED);
        let counts = c.counts();
        assert_eq!(
            (counts.copies_first_known, counts.copies_second_known),
            (0, 1)
        );
        assert_eq!(
            (counts.copies_first_sure, counts.copies_second_sure),
            (0, 1)
        );
        assert_eq!(
            (counts.copies_both_known, counts.copies_neither_known),
            (1, 0)
        );
        // A copy of a copy: its source is known as a structure, though no
        // description was ever read for it.
        c.copy(1, e, b, COPY_APPLIED);
        let counts = c.counts();
        assert_eq!(
            (counts.copies_first_sure, counts.copies_second_sure),
            (0, 2)
        );
        // Neither known.
        c.copy(1, DEST + 0x8000, DEST + 0x9000, COPY_APPLIED);
        assert_eq!(c.counts().copies_neither_known, 1);

        // Memory that was remapped holds no structure any more.
        c.memory_remapped(a, 0x1000);
        c.copy(1, DEST + 0xa000, a, COPY_APPLIED);
        assert_eq!(c.counts().copies_neither_known, 2);
        assert_eq!(c.counts().copies_second_sure, 2);
    }

    #[test]
    fn with_the_order_decided_a_copy_gives_the_destination_the_sources_description() {
        let c = ordered(CopyOrder::DestinationFirst);
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 2, 0);
        put_bottom(&mut memory, 0x2000, DEST + 0x1000, 1, 0);
        let read = &mut memory.reader();
        c.build(read, 1, 0x1000, 0, 0);
        c.build(read, 1, 0x2000, 0, 0);

        // The destination first, the source second.
        c.copy(9, DEST + 0x4000, DEST, COPY_APPLIED);
        let counts = c.counts();
        assert_eq!((counts.copies_applied, counts.copies_undecided), (1, 0));
        assert_eq!(counts.unseen, 0);
        c.with_registry(|r| {
            let copy = r.get(DEST + 0x4000).unwrap();
            assert_eq!(copy.list, 9);
            assert!(matches!(copy.origin, Origin::Copy { from: DEST, .. }));
            assert!(r.get(DEST).is_some());
        });
        assert_eq!(geometries(&c, DEST + 0x4000), Some(2));

        // Over a destination that had another description.
        c.copy(9, DEST + 0x4000, DEST + 0x1000, COPY_APPLIED);
        assert_eq!(c.counts().copies_applied, 2);
        c.with_registry(|r| {
            let copy = r.get(DEST + 0x4000).unwrap();
            assert!(matches!(copy.origin, Origin::Copy { from, .. } if from == DEST + 0x1000));
            assert_eq!(copy.generation, 1);
        });
        assert_eq!(geometries(&c, DEST + 0x4000), Some(1));

        // From a structure the registry does not know: the destination's
        // entry goes, and the copy is one more structure unseen.
        c.copy(9, DEST + 0x4000, DEST + 0x7000, COPY_APPLIED);
        let counts = c.counts();
        assert_eq!((counts.copies_applied, counts.unseen), (2, 1));
        assert_eq!((counts.unseen_copies, counts.removed_unread), (1, 1));
        assert_eq!(geometries(&c, DEST + 0x4000), None);
        assert_eq!(geometries(&c, DEST), Some(2));

        // The other order, the same arguments the other way round.
        let c = ordered(CopyOrder::SourceFirst);
        c.build(read, 1, 0x1000, 0, 0);
        c.copy(9, DEST, DEST + 0x4000, COPY_APPLIED);
        assert_eq!(c.counts().copies_applied, 1);
        assert_eq!(geometries(&c, DEST + 0x4000), Some(2));
    }

    /// On PC a clone (0) and a deserialisation (4) write a structure at the
    /// destination too, and the other modes write something else there.
    #[test]
    fn a_copy_in_another_mode_leaves_no_entry_for_what_it_wrote_over() {
        for mode in [0, 2, 3, 4, 77] {
            // Undecided: either address may be the one written.
            let c = new();
            let mut memory = Memory::default();
            put_bottom(&mut memory, 0x1000, DEST, 2, 0);
            put_bottom(&mut memory, 0x2000, DEST + 0x1000, 1, 0);
            let read = &mut memory.reader();
            c.build(read, 1, 0x1000, 0, 0);
            c.build(read, 1, 0x2000, 0, 0);
            c.copy(9, DEST, DEST + 0x1000, mode);
            let counts = c.counts();
            assert_eq!(
                (counts.copies_other_mode, counts.copies_undecided),
                (1, 0),
                "{mode}"
            );
            assert_eq!((counts.unseen, counts.unseen_copies), (1, 1), "{mode}");
            assert_eq!(counts.removed_unread, 2, "{mode}");
            assert!(c.with_registry(|r| r.is_empty()), "{mode}");

            // Decided: the destination's entry goes, the source's stays.
            let c = ordered(CopyOrder::DestinationFirst);
            c.build(read, 1, 0x1000, 0, 0);
            c.build(read, 1, 0x2000, 0, 0);
            c.copy(9, DEST, DEST + 0x1000, mode);
            let counts = c.counts();
            assert_eq!(
                (counts.copies_other_mode, counts.copies_applied),
                (1, 0),
                "{mode}"
            );
            assert_eq!((counts.unseen, counts.removed_unread), (1, 1), "{mode}");
            assert_eq!(
                (geometries(&c, DEST), geometries(&c, DEST + 0x1000)),
                (None, Some(1)),
                "{mode}"
            );
        }
    }

    #[test]
    fn remapped_memory_takes_its_structures_along() {
        let c = new();
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 1, 0);
        put_bottom(&mut memory, 0x2000, DEST + 0x1_0000, 1, 0);
        let read = &mut memory.reader();
        c.build(read, 1, 0x1000, 0, 0);
        c.build(read, 1, 0x2000, 0, 0);
        c.memory_remapped(DEST + 0x1_0000, 0x1_0000);
        assert_eq!(c.counts().removed_by_remap, 1);
        assert!(c.with_registry(|r| r.get(DEST).is_some() && r.get(DEST + 0x1_0000).is_none()));
    }

    #[test]
    fn the_addresses_structures_were_made_at_are_bounded_and_counted() {
        let bounds = Bounds {
            entries: 2,
            bytes: u64::MAX,
        };
        let (c, gauge) = with(bounds, None);
        let mut memory = Memory::default();
        for n in 0..3 {
            put_bottom(&mut memory, 0x1000 * (n + 1), DEST + n * 0x1_0000, 1, 0);
        }
        let read = &mut memory.reader();
        for n in 0..3 {
            c.build(read, 1, 0x1000 * (n + 1), 0, 0);
            // Again: an address is kept once.
            c.build(read, 1, 0x1000 * (n + 1), 0, 0);
        }
        let counts = c.counts();
        // The third had no room, in the registry or among the addresses.
        assert_eq!((counts.full, counts.made_dropped), (2, 2));
        let entries = c.with_registry(|r| r.bytes());
        assert_eq!(gauge.now(), entries + 2 * MADE_BYTES);
        // With an address not kept, no copy counts as deciding.
        c.copy(1, DEST + 0x9_0000, DEST, COPY_APPLIED);
        let counts = c.counts();
        assert_eq!(
            (counts.copies_second_known, counts.copies_second_sure),
            (1, 0)
        );

        c.memory_remapped(DEST, 0x2_0000);
        assert_eq!(gauge.now(), 0);
        drop(c);
        assert_eq!((gauge.now(), gauge.underflows()), (0, 0));
    }

    #[test]
    fn size_queries_are_counted_by_level_and_stream() {
        let c = new();
        let mut memory = Memory::default();
        let top = desc(DEST, (0, 0, 4, 0), 0x9000, 0, 0);
        let bottom = desc(DEST, (1, 0, 4, 0), 0x9000, 0, 0);
        memory.put(0x1000, top[8..].to_vec());
        memory.put(0x2000, bottom[8..].to_vec());
        {
            let read = &mut memory.reader();
            c.size_query(read, 0x1000, 0);
            c.size_query(read, 0x2000, 0x5000);
            c.size_query(read, 0x3000, 0);
        }
        let counts = c.counts();
        assert_eq!(
            (
                counts.size_queries,
                counts.size_queries_top,
                counts.size_queries_with_stream
            ),
            (3, 1, 1)
        );
        // Only the inputs' start was read, never the arrays they name.
        assert!(memory.reads.iter().all(|&(_, len)| len == 0x18));
    }

    #[test]
    fn only_what_a_description_names_is_read() {
        let c = new();
        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 3, 0);
        put_top(&mut memory, 0x3000, DEST + 0x8000, &[DEST, DEST]);
        memory.put(0x4000, words(&[(8, 0x9000), (4, 0), (4, 0)]));
        // Data the descriptions point at: present, and never to be read.
        memory.put(0x41_0000_0000, vec![0xee; 0x4000]);
        memory.put(0x42_0000_0000, vec![0xee; 0x4000]);
        memory.put(DEST, vec![0xee; 0x100]);
        {
            let read = &mut memory.reader();
            c.build(read, 1, 0x1000, 1, 0x4000);
            c.build(read, 1, 0x3000, 0, 0);
        }
        assert_eq!(c.counts().refused, 0);
        let named = [
            (0x1000, 0x30),
            (0x1100, 3 * 0x38),
            (0x4000, 0x10),
            (0x3000, 0x30),
            (0x3100, 2 * 0x40),
        ];
        assert_eq!(memory.reads_outside(&named), []);
    }

    #[test]
    fn instance_flags_and_vertex_formats_dxr_does_not_have_are_counted() {
        let c = new();
        let mut memory = Memory::default();
        // Three instances: DXR's flags, then two with a bit outside them.
        let instances = [0x0f, 0x10, 0x8f].map(|flags| Instance {
            flags,
            ..some_instance(0, DEST)
        });
        memory.put(0x1000, desc(DEST + 0x8000, (0, 0, 3, 0), 0x1100, 0, 0));
        memory.put(0x1100, instances.iter().flat_map(instance_image).collect());
        // Three triangle sets: a format DXR takes, then two it does not
        // (four 32-bit floats, and none).
        let sets = [6, 2, 0].map(|vertex_format| Triangles {
            vertex_format,
            ..some_triangles(0)
        });
        memory.put(0x2000, desc(DEST, (1, 0, 3, 0), 0x2100, 0, 0));
        memory.put(0x2100, sets.iter().flat_map(|t| triangles(1, t)).collect());
        let read = &mut memory.reader();
        c.build(read, 1, 0x1000, 0, 0);
        c.build(read, 1, 0x2000, 0, 0);
        let counts = c.counts();
        assert_eq!((counts.instances, counts.other_instance_flags), (3, 2));
        assert_eq!(
            (counts.triangle_geometries, counts.other_vertex_formats),
            (3, 2)
        );
        assert_eq!(counts.refused, 0);
    }

    /// What a console geometry layout other than PC's would show as.
    #[test]
    fn geometry_that_does_not_look_like_geometry_is_counted() {
        let good = Triangles {
            transform: 0,
            index_format: IndexFormat::U16,
            vertex_format: 6,
            index_count: 36,
            vertex_count: 24,
            index_buffer: 0x100,
            vertex_buffer: 0x200,
            vertex_stride: 12,
        };
        let unindexed = Triangles {
            index_format: IndexFormat::None,
            index_count: 0,
            index_buffer: 0,
            ..good
        };
        let odd = |t: Triangles| oddities(&GeometryKind::Triangles(t));
        assert_eq!(odd(good), [false; 3]);
        assert_eq!(odd(unindexed), [false; 3]);
        // An empty set is not odd, whatever its stride and buffers.
        let empty = Triangles {
            index_count: 0,
            vertex_count: 0,
            vertex_stride: 0,
            vertex_buffer: 0,
            ..good
        };
        assert_eq!(odd(empty), [false; 3]);

        let count = [true, false, false];
        assert_eq!(
            odd(Triangles {
                index_count: 37,
                ..good
            }),
            count
        );
        assert_eq!(
            odd(Triangles {
                vertex_count: 25,
                ..unindexed
            }),
            count
        );
        // Indexed: the vertices need not come in threes.
        assert_eq!(
            odd(Triangles {
                vertex_count: 25,
                ..good
            }),
            [false; 3]
        );

        let stride = [false, true, false];
        assert_eq!(
            odd(Triangles {
                vertex_stride: 0,
                ..good
            }),
            stride
        );
        assert_eq!(
            odd(Triangles {
                vertex_stride: STRIDE_MOST + 1,
                ..good
            }),
            stride
        );
        assert_eq!(
            odd(Triangles {
                vertex_stride: STRIDE_MOST,
                ..good
            }),
            [false; 3]
        );

        let buffer = [false, false, true];
        assert_eq!(
            odd(Triangles {
                vertex_buffer: 0,
                ..good
            }),
            buffer
        );
        assert_eq!(
            odd(Triangles {
                index_buffer: 0,
                ..good
            }),
            buffer
        );
        assert_eq!(
            odd(Triangles {
                index_buffer: 0x100,
                ..unindexed
            }),
            buffer
        );
        assert_eq!(
            odd(Triangles {
                index_count: 3,
                ..unindexed
            }),
            buffer
        );

        let boxed = |count, address, stride| {
            oddities(&GeometryKind::Boxes(Boxes {
                count,
                address,
                stride,
            }))
        };
        assert_eq!(boxed(4, 0x300, 24), [false; 3]);
        assert_eq!(boxed(0, 0, 0), [false; 3]);
        assert_eq!(boxed(4, 0x300, 0), [false; 3]);
        assert_eq!(boxed(4, 0x300, 23), stride);
        assert_eq!(boxed(4, 0x300, STRIDE_MOST + 8), stride);
        assert_eq!(boxed(4, 0, 24), buffer);

        // Counted where descriptions are recorded: one geometry of each
        // kind of oddity, one with two, and a good one.
        let c = new();
        let mut memory = Memory::default();
        let sets = [
            good,
            Triangles {
                index_count: 37,
                ..good
            },
            Triangles {
                vertex_stride: 0,
                ..good
            },
            Triangles {
                vertex_buffer: 0,
                ..good
            },
            Triangles {
                index_count: 35,
                vertex_buffer: 0,
                ..good
            },
        ];
        memory.put(0x2000, desc(DEST, (1, 0, 5, 0), 0x2100, 0, 0));
        memory.put(0x2100, sets.iter().flat_map(|t| triangles(1, t)).collect());
        c.build(&mut memory.reader(), 1, 0x2000, 0, 0);
        let counts = c.counts();
        assert_eq!(
            (counts.odd_counts, counts.odd_strides, counts.odd_buffers),
            (2, 1, 2)
        );
        assert_eq!((counts.index_u16, counts.recorded_bottom), (5, 1));
        // Odd, and recorded as read all the same: it is the title's.
        assert_eq!(geometries(&c, DEST), Some(5));
    }

    /// The counts the last totals line showed.
    fn logged(c: &Capture) -> Option<Counts> {
        c.state().logged.map(|(_, counts)| counts)
    }

    #[test]
    fn the_totals_follow_a_burst_of_calls_and_its_end() {
        let c = new();
        // Nothing called: nothing to log, by the timer or at exit.
        c.tick();
        assert!(!c.flush());
        assert_eq!(logged(&c), None);

        let mut memory = Memory::default();
        put_bottom(&mut memory, 0x1000, DEST, 1, 0);
        let read = &mut memory.reader();
        // A burst: logged at the first call and at each power of two.
        for _ in 0..3000 {
            c.build(read, 1, 0x1000, 0, 0);
        }
        assert_eq!(logged(&c).unwrap().build_calls, 2048);
        // The timer: nothing while calls arrived since its last look, then
        // the totals once they have stopped.
        c.tick();
        assert_eq!(logged(&c).unwrap().build_calls, 2048);
        c.tick();
        assert_eq!(logged(&c), Some(c.counts()));
        assert_eq!(c.counts().build_calls, 3000);
        // Nothing new: nothing more.
        assert!(!c.flush());

        // Copies and size queries alone are logged too.
        let c = new();
        for n in 0..500 {
            c.size_query(read, 0, 0);
            c.copy(1, DEST + n * 0x100, DEST + 0x10_0000, COPY_APPLIED);
        }
        let last = logged(&c).unwrap();
        assert_eq!((last.size_queries, last.copy_calls), (256, 256));
        // Where the process ends: the rest, once.
        assert!(c.flush());
        let last = logged(&c).unwrap();
        assert_eq!((last.size_queries, last.copy_calls), (500, 500));
        assert!(!c.flush());

        // A call that did not return is something to log as well.
        let c = new();
        c.entered();
        assert!(c.flush());
        assert_eq!(logged(&c).unwrap().unreturned(), 1);
    }

    #[test]
    fn a_flush_does_not_wait_for_a_capture_in_use() {
        let c = new();
        c.entered();
        let held = c.state();
        // The thread that ends the process may be inside a tap.
        assert!(!c.flush());
        drop(held);
        assert!(c.flush());
    }

    #[test]
    fn notes_are_logged_once_and_bounded() {
        let c = new();
        let mut state = c.state();
        assert!(state.first(Note::Batched));
        assert!(!state.first(Note::Batched));
        for value in 0..NOTES as u32 + 10 {
            state.first(Note::VertexFormat(value));
        }
        assert_eq!(state.notes.len(), NOTES);
        assert_eq!(state.counts.notes_dropped, 11);
        let refusal = Refusal::Unreadable(What::Description);
        assert!(!state.first(Note::Refused(Via::Build, refusal)));
    }

    #[test]
    fn title_memory_reads_nothing_without_a_host() {
        // On Windows the reader is the kernel's; elsewhere there is no
        // title, and the reader says so instead of touching the address.
        #[cfg(not(windows))]
        assert!(!title_memory()(0x1000, &mut [0; 4]));
        let _ = title_memory();
    }
}
