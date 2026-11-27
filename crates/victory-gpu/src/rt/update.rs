//! Whether a structure that changed is refitted in place or built again.
//!
//! A refit ("perform update") is cheap and keeps the structure's tree, so
//! it is only correct while the tree still fits. D3D12 allows it when the
//! source was built to allow updates and nothing differs but the vertex
//! positions, the geometry transforms or the boxes of a bottom level, or
//! the instance descriptions of a top level (their number fixed).
//! Everything else is a rebuild. The rules here are those, and stricter in
//! places, since a wrong refit is a wrong picture and a needless rebuild
//! only costs time:
//!
//! * the shape must be equal ([`super::desc::Shape`]): counts, formats,
//!   strides, flags, whether a transform is given, and the index memory's
//!   address (D3D12 would take other memory with the same contents; other
//!   memory is taken here as a sign the contents may differ);
//! * the caller must say that only positions changed ([`Change::Positions`]).
//!   What a description cannot show is the caller's to know: index contents,
//!   and whether any primitive went between active and inactive (a triangle
//!   is inactive with a NaN x in any corner, a box with a NaN least x),
//!   which a refit may not do in either direction;
//! * a compacted structure is not refitted. D3D12's text allows compaction
//!   and updates together; it is left out here until it has run on
//!   hardware;
//! * a top level whose instances refer to other bottom-level addresses
//!   than when it was built is built again (`super::device`), though D3D12
//!   lets a refit redefine instances wholly: a tree fitted to other
//!   geometry is the case a rebuild exists for;
//! * after [`Policy::rebuild_after_refits`] refits in a row the next change
//!   is a rebuild, since each refit can loosen the tree. The number is a
//!   policy, not a measured optimum: nothing has been measured. Being a
//!   policy and no rule of D3D12's, it gives way where it cannot be kept: a
//!   rebuild goes into new memory beside the old, and where the bound on
//!   structure memory has no room for that the structure is refitted once
//!   more and the refit counted ([`Plan::past_refit_limit`]), since a
//!   refit is still a correct structure and a refusal is none;
//! * a structure refitted since its last build is not compacted
//!   ([`compactable`]): the size it compacts to was reported by that build,
//!   and nothing read here says a refit leaves it unchanged.

use super::budget::{self, Over};
use super::desc::Shape;
use super::scratch::GRANULE;

/// The environment variable holding [`Policy::rebuild_after_refits`].
pub const REFITS_VARIABLE: &str = "VICTORY_RT_REBUILD_AFTER_REFITS";

/// The refits allowed in a row where the environment names none. A
/// placeholder to be replaced by a measurement.
pub const DEFAULT_REBUILD_AFTER_REFITS: u32 = 16;

/// How structures that change are kept up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// After this many refits in a row a structure is built again (or
    /// refitted once more where the memory bound has no room for the
    /// rebuild). 0: never refit, whatever the bound.
    pub rebuild_after_refits: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { rebuild_after_refits: DEFAULT_REBUILD_AFTER_REFITS }
    }
}

impl Policy {
    /// The policy a value of [`REFITS_VARIABLE`] gives: a whole number;
    /// unset or empty is the default. Anything else is an error holding it
    /// (the default is then used).
    pub fn parse(value: Option<&str>) -> Result<Policy, String> {
        match value.map(str::trim) {
            None | Some("") => Ok(Policy::default()),
            Some(text) => text
                .parse()
                .map(|rebuild_after_refits| Policy { rebuild_after_refits })
                .map_err(|_| text.to_owned()),
        }
    }

    /// The policy the environment gives; a value that is no number is
    /// logged and the default used.
    pub fn from_env() -> Policy {
        let value = std::env::var_os(REFITS_VARIABLE).map(|v| v.to_string_lossy().into_owned());
        Policy::parse(value.as_deref()).unwrap_or_else(|unknown| {
            tracing::warn!(
                "{REFITS_VARIABLE}={unknown} is not a number: {DEFAULT_REBUILD_AFTER_REFITS} refits in a row are \
                 allowed"
            );
            Policy::default()
        })
    }
}

/// What the caller says changed in a structure's source since it was built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Only what a refit may follow: vertex positions, boxes, geometry
    /// transforms; for a top level, the instances' transforms, ids, masks
    /// and flags. No primitive went between active and inactive.
    Positions,
    /// Anything else, or not known.
    Other,
}

/// What is known of a structure as it was last built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub shape: Shape,
    /// Refits since the last build.
    pub refits: u32,
    /// It is a compacted copy.
    pub compacted: bool,
    /// The bytes of its result memory, and of the scratch a refit needs.
    pub result_bytes: u64,
    pub update_scratch_bytes: u64,
}

