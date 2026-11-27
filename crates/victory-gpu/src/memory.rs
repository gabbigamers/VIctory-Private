//! Title memory at raw GPU addresses (decision 0005, the unified-memory
//! item; the contract is `victory_shader::emit`'s "Title memory").
//! Translated shaders that read or write memory at addresses they compute
//! look each address up in a page table: per 64 KiB page below 2^40, the
//! heap index of a pair of raw views (SRV, UAV) of the host copy holding
//! the page (`crate::shadow`) and the low dword of that copy's title
//! address. The copies are the ones draws and dispatches use through views,
//! so what one shader writes through a view another reads at its address,
//! within a submission too.
//!
//! Each view heap has one page table, which every list using title memory
//! with that heap bound reads, and each host copy has a pair of views in
//! slots of the heap kept for it while it lives ([`HostHeap::pair`], past
//! the per-draw dynamic slots). At each submission with such a list the
//! table is rewritten if copies came or went since, and every copy it names
//! is brought up to date (page by page, as every refresh; once per
//! submission until a step writes title memory). Shaders mark the pages
//! they write in their list's bitmap; after the submission each marked page
//! goes back into title memory ([`crate::shadow::Shadow::copy_back_page`])
//! and the bitmap is cleared.
//!
//! Cost per submission with such a list: one change check of every host
//! copy (the refresh), copies of the pages the title wrote, a compare of
//! each 64 KiB page shaders wrote, and the table's rewrite when copies came
//! or went (two views per new copy); per list a bitmap of 256 KiB.
//!
//! Memory no copy holds reads 0 and takes no writes (null views,
//! bounds-checked); shaders mark such pages (or 256 MiB regions with no
//! block) in the bitmap and [`log_frame`] counts them.
//!
//! A copy exists only for memory a draw, a view or a root argument named:
//! nothing here makes one for a page a shader reached through a pointer it
//! read. Such a page is missed again whenever a program reaches it, and
//! each miss withholds what programs using title memory wrote
//! ([`withholding`]). A list that missed does not run again: what a program
//! the title runs once wrote in it is lost, and the lists whose writes were
//! withheld are counted by pipeline. Unless switched off: each missed page
//! is looked at once the frame has a moment (is it title memory, is it in a
//! title buffer, does a copy hold it; a few system calls for at most
//! `crate::paging::ASK_PER_FRAME` pages a frame, timed in the misses line;
//! `VICTORY_MEMORY_MISS_REASONS=0` turns it off) and counted by what was
//! found, and a 256 MiB region with no copy in it gets an empty block so
//! that misses there name their page (`VICTORY_MEMORY_PROBES=0` turns it
//! off; the shader reads the same null views). `crate::paging` holds that
//! bookkeeping and its tests. With `VICTORY_MEMORY_PAGING=1` a missed page
//! of title memory also gets a copy of its own for the submissions that
//! follow: see `crate::paging` for the rules.
//! shortcut: a page two copies share without either holding all of it is
//! the larger one's, and what lies past that copy's end in the page reads 0
//! without a mark (so no miss is counted and no page on demand is made for
//! it). The pages a table names in part are counted when it is written and
//! the frame line gives the figure.
//! shortcut: an access is served by one lookup, so the part of it that runs
//! past the end of the copy holding its first byte reads 0 and is not
//! marked (a write there marks the page of its last byte).
//! shortcut: a table names copies in at most [`CAPACITY`] 256 MiB regions;
//! past that, the smallest copies read as missing (logged once).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
use windows::core::Interface;

use crate::descriptor::host::HostHeap;
use crate::host::HostList;
#[cfg(doc)]
use crate::paging::CAPACITY;
use crate::paging::{BITMAP_BYTES, BITMAP_WORDS, Facts, GivenUp, Named, RUN_BYTES, TABLE_DWORDS, Unmade, write_table};
use crate::shadow::Shadow;

/// Maximum number of dirty-page bitmaps kept in [`BITMAP_POOL`].
const BITMAP_POOL_CAP: usize = 64;
/// The key of our own view heap ([`heap`]) among the title's.
pub(crate) const OURS: usize = usize::MAX;

/// A list's use of title memory with a view heap bound.
pub(crate) struct Map {
    /// The heap's page table's host GPU address (the root SRV).
    pub(crate) table: u64,
    /// The list's page bitmap's host GPU address (the root UAV).
    pub(crate) dirty: u64,
    /// Ranges that go back byte for byte instead (root UAVs, see
    /// [`Map::precise`]).
    precise: Arc<Mutex<Vec<(u64, u64)>>>,
    /// The list's draws and dispatches using title memory, by pipeline
    /// ([`Map::used`]).
    uses: Arc<Mutex<std::collections::BTreeMap<String, u64>>>,
}

impl Map {
    /// Shaders write `[address, address + len)` through a root UAV, which
    /// goes back by its bytes before and after the run
    /// ([`Shadow::gpu_writes`]): the marked pages it touches are left to
    /// that, where the table named that copy or another of the title's for
    /// them (`crate::paging::take_marks`). A whole page from the copy would
    /// put back what it held at the submission over what the title (or
    /// DirectStorage, through physical pages no watch sees) wrote there
    /// since.
    /// shortcut: a marked page partly in such a range loses what shaders
    /// wrote in its other part.
    pub(crate) fn precise(&self, address: u64, len: u64) {
        self.precise
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((address, len));
    }

    /// A draw or dispatch of `pipeline` in the list uses title memory: if
    /// the list's writes through the table are withheld, it is counted
    /// under that name (`crate::paging::State::note_withheld`).
    pub(crate) fn used(&self, pipeline: &str) {
        let mut uses = self.uses.lock().unwrap_or_else(|e| e.into_inner());
        match uses.get_mut(pipeline) {
            Some(n) => *n += 1,
            None => {
                uses.insert(pipeline.to_owned(), 1);
            }
        }
    }
}

/// What a heap's page table names: the copies, the level-1 index of each
/// block (block 0 is the null one), and the copy each entry names (or
/// `crate::paging::NO_OWNER`).
#[derive(Default)]
struct Layout {
    copies: Vec<Arc<Shadow>>,
    /// Each copy's bit in the change tracker (`crate::watch::clean_bit`).
    bits: Vec<Option<u32>>,
    blocks: Vec<usize>,
    owner: Vec<u32>,
}

/// A 256 KiB dirty-page bitmap buffer kept zeroed while in [`BITMAP_POOL`].
struct Bitmap {
    _resource: ID3D12Resource,
    data: usize,
    gpu: u64,
}

// SAFETY: `data` is the persistent mapping of `_resource`, owned exclusively by
// one `PooledBitmap` while bound or by `BITMAP_POOL` while idle; D3D12
// resources are free-threaded.
unsafe impl Send for Bitmap {}
// SAFETY: as above; `defer_missed` and `defer_after` access the mapping
// sequentially on the submitting thread after the submission's fence waits.
unsafe impl Sync for Bitmap {}

/// Reusable zeroed dirty-page bitmap buffers, capped at [`BITMAP_POOL_CAP`].
static BITMAP_POOL: Mutex<Vec<Bitmap>> = Mutex::new(Vec::new());

impl Bitmap {
    /// Pops a zeroed bitmap from [`BITMAP_POOL`] or allocates and zeroes a new one.
    fn acquire() -> Option<Self> {
        if let Some(bitmap) = BITMAP_POOL.lock().unwrap_or_else(|e| e.into_inner()).pop() {
            return Some(bitmap);
        }
        let (resource, data) = crate::host::host()?.shared_buffer(BITMAP_BYTES)?;
        crate::host::name(&resource, || "title memory written pages".into());
        // SAFETY: `data` is a newly mapped shared buffer of at least `BITMAP_WORDS` dwords.
        unsafe { std::ptr::write_bytes(data.cast::<u32>(), 0, BITMAP_WORDS) };
        // SAFETY: `resource` is a live D3D12 buffer.
        let gpu = unsafe { resource.GetGPUVirtualAddress() };
        Some(Self {
            _resource: resource,
            data: data as usize,
            gpu,
        })
    }
}

