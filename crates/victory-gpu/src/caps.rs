//! What the host GPU can do, asked of the host and never assumed from its
//! name: one record ([`HostCaps`]) and what the translation needs of it
//! ([`assess`]). See docs/gpu-compatibility.md.
//!
//! The rule this module exists for: behaviour follows capabilities. The
//! vendor and device ids are in the record so a report says what it was
//! taken on; nothing may branch on them. A driver defect that has to be
//! worked around is the one exception, and it is named as such where it is
//! handled, with the driver versions it applies to.
//!
//! The record is plain data and [`assess`] plain logic, tested on any host.
//! `host::Host::caps` fills it from D3D12 and DXGI.

/// What the translated shaders ask of a view heap: the slots they clamp
/// their indices below (`victory_shader::emit::HEAP_VIEWS`).
pub const SHADER_HEAP_VIEWS: u32 = 1 << 21;

/// The wave sizes of the console's GPU: a program that depends on the lane
/// layout declares the one it was compiled for.
pub const CONSOLE_WAVES: [u32; 2] = [32, 64];

/// The host adapter and device, as they describe themselves.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostCaps {
    /// The adapter's own description, for reports.
    pub name: String,
    /// PCI vendor and device ids, for reports only (see the module text).
    pub vendor: u32,
    pub device: u32,
    /// The user-mode driver's version, four 16-bit parts, if the adapter
    /// gave one.
    pub driver: Option<[u16; 4]>,
    /// A software adapter (WARP).
    pub software: bool,
    /// Memory on the card, and system memory the adapter may use, in MB.
    pub dedicated_mb: u64,
    pub shared_mb: u64,
    /// The video memory the system budgets this process on the card now, in
    /// MB: it moves with what else runs, so it is asked again where used.
    pub budget_mb: Option<u64>,
    /// Shader model 6.6 or later.
    pub shader_model_6_6: bool,
    /// Wave intrinsics, and the lane counts a wave of this device can have.
    pub wave_ops: bool,
    pub wave_lanes: (u32, u32),
    pub mesh_shaders: bool,
    /// D3D12's tiers as numbers: resource binding 1 to 3; ray tracing 0
    /// (none), 10 (1.0), 11 (1.1).
    pub binding_tier: u32,
    pub raytracing_tier: u32,
    pub depth_bounds: bool,
    /// 64-bit atomics on typed resources and through the descriptor heap.
    pub atomics64: bool,
    /// The largest shader-visible view heap the device made when asked
    /// (`gpu-info` asks; a run does not, it makes the heaps it needs).
    pub view_heap: Option<u32>,
    /// Whether the device fuses a multiply-add, measured by running one;
    /// `None` if the measurement did not run.
    pub fuses_fmad: Option<bool>,
}

/// How a need is met.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Met,
    /// The title runs, with the stated loss.
    Degraded,
    /// The title's world does not run.
    Missing,
    /// Not asked or not measured on this host.
    Unknown,
}

/// One thing the translation needs of the host, and how this host meets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub need: &'static str,
    pub state: State,
    pub detail: String,
}

/// The adapter's vendor by PCI id, for reports only.
pub fn vendor_name(vendor: u32) -> &'static str {
    match vendor {
        0x10de => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        0x1414 => "Microsoft",
        0x5143 | 0x4d4f_4351 => "Qualcomm",
        _ => "unknown vendor",
    }
}

/// Whether a wave of `lanes` lanes is one this device can run.
pub fn runs_waves_of(caps: &HostCaps, lanes: u32) -> bool {
    caps.wave_ops && (caps.wave_lanes.0..=caps.wave_lanes.1).contains(&lanes)
}

