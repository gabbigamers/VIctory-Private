//! The bookkeeping of title memory by address that needs no device
//! (`crate::memory` is the device's side; the contract is
//! `victory_shader::emit`'s "Title memory"): the page table's layout, what
//! the bits of a list's bitmap mean, why a page was missed, and the pages
//! given a host copy on demand. Everything here runs and is tested on any
//! host; what asks the system or the device is behind [`Host`].
//!
//! **Pages on demand** (`VICTORY_MEMORY_PAGING=1`; off by default). A page
//! a shader reaches by an address it computed has a host copy only if a
//! draw, a view or a root argument named memory around it first. A page no
//! copy holds reads 0 and takes no writes, the miss is marked, and the
//! writes of every program using title memory stay out of title memory for
//! that frame and the next (`crate::memory::withholding`). Nothing made a
//! copy for the page, so a program that reaches it every frame misses every
//! frame. With the switch, a missed page that is title memory gets a copy of
//! its own after the submission that missed it, so the next submission finds
//! it.
//!
//! **Nothing runs again.** The miss itself is not undone: it is counted, the
//! list that missed keeps its writes out of title memory as before, and the
//! copies drop them. A program the title runs every frame reads the page the
//! next time; a program the title runs once (a build, an unpack in place)
//! that missed has lost its output for good. The lists whose writes were
//! withheld are counted by pipeline ([`State::note_withheld`]) so that a run
//! shows this.
//!
//! Where a page's copy lies. Pages on demand are held by aligned run of
//! [`RUN_PAGES`] pages (4 MiB, as the windows of `crate::shadow`): a run has
//! one host buffer, made with its first page, and each page lies in it at
//! its own offset. Neighbouring pages of a run therefore lie side by side,
//! as in title memory, and an access that starts in one and ends in the
//! next (one lookup per access) reads and writes the right bytes when both
//! are held. To have them both held, the pages after a missed page in its
//! run are looked at and given copies with it, up to the first that cannot
//! have one ([`State::admit`]).
//! shortcut: an access running past the end of a held page whose next page
//! is not beside it (the next run, a page another copy holds, a page that
//! is not title memory) reads 0 for that part and marks nothing, as at the
//! end of every copy. [`Written::cut`] counts the pages where that can
//! happen, and the misses line shows the figure; a write there marks the
//! next page, so it is a counted miss (and the place it wrote in the buffer
//! is zeroed again) or goes back with that page.
//!
//! Who may write a page's copy: shaders, through the page table; the
//! refresh before a submission, from title memory, where the title wrote the
//! page since the copy last took it (`crate::watch`); nothing else. What
//! shaders wrote goes back to title memory after their submission by the
//! rules of every copy (`crate::shadow::Shadow::copy_back_page`: not while
//! withheld, and not over a page the title wrote meanwhile). A copy is made
//! only for memory the change tracker tracks whole, so that check is never
//! skipped for it.
//!
//! A page's copy stands back for every other copy the table would name for
//! its page, whole or in part (a title buffer's, a window's): those are the
//! copies views use, and one page must not be read at two places in one
//! submission. So with the switch a table differs from the one without it
//! only in entries that named no copy. A page's copy is given up when
//! another copy has its page, or when the title has given the memory up;
//! the run's buffer is freed when the last copy in it is gone. Lists
//! recorded and not yet run name no copy (the table is written at
//! submission), and a submission that runs keeps its copies alive itself.
//!
//! Bounds: [`Pager`] holds pages in at most its limit of runs, so host
//! memory is at most that many buffers of [`RUN_BYTES`], and takes at most a
//! set number of pages and new runs a frame. Past either the page stays a
//! counted miss.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Pages per level-2 block: 4096 pages of 64 KiB, 256 MiB.
pub(crate) const BLOCK: usize = 4096;
/// Level-1 entries: 2^40 bytes in 256 MiB blocks.
pub(crate) const LEVEL1: usize = 4096;
/// Level-2 blocks a table has room for besides the null one: copies in 512
/// distinct 256 MiB regions (the world's ~4,100 copies use a few dozen).
pub(crate) const CAPACITY: usize = 512;
/// Sentinel in a table's owners for a page with no host copy.
pub(crate) const NO_OWNER: u32 = u32::MAX;
/// Total dwords in a page table buffer (level-1 + `CAPACITY + 1` blocks of
/// `BLOCK` 2-dword entries).
pub(crate) const TABLE_DWORDS: usize = LEVEL1 + (CAPACITY + 1) * BLOCK * 2;
/// Bits in a list's dirty-page bitmap (one per level-2 entry across all blocks).
pub(crate) const BITMAP_BITS: usize = (CAPACITY + 1) * BLOCK;
/// 32-bit words in a dirty-page bitmap.
pub(crate) const BITMAP_WORDS: usize = BITMAP_BITS / 32;
/// Byte size of a dirty-page bitmap buffer (256-byte aligned).
pub(crate) const BITMAP_BYTES: u64 = (BITMAP_BITS / 8).next_multiple_of(256) as u64;

/// A page of title memory, in bytes.
pub(crate) const PAGE: u64 = 1 << 16;
/// Pages of an aligned run of title memory: the pages on demand of one run
/// share one host buffer, each at its own offset in the run.
pub(crate) const RUN_PAGES: u64 = 64;
/// Bytes of a run and of its host buffer: 4 MiB.
pub(crate) const RUN_BYTES: u64 = RUN_PAGES * PAGE;
/// Pages given a copy in one frame at most, but that the pages after a
/// missed page in its run are taken with it (so up to [`RUN_PAGES`]` - 1`
/// more): each costs a first copy of 64 KiB.
pub(crate) const ADMIT_PER_FRAME: usize = 256;
/// Runs started in one frame at most: each costs a buffer of [`RUN_BYTES`].
pub(crate) const RUNS_PER_FRAME: usize = 16;
/// Frames no page is given a copy after host memory for a run's buffer was
/// refused.
pub(crate) const STALL: u64 = 30;
/// Megabytes of host buffers for pages held on demand when the switch gives
/// no figure, and the most it may ask for.
pub(crate) const DEFAULT_MB: u64 = 256;
pub(crate) const MOST_MB: u64 = 4096;
/// Regions of 256 MiB given an empty block so that misses there name their
/// page ([`Probes`]): each is 32 KiB more to write whenever a table is
/// written.
pub(crate) const PROBES: usize = 96;
/// A probe not missed for so many frames makes way for another region.
pub(crate) const PROBE_IDLE: u64 = 300;
/// Pages of a probed region found before the region may be taken for one
/// only stray addresses reach (all of them not title memory).
pub(crate) const PROBE_STRAY: u32 = 4;
/// Frames a region set aside as stray gets no probe again.
pub(crate) const PROBE_AGAIN: u64 = 3600;
/// Missed pages looked at for the first time in one frame at most without
/// the switch ([`Cache`]): each asks the system about the page (a few
/// calls, each of which can cost a millisecond in the runtime's big pools).
/// With the switch as many as may be given a copy ([`ADMIT_PER_FRAME`]).
pub(crate) const ASK_PER_FRAME: usize = 16;
/// For so many frames at least what was found of a page is taken to hold,
/// plus up to as many again by the page (so that pages found together are
/// not asked about again together), doubled each time the same is found
/// again, up to [`ASK_AGAIN_MOST`].
pub(crate) const ASK_AGAIN: u64 = 30;
pub(crate) const ASK_AGAIN_MOST: u64 = 960;
/// Pages [`Cache`] remembers at most.
pub(crate) const CACHE: usize = 8192;
/// Held pages checked at each frame's end for memory the title gave up
/// (one refused or granted read each).
pub(crate) const SWEEP: usize = 128;

/// The megabytes `VICTORY_MEMORY_PAGING_MB` asks for (`text`: its value),
/// and whether the value was understood: unset is the default; a value
/// that is no number, or 0, is the default too and not understood (the
/// caller says so once).
pub(crate) fn megabytes(text: Option<&str>) -> (u64, bool) {
    match text.map(|t| t.trim().parse::<u64>()) {
        None => (DEFAULT_MB, true),
        Some(Ok(mb)) if mb > 0 => (mb, true),
        Some(_) => (DEFAULT_MB, false),
    }
}

/// The runs `megabytes` of host buffers are: cut to [`MOST_MB`], whole
/// buffers, one at least.
pub(crate) fn limit_runs(megabytes: u64) -> usize {
    ((megabytes.min(MOST_MB) << 20) / RUN_BYTES).max(1) as usize
}

/// The most bytes of host buffers a limit of `runs` can come to.
pub(crate) fn worst_case_bytes(runs: usize) -> u64 {
    runs as u64 * RUN_BYTES
}

/// The run page `page` (a page number) lies in.
pub(crate) fn run_of(page: u64) -> u64 {
    page / RUN_PAGES
}

/// Where page `page` (a page number) lies in its run's buffer, in bytes.
pub(crate) fn offset_in_run(page: u64) -> u64 {
    (page % RUN_PAGES) * PAGE
}

/// A host copy a page table names.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Named {
    /// The title memory it copies: where it starts, its bytes.
    pub(crate) start: u64,
    pub(crate) len: u64,
    /// The heap index of its view pair.
    pub(crate) pair: u32,
    /// The low dword of the title address the view's first byte shows:
    /// `start` for a copy with a buffer of its own, the run's first address
    /// for a page on demand (its view is of the run's whole buffer).
    pub(crate) view_start: u32,
    /// Made for a missed page: any other copy named for the page has it.
    pub(crate) yields: bool,
}

impl Named {
    /// Whether it holds all of page `page` (a page number).
    fn whole(&self, page: u64) -> bool {
        page << 16 >= self.start && (page + 1) << 16 <= self.start.saturating_add(self.len)
    }
}

/// What [`write_table`] wrote.
pub(crate) struct Written {
    /// Dwords written.
    pub(crate) dwords: usize,
    /// The level-1 index of each block (block 0 is the null one).
    pub(crate) blocks: Vec<usize>,
    /// The copy each entry names, or [`NO_OWNER`].
    pub(crate) owner: Vec<u32>,
    /// Copies made for a missed page whose page another copy has: `true`
    /// where that copy holds the page whole, `false` where it holds a part
    /// (the rest of such a page reads 0, with the switch as without).
    pub(crate) superseded: Vec<(usize, bool)>,
    /// Pages named for a copy that holds only a part of them: what lies in
    /// the rest of such a page reads 0 and marks nothing (the module
    /// header's shortcut in `crate::memory`), so neither the miss counts
    /// nor pages on demand see it.
    pub(crate) partial: usize,
    /// Pages on demand whose next page is not beside them in host memory:
    /// an access that starts in one and runs past its end reads 0 for the
    /// rest and marks nothing.
    pub(crate) cut: usize,
}

/// Writes the page table for host copies `copies` and `null` for unowned
/// pages directly into `out` (which must hold at least [`TABLE_DWORDS`]
/// dwords). `probes`: 256 MiB regions (level-1 indices) that get a block of
/// null entries if no copy gave them one and there is room, so that a miss
/// there names its page instead of its region. At most [`CAPACITY`] blocks
/// besides the null one.
pub(crate) fn write_table(copies: &[Named], probes: &[usize], null: u32, out: &mut [u32]) -> Written {
    assert!(out.len() >= TABLE_DWORDS);
    // Larger copies first; those made for a missed page after all others.
    let mut order: Vec<usize> = (0..copies.len()).collect();
    order.sort_by_key(|&k| (copies[k].yields, std::cmp::Reverse(copies[k].len)));
    // Build level-1 in cached stack memory so we never read from write-combined upload memory.
    let mut level1 = [0u32; LEVEL1];
    // Block 0 is the null block (no copy).
    let mut blocks = vec![usize::MAX];
    let mut owner = vec![NO_OWNER; BLOCK];
    let mut superseded = Vec::new();
    let mut full = false;
    for &k in &order {
        let Named { start, len, yields, .. } = copies[k];
        let Some(end) = start.checked_add(len) else {
            continue;
        };
        if len == 0 || end > 1 << 40 {
            continue;
        }
        for page in start >> 16..=(end - 1) >> 16 {
            let l1 = (page >> 12) as usize;
            if level1[l1] == 0 {
                if blocks.len() > CAPACITY {
                    full = true;
                    continue;
                }
                level1[l1] = blocks.len() as u32;
                blocks.push(l1);
                owner.resize(owner.len() + BLOCK, NO_OWNER);
            }
            let entry = level1[l1] as usize * BLOCK + (page & 0xFFF) as usize;
            let held = owner[entry];
            if yields {
                // It stands back for whatever copy has the page, whole or
                // in part: the entry is the one a table without pages on
                // demand has.
                if held == NO_OWNER {
                    owner[entry] = k as u32;
                } else {
                    superseded.push((k, copies[held as usize].whole(page)));
                }
                continue;
            }
            // Larger copies come first: a copy holding the whole page takes
            // it from them; one holding part of it only takes it if free.
            if copies[k].whole(page) || held == NO_OWNER {
                owner[entry] = k as u32;
            }
        }
    }
    for &l1 in probes {
        if l1 < LEVEL1 && level1[l1] == 0 && blocks.len() <= CAPACITY {
            level1[l1] = blocks.len() as u32;
            blocks.push(l1);
            owner.resize(owner.len() + BLOCK, NO_OWNER);
        }
    }
    if full {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!(CAPACITY, "title memory copies in more 256 MiB regions than a page table has room for: the rest read as missing"));
    }
    out[..LEVEL1].copy_from_slice(&level1);
    let l2_dwords = owner.len() * 2;
    let l2 = &mut out[LEVEL1..LEVEL1 + l2_dwords];
    let (slots, _) = l2.as_chunks_mut::<2>();
    // The copy named for page `page` (a page number).
    let named = |page: u64| -> Option<&Named> {
        let block = *level1.get((page >> 12) as usize)? as usize;
        let k = owner[block * BLOCK + (page & 0xFFF) as usize];
        (block != 0 && k != NO_OWNER).then(|| &copies[k as usize])
    };
    let (mut partial, mut cut) = (0, 0);
    for (entry, (slot, &o)) in slots.iter_mut().zip(&owner).enumerate() {
        if o == NO_OWNER {
            slot[0] = null;
            slot[1] = 0;
            continue;
        }
        let copy = &copies[o as usize];
        slot[0] = copy.pair;
        slot[1] = copy.view_start;
        let page = (blocks[entry / BLOCK] as u64) << 12 | (entry % BLOCK) as u64;
        partial += usize::from(!copy.whole(page));
        if copy.yields {
            // Beside it: the next page is seen through the same view at
            // the same start, so its bytes follow this page's.
            let beside = named(page + 1).is_some_and(|next| (next.pair, next.view_start) == (copy.pair, copy.view_start));
            cut += usize::from(!beside);
        }
    }
    Written {
        dwords: LEVEL1 + l2_dwords,
        blocks,
        owner,
        superseded,
        partial,
        cut,
    }
}

