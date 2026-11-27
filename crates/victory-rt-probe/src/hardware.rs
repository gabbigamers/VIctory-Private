//! The run's device over Direct3D 12: the host ray-tracing backend
//! (`victory_gpu::rt::device::Rt`) on a queue of its own, the three
//! pipelines, the ray list and the hit buffer.
//!
//! **Not run on any hardware yet.** This file is type-checked for Windows
//! and no more: every Direct3D call in it is written from the API's
//! documentation and has never executed. It decides nothing: what is built,
//! dispatched and compared is `crate::run`'s, tested on any host over a
//! stand-in.
//!
//! One submission at a time: every call that records commands submits them
//! and waits before it returns, so nothing the card may still read is
//! written or released here, and a timestamp region is alone in its list.
//!
//! Memory, all counted by owner (`victory_gpu::account`, the backend's own
//! owners): the ray list and the poison records are upload memory and the
//! build inputs are too (`own_rt_buffers_mb`); the hit buffer is on the card
//! (`own_rt_scratch_mb`); structures are the backend's
//! (`own_rt_structures_mb`). One class's ray list and hit buffer live at a
//! time, and one path's structures and inputs; the poison records are as
//! many as the largest dispatch poisoned so far lists, at most a hit
//! buffer's worth, and stay until the device goes. docs/memory.md (R5) has
//! the sizes.
//!
//! Behind the switch a run is behind: [`Hardware::open`] opens nothing
//! unless `VICTORY_RT_BACKEND` asks for a path over host structures
//! (`native`, `hybrid`, `auto`, or `hardware`, the older name of `native`;
//! `crate::device::asked`). Whichever it names, both paths are measured.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL_12_0, ID3DBlob};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
    DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIDevice, IDXGIFactory4,
};
use windows::core::Interface;

use victory_gpu::account::{self, Owner};
use victory_gpu::rt::budget::Limits;
use victory_gpu::rt::cache::Handle;
use victory_gpu::rt::desc::{Inputs, Instance, TopDesc};
use victory_gpu::rt::device::{Buffer, Error, Outcome, Rt, commit_and_working_set};
use victory_gpu::rt::select::Lack;
use victory_gpu::rt::update::{Change, Decision, Policy};
use victory_shader::emit::host::Host;
use victory_shader::emit::rayquery::{self, Binding, Class, Form, HIT_BYTES, RAY_BYTES, RayQueryShader, Resource};

use crate::device::{
    Built, Device, Gpu, Level, Memory, POISON, Path, Structures, Trace, Traced, Unavailable, asked_by_environment,
    form_name, opened,
};
use crate::scene::Action;

/// One shader's pipeline and how its root parameters are filled: one root
/// parameter per binding, in the bindings' order.
struct Pipeline {
    root: ID3D12RootSignature,
    state: ID3D12PipelineState,
    bindings: Vec<Binding>,
}

/// One bottom level a path holds: the structure and the memory its last
/// build read.
struct HeldBottom {
    handle: Handle,
    /// Indices and transforms (triangles); `None` where the level has
    /// neither, and for boxes.
    fixed: Option<Buffer>,
    /// Vertex positions (triangles) or the boxes.
    moving: Buffer,
}

/// What a path holds.
#[derive(Default)]
struct Held {
    bottoms: Vec<HeldBottom>,
    top: Option<Handle>,
    triangle_data: Option<Buffer>,
}

pub struct Hardware {
    // Declared before the backend so that they are released first; every
    // submission that read them has run by then (see the module text).
    paths: [Held; 2],
    rays: Option<Buffer>,
    hits: Option<Buffer>,
    poison: Option<Buffer>,
    /// Records the ray list and the hit buffer hold.
    records: u32,
    pipelines: [Option<Pipeline>; 3],
    identity: Vec<(String, String)>,
    /// The content generation of the next build.
    generation: u64,
    /// The debug layer's messages, where the layer was asked for.
    layer: Option<ID3D12InfoQueue>,
    host: Host,
    rt: Rt,
}

