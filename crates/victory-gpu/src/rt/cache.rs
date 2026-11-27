//! The host structures a run holds, by the identity their caller gives
//! them, with who uses whom and when each one's memory may go. No D3D12
//! type is in it: a structure's memory is whatever payload the caller
//! stores (`super::device` stores its result buffer), and time is fence
//! values (`super::life`), so the lifetime rules are tested on any host.
//!
//! A [`Key`] names a structure for its caller: later, a title structure's
//! address. The content generation passed with it says which contents are
//! meant, so a source that changed is found stale and not served.
//!
//! The rules:
//!
//! * a top level refers to the bottom levels it was built over. Reading it
//!   reads them, so they are held with it; a bottom level that is given up
//!   stays, memory and all, while a top level built over it lives;
//! * when a bottom level is built into again (refit, rebuild or compacting
//!   copy), every top level built over it is stale: it holds the old
//!   address or the old bounds;
//! * memory a structure no longer uses (it was rebuilt elsewhere, or given
//!   up) is kept until every list that may read it is submitted and has
//!   run, then handed back for release ([`Cache::complete`]).
//!
//! **One open list.** The cache knows a single list being recorded: every
//! build and every read between two calls of [`Cache::seal`] (or
//! [`Cache::abandon`]) is in it, and they are submitted or dropped together.
//! That is what makes "readable later in the same list" and "a build in the
//! open list" mean something here. Several lists open at once (a title
//! records into many) are not modelled, and must not be mapped onto this
//! one: a second list could then read a structure whose build is in a list
//! not yet submitted, or one refitted in place after the read was recorded.
//! They would need each build and each read to carry its list, a read
//! refused while the structure's build is in another list that is not
//! submitted, and no build into memory another open list reads. None of
//! that is written.

use std::collections::HashMap;

use super::life::{Life, Refusal, State};
use super::retire::RetireQueue;
use super::update::Built;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    Bottom,
    Top,
}

/// A structure's identity for its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub level: Level,
    pub id: u64,
}

/// A structure in the cache. One that outlives its structure names nothing
/// ([`Refusal::Unknown`]), never another structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    slot: u32,
    version: u32,
}

/// What a lookup by key and generation finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    Missing,
    /// Holds that generation and may be read.
    Current(Handle),
    /// Known, but not readable as that generation: never built, stale, or
    /// built from another generation (and stale from now).
    NeedsBuild(Handle),
}

struct Entry<P> {
    /// `None` once given up.
    key: Option<Key>,
    level: Level,
    life: Life,
    /// The memory it is built in, with its bytes.
    payload: Option<(P, u64)>,
    built: Option<Built>,
    /// A top level: the distinct bottom levels it was last built over.
    bottoms: Vec<Handle>,
    /// A bottom level: the top levels built over it.
    tops: Vec<Handle>,
    /// Memory it was built in before, waiting for the lists that may read
    /// it to be submitted.
    parked: Vec<(P, u64)>,
}

struct Slot<P> {
    version: u32,
    entry: Option<Entry<P>>,
}

pub struct Cache<P> {
    slots: Vec<Slot<P>>,
    free: Vec<u32>,
    by_key: HashMap<Key, Handle>,
    /// Structures with a build in the list being recorded.
    open_builds: Vec<Handle>,
    /// What that list reads, once for each read: the structures bound in
    /// it, with the bottom levels under them, and the bottom levels its
    /// top-level builds read.
    open_reads: Vec<Handle>,
    /// Structures with a build submitted, by its fence value.
    in_flight: Vec<(u64, Handle)>,
    retired: RetireQueue<P>,
    /// The bytes of every payload not yet handed back.
    bytes: u64,
}

impl<P> Default for Cache<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P> Cache<P> {
    pub fn new() -> Self {
        Cache {
            slots: Vec::new(),
            free: Vec::new(),
            by_key: HashMap::new(),
            open_builds: Vec::new(),
            open_reads: Vec::new(),
            in_flight: Vec::new(),
            retired: RetireQueue::new(),
            bytes: 0,
        }
    }

    fn get(&self, handle: Handle) -> Option<&Entry<P>> {
        let slot = self.slots.get(handle.slot as usize)?;
        (slot.version == handle.version).then_some(slot.entry.as_ref()).flatten()
    }

    fn get_mut(&mut self, handle: Handle) -> Option<&mut Entry<P>> {
        let slot = self.slots.get_mut(handle.slot as usize)?;
        (slot.version == handle.version).then_some(slot.entry.as_mut()).flatten()
    }

    /// The structure `key` names, if there is one.
    pub fn find(&self, key: Key) -> Option<Handle> {
        self.by_key.get(&key).copied()
    }