/// Why a change is a rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Nothing was built yet (or its build was abandoned).
    NotBuilt,
    /// It was built without allowing updates.
    NotUpdatable,
    Compacted,
    ShapeChanged,
    /// The caller does not say only positions changed.
    ContentsChanged,
    /// [`Policy::rebuild_after_refits`] refits in a row.
    RefitLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Refit,
    Rebuild(Why),
}

/// How a structure last built as `built` comes to hold a source of shape
/// `wanted` that changed by `change`.
pub fn decide(built: Option<&Built>, wanted: &Shape, change: Change, policy: &Policy) -> Decision {
    let Some(built) = built else { return Decision::Rebuild(Why::NotBuilt) };
    let why = if !built.shape.flags().allow_update {
        Why::NotUpdatable
    } else if built.compacted {
        Why::Compacted
    } else if built.shape != *wanted {
        Why::ShapeChanged
    } else if change != Change::Positions {
        Why::ContentsChanged
    } else if built.refits >= policy.rebuild_after_refits {
        Why::RefitLimit
    } else {
        return Decision::Refit;
    };
    Decision::Rebuild(why)
}

/// What the device says a build of some description needs, in bytes (its
/// prebuild information): the result, the scratch of a build, the scratch
/// of a later refit (0 where the build does not allow updates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sizes {
    pub result: u64,
    pub scratch: u64,
    pub update_scratch: u64,
}

/// How one build is carried out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub decision: Decision,
    /// New result memory to make: the bytes to ask for, and those bytes in
    /// the whole 64 KiB units a buffer takes, which is what is counted.
    /// `None`: a refit, in the memory the structure has.
    pub memory: Option<(u64, u64)>,
    pub scratch_bytes: u64,
    /// What will be known of the structure once this build is recorded.
    pub built: Built,
    /// A refit made although the policy's refits in a row are used up,
    /// because the bound on structure memory has no room for the rebuild.
    pub past_refit_limit: bool,
}

/// Why a build is not carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unplanned {
    /// The device sizes the result at 0 bytes: it does not take the
    /// description.
    NoSize,
    /// The new memory would pass the bound on structure memory.
    Budget(Over),
}

/// Plans the build that makes a structure last built as `built`, in result
/// memory of `memory` bytes, hold a source of shape `wanted` that changed by
/// `change`, the device sizing it at `sizes`. `held` and `limit` are the
/// structure memory held now and its bound.
///
/// A refit is planned where [`decide`] allows one and the memory the
/// structure has still holds what the device asks for (else it is a
/// rebuild: a refit cannot grow). A rebuild goes into new memory, so both
/// are held until the old one's last reader is done: the new one must fit
/// the bound beside everything held. Where it does not, and the rebuild was
/// asked for by the policy's count of refits alone, the structure is
/// refitted once more ([`Plan::past_refit_limit`]); a policy of no refits
/// at all is kept, and the build refused.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    built: Option<&Built>,
    memory: Option<u64>,
    wanted: Shape,
    change: Change,
    policy: &Policy,
    sizes: Sizes,
    held: u64,
    limit: u64,
) -> Result<Plan, Unplanned> {
    if sizes.result == 0 {
        return Err(Unplanned::NoSize);
    }
    let mut decision = decide(built, &wanted, change, policy);
    let fits = memory.is_some_and(|bytes| bytes >= sizes.result);
    if decision == Decision::Refit && !fits {
        decision = Decision::Rebuild(Why::ShapeChanged);
    }
    let counted = sizes.result.checked_next_multiple_of(GRANULE).unwrap_or(u64::MAX);
    let room = budget::admit(held, counted, limit);
    let past_refit_limit =
        decision == Decision::Rebuild(Why::RefitLimit) && room.is_err() && fits && policy.rebuild_after_refits > 0;
    if past_refit_limit {
        decision = Decision::Refit;
    }
    match (decision, built) {
        (Decision::Refit, Some(before)) => Ok(Plan {
            decision,
            memory: None,
            // What the build it updates was sized with, or what the device
            // says now if that is more.
            scratch_bytes: sizes.update_scratch.max(before.update_scratch_bytes),
            built: Built { shape: wanted, refits: before.refits.saturating_add(1), compacted: false, ..*before },
            past_refit_limit,
        }),
        _ => {
            room.map_err(Unplanned::Budget)?;
            Ok(Plan {
                decision,
                memory: Some((sizes.result, counted)),
                scratch_bytes: sizes.scratch,
                built: Built {
                    shape: wanted,
                    refits: 0,
                    compacted: false,
                    result_bytes: counted,
                    update_scratch_bytes: sizes.update_scratch,
                },
                past_refit_limit: false,
            })
        }
    }
}

