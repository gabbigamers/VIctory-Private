//! Views of ray-tracing acceleration structures.
//!
//! The public Direct3D 12 header gives a shader resource view of a
//! structure the dimension
//! `D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE` (11). It is made
//! with no resource: the description's union holds
//! `D3D12_RAYTRACING_ACCELERATION_STRUCTURE_SRV`, whose one field,
//! `Location`, is the structure's GPU virtual address (words 4 and 5 of
//! the description: the union starts at byte 16). On the console that is an
//! address in title memory.
//!
//! What is known and what is not (2026-11-27, no title run):
//!
//! * Read in this code: such a view, made with no resource, has the view
//!   address 0 (the address comes from a buffer resource only) and a null
//!   2D host view. Translated programs that load a descriptor as data read
//!   its dwords 0-1 from the view addresses and take them for a buffer's
//!   base address (`victory_shader::emit`, "View addresses").
//! * Inferred, not observed: that a title's trace programs read a
//!   structure's address that way, so that the `Location` is what those
//!   dwords should hold. No run here shows a title making such a view at
//!   all.
//!
//! So the `Location` is given to shaders only with
//! `VICTORY_STRUCTURE_ADDRESSES=1` (off by default): a trace program that
//! gets it reads title memory at the structure's address, and a read no
//! host copy holds is a miss, which holds back every title-memory
//! write-back of that frame and the next (`crate::memory::withholding`).
//! Whether traces already miss at address 0 without it is for a run to
//! show; until one does, the default stays what it was.
//!
//! Kept here, all of it computable without a device:
//!
//! * the decoding ([`kind`], [`location`]) and the address a shader reads
//!   for such a view with the switch on ([`shader_address`],
//!   [`address_words`]);
//! * what a run says about the views a title makes ([`created`]): a count,
//!   a line per distinct `Location`, and a line and a count for a view
//!   whose dimension is none the header names;
//! * per host view heap, which slots hold such a view now ([`Live`]), and
//!   over all heaps the structures those name ([`Structures`]): where host
//!   code that wants the title's top-level structures starts.
//!
//! The counts, the log lines and the list of live structures are there in
//! either position of the switch: with it off, nothing a shader or the host
//! device sees differs from before this module.
// Used by the host heaps, which exist on Windows only; compiled on every
// host so that its tests run on any.
#![cfg_attr(not(any(windows, test)), allow(dead_code))]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use super::View;
use crate::account::{Gauge, Held, Owner};

/// `D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE`.
pub(crate) const STRUCTURE: u32 = 11;

/// What a view description's dimension says the view is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Buffer,
    Texture,
    /// A ray-tracing structure at this `Location`.
    Structure(u64),
    /// A dimension the public header does not name for this kind of view
    /// (0, "unknown", among them).
    Unknown(u32),
}

/// The kind of view the description `words` (the host's layout: format,
/// dimension, then for shader views the component mapping) describes,
/// shader resource or unordered access.
pub(crate) fn kind(words: &[u32; 10], unordered: bool) -> Kind {
    match (unordered, words[1]) {
        (_, 1) => Kind::Buffer,
        // Shader views: 1D to cube arrays. Unordered ones: 1D to 3D.
        (false, 2..=10) | (true, 2..=8) => Kind::Texture,
        (false, STRUCTURE) => Kind::Structure(u64::from(words[4]) | u64::from(words[5]) << 32),
        (_, other) => Kind::Unknown(other),
    }
}

/// The `Location` of a shader view of a ray-tracing structure; none for
/// any other view. Whatever resource the title passed with it: the header
/// gives the view no resource, and the description is all it has.
pub(crate) fn location(view: &View) -> Option<u64> {
    let words: &[u32; 10] = match view {
        View::Shader(_, Some(desc)) => desc,
        View::Placed(false, _, desc) => desc[..10].try_into().ok()?,
        _ => return None,
    };
    match kind(words, false) {
        Kind::Structure(location) => Some(location),
        _ => None,
    }
}

/// Whether a ray-tracing structure's view is treated as one: shaders read
/// its `Location` as the view's address, and the host makes no view of
/// whatever resource came with it. Only with `VICTORY_STRUCTURE_ADDRESSES`
/// on (read once); off, such a view is made exactly as before.
pub(crate) fn addresses_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = crate::switch::on("VICTORY_STRUCTURE_ADDRESSES");
        if on {
            tracing::warn!(
                "VICTORY_STRUCTURE_ADDRESSES: shaders read a ray-tracing structure view's Location as its address \
                 (experimental: compare the 'title memory this frame' lines with a run without it)"
            );
        }
        on
    })
}

/// [`shader_address`] with the switch in position `on`.
pub(crate) fn address_if(view: &View, on: bool) -> Option<u64> {
    location(view).filter(|_| on)
}

/// The address a shader reads for `view` as a ray-tracing structure's
/// view: its `Location`. None with the switch off or for any other view:
/// the view's address is then what it was before (a buffer's, else 0).
pub(crate) fn shader_address(view: &View) -> Option<u64> {
    address_if(view, addresses_on())
}

/// A view's entry in the view addresses for title address `address`: a
/// console buffer descriptor's first four dwords as far as the host knows
/// them, the address in dword 0 and bits 0-15 of dword 1.
pub(crate) fn address_words(address: u64) -> [u32; 4] {
    [address as u32, (address >> 32) as u32 & 0xFFFF, 0, 0]
}

