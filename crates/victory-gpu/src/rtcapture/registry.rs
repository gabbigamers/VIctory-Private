//! Which acceleration structure was last described at which destination.
//!
//! An entry is the description of the last build, update, unpacking or copy
//! recorded for a destination address, in the order the title recorded
//! them. That is not the order the GPU runs them in when two lists name
//! one destination, and a list that is recorded and never submitted still
//! leaves its entry: each entry says which list it was recorded on, for
//! whoever orders them by submission later.
//!
//! An entry goes when its destination is described again (replaced), when
//! the runtime maps or unmaps the memory its address lies in
//! ([`Registry::remove_range`]), or when what was written there is not
//! known ([`Registry::remove`]: a build that could not be read, a call the
//! builder failed, a copy that is not applied). Nothing reports a
//! structure the title simply stops using inside one of its own pools, so
//! entries for those stay until the address is built at again; and a
//! structure's length is not known here, so a build that overlaps another
//! without starting at its address does not remove it.
//!
//! Bounded in entries and in bytes; what does not fit is refused, and its
//! bytes are counted through `crate::account` for as long as an entry
//! lives.

use std::collections::BTreeMap;

use super::desc::{Description, Level, Postbuild};
use crate::account::{Gauge, Held};

/// How a description came to be at its destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Built or updated from the description.
    Build,
    /// Unpacked ("rehydrated") from a packed structure the title shipped;
    /// the description came with the call.
    Rehydrate,
    /// Copied from the structure at `from`, whose entry then had the
    /// sequence number `sequence`.
    Copy { from: u64, sequence: u64 },
}

pub struct Entry {
    pub description: Description,
    /// What the title asked to be written about the result.
    pub postbuild: Vec<Postbuild>,
    pub origin: Origin,
    /// The command list the call was recorded on (the title's address of
    /// it); zero for a call that takes no list.
    pub list: u64,
    /// How many records this destination has had while an entry lived
    /// there, less one: 0 for the first. Starts again after a removal, so
    /// it does not identify a structure; `sequence` does.
    pub generation: u32,
    /// The record's number among all of this registry's, from 1: unique,
    /// and newer is higher.
    pub sequence: u64,
    /// The bottom levels a top level's instances named when recorded:
    /// each address once, ascending, null left out. Empty for a bottom
    /// level.
    bottom_levels: Vec<u64>,
    held: Held,
}

impl Entry {
    pub fn bottom_levels(&self) -> &[u64] {
        &self.bottom_levels
    }

    /// The bytes counted for this entry.
    pub fn bytes(&self) -> u64 {
        self.held.bytes()
    }
}

/// The most a registry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub entries: usize,
    pub bytes: u64,
}

impl Bounds {
    /// 65,536 structures and 64 MiB. Other forks report some 3,000 static
    /// structures and a few hundred dynamic ones in the title's opening;
    /// a top level of 100,000 instances is 6.4 MiB.
    pub const DEFAULT: Bounds = Bounds {
        entries: 1 << 16,
        bytes: 64 << 20,
    };
}

/// What [`Registry::record`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub generation: u32,
    pub sequence: u64,
    /// Whether an entry was there before.
    pub replaced: bool,
}

/// The registry is full: the record was not kept. Whatever entry the
/// destination had is gone too, since it no longer describes what is
/// there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Full {
    pub removed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyRefusal {
    /// Nothing is known at the source; the destination's entry, if any,
    /// was removed (`removed`).
    UnknownSource {
        removed: bool,
    },
    /// Source and destination are one address.
    Same,
    Full(Full),
}

/// The bottom levels a top level names, by whether the registry knows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct References {
    /// Address and sequence number of each one known.
    pub known: Vec<(u64, u64)>,
    /// Addresses nothing was recorded for (or whose entry is of a top
    /// level).
    pub unknown: Vec<u64>,
}

pub struct Registry {
    entries: BTreeMap<u64, Entry>,
    bounds: Bounds,
    bytes: u64,
    sequence: u64,
    gauge: &'static Gauge,
}

impl Registry {
    /// A registry whose entries' bytes are counted on `gauge`.
    pub fn new(bounds: Bounds, gauge: &'static Gauge) -> Self {
        Registry {
            entries: BTreeMap::new(),
            bounds,
            bytes: 0,
            sequence: 0,
            gauge,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The bytes of all entries, as counted.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The number of the newest record: it changes whenever an entry is
    /// added or replaced.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn get(&self, destination: u64) -> Option<&Entry> {
        self.entries.get(&destination)
    }

    /// Every entry, by ascending destination.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &Entry)> {
        self.entries.iter().map(|(&at, entry)| (at, entry))
    }