/// RAII guard keeping a [`Bitmap`] owned by a `HostList` until `HostList::reset`
/// or `Drop`, ensuring it is zeroed before returning to [`BITMAP_POOL`].
struct PooledBitmap {
    bitmap: Option<Bitmap>,
}

impl PooledBitmap {
    fn new(bitmap: Bitmap) -> Self {
        Self {
            bitmap: Some(bitmap),
        }
    }

    fn gpu(&self) -> u64 {
        self.bitmap.as_ref().expect("bitmap present until drop").gpu
    }

    fn data(&self) -> usize {
        self.bitmap
            .as_ref()
            .expect("bitmap present until drop")
            .data
    }
}

impl Drop for PooledBitmap {
    fn drop(&mut self) {
        let Some(bitmap) = self.bitmap.take() else {
            return;
        };
        // SAFETY: `PooledBitmap` drops only when `HostList::reset` or `Drop`
        // releases its deferred closures after any GPU submission has finished;
        // `bitmap.data` is mapped for `BITMAP_WORDS` dwords.
        let marks =
            unsafe { std::slice::from_raw_parts_mut(bitmap.data as *mut u32, BITMAP_WORDS) };
        // `defer_after` clears non-zero words after execution and un-executed
        // lists start clean; verify and clear any stray non-zero words before pooling.
        if let Some(first) = marks.iter().position(|&m| m != 0) {
            for mark in &mut marks[first..] {
                if *mark != 0 {
                    *mark = 0;
                }
            }
        }
        let overflow = {
            let mut pool = BITMAP_POOL.lock().unwrap_or_else(|e| e.into_inner());
            if pool.len() < BITMAP_POOL_CAP {
                pool.push(bitmap);
                None
            } else {
                Some(bitmap)
            }
        };
        drop(overflow);
    }
}

/// A view heap's page table, shared by every list using title memory with
/// the heap bound, and the view pairs of the copies it names.
struct Shared {
    /// The host heap it was made for (a heap made later at a released one's
    /// address is another).
    host: usize,
    table: ID3D12Resource,
    data: usize,
    gpu: u64,
    /// The pair of null views entries without a copy name.
    null: u32,
    /// Each copy's view pair (its first slot), by copy.
    pairs: HashMap<usize, u32>,
    /// The view pair of each run's buffer of pages held on demand
    /// ([`Paging`]), by buffer: one pair for all the pages in it.
    run_pairs: HashMap<usize, u32>,
    /// The copies' generation it names ([`crate::shadow::generation`]).
    generation: u64,
    layout: Arc<Layout>,
    /// The table's bytes, counted while this has it.
    _held: crate::account::Held,
}

// SAFETY: `data` is the persistent mapping of `table`, written only under
// the lock; D3D12 resources are free-threaded.
unsafe impl Send for Shared {}

/// Page tables by view heap (the title heap's address, or [`OURS`]).
static SHARED: Mutex<Vec<(usize, Shared)>> = Mutex::new(Vec::new());

fn shared() -> std::sync::MutexGuard<'static, Vec<(usize, Shared)>> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

/// View heap `key` was released: its page table goes with it (lists that
/// name it hold the table), and a heap made later at its address starts
/// with none of its state.
#[cfg(windows)]
pub(crate) fn release(key: usize) {
    shared().retain(|(k, _)| *k != key);
}

/// Whether view heap `key` has a page table.
#[cfg(test)]
pub(crate) fn has_table(key: usize) -> bool {
    shared().iter().any(|(k, _)| *k == key)
}

/// Runs `f` on the host heap behind view heap `key` while it cannot go
/// away; None if it is gone or has no host heap.
fn with_heap<R>(key: usize, f: impl FnOnce(&HostHeap) -> R) -> Option<R> {
    if key == OURS {
        return heap().map(f);
    }
    crate::descriptor::with_host(key, f)
}

/// Since the last frame line: draws and dispatches with shaders using title
/// memory (by pipeline), page tables written, pages copied back, pages and
/// 256 MiB regions missed (no copy), and the first missed addresses.
#[derive(Default)]
struct Stats {
    uses: std::collections::BTreeMap<String, u64>,
    tables: u64,
    written: u64,
    /// Pages shaders wrote that were not copied back: their list read
    /// memory with no copy, so what they computed is not the console's.
    withheld: u64,
    /// The same for ranges written through views (bytes).
    withheld_bytes: u64,
    missed_pages: u64,
    missed_regions: u64,
    first_missed: Vec<u64>,
}

static STATS: std::sync::Mutex<Option<Stats>> = std::sync::Mutex::new(None);

fn stats<R>(f: impl FnOnce(&mut Stats) -> R) -> R {
    f(STATS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default())
}

/// `bytes` of a range shaders wrote through a view did not go back into
/// title memory: their list read memory with no copy.
pub(crate) fn note_withheld(bytes: u64) {
    stats(|s| s.withheld_bytes += bytes);
}

/// A draw or dispatch of `pipeline` uses title memory.
pub(crate) fn note_use(pipeline: &str) {
    stats(|s| *s.uses.entry(pipeline.to_owned()).or_default() += 1);
}

/// Frames presented, and the last one in which a list using title memory
/// read or wrote memory with no copy ([`MISS_NEVER`] if none has).
static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MISSED_IN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(MISS_NEVER);
const MISS_NEVER: u64 = u64::MAX;

/// Whether a list using title memory in the submission being completed
/// read or wrote memory with no copy (completions run one at a time).
static SUBMISSION_MISSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A submission's lists ran: whether one using title memory read or wrote
/// memory with no copy.
pub(crate) fn note_submission(missed: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    SUBMISSION_MISSED.store(missed, Relaxed);
    if missed {
        MISSED_IN.store(FRAMES.load(Relaxed), Relaxed);
    }
}

/// Whether what lists using title memory wrote stays out of title memory
/// (in the host copies, for the GPU): some such list missed memory this
/// frame or the one before. A list that missed computed from zeros, and
/// the lists after it compute from its results in the host copies, the
/// BVH builder's write addresses among them, so withholding only the lists
/// that missed broke the title's own data (run drops1: an AABB's +inf/-inf
/// in a pointer the title followed). The frame before counts too: its
/// lists' results are read in this one, before this frame's first miss.
pub(crate) fn withholding() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    withheld_in(FRAMES.load(Relaxed), MISSED_IN.load(Relaxed))
}

fn withheld_in(frame: u64, missed: u64) -> bool {
    missed != MISS_NEVER && frame <= missed.saturating_add(1)
}

/// Title memory whose host copies hold what shaders wrote this frame and
/// the frame withheld ([`withholding`]; ranges).
static DROPPED: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Shaders wrote `[address, address + len)` of title memory in host copies
/// and the submission withheld it: the copies take title memory's bytes
/// again at their next refresh, from right after the submission if a list
/// of it missed (its results would feed the frame's later submissions:
/// garbage BVH nodes over meshlet buffers, run shards20), else from the
/// frame's end ([`retake_dropped`]).
pub(crate) fn drop_withheld(address: usize, len: usize) {
    if SUBMISSION_MISSED.load(std::sync::atomic::Ordering::Relaxed) {
        crate::watch::retake(address, len);
    } else {
        DROPPED.lock().unwrap_or_else(|e| e.into_inner()).push((address, len));
    }
}