/// Values seen so far, the first `N` distinct ones: fixed storage, no
/// lock. 0 is not a value.
pub(crate) struct Distinct<const N: usize>([AtomicU64; N]);

/// What [`Distinct::note`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Noted {
    /// Not seen before: now the `n`th distinct value (from 1).
    New(usize),
    Seen,
    /// Not among the first `N`, which is all that is kept.
    Full,
}

impl<const N: usize> Distinct<N> {
    pub(crate) const fn new() -> Self {
        Self([const { AtomicU64::new(0) }; N])
    }

    /// Notes `value`. Each distinct value is `New` once, whatever the
    /// threads: entries fill in order, and whoever loses an entry to
    /// another value looks at the next. 0 is kept nowhere and never new.
    pub(crate) fn note(&self, value: u64) -> Noted {
        if value == 0 {
            return Noted::Seen;
        }
        for (i, entry) in self.0.iter().enumerate() {
            let held = match entry.compare_exchange(0, value, Relaxed, Relaxed) {
                Ok(_) => return Noted::New(i + 1),
                Err(held) => held,
            };
            if held == value {
                return Noted::Seen;
            }
        }
        Noted::Full
    }

    /// How many distinct values are kept.
    pub(crate) fn len(&self) -> usize {
        self.0.iter().take_while(|entry| entry.load(Relaxed) != 0).count()
    }

    /// Whether `value` is one of those kept (tests).
    #[cfg(test)]
    fn holds(&self, value: u64) -> bool {
        value != 0 && self.0.iter().any(|entry| entry.load(Relaxed) == value)
    }
}

/// Distinct `Location`s that get a log line each.
const LOGGED: usize = 32;

/// What the views made so far were ([`created`]).
struct Made {
    /// Shader views of a ray-tracing structure.
    structures: AtomicU64,
    /// Of them: with `Location` 0 (a null view).
    null: AtomicU64,
    /// Of them: made with a resource object, which the header rules out.
    with_resource: AtomicU64,
    /// Of them: made by the methods for memory the title placed itself.
    placed: AtomicU64,
    /// Of them: with a `Location` above 2^48, more than an entry holds.
    wide: AtomicU64,
    /// Views of a dimension the header does not name.
    unknown: AtomicU64,
    locations: Distinct<LOGGED>,
    /// Unknown dimensions, as [`unknown_key`].
    dimensions: Distinct<16>,
}

impl Made {
    const fn new() -> Self {
        Self {
            structures: AtomicU64::new(0),
            null: AtomicU64::new(0),
            with_resource: AtomicU64::new(0),
            placed: AtomicU64::new(0),
            wide: AtomicU64::new(0),
            unknown: AtomicU64::new(0),
            locations: Distinct::new(),
            dimensions: Distinct::new(),
        }
    }

    /// Counts a view of description `words` (made with a `resource` object
    /// or none, and if of memory the title placed itself, at `placed`); what
    /// to log for it: every oddity that is the first of its kind (one view
    /// can be the first of several), and its `Location` or dimension if not
    /// seen before.
    fn count(&self, words: &[u32; 10], unordered: bool, resource: bool, placed: Option<u64>) -> [Option<First>; FIRSTS] {
        const NONE: [Option<First>; FIRSTS] = [None; FIRSTS];
        match kind(words, unordered) {
            Kind::Buffer | Kind::Texture => NONE,
            Kind::Structure(location) => {
                self.structures.fetch_add(1, Relaxed);
                // Counted if `is`; whether it is the first counted.
                let first = |counter: &AtomicU64, is: bool| is && counter.fetch_add(1, Relaxed) == 0;
                let new = match self.locations.note(location) {
                    Noted::New(n) => Some(First::Location(location, n)),
                    Noted::Seen | Noted::Full => None,
                };
                [
                    first(&self.with_resource, resource).then_some(First::WithResource(location)),
                    placed.and_then(|at| first(&self.placed, true).then_some(First::Placed(location, at))),
                    first(&self.wide, location >> 48 != 0).then_some(First::Wide(location)),
                    first(&self.null, location == 0).then_some(First::Null),
                    new,
                ]
            }
            Kind::Unknown(dimension) => {
                self.unknown.fetch_add(1, Relaxed);
                match self.dimensions.note(unknown_key(dimension, unordered)) {
                    Noted::New(_) => [Some(First::Unknown(dimension, unordered)), None, None, None, None],
                    Noted::Seen | Noted::Full => NONE,
                }
            }
        }
    }

    fn counts(&self) -> Counts {
        Counts {
            structures: self.structures.load(Relaxed),
            null: self.null.load(Relaxed),
            with_resource: self.with_resource.load(Relaxed),
            placed: self.placed.load(Relaxed),
            wide: self.wide.load(Relaxed),
            locations: self.locations.len(),
            unknown: self.unknown.load(Relaxed),
        }
    }
}

/// An unknown dimension and the kind of view as one value that is never 0.
fn unknown_key(dimension: u32, unordered: bool) -> u64 {
    1 << 33 | u64::from(unordered) << 32 | u64::from(dimension)
}

/// Room for the firsts one view can be: an entry for each of the four
/// oddities and one for a `Location` (or a dimension) not seen before.
const FIRSTS: usize = 5;