fn place(path: Path) -> usize {
    match path {
        Path::Native => 0,
        Path::Hybrid => 1,
    }
}

fn form_place(form: Form) -> usize {
    match form {
        Form::Null => 0,
        Form::Triangles => 1,
        Form::Procedural => 2,
    }
}

fn said(error: Error) -> String {
    error.to_string()
}

/// A feature record of `device`, where it answers.
fn feature<T: Default>(device: &ID3D12Device, which: D3D12_FEATURE) -> Option<T> {
    let mut data = T::default();
    // SAFETY: the query writes a structure of the size given, which is the
    // one `which` names at each call below.
    unsafe { device.CheckFeatureSupport(which, (&raw mut data).cast(), size_of::<T>() as u32) }.ok()?;
    Some(data)
}

/// What the device and its adapter say of themselves, for the file's
/// header: for the report only, nothing is decided by any of it.
fn identity_of(device: &ID3D12Device) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut put = |key: &str, value: String| out.push((key.to_string(), value));
    // SAFETY: plain adapter queries; the description is a fixed array.
    unsafe {
        let adapter: Option<IDXGIAdapter3> = CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0))
            .and_then(|factory| factory.EnumAdapterByLuid(device.GetAdapterLuid()))
            .ok();
        match adapter {
            None => put("adapter", "the device's adapter did not answer".into()),
            Some(adapter) => {
                if let Ok(desc) = adapter.GetDesc1() {
                    let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
                    put("adapter", String::from_utf16_lossy(&desc.Description[..end]));
                    put("adapter_vendor_id", format!("{:04x} (for the report only)", desc.VendorId));
                    put("adapter_device_id", format!("{:04x} (for the report only)", desc.DeviceId));
                    let software = desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0;
                    put("adapter_software", if software { "yes" } else { "no" }.into());
                    put("adapter_dedicated_mb", (desc.DedicatedVideoMemory as u64 >> 20).to_string());
                    put("adapter_shared_mb", (desc.SharedSystemMemory as u64 >> 20).to_string());
                }
                match adapter.CheckInterfaceSupport(&IDXGIDevice::IID) {
                    Ok(v) => {
                        let parts = [48, 32, 16, 0].map(|shift| ((v >> shift) as u16).to_string());
                        put("driver", parts.join("."));
                    }
                    Err(_) => put("driver", "not given".into()),
                }
                let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                if adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).is_ok() {
                    put("video_budget_mb_at_start", (info.Budget >> 20).to_string());
                }
            }
        }
    }
    let tier = feature::<D3D12_FEATURE_DATA_D3D12_OPTIONS5>(device, D3D12_FEATURE_D3D12_OPTIONS5)
        .map(|o| o.RaytracingTier.0);
    put(
        "raytracing_tier",
        match tier {
            Some(0) => "none".into(),
            Some(tier) => format!("{}.{}", tier / 10, tier % 10),
            None => "unknown: the query failed".into(),
        },
    );
    put(
        "wave_lanes",
        match feature::<D3D12_FEATURE_DATA_D3D12_OPTIONS1>(device, D3D12_FEATURE_D3D12_OPTIONS1) {
            Some(o) if o.WaveOps.as_bool() => format!("{} to {}", o.WaveLaneCountMin, o.WaveLaneCountMax),
            Some(_) => "no wave operations".into(),
            None => "unknown: the query failed".into(),
        },
    );
    out
}

/// The video memory budget the adapter gives this process on the card now,
/// MiB, where it says.
fn budget_mb(device: &ID3D12Device) -> Option<u64> {
    // SAFETY: plain adapter queries with a valid out pointer.
    unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
        let adapter: IDXGIAdapter3 = factory.EnumAdapterByLuid(device.GetAdapterLuid()).ok()?;
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).ok()?;
        Some(info.Budget >> 20)
    }
}