    /// The structure `key` names, made empty if there is none.
    pub fn entry(&mut self, key: Key) -> Handle {
        if let Some(handle) = self.find(key) {
            return handle;
        }
        let entry = Entry {
            key: Some(key),
            level: key.level,
            life: Life::new(),
            payload: None,
            built: None,
            bottoms: Vec::new(),
            tops: Vec::new(),
            parked: Vec::new(),
        };
        let handle = match self.free.pop() {
            Some(slot) => {
                let at = &mut self.slots[slot as usize];
                at.entry = Some(entry);
                Handle { slot, version: at.version }
            }
            None => {
                self.slots.push(Slot { version: 0, entry: Some(entry) });
                Handle { slot: self.slots.len() as u32 - 1, version: 0 }
            }
        };
        self.by_key.insert(key, handle);
        handle
    }

    /// The structure `key` names, for a caller whose source is at content
    /// generation `generation`. One that holds another generation is stale
    /// from now on.
    pub fn lookup(&mut self, key: Key, generation: u64) -> Lookup {
        let Some(handle) = self.find(key) else { return Lookup::Missing };
        if let Some(entry) = self.get_mut(handle)
            && entry.life.generation() != generation
        {
            entry.life.invalidate();
        }
        match self.readable(handle) {
            Ok(()) => Lookup::Current(handle),
            Err(_) => Lookup::NeedsBuild(handle),
        }
    }

    pub fn level(&self, handle: Handle) -> Option<Level> {
        self.get(handle).map(|e| e.level)
    }

    pub fn state(&self, handle: Handle) -> Option<State> {
        self.get(handle).map(|e| e.life.state())
    }

    pub fn generation(&self, handle: Handle) -> Option<u64> {
        self.get(handle).map(|e| e.life.generation())
    }

    /// What is known of the structure's last build; `None` before one, and
    /// after one that was abandoned.
    pub fn built(&self, handle: Handle) -> Option<&Built> {
        self.get(handle)?.built.as_ref()
    }

    /// The memory the structure is built in.
    pub fn payload(&self, handle: Handle) -> Option<&P> {
        self.get(handle)?.payload.as_ref().map(|(payload, _)| payload)
    }

    /// The distinct bottom levels a top level was last built over.
    pub fn bottoms(&self, handle: Handle) -> &[Handle] {
        self.get(handle).map_or(&[], |e| &e.bottoms)
    }

    /// Whether something recorded now may read the structure: it is built
    /// with a barrier after, not stale and not given up; and for a top
    /// level, each bottom level it was built over can still be read
    /// through it.
    pub fn readable(&self, handle: Handle) -> Result<(), Refusal> {
        let entry = self.get(handle).ok_or(Refusal::Unknown)?;
        entry.life.readable()?;
        for &bottom in &entry.bottoms {
            let under = self.get(bottom).ok_or(Refusal::Bottom)?;
            under.life.readable_under_a_top().map_err(|_| Refusal::Bottom)?;
        }
        Ok(())
    }

    /// Whether a top level recorded now may be built over the structure: a
    /// bottom level that may be read now.
    pub fn buildable_over(&self, handle: Handle) -> Result<(), Refusal> {
        let entry = self.get(handle).ok_or(Refusal::Unknown)?;
        if entry.level != Level::Bottom {
            return Err(Refusal::Bottom);
        }
        entry.life.readable()
    }

    /// Whether [`Cache::begin_build`] would be allowed now.
    pub fn can_build(&self, handle: Handle) -> Result<(), Refusal> {
        self.get(handle).ok_or(Refusal::Unknown)?.life.can_build()
    }

    /// A build into the structure, to hold `generation`, is being recorded
    /// into the open list. Every top level built over it is stale from now.
    pub fn begin_build(&mut self, handle: Handle, generation: u64) -> Result<(), Refusal> {
        let entry = self.get_mut(handle).ok_or(Refusal::Unknown)?;
        entry.life.build(generation)?;
        let tops = entry.tops.clone();
        for top in tops {
            if let Some(top) = self.get_mut(top) {
                top.life.invalidate();
            }
        }
        if !self.open_builds.contains(&handle) {
            self.open_builds.push(handle);
        }
        Ok(())
    }