/// Why a structure is not copied into less memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uncompacted {
    /// Not built to allow compaction, or a compacted copy already.
    NotAsked,
    /// Refitted since the build that reported the size it compacts to.
    /// D3D12 says a refit leaves the size of the structure itself as it
    /// was; of the compacted size it says nothing, and a copy into too
    /// little memory writes past it. Its next rebuild reports a size again.
    Refitted,
}

/// Whether a structure last built as `built` may be copied into the size
/// that build reported.
pub fn compactable(built: &Built) -> Result<(), Uncompacted> {
    if !built.shape.flags().allow_compaction || built.compacted {
        Err(Uncompacted::NotAsked)
    } else if built.refits > 0 {
        Err(Uncompacted::Refitted)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt::desc::{BuildFlags, GeometryFlags, GeometryShape, VertexFormat};

    fn shape(vertices: u32, allow_update: bool) -> Shape {
        Shape::Bottom {
            flags: BuildFlags { allow_update, ..Default::default() },
            geometries: vec![GeometryShape::Triangles {
                format: VertexFormat::Float3,
                vertices,
                stride: 12,
                indices: None,
                transformed: false,
                flags: GeometryFlags::default(),
            }],
        }
    }

    fn built(refits: u32) -> Built {
        Built { shape: shape(30, true), refits, compacted: false, result_bytes: 4096, update_scratch_bytes: 512 }
    }

    const POLICY: Policy = Policy { rebuild_after_refits: 3 };

    #[test]
    fn positions_alone_in_an_updatable_structure_are_a_refit() {
        assert_eq!(decide(Some(&built(0)), &shape(30, true), Change::Positions, &POLICY), Decision::Refit);
        assert_eq!(decide(Some(&built(2)), &shape(30, true), Change::Positions, &POLICY), Decision::Refit);
    }

    #[test]
    fn everything_else_is_a_rebuild_with_its_reason() {
        let wanted = shape(30, true);
        let rebuild = |built: Option<&Built>, wanted: &Shape, change| match decide(built, wanted, change, &POLICY) {
            Decision::Rebuild(why) => why,
            Decision::Refit => panic!("a refit"),
        };
        assert_eq!(rebuild(None, &wanted, Change::Positions), Why::NotBuilt);
        let fixed = Built { shape: shape(30, false), ..built(0) };
        assert_eq!(rebuild(Some(&fixed), &shape(30, false), Change::Positions), Why::NotUpdatable);
        assert_eq!(rebuild(Some(&Built { compacted: true, ..built(0) }), &wanted, Change::Positions), Why::Compacted);
        assert_eq!(rebuild(Some(&built(0)), &shape(33, true), Change::Positions), Why::ShapeChanged);
        assert_eq!(
            rebuild(Some(&built(0)), &shape(30, false), Change::Positions),
            Why::ShapeChanged,
            "a refit keeps the build's flags"
        );
        assert_eq!(rebuild(Some(&built(0)), &wanted, Change::Other), Why::ContentsChanged);
        assert_eq!(rebuild(Some(&built(3)), &wanted, Change::Positions), Why::RefitLimit);
        assert_eq!(rebuild(Some(&built(9)), &wanted, Change::Positions), Why::RefitLimit);
    }

    /// The rule as a run sees it: N refits, then a rebuild, then N again.
    #[test]
    fn a_structure_is_built_again_after_the_refits_the_policy_allows() {
        for n in [0u32, 1, 3, 16] {
            let policy = Policy { rebuild_after_refits: n };
            let mut state = built(0);
            let mut kinds = Vec::new();
            for _ in 0..2 * (n + 1) {
                match decide(Some(&state), &shape(30, true), Change::Positions, &policy) {
                    Decision::Refit => {
                        state.refits += 1;
                        kinds.push('r');
                    }
                    Decision::Rebuild(why) => {
                        assert_eq!(why, Why::RefitLimit);
                        state.refits = 0;
                        kinds.push('B');
                    }
                }
            }
            let period: String = "r".repeat(n as usize) + "B";
            assert_eq!(kinds.iter().collect::<String>(), period.repeat(2), "{n} refits allowed");
        }
    }

    const SIZES: Sizes = Sizes { result: 100_000, scratch: 30_000, update_scratch: 5_000 };
    const MIB: u64 = 1 << 20;

    #[test]
    fn a_first_build_goes_into_new_memory_counted_in_whole_units() {
        let plan = plan(None, None, shape(30, true), Change::Other, &POLICY, SIZES, 0, MIB).unwrap();
        assert_eq!(plan.decision, Decision::Rebuild(Why::NotBuilt));
        assert_eq!(plan.memory, Some((100_000, 2 * GRANULE)), "asked as sized, counted as the buffer takes");
        assert_eq!(plan.scratch_bytes, 30_000);
        assert_eq!(
            plan.built,
            Built {
                shape: shape(30, true),
                refits: 0,
                compacted: false,
                result_bytes: 2 * GRANULE,
                update_scratch_bytes: 5_000
            }
        );
    }

    #[test]
    fn a_refit_keeps_the_memory_and_counts_itself() {
        let before = Built { result_bytes: 2 * GRANULE, update_scratch_bytes: 6_000, ..built(1) };
        let plan =
            plan(Some(&before), Some(100_000), shape(30, true), Change::Positions, &POLICY, SIZES, MIB, MIB).unwrap();
        assert_eq!((plan.decision, plan.memory), (Decision::Refit, None), "no new memory: the bound is not asked");
        assert_eq!(plan.scratch_bytes, 6_000, "the larger of what the build and the device say");
        assert_eq!(plan.built, Built { refits: 2, ..before.clone() });
        let more = Sizes { update_scratch: 9_000, ..SIZES };
        let wanted = shape(30, true);
        let plan = super::plan(Some(&before), Some(100_000), wanted, Change::Positions, &POLICY, more, 0, MIB).unwrap();
        assert_eq!(plan.scratch_bytes, 9_000);
    }

    /// A refit cannot grow: where the device now asks for more than the
    /// structure's memory holds, or there is no memory, it is a rebuild.
    #[test]
    fn a_refit_that_would_not_fit_its_memory_is_a_rebuild() {
        let before = built(0);
        for memory in [Some(99_999), None] {
            let plan = plan(Some(&before), memory, shape(30, true), Change::Positions, &POLICY, SIZES, 0, MIB).unwrap();
            assert_eq!(plan.decision, Decision::Rebuild(Why::ShapeChanged), "{memory:?}");
            assert_eq!(plan.memory, Some((100_000, 2 * GRANULE)));
            assert_eq!(plan.built.refits, 0);
        }
    }

    #[test]
    fn a_rebuild_resets_the_refits_and_is_no_longer_compacted() {
        let before = Built { compacted: true, ..built(3) };
        let plan = plan(Some(&before), Some(MIB), shape(30, true), Change::Positions, &POLICY, SIZES, 0, MIB).unwrap();
        assert_eq!(plan.decision, Decision::Rebuild(Why::Compacted));
        assert_eq!((plan.built.refits, plan.built.compacted, plan.scratch_bytes), (0, false, 30_000));
    }

    /// The new memory is held beside the old until the old one's readers
    /// are done, so it must fit beside everything held.
    #[test]
    fn new_memory_over_the_bound_is_refused_and_what_the_device_does_not_size_too() {
        let fits = plan(None, None, shape(30, true), Change::Other, &POLICY, SIZES, MIB - 2 * GRANULE, MIB);
        assert!(fits.is_ok());
        let over = plan(None, None, shape(30, true), Change::Other, &POLICY, SIZES, MIB - GRANULE, MIB);
        assert_eq!(over, Err(Unplanned::Budget(Over { held: MIB - GRANULE, asked: 2 * GRANULE, limit: MIB })));
        let unsized_ = Sizes { result: 0, ..SIZES };
        assert_eq!(
            plan(None, None, shape(30, true), Change::Other, &POLICY, unsized_, 0, MIB),
            Err(Unplanned::NoSize)
        );
        let huge = Sizes { result: u64::MAX - 5, ..SIZES };
        assert!(matches!(
            plan(None, None, shape(30, true), Change::Other, &POLICY, huge, 0, u64::MAX - 1),
            Err(Unplanned::Budget(_))
        ));
    }

    /// The count of refits is a policy: where the bound has no room for the
    /// rebuild it asks for, a change a refit can follow is refitted and
    /// said to be, not refused. Every other reason for a rebuild is a rule,
    /// and is refused there.
    #[test]
    fn the_refit_limit_gives_way_where_the_bound_has_no_room_for_a_rebuild() {
        let policy = Policy { rebuild_after_refits: 2 };
        let before = |refits| Built { result_bytes: 2 * GRANULE, update_scratch_bytes: 5_000, ..built(refits) };
        let plan = |before: &Built, wanted: Shape, change, policy: &Policy, held| {
            super::plan(Some(before), Some(100_000), wanted, change, policy, SIZES, held, MIB)
        };
        // Under the limit: a refit, as ever.
        let one = plan(&before(1), shape(30, true), Change::Positions, &policy, MIB).unwrap();
        assert_eq!((one.decision, one.past_refit_limit), (Decision::Refit, false));
        // At the limit with room: the rebuild the policy asks for.
        let room = plan(&before(2), shape(30, true), Change::Positions, &policy, MIB - 2 * GRANULE).unwrap();
        assert_eq!((room.decision, room.past_refit_limit), (Decision::Rebuild(Why::RefitLimit), false));
        assert_eq!((room.memory, room.built.refits), (Some((100_000, 2 * GRANULE)), 0));
        // At the limit with the bound reached: one more refit, said to be.
        let full = plan(&before(2), shape(30, true), Change::Positions, &policy, MIB).unwrap();
        assert_eq!((full.decision, full.past_refit_limit, full.memory), (Decision::Refit, true, None));
        assert_eq!(full.built, Built { refits: 3, ..before(2) }, "still counted: the next change tries again");
        assert_eq!(full.scratch_bytes, 5_000);
        let again = plan(&full.built, shape(30, true), Change::Positions, &policy, MIB - 2 * GRANULE).unwrap();
        assert_eq!(again.decision, Decision::Rebuild(Why::RefitLimit), "and is a rebuild once there is room");
        // No refits at all is not given up for want of room.
        let never = Policy { rebuild_after_refits: 0 };
        let over = Over { held: MIB, asked: 2 * GRANULE, limit: MIB };
        assert_eq!(plan(&before(0), shape(30, true), Change::Positions, &never, MIB), Err(Unplanned::Budget(over)));
        // Nor is a rebuild a refit could not replace: other contents,
        // another shape, a structure that takes no update, memory too small.
        assert_eq!(plan(&before(2), shape(30, true), Change::Other, &policy, MIB), Err(Unplanned::Budget(over)));
        assert_eq!(plan(&before(2), shape(33, true), Change::Positions, &policy, MIB), Err(Unplanned::Budget(over)));
        let fixed = Built { shape: shape(30, false), ..before(2) };
        assert_eq!(plan(&fixed, shape(30, false), Change::Positions, &policy, MIB), Err(Unplanned::Budget(over)));
        let small = super::plan(
            Some(&before(2)),
            Some(99_999),
            shape(30, true),
            Change::Positions,
            &policy,
            SIZES,
            MIB,
            MIB,
        );
        assert_eq!(small, Err(Unplanned::Budget(over)), "a refit cannot grow");
    }

    /// The size a structure compacts to is its build's: after a refit it is
    /// not known, and the structure is not copied into it.
    #[test]
    fn a_structure_is_compacted_only_as_its_build_left_it() {
        let flags = BuildFlags { allow_update: true, allow_compaction: true, ..Default::default() };
        let shape = Shape::Top { flags, instances: 4 };
        let as_built = Built { shape: shape.clone(), ..built(0) };
        assert_eq!(compactable(&as_built), Ok(()));
        assert_eq!(compactable(&Built { refits: 1, ..as_built.clone() }), Err(Uncompacted::Refitted));
        assert_eq!(compactable(&Built { compacted: true, ..as_built.clone() }), Err(Uncompacted::NotAsked));
        assert_eq!(compactable(&built(0)), Err(Uncompacted::NotAsked), "not built to allow it");
        assert_eq!(compactable(&built(1)), Err(Uncompacted::NotAsked));
        // A rebuild reports a size again.
        let refitted = Built { refits: 3, result_bytes: 2 * GRANULE, ..as_built };
        let rebuilt = plan(Some(&refitted), Some(MIB), shape, Change::Other, &POLICY, SIZES, 0, MIB).unwrap();
        assert_eq!(compactable(&rebuilt.built), Ok(()));
    }

    #[test]
    fn the_number_of_refits_is_read_from_its_variable() {
        assert_eq!(Policy::parse(None), Ok(Policy::default()));
        assert_eq!(Policy::default().rebuild_after_refits, DEFAULT_REBUILD_AFTER_REFITS);
        assert_eq!(Policy::parse(Some("")), Ok(Policy::default()));
        assert_eq!(Policy::parse(Some(" 40 ")), Ok(Policy { rebuild_after_refits: 40 }));
        assert_eq!(Policy::parse(Some("0")), Ok(Policy { rebuild_after_refits: 0 }), "never refit");
        for bad in ["-1", "many", "3.5", "4294967296"] {
            assert_eq!(Policy::parse(Some(bad)), Err(bad.to_owned()), "{bad}");
        }
    }
}