/// At a frame's end: what the frame withheld leaves the host copies, so
/// that results computed from memory with no copy (garbage record arrays
/// and node pointers of the BVH builder) do not feed later frames whose
/// writes go back (a box16 node's f16 bounds over a title object, run
/// textures9). Each copy holding such memory takes it from title memory at
/// its next refresh, through its tracker.
fn retake_dropped() {
    let dropped = std::mem::take(&mut *DROPPED.lock().unwrap_or_else(|e| e.into_inner()));
    for (address, len) in dropped {
        crate::watch::retake(address, len);
    }
}

/// The host buffer of one run of pages held on demand
/// (`crate::paging::RUN_BYTES` of title memory, aligned): each page of the
/// run that is held lies in it at its own offset, so neighbouring pages lie
/// side by side.
struct Run {
    buffer: ID3D12Resource,
    data: usize,
    gpu: u64,
    /// Its bytes, counted from the run's first copy until its last is gone.
    _held: crate::account::Held,
}

// SAFETY: `data` is the persistent mapping of `buffer`, written through the
// copies made in it (`crate::shadow`) on the submitting thread; D3D12
// resources are free-threaded.
unsafe impl Send for Run {}

/// The device's side of `crate::paging`: the pages of title memory held
/// because a shader missed them. Owner of those pages' copies and of the
/// buffers they lie in: a copy lives from the submission that missed its
/// page (or the page before it in its run) until the page is given up
/// ([`give_up`]) and the last table naming it is rewritten; a buffer from
/// its run's first copy until its last copy is gone. What is decided
/// without the device is in `state`.
struct Paging {
    state: crate::paging::State,
    /// The copy of each held page, by page number.
    copies: std::collections::BTreeMap<u64, Arc<Shadow>>,
    /// The buffers, by run.
    runs: std::collections::BTreeMap<u64, Run>,
}

/// What `crate::paging::State` asks of the system and the device.
struct Device<'a> {
    copies: &'a mut std::collections::BTreeMap<u64, Arc<Shadow>>,
    runs: &'a mut std::collections::BTreeMap<u64, Run>,
}

impl crate::paging::Host for Device<'_> {
    fn facts(&mut self, at: u64) -> Facts {
        let (in_buffer, held, part) = crate::shadow::holding(at);
        let readable = !held && !part && readable(at);
        let tracked = readable && crate::watch::tracks(at as usize, 1 << 16);
        Facts {
            held,
            part,
            readable,
            tracked,
            in_buffer,
        }
    }

    /// The copy starts as changed whole, so its first refresh takes the
    /// page from title memory.
    fn make(&mut self, page: u64) -> Result<(), Unmade> {
        let (at, run) = (page << 16, crate::paging::run_of(page));
        let r = match self.runs.entry(run) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                // What was found of the page may be some frames old: asked
                // again before a buffer is made for it.
                if !crate::watch::tracks(at as usize, 1 << 16) {
                    return Err(Unmade::Untracked);
                }
                let (buffer, data) =
                    crate::host::host().and_then(|h| h.shared_buffer(RUN_BYTES)).ok_or(Unmade::NoMemory)?;
                crate::host::name(&buffer, || format!("title pages on demand {:#x}", run * RUN_BYTES));
                // SAFETY: a live buffer.
                let gpu = unsafe { buffer.GetGPUVirtualAddress() };
                e.insert(Run {
                    buffer,
                    data: data as usize,
                    gpu,
                    _held: crate::account::Held::new(crate::account::Owner::DemandPages, RUN_BYTES),
                })
            }
        };
        let offset = crate::paging::offset_in_run(page);
        let data = (r.data + offset as usize) as *mut u8;
        let copy = Shadow::page(r.buffer.clone(), data, r.gpu + offset, at, offset, page).ok_or(Unmade::Untracked)?;
        self.copies.insert(page, copy);
        Ok(())
    }

    fn free(&mut self, run: u64) {
        self.runs.remove(&run);
    }

    fn clear(&mut self, page: u64) {
        if let Some(r) = self.runs.get(&crate::paging::run_of(page)) {
            let at = (r.data + crate::paging::offset_in_run(page) as usize) as *mut u8;
            // SAFETY: a page's place inside the run's mapping. No copy that
            // a table in use names lies there (`crate::paging::Host::clear`):
            // only an access running past the end of the page before
            // reaches it, and reads 0 or what was there, both of which are
            // that shortcut's.
            unsafe { at.write_bytes(0, 1 << 16) };
        }
    }
}

static PAGING: Mutex<Option<Paging>> = Mutex::new(None);

/// Pages whose copies on demand were dropped: taken in at the next use of
/// [`PAGING`], which a drop must not wait for (its holder may be the one
/// dropping).
static GONE: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// A copy of page `page` (a page number) held on demand is gone
/// (`crate::shadow`'s drop).
pub(crate) fn page_gone(page: u64) {
    GONE.lock().unwrap_or_else(|e| e.into_inner()).push(page);
}

/// Runs `f` on the paging state. Inside the page tables' lock where both
/// are held, never the other way; `f` takes neither that nor a heap's.
fn paging<R>(f: impl FnOnce(&mut Paging) -> R) -> R {
    let mut guard = PAGING.lock().unwrap_or_else(|e| e.into_inner());
    let p = guard.get_or_insert_with(Paging::new);
    p.take_gone();
    f(p)
}

impl Paging {
    /// The state the switches ask for (read once).
    fn new() -> Self {
        let on = crate::switch::on("VICTORY_MEMORY_PAGING");
        let off = |name: &str| std::env::var_os(name).is_some_and(|v| v == "0");
        let (probe, look) = (!off("VICTORY_MEMORY_PROBES"), !off("VICTORY_MEMORY_MISS_REASONS"));
        let asked = std::env::var_os("VICTORY_MEMORY_PAGING_MB").map(|v| v.to_string_lossy().into_owned());
        let (megabytes, understood) = crate::paging::megabytes(asked.as_deref());
        if !understood {
            tracing::warn!(
                "VICTORY_MEMORY_PAGING_MB={} is not a number of megabytes above 0: the default of {megabytes} is used",
                asked.as_deref().unwrap_or_default()
            );
        }
        let state = crate::paging::State::new(on, probe, look, megabytes);
        if on {
            tracing::info!(
                "title memory: pages shaders miss get a host copy on demand (VICTORY_MEMORY_PAGING): at most {} \
                 buffers of 4 MiB ({:.0} MB of host memory), {} pages and {} new buffers a frame",
                state.limit(),
                crate::paging::worst_case_bytes(state.limit()) as f64 / 1e6,
                crate::paging::ADMIT_PER_FRAME,
                crate::paging::RUNS_PER_FRAME
            );
            if !look {
                tracing::warn!("VICTORY_MEMORY_MISS_REASONS=0 is ignored with VICTORY_MEMORY_PAGING: a page is looked at before it gets a copy");
            }
            if !probe {
                tracing::warn!("VICTORY_MEMORY_PROBES=0 with VICTORY_MEMORY_PAGING: no page in a 256 MiB region without a copy gets one");
            }
        }
        Paging {
            state,
            copies: Default::default(),
            runs: Default::default(),
        }
    }

    /// Copies that are gone leave their places zeroed, and a run with no
    /// copy left has its buffer freed: no table has views of it any more (a
    /// table drops them when it no longer names a page of the buffer, and a
    /// copy is gone only after every table naming it was rewritten).
    fn take_gone(&mut self) {
        let gone = std::mem::take(&mut *GONE.lock().unwrap_or_else(|e| e.into_inner()));
        let Paging { state, copies, runs } = self;
        for page in gone {
            state.gone(page, &mut Device { copies, runs });
        }
    }
}