/// A root signature with one root parameter per binding, in their order:
/// root constants for the constants, root descriptors for the rest.
fn root_signature(host: &Host, shader: &RayQueryShader) -> Result<ID3D12RootSignature, String> {
    let params: Vec<D3D12_ROOT_PARAMETER1> = shader
        .bindings
        .iter()
        .map(|binding| {
            let (kind, anonymous) = match binding.class {
                Class::Constants => (
                    D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                    D3D12_ROOT_PARAMETER1_0 {
                        Constants: D3D12_ROOT_CONSTANTS {
                            ShaderRegister: binding.register,
                            RegisterSpace: binding.space,
                            Num32BitValues: 4,
                        },
                    },
                ),
                class => (
                    if class == Class::RawBufferWritten {
                        D3D12_ROOT_PARAMETER_TYPE_UAV
                    } else {
                        D3D12_ROOT_PARAMETER_TYPE_SRV
                    },
                    D3D12_ROOT_PARAMETER1_0 {
                        Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                            ShaderRegister: binding.register,
                            RegisterSpace: binding.space,
                            Flags: D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                        },
                    },
                ),
            };
            D3D12_ROOT_PARAMETER1 {
                ParameterType: kind,
                Anonymous: anonymous,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            }
        })
        .collect();
    let description = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
            Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                NumStaticSamplers: 0,
                pStaticSamplers: std::ptr::null(),
                Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
            },
        },
    };
    let mut blob: Option<ID3DBlob> = None;
    // SAFETY: the description points at `params`, which outlives the call.
    unsafe { D3D12SerializeVersionedRootSignature(&description, &mut blob, None) }
        .map_err(|e| format!("the root signature did not serialise: {e}"))?;
    let blob = blob.ok_or("no root signature blob")?;
    // SAFETY: the blob's own pointer and size; it lives past the use.
    let bytes = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) };
    host.root_signature_bytes(bytes)
}

/// A barrier that moves `resource` from one state to another.
fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: a borrowed pointer; the barrier never releases it.
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

/// Turns the Direct3D 12 debug layer on for the devices made after it.
fn debug_layer_on() -> Result<(), String> {
    let mut layer: Option<ID3D12Debug> = None;
    // SAFETY: a plain interface query with a valid out pointer.
    unsafe { D3D12GetDebugInterface(&mut layer) }.map_err(|e| e.to_string())?;
    let layer = layer.ok_or("the query gave no interface")?;
    // SAFETY: called before this process makes its device, as the layer asks.
    unsafe { layer.EnableDebugLayer() };
    Ok(())
}

/// The name of a debug-layer message's severity.
fn severity(of: D3D12_MESSAGE_SEVERITY) -> &'static str {
    match of {
        D3D12_MESSAGE_SEVERITY_CORRUPTION => "corruption",
        D3D12_MESSAGE_SEVERITY_ERROR => "error",
        D3D12_MESSAGE_SEVERITY_WARNING => "warning",
        D3D12_MESSAGE_SEVERITY_INFO => "info",
        _ => "message",
    }
}

