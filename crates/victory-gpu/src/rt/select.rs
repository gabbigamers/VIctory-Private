//! Which ray-tracing backend a run uses: the software traversal the
//! translated programs carry, or the experimental hardware one. Decided
//! from what was asked for (`VICTORY_RT_BACKEND`) and what the device
//! reports ([`crate::caps::HostCaps`]), never from its vendor or model.
//!
//! Software is the default until the hardware backend has been validated on
//! hardware: it is chosen only when asked for by name or by `auto`. Asking
//! for hardware on a device that cannot run it is reported and gives
//! software, so a run never loses its ray tracing to this switch.

use crate::caps::HostCaps;

/// The environment variable that asks for a backend.
pub const VARIABLE: &str = "VICTORY_RT_BACKEND";

/// A ray-tracing backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The translated programs walk the title's own structures, node by
    /// node (`victory_shader`'s emulation of the node instruction). The
    /// reference, and what every case the hardware backend refuses uses.
    Software,
    /// Host acceleration structures traversed by inline `RayQuery` (DXR
    /// 1.1) in translated programs. Experimental.
    HardwareInline,
}

/// What was asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Request {
    /// The software backend: also what an unset variable means.
    #[default]
    Software,
    /// The hardware backend, if the device can run it.
    Hardware,
    /// Whatever the device's capabilities allow: hardware where it can.
    Auto,
}

impl Request {
    /// The request a value of [`VARIABLE`] makes: `software`, `hardware` or
    /// `auto`, in any case, spaces around it ignored; unset or empty is
    /// software. Any other value is an error holding it, for the caller to
    /// report (the run then uses software).
    pub fn parse(value: Option<&str>) -> Result<Request, String> {
        let Some(value) = value else { return Ok(Request::Software) };
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "software" => Ok(Request::Software),
            "hardware" => Ok(Request::Hardware),
            "auto" => Ok(Request::Auto),
            _ => Err(value.to_owned()),
        }
    }

    /// The request the environment makes, and the value it holds if that is
    /// not one of the three (software then).
    pub fn from_env() -> (Request, Option<String>) {
        let value = std::env::var_os(VARIABLE).map(|v| v.to_string_lossy().into_owned());
        match Request::parse(value.as_deref()) {
            Ok(request) => (request, None),
            Err(unknown) => (Request::Software, Some(unknown)),
        }
    }
}

/// One thing the hardware backend needs that a device does not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lack {
    /// Ray tracing below tier 1.1 (the tier found, as [`HostCaps`] numbers
    /// it): inline `RayQuery` is a 1.1 feature.
    Tier(u32),
    /// No shader model 6.6: translated programs are 6.6 DXIL, and
    /// `RayQuery` needs 6.5.
    ShaderModel,
    /// The device does not answer as `ID3D12Device5`, the interface
    /// structures are sized through.
    Device,
}

impl std::fmt::Display for Lack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Lack::Tier(0) => write!(f, "no ray tracing"),
            Lack::Tier(tier) => write!(f, "ray tracing tier {}.{} (1.1 needed)", tier / 10, tier % 10),
            Lack::ShaderModel => write!(f, "no shader model 6.6"),
            Lack::Device => write!(f, "no ID3D12Device5"),
        }
    }
}

/// What the hardware backend needs and `caps` does not have, in a fixed
/// order; empty if the device can run it. Reads capabilities only.
pub fn lacks(caps: &HostCaps) -> Vec<Lack> {
    let mut lacks = Vec::new();
    if caps.raytracing_tier < 11 {
        lacks.push(Lack::Tier(caps.raytracing_tier));
    }
    if !caps.shader_model_6_6 {
        lacks.push(Lack::ShaderModel);
    }
    if !caps.raytracing_device {
        lacks.push(Lack::Device);
    }
    lacks
}

/// The backend chosen, with what it was chosen from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub backend: Backend,
    pub request: Request,
    /// What the device lacks for the hardware backend (empty if nothing),
    /// whatever was asked for.
    pub lacks: Vec<Lack>,
}

