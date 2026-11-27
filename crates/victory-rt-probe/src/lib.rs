//! The synthetic ray-tracing probe (docs/dxr-validation.md): `victory-rt`'s
//! generated scenes and rays through the host ray-tracing backend
//! (`victory_gpu::rt`) with the three shaders of
//! `victory_shader::emit::rayquery`, each path's hit records compared with
//! the CPU reference and with the CPU model of the software traversal, and
//! each cost measured apart. It reads no file of any title.
//!
//! **Nothing here has run on a device.** Everything that can be decided
//! without one is plain data and plain logic, tested on any host:
//!
//! * [`scene`]: a scene as the probe hands it to a device;
//! * [`adapter`]: the only use of `victory-rt` (cases, reference, software
//!   model, comparison, the format of the hit records);
//! * [`inputs`]: a scene as the bytes and descriptions a host build reads;
//! * [`rays`]: generated rays as what the shaders take;
//! * [`records`]: hit records read back, refusals among them;
//! * [`stats`]: the figures of repeated measurements;
//! * [`row`], [`file`]: the result file;
//! * [`plan`]: what a run does, counted;
//! * [`run`]: the run itself, over a [`device::Device`].
//!
//! [`device`] is what the run asks of a device. `hardware` (Windows only)
//! answers it with Direct3D 12, behind the switch a run's native and
//! hybrid ray tracing is behind (`VICTORY_RT_BACKEND`, read through
//! `victory_gpu::rt::select`'s parser by [`device::asked`]: `native`,
//! `hybrid`, `hardware` or `auto` open it, and the probe measures both
//! paths whichever is named), and is type-checked for Windows only; the
//! host tests answer it with a brute-force stand-in on the CPU, which is
//! how the run's own logic is tested without a device. The stand-in is
//! test code: it is no path, and no result file is written from it.

pub mod adapter;
pub mod device;
pub mod file;
#[cfg(windows)]
pub mod hardware;
pub mod inputs;
pub mod plan;
pub mod rays;
pub mod records;
pub mod row;
pub mod run;
pub mod scene;
pub mod stats;

#[cfg(test)]
mod standin;

/// The version of the probe: of what it runs and of the result file's
/// columns. A file says which wrote it. The number is defined once, in
/// `victory_gpu::rt::select`, beside what the selection accepts verdicts
/// of the probe by (`select::Standard::probe`): this crate depends on
/// that one, and that one cannot depend on this.
pub const PROBE_VERSION: u32 = victory_gpu::rt::select::PROBE_VERSION;