impl Hardware {
    /// The default adapter's device, as a run uses, with the host
    /// ray-tracing backend on a queue of its own. Behind the switch a run
    /// is behind: unless `VICTORY_RT_BACKEND` asks for a path over host
    /// structures (`native`, `hybrid`, `auto`, or the older name
    /// `hardware`) nothing is opened, no device is made and nothing is
    /// allocated. Where it does, the device's capabilities decide whether
    /// the backend opens, and both paths are measured whichever is named.
    ///
    /// With `debug_layer`, the Direct3D 12 debug layer is turned on first
    /// and its messages are handed to the run ([`Device::messages`]); the
    /// file says so, since no time of such a run is a measurement. Where
    /// the layer cannot be turned on, nothing runs.
    pub fn open(debug_layer: bool) -> Result<Hardware, Unavailable> {
        let request = asked_by_environment()?;
        if debug_layer {
            debug_layer_on().map_err(Unavailable::NoDebugLayer)?;
        }
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: plain device creation with a valid out pointer.
        unsafe { D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device) }
            .map_err(|e| Unavailable::NoDevice(e.to_string()))?;
        let device = device.ok_or_else(|| Unavailable::NoDevice("the call gave none".into()))?;
        let layer = match debug_layer {
            true => Some(device.cast::<ID3D12InfoQueue>().map_err(|e| {
                Unavailable::NoDebugLayer(format!("the device made under it has no message queue: {e}"))
            })?),
            false => None,
        };
        let mut identity = identity_of(&device);
        let named = |key: &str| identity.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let adapter = named("adapter").unwrap_or_else(|| "the device".into());

        let limits = Limits::from_env(budget_mb(&device));
        let policy = Policy::from_env();
        let rt = Rt::open_on(&device, limits, policy).map_err(|e| match e {
            Error::Unsupported(lacks) => {
                let tier = named("raytracing_tier").unwrap_or_default();
                let mut said: Vec<String> = lacks.iter().map(Lack::to_string).collect();
                said.push(format!("it reports ray tracing tier: {tier}"));
                Unavailable::Lacks { adapter: adapter.clone(), lacks: said }
            }
            other => Unavailable::Backend(other.to_string()),
        })?;
        let model = feature_model(&device);
        identity.push(("shader_model_6_6".into(), if model { "yes" } else { "no" }.into()));
        identity.push(("rt_backend_switch".into(), opened(request)));
        identity.push((
            "d3d12_debug_layer".into(),
            match debug_layer {
                true => "on: a run under the validation layer, whose times are no measurements".into(),
                false => "off".into(),
            },
        ));
        identity.push(("rt_structures_bound_mb".into(), (limits.structures >> 20).to_string()));
        identity.push(("rt_rebuild_after_refits".into(), policy.rebuild_after_refits.to_string()));
        Ok(Hardware {
            paths: Default::default(),
            rays: None,
            hits: None,
            poison: None,
            records: 0,
            pipelines: [None, None, None],
            identity,
            generation: 0,
            layer,
            host: Host::from_device(device),
            rt,
        })
    }

    /// The region `label`'s time out of `timings`.
    fn region(timings: &[victory_gpu::rt::timing::Timing], label: &str) -> Option<f64> {
        timings.iter().find(|t| t.label == label).and_then(|t| t.gpu_ms)
    }
}

/// Whether `device` runs shader model 6.6.
fn feature_model(device: &ID3D12Device) -> bool {
    let mut model = D3D12_FEATURE_DATA_SHADER_MODEL { HighestShaderModel: D3D_SHADER_MODEL_6_6 };
    // SAFETY: the query writes a structure of the size given.
    let asked = unsafe {
        device.CheckFeatureSupport(D3D12_FEATURE_SHADER_MODEL, (&raw mut model).cast(), size_of_val(&model) as u32)
    };
    asked.is_ok() && model.HighestShaderModel.0 >= D3D_SHADER_MODEL_6_6.0
}

/// What one structure's build call did, with the label of its timestamp
/// region.
struct Recorded {
    label: String,
    top: bool,
    refit: bool,
}

/// Takes note of what the backend decided for one structure: counted, and
/// said where a refit was asked and a build made.
fn decided(built: &mut Built, recorded: &mut Vec<Recorded>, what: &str, top: bool, asked: Action, outcome: &Outcome) {
    let refit = outcome.decision == Decision::Refit;
    if refit {
        built.refits += 1;
    } else {
        built.builds += 1;
    }
    if let (Action::Refit, Decision::Rebuild(why)) = (asked, outcome.decision) {
        built.notes.push(format!("{what}: asked to be refitted; the backend built it again ({why:?})"));
    }
    recorded.push(Recorded { label: what.to_string(), top, refit });
}