    /// The build begun with [`Cache::begin_build`] is recorded: what was
    /// built, the memory it was built into if that is new (`bytes` long;
    /// the memory used before waits for its last reader), and for a top
    /// level the bottom levels it was built over. The build reads those, so
    /// they are held as by any reader until the list is submitted.
    pub fn finish_build(&mut self, handle: Handle, built: Built, payload: Option<(P, u64)>, bottoms: &[Handle]) {
        let Some(entry) = self.get_mut(handle) else { return };
        entry.built = Some(built);
        let mut added = 0;
        if let Some(new) = payload {
            added = new.1;
            if let Some(old) = entry.payload.replace(new) {
                entry.parked.push(old);
            }
        }
        let mut distinct: Vec<Handle> = Vec::with_capacity(bottoms.len());
        for &bottom in bottoms {
            if !distinct.contains(&bottom) {
                distinct.push(bottom);
            }
        }
        let before = std::mem::replace(&mut entry.bottoms, distinct.clone());
        self.bytes += added;
        for &bottom in &distinct {
            if let Some(under) = self.get_mut(bottom) {
                under.life.pin();
                self.open_reads.push(bottom);
            }
        }
        for &bottom in distinct.iter().filter(|b| !before.contains(b)) {
            if let Some(under) = self.get_mut(bottom) {
                under.tops.push(handle);
            }
        }
        for &bottom in before.iter().filter(|b| !distinct.contains(b)) {
            self.unlink(bottom, handle);
        }
    }

    /// The barrier after the structure's build is recorded.
    pub fn barrier(&mut self, handle: Handle) {
        if let Some(entry) = self.get_mut(handle) {
            entry.life.barrier();
        }
    }

    /// The open list reads the structure (and through a top level its
    /// bottom levels): they are held until the list is sealed or abandoned.
    /// Refused, and nothing held, if it may not be read now.
    pub fn pin(&mut self, handle: Handle) -> Result<(), Refusal> {
        self.readable(handle)?;
        let bottoms = self.bottoms(handle).to_vec();
        for held in std::iter::once(handle).chain(bottoms) {
            if let Some(entry) = self.get_mut(held) {
                entry.life.pin();
                self.open_reads.push(held);
            }
        }
        Ok(())
    }

    /// The open list was submitted with fence value `fence`: its builds are
    /// in flight until that value is complete, and what it reads is held
    /// until then. The next build or read is in a new list.
    pub fn seal(&mut self, fence: u64) {
        for handle in std::mem::take(&mut self.open_builds) {
            if let Some(entry) = self.get_mut(handle) {
                entry.life.build_submitted(fence);
                self.in_flight.push((fence, handle));
            }
            self.settle(handle);
        }
        for handle in std::mem::take(&mut self.open_reads) {
            if let Some(entry) = self.get_mut(handle) {
                entry.life.sealed(fence);
            }
            self.settle(handle);
        }
    }

    /// The open list was dropped without being submitted: none of its
    /// builds runs and nothing in it reads. Each structure it built into is
    /// stale (or empty), and its next build is a whole one.
    pub fn abandon(&mut self) {
        for handle in std::mem::take(&mut self.open_builds) {
            if let Some(entry) = self.get_mut(handle) {
                entry.life.build_abandoned();
                entry.built = None;
            }
            self.settle(handle);
        }
        for handle in std::mem::take(&mut self.open_reads) {
            if let Some(entry) = self.get_mut(handle) {
                entry.life.unpinned();
            }
            self.settle(handle);
        }
    }

    /// Every submission up to fence value `completed` has run: builds among
    /// them are done, and the memory nothing later can read is handed back,
    /// for the caller to drop.
    pub fn complete(&mut self, completed: u64) -> Vec<P> {
        let mut waiting = Vec::new();
        for (fence, handle) in std::mem::take(&mut self.in_flight) {
            if fence > completed {
                waiting.push((fence, handle));
            } else if let Some(entry) = self.get_mut(handle) {
                entry.life.completed(completed);
            }
        }
        self.in_flight = waiting;
        let before = self.retired.bytes();
        let released = self.retired.release(completed);
        self.bytes -= before - self.retired.bytes();
        released
    }

    /// The structure is given up: its key names nothing, nothing new reads
    /// it, and its memory goes once no list and no top level can read it.
    /// A top level built over it is not made stale: it holds what it was
    /// built from, and is read as before if the structure could be read
    /// when it was given up (one given up while stale leaves its top levels
    /// unreadable until they are built again). Whether there was such a
    /// structure.
    pub fn retire(&mut self, handle: Handle) -> bool {
        let Some(entry) = self.get_mut(handle) else { return false };
        if entry.life.state() == State::Retired {
            return false;
        }
        entry.life.retire();
        let key = entry.key.take();
        let bottoms = std::mem::take(&mut entry.bottoms);
        if let Some(key) = key {
            self.by_key.remove(&key);
        }
        for bottom in bottoms {
            self.unlink(bottom, handle);
        }
        self.settle(handle);
        true
    }

    /// Every structure with a key, to give them all up.
    pub fn handles(&self) -> Vec<Handle> {
        self.by_key.values().copied().collect()
    }

    /// `top` is no longer built over `bottom`.
    fn unlink(&mut self, bottom: Handle, top: Handle) {
        if let Some(under) = self.get_mut(bottom) {
            under.tops.retain(|&t| t != top);
        }
        self.settle(bottom);
    }