impl Selection {
    /// Whether hardware was asked for by name and the device cannot run it.
    pub fn fell_back(&self) -> bool {
        self.request == Request::Hardware && self.backend == Backend::Software
    }

    /// One line for the log and the `gpu-info` report.
    pub fn describe(&self) -> String {
        let lacks = || self.lacks.iter().map(Lack::to_string).collect::<Vec<_>>().join(", ");
        let able = if self.lacks.is_empty() {
            "this device could run the experimental hardware backend".to_owned()
        } else {
            format!("this device cannot run the hardware backend: {}", lacks())
        };
        match (self.request, self.backend) {
            (Request::Software, _) => format!("software ({VARIABLE} is unset or software; {able})"),
            (Request::Hardware, Backend::HardwareInline) => {
                format!("hardware inline RayQuery, experimental ({VARIABLE}=hardware)")
            }
            (Request::Hardware, Backend::Software) => {
                format!("software, though {VARIABLE}=hardware asked for hardware: {able}")
            }
            (Request::Auto, Backend::HardwareInline) => {
                format!("hardware inline RayQuery, experimental ({VARIABLE}=auto)")
            }
            (Request::Auto, Backend::Software) => format!("software ({VARIABLE}=auto; {able})"),
        }
    }
}

/// The backend for a device with `caps` under `request`.
pub fn select(caps: &HostCaps, request: Request) -> Selection {
    let lacks = lacks(caps);
    let backend = match request {
        Request::Software => Backend::Software,
        Request::Hardware | Request::Auto if lacks.is_empty() => Backend::HardwareInline,
        Request::Hardware | Request::Auto => Backend::Software,
    };
    Selection { backend, request, lacks }
}