impl Device for Hardware {
    fn identity(&self) -> Vec<(String, String)> {
        self.identity.clone()
    }

    fn pipeline(&mut self, shader: &RayQueryShader) -> Result<Duration, String> {
        let started = Instant::now();
        let root = root_signature(&self.host, shader)?;
        let state = self.host.compute_pipeline_with(&root, &shader.dxil)?;
        let took = started.elapsed();
        self.pipelines[form_place(shader.form)] = Some(Pipeline { root, state, bindings: shader.bindings.clone() });
        Ok(took)
    }

    fn rays(&mut self, list: &[u8]) -> Result<Duration, String> {
        // The last class's list and records go: every dispatch that read or
        // wrote them has run, and what a failed step may have left recorded
        // is submitted and waited for first.
        self.rt.flush().map_err(said)?;
        (self.rays, self.hits, self.records) = (None, None, 0);
        let records = list.len() as u64 / u64::from(RAY_BYTES);
        if records == 0 || records > u64::from(rayquery::MAX_RAYS) {
            return Err(format!("a ray list of {records} records: one dispatch lists 1 to {}", rayquery::MAX_RAYS));
        }
        let started = Instant::now();
        self.rays = Some(self.rt.upload(list).map_err(said)?);
        let took = started.elapsed();
        self.hits = Some(self.rt.output(records * u64::from(HIT_BYTES)).map_err(said)?);
        self.records = records as u32;
        Ok(took)
    }