/// What a set bit of a list's bitmap says, by the table the list ran with
/// (`blocks`, `owner`: [`Written`]'s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    /// An access at or above 2^40, where the table does not reach. (Bit 0
    /// while the lowest 256 MiB have a block; without one the bit is
    /// [`Mark::Region`] 0, which may be either.)
    Above,
    /// An access in a 256 MiB region with no block (its level-1 index): no
    /// copy anywhere in it, and the page is not known.
    Region(usize),
    /// An access to the page at this address, which no copy holds.
    Missed(u64),
    /// A write to the page at this address, in copy `.0`.
    Written(usize, u64),
    /// A bit no entry of the table stands for.
    Stray,
}

/// The meaning of bit `entry`.
pub(crate) fn mark(entry: usize, blocks: &[usize], owner: &[u32]) -> Mark {
    let (block, page) = (entry / BLOCK, entry % BLOCK);
    if block == 0 {
        // Block 0's bits are level-1 indices.
        return if entry == 0 && blocks.iter().skip(1).any(|&l1| l1 == 0) { Mark::Above } else { Mark::Region(entry) };
    }
    let Some(&l1) = blocks.get(block) else {
        return Mark::Stray;
    };
    let at = (l1 << 28 | page << 16) as u64;
    match owner.get(entry) {
        Some(&k) if k != NO_OWNER => Mark::Written(k as usize, at),
        _ => Mark::Missed(at),
    }
}

/// The set bits of bitmap words `words`.
pub(crate) fn set_bits(words: &[u32]) -> impl Iterator<Item = usize> + '_ {
    words.iter().enumerate().filter(|(_, w)| **w != 0).flat_map(|(w, &word)| {
        let mut bits = word;
        std::iter::from_fn(move || {
            (bits != 0).then(|| {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                w * 32 + bit
            })
        })
    })
}

/// Whether any marked page in `marks` fell outside a valid copy.
pub(crate) fn has_missed(marks: &[u32], blocks: &[usize], owner: &[u32]) -> bool {
    set_bits(marks).any(|entry| !matches!(mark(entry, blocks, owner), Mark::Written(..)))
}

/// A list's marks after it ran, by what each is ([`take_marks`]).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Marks {
    /// Accesses above the table's reach, and bits no entry stands for.
    pub(crate) above: u64,
    pub(crate) stray: u64,
    /// 256 MiB regions with no block that were reached (level-1 indices).
    pub(crate) regions: Vec<usize>,
    /// Pages no copy holds that were reached (addresses).
    pub(crate) missed: Vec<u64>,
    /// Pages written that go back by page: the copy (its index in the
    /// table's) and the page's address.
    pub(crate) writes: Vec<(usize, u64)>,
    /// Pages written that a root UAV's range touches and its own copy
    /// holds: left to that range's byte-for-byte copy-back.
    pub(crate) precise: u64,
    /// Pages written that a root UAV's range touches but a page on demand
    /// holds: they go back by page (among `writes`), since the root UAV's
    /// copy never saw the write.
    pub(crate) precise_paged: u64,
}

impl Marks {
    /// Accesses that found no copy: any makes the list one that missed
    /// (the same verdict as [`has_missed`]).
    pub(crate) fn misses(&self) -> u64 {
        self.above + self.stray + (self.regions.len() + self.missed.len()) as u64
    }

    /// Pages the list wrote through the table.
    pub(crate) fn wrote(&self) -> u64 {
        self.writes.len() as u64 + self.precise
    }
}

/// Takes the set bits out of a list's bitmap `words` (cleared for its next
/// run) and sorts them by the table the list ran with. `precise`: ranges
/// (address, bytes) that go back byte for byte through a root UAV's own
/// copy; `yields(k)`: whether copy `k` is a page on demand. A written page
/// in such a range is left to the range's copy-back only where the copy
/// the table named for it is not a page on demand: a root UAV is never
/// declared on one, so its copy-back would find nothing there and the
/// write would be lost.
pub(crate) fn take_marks(
    words: &mut [u32],
    blocks: &[usize],
    owner: &[u32],
    precise: &[(u64, u64)],
    yields: impl Fn(usize) -> bool,
) -> Marks {
    let mut m = Marks::default();
    for (w, word) in words.iter_mut().enumerate() {
        if *word == 0 {
            continue;
        }
        let mut bits = std::mem::take(word);
        while bits != 0 {
            let entry = w * 32 + bits.trailing_zeros() as usize;
            bits &= bits - 1;
            match mark(entry, blocks, owner) {
                Mark::Above => m.above += 1,
                Mark::Stray => m.stray += 1,
                Mark::Region(l1) => m.regions.push(l1),
                Mark::Missed(at) => m.missed.push(at),
                Mark::Written(k, at) => {
                    let ranged = precise.iter().any(|&(a, n)| a < at + PAGE && at < a.saturating_add(n));
                    if ranged && !yields(k) {
                        m.precise += 1;
                    } else {
                        m.precise_paged += u64::from(ranged);
                        m.writes.push((k, at));
                    }
                }
            }
        }
    }
    m
}

/// Why an access found no copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Reason {
    /// At or above 2^40 ([`Mark::Above`]).
    Above,
    /// In a 256 MiB region with no block ([`Mark::Region`]).
    Region,
    /// Nothing of the page can be read: the title has no memory there (a
    /// stray pointer, or memory it gave up).
    Absent,
    /// Readable, but not memory the change tracker tracks (a heap, a stack,
    /// an image): never given a copy, since what shaders wrote would go
    /// back into it unchecked.
    Foreign,
    /// Memory the change tracker tracks, in no title buffer object, with no
    /// copy. The tracker takes for the runtime's GPU memory its
    /// write-watched pools and, by exclusion, any mapped view (whatever is
    /// neither an image nor plain committed private memory): a mapped view
    /// that is not the title's is counted here too, and with the switch
    /// gets a copy and the tracker's guards as a window around it does
    /// today. The GPU layer has no list of the runtime's allocations to
    /// ask instead.
    Loose,
    /// Tracked memory inside a title buffer that no draw or view has used,
    /// so that it has no copy.
    Buffer,
    /// A copy holds the page whole and the table did not name it: made
    /// after the table was written (a page on demand made for a list
    /// earlier in the same submission is the usual case), or left out for
    /// want of room or slots.
    Unnamed,
    /// A copy holds a part of the page and the table did not name it. Never
    /// given a copy on demand: the table would name the other copy for the
    /// page once it can.
    Shared,
    /// Not looked at: more pages missed in the frame than are asked about,
    /// or looking is switched off.
    Unasked,
}

impl Reason {
    pub(crate) const ALL: [Reason; 9] = [
        Reason::Above,
        Reason::Region,
        Reason::Absent,
        Reason::Foreign,
        Reason::Loose,
        Reason::Buffer,
        Reason::Unnamed,
        Reason::Shared,
        Reason::Unasked,
    ];

    /// As the log names it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Reason::Above => "at or above 2^40",
            Reason::Region => "in a 256 MiB region with no copy",
            Reason::Absent => "not title memory (nothing readable)",
            Reason::Foreign => "readable but not tracked as the title's GPU memory",
            Reason::Loose => "tracked memory in no buffer object",
            Reason::Buffer => "in a title buffer with no copy",
            Reason::Unnamed => "copied but not named by the table",
            Reason::Shared => "copied in part but not named by the table",
            Reason::Unasked => "not looked at",
        }
    }

    /// Whether a page missed for this reason gets a copy on demand.
    pub(crate) const fn pageable(self) -> bool {
        matches!(self, Reason::Loose | Reason::Buffer)
    }

    /// Whether the page was looked at and is not the title's memory.
    const fn stray(self) -> Option<bool> {
        match self {
            Reason::Absent | Reason::Foreign => Some(true),
            Reason::Loose | Reason::Buffer | Reason::Unnamed | Reason::Shared => Some(false),
            Reason::Above | Reason::Region | Reason::Unasked => None,
        }
    }
}

/// What is known of a missed page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// A host copy holds all of it.
    pub(crate) held: bool,
    /// A host copy holds a part of it, and none all.
    pub(crate) part: bool,
    /// Some of it can be read.
    pub(crate) readable: bool,
    /// All of it is memory the change tracker tracks.
    pub(crate) tracked: bool,
    /// A title buffer object holds all of it.
    pub(crate) in_buffer: bool,
}

pub(crate) fn classify(f: Facts) -> Reason {
    match f {
        Facts { held: true, .. } => Reason::Unnamed,
        Facts { part: true, .. } => Reason::Shared,
        Facts { readable: false, .. } => Reason::Absent,
        Facts { tracked: false, .. } => Reason::Foreign,
        Facts { in_buffer: true, .. } => Reason::Buffer,
        _ => Reason::Loose,
    }
}

/// What was found of a missed page, when, and for how many frames it is
/// taken to hold.
#[derive(Clone, Copy)]
struct Found {
    reason: Reason,
    at: u64,
    wait: u64,
}

/// What was found of missed pages: finding it asks the system, and a page
/// missed once is missed in every submission of the frame and, without a
/// copy, in every frame after. An answer holds for [`ASK_AGAIN`] frames and
/// some more by the page, and twice as long each time the same answer is
/// found again, so a steady set of missed pages is soon asked about a few
/// pages a frame at most.
#[derive(Default)]
pub(crate) struct Cache {
    found: HashMap<u64, Found>,
    /// The frame `asked` counts for, and the pages asked about in it.
    frame: u64,
    asked: usize,
}

impl Cache {
    /// What is known of page `page` in frame `frame`, if it still holds.
    pub(crate) fn get(&self, page: u64, frame: u64) -> Option<Reason> {
        self.found.get(&page).filter(|f| frame.saturating_sub(f.at) < f.wait).map(|f| f.reason)
    }

    /// Whether another page may be asked about in frame `frame`, of `most`
    /// a frame; counts it.
    pub(crate) fn ask(&mut self, frame: u64, most: usize) -> bool {
        if self.frame != frame {
            (self.frame, self.asked) = (frame, 0);
        }
        if self.asked >= most {
            return false;
        }
        self.asked += 1;
        true
    }

    pub(crate) fn put(&mut self, page: u64, frame: u64, reason: Reason) {
        let wait = match self.found.get(&page) {
            Some(f) if f.reason == reason => (f.wait * 2).min(ASK_AGAIN_MOST),
            // Spread by the page, so pages found in one frame come up
            // again over the frames after and not together.
            _ => ASK_AGAIN + (page.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) % ASK_AGAIN,
        };
        if self.found.len() >= CACHE && !self.found.contains_key(&page) {
            // Full: the eighth asked about longest ago makes room.
            let mut oldest: Vec<(u64, u64)> = self.found.iter().map(|(&p, f)| (f.at, p)).collect();
            let n = CACHE / 8;
            oldest.select_nth_unstable(n);
            for &(_, p) in &oldest[..n] {
                self.found.remove(&p);
            }
        }
        self.found.insert(page, Found { reason, at: frame, wait });
    }

    /// Page `page` got or lost its copy: what was found no longer holds.
    pub(crate) fn forget(&mut self, page: u64) {
        self.found.remove(&page);
    }
}

/// What [`Pager::want`] decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Admission {
    /// The page is held already.
    Held,
    /// The page is taken: the caller makes its copy, or says
    /// [`Pager::failed`].
    Admit,
    /// The page's run has no buffer and every buffer allowed is in use.
    Full,
    /// Enough pages or runs were taken this frame, or host memory was
    /// refused a moment ago.
    Later,
}

/// What [`Pager::gone`] says of a copy that is gone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Gone {
    /// The page's place in its run's buffer holds no copy now: it is to be
    /// zeroed, so that an access running into it from the page before reads
    /// 0 and not what the copy last held.
    pub(crate) clear: bool,
    /// The run whose buffer holds no copy at all now and is to be freed.
    pub(crate) free: Option<u64>,
}

/// What is held on demand ([`Pager::held`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Holding {
    /// Pages held, and the runs (buffers) in use.
    pub(crate) pages: usize,
    pub(crate) runs: usize,
    /// Copies of pages given up that something still holds.
    pub(crate) leaving: usize,
}

/// The pages held on demand, by run. A run is in use, and has its buffer,
/// from its first page until the last copy in it is gone: copies of pages
/// given up live until every table that named them is rewritten, and keep
/// the buffer with them. Such a page may be held again meanwhile, at the
/// same place (the old copy is named by no table any submission uses), so a
/// page given up costs no host memory beyond its run's buffer.
pub(crate) struct Pager {
    limit: usize,
    per_frame: usize,
    runs_per_frame: usize,
    /// Pages and runs taken this frame.
    admitted: usize,
    started: usize,
    /// Frames left in which nothing is taken ([`STALL`]).
    stalled: u64,
    /// Held pages (page numbers).
    pages: BTreeSet<u64>,
    /// Copies of pages given up that are not gone yet, by page.
    leaving: BTreeMap<u64, u32>,
    /// Runs in use: held pages and leaving copies in each.
    runs: BTreeMap<u64, u32>,
    /// Where [`Pager::sweep`] goes on.
    cursor: u64,
}

impl Pager {
    /// Pages in at most `limit` runs; `per_frame` pages and
    /// `runs_per_frame` new runs taken in one frame.
    pub(crate) fn new(limit: usize, per_frame: usize, runs_per_frame: usize) -> Self {
        Self {
            limit,
            per_frame,
            runs_per_frame,
            admitted: 0,
            started: 0,
            stalled: 0,
            pages: BTreeSet::new(),
            leaving: BTreeMap::new(),
            runs: BTreeMap::new(),
            cursor: 0,
        }
    }

    /// Whether missed page `page` (a page number) is taken.
    pub(crate) fn want(&mut self, page: u64) -> Admission {
        if self.pages.contains(&page) {
            return Admission::Held;
        }
        if self.stalled > 0 || self.admitted >= self.per_frame {
            return Admission::Later;
        }
        if !self.runs.contains_key(&run_of(page)) {
            if self.runs.len() >= self.limit {
                return Admission::Full;
            }
            if self.started >= self.runs_per_frame {
                return Admission::Later;
            }
            self.started += 1;
        }
        self.take(page);
        Admission::Admit
    }

    /// Takes page `page` because the page before it in its run was just
    /// taken, whatever the frame's count: its run has its buffer, so it
    /// costs a first copy only. False if it is held already or its run is
    /// not in use.
    pub(crate) fn follow(&mut self, page: u64) -> bool {
        if self.pages.contains(&page) || !self.runs.contains_key(&run_of(page)) {
            return false;
        }
        self.take(page);
        true
    }

    fn take(&mut self, page: u64) {
        self.pages.insert(page);
        *self.runs.entry(run_of(page)).or_default() += 1;
        self.admitted += 1;
    }

    /// One fewer page or copy in `run`: the run, if nothing is left in it.
    fn leave(&mut self, run: u64) -> Option<u64> {
        let n = self.runs.get_mut(&run)?;
        *n -= 1;
        (*n == 0).then(|| {
            self.runs.remove(&run);
            run
        })
    }