    /// Keeps `description` as what is at its destination now, in place of
    /// any entry there.
    pub fn record(
        &mut self,
        description: Description,
        postbuild: Vec<Postbuild>,
        origin: Origin,
        list: u64,
    ) -> Result<Recorded, Full> {
        let mut bottom_levels: Vec<u64> = match &description.level {
            Level::Top(top) => top
                .instances
                .iter()
                .map(|i| i.bottom_level)
                .filter(|&b| b != 0)
                .collect(),
            Level::Bottom(_) => Vec::new(),
        };
        bottom_levels.sort_unstable();
        bottom_levels.dedup();
        bottom_levels.shrink_to_fit();
        let bytes = size_of::<Entry>() as u64
            + description.heap_bytes()
            + (postbuild.capacity() * size_of::<Postbuild>()) as u64
            + (bottom_levels.capacity() * size_of::<u64>()) as u64;
        // What was there is overwritten, whether or not the new one fits.
        let previous = self.take(description.destination);
        let fits = self.entries.len() < self.bounds.entries
            && self
                .bytes
                .checked_add(bytes)
                .is_some_and(|all| all <= self.bounds.bytes);
        if !fits {
            return Err(Full {
                removed: previous.is_some(),
            });
        }
        self.sequence += 1;
        let recorded = Recorded {
            generation: previous.map_or(0, |generation| generation.saturating_add(1)),
            sequence: self.sequence,
            replaced: previous.is_some(),
        };
        self.bytes += bytes;
        self.entries.insert(
            description.destination,
            Entry {
                description,
                postbuild,
                origin,
                list,
                generation: recorded.generation,
                sequence: recorded.sequence,
                bottom_levels,
                held: Held::on(self.gauge, bytes),
            },
        );
        Ok(recorded)
    }

    /// The structure at `source` was copied to `destination` (a clone or a
    /// compaction: the same geometry or instances): the destination gets
    /// the source's description. The source's entry stays.
    pub fn copy(
        &mut self,
        source: u64,
        destination: u64,
        list: u64,
    ) -> Result<Recorded, CopyRefusal> {
        if source == destination {
            return Err(CopyRefusal::Same);
        }
        let Some(from) = self.entries.get(&source) else {
            let removed = self.remove(destination);
            return Err(CopyRefusal::UnknownSource { removed });
        };
        let description = Description {
            destination,
            ..from.description.clone()
        };
        let origin = Origin::Copy {
            from: source,
            sequence: from.sequence,
        };
        self.record(description, Vec::new(), origin, list)
            .map_err(CopyRefusal::Full)
    }

    /// Removes the entry at `destination`, giving its generation.
    fn take(&mut self, destination: u64) -> Option<u32> {
        let entry = self.entries.remove(&destination)?;
        self.bytes -= entry.bytes();
        Some(entry.generation)
    }

    /// Forgets what is at `destination`; whether there was an entry.
    pub fn remove(&mut self, destination: u64) -> bool {
        self.take(destination).is_some()
    }

    /// Forgets every entry whose destination lies in `[start, start +
    /// len)`: that memory shows something else now. How many went.
    pub fn remove_range(&mut self, start: u64, len: u64) -> usize {
        let end = start.saturating_add(len);
        let gone: Vec<u64> = self.entries.range(start..end).map(|(&at, _)| at).collect();
        for &at in &gone {
            self.take(at);
        }
        gone.len()
    }

    /// The bottom levels the top level at `destination` names, by whether
    /// a bottom level is recorded at each address; `None` if no top level
    /// is recorded there.
    pub fn resolve(&self, destination: u64) -> Option<References> {
        let top = self
            .entries
            .get(&destination)
            .filter(|e| e.description.is_top())?;
        let mut references = References::default();
        for &at in &top.bottom_levels {
            match self.entries.get(&at).filter(|e| !e.description.is_top()) {
                Some(bottom) => references.known.push((at, bottom.sequence)),
                None => references.unknown.push(at),
            }
        }
        Some(references)
    }
}

#[cfg(test)]
mod tests {
    use super::super::desc::image::{some_instance, some_triangles};
    use super::super::desc::{BottomLevel, Geometry, GeometryKind, Layout, TopLevel, build_flags};
    use super::*;