    fn build(&mut self, s: &Structures) -> Result<Built, String> {
        let at = place(s.path);
        // What a step that failed left timed is not this call's.
        drop(self.rt.timings());
        let mut built = Built::default();
        let mut recorded: Vec<Recorded> = Vec::new();
        let (mut upload, mut record) = (Duration::ZERO, Duration::ZERO);
        // Structures are named to the backend by path and place.
        let id = |i: u64| (at as u64) << 32 | i;

        for (i, (level, action)) in s.bottoms.iter().enumerate() {
            let (fixed, moving): (&[u8], &[u8]) = match level {
                Level::Triangles(t) => (&t.fixed, &t.positions),
                Level::Boxes(b) => (&[], &b.boxes),
            };
            let have = self.paths[at].bottoms.get(i).map(|b| b.moving.bytes());
            let what = format!("bottom level {i}");
            // The inputs, into upload memory. A refit uploads what moves
            // and no more; a build uploads all of it.
            let started = Instant::now();
            let mut uploaded = moving.len() as u64;
            let (change, new) = match (*action, have) {
                (Action::Keep, Some(_)) => continue,
                (Action::Keep, None) => return Err(format!("{what} is kept before it was built")),
                // A refit's positions (or boxes) are written over the ones
                // its structure was built from: no submission still reads
                // them, and the description stays the same.
                (Action::Refit, Some(bytes)) if bytes == moving.len().max(1) as u64 => {
                    if !self.paths[at].bottoms[i].moving.write(0, moving) {
                        return Err(format!("{what}: its upload memory did not take the new positions"));
                    }
                    (Change::Positions, None)
                }
                (Action::Refit, _) => {
                    return Err(format!("{what}: a refit of inputs of another size, or before a build"));
                }
                (Action::Rebuild, have) => {
                    if have.is_none() && i != self.paths[at].bottoms.len() {
                        return Err(format!("{what} is built before the one ahead of it"));
                    }
                    uploaded += fixed.len() as u64;
                    let fixed = match fixed.is_empty() {
                        true => None,
                        false => Some(self.rt.upload(fixed).map_err(said)?),
                    };
                    (Change::Other, Some((fixed, self.rt.upload(moving).map_err(said)?)))
                }
            };
            built.input_bytes += uploaded;
            upload += started.elapsed();

            // The description and the build, recorded as a region of its
            // own with the barriers the backend puts around it. The backend
            // asks the device for the sizes and makes a new structure's
            // memory in this call, so both are in this time.
            let started = Instant::now();
            let (fixed_at, moving_at) = match (&new, self.paths[at].bottoms.get(i)) {
                (Some((fixed, moving)), _) => (fixed.as_ref().map_or(0, Buffer::address), moving.address()),
                (None, Some(held)) => (held.fixed.as_ref().map_or(0, Buffer::address), held.moving.address()),
                (None, None) => return Err(format!("{what}: no inputs")),
            };
            let desc = match level {
                Level::Triangles(t) => t.desc(fixed_at, moving_at),
                Level::Boxes(b) => b.desc(moving_at),
            };
            self.generation += 1;
            let generation = self.generation;
            let outcome = self
                .rt
                .timed(&what, |rt| rt.build_bottom(id(i as u64), generation, &desc, change))
                .map_err(|e| format!("{what}: {e}"))?;
            decided(&mut built, &mut recorded, &what, false, *action, &outcome);
            if let Some((fixed, moving)) = new {
                let held = HeldBottom { handle: outcome.handle, fixed, moving };
                // The inputs of the build before this one go here: its
                // submission has run, and the structure does not refer to
                // them after that.
                match self.paths[at].bottoms.get_mut(i) {
                    Some(slot) => *slot = held,
                    None => self.paths[at].bottoms.push(held),
                }
            }
            record += started.elapsed();
        }

        if s.top != Action::Keep {
            let started = Instant::now();
            let instances = s
                .instances
                .iter()
                .enumerate()
                .map(|(n, instance)| {
                    let bottom = self.paths[at].bottoms.get(instance.bottom as usize).filter(|_| {
                        (instance.bottom as usize) < s.bottoms.len()
                    });
                    let bottom = bottom.ok_or_else(|| format!("instance {n}: no bottom level {}", instance.bottom))?;
                    Ok(Instance {
                        transform: instance.transform,
                        id: instance.id,
                        mask: instance.mask,
                        hit_group: 0,
                        flags: instance.flags,
                        bottom: Some(bottom.handle),
                    })
                })
                .collect::<Result<Vec<Instance>, String>>()?;
            let desc = TopDesc { instances, flags: s.top_flags, inputs: Inputs::Ready };
            let change = if s.top == Action::Refit { Change::Positions } else { Change::Other };
            self.generation += 1;
            let generation = self.generation;
            let outcome = self
                .rt
                .timed("top level", |rt| rt.build_top(id(u64::from(u32::MAX)), generation, &desc, change))
                .map_err(|e| format!("top level: {e}"))?;
            decided(&mut built, &mut recorded, "top level", true, s.top, &outcome);
            self.paths[at].top = Some(outcome.handle);
            record += started.elapsed();
        }
        // Bottom levels the scene no longer has: given up once the top
        // level is built without them. Their memory goes when the list has
        // run.
        let extra: Vec<HeldBottom> = match self.paths[at].bottoms.len() > s.bottoms.len() {
            true => self.paths[at].bottoms.drain(s.bottoms.len()..).collect(),
            false => Vec::new(),
        };
        for bottom in &extra {
            self.rt.free(bottom.handle);
        }
        // The hybrid's triangle data. The one before it is kept until this
        // submission has run, though no dispatch is recorded in it.
        let mut replaced = None;
        if let Some(data) = s.triangle_data {
            let started = Instant::now();
            replaced = self.paths[at].triangle_data.replace(self.rt.upload(data).map_err(said)?);
            built.input_bytes += data.len() as u64;
            upload += started.elapsed();
        }

        (built.upload_cpu, built.record_cpu) = (upload, record);
        built.wait = self.rt.flush().map_err(said)?;
        drop((extra, replaced));
        let timings = self.rt.timings();
        for structure in &recorded {
            let gpu_ms = Hardware::region(&timings, &structure.label);
            let sum: &mut Gpu = match (structure.top, structure.refit) {
                (false, false) => &mut built.bottom_build,
                (false, true) => &mut built.bottom_refit,
                (true, false) => &mut built.top_build,
                (true, true) => &mut built.top_refit,
            };
            sum.add(gpu_ms);
        }
        Ok(built)
    }