    /// Moves memory the structure no longer uses to the queue once the last
    /// submission that may read it is known, and forgets a structure that
    /// is given up and held by nothing.
    fn settle(&mut self, handle: Handle) {
        let Some(entry) = self.get_mut(handle) else { return };
        let Some(after) = entry.life.releasable() else { return };
        let mut going = std::mem::take(&mut entry.parked);
        let gone = entry.life.state() == State::Retired && entry.tops.is_empty();
        if gone {
            going.extend(entry.payload.take());
        }
        for (payload, bytes) in going {
            self.retired.retire(payload, after, bytes);
        }
        if gone {
            let slot = &mut self.slots[handle.slot as usize];
            slot.entry = None;
            slot.version = slot.version.wrapping_add(1);
            self.free.push(handle.slot);
        }
    }

    /// Structures with a key.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// The bytes of all memory stored here and not yet handed back: live
    /// structures', and what waits for its last reader.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Memory given up and waiting for a fence value: how many pieces,
    /// their bytes, and the highest value any waits for.
    pub fn waiting(&self) -> (usize, u64, Option<u64>) {
        (self.retired.len(), self.retired.bytes(), self.retired.latest())
    }

    /// Whether a build is recorded in the open list.
    pub fn has_open_builds(&self) -> bool {
        !self.open_builds.is_empty()
    }

    /// Whether the open list reads anything.
    pub fn has_open_reads(&self) -> bool {
        !self.open_reads.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt::desc::{BuildFlags, Shape};

    const fn bottom(id: u64) -> Key {
        Key { level: Level::Bottom, id }
    }

    const fn top(id: u64) -> Key {
        Key { level: Level::Top, id }
    }

    fn built() -> Built {
        Built {
            shape: Shape::Top { flags: BuildFlags::default(), instances: 1 },
            refits: 0,
            compacted: false,
            result_bytes: 100,
            update_scratch_bytes: 0,
        }
    }

    /// Records a build of `key` into new memory `name`, with its barrier,
    /// in the open list.
    fn build(
        cache: &mut Cache<&'static str>,
        key: Key,
        generation: u64,
        name: &'static str,
        over: &[Handle],
    ) -> Handle {
        let handle = cache.entry(key);
        cache.begin_build(handle, generation).unwrap();
        cache.finish_build(handle, built(), Some((name, 100)), over);
        cache.barrier(handle);
        handle
    }

    /// A bottom level and a top level over it, built by submission 1, which
    /// has run.
    fn scene() -> (Cache<&'static str>, Handle, Handle) {
        let mut cache = Cache::new();
        let b = build(&mut cache, bottom(0x1000), 1, "b1", &[]);
        let t = build(&mut cache, top(0x2000), 1, "t1", &[b]);
        cache.seal(1);
        assert!(cache.complete(1).is_empty());
        (cache, b, t)
    }

    #[test]
    fn a_key_and_a_generation_find_what_may_be_read() {
        let mut cache: Cache<&str> = Cache::new();
        assert_eq!(cache.lookup(bottom(1), 1), Lookup::Missing);
        let b = cache.entry(bottom(1));
        assert_eq!(cache.entry(bottom(1)), b, "one structure a key");
        assert_ne!(cache.entry(top(1)), b, "the level is part of the key");
        assert_eq!(cache.lookup(bottom(1), 1), Lookup::NeedsBuild(b), "known and not built");
        build(&mut cache, bottom(1), 1, "b", &[]);
        assert_eq!(cache.lookup(bottom(1), 1), Lookup::Current(b), "readable after its barrier, in the same list");
        assert_eq!(cache.state(b), Some(State::Building));
        cache.seal(1);
        cache.complete(1);
        assert_eq!((cache.state(b), cache.generation(b)), (Some(State::Ready), Some(1)));
        assert_eq!(cache.lookup(bottom(1), 2), Lookup::NeedsBuild(b), "the source moved on");
        assert_eq!(cache.state(b), Some(State::Stale));
        assert_eq!(cache.lookup(bottom(1), 1), Lookup::NeedsBuild(b), "and going back does not make it current");
    }

    #[test]
    fn reading_a_top_level_holds_the_bottom_levels_it_was_built_over() {
        let (mut cache, b, t) = scene();
        assert_eq!(cache.bottoms(t), [b]);
        cache.pin(t).unwrap();
        assert!(cache.has_open_reads() && !cache.has_open_builds());
        // Both are given up while the list is still open.
        assert!(cache.retire(t));
        assert!(cache.retire(b));
        assert_eq!(cache.find(top(0x2000)), None);
        assert_eq!(cache.waiting().0, 0, "the open list may read both");
        assert!(cache.complete(100).is_empty());
        cache.seal(7);
        assert!(!cache.has_open_reads());
        assert_eq!(cache.waiting(), (2, 200, Some(7)));
        assert!(cache.complete(6).is_empty(), "submission 7 has not run");
        let mut released = cache.complete(7);
        released.sort_unstable();
        assert_eq!(released, ["b1", "t1"]);
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.state(b), None, "the handle names nothing now");
    }