/// The backend for a device with `caps` under what the environment asks,
/// with its outcome logged once a process: a value of the variable that is
/// not understood, and hardware asked for where the device cannot run it,
/// as warnings.
pub fn selected(caps: &HostCaps) -> Selection {
    let (request, unknown) = Request::from_env();
    let selection = select(caps, request);
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(unknown) = &unknown {
            tracing::warn!(
                "{VARIABLE}={unknown} is not software, hardware or auto: the software ray-tracing backend is used"
            );
        }
        if selection.fell_back() {
            tracing::warn!("ray tracing backend: {}", selection.describe());
        } else {
            tracing::info!("ray tracing backend: {}", selection.describe());
        }
    });
    selection
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device with everything the hardware backend needs, by capability.
    fn able() -> HostCaps {
        HostCaps { raytracing_tier: 11, shader_model_6_6: true, raytracing_device: true, ..Default::default() }
    }

    #[test]
    fn software_is_what_an_unset_variable_means() {
        assert_eq!(Request::parse(None), Ok(Request::Software));
        assert_eq!(Request::parse(Some("")), Ok(Request::Software));
        assert_eq!(Request::default(), Request::Software);
        let chosen = select(&able(), Request::default());
        assert_eq!(chosen.backend, Backend::Software, "a capable device does not turn the backend on");
        assert!(!chosen.fell_back());
    }

    #[test]
    fn the_three_values_are_read_in_any_case() {
        for (text, request) in [
            ("software", Request::Software),
            ("Hardware", Request::Hardware),
            (" AUTO ", Request::Auto),
            ("hardware\n", Request::Hardware),
        ] {
            assert_eq!(Request::parse(Some(text)), Ok(request), "{text:?}");
        }
    }

    /// A value that is not one of the three is handed back, not guessed at:
    /// `1` and `on` are how the on/off switches are set, and are not a
    /// backend.
    #[test]
    fn any_other_value_is_an_error_holding_it() {
        for text in ["1", "on", "dxr", "hard ware", "software,hardware", "0"] {
            assert_eq!(Request::parse(Some(text)), Err(text.to_owned()), "{text:?}");
        }
    }

    #[test]
    fn hardware_is_chosen_only_when_asked_for_and_possible() {
        assert_eq!(select(&able(), Request::Hardware).backend, Backend::HardwareInline);
        assert_eq!(select(&able(), Request::Auto).backend, Backend::HardwareInline);
        assert_eq!(select(&able(), Request::Software).backend, Backend::Software);
        assert!(select(&able(), Request::Hardware).lacks.is_empty());
    }

    #[test]
    fn each_missing_capability_gives_software_and_is_named() {
        for (caps, lack) in [
            (HostCaps { raytracing_tier: 10, ..able() }, Lack::Tier(10)),
            (HostCaps { raytracing_tier: 0, ..able() }, Lack::Tier(0)),
            (HostCaps { shader_model_6_6: false, ..able() }, Lack::ShaderModel),
            (HostCaps { raytracing_device: false, ..able() }, Lack::Device),
        ] {
            for request in [Request::Hardware, Request::Auto] {
                let chosen = select(&caps, request);
                assert_eq!(chosen.backend, Backend::Software, "{lack:?} {request:?}");
                assert_eq!(chosen.lacks, [lack]);
                assert_eq!(chosen.fell_back(), request == Request::Hardware, "only a request by name is refused");
                assert!(chosen.describe().contains(&lack.to_string()), "{}", chosen.describe());
            }
        }
        let nothing = select(&HostCaps::default(), Request::Hardware);
        assert_eq!(nothing.lacks, [Lack::Tier(0), Lack::ShaderModel, Lack::Device]);
    }

    /// The rule of `crate::caps`: the same capabilities choose the same
    /// backend under any vendor, device, driver or adapter kind.
    #[test]
    fn the_choice_does_not_depend_on_who_made_the_device() {
        for request in [Request::Software, Request::Hardware, Request::Auto] {
            for base in [able(), HostCaps { raytracing_tier: 10, ..able() }] {
                let expected = select(&base, request);
                let makers = [(0x10de, 0x2c05), (0x1002, 0x73df), (0x8086, 0x56a0), (0x1414, 0x8c)];
                for (vendor, device) in makers {
                    let other = HostCaps {
                        vendor,
                        device,
                        name: format!("{vendor:#x}"),
                        driver: Some([32, 0, 15, 1]),
                        software: vendor == 0x1414,
                        dedicated_mb: u64::from(device),
                        ..base.clone()
                    };
                    assert_eq!(select(&other, request), expected, "{vendor:#x} {request:?}");
                }
            }
        }
    }

    #[test]
    fn a_higher_tier_than_the_one_needed_is_enough() {
        let later = HostCaps { raytracing_tier: 12, ..able() };
        assert_eq!(select(&later, Request::Auto).backend, Backend::HardwareInline);
    }

    #[test]
    fn the_line_says_what_was_chosen_and_why() {
        let line = |caps: &HostCaps, request| select(caps, request).describe();
        assert_eq!(
            line(&able(), Request::Software),
            "software (VICTORY_RT_BACKEND is unset or software; this device could run the experimental hardware \
             backend)"
        );
        assert_eq!(
            line(&able(), Request::Hardware),
            "hardware inline RayQuery, experimental (VICTORY_RT_BACKEND=hardware)"
        );
        assert_eq!(line(&able(), Request::Auto), "hardware inline RayQuery, experimental (VICTORY_RT_BACKEND=auto)");
        let old = HostCaps { raytracing_tier: 10, ..able() };
        assert_eq!(
            line(&old, Request::Hardware),
            "software, though VICTORY_RT_BACKEND=hardware asked for hardware: this device cannot run the hardware \
             backend: ray tracing tier 1.0 (1.1 needed)"
        );
        assert_eq!(
            line(&HostCaps { raytracing_device: false, ..able() }, Request::Auto),
            "software (VICTORY_RT_BACKEND=auto; this device cannot run the hardware backend: no ID3D12Device5)"
        );
    }
}