/// Whether any of the 64 KiB page at `page` can be read. The title commits
/// by 4 KiB pieces, so the first byte of each is tried, through the kernel
/// (which refuses what is not mapped instead of faulting). Where the first
/// piece cannot be read the system is asked once whether the whole page is
/// free or reserved, which is the common case of a stray address and
/// answers it without fifteen more refused reads.
fn readable(page: u64) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut byte = [0u8; 1];
    let mut piece = |k: u64| crate::host::read_into((page + k * 4096) as *const u8, &mut byte);
    if piece(0) {
        return true;
    }
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let asked = unsafe { VirtualQuery(page as *const _, &mut info, size_of_val(&info)) } != 0;
    let end = (info.BaseAddress as u64).saturating_add(info.RegionSize as u64);
    if asked && info.State != MEM_COMMIT && end >= page + (1 << 16) {
        return false;
    }
    (1..16).any(piece)
}

/// After a list ran: its misses (`crate::paging::State::note_misses`, which
/// counts each by what it is, gives a region a block so that its next miss
/// names the page and, with the switch, gives a page that is title memory a
/// copy for the next submission's table to name). The miss stays one: this
/// list's writes are withheld as before, and nothing runs it again.
fn note_misses(m: &crate::paging::Marks) {
    let changed = paging(|p| {
        let Paging { state, copies, runs } = p;
        state.note_misses(m, &mut Device { copies, runs })
    });
    if changed {
        crate::shadow::changed();
    }
}

/// The copies of the pages held on demand, and the regions whose misses
/// are to name their page: what a page table names besides
/// [`crate::shadow::all`].
fn paged() -> (Vec<Arc<Shadow>>, Vec<usize>) {
    paging(|p| (p.copies.values().cloned().collect(), p.state.probes()))
}

/// Pages held on demand (page numbers) are given up, each for its reason:
/// another copy has them, or the title gave their memory up. Their copies
/// live until the tables naming them are rewritten (at each one's next
/// submission), and their run's buffer with them. Nothing is lost with a
/// copy: what shaders wrote in it went back to title memory, or was
/// withheld and dropped, when its submission ended.
fn give_up(pages: &[(u64, GivenUp)]) {
    let dropped: Vec<Arc<Shadow>> = paging(|p| {
        pages.iter().filter_map(|&(page, why)| p.state.give_up(page, why).then(|| p.copies.remove(&page)).flatten()).collect()
    });
    if !dropped.is_empty() {
        crate::shadow::changed();
    }
}

/// At a frame's end (`withheld`: its writes through title memory stayed out
/// of title memory): the misses are logged, by frame at first and then by
/// period, and some held pages are checked for memory the title gave up.
fn paging_frame(withheld: bool) {
    let kept = crate::shadow::take_kept_titles();
    let (line, sweep) = paging(|p| p.state.frame_end(withheld, kept));
    if let Some(line) = line {
        tracing::info!("{line}");
    }
    let vanished: Vec<(u64, GivenUp)> =
        sweep.into_iter().filter(|&page| !readable(page << 16)).map(|page| (page, GivenUp::Vanished)).collect();
    if !vanished.is_empty() {
        give_up(&vanished);
    }
}

/// Logs what title memory did since the last call, if anything: a line per
/// presented frame while shaders use it.
pub(crate) fn log_frame() {
    let withheld = withholding();
    FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    retake_dropped();
    paging_frame(withheld);
    let s = stats(std::mem::take);
    if s.uses.is_empty() && s.written + s.withheld + s.withheld_bytes + s.missed_pages + s.missed_regions == 0 {
        return;
    }
    let mut uses: Vec<(&String, &u64)> = s.uses.iter().collect();
    uses.sort_by(|a, b| b.1.cmp(a.1));
    let uses: Vec<String> = uses.iter().take(32).map(|(p, n)| format!("{n} {p}")).collect();
    let missed: Vec<String> = s.first_missed.iter().map(|a| format!("{a:#x}")).collect();
    // Pages the table written last names for a copy holding part of them:
    // the rest of such a page reads 0 with no miss to count, so a frame
    // without misses does not show that shaders found all they reached.
    let partial = paging(|p| p.state.partial());
    tracing::info!(
        "title memory this frame: {} draws/dispatches ({}), {} page tables, {} pages \
         copied back, {} withheld and {:.1} MB of views kept (their list read memory with no copy), missed {} pages \
         and {} 256 MiB regions with no copy (first: {}); {partial} pages are named for a copy holding part of \
         them (the rest of such a page reads 0 and is not counted as missed)",
        s.uses.values().sum::<u64>(),
        uses.join(", "),
        s.tables,
        s.written,
        s.withheld,
        s.withheld_bytes as f64 / 1e6,
        s.missed_pages,
        s.missed_regions,
        missed.join(" ")
    );
}

/// Our view heap for lists that bind none of the title's (dynamic slots
/// only).
pub(crate) fn heap() -> Option<&'static crate::descriptor::host::HostHeap> {
    static HEAP: std::sync::OnceLock<Option<crate::descriptor::host::HostHeap>> =
        std::sync::OnceLock::new();
    HEAP.get_or_init(|| crate::descriptor::host::HostHeap::new(false, 0))
        .as_ref()
}

impl Shared {
    /// A page table for `heap`, named nothing yet (written at the first
    /// submission that uses it).
    fn new(heap: &HostHeap) -> Option<Shared> {
        let bytes = TABLE_DWORDS as u64 * 4;
        let (table, data) = crate::host::host()?.upload_buffer(bytes)?;
        let held = crate::account::Held::new(crate::account::Owner::HeapTables, bytes);
        crate::host::name(&table, || "title memory page table".into());
        let null = heap.pair()?;
        heap.raw_pair_at(null, None, 0);
        // SAFETY: a live buffer.
        let gpu = unsafe { table.GetGPUVirtualAddress() };
        Some(Shared {
            host: std::ptr::from_ref(heap) as usize,
            table,
            data: data as usize,
            gpu,
            null,
            pairs: HashMap::new(),
            run_pairs: HashMap::new(),
            generation: u64::MAX,
            layout: Arc::default(),
            _held: held,
        })
    }