/// The first view of its like: what [`created`] logs once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum First {
    /// A `Location` not seen before, the `n`th distinct one.
    Location(u64, usize),
    Null,
    WithResource(u64),
    /// Its `Location`, and the address of the placed memory.
    Placed(u64, u64),
    Wide(u64),
    /// A dimension the header does not name, of a shader or an unordered
    /// access view.
    Unknown(u32, bool),
}

static MADE: Made = Made::new();

/// The title made a shader (or `unordered` access) view of description
/// `words`, with a `resource` object or none, and if by a method for memory
/// it placed itself, of the memory at `placed`. Counted, and logged if it
/// is the first of its like: a ray-tracing structure at a `Location` not
/// seen before (the first [`LOGGED`] distinct ones), each oddity of such a
/// view, and a dimension the header does not name.
pub(crate) fn created(words: &[u32; 10], unordered: bool, resource: bool, placed: Option<u64>) {
    for first in MADE.count(words, unordered, resource, placed).into_iter().flatten() {
        log(first, words, addresses_on());
    }
}

/// What a line about the first view of its like says is done with it, with
/// the switch in position `on`: only then is anything done differently
/// from before.
fn done(first: First, on: bool) -> &'static str {
    match (first, on) {
        (First::Location(..), true) => "shaders read its Location as the view's address",
        (First::Location(..), false) => {
            "its Location is recorded and not given to shaders, which read the view's address as before \
             (0 for a view made with no resource); VICTORY_STRUCTURE_ADDRESSES=1 gives them the Location"
        }
        (First::Null, _) => "a null view",
        (First::WithResource(_), true) => {
            "its Location is used and the resource is not (before, shaders read a buffer resource's address \
             and the host viewed the resource)"
        }
        (First::WithResource(_), false) => {
            "made by its resource as before (shaders read a buffer resource's address); with \
             VICTORY_STRUCTURE_ADDRESSES=1 its Location is used and the resource is not"
        }
        (First::Placed(..), true) => {
            "its Location is used and the memory's address is not (before, shaders read a placed buffer's address)"
        }
        (First::Placed(..), false) => {
            "made of that memory as before (shaders read a placed buffer's address); with \
             VICTORY_STRUCTURE_ADDRESSES=1 its Location is used and the memory's address is not"
        }
        (First::Wide(_), true) => "shaders read its low 48 bits, all a view address holds",
        (First::Wide(_), false) => {
            "more than a view address holds: with VICTORY_STRUCTURE_ADDRESSES=1 shaders would read its low 48 bits"
        }
        (First::Unknown(..), _) => {
            "recorded as described and made as before (by its resource if the host takes the description, \
             else a slot that reads zeros)"
        }
    }
}

fn log(first: First, words: &[u32; 10], on: bool) {
    let hex = |address: u64| format!("{address:#x}");
    let done = done(first, on);
    match first {
        First::Location(location, n) => {
            tracing::info!(
                location = hex(location),
                distinct = n,
                "a view of a ray-tracing structure (dimension 11): {done}"
            );
            if n == LOGGED {
                tracing::info!(
                    "{LOGGED} distinct Locations of ray-tracing structure views logged: further ones are counted, not logged"
                );
            }
        }
        First::Null => {
            tracing::info!("a view of a ray-tracing structure with Location 0: {done} (logged once, every one counted)");
        }
        First::WithResource(location) => tracing::warn!(
            location = hex(location),
            "a view of a ray-tracing structure made with a resource object, where the header has none: \
             {done} (logged once, every one counted)"
        ),
        First::Placed(location, address) => tracing::warn!(
            location = hex(location),
            address = hex(address),
            "a view of a ray-tracing structure made as a view of placed memory: {done} (logged once, every one counted)"
        ),
        First::Wide(location) => tracing::warn!(
            location = hex(location),
            "a ray-tracing structure's Location above 2^48: {done} (logged once, every one counted)"
        ),
        First::Unknown(dimension, unordered) => tracing::warn!(
            dimension,
            unordered,
            description = ?words,
            "a view of a dimension the Direct3D 12 header does not name: {done}; logged once per dimension, \
             every one counted"
        ),
    }
}

/// Whether a view of unknown dimension `dimension` (of an `unordered`
/// access view or a shader view) was made and logged (tests; one of the
/// first 16 such kinds in the process, all that are kept).
#[cfg(test)]
pub(crate) fn unknown_dimension_noted(dimension: u32, unordered: bool) -> bool {
    MADE.dimensions.holds(unknown_key(dimension, unordered))
}

/// What [`created`] counted so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Counts {
    pub(crate) structures: u64,
    pub(crate) null: u64,
    pub(crate) with_resource: u64,
    pub(crate) placed: u64,
    /// With a `Location` above 2^48, of which a view address holds the low
    /// 48 bits.
    pub(crate) wide: u64,
    /// Distinct `Location`s, at most [`LOGGED`].
    pub(crate) locations: usize,
    pub(crate) unknown: u64,
}

/// The counts of the views made so far.
pub(crate) fn counts() -> Counts {
    MADE.counts()
}

/// Slots of one heap that [`Live`] follows at most. A slot past them is
/// counted and never followed: the heap's list is then not complete, and
/// says so, for as long as the heap lives.
pub(crate) const MAX_SLOTS: usize = 1 << 14;

/// Structures a query names at most ([`Structures::addresses`]).
pub(crate) const MAX_ADDRESSES: usize = 64;