    fn trace(&mut self, trace: &Trace) -> Result<Traced, String> {
        let name = form_name(trace.form);
        let groups = rayquery::groups(trace.rays)
            .filter(|_| trace.rays > 0 && trace.rays <= self.records)
            .ok_or_else(|| format!("{} rays listed of {} uploaded", trace.rays, self.records))?;
        let pipeline =
            self.pipelines[form_place(trace.form)].as_ref().ok_or_else(|| format!("no {name} pipeline"))?;
        let (Some(rays), Some(hits)) = (&self.rays, &self.hits) else {
            return Err("no ray list was uploaded".into());
        };
        let held = trace.path.map(|path| &self.paths[place(path)]);
        // What a step that failed left timed is not this call's.
        drop(self.rt.timings());
        let started = Instant::now();

        if trace.poison {
            let bytes = u64::from(trace.rays) * u64::from(HIT_BYTES);
            if self.poison.as_ref().is_none_or(|poison| poison.bytes() < bytes) {
                let words: Vec<u8> = std::iter::repeat_n(POISON.to_le_bytes(), (bytes / 4) as usize).flatten().collect();
                self.poison = Some(self.rt.upload(&words).map_err(said)?);
            }
            let Some(poison) = &self.poison else { return Err("no poison records".into()) };
            let (to, from, state) = (hits.resource().clone(), poison.resource().clone(), hits.state());
            self.rt
                .record(|list| {
                    // SAFETY: an open list and two live buffers of ours,
                    // the hit buffer at least `bytes` long and moved to the
                    // copy-destination state for the copy and back; the
                    // poison buffer is upload memory, readable as it is.
                    unsafe {
                        list.ResourceBarrier(&[transition(&to, state, D3D12_RESOURCE_STATE_COPY_DEST)]);
                        list.CopyBufferRegion(&to, 0, &from, 0, bytes);
                        list.ResourceBarrier(&[transition(&to, D3D12_RESOURCE_STATE_COPY_DEST, state)]);
                    }
                })
                .map_err(said)?;
        }

        let constants = [trace.rays, 0u32, 0, 0];
        let (root, state) = (pipeline.root.clone(), pipeline.state.clone());
        self.rt
            .record(|list| {
                // SAFETY: an open list; the root signature and the pipeline
                // live as long as `self`, which waits for the list below.
                unsafe {
                    list.SetComputeRootSignature(&root);
                    list.SetPipelineState(&state);
                }
            })
            .map_err(said)?;
        for (n, binding) in pipeline.bindings.iter().enumerate() {
            let n = n as u32;
            // A root view's address: the buffer lives until the list has
            // run, and the shader reads or writes inside the listed rays.
            let view = match binding.resource {
                Resource::Constants => {
                    self.rt
                        .record(|list| {
                            // SAFETY: an open list with the root signature
                            // set; four dwords are read from `constants`
                            // during the call.
                            unsafe { list.SetComputeRoot32BitConstants(n, 4, constants.as_ptr().cast(), 0) }
                        })
                        .map_err(said)?;
                    continue;
                }
                Resource::Scene => {
                    let top = held.and_then(|held| held.top).ok_or("the path has no top level")?;
                    self.rt.bind_top(n, top).map_err(|e| format!("binding the top level: {e}"))?;
                    continue;
                }
                Resource::Rays => rays.address(),
                Resource::Hits => hits.address(),
                Resource::Triangles => held
                    .and_then(|held| held.triangle_data.as_ref())
                    .map(Buffer::address)
                    .ok_or("the path has no triangle data")?,
            };
            let written = binding.class == Class::RawBufferWritten;
            self.rt
                .record(|list| {
                    // SAFETY: an open list with the root signature set, and
                    // the address of a live buffer of ours.
                    unsafe {
                        if written {
                            list.SetComputeRootUnorderedAccessView(n, view);
                        } else {
                            list.SetComputeRootShaderResourceView(n, view);
                        }
                    }
                })
                .map_err(said)?;
        }
        // The region that is timed: the dispatch and nothing else.
        self.rt
            .timed("trace", |rt| {
                rt.record(|list| {
                    // SAFETY: an open list with pipeline, root signature
                    // and every root parameter set above.
                    unsafe { list.Dispatch(groups, 1, 1) }
                })
            })
            .map_err(said)?;
        let record_cpu = started.elapsed();
        let wait = self.rt.flush().map_err(said)?;
        let gpu_ms = Hardware::region(&self.rt.timings(), "trace");
        Ok(Traced { gpu_ms, record_cpu, wait })
    }