    /// The copy of page `page`, just taken, could not be made: it is not
    /// held (the frame's count is not given back: the attempt cost what a
    /// page costs). `stall`: host memory was refused, and nothing is taken
    /// for [`STALL`] frames. The page's run, if nothing is left in it (its
    /// buffer, if one was made, is to be freed).
    pub(crate) fn failed(&mut self, page: u64, stall: bool) -> Option<u64> {
        if stall {
            self.stalled = STALL;
        }
        self.pages.remove(&page).then(|| self.leave(run_of(page))).flatten()
    }

    /// Page `page` is given up: its copy lives until [`Pager::gone`].
    /// Whether the page was held.
    pub(crate) fn release(&mut self, page: u64) -> bool {
        let held = self.pages.remove(&page);
        if held {
            *self.leaving.entry(page).or_default() += 1;
        }
        held
    }

    /// A copy of page `page` that was given up is gone.
    pub(crate) fn gone(&mut self, page: u64) -> Gone {
        let Some(n) = self.leaving.get_mut(&page) else {
            // Dropped while still held (the whole state is going): nothing
            // to free twice.
            return Gone::default();
        };
        *n -= 1;
        if *n == 0 {
            self.leaving.remove(&page);
        }
        let free = self.leave(run_of(page));
        Gone {
            clear: free.is_none() && !self.pages.contains(&page) && !self.leaving.contains_key(&page),
            free,
        }
    }

    /// Whether page `page` is held.
    pub(crate) fn holds(&self, page: u64) -> bool {
        self.pages.contains(&page)
    }

    /// A frame ended.
    pub(crate) fn frame(&mut self) {
        (self.admitted, self.started) = (0, 0);
        self.stalled = self.stalled.saturating_sub(1);
    }

    /// The next `n` held pages in turn, by address, round and round.
    pub(crate) fn sweep(&mut self, n: usize) -> Vec<u64> {
        let n = n.min(self.pages.len());
        let pages: Vec<u64> = self.pages.range(self.cursor..).chain(self.pages.range(..self.cursor)).take(n).copied().collect();
        if let Some(&last) = pages.last() {
            self.cursor = last + 1;
        }
        pages
    }

    pub(crate) fn held(&self) -> Holding {
        Holding {
            pages: self.pages.len(),
            runs: self.runs.len(),
            leaving: self.leaving.values().map(|&n| n as usize).sum(),
        }
    }

    /// The most runs in use.
    pub(crate) fn limit(&self) -> usize {
        self.limit
    }
}

/// What [`Probes::note`] did with a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Probed {
    /// It has a probe already.
    Has,
    /// It got one (the tables are to be written again).
    Added,
    /// It gets none for now: every page looked at there was not title
    /// memory, and its probe went to another region.
    Aside,
    /// It gets none: every probe is in use by a region that may be title
    /// memory.
    Full,
}

struct Probe {
    l1: usize,
    /// The frame an access there last missed.
    last: u64,
    /// Missed pages there that were looked at, and those of them that were
    /// not title memory.
    looked: u32,
    stray: u32,
}

/// The 256 MiB regions given an empty block ([`write_table`]'s `probes`):
/// those a shader reached that no copy lies in. With a block, the next miss
/// there names its page, which can then be looked at and given a copy.
/// What the shader sees does not change: every entry of the block names the
/// null views.
///
/// At most [`PROBES`]. When all are in use a new region takes the probe of
/// one not missed for [`PROBE_IDLE`] frames, else of one where every page
/// looked at was not title memory (stray addresses: such a region is set
/// aside for [`PROBE_AGAIN`] frames, so that strays do not take the probe
/// back in the next frame), else it gets none and that is counted.
#[derive(Default)]
pub(crate) struct Probes {
    regions: Vec<Probe>,
    /// Regions set aside as stray, and the frame it was done.
    aside: BTreeMap<usize, u64>,
}

impl Probes {
    /// An access in region `l1` with no block missed in frame `frame`.
    pub(crate) fn note(&mut self, l1: usize, frame: u64) -> Probed {
        if l1 >= LEVEL1 {
            return Probed::Full;
        }
        if let Some(region) = self.regions.iter_mut().find(|r| r.l1 == l1) {
            region.last = frame;
            return Probed::Has;
        }
        if let Some(&since) = self.aside.get(&l1) {
            if frame.saturating_sub(since) < PROBE_AGAIN {
                return Probed::Aside;
            }
            self.aside.remove(&l1);
        }
        let new = Probe {
            l1,
            last: frame,
            looked: 0,
            stray: 0,
        };
        if self.regions.len() < PROBES {
            self.regions.push(new);
            return Probed::Added;
        }
        // Full: the region idle longest makes way, if idle long enough.
        if let Some(oldest) = self.regions.iter_mut().min_by_key(|r| r.last)
            && frame.saturating_sub(oldest.last) >= PROBE_IDLE
        {
            *oldest = new;
            return Probed::Added;
        }
        // Else the region most surely stray.
        let stray = self
            .regions
            .iter_mut()
            .filter(|r| r.looked >= PROBE_STRAY && r.stray == r.looked)
            .max_by_key(|r| (r.looked, std::cmp::Reverse(r.l1)));
        match stray {
            Some(region) => {
                self.aside.insert(region.l1, frame);
                *region = new;
                Probed::Added
            }
            None => Probed::Full,
        }
    }

    /// A page of region `l1` missed in frame `frame`: its probe is in use.
    /// `stray`: the page was looked at, and whether it is not title memory.
    pub(crate) fn touch(&mut self, l1: usize, frame: u64, stray: Option<bool>) {
        if let Some(region) = self.regions.iter_mut().find(|r| r.l1 == l1) {
            region.last = frame;
            if let Some(stray) = stray {
                region.looked = region.looked.saturating_add(1);
                region.stray = region.stray.saturating_add(u32::from(stray));
            }
        }
    }

    pub(crate) fn list(&self) -> Vec<usize> {
        self.regions.iter().map(|r| r.l1).collect()
    }
}

/// Why a page on demand is given up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GivenUp {
    /// Another copy holds the page whole.
    Whole,
    /// Another copy holds a part of the page and the table names it.
    Part,
    /// The title gave the memory up.
    Vanished,
}

/// Misses and what became of them over a period of frames.
#[derive(Default)]
pub(crate) struct Tally {
    /// Frames in the period, those with a miss, and those whose writes
    /// through title memory were withheld.
    pub(crate) frames: u64,
    pub(crate) missing: u64,
    pub(crate) withheld: u64,
    /// Misses by [`Reason`] (in [`Reason::ALL`]'s order): a page counts once
    /// for each list of each submission that missed it.
    reasons: [u64; Reason::ALL.len()],
    /// Pages the system was asked about, and the microseconds that took.
    pub(crate) asked: u64,
    pub(crate) ask_micros: u64,
    /// Region misses whose region got no probe: all in use, or set aside.
    pub(crate) unprobed: u64,
    pub(crate) aside: u64,
    /// Pages given a copy for a miss, and after such a page in its run;
    /// refused because every buffer was in use, put off to a later frame,
    /// failed for want of host memory, and refused by the change tracker.
    pub(crate) admitted: u64,
    pub(crate) followed: u64,
    pub(crate) full: u64,
    pub(crate) later: u64,
    pub(crate) failed: u64,
    pub(crate) untracked: u64,
    /// Pages given up: another copy holds them whole, or a part of them;
    /// the title gave the memory up.
    pub(crate) superseded: u64,
    pub(crate) shared: u64,
    pub(crate) vanished: u64,
    /// Pages shaders wrote by address whose writes were lost: the title
    /// wrote the page during the submission, and its bytes were kept.
    pub(crate) kept_titles: u64,
    /// Written pages in a root UAV's range that went back by page because
    /// a page on demand held them ([`Marks::precise_paged`]).
    pub(crate) precise_paged: u64,
    /// Draws and dispatches using title memory in lists that wrote through
    /// the table and had those writes withheld, by pipeline (at most
    /// [`Tally::NAMES`], the rest together).
    lost: BTreeMap<String, u64>,
    lost_others: u64,
    /// Misses by 256 MiB region (level-1 index).
    regions: BTreeMap<usize, u64>,
    /// Whether the frame being counted had a miss.
    missed_now: bool,
}

/// What a misses line says besides the tally: the state when it is written.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Now {
    /// Whether pages on demand are on, what is held and the limit in runs.
    pub(crate) on: bool,
    pub(crate) held: Holding,
    pub(crate) limit: usize,
    /// [`Written::cut`] and [`Written::partial`] of the table written last.
    pub(crate) cut: usize,
    pub(crate) partial: usize,
}

impl Tally {
    /// Pipelines counted by name.
    pub(crate) const NAMES: usize = 32;

    /// An access missed for `reason`, in region `l1` if that is known.
    pub(crate) fn miss(&mut self, reason: Reason, l1: Option<usize>) {
        self.reasons[reason as usize] += 1;
        self.missed_now = true;
        if let Some(l1) = l1 {
            *self.regions.entry(l1).or_default() += 1;
        }
    }

    /// `n` draws or dispatches of `pipeline` were in a list whose writes
    /// through the table were withheld.
    pub(crate) fn lost(&mut self, pipeline: &str, n: u64) {
        if let Some(count) = self.lost.get_mut(pipeline) {
            *count += n;
        } else if self.lost.len() < Self::NAMES {
            self.lost.insert(pipeline.to_owned(), n);
        } else {
            self.lost_others += n;
        }
    }

    /// A frame ended; `withheld`: its writes through title memory stayed
    /// out of title memory.
    pub(crate) fn frame(&mut self, withheld: bool) {
        self.frames += 1;
        self.missing += u64::from(std::mem::take(&mut self.missed_now));
        self.withheld += u64::from(withheld);
    }

    pub(crate) fn count(&self, reason: Reason) -> u64 {
        self.reasons[reason as usize]
    }

    /// Whether the period has anything to report.
    pub(crate) fn any(&self) -> bool {
        self.reasons.iter().any(|&n| n > 0)
            || self.admitted + self.followed + self.full + self.later + self.failed + self.untracked > 0
            || self.superseded + self.shared + self.vanished > 0
            || self.kept_titles + self.precise_paged + self.lost_others > 0
            || !self.lost.is_empty()
    }

    /// The period's line: misses by reason, what looking at them cost, what
    /// the switch did with them and what is held, whose writes were
    /// withheld, and the regions missed most (their first address;
    /// addresses are not listed singly).
    pub(crate) fn line(&self, now: Now) -> String {
        use std::fmt::Write;
        let mut text = format!(
            "title memory misses over {} frames ({} with a miss, {} with writes withheld):",
            self.frames, self.missing, self.withheld
        );
        let mut sep = " ";
        for reason in Reason::ALL {
            let n = self.count(reason);
            if n > 0 {
                let _ = write!(text, "{sep}{n} {}", reason.name());
                sep = ", ";
            }
        }
        if sep == " " {
            text.push_str(" none");
        }
        if self.asked > 0 {
            let _ = write!(text, "; {} pages looked at in {:.2} ms", self.asked, self.ask_micros as f64 / 1e3);
        }
        if self.unprobed + self.aside > 0 {
            let _ = write!(
                text,
                "; of the region misses {} were in regions refused a block (all {PROBES} in use) and {} in regions \
                 set aside (every page looked at there was not title memory): their pages are not named",
                self.unprobed, self.aside
            );
        }
        if now.on {
            let _ = write!(
                text,
                "; pages on demand: {} given a copy for a miss and {} after them in their 4 MiB, {} refused (all {} \
                 buffers in use), {} put off, {} failed for want of host memory, {} refused by the change tracker, \
                 {} given up to a copy holding the page, {} to a copy holding part of it, {} given up with their \
                 memory; {} held in {} buffers ({:.1} MB), {} old copies not yet freed; {} held pages end where the \
                 next page is not beside them (an access running past reads 0 there and marks nothing)",
                self.admitted,
                self.followed,
                self.full,
                now.limit,
                self.later,
                self.failed,
                self.untracked,
                self.superseded,
                self.shared,
                self.vanished,
                now.held.pages,
                now.held.runs,
                worst_case_bytes(now.held.runs) as f64 / 1e6,
                now.held.leaving,
                now.cut
            );
        } else {
            let pageable = self.count(Reason::Loose) + self.count(Reason::Buffer);
            let _ = write!(text, "; pages on demand off (VICTORY_MEMORY_PAGING=1 would give {pageable} of these misses a copy)");
        }
        let _ = write!(
            text,
            "; the table names {} pages for a copy holding part of them (the rest of such a page reads 0 and marks nothing)",
            now.partial
        );
        if self.kept_titles > 0 {
            let _ = write!(
                text,
                "; {} pages shaders wrote kept the title's bytes (the title wrote them during the submission)",
                self.kept_titles
            );
        }
        if self.precise_paged > 0 {
            let _ = write!(
                text,
                "; {} written pages in a root UAV's range went back by page (a page on demand held them)",
                self.precise_paged
            );
        }
        let lost: u64 = self.lost.values().sum::<u64>() + self.lost_others;
        if lost > 0 {
            let mut names: Vec<(&String, &u64)> = self.lost.iter().collect();
            names.sort_by_key(|&(name, &n)| (std::cmp::Reverse(n), name));
            let listed: Vec<String> = names.iter().take(8).map(|(name, n)| format!("{n} {name}")).collect();
            let _ = write!(
                text,
                "; {lost} draws/dispatches were in lists whose writes through the table were withheld ({}{}): a \
                 program the title runs only once has lost its output",
                listed.join(", "),
                if names.len() > 8 || self.lost_others > 0 { ", ..." } else { "" }
            );
        }
        let mut regions: Vec<(usize, u64)> = self.regions.iter().map(|(&l1, &n)| (l1, n)).collect();
        regions.sort_by_key(|&(l1, n)| (std::cmp::Reverse(n), l1));
        if !regions.is_empty() {
            let listed: Vec<String> =
                regions.iter().take(6).map(|&(l1, n)| format!("{:#x} x{n}", (l1 as u64) << 28)).collect();
            let _ = write!(text, "; 256 MiB regions missed most: {} ({} in all)", listed.join(", "), regions.len());
        }
        text
    }
}

/// When the tally is logged: each of the first frames with something to
/// report on its own, then one line for every [`Report::PERIOD`] frames
/// that had any.
#[derive(Default)]
pub(crate) struct Report {
    alone: u64,
}

impl Report {
    pub(crate) const ALONE: u64 = 8;
    pub(crate) const PERIOD: u64 = 60;

    /// Whether `tally` is to be logged now (and started again).
    pub(crate) fn due(&mut self, tally: &Tally) -> bool {
        if !tally.any() {
            // Nothing to say: a quiet period is not kept growing.
            return tally.frames >= Self::PERIOD;
        }
        if self.alone < Self::ALONE {
            self.alone += 1;
            return true;
        }
        tally.frames >= Self::PERIOD
    }
}

/// The kinds of miss already said once: a line for each new pair of reason
/// and 256 MiB region, up to [`Seen::MOST`].
#[derive(Default)]
pub(crate) struct Seen {
    kinds: BTreeSet<(Reason, usize)>,
}