/// The slots of one host view heap (the title's and those of tables made
/// afresh) that hold a view of a ray-tracing structure now, and the
/// distinct structures they name.
///
/// Owner and lifetime: the host heap, for as long as it lives. Bounded: at
/// most [`MAX_SLOTS`] entries in each of two maps (about 30 bytes an entry,
/// under 1 MiB a heap; empty until a title makes such a view), counted by
/// entries as [`Owner::StructureSlots`].
pub(crate) struct Live {
    /// Slot to the `Location` its view names.
    slots: BTreeMap<u32, u64>,
    /// `Location` to how many of `slots` name it.
    locations: BTreeMap<u64, u32>,
    /// Structure views written to slots there was no room to follow.
    unfollowed: u64,
    held: Held,
}

impl Live {
    pub(crate) fn new() -> Self {
        Self::on(crate::account::gauge(Owner::StructureSlots))
    }

    /// As [`Live::new`], counted on `gauge`.
    pub(crate) fn on(gauge: &'static Gauge) -> Self {
        Self {
            slots: BTreeMap::new(),
            locations: BTreeMap::new(),
            unfollowed: 0,
            held: Held::on(gauge, 0),
        }
    }

    /// Slot `slot` now holds a view of the structure at `location`, or
    /// (none, or `Location` 0) anything else. False if the slot could not
    /// be followed: the table is full.
    pub(crate) fn set(&mut self, slot: u32, location: Option<u64>) -> bool {
        let location = location.filter(|&l| l != 0);
        if self.slots.get(&slot).copied() == location {
            return true;
        }
        if location.is_some() && !self.slots.contains_key(&slot) && self.slots.len() >= MAX_SLOTS {
            self.unfollowed += 1;
            return false;
        }
        let old = match location {
            Some(location) => {
                *self.locations.entry(location).or_insert(0) += 1;
                self.slots.insert(slot, location)
            }
            None => self.slots.remove(&slot),
        };
        if let Some(old) = old {
            self.forget(old);
        }
        self.held.resize(self.slots.len() as u64);
        true
    }

    /// One slot fewer names `location`.
    fn forget(&mut self, location: u64) {
        if let Some(n) = self.locations.get_mut(&location) {
            *n -= 1;
            if *n == 0 {
                self.locations.remove(&location);
            }
        }
    }

    /// Slots `first..first + n` hold nothing any more (dynamic slots given
    /// back). Whatever they hold is forgotten, so the caller clears while
    /// the slots are still its own: before another thread can take them
    /// and make a view there.
    pub(crate) fn clear(&mut self, first: u32, n: u32) {
        let Some(more) = n.checked_sub(1) else {
            return;
        };
        let gone: Vec<u32> = self.slots.range(first..=first.saturating_add(more)).map(|(&slot, _)| slot).collect();
        for slot in gone {
            if let Some(old) = self.slots.remove(&slot) {
                self.forget(old);
            }
        }
        self.held.resize(self.slots.len() as u64);
    }

    /// How many slots are followed.
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Adds this heap's slots and structures to `found`.
    pub(crate) fn add_to(&self, found: &mut Structures) {
        found.slots += self.slots.len() as u64;
        found.unfollowed += self.unfollowed;
        for &location in self.locations.keys() {
            found.add(location);
        }
    }
}

/// The top-level ray-tracing structures the title's descriptors name now:
/// the answer to [`crate::descriptor::live_structures`].
///
/// "Now" is by descriptor, not by content: a structure is listed while a
/// slot of a live shader-visible view heap (or of a table made afresh for
/// a draw not yet reset) holds a view of it. A slot the title has not
/// rewritten since it gave a structure up still lists it, and nothing here
/// says the memory at an address holds a built structure: a reader checks
/// what it finds there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Structures {
    /// The structures' addresses in title memory (each view's `Location`):
    /// distinct, ascending, at most [`MAX_ADDRESSES`].
    pub(crate) addresses: Vec<u64>,
    /// Slots holding a structure view, over all heaps.
    pub(crate) slots: u64,
    /// Structure views written to slots that were not followed
    /// ([`MAX_SLOTS`]), or structures past [`MAX_ADDRESSES`] (one for each
    /// heap that names it): not zero means `addresses` may lack some. Not
    /// a number of structures.
    pub(crate) unfollowed: u64,
}

impl Structures {
    pub(crate) fn none() -> Self {
        Self { addresses: Vec::new(), slots: 0, unfollowed: 0 }
    }

    /// Whether `addresses` is every structure a slot names.
    pub(crate) fn complete(&self) -> bool {
        self.unfollowed == 0
    }

    fn add(&mut self, location: u64) {
        if let Err(at) = self.addresses.binary_search(&location) {
            if self.addresses.len() < MAX_ADDRESSES {
                self.addresses.insert(at, location);
            } else {
                self.unfollowed += 1;
            }
        }
    }
}