    /// Names every host copy there is now (`now`: their generation): view
    /// pairs made for new copies, those of copies gone made null and given
    /// back, the table rewritten. Only while no submission runs (the GPU
    /// reads the table and the views).
    fn rewrite(&mut self, heap: &HostHeap, now: u64) {
        let mut copies = crate::shadow::all();
        // Pages held on demand, and the regions whose misses are to name
        // their page.
        let (held, probes) = paged();
        copies.extend(held);
        let live: HashSet<usize> = copies.iter().map(|c| Arc::as_ptr(c) as usize).collect();
        let runs: HashSet<usize> = copies.iter().filter(|c| c.paged()).map(|c| c.buffer.as_raw() as usize).collect();
        for (pairs, live) in [(&mut self.pairs, &live), (&mut self.run_pairs, &runs)] {
            pairs.retain(|key, &mut slot| {
                let keep = live.contains(key);
                if !keep {
                    heap.raw_pair_at(slot, None, 0);
                    heap.free_pair(slot);
                }
                keep
            });
        }
        let (mut named, mut entries) = (Vec::with_capacity(copies.len()), Vec::with_capacity(copies.len()));
        for c in copies {
            // A page held on demand is seen through the views of its run's
            // whole buffer, as its neighbours in the run are.
            let (pairs, key, bytes) = if c.paged() {
                (&mut self.run_pairs, c.buffer.as_raw() as usize, RUN_BYTES)
            } else {
                (&mut self.pairs, Arc::as_ptr(&c) as usize, c.range().1.next_multiple_of(4))
            };
            let pair = match pairs.get(&key) {
                Some(&slot) => slot,
                None => {
                    let Some(slot) = heap.pair() else {
                        static ONCE: std::sync::Once = std::sync::Once::new();
                        ONCE.call_once(|| tracing::warn!("no descriptor slots for a title memory copy's views: it reads as missing"));
                        continue;
                    };
                    heap.raw_pair_at(slot, Some(&c.buffer), bytes);
                    pairs.insert(key, slot);
                    slot
                }
            };
            let (start, len) = c.range();
            entries.push(Named {
                start,
                len,
                pair,
                // Byte `offset(start)` of the view shows the copy's first:
                // for a page on demand the view starts at its run's start.
                view_start: start.wrapping_sub(c.offset(start)) as u32,
                yields: c.paged(),
            });
            named.push(c);
        }
        // SAFETY: `self.data` is the persistent mapping of `self.table`, with room
        // for `TABLE_DWORDS` dwords; no submission runs.
        let out = unsafe { std::slice::from_raw_parts_mut(self.data as *mut u32, TABLE_DWORDS) };
        let written = write_table(&entries, &probes, self.null, out);
        tracing::debug!(
            copies = entries.len(),
            blocks = written.blocks.len() - 1,
            dwords = written.dwords,
            partial = written.partial,
            cut = written.cut,
            "title memory page table"
        );
        // Pages on demand whose page another copy has are given up (the
        // table names that copy for them already).
        let superseded: Vec<(u64, GivenUp)> = written
            .superseded
            .iter()
            .map(|&(k, whole)| (entries[k].start >> 16, if whole { GivenUp::Whole } else { GivenUp::Part }))
            .collect();
        if !superseded.is_empty() {
            give_up(&superseded);
        }
        paging(|p| p.state.table(written.cut, written.partial));
        self.layout = Arc::new(Layout {
            bits: named.iter().map(|c| c.dirty_bit()).collect(),
            copies: named,
            blocks: written.blocks,
            owner: written.owner,
        });
        self.generation = now;
        stats(|s| s.tables += 1);
    }
}

/// The page table of view heap `key`, as the last submission using it
/// left it.
fn layout(key: usize) -> Arc<Layout> {
    shared()
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, s)| s.layout.clone())
        .unwrap_or_default()
}

/// At submission, for a list using title memory with view heap `key`
/// bound: the heap's page table names every host copy there is, each
/// brought up to date (`add` takes a copy's range, once per submission
/// until a step writes title memory).
pub(crate) fn prepare(key: usize, mut add: impl FnMut(&Arc<Shadow>, usize, usize)) {
    let layout = with_heap(key, |heap| {
        let mut shared = shared();
        let host = std::ptr::from_ref(heap) as usize;
        let (_, s) = shared.iter_mut().find(|(k, s)| *k == key && s.host == host)?;
        let now = crate::shadow::generation();
        if s.generation != now {
            s.rewrite(heap, now);
        }
        Some(s.layout.clone())
    })
    .flatten();
    // Copies the title has not written since they took it are passed by
    // without touching them (the table names every copy, ~4,700 in the world).
    for l in layout.iter() {
        for (c, bit) in l.copies.iter().zip(&l.bits) {
            if !crate::watch::clean_bit(*bit) {
                let (start, len) = c.range();
                add(c, start as usize, len as usize);
            }
        }
    }
}