    #[test]
    fn a_bottom_level_given_up_stays_while_a_top_level_over_it_lives() {
        let (mut cache, b, t) = scene();
        assert!(cache.retire(b));
        assert!(!cache.retire(b), "once");
        assert_eq!(cache.find(bottom(0x1000)), None);
        assert_eq!(cache.state(b), Some(State::Retired));
        assert!(cache.complete(50).is_empty(), "the top level can still be read, and reads it");
        assert_eq!(cache.readable(t), Ok(()));
        assert_eq!(cache.buildable_over(b), Err(Refusal::Retired), "nothing new is built over it");
        // A list reads the top level in submission 5, then it is given up.
        cache.pin(t).unwrap();
        cache.seal(5);
        cache.retire(t);
        assert_eq!(cache.waiting(), (2, 200, Some(5)), "the bottom level waits for the top level's last reader too");
        assert_eq!(cache.complete(5).len(), 2);
    }

    #[test]
    fn building_into_a_bottom_level_makes_the_top_levels_over_it_stale() {
        let (mut cache, b, t) = scene();
        // An earlier list read the top level in submission 2.
        cache.pin(t).unwrap();
        cache.seal(2);
        // A rebuild into new memory.
        cache.begin_build(b, 2).unwrap();
        assert_eq!(cache.state(t), Some(State::Stale));
        assert_eq!(cache.readable(t), Err(Refusal::Stale));
        assert_eq!(cache.pin(t), Err(Refusal::Stale));
        assert!(!cache.has_open_reads(), "a refused read holds nothing");
        cache.finish_build(b, built(), Some(("b2", 100)), &[]);
        cache.barrier(b);
        assert_eq!(cache.payload(b), Some(&"b2"));
        assert_eq!(cache.waiting().0, 0, "the old memory waits for the open list");
        cache.seal(3);
        assert_eq!(cache.waiting(), (1, 100, Some(3)));
        assert_eq!(cache.complete(2), [""; 0], "submission 2 read it through the top level; 3 may too");
        assert_eq!(cache.complete(3), ["b1"]);
        // The top level is read again once it is built over the new one.
        let t2 = build(&mut cache, top(0x2000), 2, "t2", &[b]);
        assert_eq!(t2, t);
        assert_eq!(cache.readable(t), Ok(()));
        cache.seal(4);
        assert_eq!(cache.complete(4), ["t1"]);
        assert_eq!(cache.bytes(), 200);
    }

    #[test]
    fn a_refit_in_place_parks_nothing_and_still_makes_the_top_level_stale() {
        let (mut cache, b, t) = scene();
        cache.begin_build(b, 2).unwrap();
        cache.finish_build(b, Built { refits: 1, ..built() }, None, &[]);
        cache.barrier(b);
        assert_eq!((cache.payload(b), cache.built(b).map(|b| b.refits)), (Some(&"b1"), Some(1)));
        assert_eq!(cache.readable(b), Ok(()));
        assert_eq!(cache.readable(t), Err(Refusal::Stale), "its bounds may have moved");
        cache.seal(2);
        assert_eq!(cache.complete(2), [""; 0]);
        assert_eq!(cache.bytes(), 200);
    }

    #[test]
    fn a_top_level_is_not_built_over_what_cannot_be_read() {
        let mut cache: Cache<&str> = Cache::new();
        let b = cache.entry(bottom(1));
        assert_eq!(cache.buildable_over(b), Err(Refusal::Empty));
        cache.begin_build(b, 1).unwrap();
        cache.finish_build(b, built(), Some(("b", 100)), &[]);
        assert_eq!(cache.buildable_over(b), Err(Refusal::NoBarrier), "its build is not ordered before the top level's");
        cache.barrier(b);
        assert_eq!(cache.buildable_over(b), Ok(()));
        let t = build(&mut cache, top(1), 1, "t", &[b]);
        assert_eq!(cache.buildable_over(t), Err(Refusal::Bottom), "a top level is no instance");
        cache.lookup(bottom(1), 2);
        assert_eq!(cache.buildable_over(b), Err(Refusal::Stale));
        assert_eq!(cache.readable(t), Err(Refusal::Bottom), "a top level over a stale bottom level is not read");
    }