/// A line for the five-second report: what views of ray-tracing
/// structures the title made, which structures its heaps name now, and
/// whether shaders read those addresses (the switch's position).
pub(crate) fn line(counts: Counts, live: &Structures, addresses_on: bool) -> String {
    let mut text = format!(
        "ray-tracing structure views: {} made ({} with Location 0, {} with a resource, {} of placed memory, \
         {} with a Location above 2^48), {}{} distinct Locations; ",
        counts.structures,
        counts.null,
        counts.with_resource,
        counts.placed,
        counts.wide,
        counts.locations,
        if counts.locations >= LOGGED { " or more" } else { "" },
    );
    let addresses: Vec<String> = live.addresses.iter().map(|a| format!("{a:#x}")).collect();
    text.push_str(&format!(
        "{} slots of host view heaps hold one now, naming {} structures",
        live.slots,
        live.addresses.len()
    ));
    if !addresses.is_empty() {
        text.push_str(&format!(": {}", addresses.join(", ")));
    }
    if !live.complete() {
        text.push_str(&format!(" (not all: {} not followed)", live.unfollowed));
    }
    text.push_str(&format!("; views of a dimension not known: {}", counts.unknown));
    text.push_str(if addresses_on {
        "; VICTORY_STRUCTURE_ADDRESSES: shaders read each one's Location as its address"
    } else {
        "; shaders do not read these addresses (VICTORY_STRUCTURE_ADDRESSES is off)"
    });
    text
}