/// `list` uses title memory with view heap `key` bound: the heap's page
/// table (made on first use) and a page bitmap of the list's, the table
/// brought up to date at each submission of the list, its written pages
/// copied back after.
pub(crate) fn bind(list: &HostList, key: usize) -> Option<Map> {
    let table = with_heap(key, |heap| {
        let mut shared = shared();
        let host = std::ptr::from_ref(heap) as usize;
        if let Some((_, s)) = shared.iter().find(|(k, s)| *k == key && s.host == host) {
            list.keep(&s.table);
            return Some(s.gpu);
        }
        // A released heap's table goes with it (lists that name it hold it).
        shared.retain(|(k, _)| *k != key);
        let s = Shared::new(heap)?;
        list.keep(&s.table);
        let gpu = s.gpu;
        shared.push((key, s));
        Some(gpu)
    })
    .flatten()?;
    let pooled = Arc::new(PooledBitmap::new(Bitmap::acquire()?));
    let dirty = pooled.gpu();
    list.defer_memory(key);
    let precise: Arc<Mutex<Vec<(u64, u64)>>> = Arc::default();
    let byte_for_byte = precise.clone();
    let uses: Arc<Mutex<std::collections::BTreeMap<String, u64>>> = Arc::default();
    let used = uses.clone();
    let missed_bitmap = Arc::clone(&pooled);
    list.defer_missed(move || {
        let l = layout(key);
        // SAFETY: the bitmap is mapped and the list's run is over.
        let marks =
            unsafe { std::slice::from_raw_parts(missed_bitmap.data() as *const u32, BITMAP_WORDS) };
        crate::paging::has_missed(marks, &l.blocks, &l.owner)
    });
    list.defer_after(move || {
        let Layout { copies, blocks, owner, .. } = &*layout(key);
        let precise = byte_for_byte.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the bitmap is mapped and the list's run is over.
        let marks =
            unsafe { std::slice::from_raw_parts_mut(pooled.data() as *mut u32, BITMAP_WORDS) };
        // Block 0's bits are 256 MiB regions with no block; a written page
        // in a root UAV's range is left to that range's copy-back unless a
        // page on demand held it.
        let m = crate::paging::take_marks(marks, blocks, owner, &precise, |k| copies[k].paged());
        let (pages, regions) = (m.missed.len() as u64, m.regions.len() as u64 + m.above + m.stray);
        let missed: Vec<u64> =
            m.regions.iter().map(|&l1| (l1 as u64) << 28).chain(m.missed.iter().copied()).collect();
        // Counted by what each is, and (with the switch) given a copy for
        // the submissions that follow.
        if m.misses() + m.precise_paged > 0 {
            note_misses(&m);
        }
        // What shaders computed from memory with no copy (read as zeros) is
        // not the console's: it does not go into title memory, where it broke
        // the title's own data (garbage pointers in title threads, run c1),
        // nor while any list using title memory misses ([`withholding`]).
        let withheld = m.misses() > 0 || withholding();
        let mut written = 0;
        if withheld {
            for &(_, at) in &m.writes {
                drop_withheld(at as usize, 1 << 16);
            }
            // The list is not run again: whose output that loses is counted.
            if m.wrote() > 0 {
                let used = used.lock().unwrap_or_else(|e| e.into_inner());
                paging(|p| p.state.note_withheld(used.iter().map(|(name, &n)| (name.as_str(), n))));
            }
        } else {
            for &(k, at) in &m.writes {
                let n = copies[k].copy_back_page(at as usize);
                tracing::debug!(
                    page = format_args!("{at:#x}"),
                    copy = format_args!("{:#x?}", copies[k].range()),
                    bytes = n,
                    "title memory page written back"
                );
                written += 1;
            }
        }
        stats(|s| {
            s.written += written;
            s.withheld += if withheld { m.writes.len() as u64 } else { 0 };
            s.missed_pages += pages;
            s.missed_regions += regions;
            let room = 4usize.saturating_sub(s.first_missed.len());
            s.first_missed.extend(missed.into_iter().take(room));
        });
    });
    Some(Map {
        table,
        dirty,
        precise,
        uses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes of lists using title memory are held back from a frame with a
    /// miss through the next one, then go back again.
    #[test]
    fn a_miss_holds_back_its_frame_and_the_next() {
        assert!(!withheld_in(0, MISS_NEVER));
        assert!(!withheld_in(u64::MAX, MISS_NEVER));
        assert!(withheld_in(7, 7));
        assert!(withheld_in(8, 7), "the next frame reads this one's results");
        assert!(!withheld_in(9, 7));
    }

    fn gpu_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn bitmap_pool_reuses_and_zeroes_across_resets() {
        let _guard = gpu_test_lock();
        let (Some(host), Some(_views)) = (crate::host::host(), heap()) else {
            return; // no host GPU here
        };
        let a = host.new_list().unwrap();
        let b = host.new_list().unwrap();
        let map_a = bind(&a, OURS).expect("bitmap for list a");
        let map_b = bind(&b, OURS).expect("bitmap for list b");
        let va_a = map_a.dirty;
        let va_b = map_b.dirty;
        assert_ne!(va_a, va_b, "simultaneously open lists get distinct bitmaps");
        drop((map_a, map_b));
        a.close();
        a.reset();
        let map_a2 = bind(&a, OURS).expect("reused bitmap for list a");
        assert_eq!(map_a2.dirty, va_a, "reset returns bitmap to pool for reuse");
        assert_ne!(map_a2.dirty, va_b, "list b still holds its bitmap");
        drop(map_a2);
        a.close();
        b.close();
        a.reset();
        b.reset();

        // Also verify PooledBitmap zeroes dirty words when dropped without executing.
        let pooled = PooledBitmap::new(Bitmap::acquire().unwrap());
        let data = pooled.data();
        // SAFETY: exclusive ownership of this newly acquired bitmap.
        unsafe { (data as *mut u32).add(7).write(0xDEAD_BEEF) };
        drop(pooled);
        let reused = Bitmap::acquire().unwrap();
        // SAFETY: `reused.data` is mapped for `BITMAP_WORDS` dwords.
        let words = unsafe { std::slice::from_raw_parts(reused.data as *const u32, BITMAP_WORDS) };
        assert!(
            words.iter().all(|&w| w == 0),
            "pooled bitmap must be zeroed on return"
        );
        drop(PooledBitmap::new(reused));
    }

    /// Reads 4 dwords of title memory at (`lo`, `hi`) through the page
    /// table and the heap's views, as translated shaders do, into `dst`
    /// from byte `at`.
    const SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0, numDescriptors=unbounded)), SRV(t0, space=2), UAV(u0), RootConstants(num32BitConstants=3, b0)"
ByteAddressBuffer views[] : register(t0);
ByteAddressBuffer table : register(t0, space2);
RWByteAddressBuffer dst : register(u0);
cbuffer c : register(b0) { uint lo; uint hi; uint at; }
[RootSignature(RS)]
[numthreads(4, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint a = lo + id.x * 4;
    bool inside = hi < 256;
    uint l1 = inside ? (hi << 4 | a >> 28) : 0;
    uint block = inside ? table.Load(l1 * 4) : 0;
    uint entry = block << 12 | (a >> 16 & 0xFFF);
    uint2 e = table.Load2((4096 + 2 * entry) * 4);
    dst.Store(at + id.x * 4, views[NonUniformResourceIndex(e.x)].Load(a - e.y));
}
"#;

    fn pipeline(source: &str) -> (
        windows::Win32::Graphics::Direct3D12::ID3D12RootSignature,
        windows::Win32::Graphics::Direct3D12::ID3D12PipelineState,
    ) {
        use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_ENABLE_UNBOUNDED_DESCRIPTOR_TABLES, D3DCompile};
        use windows::Win32::Graphics::Direct3D::ID3DBlob;
        use windows::Win32::Graphics::Direct3D12::*;
        use windows::core::s;
        let host = crate::host::host().unwrap();
        let (mut code, mut errors): (Option<ID3DBlob>, Option<ID3DBlob>) = (None, None);
        // SAFETY: the source and names outlive the call; the errors are text.
        unsafe {
            let made = D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                s!("memory"),
                None,
                None,
                s!("main"),
                s!("cs_5_1"),
                D3DCOMPILE_ENABLE_UNBOUNDED_DESCRIPTOR_TABLES,
                0,
                &mut code,
                Some(&mut errors),
            );
            let text = errors.map(|e| String::from_utf8_lossy(std::slice::from_raw_parts(e.GetBufferPointer().cast::<u8>(), e.GetBufferSize())).into_owned());
            assert!(made.is_ok(), "{text:?}");
        }
        let code = code.unwrap();
        // SAFETY: the blob's own bytes; a compiled shader with its root
        // signature inside.
        unsafe {
            let bytes = std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
            let root: ID3D12RootSignature = host.device.CreateRootSignature(0, bytes).unwrap();
            let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                CS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: bytes.as_ptr().cast(),
                    BytecodeLength: bytes.len(),
                },
                ..Default::default()
            };
            let pso = host.device.CreateComputePipelineState(&desc).unwrap();
            (root, pso)
        }
    }

    /// A copy's view pair, in view heap `key`'s table.
    fn pair_of(key: usize, copy: &Arc<Shadow>) -> Option<u32> {
        shared()
            .iter()
            .find(|(k, _)| *k == key)?
            .1
            .pairs
            .get(&(Arc::as_ptr(copy) as usize))
            .copied()
    }

    /// Two lists of a submission read title memory through one page table;
    /// the copies' views stay where they are for later lists and
    /// submissions, also when another copy comes; none takes a per-draw
    /// dynamic slot.
    #[test]
    fn lists_share_a_table_and_copies_keep_their_views() {
        let _guard = gpu_test_lock();
        use crate::watch::PAGE;
        let (Some(host), Some(views)) = (crate::host::host(), heap()) else {
            return; // no host GPU here
        };
        let (root, pso) = pipeline(SOURCE);
        let (out, at) = host.shared_buffer(256).unwrap();
        // SAFETY: a live buffer.
        let out_gpu = unsafe { out.GetGPUVirtualAddress() };
        // Title memory (below 2^40, as the runtime places it): 1, 2 and
        // (from the second submission on) 1 page, write-watched as its pools.
        let pool = |pages: usize| -> usize {
            use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
            (0..256u64)
                .map(|k| 0x40_0000_0000 + (k << 28))
                // SAFETY: a fresh allocation where nothing is mapped, or none.
                .map(|hint| unsafe { VirtualAlloc(hint as *const _, pages * PAGE, MEM_RESERVE | MEM_COMMIT | crate::watch::tests::MEM_WRITE_WATCH, PAGE_READWRITE) } as usize)
                .find(|&p| p != 0)
                .expect("memory below 2^40")
        };
        let memory = [pool(1), pool(2), pool(1)];
        let copy = |p: usize, pages: usize| {
            crate::shadow::register(p, p as u64, (pages * PAGE) as u64);
            crate::shadow::find(p as u64, (pages * PAGE) as u64).unwrap()
        };
        let (a, b) = (copy(memory[0], 1), copy(memory[1], 2));
        let lists = [host.new_list().unwrap(), host.new_list().unwrap()];
        // Each list reads 4 dwords at its address into its 16 bytes of `out`.
        let record = |reads: [usize; 2]| -> Vec<Map> {
            let maps: Vec<Map> = lists.iter().map(|l| bind(l, OURS).expect("a page table")).collect();
            for (k, (list, &address)) in lists.iter().zip(&reads).enumerate() {
                let constants = [address as u32, (address >> 32) as u32, 16 * k as u32];
                // SAFETY: an open list, live objects of ours.
                unsafe {
                    list.list.SetDescriptorHeaps(&[Some(views.heap.clone())]);
                    list.list.SetComputeRootSignature(&root);
                    list.list.SetComputeRootDescriptorTable(0, views.gpu(0));
                    list.list.SetComputeRootShaderResourceView(1, maps[k].table);
                    list.list.SetComputeRootUnorderedAccessView(2, out_gpu);
                    list.list.SetComputeRoot32BitConstants(3, 3, constants.as_ptr().cast(), 0);
                    list.list.SetPipelineState(&pso);
                    list.list.Dispatch(1, 1, 1);
                }
                list.close();
            }
            maps
        };
        // The title writes after recording, before submitting.
        let fill = |p: usize, base: u32| {
            for k in 0..4 {
                // SAFETY: our pool (a guarded page faults into the handler).
                unsafe { (p as *mut u32).add(k).write_volatile(base + k as u32) };
            }
        };
        let read = || -> Vec<u32> {
            // SAFETY: the mapped output, the GPU done with it.
            unsafe { std::slice::from_raw_parts(at.cast::<u32>(), 8).to_vec() }
        };
        let maps = record([memory[0] + 16, memory[1] + PAGE]);
        assert_eq!(maps[0].table, maps[1].table, "one table for the heap");
        assert_ne!(maps[0].dirty, maps[1].dirty, "a bitmap per list");
        fill(memory[0] + 16, 0x100);
        fill(memory[1] + PAGE, 0x200);
        host.execute(&[&lists[0], &lists[1]]);
        assert_eq!(read(), [0x100, 0x101, 0x102, 0x103, 0x200, 0x201, 0x202, 0x203]);
        let pairs = (pair_of(OURS, &a).unwrap(), pair_of(OURS, &b).unwrap());
        assert!(pairs.0 >= 1 << 18 && pairs.1 >= 1 << 18, "past the dynamic slots");
        // A second submission, after a third copy came.
        let c = copy(memory[2], 1);
        for list in &lists {
            list.reset();
        }
        record([memory[2] + 32, memory[0] + 16]);
        fill(memory[2] + 32, 0x300);
        fill(memory[0] + 16, 0x400);
        host.execute(&[&lists[0], &lists[1]]);
        assert_eq!(read(), [0x300, 0x301, 0x302, 0x303, 0x400, 0x401, 0x402, 0x403]);
        assert_eq!((pair_of(OURS, &a).unwrap(), pair_of(OURS, &b).unwrap()), pairs, "views kept");
        assert!(pair_of(OURS, &c).is_some());
        for (&p, copy) in memory.iter().zip([a, b, c]) {
            crate::shadow::unregister(p, p as u64);
            drop(copy);
        }
    }

    /// Writes `value` at (`lo`, `hi`) through the page table, marking its
    /// page as translated shaders do, and with `missing_hi` non-zero reads
    /// (`missing_lo`, `missing_hi`), marking it if no copy holds it.
    const WRITE: &str = r#"