    fn hits(&mut self, records: u32) -> Result<(Vec<u8>, Duration), String> {
        let hits = self.hits.as_ref().filter(|_| records <= self.records);
        let hits = hits.ok_or_else(|| format!("{records} records asked of a hit buffer of {}", self.records))?;
        self.rt.read_back(hits, 0, u64::from(records) * u64::from(HIT_BYTES)).map_err(said)
    }

    fn release(&mut self, path: Path) -> Result<(), String> {
        let held = std::mem::take(&mut self.paths[place(path)]);
        if let Some(top) = held.top {
            self.rt.free(top);
        }
        for bottom in &held.bottoms {
            self.rt.free(bottom.handle);
        }
        // Submits what a failed step may have left recorded, waits, and
        // lets the backend release what was given up; with nothing
        // recorded there is nothing to wait for.
        self.rt.flush().map_err(said)?;
        drop(held);
        Ok(())
    }

    fn memory(&mut self) -> Memory {
        let video = self.rt.video_memory_mb();
        let (commit, working_set) = commit_and_working_set();
        Memory {
            structures: account::gauge(Owner::RtStructures).now(),
            scratch: account::gauge(Owner::RtScratch).now(),
            buffers: account::gauge(Owner::RtBuffers).now(),
            video_local_mb: video.map(|[local, _]| local.0),
            video_budget_mb: video.map(|[local, _]| local.1),
            video_shared_mb: video.map(|[_, shared]| shared.0),
            commit: (commit > 0).then_some(commit),
            working_set: (working_set > 0).then_some(working_set),
        }
    }

    fn counters(&self) -> String {
        self.rt.stats().line()
    }

    fn messages(&mut self) -> Vec<String> {
        let Some(queue) = &self.layer else { return Vec::new() };
        let mut out = Vec::new();
        // SAFETY: each message is read into a buffer of the size the queue
        // reports for it, aligned for the structure, and its text is read
        // while the buffer lives; then the queue is emptied.
        unsafe {
            for i in 0..queue.GetNumStoredMessages() {
                let mut size = 0;
                if queue.GetMessage(i, None, &mut size).is_err() || size == 0 {
                    continue;
                }
                let mut buffer = vec![0u64; size.div_ceil(8)];
                let message = buffer.as_mut_ptr().cast::<D3D12_MESSAGE>();
                if queue.GetMessage(i, Some(message), &mut size).is_ok() {
                    let m = &*message;
                    let text = std::slice::from_raw_parts(m.pDescription, m.DescriptionByteLength);
                    let text = String::from_utf8_lossy(text);
                    out.push(format!("{} {}: {}", severity(m.Severity), m.ID.0, text.trim_end_matches('\0')));
                }
            }
            queue.ClearStoredMessages();
        }
        out
    }
}