    /// Giving a stale bottom level up does not make the top level over it
    /// readable again: it would be read over contents its caller has said
    /// are out of date. It is read again once it is built over something
    /// that can be.
    #[test]
    fn a_top_level_over_a_bottom_level_given_up_while_stale_is_not_read() {
        let (mut cache, b, t) = scene();
        assert_eq!(cache.lookup(top(0x2000), 1), Lookup::Current(t));
        // The bottom level's source moves on.
        assert_eq!(cache.lookup(bottom(0x1000), 2), Lookup::NeedsBuild(b));
        assert_eq!(cache.readable(t), Err(Refusal::Bottom));
        assert_eq!(cache.lookup(top(0x2000), 1), Lookup::NeedsBuild(t));
        // And is then freed.
        assert!(cache.retire(b));
        assert_eq!(cache.state(b), Some(State::Retired), "held by the top level");
        assert_eq!(cache.readable(t), Err(Refusal::Bottom), "given up is not brought up to date");
        assert_eq!(cache.lookup(top(0x2000), 1), Lookup::NeedsBuild(t));
        assert_eq!(cache.pin(t), Err(Refusal::Bottom));
        assert!(!cache.has_open_reads());
        // Built again over another bottom level, it is read, and the old
        // one goes.
        let other = build(&mut cache, bottom(0x1100), 1, "c1", &[]);
        build(&mut cache, top(0x2000), 2, "t2", &[other]);
        assert_eq!(cache.lookup(top(0x2000), 2), Lookup::Current(t));
        assert_eq!(cache.state(b), None);
        cache.seal(2);
        let mut released = cache.complete(2);
        released.sort_unstable();
        assert_eq!(released, ["b1", "t1"]);

        // The same for a bottom level whose refit was recorded and dropped.
        let (mut cache, b, t) = scene();
        cache.begin_build(b, 2).unwrap();
        cache.finish_build(b, Built { refits: 1, ..built() }, None, &[]);
        cache.barrier(b);
        cache.abandon();
        assert_eq!(cache.state(b), Some(State::Stale));
        cache.retire(b);
        assert_eq!(cache.readable(t), Err(Refusal::Stale), "made stale when the refit was recorded");
        // And a top level left readable by a bottom level given up while it
        // could be read stays so.
        let (mut cache, b, t) = scene();
        cache.retire(b);
        assert_eq!(cache.lookup(top(0x2000), 1), Lookup::Current(t));
    }

    /// The cache knows one open list: what is recorded between two seals
    /// is submitted, or dropped, together. A read of a structure whose
    /// build is in that list is in that list too, so it never runs without
    /// the build.
    #[test]
    fn reads_and_builds_of_the_open_list_are_submitted_or_dropped_together() {
        let mut cache: Cache<&str> = Cache::new();
        let b = build(&mut cache, bottom(1), 1, "b", &[]);
        let t = build(&mut cache, top(1), 1, "t", &[b]);
        cache.pin(t).unwrap();
        assert!(cache.has_open_builds() && cache.has_open_reads());
        // Dropped: the builds never run, and nothing read them.
        cache.abandon();
        assert!(!cache.has_open_builds() && !cache.has_open_reads());
        assert_eq!((cache.state(b), cache.state(t)), (Some(State::Empty), Some(State::Empty)));
        assert_eq!(cache.pin(t), Err(Refusal::Empty));
        cache.retire(t);
        cache.retire(b);
        let mut released = cache.complete(0);
        released.sort_unstable();
        assert_eq!(released, ["b", "t"], "memory no list ever named goes at once");
        // Submitted: the read's fence value is the builds'.
        let b = build(&mut cache, bottom(1), 1, "b", &[]);
        let t = build(&mut cache, top(1), 1, "t", &[b]);
        cache.pin(t).unwrap();
        cache.seal(4);
        cache.retire(t);
        cache.retire(b);
        assert_eq!(cache.waiting(), (2, 200, Some(4)));
        assert!(cache.complete(3).is_empty());
        assert_eq!(cache.complete(4).len(), 2);
    }

    #[test]
    fn a_top_level_built_again_over_other_bottom_levels_lets_the_old_ones_go() {
        let (mut cache, b, t) = scene();
        let other = build(&mut cache, bottom(0x1100), 1, "c1", &[]);
        cache.retire(b);
        assert_eq!(cache.state(b), Some(State::Retired), "held by the top level");
        build(&mut cache, top(0x2000), 2, "t2", &[other, other]);
        assert_eq!(cache.bottoms(t), [other], "each bottom level once");
        assert_eq!(cache.state(b), None, "nothing holds it now");
        cache.seal(2);
        let mut released = cache.complete(2);
        released.sort_unstable();
        assert_eq!(released, ["b1", "t1"]);
        // And the new bottom level is now what the top level holds.
        cache.retire(other);
        assert_eq!(cache.state(other), Some(State::Retired));
        assert!(cache.complete(99).is_empty());
    }

