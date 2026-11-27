//! How much memory the backend may hold: bounds for the structures and for
//! its two pools, from what the device reports and what the user asks, as
//! plain arithmetic tested on any host.
//!
//! Every number here is a policy until a run on hardware has shown what a
//! title's structures take: none is measured. A build that would pass a
//! bound is refused and counted, and its case stays with the software
//! backend; nothing is evicted to make room, since a structure in use
//! cannot be given up and one not in use is the caller's to give up.

use super::scratch;

/// The environment variable holding the bound on structure memory, in MiB.
pub const STRUCTURES_VARIABLE: &str = "VICTORY_RT_STRUCTURES_MB";

const MIB: u64 = 1 << 20;

/// The share of the device's video memory budget structures may take where
/// the user names no bound: a quarter, within these limits.
const BUDGET_SHARE: u64 = 4;
const LEAST: u64 = 64 * MIB;
const MOST: u64 = 2048 * MIB;
/// The bound where the device reports no budget.
const UNKNOWN_BUDGET: u64 = 256 * MIB;

/// The backend's bounds, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Result memory of structures: live ones, and ones given up that wait
    /// for their last reader.
    pub structures: u64,
    /// Build scratch.
    pub scratch: scratch::Limits,
    /// Instance descriptions of top-level builds.
    pub instances: scratch::Limits,
}

impl Limits {
    /// The bounds for a device that reports a video memory budget of
    /// `budget_mb` MB now, with `asked` the value of
    /// [`STRUCTURES_VARIABLE`]: a whole number of MiB bounds the structures
    /// at that; unset or empty leaves it to the device's budget. Any other
    /// value is handed back beside the bounds, for the caller to report.
    pub fn for_device(budget_mb: Option<u64>, asked: Option<&str>) -> (Limits, Option<String>) {
        let (asked_mb, unknown) = match asked.map(str::trim) {
            None | Some("") => (None, None),
            Some(text) => match text.parse::<u64>() {
                Ok(mb) => (Some(mb), None),
                Err(_) => (None, Some(text.to_owned())),
            },
        };
        let structures = match asked_mb {
            Some(mb) => mb.saturating_mul(MIB),
            None => match budget_mb {
                Some(mb) => (mb.saturating_mul(MIB) / BUDGET_SHARE).clamp(LEAST, MOST),
                None => UNKNOWN_BUDGET,
            },
        };
        let limits = Limits {
            structures,
            scratch: scratch::Limits { align: 256, chunk: 4 * MIB, keep: 32 * MIB, live: 256 * MIB },
            instances: scratch::Limits { align: 16, chunk: MIB, keep: 4 * MIB, live: 64 * MIB },
        };
        (limits, unknown)
    }

    /// [`Limits::for_device`] with what the environment asks; a value that
    /// is no number is logged and left out.
    pub fn from_env(budget_mb: Option<u64>) -> Limits {
        let asked = std::env::var_os(STRUCTURES_VARIABLE).map(|v| v.to_string_lossy().into_owned());
        let (limits, unknown) = Limits::for_device(budget_mb, asked.as_deref());
        if let Some(unknown) = unknown {
            tracing::warn!(
                "{STRUCTURES_VARIABLE}={unknown} is not a number of MiB: ray-tracing structures are bounded at {} MiB",
                limits.structures / MIB
            );
        }
        limits
    }
}

/// A request for structure memory that would pass the bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Over {
    pub held: u64,
    pub asked: u64,
    pub limit: u64,
}

impl std::fmt::Display for Over {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} bytes asked with {} held, over the {} bytes structures may take ({STRUCTURES_VARIABLE})",
            self.asked, self.held, self.limit
        )
    }
}

/// Whether `asked` more bytes of structure memory fit beside the `held`
/// ones under `limit`.
pub fn admit(held: u64, asked: u64, limit: u64) -> Result<(), Over> {
    match held.checked_add(asked) {
        Some(total) if total <= limit => Ok(()),
        _ => Err(Over { held, asked, limit }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bound_follows_the_devices_budget_within_limits() {
        let bound = |budget_mb| Limits::for_device(budget_mb, None).0.structures;
        assert_eq!(bound(Some(8_000)), 2_000 * MIB, "a quarter");
        assert_eq!(bound(Some(15_200)), MOST);
        assert_eq!(bound(Some(100)), LEAST);
        assert_eq!(bound(Some(0)), LEAST);
        assert_eq!(bound(None), UNKNOWN_BUDGET);
        assert_eq!(bound(Some(u64::MAX)), MOST, "no overflow");
    }

    #[test]
    fn a_bound_the_user_names_is_the_bound() {
        let bound = |asked| Limits::for_device(Some(8_000), Some(asked));
        assert_eq!(bound("300").0.structures, 300 * MIB);
        assert_eq!(bound(" 4096 ").0.structures, 4096 * MIB, "above what the budget would give, if asked");
        assert_eq!(bound("0").0.structures, 0, "no structure at all");
        assert_eq!(bound(""), (Limits::for_device(Some(8_000), None).0, None));
        for bad in ["lots", "-5", "1.5"] {
            let (limits, unknown) = bound(bad);
            assert_eq!(unknown.as_deref(), Some(bad));
            assert_eq!(limits.structures, 2_000 * MIB, "the device's budget decides then");
        }
    }

    #[test]
    fn the_pools_are_bounded_and_aligned_as_d3d12_asks() {
        let (limits, _) = Limits::for_device(None, None);
        assert_eq!(limits.scratch.align, crate::rt::desc::STRUCTURE_ALIGN);
        assert_eq!(limits.instances.align, crate::rt::desc::INSTANCE_ALIGN);
        for pool in [limits.scratch, limits.instances] {
            assert!(pool.chunk <= pool.keep && pool.keep <= pool.live, "{pool:?}");
        }
    }

    #[test]
    fn memory_is_admitted_up_to_the_bound_and_not_past_it() {
        assert_eq!(admit(0, 100, 100), Ok(()));
        assert_eq!(admit(60, 40, 100), Ok(()));
        assert_eq!(admit(60, 41, 100), Err(Over { held: 60, asked: 41, limit: 100 }));
        assert_eq!(admit(0, 1, 0), Err(Over { held: 0, asked: 1, limit: 0 }));
        assert!(admit(u64::MAX, 1, u64::MAX).is_err(), "no overflow");
    }
}