    fn gauge() -> &'static Gauge {
        Box::leak(Box::new(Gauge::new()))
    }

    fn bottom(destination: u64, geometries: u32) -> Description {
        Description {
            destination,
            source: 0,
            scratch: 0x5000,
            flags: build_flags::ALLOW_COMPACTION,
            level: Level::Bottom(BottomLevel {
                layout: Layout::Array,
                address: 0x7000,
                geometries: (0..geometries)
                    .map(|n| Geometry {
                        flags: 1,
                        kind: GeometryKind::Triangles(some_triangles(n)),
                    })
                    .collect(),
            }),
        }
    }

    fn top(destination: u64, bottoms: &[u64]) -> Description {
        Description {
            destination,
            source: 0,
            scratch: 0x5000,
            flags: 0,
            level: Level::Top(TopLevel {
                layout: Layout::Array,
                address: 0x9000,
                instances: bottoms
                    .iter()
                    .enumerate()
                    .map(|(n, &b)| some_instance(n as u32, b))
                    .collect(),
            }),
        }
    }

    #[test]
    fn a_rebuild_replaces_the_entry_and_counts_a_generation() {
        let gauge = gauge();
        let mut r = Registry::new(Bounds::DEFAULT, gauge);
        let first = r
            .record(bottom(0x1000, 2), Vec::new(), Origin::Build, 7)
            .unwrap();
        assert_eq!(
            first,
            Recorded {
                generation: 0,
                sequence: 1,
                replaced: false
            }
        );
        let other = r
            .record(bottom(0x2000, 1), Vec::new(), Origin::Build, 7)
            .unwrap();
        assert_eq!((other.generation, other.sequence), (0, 2));

        let mut update = bottom(0x1000, 3);
        update.flags |= build_flags::PERFORM_UPDATE;
        update.source = 0x1000;
        let second = r
            .record(update.clone(), Vec::new(), Origin::Build, 8)
            .unwrap();
        assert_eq!(
            second,
            Recorded {
                generation: 1,
                sequence: 3,
                replaced: true
            }
        );
        assert_eq!((r.len(), r.sequence()), (2, 3));
        let entry = r.get(0x1000).unwrap();
        assert_eq!(entry.description, update);
        assert_eq!((entry.list, entry.generation, entry.sequence), (8, 1, 3));
        assert_eq!(r.get(0x2000).unwrap().sequence, 2);
        // Counted while they live, and exactly.
        assert_eq!(gauge.now(), r.bytes());
        assert_eq!(r.bytes(), r.iter().map(|(_, e)| e.bytes()).sum::<u64>());
        drop(r);
        assert_eq!((gauge.now(), gauge.underflows()), (0, 0));
    }

    #[test]
    fn a_top_level_knows_its_bottom_levels_once_each() {
        let mut r = Registry::new(Bounds::DEFAULT, gauge());
        r.record(bottom(0x1000, 1), Vec::new(), Origin::Build, 0)
            .unwrap();
        r.record(bottom(0x3000, 1), Vec::new(), Origin::Rehydrate, 0)
            .unwrap();
        // Named twice, a null, one never recorded, and the top level itself.
        let names = [0x3000, 0x1000, 0, 0x3000, 0x2000, 0x8000];
        r.record(top(0x8000, &names), Vec::new(), Origin::Build, 0)
            .unwrap();
        assert_eq!(
            r.get(0x8000).unwrap().bottom_levels(),
            [0x1000, 0x2000, 0x3000, 0x8000]
        );
        assert_eq!(
            r.resolve(0x8000).unwrap(),
            References {
                known: vec![(0x1000, 1), (0x3000, 2)],
                unknown: vec![0x2000, 0x8000],
            }
        );
        assert_eq!(r.resolve(0x1000), None, "not a top level");
        assert_eq!(r.resolve(0x4000), None);

        // The missing one arrives; one that was there is rebuilt.
        r.record(bottom(0x2000, 1), Vec::new(), Origin::Build, 0)
            .unwrap();
        r.record(bottom(0x1000, 4), Vec::new(), Origin::Build, 0)
            .unwrap();
        let now = r.resolve(0x8000).unwrap();
        assert_eq!(now.known, [(0x1000, 5), (0x2000, 4), (0x3000, 2)]);
        assert_eq!(now.unknown, [0x8000]);
    }

    #[test]
    fn a_copy_gives_the_destination_the_sources_description() {
        let mut r = Registry::new(Bounds::DEFAULT, gauge());
        r.record(bottom(0x1000, 3), Vec::new(), Origin::Build, 1)
            .unwrap();
        let copied = r.copy(0x1000, 0x6000, 2).unwrap();
        assert_eq!(
            (copied.generation, copied.sequence, copied.replaced),
            (0, 2, false)
        );
        let (source, compact) = (r.get(0x1000).unwrap(), r.get(0x6000).unwrap());
        assert_eq!(compact.description.destination, 0x6000);
        assert_eq!(compact.description.level, source.description.level);
        assert_eq!(compact.description.flags, source.description.flags);
        assert_eq!(
            compact.origin,
            Origin::Copy {
                from: 0x1000,
                sequence: 1
            }
        );
        assert_eq!((compact.list, source.sequence), (2, 1));

        // From nowhere: the destination is not what it was either.
        assert_eq!(
            r.copy(0x7000, 0x6000, 2),
            Err(CopyRefusal::UnknownSource { removed: true })
        );
        assert!(r.get(0x6000).is_none());
        assert_eq!(
            r.copy(0x7000, 0x6000, 2),
            Err(CopyRefusal::UnknownSource { removed: false })
        );
        assert_eq!(r.copy(0x1000, 0x1000, 2), Err(CopyRefusal::Same));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn entries_go_with_the_memory_they_lie_in() {
        let gauge = gauge();
        let mut r = Registry::new(Bounds::DEFAULT, gauge);
        for at in [0x1_0000, 0x1_ff00, 0x2_0000, 0x2_ffff, 0x3_0000] {
            r.record(bottom(at, 1), Vec::new(), Origin::Build, 0)
                .unwrap();
        }
        // The page from 0x20000: its first and last byte, not its neighbours.
        assert_eq!(r.remove_range(0x2_0000, 0x1_0000), 2);
        let left: Vec<u64> = r.iter().map(|(at, _)| at).collect();
        assert_eq!(left, [0x1_0000, 0x1_ff00, 0x3_0000]);
        assert_eq!(r.remove_range(0x2_0000, 0x1_0000), 0);
        assert_eq!(gauge.now(), r.bytes());

        // A rebuild after a removal starts a new count; its number is new.
        let again = r
            .record(bottom(0x2_0000, 1), Vec::new(), Origin::Build, 0)
            .unwrap();
        assert_eq!(
            (again.generation, again.sequence, again.replaced),
            (0, 6, false)
        );

        assert!(r.remove(0x1_0000));
        assert!(!r.remove(0x1_0000));
        // To the end of memory, without wrapping.
        assert_eq!(r.remove_range(0x1_0000, u64::MAX), 3);
        assert!(r.is_empty());
        assert_eq!((r.bytes(), gauge.now(), gauge.underflows()), (0, 0, 0));
    }

    #[test]
    fn a_full_registry_refuses_and_forgets_what_was_overwritten() {
        let gauge = gauge();
        let bounds = Bounds {
            entries: 2,
            bytes: u64::MAX,
        };
        let mut r = Registry::new(bounds, gauge);
        r.record(bottom(0x1000, 1), Vec::new(), Origin::Build, 0)
            .unwrap();
        r.record(bottom(0x2000, 1), Vec::new(), Origin::Build, 0)
            .unwrap();
        assert_eq!(
            r.record(bottom(0x3000, 1), Vec::new(), Origin::Build, 0),
            Err(Full { removed: false })
        );
        assert_eq!((r.len(), r.sequence()), (2, 2));
        // A rebuild of one that is there still fits: it takes its place.
        assert!(
            r.record(bottom(0x2000, 9), Vec::new(), Origin::Build, 0)
                .unwrap()
                .replaced
        );

        // By bytes: room for a small entry and not for a large one.
        let one = r.get(0x1000).unwrap().bytes();
        let bounds = Bounds {
            entries: 100,
            bytes: 2 * one,
        };
        let mut r = Registry::new(bounds, gauge);
        r.record(bottom(0x1000, 1), Vec::new(), Origin::Build, 0)
            .unwrap();
        r.record(bottom(0x2000, 1), Vec::new(), Origin::Build, 0)
            .unwrap();
        // The larger rebuild does not fit, and the old description of that
        // address is no longer true: neither stays.
        assert_eq!(
            r.record(bottom(0x2000, 50), Vec::new(), Origin::Build, 0),
            Err(Full { removed: true })
        );
        assert!(r.get(0x2000).is_none());
        assert_eq!(r.bytes(), one);
        assert!(r.bytes() <= bounds.bytes);
        // A copy that does not fit is refused the same way.
        r.record(bottom(0x2000, 1), Vec::new(), Origin::Build, 0)
            .unwrap();
        assert_eq!(
            r.copy(0x1000, 0x4000, 0),
            Err(CopyRefusal::Full(Full { removed: false }))
        );
    }
}