/// Every need of the translation against `caps`, in a fixed order. No
/// finding reads the vendor or the device id.
pub fn assess(caps: &HostCaps) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut find = |need, state, detail: String| findings.push(Finding { need, state, detail });
    let have = |met: bool, or: State| if met { State::Met } else { or };

    find(
        "shader model 6.6",
        have(caps.shader_model_6_6, State::Missing),
        "translated shaders are shader model 6.6 DXIL (heap-indexed views)".into(),
    );
    find(
        "mesh shaders",
        have(caps.mesh_shaders, State::Missing),
        "the world's geometry is drawn by translated mesh shaders".into(),
    );
    find(
        "resource binding tier 3",
        have(caps.binding_tier >= 3, State::Missing),
        format!("tier {}; shaders index the whole view heap", caps.binding_tier),
    );
    for lanes in CONSOLE_WAVES {
        let (min, max) = caps.wave_lanes;
        let runs = runs_waves_of(caps, lanes);
        find(
            if lanes == 32 { "32-lane waves" } else { "64-lane waves" },
            have(runs, State::Degraded),
            if runs {
                format!("host waves have {min} to {max} lanes")
            } else {
                format!(
                    "host waves have {min} to {max} lanes: a program that declares {lanes}-lane waves gets no \
                     host pipeline and its work is left out (counted in the log as \"wave sizes\"); \
                     VICTORY_WAVE_EMULATION=1 translates those whose thread group is one console wave to \
                     carry the wave themselves (experimental, off by default)"
                )
            },
        );
    }
    find(
        "view heap of 2^21 slots",
        match caps.view_heap {
            None => State::Unknown,
            Some(n) => have(n >= SHADER_HEAP_VIEWS, State::Degraded),
        },
        match caps.view_heap {
            None => "not asked on this host".into(),
            Some(n) if n >= SHADER_HEAP_VIEWS => format!("the device made a heap of {n} views"),
            Some(n) => format!(
                "the device's largest heap has {n} views: slots past it are not there, and shaders clamp \
                 their indices to 2^21, until heap indices are remapped to the host's size"
            ),
        },
    );
    find(
        "fused multiply-add",
        match caps.fuses_fmad {
            None => State::Unknown,
            Some(fused) => have(fused, State::Degraded),
        },
        match caps.fuses_fmad {
            None => "not measured on this host".into(),
            Some(true) => "the device fuses, as the console does".into(),
            Some(false) => "the device rounds the product first: the console's fused results can differ in \
                            kind here, until the lowering computes them exactly itself"
                .into(),
        },
    );
    find(
        "64-bit atomics on heap resources",
        have(caps.atomics64, State::Degraded),
        "translated 64-bit image atomics need them".into(),
    );
    find(
        "depth bounds test",
        have(caps.depth_bounds, State::Degraded),
        "pipelines that test depth bounds draw without the test where the host has none".into(),
    );
    find(
        "ray tracing tier 1.1",
        have(caps.raytracing_tier >= 11, State::Degraded),
        "for building the title's ray-tracing structures on the host (not done yet on any host)".into(),
    );
    findings
}

/// The worst state among `findings`: [`State::Missing`] before
/// [`State::Degraded`] before [`State::Unknown`] before [`State::Met`].
pub fn overall(findings: &[Finding]) -> State {
    let rank = |s: State| match s {
        State::Missing => 3,
        State::Degraded => 2,
        State::Unknown => 1,
        State::Met => 0,
    };
    findings.iter().map(|f| f.state).max_by_key(|&s| rank(s)).unwrap_or(State::Met)
}