#define RS "DescriptorTable(UAV(u0, numDescriptors=unbounded)), SRV(t0, space=2), UAV(u0, space=2), RootConstants(num32BitConstants=5, b0)"
RWByteAddressBuffer views[] : register(u0);
ByteAddressBuffer table : register(t0, space2);
RWByteAddressBuffer dirty : register(u0, space2);
cbuffer c : register(b0) { uint lo; uint hi; uint value; uint missing_lo; uint missing_hi; }
void access(uint a, uint h, bool write) {
    bool inside = h < 256;
    uint l1 = inside ? (h << 4 | a >> 28) : 0;
    uint block = inside ? table.Load(l1 * 4) : 0;
    uint entry = block << 12 | (a >> 16 & 0xFFF);
    uint2 e = table.Load2((4096 + 2 * entry) * 4);
    bool miss = e.x == table.Load(4096 * 4);
    uint n = miss && block == 0 ? l1 : entry;
    if (write || miss) {
        dirty.InterlockedOr(n >> 5 << 2, 1u << (n & 31));
    }
    if (write && !miss) {
        views[NonUniformResourceIndex(e.x + 1)].Store(a - e.y, value);
    }
}
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {
    access(lo, hi, true);
    if (missing_hi != 0) {
        access(missing_lo, missing_hi, false);
    }
}
"#;

    /// What a submission that missed memory wrote through the page table
    /// leaves the host copies right after it, so the next submission reads
    /// title memory's bytes (run shards20: garbage BVH nodes over meshlet
    /// buffers until the frame's end); one that missed nothing leaves its
    /// writes in the copies.
    #[test]
    fn a_submission_that_missed_leaves_the_copies_at_once() {
        let _guard = gpu_test_lock();
        use crate::watch::PAGE;
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
        let (Some(host), Some(views)) = (crate::host::host(), heap()) else {
            return; // no host GPU here
        };
        let ((write_root, writer), (read_root, reader)) = (pipeline(WRITE), pipeline(SOURCE));
        let (out, out_cpu) = host.shared_buffer(256).unwrap();
        // SAFETY: a live buffer.
        let out_gpu = unsafe { out.GetGPUVirtualAddress() };
        // A page of title memory below 2^40, write-watched as the pools.
        let p = (0..256u64)
            .map(|k| 0x50_0000_0000 + (k << 28))
            // SAFETY: a fresh allocation where nothing is mapped, or none.
            .map(|hint| unsafe { VirtualAlloc(hint as *const _, PAGE, MEM_RESERVE | MEM_COMMIT | crate::watch::tests::MEM_WRITE_WATCH, PAGE_READWRITE) } as usize)
            .find(|&p| p != 0)
            .expect("memory below 2^40");
        let at = p + 16;
        // SAFETY: our pool, not guarded yet.
        unsafe { (at as *mut u32).write_volatile(0x11) };
        crate::shadow::register(p, p as u64, PAGE as u64);
        let copy = crate::shadow::find(p as u64, PAGE as u64).unwrap();
        let list = host.new_list().unwrap();
        // One submission of one dispatch; the list is open again after it.
        let submit = |root: &windows::Win32::Graphics::Direct3D12::ID3D12RootSignature,
                      pso: &windows::Win32::Graphics::Direct3D12::ID3D12PipelineState,
                      uav: Option<u64>,
                      constants: &[u32]| {
            let map = bind(&list, OURS).expect("a page table");
            // SAFETY: an open list, live objects of ours.
            unsafe {
                list.list.SetDescriptorHeaps(&[Some(views.heap.clone())]);
                list.list.SetComputeRootSignature(root);
                list.list.SetComputeRootDescriptorTable(0, views.gpu(0));
                list.list.SetComputeRootShaderResourceView(1, map.table);
                list.list.SetComputeRootUnorderedAccessView(2, uav.unwrap_or(map.dirty));
                list.list.SetComputeRoot32BitConstants(3, constants.len() as u32, constants.as_ptr().cast(), 0);
                list.list.SetPipelineState(pso);
                list.list.Dispatch(1, 1, 1);
            }
            list.close();
            host.execute(&[&list]);
            list.reset();
        };
        let write = |value: u32, miss: bool| {
            // Memory no copy holds, in a 256 MiB region with no block.
            let missing: u64 = if miss { 0xff_f000_0000 } else { 0 };
            let constants = [at as u32, (at >> 32) as u32, value, missing as u32, (missing >> 32) as u32];
            submit(&write_root, &writer, None, &constants);
        };
        let read = || -> u32 {
            submit(&read_root, &reader, Some(out_gpu), &[at as u32, (at >> 32) as u32, 0]);
            // SAFETY: the mapped output, the GPU done with it.
            unsafe { out_cpu.cast::<u32>().read_volatile() }
        };
        // SAFETY: our pool.
        let title = || unsafe { (at as *const u32).read_volatile() };
        write(0x33, true);
        // Another test's submission may have refreshed the copy already.
        let held = copy.read_host(at as u64, 1)[0];
        assert!(!copy.clean() || held == 0x11, "the written page counts as changed at once");
        assert_eq!(title(), 0x11, "nothing went back");
        assert_eq!(read(), 0x11, "the next submission reads title memory's bytes");
        // Unless another test ends a frame meanwhile (its retake is right).
        let frame = FRAMES.load(std::sync::atomic::Ordering::Relaxed);
        write(0x44, false);
        let kept = read();
        if FRAMES.load(std::sync::atomic::Ordering::Relaxed) == frame {
            assert_eq!(kept, 0x44, "a submission that missed nothing keeps its writes");
        }
        crate::shadow::unregister(p, p as u64);
        drop(copy);
    }

    /// Reads 4 dwords of title memory from (`lo`, `hi`) on with one lookup,
    /// as a translated shader's load of several dwords does (the dwords
    /// follow the first in the view the first was found in), into `dst`
    /// from byte `at`.
    const RECORD: &str = r#"