    /// A top level's build reads its bottom levels: one given up right
    /// after is kept until that build has run, though nothing else ever
    /// read it.
    #[test]
    fn a_top_level_build_holds_what_it_reads() {
        let mut cache: Cache<&str> = Cache::new();
        let b = build(&mut cache, bottom(1), 1, "b", &[]);
        cache.seal(1);
        assert!(cache.complete(1).is_empty());
        let t = build(&mut cache, top(1), 1, "t", &[b]);
        cache.retire(t);
        cache.retire(b);
        assert_eq!(cache.waiting().0, 0, "the build is in the open list");
        cache.seal(4);
        assert_eq!(cache.waiting(), (2, 200, Some(4)));
        assert!(cache.complete(3).is_empty(), "submission 4 builds over it");
        assert_eq!(cache.complete(4).len(), 2);
    }

    #[test]
    fn a_dropped_list_undoes_its_pins_and_its_builds() {
        let (mut cache, b, t) = scene();
        cache.pin(t).unwrap();
        cache.retire(t);
        cache.abandon();
        assert_eq!(cache.waiting(), (1, 100, Some(1)), "only its build ever used it");
        // A refit recorded and never submitted: stale, and the next build
        // is a whole one.
        cache.begin_build(b, 2).unwrap();
        cache.finish_build(b, Built { refits: 1, ..built() }, None, &[]);
        cache.barrier(b);
        assert!(cache.has_open_builds());
        cache.abandon();
        assert!(!cache.has_open_builds());
        assert_eq!(cache.state(b), Some(State::Stale));
        assert!(cache.built(b).is_none());
        // A first build recorded and never submitted: empty; its memory was
        // never used and goes at once when it is given up.
        let fresh = build(&mut cache, bottom(9), 1, "f", &[]);
        cache.abandon();
        assert_eq!(cache.state(fresh), Some(State::Empty));
        cache.retire(fresh);
        let mut released = cache.complete(1);
        released.sort_unstable();
        assert_eq!(released, ["f", "t1"]);
    }

    #[test]
    fn a_handle_that_outlived_its_structure_names_nothing() {
        let mut cache: Cache<&str> = Cache::new();
        let old = build(&mut cache, bottom(1), 1, "old", &[]);
        cache.seal(1);
        cache.retire(old);
        assert_eq!(cache.complete(1), ["old"]);
        let new = build(&mut cache, bottom(2), 1, "new", &[]);
        assert_ne!(new, old, "the slot is used again under another version");
        assert_eq!(cache.readable(old), Err(Refusal::Unknown));
        assert_eq!(cache.begin_build(old, 1), Err(Refusal::Unknown));
        assert_eq!(cache.pin(old), Err(Refusal::Unknown));
        assert!(!cache.retire(old));
        assert_eq!(cache.payload(old), None);
        assert_eq!(cache.payload(new), Some(&"new"));
        assert_eq!((cache.len(), cache.handles()), (1, vec![new]));
    }