/// The record and its findings as text: what `victory gpu-info` prints and
/// a run logs, and what a hardware report in docs/gpu-compatibility.md is
/// taken from. Nothing in it comes from a title.
pub fn report(caps: &HostCaps) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let driver = caps.driver.map_or("unknown".to_string(), |[a, b, c, d]| format!("{a}.{b}.{c}.{d}"));
    let _ = writeln!(
        out,
        "adapter: {} ({} {:#06x}:{:#06x}{}), driver {driver}",
        caps.name,
        vendor_name(caps.vendor),
        caps.vendor,
        caps.device,
        if caps.software { ", software" } else { "" },
    );
    let budget = caps.budget_mb.map_or("unknown".to_string(), |mb| format!("{mb} MB"));
    let _ = writeln!(
        out,
        "memory: {} MB on the card, {} MB shared, budget now {budget}",
        caps.dedicated_mb, caps.shared_mb
    );
    for f in assess(caps) {
        let state = match f.state {
            State::Met => "met",
            State::Degraded => "DEGRADED",
            State::Missing => "MISSING",
            State::Unknown => "unknown",
        };
        let _ = writeln!(out, "{state:>8}  {}: {}", f.need, f.detail);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device that meets every need, described by capabilities alone.
    fn full() -> HostCaps {
        HostCaps {
            name: "a device".into(),
            shader_model_6_6: true,
            wave_ops: true,
            wave_lanes: (32, 64),
            mesh_shaders: true,
            binding_tier: 3,
            raytracing_tier: 11,
            depth_bounds: true,
            atomics64: true,
            view_heap: Some(SHADER_HEAP_VIEWS + (1 << 18)),
            fuses_fmad: Some(true),
            ..Default::default()
        }
    }

    fn state(caps: &HostCaps, need: &str) -> State {
        assess(caps).into_iter().find(|f| f.need == need).unwrap_or_else(|| panic!("{need}")).state
    }

    #[test]
    fn a_device_with_every_capability_meets_every_need() {
        let findings = assess(&full());
        assert!(findings.iter().all(|f| f.state == State::Met), "{findings:?}");
        assert_eq!(overall(&findings), State::Met);
    }

    /// The rule of the module: the same capabilities give the same findings
    /// under any vendor, device or driver.
    #[test]
    fn findings_do_not_depend_on_who_made_the_device() {
        let base = full();
        for (vendor, device) in [(0x10de, 0x2c05), (0x1002, 0x73df), (0x8086, 0x56a0), (0x1414, 0x8c), (0, 0)] {
            let other = HostCaps {
                vendor,
                device,
                name: format!("{vendor:#x}"),
                driver: Some([32, 0, 15, 9999]),
                ..base.clone()
            };
            assert_eq!(assess(&other), assess(&base), "{vendor:#x}");
        }
        let lesser = HostCaps { wave_lanes: (32, 32), fuses_fmad: Some(false), ..base.clone() };
        let named = HostCaps { vendor: 0x10de, ..lesser.clone() };
        assert_eq!(assess(&named), assess(&lesser));
    }

    #[test]
    fn a_wave_size_outside_the_hosts_range_is_a_stated_loss_not_a_refusal() {
        let narrow = HostCaps { wave_lanes: (32, 32), ..full() };
        assert!(runs_waves_of(&narrow, 32) && !runs_waves_of(&narrow, 64));
        assert_eq!(state(&narrow, "32-lane waves"), State::Met);
        assert_eq!(state(&narrow, "64-lane waves"), State::Degraded);
        assert_eq!(overall(&assess(&narrow)), State::Degraded);

        let wide = HostCaps { wave_lanes: (4, 128), ..full() };
        assert!(runs_waves_of(&wide, 32) && runs_waves_of(&wide, 64));

        let none = HostCaps { wave_ops: false, ..full() };
        assert!(!runs_waves_of(&none, 32), "a lane range means nothing without wave intrinsics");
        assert_eq!(state(&none, "32-lane waves"), State::Degraded);
    }

    #[test]
    fn what_the_world_cannot_do_without_is_missing() {
        for (need, caps) in [
            ("shader model 6.6", HostCaps { shader_model_6_6: false, ..full() }),
            ("mesh shaders", HostCaps { mesh_shaders: false, ..full() }),
            ("resource binding tier 3", HostCaps { binding_tier: 2, ..full() }),
        ] {
            assert_eq!(state(&caps, need), State::Missing, "{need}");
            assert_eq!(overall(&assess(&caps)), State::Missing, "{need}");
        }
    }

    #[test]
    fn what_was_not_asked_is_unknown_and_never_reported_as_met() {
        let unasked = HostCaps { view_heap: None, fuses_fmad: None, ..full() };
        assert_eq!(state(&unasked, "view heap of 2^21 slots"), State::Unknown);
        assert_eq!(state(&unasked, "fused multiply-add"), State::Unknown);
        assert_eq!(overall(&assess(&unasked)), State::Unknown);
        // A known loss outranks an unknown.
        let worse = HostCaps { depth_bounds: false, ..unasked };
        assert_eq!(overall(&assess(&worse)), State::Degraded);
    }

    #[test]
    fn a_capped_view_heap_and_an_unfused_multiply_add_are_losses() {
        let capped = HostCaps { view_heap: Some(1_000_000), ..full() };
        assert_eq!(state(&capped, "view heap of 2^21 slots"), State::Degraded);
        let exact = HostCaps { view_heap: Some(SHADER_HEAP_VIEWS), ..full() };
        assert_eq!(state(&exact, "view heap of 2^21 slots"), State::Met);
        let unfused = HostCaps { fuses_fmad: Some(false), ..full() };
        assert_eq!(state(&unfused, "fused multiply-add"), State::Degraded);
    }

    #[test]
    fn the_report_names_the_adapter_and_every_finding() {
        let caps = HostCaps {
            name: "Some Adapter".into(),
            vendor: 0x10de,
            device: 0x1234,
            driver: Some([32, 0, 15, 7000]),
            dedicated_mb: 16_000,
            shared_mb: 8_000,
            budget_mb: Some(15_200),
            wave_lanes: (32, 32),
            ..full()
        };
        let text = report(&caps);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "adapter: Some Adapter (NVIDIA 0x10de:0x1234), driver 32.0.15.7000");
        assert_eq!(lines[1], "memory: 16000 MB on the card, 8000 MB shared, budget now 15200 MB");
        assert_eq!(lines.len(), 2 + assess(&caps).len());
        assert!(lines.iter().any(|l| l.starts_with("DEGRADED  64-lane waves: host waves have 32 to 32 lanes")));
        assert!(lines.iter().any(|l| l.starts_with("     met  mesh shaders")));
        let unknown = report(&HostCaps::default());
        assert!(unknown.starts_with("adapter:  (unknown vendor 0x0000:0x0000), driver unknown"), "{unknown}");
    }

    #[test]
    fn the_heap_size_shaders_assume_is_the_translators() {
        assert_eq!(SHADER_HEAP_VIEWS, victory_shader::emit::HEAP_VIEWS);
    }
}