impl Seen {
    pub(crate) const MOST: usize = 256;

    /// Whether this is the first miss for `reason` in region `l1` (and one
    /// of the first [`Seen::MOST`] kinds).
    pub(crate) fn first(&mut self, reason: Reason, l1: usize) -> bool {
        self.kinds.len() < Self::MOST && self.kinds.insert((reason, l1))
    }
}

/// Why [`Host::make`] made no copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unmade {
    /// No host memory for the run's buffer.
    NoMemory,
    /// The change tracker does not track all of the page after all.
    Untracked,
}

/// What [`State`] asks of the system and the device (`crate::memory` on a
/// device; the tests' own in the tests).
pub(crate) trait Host {
    /// What is known of the page at address `at` from the system and the
    /// other copies: whether a copy holds it, whether it can be read,
    /// whether the change tracker tracks it, whether a title buffer holds
    /// it. Asks the system only as far as the answer needs.
    fn facts(&mut self, at: u64) -> Facts;
    /// Makes the copy of page `page` (a page number), and its run's buffer
    /// if this is the run's first page.
    fn make(&mut self, page: u64) -> Result<(), Unmade>;
    /// Run `run` holds no copy: its buffer, if it has one, is freed.
    fn free(&mut self, run: u64);
    /// Page `page`'s place in its run's buffer holds no copy that a table
    /// in use names: zeroed.
    fn clear(&mut self, page: u64);
}

/// Everything about misses and pages on demand that is decided without the
/// device: `crate::memory` holds one, with the copies and buffers beside it.
pub(crate) struct State {
    /// `VICTORY_MEMORY_PAGING`: a missed page of title memory gets a copy.
    /// Off, misses are only looked at and counted.
    on: bool,
    /// Regions with no copy get an empty block so that their misses name
    /// the page (off with `VICTORY_MEMORY_PROBES=0`: misses there then stay
    /// counted by region, and no page of such a region gets a copy).
    probe: bool,
    /// Missed pages are looked at (off with `VICTORY_MEMORY_MISS_REASONS=0`
    /// while pages on demand are off: every missed page is then counted as
    /// not looked at and the system is asked nothing).
    look: bool,
    pager: Pager,
    probes: Probes,
    cache: Cache,
    tally: Tally,
    report: Report,
    seen: Seen,
    /// Pipelines whose withheld writes were said once.
    said: BTreeSet<String>,
    /// Frames ended.
    frame: u64,
    /// Of the table written last ([`State::table`]).
    cut: usize,
    partial: usize,
}

impl State {
    /// `megabytes`: the most host memory for pages on demand.
    pub(crate) fn new(on: bool, probe: bool, look: bool, megabytes: u64) -> Self {
        State {
            on,
            probe,
            look: look || on,
            pager: Pager::new(limit_runs(megabytes), ADMIT_PER_FRAME, RUNS_PER_FRAME),
            probes: Probes::default(),
            cache: Cache::default(),
            tally: Tally::default(),
            report: Report::default(),
            seen: Seen::default(),
            said: BTreeSet::new(),
            frame: 0,
            cut: 0,
            partial: 0,
        }
    }

    /// Turns pages on demand on or off (the device's tests).
    #[cfg(all(test, windows))]
    pub(crate) fn set_on(&mut self, on: bool) {
        self.on = on;
        self.look |= on;
    }

    /// Whether page `page` (a page number) is held on demand.
    #[cfg(test)]
    pub(crate) fn holds(&self, page: u64) -> bool {
        self.pager.holds(page)
    }

    /// The most runs in use.
    pub(crate) fn limit(&self) -> usize {
        self.pager.limit()
    }

    /// What is held.
    #[cfg(test)]
    pub(crate) fn held(&self) -> Holding {
        self.pager.held()
    }

    /// The regions whose misses are to name their page.
    pub(crate) fn probes(&self) -> Vec<usize> {
        self.probes.list()
    }

    /// A table was written: its [`Written::cut`] and [`Written::partial`].
    pub(crate) fn table(&mut self, cut: usize, partial: usize) {
        (self.cut, self.partial) = (cut, partial);
    }

    /// Pages a table names for a copy holding part of them (the table
    /// written last).
    pub(crate) fn partial(&self) -> usize {
        self.partial
    }

    fn now(&self) -> Now {
        Now {
            on: self.on,
            held: self.pager.held(),
            limit: self.pager.limit(),
            cut: self.cut,
            partial: self.partial,
        }
    }

    /// Counts a miss, and says a kind of miss the first time it is seen.
    fn miss(&mut self, reason: Reason, l1: usize) {
        self.tally.miss(reason, Some(l1));
        if self.seen.first(reason, l1) {
            tracing::info!(
                "title memory: first miss of this kind: {}, in the 256 MiB at {:#x}",
                reason.name(),
                (l1 as u64) << 28
            );
        }
    }

    /// Asks `host` about the page at `at`, counting the call and its time.
    fn ask(&mut self, at: u64, host: &mut impl Host) -> Reason {
        let started = std::time::Instant::now();
        let facts = host.facts(at);
        self.tally.asked += 1;
        self.tally.ask_micros += started.elapsed().as_micros() as u64;
        classify(facts)
    }

    /// Why missed page `page` has no copy the table named: what is known
    /// already, else what `host` finds, if looking is on and the frame has
    /// asks left.
    fn look_at(&mut self, page: u64, host: &mut impl Host) -> Reason {
        if self.pager.holds(page) {
            // Held on demand since the table was written.
            return Reason::Unnamed;
        }
        if let Some(reason) = self.cache.get(page, self.frame) {
            return reason;
        }
        let most = if self.on { ADMIT_PER_FRAME } else { ASK_PER_FRAME };
        if !self.look || !self.cache.ask(self.frame, most) {
            return Reason::Unasked;
        }
        let reason = self.ask(page << 16, host);
        self.cache.put(page, self.frame, reason);
        reason
    }

    /// After a list ran: its misses ([`Marks`]). Each is counted by what it
    /// is; a region gets a block so that its next miss names the page; and
    /// with the switch a page that is title memory gets a copy, which the
    /// next submission's table names. Whether the tables are to be written
    /// again (a probe or a copy was added).
    pub(crate) fn note_misses(&mut self, m: &Marks, host: &mut impl Host) -> bool {
        let frame = self.frame;
        for _ in 0..m.above {
            self.tally.miss(Reason::Above, None);
        }
        for _ in 0..m.stray {
            self.tally.miss(Reason::Region, None);
        }
        self.tally.precise_paged += m.precise_paged;
        let mut changed = false;
        for &l1 in &m.regions {
            self.miss(Reason::Region, l1);
            if !self.probe {
                continue;
            }
            match self.probes.note(l1, frame) {
                Probed::Has => {}
                Probed::Added => changed = true,
                Probed::Aside => self.tally.aside += 1,
                Probed::Full => {
                    self.tally.unprobed += 1;
                    static ONCE: std::sync::Once = std::sync::Once::new();
                    ONCE.call_once(|| {
                        tracing::warn!(
                            "title memory: a 256 MiB region a shader reached gets no page-table block for its \
                             misses: all {PROBES} are in use by regions that may be title memory. Its misses stay \
                             counted by region and no page there gets a copy (logged once; the misses line counts them)"
                        )
                    });
                }
            }
        }
        for &at in &m.missed {
            let (page, l1) = (at >> 16, (at >> 28) as usize);
            let reason = self.look_at(page, host);
            self.probes.touch(l1, frame, reason.stray());
            self.miss(reason, l1);
            if self.on && reason.pageable() {
                changed |= self.admit(page, host);
            }
            // Its place may hold what a write running past the end of the
            // held page before it left there (that write is this miss): it
            // reads 0 again, as a place with no copy does.
            if !page.is_multiple_of(RUN_PAGES) && self.pager.holds(page - 1) && !self.pager.holds(page) {
                host.clear(page);
            }
        }
        changed
    }

    /// Gives missed page `page` a copy if its run has or can have a buffer,
    /// and with it the pages after it in its run, up to the first that
    /// cannot have one (held by another copy, not title memory, not
    /// tracked) or is held already: those lie beside it in the buffer, so an
    /// access running from one into the next finds its bytes. Whether a
    /// copy was made (the tables are to name it).
    fn admit(&mut self, page: u64, host: &mut impl Host) -> bool {
        match self.pager.want(page) {
            Admission::Held => return false,
            Admission::Full => {
                self.tally.full += 1;
                return false;
            }
            Admission::Later => {
                self.tally.later += 1;
                return false;
            }
            Admission::Admit => {}
        }
        if !self.make(page, host) {
            return false;
        }
        self.tally.admitted += 1;
        let end = (run_of(page) + 1) * RUN_PAGES;
        for next in page + 1..end {
            if self.pager.holds(next) {
                break;
            }
            let reason = self.ask(next << 16, host);
            if !reason.pageable() {
                // Known now, should it be missed itself.
                self.cache.put(next, self.frame, reason);
                break;
            }
            if !self.pager.follow(next) || !self.make(next, host) {
                break;
            }
            self.tally.followed += 1;
        }
        true
    }

    /// Has `host` make the copy of page `page`, which the pager just took.
    fn make(&mut self, page: u64, host: &mut impl Host) -> bool {
        match host.make(page) {
            Ok(()) => {
                self.cache.forget(page);
                true
            }
            Err(why) => {
                // It stays a miss. Without host memory nothing is tried for
                // a while (each try may ask for a buffer), and what was
                // found of the page stands; a page the tracker refuses is
                // not the title's GPU memory after all.
                if let Some(run) = self.pager.failed(page, why == Unmade::NoMemory) {
                    host.free(run);
                }
                match why {
                    Unmade::NoMemory => self.tally.failed += 1,
                    Unmade::Untracked => {
                        self.tally.untracked += 1;
                        self.cache.put(page, self.frame, Reason::Foreign);
                    }
                }
                false
            }
        }
    }

    /// Page `page` (a page number) held on demand is given up for `why`.
    /// Whether it was held: the caller then lets go of its copy, which
    /// lives until the tables naming it are rewritten ([`State::gone`]).
    pub(crate) fn give_up(&mut self, page: u64, why: GivenUp) -> bool {
        if !self.pager.release(page) {
            return false;
        }
        self.cache.forget(page);
        match why {
            GivenUp::Whole => self.tally.superseded += 1,
            GivenUp::Part => self.tally.shared += 1,
            GivenUp::Vanished => self.tally.vanished += 1,
        }
        true
    }

    /// A copy of page `page` that was given up is gone: its place is
    /// zeroed, or its run's buffer freed with its last copy.
    pub(crate) fn gone(&mut self, page: u64, host: &mut impl Host) {
        let gone = self.pager.gone(page);
        if let Some(run) = gone.free {
            host.free(run);
        } else if gone.clear {
            host.clear(page);
        }
    }