#define RS "DescriptorTable(SRV(t0, numDescriptors=unbounded)), SRV(t0, space=2), UAV(u0), RootConstants(num32BitConstants=3, b0)"
ByteAddressBuffer views[] : register(t0);
ByteAddressBuffer table : register(t0, space2);
RWByteAddressBuffer dst : register(u0);
cbuffer c : register(b0) { uint lo; uint hi; uint at; }
[RootSignature(RS)]
[numthreads(4, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    bool inside = hi < 256;
    uint l1 = inside ? (hi << 4 | lo >> 28) : 0;
    uint block = inside ? table.Load(l1 * 4) : 0;
    uint entry = block << 12 | (lo >> 16 & 0xFFF);
    uint2 e = table.Load2((4096 + 2 * entry) * 4);
    dst.Store(at + id.x * 4, views[NonUniformResourceIndex(e.x)].Load(lo - e.y + id.x * 4));
}
"#;

    /// Two neighbouring pages of title memory in no title buffer, reached
    /// by address. The write that misses the first is dropped and nothing
    /// goes back, as without the switch. With pages on demand the page then
    /// has a copy, and the page after it too, beside it: the next
    /// submission reads the title's bytes there, a record that starts in
    /// the first page and ends in the second is read whole by one lookup,
    /// and once nothing is withheld any more a write through the table into
    /// the second page goes back into title memory. Given up, the pages are
    /// missed as before.
    #[test]
    fn a_missed_page_of_title_memory_gets_a_copy_for_the_next_submission() {
        let _guard = gpu_test_lock();
        use crate::watch::PAGE;
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
        let (Some(host), Some(views)) = (crate::host::host(), heap()) else {
            return; // no host GPU here
        };
        let ((write_root, writer), (read_root, reader), (record_root, recorder)) =
            (pipeline(WRITE), pipeline(SOURCE), pipeline(RECORD));
        let (out, out_cpu) = host.shared_buffer(256).unwrap();
        // SAFETY: a live buffer.
        let out_gpu = unsafe { out.GetGPUVirtualAddress() };
        // Two pages of title memory below 2^40, write-watched as the pools,
        // at the start of a 4 MiB run and with no title buffer over them.
        let p = (0..256u64)
            .map(|k| 0x60_0000_0000 + (k << 28))
            // SAFETY: a fresh allocation where nothing is mapped, or none.
            .map(|hint| unsafe { VirtualAlloc(hint as *const _, 2 * PAGE, MEM_RESERVE | MEM_COMMIT | crate::watch::tests::MEM_WRITE_WATCH, PAGE_READWRITE) } as usize)
            .find(|&p| p != 0 && (p as u64).is_multiple_of(RUN_BYTES))
            .expect("memory below 2^40");
        let page = p as u64 >> 16;
        // The first page's last two dwords and the second's first two: a
        // record across the boundary. `at` is in the second page.
        let (record, at) = (p + PAGE - 8, p + PAGE + 16);
        for k in 0..4 {
            // SAFETY: our pool, not guarded yet.
            unsafe { ((record + 4 * k) as *mut u32).write_volatile(0x70 + k as u32) };
        }
        // SAFETY: as above.
        unsafe { (at as *mut u32).write_volatile(0x11) };
        let list = host.new_list().unwrap();
        // One submission of one dispatch; the list is open again after it.
        let submit = |root: &windows::Win32::Graphics::Direct3D12::ID3D12RootSignature,
                      pso: &windows::Win32::Graphics::Direct3D12::ID3D12PipelineState,
                      uav: Option<u64>,
                      constants: &[u32]| {
            let map = bind(&list, OURS).expect("a page table");
            // SAFETY: an open list, live objects of ours.
            unsafe {
                list.list.SetDescriptorHeaps(&[Some(views.heap.clone())]);
                list.list.SetComputeRootSignature(root);
                list.list.SetComputeRootDescriptorTable(0, views.gpu(0));
                list.list.SetComputeRootShaderResourceView(1, map.table);
                list.list.SetComputeRootUnorderedAccessView(2, uav.unwrap_or(map.dirty));
                list.list.SetComputeRoot32BitConstants(3, constants.len() as u32, constants.as_ptr().cast(), 0);
                list.list.SetPipelineState(pso);
                list.list.Dispatch(1, 1, 1);
            }
            list.close();
            host.execute(&[&list]);
            list.reset();
        };
        let write = |to: usize, value: u32| submit(&write_root, &writer, None, &[to as u32, (to >> 32) as u32, value, 0, 0]);
        let read = |root, pso, from: usize| -> Vec<u32> {
            submit(root, pso, Some(out_gpu), &[from as u32, (from >> 32) as u32, 0]);
            // SAFETY: the mapped output, the GPU done with it.
            unsafe { std::slice::from_raw_parts(out_cpu.cast::<u32>(), 4).to_vec() }
        };
        // SAFETY: our pool.
        let title = || unsafe { (at as *const u32).read_volatile() };
        let held = |page: u64| paging(|p| p.state.holds(page));
        paging(|p| p.state.set_on(true));
        // The first miss may only name the 256 MiB region (no copy lies in
        // it); the region then has a block and the next names the page.
        for _ in 0..3 {
            if !held(page) {
                write(p + 32, 0x33);
            }
        }
        assert!(held(page), "the missed page has a copy");
        assert!(held(page + 1), "and the page after it in its run, beside it");
        // SAFETY: our pool.
        assert_ne!(unsafe { ((p + 32) as *const u32).read_volatile() }, 0x33, "the writes that missed were dropped");
        assert_eq!(read(&read_root, &reader, at)[0], 0x11, "the next submission reads the title's bytes through the page's copy");
        assert_eq!(read(&record_root, &recorder, record), [0x70, 0x71, 0x72, 0x73], "a record across the two pages, by one lookup");
        // Nothing is withheld two frames after the last miss.
        for value in [0x44, 0x45, 0x46] {
            if title() == 0x11 {
                log_frame();
                log_frame();
                write(at, value);
            }
        }
        assert_ne!(title(), 0x11, "a write through the second page's copy goes back into title memory");
        assert_eq!(read(&read_root, &reader, at)[0], title(), "and the copy holds it");
        // Given up: no longer held (the buffer goes once the tables have
        // been rewritten and the copies are gone, which another thread may
        // do), and without the switch the pages are missed as before.
        paging(|p| p.state.set_on(false));
        give_up(&[(page, GivenUp::Vanished), (page + 1, GivenUp::Vanished)]);
        assert!(!held(page) && !held(page + 1));
        write(at, 0x55);
        assert!(!held(page + 1));
        assert_ne!(title(), 0x55, "without a copy the write is dropped again");
    }
}