/// [`line`] of what is so now, if it differs from the last line given (the
/// first is always given, so that a run's log says "0 made" too).
pub(crate) fn report(live: &Structures) -> Option<String> {
    static LAST: std::sync::Mutex<Option<(Counts, u64, u64, u64)>> = std::sync::Mutex::new(None);
    let counts = counts();
    // The addresses as one number: enough to tell a change.
    let sum = live.addresses.iter().fold(live.addresses.len() as u64, |sum, &a| sum.rotate_left(7) ^ a);
    let now = (counts, live.slots, live.unfollowed, sum);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last == Some(now) {
        return None;
    }
    *last = Some(now);
    Some(line(counts, live, addresses_on()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The description of a structure's view as a title writes it: the
    /// public `D3D12_SHADER_RESOURCE_VIEW_DESC`, 40 bytes, with format at
    /// byte 0, dimension at 4, component mapping at 8 and the union (here
    /// `Location`) at 16.
    fn structure_bytes(location: u64) -> [u8; 40] {
        let mut bytes = [0u8; 40];
        bytes[4..8].copy_from_slice(&11u32.to_le_bytes());
        // D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING.
        bytes[8..12].copy_from_slice(&0x1688u32.to_le_bytes());
        bytes[16..24].copy_from_slice(&location.to_le_bytes());
        bytes
    }

    fn words_of(bytes: &[u8; 40]) -> [u32; 10] {
        std::array::from_fn(|i| u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap()))
    }

    #[test]
    fn a_structure_views_location_is_words_four_and_five() {
        let words = words_of(&structure_bytes(0xd_4321_0000));
        assert_eq!(words, [0, 11, 0x1688, 0, 0x4321_0000, 0xd, 0, 0, 0, 0]);
        assert_eq!(kind(&words, false), Kind::Structure(0xd_4321_0000));
        assert_eq!(location(&View::Shader(0, Some(words))), Some(0xd_4321_0000));
        // Whatever resource came with it.
        assert_eq!(location(&View::Shader(0x7000, Some(words))), Some(0xd_4321_0000));
        // The same description first in a placed view's.
        let mut placed = [0u32; 16];
        placed[..10].copy_from_slice(&words);
        (placed[10], placed[11]) = (0x1000, 0x2);
        assert_eq!(location(&View::Placed(false, [0; 14], placed)), Some(0xd_4321_0000));
    }

    #[test]
    fn other_views_have_no_location() {
        // A buffer view whose first element sits where a Location would.
        let buffer = [0, 1, 0x1688, 0, 0x4321_0000, 0xd, 16, 4, 0, 0];
        assert_eq!(kind(&buffer, false), Kind::Buffer);
        assert_eq!(location(&View::Shader(0, Some(buffer))), None);
        let structure = words_of(&structure_bytes(0x1_0000));
        assert_eq!(location(&View::Shader(0, None)), None, "the resource's own view");
        assert_eq!(location(&View::Unordered(0, 0, Some(structure))), None, "unordered access has no such dimension");
        let mut placed = [0u32; 16];
        placed[..10].copy_from_slice(&structure);
        assert_eq!(location(&View::Placed(true, [0; 14], placed)), None);
        assert_eq!(location(&View::Constant(0x1_0000, 256)), None);
    }

    #[test]
    fn dimensions_are_told_apart_by_the_kind_of_view() {
        let of = |dimension| [0, dimension, 0, 0, 0x100, 0, 0, 0, 0, 0];
        for unordered in [false, true] {
            assert_eq!(kind(&of(0), unordered), Kind::Unknown(0));
            assert_eq!(kind(&of(1), unordered), Kind::Buffer);
            for texture in 2..=8 {
                assert_eq!(kind(&of(texture), unordered), Kind::Texture, "{texture}");
            }
            assert_eq!(kind(&of(12), unordered), Kind::Unknown(12));
            assert_eq!(kind(&of(u32::MAX), unordered), Kind::Unknown(u32::MAX));
        }
        // Cubes and cube arrays are shader views only, as structures are.
        for shader_only in [9, 10] {
            assert_eq!(kind(&of(shader_only), false), Kind::Texture);
            assert_eq!(kind(&of(shader_only), true), Kind::Unknown(shader_only));
        }
        assert_eq!(kind(&of(11), false), Kind::Structure(0x100));
        assert_eq!(kind(&of(11), true), Kind::Unknown(11));
    }

    #[test]
    fn an_address_entry_holds_48_bits() {
        assert_eq!(address_words(0xd_4321_0000), [0x4321_0000, 0xd, 0, 0]);
        assert_eq!(address_words(0xffff_1234_5678_9abc), [0x5678_9abc, 0x1234, 0, 0]);
        assert_eq!(address_words(0), [0; 4]);
    }

    #[test]
    fn each_distinct_value_is_new_once() {
        let seen = Distinct::<3>::new();
        assert_eq!(seen.len(), 0);
        assert_eq!(seen.note(0x100), Noted::New(1));
        assert_eq!(seen.note(0x100), Noted::Seen);
        assert_eq!(seen.note(0x200), Noted::New(2));
        assert_eq!(seen.note(0x300), Noted::New(3));
        assert_eq!(seen.note(0x200), Noted::Seen);
        assert_eq!(seen.note(0x400), Noted::Full);
        assert_eq!(seen.note(0x400), Noted::Full);
        assert_eq!(seen.len(), 3);
        // 0 marks an entry not used: never a value, in a full table or an
        // empty one.
        assert_eq!(seen.note(0), Noted::Seen);
        let empty = Distinct::<3>::new();
        assert_eq!((empty.note(0), empty.len()), (Noted::Seen, 0));
        assert_eq!(empty.note(0x100), Noted::New(1));
    }

    #[test]
    fn distinct_values_from_many_threads_are_new_once_each() {
        let seen = Distinct::<8>::new();
        let new = AtomicU64::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for value in 1..=6u64 {
                        if matches!(seen.note(value << 20), Noted::New(_)) {
                            new.fetch_add(1, Relaxed);
                        }
                    }
                });
            }
        });
        assert_eq!((new.load(Relaxed), seen.len()), (6, 6));
    }

    /// What a view was the first of, in the order said.
    fn firsts(found: [Option<First>; FIRSTS]) -> Vec<First> {
        found.into_iter().flatten().collect()
    }

    #[test]
    fn views_made_are_counted_and_the_first_of_each_like_is_logged() {
        let made = Made::new();
        let structure = |location| words_of(&structure_bytes(location));
        let plain = |made: &Made, location| firsts(made.count(&structure(location), false, false, None));
        let buffer = [0, 1, 0, 0, 0, 0, 16, 4, 0, 0];
        assert_eq!(firsts(made.count(&buffer, false, true, None)), []);
        assert_eq!(plain(&made, 0x1000), [First::Location(0x1000, 1)]);
        assert_eq!(plain(&made, 0x1000), [], "seen");
        assert_eq!(plain(&made, 0x2000), [First::Location(0x2000, 2)]);
        assert_eq!(plain(&made, 0), [First::Null]);
        assert_eq!(plain(&made, 0), []);
        // An oddity is said once, beside the Location's own line.
        assert_eq!(
            firsts(made.count(&structure(0x3000), false, true, None)),
            [First::WithResource(0x3000), First::Location(0x3000, 3)]
        );
        assert_eq!(firsts(made.count(&structure(0x3000), false, true, None)), []);
        assert_eq!(
            firsts(made.count(&structure(0x4000), false, false, Some(0x9000))),
            [First::Placed(0x4000, 0x9000), First::Location(0x4000, 4)]
        );
        assert_eq!(plain(&made, 1 << 50), [First::Wide(1 << 50), First::Location(1 << 50, 5)]);
        let unknown = [0, 12, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(firsts(made.count(&unknown, false, true, None)), [First::Unknown(12, false)]);
        assert_eq!(firsts(made.count(&unknown, false, true, None)), []);
        let unordered = firsts(made.count(&unknown, true, true, None));
        assert_eq!(unordered, [First::Unknown(12, true)], "an unordered view's is its own");
        assert_eq!(firsts(made.count(&structure(0x1000), true, false, None)), [First::Unknown(11, true)]);
        assert_eq!(
            made.counts(),
            Counts { structures: 9, null: 2, with_resource: 2, placed: 1, wide: 1, locations: 5, unknown: 4 }
        );
    }

    /// A view that is the first of several oddities at once says each: none
    /// is used up without its line.
    #[test]
    fn a_view_that_is_the_first_of_several_oddities_says_each() {
        let made = Made::new();
        let structure = |location| words_of(&structure_bytes(location));
        // Made with a resource, and its Location above 2^48.
        assert_eq!(
            firsts(made.count(&structure(1 << 50), false, true, None)),
            [First::WithResource(1 << 50), First::Wide(1 << 50), First::Location(1 << 50, 1)]
        );
        // Of placed memory, and a null view.
        assert_eq!(
            firsts(made.count(&structure(0), false, false, Some(0x9000))),
            [First::Placed(0, 0x9000), First::Null]
        );
        // Each was said: the same again says nothing, and is counted.
        assert_eq!(firsts(made.count(&structure(1 << 50), false, true, None)), []);
        assert_eq!(firsts(made.count(&structure(0), false, false, Some(0x9000))), []);
        // Three at once (a null view's Location is not also above 2^48), on
        // a table that has seen none.
        let made = Made::new();
        assert_eq!(
            firsts(made.count(&structure(0), false, true, Some(0x9000))),
            [First::WithResource(0), First::Placed(0, 0x9000), First::Null]
        );
        let all = firsts(made.count(&structure(3 << 48), false, true, Some(0x9000)));
        assert_eq!(all, [First::Wide(3 << 48), First::Location(3 << 48, 1)]);
        assert_eq!(
            made.counts(),
            Counts { structures: 2, null: 1, with_resource: 2, placed: 2, wide: 1, locations: 1, unknown: 0 }
        );
    }

    #[test]
    fn locations_past_those_logged_are_still_counted() {
        let made = Made::new();
        for i in 1..=LOGGED as u64 + 5 {
            let words = words_of(&structure_bytes(i << 16));
            let new = firsts(made.count(&words, false, false, None));
            assert_eq!(new.len(), usize::from(i <= LOGGED as u64), "{i}");
        }
        let counts = made.counts();
        assert_eq!((counts.structures, counts.locations), (LOGGED as u64 + 5, LOGGED));
    }

    /// The switch in both positions: off, a structure's view has no address
    /// of its own for shaders (the host heap then computes what it did
    /// before); on, its Location.
    #[test]
    fn a_structures_location_is_a_shaders_address_only_with_the_switch_on() {
        let structure = View::Shader(0, Some(words_of(&structure_bytes(0xd_4321_0000))));
        assert_eq!(address_if(&structure, true), Some(0xd_4321_0000));
        assert_eq!(address_if(&structure, false), None);
        // Whatever came with it, and of placed memory too.
        let with_resource = View::Shader(0x7000, Some(words_of(&structure_bytes(0x1_0000))));
        assert_eq!((address_if(&with_resource, true), address_if(&with_resource, false)), (Some(0x1_0000), None));
        let mut placed = [0u32; 16];
        placed[..10].copy_from_slice(&words_of(&structure_bytes(0x2_0000)));
        let placed = View::Placed(false, [0; 14], placed);
        assert_eq!((address_if(&placed, true), address_if(&placed, false)), (Some(0x2_0000), None));
        // A null structure view: Location 0 is still its address.
        let null = View::Shader(0, Some(words_of(&structure_bytes(0))));
        assert_eq!((address_if(&null, true), address_if(&null, false)), (Some(0), None));
        // No other view has one in either position.
        let buffer = View::Shader(0, Some([0, 1, 0x1688, 0, 0x4321_0000, 0xd, 16, 4, 0, 0]));
        for on in [false, true] {
            assert_eq!(address_if(&buffer, on), None);
            assert_eq!(address_if(&View::Shader(0, None), on), None);
            assert_eq!(address_if(&View::Constant(0x1_0000, 256), on), None);
        }
        // The process's own position is one of the two.
        assert_eq!(shader_address(&structure), address_if(&structure, addresses_on()));
    }

    /// What a first-of-its-like line says is done: with the switch off,
    /// nothing is said to be used that is not.
    #[test]
    fn a_line_says_what_is_done_in_the_switchs_position() {
        let odd = [First::Location(0x1000, 1), First::WithResource(0x1000), First::Placed(0x1000, 0x9000), First::Wide(1 << 50)];
        for first in odd {
            let (on, off) = (done(first, true), done(first, false));
            assert_ne!(on, off, "{first:?}");
            assert!(!on.contains("VICTORY_STRUCTURE_ADDRESSES"), "{on}");
            assert!(off.contains("with VICTORY_STRUCTURE_ADDRESSES=1") || off.contains("VICTORY_STRUCTURE_ADDRESSES=1 gives"), "{off}");
            assert!(off.contains("as before") || matches!(first, First::Wide(_)), "{off}");
        }
        for same in [First::Null, First::Unknown(12, false)] {
            assert_eq!(done(same, true), done(same, false));
        }
    }

    /// A table of its own gauge, so that tests running at once do not
    /// count on each other's.
    macro_rules! live {
        () => {{
            static GAUGE: Gauge = Gauge::new();
            (Live::on(&GAUGE), &GAUGE)
        }};
    }

    fn found(live: &Live) -> Structures {
        let mut found = Structures::none();
        live.add_to(&mut found);
        found
    }

    /// Create, copy, rewrite and free, as a host heap reports them: a
    /// structure is listed while a slot names it.
    #[test]
    fn a_structure_is_live_while_a_slot_names_it() {
        let (mut live, gauge) = live!();
        let (a, b) = (0xd_4321_0000u64, 0xd_6000_0000u64);
        assert!(found(&live).addresses.is_empty());
        // Made at slot 7, copied to 9 and to a table made afresh.
        assert!(live.set(7, Some(a)));
        assert!(live.set(9, Some(a)));
        assert!(live.set(1_000_000, Some(a)));
        assert_eq!((found(&live).addresses, live.len(), gauge.now()), (vec![a], 3, 3));
        // Another structure, then the first slot rewritten to name it.
        assert!(live.set(8, Some(b)));
        assert!(live.set(7, Some(b)));
        assert_eq!(found(&live), Structures { addresses: vec![a, b], slots: 4, unfollowed: 0 });
        // The same again changes nothing.
        assert!(live.set(7, Some(b)));
        assert_eq!((live.len(), gauge.now()), (4, 4));
        // Slot 9 gets another kind of view, the table is given back.
        assert!(live.set(9, None));
        assert_eq!(found(&live).addresses, [a, b], "the table still names it");
        live.clear(1_000_000, 4);
        assert_eq!(found(&live).addresses, [b]);
        // A null structure view names nothing.
        assert!(live.set(8, Some(0)));
        assert!(live.set(7, None));
        assert!(live.set(7, None), "a slot not followed");
        assert_eq!((found(&live), gauge.now()), (Structures::none(), 0));
        assert!(found(&live).complete());
    }

    #[test]
    fn a_released_heaps_slots_are_no_longer_counted() {
        let (mut live, gauge) = live!();
        for slot in 0..5 {
            live.set(slot, Some(0x1000));
        }
        assert_eq!(gauge.now(), 5);
        drop(live);
        assert_eq!((gauge.now(), gauge.underflows()), (0, 0));
    }

    #[test]
    fn clearing_takes_only_the_slots_given_back() {
        let (mut live, _) = live!();
        for slot in [10, 11, 12, 13, u32::MAX] {
            live.set(slot, Some(u64::from(slot) << 12));
        }
        live.clear(11, 2);
        assert_eq!(found(&live).addresses, [10 << 12, 13 << 12, u64::from(u32::MAX) << 12]);
        live.clear(u32::MAX, 5);
        assert_eq!(found(&live).addresses, [10 << 12, 13 << 12]);
        live.clear(0, 0);
        assert_eq!(live.len(), 2);
    }

    /// Why a heap forgets slots given back before they are free to take
    /// (`HostHeap::release`): clearing forgets whatever a slot holds, the
    /// view of whoever took the slot next too.
    #[test]
    fn clearing_a_slot_forgets_what_its_next_holder_made_there() {
        let (a, b) = (0xd_4321_0000u64, 0xd_6000_0000u64);
        // The order a heap keeps: cleared, then taken and made again.
        let (mut live, _) = live!();
        live.set(40, Some(a));
        live.clear(40, 1);
        live.set(40, Some(b));
        assert_eq!(found(&live), Structures { addresses: vec![b], slots: 1, unfollowed: 0 });
        // The other order loses the new view, and the list does not say so.
        let (mut live, gauge) = live!();
        live.set(40, Some(a));
        live.set(40, Some(b));
        live.clear(40, 1);
        assert_eq!((found(&live), gauge.now()), (Structures::none(), 0));
        assert!(found(&live).complete());
    }

    /// Past the table's room a slot is not followed, and the list says it
    /// is not complete from then on (what the slot holds later is unknown).
    #[test]
    fn a_full_table_says_its_list_is_not_complete() {
        let (mut live, gauge) = live!();
        for slot in 0..MAX_SLOTS as u32 {
            assert!(live.set(slot, Some(0x1000)));
        }
        assert!(found(&live).complete());
        assert!(!live.set(MAX_SLOTS as u32, Some(0x2000)), "no room");
        assert_eq!((live.len(), gauge.now()), (MAX_SLOTS, MAX_SLOTS as u64));
        let list = found(&live);
        assert_eq!((list.addresses.as_slice(), list.unfollowed, list.complete()), ([0x1000].as_slice(), 1, false));
        // A followed slot is still rewritten and freed.
        assert!(live.set(3, Some(0x3000)));
        assert!(live.set(4, None));
        assert_eq!(found(&live).addresses, [0x1000, 0x3000]);
        // Room again, but the slot refused before stays unknown.
        assert!(live.set(MAX_SLOTS as u32 + 1, Some(0x4000)));
        assert!(!found(&live).complete());
    }

    #[test]
    fn a_query_names_a_bounded_number_of_structures() {
        let (mut one, _) = live!();
        let (mut other, _) = live!();
        for i in 0..40u32 {
            one.set(i, Some(u64::from(i + 1) << 16));
            // Half of them the same structures as the first heap's.
            other.set(i, Some(u64::from(i + 21) << 16));
        }
        let mut list = Structures::none();
        one.add_to(&mut list);
        other.add_to(&mut list);
        assert_eq!((list.slots, list.addresses.len(), list.complete()), (80, 60, true));
        assert!(list.addresses.is_sorted());
        for i in 100..110u32 {
            other.set(i, Some(u64::from(i) << 16));
        }
        let mut list = Structures::none();
        one.add_to(&mut list);
        other.add_to(&mut list);
        assert_eq!((list.addresses.len(), list.unfollowed, list.complete()), (MAX_ADDRESSES, 6, false));
    }

    #[test]
    fn the_report_line_says_what_was_made_and_what_is_live() {
        let counts =
            Counts { structures: 12, null: 1, with_resource: 0, placed: 0, wide: 2, locations: 2, unknown: 3 };
        let live = Structures { addresses: vec![0xd_4321_0000, 0xd_6000_0000], slots: 5, unfollowed: 0 };
        assert_eq!(
            line(counts, &live, true),
            "ray-tracing structure views: 12 made (1 with Location 0, 0 with a resource, 0 of placed memory, \
             2 with a Location above 2^48), 2 distinct Locations; 5 slots of host view heaps hold one now, \
             naming 2 structures: 0xd43210000, 0xd60000000; views of a dimension not known: 3; \
             VICTORY_STRUCTURE_ADDRESSES: shaders read each one's Location as its address"
        );
        // The default position: said too, so that a log tells which run it is.
        let none = line(Counts::default(), &Structures::none(), false);
        assert!(none.starts_with("ray-tracing structure views: 0 made"), "{none}");
        assert!(none.contains("0 slots of host view heaps hold one now, naming 0 structures;"), "{none}");
        assert!(none.ends_with("; shaders do not read these addresses (VICTORY_STRUCTURE_ADDRESSES is off)"), "{none}");
        let partial = Structures { unfollowed: 4, ..live };
        let text = line(Counts { locations: LOGGED, ..counts }, &partial, false);
        assert!(text.contains("32 or more distinct Locations"), "{text}");
        assert!(text.contains("(not all: 4 not followed)"), "{text}");
        assert!(text.contains("2 with a Location above 2^48"), "{text}");
    }
}