    /// A list that wrote through the table had its writes withheld (it
    /// missed, or a list missed this frame or the one before): `uses`, its
    /// draws and dispatches using title memory by pipeline. Counted, and
    /// each pipeline said once: the list does not run again, so what a
    /// program run only once wrote is lost.
    pub(crate) fn note_withheld<'a>(&mut self, uses: impl IntoIterator<Item = (&'a str, u64)>) {
        for (pipeline, n) in uses {
            self.tally.lost(pipeline, n);
            if self.said.len() < Tally::NAMES && !self.said.contains(pipeline) {
                self.said.insert(pipeline.to_owned());
                tracing::info!(
                    "title memory: the writes of a list using pipeline {pipeline} were withheld (first time for \
                     this pipeline, frame {}): the list does not run again, so if the title runs this program \
                     only once its output is lost",
                    self.frame
                );
            }
        }
    }

    /// A frame ended (`withheld`: its writes through title memory stayed
    /// out of title memory; `kept`: pages shaders wrote that kept the
    /// title's bytes): the misses line if one is due, and the held pages
    /// to check now for memory the title gave up.
    pub(crate) fn frame_end(&mut self, withheld: bool, kept: u64) -> (Option<String>, Vec<u64>) {
        self.tally.kept_titles += kept;
        self.tally.frame(withheld);
        let mut line = None;
        if self.report.due(&self.tally) {
            let tally = std::mem::take(&mut self.tally);
            if tally.any() {
                line = Some(tally.line(self.now()));
            }
        }
        self.pager.frame();
        self.frame += 1;
        (line, self.pager.sweep(SWEEP))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(copies: &[(u64, u64)], pairs: &[u32]) -> Vec<Named> {
        copies
            .iter()
            .zip(pairs)
            .map(|(&(start, len), &pair)| Named {
                start,
                len,
                pair,
                view_start: start as u32,
                yields: false,
            })
            .collect()
    }

    /// A page on demand at `start`, seen through its run's view pair `pair`.
    fn on_demand(start: u64, pair: u32) -> Named {
        Named {
            start,
            len: PAGE,
            pair,
            view_start: (start & !(RUN_BYTES - 1)) as u32,
            yields: true,
        }
    }

    fn table(copies: &[Named], probes: &[usize], null: u32) -> (Vec<u32>, Written) {
        let mut dwords = vec![0u32; TABLE_DWORDS];
        let written = write_table(copies, probes, null, &mut dwords);
        dwords.truncate(written.dwords);
        (dwords, written)
    }

    /// The shader's lookup (`victory_shader` lowering) over a table's
    /// dwords: the view index and the byte offset in that view. An access
    /// is served by one lookup: its dwords follow at `+ 4k`.
    fn look_up(dwords: &[u32], address: u64) -> (u32, u32) {
        let (lo, hi) = (address as u32, (address >> 32) as u32);
        let inside = hi < 256;
        let l1 = if inside { hi << 4 | lo >> 28 } else { 0 };
        let block = if inside { dwords[l1 as usize] } else { 0 };
        let entry = block << 12 | (lo >> 16 & 0xFFF);
        let at = 4096 + 2 * entry as usize;
        (dwords[at], lo.wrapping_sub(dwords[at + 1]))
    }

    /// The bit the shader marks for an access at `address` that misses, or
    /// that writes (`victory_shader` lowering, `Lower::memory`).
    fn marked_bit(dwords: &[u32], address: u64) -> usize {
        let (lo, hi) = (address as u32, (address >> 32) as u32);
        let inside = hi < 256;
        let l1 = if inside { hi << 4 | lo >> 28 } else { 0 };
        let block = if inside { dwords[l1 as usize] } else { 0 };
        let entry = block << 12 | (lo >> 16 & 0xFFF);
        let miss = dwords[4096 + 2 * entry as usize] == dwords[4096];
        (if miss && block == 0 { l1 } else { entry }) as usize
    }

    fn entry_of(blocks: &[usize], address: u64) -> usize {
        blocks.iter().position(|&l| l == (address >> 28) as usize).unwrap() * BLOCK + (address >> 16 & 0xFFF) as usize
    }

    fn set(words: &mut [u32], bit: usize) {
        words[bit / 32] |= 1 << (bit % 32);
    }

    /// Numbers for the tests that try many cases: the same every run.
    struct Dice(u64);

    impl Dice {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[test]
    fn addresses_find_the_copy_holding_them() {
        // A 4 MiB window, a buffer inside it from 0x40_0000+0x1_8000
        // (pages in part), and a buffer above 4 GiB.
        let copies = named(&[(0x40_0000, 0x40_0000), (0x41_8000, 0x3_0000), (0xd_1234_0000, 0x2_0000)], &[102, 104, 106]);
        let (dwords, w) = table(&copies, &[], 100);
        // Window, whole pages of the inner buffer, the high buffer.
        assert_eq!(look_up(&dwords, 0x40_0010), (102, 0x10));
        assert_eq!(look_up(&dwords, 0x42_0004), (104, 0x8004));
        assert_eq!(look_up(&dwords, 0xd_1235_fffc), (106, 0x1_fffc));
        // Pages the inner buffer holds in part stay the window's.
        assert_eq!(look_up(&dwords, 0x41_8000), (102, 0x1_8000));
        assert_eq!(look_up(&dwords, 0x44_0000), (102, 0x4_0000));
        // No copy, and above 2^40: the null pair, which reads nothing.
        assert_eq!(look_up(&dwords, 0x9000_0000).0, 100);
        assert_eq!(look_up(&dwords, 0x200_0000_0000).0, 100);
        // Entries map back to their copy and page.
        assert_eq!(w.owner[entry_of(&w.blocks, 0x42_0000)], 1);
        assert_eq!(w.owner[entry_of(&w.blocks, 0xd_1234_0000)], 2);
        assert_eq!(w.blocks.len(), 3, "null, low 256 MiB, the high buffer's");
        assert!(w.superseded.is_empty());
        assert_eq!((w.partial, w.cut), (0, 0), "every named page is held whole by its copy");
    }

    /// Copies in more regions than a table has room for: the largest are
    /// named, the rest read as missing; the table fits its buffer.
    #[test]
    fn a_full_table_names_the_largest_copies() {
        let ranges: Vec<(u64, u64)> = (0..CAPACITY as u64 + 2).map(|k| (k << 28, 0x1_0000 + k)).collect();
        let pairs: Vec<u32> = (0..ranges.len() as u32).map(|k| 10 + 2 * k).collect();
        let (dwords, w) = table(&named(&ranges, &pairs), &[], 4);
        assert_eq!(w.blocks.len(), CAPACITY + 1);
        assert_eq!(dwords.len(), LEVEL1 + (CAPACITY + 1) * BLOCK * 2);
        assert_eq!(look_up(&dwords, (CAPACITY as u64 + 1) << 28).0, pairs[CAPACITY + 1]);
        assert_eq!(look_up(&dwords, 0).0, 4, "the smallest copy is left out");
    }

    #[test]
    fn table_skips_overflowing_and_out_of_range_copies() {
        let copies = named(&[(u64::MAX - 0x100, 0x200), (1u64 << 40, 0x1_0000), (0x40_0000, 0x1_0000)], &[102, 104, 106]);
        let (dwords, w) = table(&copies, &[], 100);
        assert_eq!(w.blocks, vec![usize::MAX, 0]);
        assert_eq!(look_up(&dwords, 0x40_0010), (106, 0x10));
        assert_eq!(look_up(&dwords, 0).0, 100);
        assert_eq!(w.owner[BLOCK + 0x40], 2);
    }

    #[test]
    fn has_missed_checks_block_zero_out_of_range_unowned_and_owned() {
        let mut owner = vec![NO_OWNER; 2 * BLOCK];
        owner[BLOCK + 5] = 0;
        let blocks = vec![usize::MAX, 0];
        let mut marks = vec![0u32; BITMAP_WORDS];
        assert!(!has_missed(&marks, &blocks, &owner));

        // Owned entry in block 1.
        let owned = BLOCK + 5;
        marks[owned / 32] = 1 << (owned % 32);
        assert!(!has_missed(&marks, &blocks, &owner));

        // Unowned entry in the same word as an owned entry.
        let unowned = BLOCK + 6;
        marks[unowned / 32] |= 1 << (unowned % 32);
        assert!(has_missed(&marks, &blocks, &owner));

        // Block 0 bit (missing 256 MiB region).
        marks.fill(0);
        marks[0] = 1 << 3;
        assert!(has_missed(&marks, &blocks, &owner));

        // Out-of-range block.
        marks.fill(0);
        let oob = 2 * BLOCK + 1;
        marks[oob / 32] = 1 << (oob % 32);
        assert!(has_missed(&marks, &blocks, &owner));
    }

    #[test]
    fn set_bits_are_listed_in_order() {
        let mut words = vec![0u32; 4];
        words[0] = 0x8000_0001;
        words[2] = 0b110;
        assert_eq!(set_bits(&words).collect::<Vec<_>>(), [0, 31, 65, 66]);
        assert_eq!(set_bits(&[0; 8]).count(), 0);
    }

    /// A copy made for a missed page has the page only while the table
    /// would name no other copy for it. A copy holding the page whole takes
    /// it, and so does one holding a part: the entry is then what it is
    /// without pages on demand (the part's copy, past whose end the page
    /// reads 0), and the page's copy is reported as no longer needed.
    #[test]
    fn a_page_on_demand_stands_back_for_every_copy_that_has_its_page() {
        // A buffer of 3.5 pages from 0x10_0000; pages on demand at its
        // second page (held whole), its last (held in part) and beside it.
        let mut copies = named(&[(0x10_0000, 0x3_8000)], &[50]);
        let (dwords, w) = table(&copies, &[], 4);
        assert_eq!(look_up(&dwords, 0x13_9000), (50, 0x3_9000), "past the copy's 0x3_8000 bytes: reads 0");
        assert_eq!(marked_bit(&dwords, 0x13_9000), entry_of(&w.blocks, 0x13_0000), "and a read there marks nothing: no miss");
        assert_eq!(w.partial, 1, "which is why such pages are counted");
        copies.push(on_demand(0x11_0000, 60));
        copies.push(on_demand(0x13_0000, 60));
        copies.push(on_demand(0x20_0000, 60));
        let (with, w) = table(&copies, &[], 4);
        assert_eq!(look_up(&with, 0x11_0008), (50, 0x1_0008), "the buffer's copy, which views use");
        assert_eq!(look_up(&with, 0x13_0008), (50, 0x3_0008), "the buffer's copy here too: one place for its bytes");
        assert_eq!(look_up(&with, 0x13_9000), look_up(&dwords, 0x13_9000), "as without pages on demand");
        assert_eq!(w.superseded, [(1, true), (2, false)]);
        assert_eq!(look_up(&with, 0x20_fffc), (60, 0x20_fffc), "at its own offset in its run's buffer");
        assert_eq!(look_up(&with, 0x21_0000).0, 4, "the next page is no one's");
        assert_eq!((w.partial, w.cut), (1, 1));
        // A far smaller copy than the page's that holds only a part of it
        // takes the page too.
        copies.push(Named {
            start: 0x20_0100,
            len: 0x100,
            pair: 70,
            view_start: 0x20_0100,
            yields: false,
        });
        let (dwords, w) = table(&copies, &[], 4);
        assert_eq!(look_up(&dwords, 0x20_0180), (70, 0x80));
        assert_eq!(w.superseded, [(1, true), (2, false), (3, false)]);
        assert_eq!((w.partial, w.cut), (2, 0));
        // And one of exactly the page.
        copies.push(Named {
            start: 0x20_0000,
            len: PAGE,
            pair: 72,
            view_start: 0x20_0000,
            yields: false,
        });
        let (dwords, w) = table(&copies, &[], 4);
        assert_eq!(look_up(&dwords, 0x20_0180), (72, 0x180));
        assert_eq!(w.superseded, [(1, true), (2, false), (3, true)]);
    }

    /// With pages on demand a table differs from the one without them only
    /// in entries that named no copy: whatever copies there are, every
    /// address a copy was named for finds the same view and offset.
    #[test]
    fn pages_on_demand_change_only_entries_that_named_no_copy() {
        let mut dice = Dice(0x2545_F491_4F6C_DD1D);
        // Pages on demand that got their page, and that stood back for a
        // copy holding it whole and for one holding a part.
        let (mut own, mut whole, mut part) = (0, 0, 0);
        for round in 0..40 {
            let mut copies = Vec::new();
            for k in 0..1 + dice.below(12) {
                let window = dice.below(4) == 0;
                let start = if window { dice.below(0x100) << 22 } else { dice.below(0x4_0000) << 12 };
                let len = if window { 4 << 20 } else { 0x100 + dice.below(0x30_0000) };
                copies.push(Named {
                    start,
                    len,
                    pair: 100 + 2 * k as u32,
                    view_start: start as u32,
                    yields: false,
                });
            }
            let (base, wb) = table(&copies, &[], 4);
            let first = copies.len();
            // Pages anywhere, and in and around the copies.
            let pages: BTreeSet<u64> = (0..1 + dice.below(60))
                .map(|_| {
                    if dice.below(3) == 0 {
                        return dice.below(0x4800);
                    }
                    let near = copies[dice.below(first as u64) as usize];
                    let (from, to) = (near.start >> 16, (near.start + near.len - 1) >> 16);
                    (from + dice.below(to - from + 3)).saturating_sub(1)
                })
                .collect();
            copies.extend(pages.iter().map(|&p| on_demand(p << 16, 1000 + 2 * run_of(p) as u32)));
            let (with, ww) = table(&copies, &[], 4);
            assert_eq!(ww.blocks[..wb.blocks.len()], wb.blocks[..], "round {round}: the copies' blocks are where they were");
            for (entry, &o) in wb.owner.iter().enumerate() {
                let at = LEVEL1 + 2 * entry;
                let page = (wb.blocks[entry / BLOCK] as u64) << 12 | (entry % BLOCK) as u64;
                match ww.owner[entry] {
                    now if now == o => assert_eq!(with[at..at + 2], base[at..at + 2]),
                    now => {
                        assert_eq!(o, NO_OWNER, "round {round}: page {page:#x} had a copy and has another");
                        assert!(copies[now as usize].yields && copies[now as usize].start >> 16 == page);
                    }
                }
            }
            // Each page on demand has its page, or is reported.
            for (k, copy) in copies.iter().enumerate().skip(first) {
                let owns = ww.owner[entry_of(&ww.blocks, copy.start)] == k as u32;
                let reported = ww.superseded.iter().find(|&&(s, _)| s == k);
                assert!(owns != reported.is_some(), "round {round}: page {:#x}", copy.start);
                own += usize::from(owns);
                whole += usize::from(reported.is_some_and(|r| r.1));
                part += usize::from(reported.is_some_and(|r| !r.1));
            }
        }
        assert!(own > 100 && whole > 100 && part > 10, "each case tried: {own} {whole} {part}");
    }

    /// The table's rule before pages on demand and probes, written out
    /// plainly: copies in order of size, the largest first; a copy takes a
    /// page it holds whole, or a page nothing has taken.
    fn plain_table(copies: &[Named], null: u32) -> Vec<u32> {
        let mut order: Vec<usize> = (0..copies.len()).collect();
        order.sort_by_key(|&k| std::cmp::Reverse(copies[k].len));
        let mut level1 = vec![0u32; LEVEL1];
        let mut entries = vec![[null, 0u32]; BLOCK];
        for k in order {
            let c = copies[k];
            for page in c.start >> 16..=(c.start + c.len - 1) >> 16 {
                let l1 = (page >> 12) as usize;
                if level1[l1] == 0 {
                    level1[l1] = (entries.len() / BLOCK) as u32;
                    entries.resize(entries.len() + BLOCK, [null, 0]);
                }
                let entry = &mut entries[level1[l1] as usize * BLOCK + (page & 0xFFF) as usize];
                let whole = page << 16 >= c.start && (page + 1) << 16 <= c.start + c.len;
                if whole || entry[0] == null {
                    *entry = [c.pair, c.start as u32];
                }
            }
        }
        level1.into_iter().chain(entries.into_iter().flatten()).collect()
    }

    /// With no page on demand the table is, dword for dword, what the rule
    /// before them gives. With probes every address still finds the same
    /// view and offset, and the host reads the same verdict (missed or not)
    /// from the bit a shader marks for it: only which bit differs.
    #[test]
    fn without_pages_on_demand_the_table_is_what_it_was() {
        let mut dice = Dice(0x8533_31E5_9A1C_6D2B);
        for round in 0..40 {
            let copies: Vec<Named> = (0..1 + dice.below(40))
                .map(|k| {
                    let window = dice.below(4) == 0;
                    let start = if window { dice.below(0x4000) << 22 } else { dice.below(0x400_0000) << 12 };
                    Named {
                        start,
                        len: if window { 4 << 20 } else { 0x40 + dice.below(0x30_0000) },
                        pair: 100 + 2 * k as u32,
                        view_start: start as u32,
                        yields: false,
                    }
                })
                .collect();
            let (plain, w0) = table(&copies, &[], 4);
            assert!(plain == plain_table(&copies, 4), "round {round}");
            let probes: Vec<usize> = (0..dice.below(100)).map(|_| dice.below(LEVEL1 as u64) as usize).collect();
            let (probed, w1) = table(&copies, &probes, 4);
            for _ in 0..2000 {
                // In and around the copies, in the probed regions, anywhere.
                let address = match dice.below(4) {
                    0 => dice.below(1 << 41),
                    1 if !probes.is_empty() => (probes[dice.below(probes.len() as u64) as usize] as u64) << 28 | dice.below(1 << 28),
                    _ => {
                        let near = copies[dice.below(copies.len() as u64) as usize];
                        (near.start + dice.below(near.len + 0x2_0000)).saturating_sub(0x1_0000)
                    }
                };
                assert_eq!(look_up(&probed, address), look_up(&plain, address), "round {round}: {address:#x}");
                // What `has_missed` says of a bitmap with that one bit.
                let verdict = |dwords: &[u32], w: &Written| {
                    !matches!(mark(marked_bit(dwords, address), &w.blocks, &w.owner), Mark::Written(..))
                };
                assert_eq!(verdict(&probed, &w1), verdict(&plain, &w0), "round {round}: {address:#x}");
            }
        }
    }

    /// Neighbouring pages on demand of one run lie side by side: an access
    /// that starts in one and ends in the next, served by one lookup, finds
    /// the next page's bytes. Where the next page is not beside it (not
    /// held, or in the next run) the page is counted as cut.
    #[test]
    fn neighbouring_pages_on_demand_lie_side_by_side() {
        let run: u64 = 0x1_2340_0000;
        assert_eq!(run % RUN_BYTES, 0);
        let copies = [
            on_demand(run + 0x1_0000, 60),
            on_demand(run + 0x2_0000, 60),
            on_demand(run + 0x3f_0000, 60),
            on_demand(run + 0x40_0000, 62),
        ];
        let (dwords, w) = table(&copies, &[], 4);
        // A 12-byte record 8 bytes before the first page's end.
        let (view, offset) = look_up(&dwords, run + 0x1_fff8);
        assert_eq!((view, offset), (60, 0x1_fff8));
        assert_eq!(look_up(&dwords, run + 0x2_0000), (view, offset + 8), "its third dword is the next page's first");
        // A write there marks the page of its last byte, which goes back
        // with that page's copy.
        let last = marked_bit(&dwords, run + 0x1_fff8 + 11);
        assert_eq!(mark(last, &w.blocks, &w.owner), Mark::Written(1, run + 0x2_0000));
        // The run's last page: what follows is past the buffer's end (reads
        // 0), though the next run's first page is held, in its own buffer.
        let (view, offset) = look_up(&dwords, run + 0x3f_fff8);
        assert_eq!((view, u64::from(offset) + 8), (60, RUN_BYTES));
        assert_eq!(look_up(&dwords, run + 0x40_0000), (62, 0));
        // Cut: the second page (the third is not held), the run's last, and
        // the next run's first.
        assert_eq!(w.cut, 3);
        assert_eq!(offset_in_run((run + 0x3f_0000) >> 16), 0x3f_0000);
        assert_eq!(run_of((run + 0x40_0000) >> 16), run_of(run >> 16) + 1);
    }

    /// When the table is full, pages on demand are the copies left out, not
    /// a title buffer's.
    #[test]
    fn pages_on_demand_get_their_blocks_last() {
        let mut copies: Vec<Named> = (0..CAPACITY as u64).map(|k| Named {
            start: (k + 2) << 28,
            len: 0x100,
            pair: 10,
            view_start: 0,
            yields: false,
        })
        .collect();
        // A whole page on demand, larger than every buffer, listed first.
        copies.insert(0, on_demand(1 << 28, 8));
        let (dwords, w) = table(&copies, &[], 4);
        assert_eq!(look_up(&dwords, 1 << 28).0, 4, "left out");
        assert!((2..CAPACITY as u64 + 2).all(|k| look_up(&dwords, k << 28).0 == 10));
        assert!(w.superseded.is_empty(), "left out is not held by another");
    }

    /// A probed region's misses name their page: the shader marks the
    /// page's entry instead of the region's bit, reads the null views as
    /// before, and the host reads the page's address back from the bit.
    #[test]
    fn a_probe_makes_a_regions_misses_name_their_page() {
        let copies = named(&[(0x40_0000, 0x1_0000)], &[102]);
        let missed: u64 = 0x12_3456_789c;
        let l1 = (missed >> 28) as usize;
        let (plain, w0) = table(&copies, &[], 100);
        let bit = marked_bit(&plain, missed);
        assert_eq!(mark(bit, &w0.blocks, &w0.owner), Mark::Region(l1));
        assert_eq!(look_up(&plain, missed).0, 100);
        let (probed, w1) = table(&copies, &[l1], 100);
        let bit = marked_bit(&probed, missed);
        assert_eq!(mark(bit, &w1.blocks, &w1.owner), Mark::Missed(missed & !0xFFFF));
        assert_eq!(look_up(&probed, missed).0, 100, "still the null views");
        assert_eq!(w1.blocks.len(), w0.blocks.len() + 1);
        // What copies hold is where it was, and a write there is one.
        assert_eq!(look_up(&probed, 0x40_0010), look_up(&plain, 0x40_0010));
        let written = marked_bit(&probed, 0x40_0010);
        assert_eq!(mark(written, &w1.blocks, &w1.owner), Mark::Written(0, 0x40_0000));
        // A region a copy lies in gets no second block; a probe outside the
        // table is ignored.
        let (_, w2) = table(&copies, &[0, LEVEL1, l1, l1], 100);
        assert_eq!(w2.blocks.len(), w1.blocks.len());
    }

    /// Probes never take a block from a copy: copies are placed first.
    #[test]
    fn probes_take_only_the_blocks_copies_leave() {
        let ranges: Vec<(u64, u64)> = (0..CAPACITY as u64).map(|k| ((k + 1) << 28, 0x100)).collect();
        let pairs = vec![10; ranges.len()];
        let (dwords, w) = table(&named(&ranges, &pairs), &[3000, 3001], 4);
        assert_eq!(w.blocks.len(), CAPACITY + 1);
        assert!(!w.blocks.contains(&3000), "no room: the region stays a region");
        assert_eq!(mark(marked_bit(&dwords, 3000 << 28), &w.blocks, &w.owner), Mark::Region(3000));
        assert_eq!(look_up(&dwords, (CAPACITY as u64) << 28).0, 10);
    }

    /// Bit 0 is an access above the table's reach once the lowest 256 MiB
    /// have a block, and may be either before.
    #[test]
    fn bit_zero_is_above_the_table_only_once_the_lowest_region_has_a_block() {
        let (dwords, w) = table(&[], &[], 100);
        assert_eq!(marked_bit(&dwords, 0x200_0000_0000), 0);
        assert_eq!(marked_bit(&dwords, 0x1234), 0);
        assert_eq!(mark(0, &w.blocks, &w.owner), Mark::Region(0));
        let (dwords, w) = table(&[], &[0], 100);
        assert_eq!(mark(marked_bit(&dwords, 0x200_0000_0000), &w.blocks, &w.owner), Mark::Above);
        assert_eq!(mark(marked_bit(&dwords, 0x1234), &w.blocks, &w.owner), Mark::Missed(0));
        assert_eq!(mark(7, &w.blocks, &w.owner), Mark::Region(7));
        assert_eq!(mark(5 * BLOCK, &w.blocks, &w.owner), Mark::Stray);
    }

    /// A list's marks are sorted by what they are and cleared: misses of
    /// each kind, pages that go back by page, and pages left to a root
    /// UAV's own copy-back, which a page on demand's never are.
    #[test]
    fn a_lists_marks_are_sorted_by_what_they_are() {
        let mut copies = named(&[(0x10_0000, 0x4_0000)], &[50]);
        copies.push(on_demand(0x20_0000, 60));
        let (dwords, w) = table(&copies, &[3], 4);
        let mut words = vec![0u32; BITMAP_WORDS];
        // Writes in the buffer (one in a root UAV's range) and in the page
        // on demand (a root UAV's range names it too); a page with no copy,
        // a region with no block, an address above the table, a stray bit.
        for address in [0x11_0040, 0x12_0010, 0x20_0080, 0x30_0000, 5 << 28, 0x200_0000_0000] {
            set(&mut words, marked_bit(&dwords, address));
        }
        set(&mut words, 3 * BLOCK + 1);
        assert!(has_missed(&words, &w.blocks, &w.owner));
        let precise = [(0x12_0010, 0x20), (0x20_0000, 0x100)];
        let yields = |k: usize| copies[k].yields;
        let m = take_marks(&mut words, &w.blocks, &w.owner, &precise, yields);
        assert_eq!(
            m,
            Marks {
                above: 1,
                stray: 1,
                regions: vec![5],
                missed: vec![0x30_0000],
                writes: vec![(0, 0x11_0000), (1, 0x20_0000)],
                precise: 1,
                precise_paged: 1,
            }
        );
        assert_eq!((m.misses(), m.wrote()), (4, 3));
        assert!(words.iter().all(|&w| w == 0), "cleared for the list's next run");
        // Writes alone are no miss.
        for address in [0x11_0040, 0x12_0010, 0x20_0080] {
            set(&mut words, marked_bit(&dwords, address));
        }
        assert!(!has_missed(&words, &w.blocks, &w.owner));
        assert_eq!(take_marks(&mut words, &w.blocks, &w.owner, &precise, yields).misses(), 0);
    }

    /// A root UAV's bytes in a page its copy holds in part: the table names
    /// that copy for the page, with a page on demand there or without, so a
    /// write by address lands where the root UAV's copy-back looks. Were
    /// the copy not named (no slot for its views), the page on demand has
    /// the page and the written page goes back by page.
    #[test]
    fn a_root_uavs_page_goes_back_from_the_copy_the_table_named() {
        let buffer = named(&[(0x10_0000, 0x3_8000)], &[50]);
        let page = on_demand(0x13_0000, 60);
        let precise = [(0x13_0000, 0x8000)];
        let both = [buffer[0], page];
        let (dwords, w) = table(&both, &[], 4);
        let mut words = vec![0u32; BITMAP_WORDS];
        set(&mut words, marked_bit(&dwords, 0x13_0008));
        assert_eq!(look_up(&dwords, 0x13_0008), (50, 0x3_0008));
        let m = take_marks(&mut words, &w.blocks, &w.owner, &precise, |k| both[k].yields);
        assert_eq!((m.precise, m.precise_paged, m.writes.len()), (1, 0, 0), "left to the buffer's copy-back");
        let alone = [page];
        let (dwords, w) = table(&alone, &[], 4);
        set(&mut words, marked_bit(&dwords, 0x13_0008));
        assert_eq!(look_up(&dwords, 0x13_0008), (60, 0x13_0008));
        let m = take_marks(&mut words, &w.blocks, &w.owner, &precise, |k| alone[k].yields);
        assert_eq!((m.precise, m.precise_paged, m.writes), (0, 1, vec![(0, 0x13_0000)]), "by page, not lost");
    }

    /// The host's two readings of a bitmap agree on whether a list missed.
    #[test]
    fn both_readings_of_a_bitmap_agree_on_a_miss() {
        let mut dice = Dice(0x9E37_79B9_7F4A_7C15);
        let mut copies = named(&[(0x10_0000, 0x4_0000), (0x9000_0000, 0x40_0000)], &[50, 52]);
        copies.push(on_demand(0x20_0000, 60));
        let (_, w) = table(&copies, &[5], 4);
        let bits = w.blocks.len() * BLOCK + 64;
        for _ in 0..200 {
            let mut words = vec![0u32; BITMAP_WORDS];
            for _ in 0..dice.below(4) {
                // Mostly written pages, now and then anything.
                let bit = match dice.below(3) {
                    0 => dice.below(bits as u64) as usize,
                    _ => entry_of(&w.blocks, 0x10_0000 + (dice.below(4) << 16)),
                };
                set(&mut words, bit);
            }
            let missed = has_missed(&words, &w.blocks, &w.owner);
            let m = take_marks(&mut words, &w.blocks, &w.owner, &[], |k| copies[k].yields);
            assert_eq!(missed, m.misses() > 0);
        }
    }

    #[test]
    fn a_missed_page_is_classified_by_what_is_known_of_it() {
        let f = Facts::default();
        assert_eq!(classify(f), Reason::Absent);
        assert_eq!(classify(Facts { readable: true, ..f }), Reason::Foreign);
        assert_eq!(classify(Facts { readable: true, tracked: true, ..f }), Reason::Loose);
        assert_eq!(classify(Facts { readable: true, tracked: true, in_buffer: true, ..f }), Reason::Buffer);
        assert_eq!(classify(Facts { held: true, ..f }), Reason::Unnamed);
        assert_eq!(classify(Facts { part: true, readable: true, tracked: true, ..f }), Reason::Shared);
        // A buffer in memory that is not tracked gets no page either.
        assert_eq!(classify(Facts { readable: true, in_buffer: true, ..f }), Reason::Foreign);
        let pageable: Vec<Reason> = Reason::ALL.into_iter().filter(|r| r.pageable()).collect();
        assert_eq!(pageable, [Reason::Loose, Reason::Buffer], "only tracked memory with no copy at all");
        for (i, reason) in Reason::ALL.into_iter().enumerate() {
            assert_eq!(reason as usize, i);
        }
        assert_eq!((Reason::Absent.stray(), Reason::Loose.stray(), Reason::Unasked.stray()), (Some(true), Some(false), None));
    }

    #[test]
    fn pages_are_held_in_a_bounded_number_of_runs() {
        let mut p = Pager::new(2, 100, 100);
        assert_eq!(p.want(0), Admission::Admit);
        assert_eq!(p.want(0), Admission::Held, "asked again by another list");
        assert_eq!(p.want(1), Admission::Admit, "the same run: no new buffer");
        assert_eq!(p.want(RUN_PAGES), Admission::Admit, "a second run");
        assert_eq!(p.want(2 * RUN_PAGES), Admission::Full, "two buffers in all");
        assert_eq!(p.want(RUN_PAGES + 7), Admission::Admit);
        assert_eq!(p.held(), Holding { pages: 4, runs: 2, leaving: 0 });
        assert!(p.holds(1) && !p.holds(2));
        assert_eq!(p.limit(), 2);
    }

    #[test]
    fn so_many_pages_and_new_runs_are_taken_in_a_frame() {
        let mut p = Pager::new(8, 2, 100);
        assert_eq!((p.want(0), p.want(1)), (Admission::Admit, Admission::Admit));
        assert_eq!(p.want(2), Admission::Later, "two a frame");
        // The pages after a page just taken come with it whatever the count.
        assert!(p.follow(2));
        assert!(!p.follow(2), "held already");
        assert!(!p.follow(RUN_PAGES), "its run has no buffer: not without a miss of its own");
        p.frame();
        assert_eq!(p.want(3), Admission::Admit);
        let mut p = Pager::new(8, 100, 1);
        assert_eq!(p.want(0), Admission::Admit);
        assert_eq!(p.want(RUN_PAGES), Admission::Later, "one new buffer a frame");
        assert_eq!(p.want(1), Admission::Admit, "a run in use takes pages");
        p.frame();
        assert_eq!(p.want(RUN_PAGES), Admission::Admit);
    }

    /// A page given up keeps its run's buffer until its copy is gone, so
    /// the limit bounds the memory and not only the pages named; the page
    /// may be held again meanwhile, at the same place.
    #[test]
    fn a_run_is_free_only_when_its_last_copy_is_gone() {
        let mut p = Pager::new(1, 100, 100);
        assert_eq!(p.want(5), Admission::Admit);
        assert!(p.release(5));
        assert!(!p.release(5));
        assert_eq!(p.held(), Holding { pages: 0, runs: 1, leaving: 1 });
        assert_eq!(p.want(RUN_PAGES), Admission::Full, "the copy in run 0 still lives");
        assert_eq!(p.want(5), Admission::Admit, "the same place in the buffer that is there");
        assert_eq!(p.gone(5), Gone::default(), "the old copy: the page is held again, nothing to clear");
        assert!(p.release(5));
        assert_eq!(p.gone(5), Gone { clear: false, free: Some(0) }, "the run's last copy");
        assert_eq!(p.gone(5), Gone::default(), "counted once");
        assert_eq!(p.want(RUN_PAGES), Admission::Admit);
        // A place left empty beside pages still held is to be zeroed.
        assert_eq!(p.want(RUN_PAGES + 1), Admission::Admit);
        assert!(p.release(RUN_PAGES));
        assert_eq!(p.gone(RUN_PAGES), Gone { clear: true, free: None });
    }

    #[test]
    fn a_copy_that_could_not_be_made_frees_its_run_and_host_memory_refused_stalls() {
        let mut p = Pager::new(2, 100, 100);
        assert_eq!(p.want(0), Admission::Admit);
        assert_eq!(p.failed(0, false), Some(0), "the run it was made for alone");
        assert!(!p.holds(0));
        assert_eq!((p.want(0), p.want(1)), (Admission::Admit, Admission::Admit));
        assert_eq!(p.failed(1, false), None, "page 0 is in the run");
        assert_eq!(p.failed(9, false), None, "not held");
        assert_eq!(p.want(2), Admission::Admit);
        assert_eq!(p.failed(2, true), None);
        for _ in 0..STALL {
            assert_eq!(p.want(2), Admission::Later, "no try while host memory was just refused");
            p.frame();
        }
        assert_eq!(p.want(2), Admission::Admit);
    }

    #[test]
    fn the_sweep_goes_round_the_held_pages() {
        let mut p = Pager::new(8, 100, 100);
        for page in [40, 10, 30, 20] {
            p.want(page);
        }
        assert_eq!(p.sweep(3), [10, 20, 30]);
        assert_eq!(p.sweep(3), [40, 10, 20]);
        p.release(30);
        assert_eq!(p.sweep(8), [40, 10, 20], "each once, however many are asked for");
        assert!(Pager::new(8, 8, 8).sweep(4).is_empty());
    }

    /// The bound as a formula: whole buffers of [`RUN_BYTES`], as many as
    /// the megabytes asked for hold; and under any order of misses, pages
    /// given up and copies going, no more buffers exist than the limit.
    #[test]
    fn the_limit_bounds_the_host_memory() {
        assert!(BITMAP_BYTES * 8 >= BITMAP_BITS as u64 && BITMAP_BYTES.is_multiple_of(256));
        assert_eq!(RUN_BYTES, 4 << 20);
        assert_eq!(megabytes(None), (DEFAULT_MB, true));
        assert_eq!(megabytes(Some(" 512 ")), (512, true));
        assert_eq!(megabytes(Some("0")), (DEFAULT_MB, false), "no figure: said, not taken quietly");
        assert_eq!(megabytes(Some("lots")), (DEFAULT_MB, false));
        assert_eq!(megabytes(Some("-4")), (DEFAULT_MB, false));
        assert_eq!(limit_runs(DEFAULT_MB), 64);
        assert_eq!(limit_runs(1), 1, "one buffer at least");
        assert_eq!(limit_runs(7), 1);
        assert_eq!(limit_runs(u64::MAX >> 20), (MOST_MB / 4) as usize);
        assert_eq!(worst_case_bytes(0), 0);
        assert_eq!(worst_case_bytes(limit_runs(DEFAULT_MB)), 268_435_456);
        assert_eq!(worst_case_bytes(limit_runs(MOST_MB)), 4_294_967_296);
        // A frame's new runs and a sweep are small beside the default.
        assert!(RUNS_PER_FRAME <= limit_runs(DEFAULT_MB) && SWEEP <= ADMIT_PER_FRAME);
        let mut dice = Dice(0xD1B5_4A32_D192_ED03);
        for limit in [1, 2, 5, 64] {
            let mut p = Pager::new(limit, usize::MAX, usize::MAX);
            // The buffers a host would hold, and the copies not yet gone.
            let mut buffers = BTreeSet::new();
            let mut leaving = Vec::new();
            for _ in 0..4000 {
                let page = dice.below(12 * RUN_PAGES);
                match dice.below(5) {
                    0 | 1 => {
                        if p.want(page) == Admission::Admit {
                            buffers.insert(run_of(page));
                            if dice.below(8) == 0
                                && let Some(run) = p.failed(page, false)
                            {
                                buffers.remove(&run);
                            }
                        }
                    }
                    2 => {
                        if p.follow(page) {
                            assert!(buffers.contains(&run_of(page)), "a followed page needs no buffer");
                        }
                    }
                    3 => {
                        if p.release(page) {
                            leaving.push(page);
                        }
                    }
                    _ => {
                        if !leaving.is_empty() {
                            let k = dice.below(leaving.len() as u64) as usize;
                            if let Some(run) = p.gone(leaving.swap_remove(k)).free {
                                assert!(buffers.remove(&run));
                            }
                        }
                    }
                }
                let held = p.held();
                assert_eq!(buffers.len(), held.runs);
                assert!(buffers.len() <= limit && worst_case_bytes(buffers.len()) <= worst_case_bytes(limit));
                assert_eq!(held.leaving, leaving.len());
            }
        }
    }

    #[test]
    fn regions_are_probed_up_to_a_bound_and_idle_ones_make_way() {
        let mut p = Probes::default();
        assert_eq!(p.note(7, 0), Probed::Added);
        assert_eq!(p.note(7, 1), Probed::Has);
        assert_eq!(p.note(LEVEL1, 1), Probed::Full, "outside the table");
        for l1 in 100..100 + PROBES - 1 {
            assert_eq!(p.note(l1, 1), Probed::Added);
        }
        assert_eq!(p.list().len(), PROBES);
        assert_eq!(p.note(9, 2), Probed::Full, "all in use, none idle for long, none known to be stray");
        // Region 7's pages keep missing; the others fall idle.
        p.touch(7, PROBE_IDLE + 1, None);
        assert_eq!(p.note(9, PROBE_IDLE + 1), Probed::Added);
        let list = p.list();
        assert!(list.contains(&7) && list.contains(&9) && list.len() == PROBES);
    }

    /// Stray addresses in as many regions as there are probes, missed every
    /// frame, do not keep a region of title memory reached later from
    /// having its pages named: a region where every page looked at was not
    /// title memory makes way, and is set aside so that it does not take
    /// the probe back in the next frame (each change rewrites the tables).
    #[test]
    fn regions_only_strays_reach_make_way_for_others() {
        let mut p = Probes::default();
        let strays: Vec<usize> = (1000..1000 + PROBES).collect();
        for &l1 in &strays {
            assert_eq!(p.note(l1, 0), Probed::Added);
        }
        let title = 77;
        let (mut changes, mut refused) = (0, 0);
        let mut aside = BTreeSet::new();
        for frame in 1..5000 {
            for &l1 in &strays {
                if p.list().contains(&l1) {
                    // Its pages are named, looked at, and nothing is there.
                    p.touch(l1, frame, Some(true));
                } else {
                    match p.note(l1, frame) {
                        Probed::Aside => {
                            aside.insert(l1);
                        }
                        Probed::Added => changes += 1,
                        other => panic!("{other:?}"),
                    }
                }
            }
            if p.list().contains(&title) {
                p.touch(title, frame, Some(false));
            } else {
                match p.note(title, frame) {
                    Probed::Added => changes += 1,
                    Probed::Full => refused += 1,
                    other => panic!("{other:?}"),
                }
            }
        }
        assert!(p.list().contains(&title), "the title's region has its probe");
        assert!(refused < PROBE_STRAY as usize + 1, "after the first few frames: {refused}");
        assert!(!aside.is_empty());
        assert!(changes <= 3, "the tables are not rewritten for strays every frame: {changes}");
    }

    /// A region whose pages were not looked at, or where one page is title
    /// memory, is never taken for stray.
    #[test]
    fn a_region_that_may_be_title_memory_keeps_its_probe() {
        let mut p = Probes::default();
        for l1 in 0..PROBES {
            p.note(l1, 0);
            for _ in 0..PROBE_STRAY + 1 {
                // Not looked at; or looked at with one page of the title's.
                p.touch(l1, 1, (l1 % 2 == 0).then_some(true));
            }
            if l1 % 2 == 0 {
                p.touch(l1, 1, Some(false));
            }
        }
        assert_eq!(p.note(3000, 2), Probed::Full);
        assert_eq!(p.list().len(), PROBES);
    }

    #[test]
    fn what_was_found_of_a_page_holds_for_a_while_and_longer_each_time() {
        let mut c = Cache::default();
        assert_eq!(c.get(5, 0), None);
        c.put(5, 0, Reason::Absent);
        let wait = c.found[&5].wait;
        assert!((ASK_AGAIN..2 * ASK_AGAIN).contains(&wait));
        assert_eq!(c.get(5, wait - 1), Some(Reason::Absent));
        assert_eq!(c.get(5, wait), None, "asked again: the title may have the memory now");
        // The same answer again holds twice as long, up to a bound.
        c.put(5, wait, Reason::Absent);
        assert_eq!(c.found[&5].wait, 2 * wait);
        assert_eq!(c.get(5, 3 * wait - 1), Some(Reason::Absent));
        for _ in 0..10 {
            c.put(5, 0, Reason::Absent);
        }
        assert_eq!(c.found[&5].wait, ASK_AGAIN_MOST);
        // Another answer starts again.
        c.put(5, 0, Reason::Loose);
        assert_eq!(c.found[&5].wait, wait);
        c.forget(5);
        assert_eq!(c.get(5, 0), None);
        // Pages found together are not asked about again together.
        let waits: BTreeSet<u64> = (0..64).map(|page| {
            c.put(page, 0, Reason::Absent);
            c.found[&page].wait
        })
        .collect();
        assert!(waits.len() > 8, "{waits:?}");
        // Asking is bounded by frame.
        assert!((0..3).all(|_| c.ask(7, 3)));
        assert!(!c.ask(7, 3), "the rest of the frame's pages are not looked at");
        assert!(c.ask(8, 3));
    }

    /// A steady set of missed pages, missed in every frame (the size the
    /// forks report at the top of their range): the system is asked about
    /// at most [`ASK_PER_FRAME`] pages in any frame, every page is looked
    /// at within a few seconds, and after that about a page a frame.
    #[test]
    fn a_steady_set_of_missed_pages_is_soon_asked_about_rarely() {
        let mut c = Cache::default();
        let pages: Vec<u64> = (0..730).map(|k| 0x4_0000 + 3 * k).collect();
        let mut asks = Vec::new();
        for frame in 0..3000u64 {
            let mut asked = 0;
            for &page in &pages {
                if c.get(page, frame).is_none() && c.ask(frame, ASK_PER_FRAME) {
                    c.put(page, frame, Reason::Absent);
                    asked += 1;
                }
            }
            asks.push(asked);
            if frame == 300 {
                assert!(pages.iter().all(|&page| c.get(page, frame).is_some()), "all looked at by now");
            }
        }
        assert!(asks.iter().all(|&n| n <= ASK_PER_FRAME));
        let late: usize = asks[1000..].iter().sum();
        assert!(late <= 3 * pages.len(), "{late} asks over the last 2000 frames");
    }

    /// Full, the cache drops the pages asked about longest ago, an eighth
    /// at a time, and keeps the rest.
    #[test]
    fn a_full_cache_makes_room_piecewise() {
        let mut c = Cache::default();
        for page in 0..CACHE as u64 {
            c.put(page, page, Reason::Absent);
        }
        assert_eq!(c.found.len(), CACHE);
        c.put(1 << 30, CACHE as u64, Reason::Loose);
        assert_eq!(c.found.len(), CACHE - CACHE / 8 + 1);
        assert_eq!(c.get(1 << 30, CACHE as u64), Some(Reason::Loose));
        assert!(!c.found.contains_key(&0) && !c.found.contains_key(&(CACHE as u64 / 8 - 1)));
        assert!(c.found.contains_key(&(CACHE as u64 / 8)) && c.found.contains_key(&(CACHE as u64 - 1)));
        // A page already there is put without making room.
        c.put(CACHE as u64 - 1, 0, Reason::Absent);
        assert_eq!(c.found.len(), CACHE - CACHE / 8 + 1);
    }

    /// The system and the device, for the tests of [`State`]: what is known
    /// of each page (nothing readable where none is given), and what was
    /// asked and made.
    #[derive(Default)]
    struct Fake {
        facts: HashMap<u64, Facts>,
        asked: Vec<u64>,
        tries: usize,
        made: BTreeSet<u64>,
        buffers: BTreeSet<u64>,
        cleared: Vec<u64>,
        no_memory: bool,
        untracked: BTreeSet<u64>,
    }

    /// Tracked memory of the title's in no buffer, with no copy.
    const TITLE: Facts = Facts {
        held: false,
        part: false,
        readable: true,
        tracked: true,
        in_buffer: false,
    };

    impl Fake {
        fn with(title: impl IntoIterator<Item = u64>) -> Self {
            Fake {
                facts: title.into_iter().map(|page| (page, TITLE)).collect(),
                ..Default::default()
            }
        }
    }

    impl Host for Fake {
        fn facts(&mut self, at: u64) -> Facts {
            assert_eq!(at % PAGE, 0);
            self.asked.push(at >> 16);
            self.facts.get(&(at >> 16)).copied().unwrap_or_default()
        }

        fn make(&mut self, page: u64) -> Result<(), Unmade> {
            self.tries += 1;
            if self.no_memory && !self.buffers.contains(&run_of(page)) {
                return Err(Unmade::NoMemory);
            }
            // The buffer is made before the tracker is asked for the copy.
            self.buffers.insert(run_of(page));
            if self.untracked.contains(&page) {
                return Err(Unmade::Untracked);
            }
            assert!(self.made.insert(page), "a page is made once while held");
            Ok(())
        }

        fn free(&mut self, run: u64) {
            self.buffers.remove(&run);
        }

        fn clear(&mut self, page: u64) {
            self.cleared.push(page);
        }
    }

    fn missed(pages: &[u64]) -> Marks {
        Marks {
            missed: pages.iter().map(|page| page << 16).collect(),
            ..Default::default()
        }
    }

    /// Without the switch a missed page is looked at once, counted by what
    /// it is, and nothing is made.
    #[test]
    fn without_the_switch_misses_are_looked_at_and_counted_only() {
        let mut s = State::new(false, true, true, DEFAULT_MB);
        let mut host = Fake::with([0x100]);
        assert!(!s.note_misses(&missed(&[0x100, 0x101]), &mut host), "no table changes");
        assert_eq!(host.asked, [0x100, 0x101]);
        // Another list, another submission of the frame: nothing is asked.
        assert!(!s.note_misses(&missed(&[0x100, 0x101]), &mut host));
        assert_eq!(host.asked, [0x100, 0x101]);
        assert_eq!((s.tally.count(Reason::Loose), s.tally.count(Reason::Absent)), (2, 2));
        assert_eq!((s.tally.asked, host.tries), (2, 0));
        assert!(!s.holds(0x100) && s.held() == Holding::default());
        let (line, sweep) = s.frame_end(true, 0);
        let line = line.expect("a frame with misses is reported");
        assert!(line.contains("VICTORY_MEMORY_PAGING=1 would give 2 of these misses a copy"), "{line}");
        assert!(line.contains("; 2 pages looked at in "), "{line}");
        assert!(sweep.is_empty());
    }

    /// With looking switched off the system is asked nothing, and each
    /// missed page is counted as not looked at.
    #[test]
    fn with_looking_off_nothing_is_asked() {
        let mut s = State::new(false, true, false, DEFAULT_MB);
        let mut host = Fake::with([0x100]);
        assert!(!s.note_misses(&missed(&[0x100, 0x101]), &mut host));
        assert!(host.asked.is_empty());
        assert_eq!(s.tally.count(Reason::Unasked), 2);
        // Pages on demand must look: the switch for them turns looking on.
        let mut s = State::new(true, true, false, DEFAULT_MB);
        assert!(s.note_misses(&missed(&[0x100]), &mut host));
        assert_eq!(host.asked[0], 0x100);
    }

    /// At most [`ASK_PER_FRAME`] pages are looked at in a frame without the
    /// switch; the rest are counted as not looked at, and come up in the
    /// frames after.
    #[test]
    fn so_many_pages_are_looked_at_in_a_frame() {
        let mut s = State::new(false, true, true, DEFAULT_MB);
        let mut host = Fake::default();
        let pages: Vec<u64> = (0..ASK_PER_FRAME as u64 + 5).collect();
        s.note_misses(&missed(&pages), &mut host);
        assert_eq!(host.asked.len(), ASK_PER_FRAME);
        assert_eq!((s.tally.count(Reason::Absent), s.tally.count(Reason::Unasked)), (ASK_PER_FRAME as u64, 5));
        s.frame_end(true, 0);
        s.note_misses(&missed(&pages), &mut host);
        assert_eq!(host.asked.len(), ASK_PER_FRAME + 5);
    }

    /// With the switch a missed page of title memory gets a copy, and so do
    /// the pages after it in its run up to the first that cannot have one:
    /// they lie beside it. A second list that missed the page in the same
    /// submission finds it held and asks nothing.
    #[test]
    fn a_missed_page_gets_a_copy_and_the_pages_after_it_in_its_run() {
        let mut s = State::new(true, true, true, DEFAULT_MB);
        // Run 4 is pages 0x100 to 0x13f. Title memory from 0x100 to 0x103.
        let mut host = Fake::with(0x100..0x104);
        assert!(s.note_misses(&missed(&[0x101]), &mut host), "the tables are to name the copies");
        assert_eq!(host.made, BTreeSet::from([0x101, 0x102, 0x103]), "not the page before: nothing runs into it unseen");
        assert_eq!(host.asked, [0x101, 0x102, 0x103, 0x104], "the first page that is not title memory ends it");
        assert_eq!(host.buffers, BTreeSet::from([4]));
        assert_eq!((s.tally.admitted, s.tally.followed), (1, 2));
        assert_eq!(s.held(), Holding { pages: 3, runs: 1, leaving: 0 });
        assert!(!s.note_misses(&missed(&[0x101, 0x102]), &mut host));
        assert_eq!(host.asked.len(), 4, "held: no system call");
        assert_eq!(s.tally.count(Reason::Unnamed), 2, "copied, and the table of that submission did not name it");
        assert!(host.cleared.is_empty());
        // The page after the last one held is missed (a write that ran past
        // the held page's end marks it): what the write left in the buffer
        // there is zeroed. A page with no held page before it has no place
        // anything could have written.
        assert!(!s.note_misses(&missed(&[0x104, 0x120]), &mut host));
        assert_eq!(host.cleared, [0x104]);
        assert_eq!(host.asked[4..], [0x120], "what ended the pages taken is known already");
        // A page another copy holds ends the run of pages taken, and so
        // does the run's end.
        host.facts.insert(0x106, TITLE);
        host.facts.insert(0x107, Facts { held: true, ..Facts::default() });
        host.facts.insert(0x108, TITLE);
        assert!(s.note_misses(&missed(&[0x106]), &mut host));
        assert!(s.holds(0x106) && !s.holds(0x107) && !s.holds(0x108));
        host.facts.insert(0x13f, TITLE);
        host.facts.insert(0x140, TITLE);
        let asked = host.asked.len();
        assert!(s.note_misses(&missed(&[0x13f]), &mut host));
        assert!(s.holds(0x13f) && !s.holds(0x140));
        assert_eq!(host.asked[asked..], [0x13f], "the next run's page is asked about when it is missed itself");
        // The pages held are swept at the frame's end.
        let (line, sweep) = s.frame_end(true, 0);
        assert_eq!(sweep, [0x101, 0x102, 0x103, 0x106, 0x13f]);
        let line = line.unwrap();
        assert!(line.contains("pages on demand: 3 given a copy for a miss and 2 after them in their 4 MiB"), "{line}");
        assert!(line.contains("5 held in 1 buffers (4.2 MB), 0 old copies not yet freed"), "{line}");
    }

    /// Past the limit of buffers a page stays a counted miss; a page of a
    /// run that has its buffer is still taken.
    #[test]
    fn a_page_whose_run_has_no_buffer_left_stays_a_miss() {
        let mut s = State::new(true, true, true, 4);
        assert_eq!(s.limit(), 1);
        let mut host = Fake::with([0x100, 0x110, 0x240]);
        assert!(s.note_misses(&missed(&[0x100]), &mut host));
        assert!(!s.note_misses(&missed(&[0x240]), &mut host));
        assert_eq!((s.tally.full, host.tries), (1, 1));
        assert!(s.note_misses(&missed(&[0x110]), &mut host));
        assert_eq!(host.buffers, BTreeSet::from([4]));
        assert_eq!(s.tally.count(Reason::Loose), 3, "each stays counted as the miss it was");
    }

    /// Host memory refused: nothing is held, nothing is tried again for
    /// some frames, and the page is not taken for memory of another kind.
    #[test]
    fn without_host_memory_nothing_is_held_and_nothing_tried_for_a_while() {
        let mut s = State::new(true, true, true, DEFAULT_MB);
        let mut host = Fake::with([0x100, 0x200]);
        host.no_memory = true;
        assert!(!s.note_misses(&missed(&[0x100, 0x200]), &mut host));
        assert_eq!((s.tally.failed, s.tally.later, host.tries), (1, 1, 1), "the second page is not tried");
        assert_eq!(s.cache.get(0x100, 0), Some(Reason::Loose), "still what it was found to be");
        assert!(host.buffers.is_empty() && s.held() == Holding::default());
        host.no_memory = false;
        for _ in 0..STALL {
            assert!(!s.note_misses(&missed(&[0x100]), &mut host));
            s.frame_end(true, 0);
        }
        assert_eq!(host.tries, 1);
        assert!(s.note_misses(&missed(&[0x100]), &mut host));
        assert!(s.holds(0x100));
    }

    /// A page the change tracker refuses when its copy is made: not held,
    /// its run's buffer freed if made for it alone, and the page then
    /// counted as memory that is not tracked, without asking again.
    #[test]
    fn a_page_the_tracker_refuses_is_not_held() {
        let mut s = State::new(true, true, true, DEFAULT_MB);
        let mut host = Fake::with([0x100, 0x101]);
        host.untracked.insert(0x100);
        assert!(!s.note_misses(&missed(&[0x100]), &mut host));
        assert_eq!((s.tally.untracked, s.tally.failed), (1, 0));
        assert!(host.buffers.is_empty(), "freed with its only page");
        assert!(!s.note_misses(&missed(&[0x100]), &mut host));
        assert_eq!(host.asked, [0x100]);
        assert_eq!((s.tally.count(Reason::Loose), s.tally.count(Reason::Foreign)), (1, 1));
        // Refused after the page before it was taken: that one stays.
        host.untracked.insert(0x102);
        host.facts.insert(0x102, TITLE);
        assert!(s.note_misses(&missed(&[0x101]), &mut host));
        assert!(s.holds(0x101) && !s.holds(0x102));
        assert_eq!(host.buffers, BTreeSet::from([4]));
    }

    /// A page given up is counted by why; when its copy is gone its place
    /// is zeroed, or the run's buffer freed with the run's last copy.
    #[test]
    fn a_page_given_up_leaves_its_place_empty_and_its_run_its_buffer() {
        let mut s = State::new(true, true, true, DEFAULT_MB);
        let mut host = Fake::with([0x101, 0x102]);
        assert!(s.note_misses(&missed(&[0x101]), &mut host));
        assert!(s.give_up(0x102, GivenUp::Part));
        assert!(!s.give_up(0x102, GivenUp::Part), "once");
        assert!(!s.give_up(0x999, GivenUp::Whole), "not held");
        assert_eq!(s.held(), Holding { pages: 1, runs: 1, leaving: 1 });
        s.gone(0x102, &mut host);
        assert_eq!((host.cleared.as_slice(), host.buffers.len()), (&[0x102][..], 1));
        assert!(s.give_up(0x101, GivenUp::Whole));
        s.gone(0x101, &mut host);
        assert!(host.buffers.is_empty(), "the run's last copy");
        assert_eq!(host.cleared, [0x102], "a freed buffer is not zeroed");
        assert!(!s.give_up(0x101, GivenUp::Vanished), "given up already");
        assert_eq!((s.tally.shared, s.tally.superseded, s.tally.vanished), (1, 1, 0));
        // Given up, a page that is missed again is looked at again.
        host.made.clear();
        let asked = host.asked.len();
        assert!(s.note_misses(&missed(&[0x101]), &mut host));
        assert_eq!(host.asked[asked], 0x101);
    }

    /// A region with no block gets one, once, unless probes are off; the
    /// kinds of miss a bitmap has beside pages are counted.
    #[test]
    fn a_region_reached_gets_a_block_for_its_next_miss() {
        let marks = Marks {
            above: 2,
            stray: 1,
            regions: vec![7, 9],
            ..Default::default()
        };
        let mut host = Fake::default();
        let mut s = State::new(false, true, true, DEFAULT_MB);
        assert!(s.note_misses(&marks, &mut host), "the tables are written again with the blocks");
        assert_eq!(s.probes(), [7, 9]);
        assert!(!s.note_misses(&marks, &mut host));
        assert_eq!((s.tally.count(Reason::Above), s.tally.count(Reason::Region)), (4, 6));
        assert!(host.asked.is_empty());
        let mut s = State::new(false, false, true, DEFAULT_MB);
        assert!(!s.note_misses(&marks, &mut host));
        assert!(s.probes().is_empty());
    }

    /// Regions that get no block are counted: all probes in use, or the
    /// region set aside.
    #[test]
    fn regions_refused_a_block_are_counted() {
        let mut s = State::new(false, true, true, DEFAULT_MB);
        let mut host = Fake::default();
        let regions: Vec<usize> = (0..PROBES + 2).collect();
        s.note_misses(&Marks { regions, ..Default::default() }, &mut host);
        assert_eq!(s.probes().len(), PROBES);
        assert_eq!(s.tally.unprobed, 2);
        let (line, _) = s.frame_end(true, 0);
        assert!(line.unwrap().contains("2 were in regions refused a block (all 96 in use)"));
    }

    /// The lists whose writes were withheld are counted by pipeline and
    /// reported: nothing runs them again.
    #[test]
    fn lists_whose_writes_were_withheld_are_counted_by_pipeline() {
        let mut s = State::new(false, true, true, DEFAULT_MB);
        s.note_withheld([("builder", 2), ("tracer", 1)]);
        s.note_withheld([("builder", 1)]);
        s.table(3, 7);
        assert_eq!(s.partial(), 7);
        let (line, _) = s.frame_end(true, 0);
        let line = line.expect("withheld writes are something to say");
        assert!(
            line.contains("4 draws/dispatches were in lists whose writes through the table were withheld (3 builder, 1 tracer)"),
            "{line}"
        );
        assert!(line.contains("the table names 7 pages for a copy holding part of them"), "{line}");
        // More names than are kept are counted together.
        let names: Vec<String> = (0..Tally::NAMES + 3).map(|k| format!("p{k:02}")).collect();
        s.note_withheld(names.iter().map(|name| (name.as_str(), 1)));
        assert_eq!((s.tally.lost.len(), s.tally.lost_others), (Tally::NAMES, 3));
        let (line, _) = s.frame_end(true, 0);
        assert!(line.unwrap().contains(&format!("{} draws/dispatches", Tally::NAMES + 3)));
    }

    #[test]
    fn the_tally_counts_frames_reasons_and_regions() {
        let mut t = Tally::default();
        assert!(!t.any());
        t.frame(false);
        t.miss(Reason::Loose, Some(0x41));
        t.miss(Reason::Loose, Some(0x41));
        t.miss(Reason::Absent, Some(0x7));
        t.miss(Reason::Above, None);
        t.frame(true);
        t.frame(true);
        assert_eq!((t.frames, t.missing, t.withheld), (3, 1, 2));
        assert_eq!((t.count(Reason::Loose), t.count(Reason::Absent), t.count(Reason::Buffer)), (2, 1, 0));
        let off = t.line(Now::default());
        assert!(off.starts_with("title memory misses over 3 frames (1 with a miss, 2 with writes withheld): "), "{off}");
        assert!(off.contains("1 at or above 2^40, 1 not title memory (nothing readable), 2 tracked memory in no buffer object"), "{off}");
        assert!(off.contains("VICTORY_MEMORY_PAGING=1 would give 2 of these misses a copy"), "{off}");
        assert!(off.ends_with("256 MiB regions missed most: 0x410000000 x2, 0x70000000 x1 (2 in all)"), "{off}");
        assert!(!off.contains("kept the title's bytes") && !off.contains("looked at") && !off.contains("refused a block"), "{off}");
        t.admitted = 2;
        t.full = 1;
        t.kept_titles = 3;
        t.precise_paged = 1;
        t.aside = 4;
        let now = Now {
            on: true,
            held: Holding { pages: 2, runs: 1, leaving: 1 },
            limit: 16,
            cut: 5,
            partial: 0,
        };
        let on = t.line(now);
        assert!(on.contains("; 3 pages shaders wrote kept the title's bytes"), "{on}");
        assert!(on.contains("pages on demand: 2 given a copy for a miss and 0 after them in their 4 MiB, 1 refused (all 16 buffers in use)"), "{on}");
        assert!(on.contains("2 held in 1 buffers (4.2 MB), 1 old copies not yet freed; 5 held pages end where the next page is not beside them"), "{on}");
        assert!(on.contains("; 1 written pages in a root UAV's range went back by page"), "{on}");
        assert!(on.contains("0 were in regions refused a block (all 96 in use) and 4 in regions set aside"), "{on}");
    }

    #[test]
    fn the_first_frames_are_reported_alone_then_by_period() {
        let mut r = Report::default();
        let mut quiet = Tally::default();
        quiet.frame(false);
        assert!(!r.due(&quiet), "nothing to say");
        let lost = Tally {
            kept_titles: 1,
            ..Default::default()
        };
        assert!(lost.any(), "writes lost to the title's are something to say");
        let mut t = Tally::default();
        t.miss(Reason::Region, Some(1));
        t.frame(true);
        for _ in 0..Report::ALONE {
            assert!(r.due(&t));
        }
        assert!(!r.due(&t), "from now on by period");
        for _ in 1..Report::PERIOD {
            t.frame(true);
        }
        assert!(r.due(&t));
        // A quiet period is dropped, not grown without end.
        for _ in 1..Report::PERIOD {
            quiet.frame(false);
        }
        assert!(r.due(&quiet));
    }

    #[test]
    fn each_kind_of_miss_is_said_once() {
        let mut s = Seen::default();
        assert!(s.first(Reason::Loose, 3));
        assert!(!s.first(Reason::Loose, 3));
        assert!(s.first(Reason::Absent, 3) && s.first(Reason::Loose, 4));
        for l1 in 0..Seen::MOST {
            s.first(Reason::Region, 1000 + l1);
        }
        assert!(!s.first(Reason::Foreign, 1), "bounded");
    }
}