    /// The property the whole module is for, under any interleaving of
    /// builds, refits, copies, reads, submissions, completions, dropped
    /// lists and structures given up: no memory is handed back while a list
    /// that names it is open or has not run, and all of it is handed back
    /// in the end, once. The oracle beside the cache knows only which
    /// memory each list names: what a build writes (and a refit or a copy
    /// reads), the bottom-level memory a top-level build reads, and for a
    /// read of a top level its memory and the bottom-level memory it was
    /// built over.
    #[test]
    fn no_interleaving_releases_memory_a_list_may_still_read() {
        use std::collections::{HashMap, HashSet};
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let (mut reads, mut early_refusals) = (0u32, 0u32);
        for _ in 0..400 {
            let mut cache: Cache<u32> = Cache::new();
            let mut made = 0u32;
            // The oracle: the highest fence value of a submitted list that
            // names each piece of memory, what the open list names, what a
            // top level's memory was built over, what was handed back.
            let mut last_use: HashMap<u32, u64> = HashMap::new();
            let mut open: HashSet<u32> = HashSet::new();
            let mut under: HashMap<u32, Vec<u32>> = HashMap::new();
            let mut released: HashSet<u32> = HashSet::new();
            let (mut fence, mut completed) = (0u64, 0u64);
            let mut check = |handed: Vec<u32>, completed: u64, open: &HashSet<u32>, last_use: &HashMap<u32, u64>| {
                for memory in handed {
                    assert!(!open.contains(&memory), "memory {memory} handed back while the open list names it");
                    let used = last_use.get(&memory).copied().unwrap_or(0);
                    assert!(used <= completed, "memory {memory} handed back at {completed}, used by {used}");
                    assert!(released.insert(memory), "memory {memory} handed back twice");
                }
            };
            for _ in 0..80 {
                match next(11) {
                    // A bottom level is built: in place (a refit), into new
                    // memory, or into new memory from the old (a copy).
                    0..=2 => {
                        let handle = cache.entry(bottom(next(3)));
                        if cache.can_build(handle).is_err() {
                            continue;
                        }
                        let old = cache.payload(handle).copied();
                        let in_place = old.is_some() && cache.built(handle).is_some() && next(2) == 0;
                        cache.begin_build(handle, next(4)).unwrap();
                        let new = (!in_place).then(|| {
                            made += 1;
                            (made, 10)
                        });
                        open.insert(new.map_or_else(|| old.unwrap(), |(memory, _)| memory));
                        if let (Some(old), true) = (old, !in_place && next(2) == 0) {
                            open.insert(old);
                        }
                        cache.finish_build(handle, built(), new, &[]);
                        if next(6) != 0 {
                            cache.barrier(handle);
                        }
                    }
                    // A top level is built over bottom levels that may be
                    // built over now.
                    3 | 4 => {
                        let handle = cache.entry(top(next(2)));
                        let over: Vec<Handle> = (0..3)
                            .filter_map(|id| cache.find(bottom(id)))
                            .filter(|&b| cache.buildable_over(b).is_ok() && next(3) != 0)
                            .collect();
                        if cache.can_build(handle).is_err() {
                            continue;
                        }
                        let old = cache.payload(handle).copied();
                        let in_place = old.is_some() && cache.built(handle).is_some() && next(2) == 0;
                        cache.begin_build(handle, next(4)).unwrap();
                        let new = (!in_place).then(|| {
                            made += 1;
                            (made, 10)
                        });
                        let memory = new.map_or_else(|| old.unwrap(), |(memory, _)| memory);
                        let read: Vec<u32> = over.iter().map(|&b| *cache.payload(b).unwrap()).collect();
                        open.insert(memory);
                        open.extend(&read);
                        under.insert(memory, read);
                        cache.finish_build(handle, built(), new, &over);
                        if next(6) != 0 {
                            cache.barrier(handle);
                        }
                    }
                    // A list reads a top level, if it may.
                    5 | 6 => {
                        let Some(handle) = cache.find(top(next(2))) else { continue };
                        match cache.pin(handle) {
                            Ok(()) => {
                                let memory = *cache.payload(handle).unwrap();
                                // What it reads through to is what the cache
                                // holds for it.
                                let mut held: Vec<u32> =
                                    cache.bottoms(handle).iter().map(|&b| *cache.payload(b).unwrap()).collect();
                                let mut built_over = under[&memory].clone();
                                held.sort_unstable();
                                built_over.sort_unstable();
                                built_over.dedup();
                                assert_eq!(held, built_over, "a readable top level over memory it was not built over");
                                open.insert(memory);
                                open.extend(&under[&memory]);
                                reads += 1;
                            }
                            Err(_) => early_refusals += 1,
                        }
                    }
                    // The open list is submitted.
                    7 => {
                        fence += 1;
                        cache.seal(fence);
                        for memory in open.drain() {
                            last_use.insert(memory, fence);
                        }
                    }
                    // Some submissions have run.
                    8 => {
                        completed = fence.min(completed + next(3));
                        check(cache.complete(completed), completed, &open, &last_use);
                    }
                    // A structure is given up.
                    9 => {
                        let key = if next(2) == 0 { bottom(next(3)) } else { top(next(2)) };
                        if let Some(handle) = cache.find(key) {
                            cache.retire(handle);
                        }
                    }
                    // The open list is dropped: it never runs.
                    _ => {
                        cache.abandon();
                        open.clear();
                    }
                }
            }
            // The end of a run: the open list goes, everything is given up,
            // and every submission runs.
            fence += 1;
            cache.seal(fence);
            for memory in open.drain() {
                last_use.insert(memory, fence);
            }
            for handle in cache.handles() {
                cache.retire(handle);
            }
            check(cache.complete(fence - 1), fence - 1, &open, &last_use);
            check(cache.complete(fence), fence, &open, &last_use);
            assert_eq!(released.len() as u32, made, "all memory made is handed back");
            assert_eq!((cache.bytes(), cache.waiting().0, cache.len()), (0, 0, 0));
        }
        assert!(reads > 500 && early_refusals > 500, "{reads} reads, {early_refusals} refusals: both paths are walked");
    }

    #[test]
    fn bytes_count_everything_not_yet_handed_back() {
        let (mut cache, b, t) = scene();
        assert_eq!(cache.bytes(), 200);
        cache.begin_build(b, 2).unwrap();
        cache.finish_build(b, built(), Some(("b2", 60)), &[]);
        cache.barrier(b);
        assert_eq!(cache.bytes(), 260, "the old memory is still held");
        cache.seal(2);
        cache.retire(t);
        assert_eq!(cache.bytes(), 260);
        assert_eq!(cache.complete(1), ["t1"]);
        assert_eq!(cache.bytes(), 160);
        assert_eq!(cache.complete(2), ["b1"]);
        assert_eq!(cache.bytes(), 60);
    }
}
