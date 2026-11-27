//! The D3D12 side of the experimental backend: host structures built,
//! refitted, compacted and released with the calls of `ID3D12Device5` and
//! `ID3D12GraphicsCommandList4`, as a thin layer over the device-free parts
//! of `crate::rt`, which decide everything that can be decided without a
//! device and are tested on any host.
//!
//! **Not run on any hardware yet.** This file is type-checked for Windows
//! and no more: every D3D12 call in it is written from the API's
//! documentation and has never executed.
//!
//! An [`Rt`] records into one command list of its own, and that is all
//! `super::cache` models: every build and every read between two submits
//! is in that list. Its user (a validation tool today) uploads geometry,
//! builds, binds a top level and dispatches through it, then submits and
//! waits. Nothing in a run's draw or submission path calls it, and joining
//! it to a title's many open lists needs more than is here
//! (`super::cache`).
//!
//! What it leaves to its caller, since only the caller can know it: that
//! the memory a build reads holds what it should when the list runs, stays
//! alive and unwritten until the list has run ([`Rt::keep`] holds an
//! object that long), and is readable by a build (upload memory is; a
//! buffer elsewhere must be in the non-pixel-shader-resource state).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::IDXGIAdapter3;
use windows::core::Interface;

use crate::account::{Held, Owner};
use crate::host::Host;

use super::budget::{self, Limits};
use super::cache::{Cache, Handle, Key, Level, Lookup};
use super::desc::{self, BottomDesc, Geometry, INSTANCE_BYTES, TopDesc};
use super::life::{Refusal, State};
use super::retire::RetireQueue;
use super::scratch::{self, GRANULE, Pool};
use super::select::{self, Lack};
use super::stats::{Count, Stats};
use super::timing::{Region, Regions, Results, Timing};
use super::update::{self, Built, Change, Decision, Policy, Uncompacted, Unplanned, Why};

// The numbers `super::desc` writes out are the system's.
const _: () = {
    use desc::*;
    assert!(STRUCTURE_ALIGN == D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BYTE_ALIGNMENT as u64);
    assert!(INSTANCE_ALIGN == D3D12_RAYTRACING_INSTANCE_DESCS_BYTE_ALIGNMENT as u64);
    assert!(TRANSFORM_ALIGN == D3D12_RAYTRACING_TRANSFORM3X4_BYTE_ALIGNMENT as u64);
    assert!(BOX_ALIGN == D3D12_RAYTRACING_AABB_BYTE_ALIGNMENT as u64);
    assert!(MAX_GEOMETRIES == D3D12_RAYTRACING_MAX_GEOMETRIES_PER_BOTTOM_LEVEL_ACCELERATION_STRUCTURE as u64);
    assert!(MAX_PRIMITIVES == D3D12_RAYTRACING_MAX_PRIMITIVES_PER_BOTTOM_LEVEL_ACCELERATION_STRUCTURE as u64);
    assert!(MAX_INSTANCES == D3D12_RAYTRACING_MAX_INSTANCES_PER_TOP_LEVEL_ACCELERATION_STRUCTURE as u64);
    assert!(INSTANCE_BYTES == size_of::<D3D12_RAYTRACING_INSTANCE_DESC>());
    assert!(BOX_BYTES == size_of::<D3D12_RAYTRACING_AABB>() as u64);
    assert!(TRANSFORM_BYTES == size_of::<[f32; 12]>() as u64);
    // The strides D3D12 reads are 32 bits of a 64-bit field.
    assert!(MAX_STRIDE == u32::MAX as u64);
    assert!(size_of::<D3D12_GPU_VIRTUAL_ADDRESS_AND_STRIDE>() == 16);
    assert!(PERFORM_UPDATE as i32 == D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PERFORM_UPDATE.0);
    let all =
        BuildFlags { allow_update: true, allow_compaction: true, prefer: Prefer::FastTrace, minimize_memory: true };
    assert!(
        all.bits() as i32
            == D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_ALLOW_UPDATE.0
                | D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_ALLOW_COMPACTION.0
                | D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PREFER_FAST_TRACE.0
                | D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_MINIMIZE_MEMORY.0
    );
    let fast_build =
        BuildFlags { allow_update: false, allow_compaction: false, prefer: Prefer::FastBuild, minimize_memory: false };
    assert!(fast_build.bits() as i32 == D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PREFER_FAST_BUILD.0);
    let both = GeometryFlags { opaque: true, no_duplicate_any_hit: true };
    assert!(
        both.bits() as i32
            == D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE.0 | D3D12_RAYTRACING_GEOMETRY_FLAG_NO_DUPLICATE_ANYHIT_INVOCATION.0
    );
    assert!(INSTANCE_CULL_DISABLE as i32 == D3D12_RAYTRACING_INSTANCE_FLAG_TRIANGLE_CULL_DISABLE.0);
    assert!(INSTANCE_FRONT_COUNTERCLOCKWISE as i32 == D3D12_RAYTRACING_INSTANCE_FLAG_TRIANGLE_FRONT_COUNTERCLOCKWISE.0);
    assert!(INSTANCE_FORCE_OPAQUE as i32 == D3D12_RAYTRACING_INSTANCE_FLAG_FORCE_OPAQUE.0);
    assert!(INSTANCE_FORCE_NON_OPAQUE as i32 == D3D12_RAYTRACING_INSTANCE_FLAG_FORCE_NON_OPAQUE.0);
    assert!(VertexFormat::Float3.dxgi() as i32 == DXGI_FORMAT_R32G32B32_FLOAT.0);
    assert!(VertexFormat::Float2.dxgi() as i32 == DXGI_FORMAT_R32G32_FLOAT.0);
    assert!(VertexFormat::Half4.dxgi() as i32 == DXGI_FORMAT_R16G16B16A16_FLOAT.0);
    assert!(VertexFormat::Half2.dxgi() as i32 == DXGI_FORMAT_R16G16_FLOAT.0);
    assert!(VertexFormat::Snorm16x4.dxgi() as i32 == DXGI_FORMAT_R16G16B16A16_SNORM.0);
    assert!(VertexFormat::Snorm16x2.dxgi() as i32 == DXGI_FORMAT_R16G16_SNORM.0);
    assert!(IndexFormat::U16.dxgi() as i32 == DXGI_FORMAT_R16_UINT.0);
    assert!(IndexFormat::U32.dxgi() as i32 == DXGI_FORMAT_R32_UINT.0);
    assert!(DXGI_FORMAT_UNKNOWN.0 == 0);
    assert!(D3D12_RAYTRACING_TIER_1_1.0 == 11);
};

/// Why the backend did not do what was asked. Every one is counted
/// ([`Rt::stats`]); the caller's case stays with the software backend.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// No path over host structures is asked for, or the device cannot run
    /// one: the line that says what is selected, and why (`super::select`).
    NotSelected(String),
    /// The device lacks what the backend needs.
    Unsupported(Vec<Lack>),
    /// A description D3D12 or this backend does not take.
    Invalid(desc::Invalid),
    /// A structure's state forbids it.
    State(Refusal),
    /// Structure memory would pass its bound.
    Budget(budget::Over),
    /// Scratch or instance memory would pass its pool's bound.
    Pool(scratch::Refused),
    /// A D3D12 call failed, or answered what cannot be used.
    Device(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotSelected(line) => write!(f, "no path over host structures is selected: {line}"),
            Error::Unsupported(lacks) => {
                let lacks: Vec<String> = lacks.iter().map(Lack::to_string).collect();
                write!(f, "the device cannot run the hardware backend: {}", lacks.join(", "))
            }
            Error::Invalid(invalid) => write!(f, "not built: {invalid}"),
            Error::State(refusal) => write!(f, "refused: {refusal}"),
            Error::Budget(over) => write!(f, "refused: {over}"),
            Error::Pool(refused) => write!(f, "refused: {refused}"),
            Error::Device(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for Error {}

/// A failed D3D12 call as an [`Error`], with what was being done.
fn failed(what: &'static str) -> impl Fn(windows::core::Error) -> Error {
    move |e| Error::Device(format!("{what}: {e}"))
}

/// Where a buffer lives and what state it is made in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// A structure's result memory: video memory, in the acceleration
    /// structure state for good.
    Structure,
    /// Video memory shaders and builds write: scratch, compacted sizes, a
    /// validation dispatch's output. In the unordered-access state.
    Unordered,
    /// Upload memory, mapped.
    Upload,
    /// Read-back memory.
    Readback,
}

/// A buffer of the backend's, counted under its owner while it lives.
pub struct Buffer {
    resource: ID3D12Resource,
    address: u64,
    bytes: u64,
    /// Where upload memory is mapped; 0 for the other kinds.
    mapped: usize,
    /// The state it is in between the backend's commands.
    state: D3D12_RESOURCE_STATES,
    _held: Held,
}

impl Buffer {
    /// The host GPU address of its first byte.
    pub fn address(&self) -> u64 {
        self.address
    }

    /// The bytes asked for when it was made.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }

    /// The state it is in between the backend's commands: a caller that
    /// records a copy into or out of it moves it from this state and back.
    pub fn state(&self) -> D3D12_RESOURCE_STATES {
        self.state
    }

    /// Writes `bytes` at `offset` of upload memory. Whether it did: not
    /// outside the buffer, and not into memory the CPU does not map. The
    /// caller keeps the ordering contract: nothing submitted and not yet
    /// run reads these bytes.
    pub fn write(&self, offset: u64, bytes: &[u8]) -> bool {
        let fits = offset.checked_add(bytes.len() as u64).is_some_and(|end| end <= self.bytes);
        if self.mapped == 0 || !fits {
            return false;
        }
        // SAFETY: the buffer is mapped for its whole length and lives as
        // long as `self`; the range was checked against that length.
        unsafe {
            let to = (self.mapped as *mut u8).add(offset as usize);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), to, bytes.len());
        }
        true
    }
}

fn make_buffer(device: &ID3D12Device5, bytes: u64, place: Place, owner: Owner) -> windows::core::Result<Buffer> {
    let bytes = bytes.max(1);
    let heap = D3D12_HEAP_PROPERTIES {
        Type: match place {
            Place::Structure | Place::Unordered => D3D12_HEAP_TYPE_DEFAULT,
            Place::Upload => D3D12_HEAP_TYPE_UPLOAD,
            Place::Readback => D3D12_HEAP_TYPE_READBACK,
        },
        ..Default::default()
    };
    let written = matches!(place, Place::Structure | Place::Unordered);
    let description = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: if written { D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS } else { D3D12_RESOURCE_FLAG_NONE },
        ..Default::default()
    };
    let state = match place {
        Place::Structure => D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE,
        Place::Unordered => D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        Place::Upload => D3D12_RESOURCE_STATE_GENERIC_READ,
        Place::Readback => D3D12_RESOURCE_STATE_COPY_DEST,
    };
    let mut resource: Option<ID3D12Resource> = None;
    // SAFETY: plain creation with a valid description.
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &description, state, None, &mut resource) }?;
    let resource = resource.ok_or_else(windows::core::Error::empty)?;
    let mut mapped = std::ptr::null_mut();
    if place == Place::Upload {
        // SAFETY: an upload buffer, mapped until it is released.
        unsafe { resource.Map(0, None, Some(&mut mapped)) }?;
    }
    // SAFETY: a live buffer.
    let address = unsafe { resource.GetGPUVirtualAddress() };
    // A committed buffer takes whole 64 KiB units: that is what is counted.
    let held = Held::new(owner, bytes.next_multiple_of(GRANULE));
    Ok(Buffer { resource, address, bytes, mapped: mapped as usize, state, _held: held })
}

/// A barrier that orders every write to `resource` before what follows.
fn uav_barrier(resource: &ID3D12Resource) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                // SAFETY: a borrowed pointer; the barrier never releases it.
                pResource: unsafe { std::mem::transmute_copy(resource) },
            }),
        },
    }
}

/// The bytes at `offset` of read-back memory.
fn read(buffer: &ID3D12Resource, offset: usize, len: usize) -> windows::core::Result<Vec<u8>> {
    let mut data = std::ptr::null_mut();
    let range = D3D12_RANGE { Begin: offset, End: offset + len };
    // SAFETY: a read-back buffer at least `offset + len` long, mapped for
    // the copy; nothing was written through the mapping.
    unsafe {
        buffer.Map(0, Some(&range), Some(&mut data))?;
        let bytes = std::slice::from_raw_parts(data.cast::<u8>().add(offset), len).to_vec();
        buffer.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
        Ok(bytes)
    }
}

/// The queue lists run on and the fence that counts them.
enum Queue {
    /// The run's own (`crate::host`): fence values are shared with the
    /// title's submissions.
    Shared(&'static Host),
    /// One made for this backend alone, on a device of the caller's.
    Own { queue: ID3D12CommandQueue, fence: ID3D12Fence, submitted: u64 },
}

impl Queue {
    fn submit(&mut self, list: &ID3D12GraphicsCommandList4) -> windows::core::Result<u64> {
        let lists = [Some(list.cast::<ID3D12CommandList>()?)];
        match self {
            Queue::Shared(host) => host.submit_unwaited(&lists),
            Queue::Own { queue, fence, submitted } => {
                *submitted += 1;
                // SAFETY: a closed list; the queue and fence are ours.
                unsafe {
                    queue.ExecuteCommandLists(&lists);
                    queue.Signal(&*fence, *submitted)?;
                }
                Ok(*submitted)
            }
        }
    }

    fn completed(&self) -> u64 {
        match self {
            Queue::Shared(host) => host.completed(),
            // SAFETY: a plain query.
            Queue::Own { fence, .. } => unsafe { fence.GetCompletedValue() },
        }
    }

    fn wait(&self, value: u64) -> windows::core::Result<()> {
        match self {
            Queue::Shared(host) => host.wait_for(value),
            // SAFETY: given no event, the call itself waits for the value.
            Queue::Own { fence, .. } => unsafe { fence.SetEventOnCompletion(value, HANDLE::default()) },
        }
    }

    fn frequency(&self) -> windows::core::Result<u64> {
        match self {
            Queue::Shared(host) => host.timestamp_frequency(),
            // SAFETY: a plain query.
            Queue::Own { queue, .. } => unsafe { queue.GetTimestampFrequency() },
        }
    }
}

/// A structure's memory, and what tells one build of it from another.
struct Structure {
    buffer: Buffer,
    /// Which build of this backend made it.
    serial: u64,
    /// A top level: each instance's bottom-level address when built.
    under: Vec<u64>,
}

/// Timestamp pairs a list may use.
const REGIONS: u32 = 256;

/// Timed regions kept until [`Rt::timings`] reads them; beyond that the
/// oldest are dropped and counted.
const TIMINGS_KEPT: usize = 4096;

struct Timer {
    heap: ID3D12QueryHeap,
    readback: Buffer,
    frequency: u64,
    /// The open list's regions.
    regions: Regions,
    /// A submitted list's, by its fence value, until they are read.
    pending: Option<(u64, Regions)>,
    done: Results,
}

/// Compacted sizes a list may ask for.
const SIZE_SLOTS: usize = 4096;

/// Where builds report the size their structure would compact to.
struct Sizes {
    buffer: Buffer,
    readback: Buffer,
    /// The open list's requests, by slot: the structure and its build.
    open: Vec<(Handle, u64)>,
    /// A submitted list's, by its fence value, until they are read.
    pending: Option<(u64, Vec<(Handle, u64)>)>,
    /// The size read for a structure's build: at most one for each
    /// structure that is not given up.
    known: HashMap<Handle, (u64, u64)>,
}

/// Command allocators kept: lists submitted and not yet run each hold one.
const ALLOCATORS: usize = 4;

/// What a build call did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub handle: Handle,
    /// A refit, or a build with its reason ([`Why::NotBuilt`] the first
    /// time).
    pub decision: Decision,
    /// The bytes of the structure's memory and of the scratch this build
    /// used.
    pub result_bytes: u64,
    pub scratch_bytes: u64,
}

/// What [`Rt::compact`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compacted {
    /// Copied into less memory: the bytes before and after.
    Done { before: u64, after: u64 },
    /// The compacted size is no smaller in whole 64 KiB units.
    NotSmaller,
    /// Not built to allow compaction, or compacted already.
    NotAsked,
    /// Its build got no slot to report the size in.
    SizeUnknown,
    /// Refitted since the build that reported the size: what it would
    /// compact to now is not known (`super::update::compactable`). Counted.
    Refitted,
}

/// The memory the backend holds now, bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    /// Structure memory: live, and given up but not yet released.
    pub structures: u64,
    /// Of that, given up and waiting for its last reader.
    pub structures_waiting: u64,
    /// The scratch pool and the instance pool, in use or kept.
    pub scratch: u64,
    pub instances: u64,
}

/// The experimental hardware backend on one device.
pub struct Rt {
    device: ID3D12Device5,
    queue: Queue,
    list: ID3D12GraphicsCommandList4,
    /// Each with the fence value of the submission that last used it (0:
    /// none).
    allocators: Vec<(ID3D12CommandAllocator, u64)>,
    /// The allocator the open list records into.
    current: usize,
    /// The list is open: commands are being recorded.
    open: bool,
    /// The fence value of the last list submitted from here.
    last: u64,
    /// A list was handed to the queue and no fence value could be set for
    /// it: it may run, and nothing says when it has.
    lost: bool,
    /// The structures, and what the open list builds and reads.
    cache: Cache<Structure>,
    scratch: Pool<Buffer>,
    instances: Pool<Buffer>,
    /// Objects the open list names, and those of submitted lists until
    /// they have run.
    kept_open: Vec<windows::core::IUnknown>,
    kept: RetireQueue<Vec<windows::core::IUnknown>>,
    limits: Limits,
    policy: Policy,
    stats: Stats,
    serial: u64,
    timer: Option<Timer>,
    sizes: Option<Sizes>,
    /// The adapter the device is on, looked up when its memory is first
    /// asked for.
    adapter: std::cell::OnceCell<Option<IDXGIAdapter3>>,
}

/// What `device` lacks for the backend, asked of it.
fn lacks_of(device: &ID3D12Device) -> Vec<Lack> {
    let mut caps = crate::caps::HostCaps::default();
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS5::default();
    // SAFETY: the query writes a structure of the size given.
    let asked = unsafe {
        let size = size_of_val(&options) as u32;
        device.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS5, (&raw mut options).cast(), size)
    };
    if asked.is_ok() {
        caps.raytracing_tier = options.RaytracingTier.0 as u32;
    }
    let mut model = D3D12_FEATURE_DATA_SHADER_MODEL { HighestShaderModel: D3D_SHADER_MODEL_6_6 };
    // SAFETY: the query writes a structure of the size given.
    let asked = unsafe {
        device.CheckFeatureSupport(D3D12_FEATURE_SHADER_MODEL, (&raw mut model).cast(), size_of_val(&model) as u32)
    };
    caps.shader_model_6_6 = asked.is_ok() && model.HighestShaderModel.0 >= D3D_SHADER_MODEL_6_6.0;
    caps.raytracing_device = device.cast::<ID3D12Device5>().is_ok();
    select::lacks(&caps)
}

impl Rt {
    /// The backend on the run's device and queue, if a path over host
    /// structures may be selected: `VICTORY_RT_BACKEND` forces one, or asks
    /// for `auto` where a validation record can count (not in a build that
    /// accepts no rule of equivalence, as this one), and the device can
    /// run it (`super::select::Selection::host_paths`). Otherwise an
    /// error, with the line that says why: the caller then uses the
    /// software path.
    pub fn open() -> Result<Rt, Error> {
        let host = crate::host::host().ok_or_else(|| Error::Device("no host device".into()))?;
        let caps = host.caps(false);
        let selection = select::selected(&caps);
        if !selection.host_paths() {
            return Err(Error::NotSelected(selection.describe()));
        }
        Rt::with(&host.device, Queue::Shared(host), Limits::from_env(caps.budget_mb), Policy::from_env())
    }

    /// The backend on `device` with a queue of its own, whatever
    /// `VICTORY_RT_BACKEND` says: for a validation tool or a test that asks
    /// for it by this call. The device's capabilities still decide.
    pub fn open_on(device: &ID3D12Device, limits: Limits, policy: Policy) -> Result<Rt, Error> {
        // SAFETY: plain object creation.
        let (queue, fence) = unsafe {
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .map_err(failed("no command queue"))?;
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(failed("no fence"))?;
            (queue, fence)
        };
        Rt::with(device, Queue::Own { queue, fence, submitted: 0 }, limits, policy)
    }

    fn with(device: &ID3D12Device, queue: Queue, limits: Limits, policy: Policy) -> Result<Rt, Error> {
        let lacks = lacks_of(device);
        if !lacks.is_empty() {
            return Err(Error::Unsupported(lacks));
        }
        let device: ID3D12Device5 = device.cast().map_err(|_| Error::Unsupported(vec![Lack::Device]))?;
        // SAFETY: plain object creation; the list is closed until used.
        let (allocator, list) = unsafe {
            let allocator: ID3D12CommandAllocator = device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .map_err(failed("no command allocator"))?;
            // Asked for by the interface builds are recorded through: a
            // runtime without it fails here.
            let list: ID3D12GraphicsCommandList4 = device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                .map_err(failed("no ID3D12GraphicsCommandList4"))?;
            list.Close().map_err(failed("a new list did not close"))?;
            (allocator, list)
        };
        Ok(Rt {
            device,
            queue,
            list,
            allocators: vec![(allocator, 0)],
            current: 0,
            open: false,
            last: 0,
            lost: false,
            cache: Cache::new(),
            scratch: Pool::new(limits.scratch),
            instances: Pool::new(limits.instances),
            kept_open: Vec::new(),
            kept: RetireQueue::new(),
            limits,
            policy,
            stats: Stats::new(),
            serial: 0,
            timer: None,
            sizes: None,
            adapter: std::cell::OnceCell::new(),
        })
    }

    pub fn device(&self) -> &ID3D12Device5 {
        &self.device
    }

    /// What the backend did and refused so far.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn memory(&self) -> Memory {
        Memory {
            structures: self.cache.bytes(),
            structures_waiting: self.cache.waiting().1,
            scratch: self.scratch.bytes(),
            instances: self.instances.bytes(),
        }
    }

    /// The video memory our process uses and its budget, MB, on the card
    /// and in system memory the GPU reads, as the system's memory manager
    /// sees them for the adapter this backend's device is on: the whole
    /// process's, not this backend's share. `None` where the adapter does
    /// not say. For a validation tool's memory figures.
    pub fn video_memory_mb(&self) -> Option<[(f64, f64); 2]> {
        let adapter = self.adapter.get_or_init(|| crate::host::adapter_of(&self.device));
        crate::host::video_memory_of(adapter.as_ref()?)
    }

    /// Opens the list for recording if it is not open: on an allocator no
    /// submission still uses, waiting for one if all are in use.
    fn begin(&mut self) -> Result<(), Error> {
        if self.open {
            return Ok(());
        }
        let completed = self.queue.completed();
        let at = match self.allocators.iter().position(|(_, busy)| *busy <= completed) {
            Some(at) => at,
            None if self.allocators.len() < ALLOCATORS => {
                // SAFETY: plain object creation.
                let allocator = unsafe { self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }
                    .map_err(failed("no command allocator"))?;
                self.allocators.push((allocator, 0));
                self.allocators.len() - 1
            }
            None => {
                // The one whose submission is the oldest.
                let oldest = self.allocators.iter().enumerate().min_by_key(|(_, (_, busy))| *busy);
                let (at, until) = oldest.map_or((0, 0), |(at, (_, busy))| (at, *busy));
                self.wait(until)?;
                at
            }
        };
        let allocator = &self.allocators[at].0;
        // SAFETY: the allocator's last submission has run, and the list is
        // closed.
        unsafe { allocator.Reset().and_then(|()| self.list.Reset(allocator, None)) }
            .map_err(failed("the list did not reset"))?;
        (self.current, self.open) = (at, true);
        Ok(())
    }

    /// Records commands of the caller's into the list: its root signature,
    /// pipeline, bindings and dispatch.
    pub fn record<R>(&mut self, commands: impl FnOnce(&ID3D12GraphicsCommandList4) -> R) -> Result<R, Error> {
        self.begin()?;
        Ok(commands(&self.list))
    }

    /// Keeps `object` alive until the list being recorded has run: a
    /// pipeline, a root signature, a buffer a build or a dispatch reads.
    pub fn keep(&mut self, object: &impl Interface) {
        if let Ok(object) = object.cast::<windows::core::IUnknown>() {
            self.kept_open.push(object);
        }
    }

    /// Upload memory holding `bytes`: geometry for a build, or anything a
    /// dispatch reads. Readable by builds and shaders as it is. The caller
    /// holds it (or [`Rt::keep`]s it) until every list that names it has
    /// run.
    pub fn upload(&mut self, bytes: &[u8]) -> Result<Buffer, Error> {
        let buffer = make_buffer(&self.device, bytes.len() as u64, Place::Upload, Owner::RtBuffers)
            .map_err(failed("no upload buffer"))?;
        buffer.write(0, bytes);
        Ok(buffer)
    }

    /// Video memory of `bytes` bytes a dispatch writes through an unordered
    /// access view (its address as a root view): a validation run's
    /// results, read with [`Rt::read_back`]. Counted with the backend's
    /// other video memory that is no structure (`Owner::RtScratch`).
    pub fn output(&mut self, bytes: u64) -> Result<Buffer, Error> {
        make_buffer(&self.device, bytes, Place::Unordered, Owner::RtScratch).map_err(failed("no output buffer"))
    }

    /// The structure `key` names for a source at content generation
    /// `generation`: whether it can be read as it is.
    pub fn lookup(&mut self, key: Key, generation: u64) -> Lookup {
        self.collect();
        self.cache.lookup(key, generation)
    }

    pub fn state(&self, handle: Handle) -> Option<State> {
        self.cache.state(handle)
    }

    /// What is known of the structure's last build.
    pub fn built(&self, handle: Handle) -> Option<&Built> {
        self.cache.built(handle)
    }

    /// Counts a refused build under what refused it.
    fn refused(&self, error: Error) -> Error {
        self.stats.one(match &error {
            Error::Invalid(_) => Count::BuildsRefusedInvalid,
            Error::State(_) => Count::BuildsRefusedState,
            Error::Budget(_) => Count::BuildsRefusedBudget,
            Error::Pool(_) => Count::PoolRefusals,
            Error::NotSelected(_) | Error::Unsupported(_) | Error::Device(_) => Count::BuildsRefusedDevice,
        });
        error
    }

    /// Builds the bottom level `id` names to hold `desc`, a source at
    /// content generation `generation` that differs from what was built
    /// last by `change`: a refit in place where `super::update` allows one,
    /// else a build into new memory. Recorded into the list with the
    /// barrier after it (and, for a refit, one before it); the structure
    /// may be read by what is recorded next. The memory `desc` names is the
    /// caller's to keep as the module text says.
    pub fn build_bottom(
        &mut self,
        id: u64,
        generation: u64,
        desc: &BottomDesc,
        change: Change,
    ) -> Result<Outcome, Error> {
        if let Err(invalid) = desc.check() {
            return Err(self.refused(Error::Invalid(invalid)));
        }
        let geometries: Vec<D3D12_RAYTRACING_GEOMETRY_DESC> = desc.geometries.iter().map(geometry).collect();
        let inputs = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS {
            Type: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL,
            Flags: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAGS(desc.flags.bits() as i32),
            NumDescs: geometries.len() as u32,
            DescsLayout: D3D12_ELEMENTS_LAYOUT_ARRAY,
            Anonymous: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS_0 { pGeometryDescs: geometries.as_ptr() },
        };
        let key = Key { level: Level::Bottom, id };
        // `geometries` lives until the build is recorded, below.
        self.build(key, generation, desc.shape(), change, inputs, Vec::new(), Vec::new()).map_err(|e| self.refused(e))
    }

    /// Builds the top level `id` names over `desc`'s instances, as
    /// [`Rt::build_bottom`] builds a bottom level. Each instance's bottom
    /// level must be readable now: built, with its barrier, and not stale.
    /// A refit is made only where every instance refers to the bottom level
    /// and address it did when the top level was last built.
    pub fn build_top(&mut self, id: u64, generation: u64, desc: &TopDesc, change: Change) -> Result<Outcome, Error> {
        if let Err(invalid) = desc.check() {
            return Err(self.refused(Error::Invalid(invalid)));
        }
        let key = Key { level: Level::Top, id };
        let mut under = Vec::with_capacity(desc.instances.len());
        let mut bottoms = Vec::new();
        for instance in &desc.instances {
            under.push(match instance.bottom {
                None => 0,
                Some(bottom) => {
                    if let Err(refusal) = self.cache.buildable_over(bottom) {
                        return Err(self.refused(Error::State(refusal)));
                    }
                    bottoms.push(bottom);
                    self.cache.payload(bottom).map_or(0, |s| s.buffer.address)
                }
            });
        }
        // The descriptions, in memory nothing else uses until this list has
        // run.
        let bytes = (desc.instances.len() * INSTANCE_BYTES) as u64;
        let completed = self.queue.completed();
        let device = &self.device;
        let taken = self.instances.take(bytes, completed, |bytes| {
            make_buffer(device, bytes, Place::Upload, Owner::RtBuffers).ok()
        });
        let address = match taken {
            Ok((buffer, offset)) => {
                for (i, (instance, &bottom)) in desc.instances.iter().zip(&under).enumerate() {
                    buffer.write(offset + (i * INSTANCE_BYTES) as u64, &instance.encode(bottom));
                }
                buffer.address + offset
            }
            Err(refused) => return Err(self.refused(Error::Pool(refused))),
        };
        let inputs = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS {
            Type: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL,
            Flags: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAGS(desc.flags.bits() as i32),
            NumDescs: desc.instances.len() as u32,
            DescsLayout: D3D12_ELEMENTS_LAYOUT_ARRAY,
            Anonymous: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS_0 { InstanceDescs: address },
        };
        self.build(key, generation, desc.shape(), change, inputs, bottoms, under).map_err(|e| self.refused(e))
    }

    /// The part bottom and top levels share: decides refit or rebuild,
    /// sizes the build, takes memory, and records the build and its
    /// barrier. If it fails nothing is recorded and no structure changes
    /// state (pool memory already taken for the list is its until it is
    /// submitted); a structure the call itself made known under `key` is
    /// forgotten again, so that refused builds leave nothing behind.
    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        key: Key,
        generation: u64,
        shape: desc::Shape,
        change: Change,
        inputs: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS,
        bottoms: Vec<Handle>,
        under: Vec<u64>,
    ) -> Result<Outcome, Error> {
        self.collect();
        self.begin()?;
        let known = self.cache.find(key).is_some();
        let handle = self.cache.entry(key);
        let outcome = self.build_into(handle, key.level, generation, shape, change, inputs, bottoms, under);
        if outcome.is_err() && !known {
            // Empty, with no memory and nothing built over it: it goes at
            // once, and its handle was given to nobody.
            self.cache.retire(handle);
        }
        outcome
    }

    /// [`Rt::build`] for the structure `handle`, with the list open. An
    /// error leaves the structure as it was.
    #[allow(clippy::too_many_arguments)]
    fn build_into(
        &mut self,
        handle: Handle,
        level: Level,
        generation: u64,
        shape: desc::Shape,
        change: Change,
        mut inputs: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS,
        bottoms: Vec<Handle>,
        under: Vec<u64>,
    ) -> Result<Outcome, Error> {
        self.cache.can_build(handle).map_err(Error::State)?;
        let before = self.cache.built(handle).cloned();
        let existing =
            self.cache.payload(handle).map(|s| (s.buffer.resource.clone(), s.buffer.address, s.buffer.bytes));
        // A top level whose instances refer elsewhere than when it was
        // built is built again, whatever the caller says changed.
        let moved = self.cache.payload(handle).is_some_and(|s| s.under != under);
        let change = if moved { Change::Other } else { change };

        let mut info = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO::default();
        // SAFETY: `inputs` and what it points to (the caller's geometry
        // descriptions) are alive; the call writes `info`.
        unsafe { self.device.GetRaytracingAccelerationStructurePrebuildInfo(&inputs, &mut info) };
        let sizes = update::Sizes {
            result: info.ResultDataMaxSizeInBytes,
            scratch: info.ScratchDataSizeInBytes,
            update_scratch: info.UpdateScratchDataSizeInBytes,
        };
        let memory = existing.as_ref().map(|&(_, _, bytes)| bytes);
        let (held, limit) = (self.cache.bytes(), self.limits.structures);
        let plan = update::plan(before.as_ref(), memory, shape, change, &self.policy, sizes, held, limit).map_err(
            |unplanned| match unplanned {
                Unplanned::NoSize => Error::Device(
                    "the device sizes the build at 0 bytes: it does not take the description".into(),
                ),
                Unplanned::Budget(over) => Error::Budget(over),
            },
        )?;
        let (decision, scratch_bytes, past_refit_limit) = (plan.decision, plan.scratch_bytes, plan.past_refit_limit);
        let refit = decision == Decision::Refit;
        let new = match plan.memory {
            None => None,
            Some((bytes, counted)) => {
                let buffer = make_buffer(&self.device, bytes, Place::Structure, Owner::RtStructures)
                    .map_err(failed("no memory for the structure"))?;
                Some((buffer, counted))
            }
        };
        let (resource, destination) = match (&new, &existing) {
            (Some((buffer, _)), _) => (buffer.resource.clone(), buffer.address),
            (None, Some((resource, address, _))) => (resource.clone(), *address),
            (None, None) => return Err(Error::State(Refusal::Empty)),
        };
        let completed = self.queue.completed();
        let device = &self.device;
        let (scratch, offset) = self
            .scratch
            .take(scratch_bytes, completed, |bytes| make_buffer(device, bytes, Place::Unordered, Owner::RtScratch).ok())
            .map_err(Error::Pool)?;
        let scratch = scratch.address + offset;

        self.serial += 1;
        let serial = self.serial;
        // The size it would compact to, where the build allows compaction
        // and a slot is free.
        let size = match (refit, plan.built.shape.flags().allow_compaction) {
            (false, true) => {
                let slot = self.size_slot(handle, serial);
                if slot.is_none() {
                    self.stats.one(Count::CompactionsSkipped);
                }
                slot
            }
            _ => None,
        };

        // Allowed, as asked above: from here on the build is recorded.
        self.cache.begin_build(handle, generation).map_err(Error::State)?;
        if refit {
            inputs.Flags.0 |= desc::PERFORM_UPDATE as i32;
        }
        let build = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC {
            DestAccelerationStructureData: destination,
            Inputs: inputs,
            // A refit reads the structure it writes.
            SourceAccelerationStructureData: if refit { destination } else { 0 },
            ScratchAccelerationStructureData: scratch,
        };
        // SAFETY: an open list; the build's memory is ours and lives until
        // the list has run (the cache, the pools); what `inputs` points to
        // is alive for this call, which copies it. The barrier after orders
        // the build before every later read of the structure. A refit
        // writes memory that something earlier in this list may read (a
        // dispatch through a top level, a top-level build over it): the
        // barrier before it orders it after those. A build into new memory
        // has nothing before it to be ordered after.
        unsafe {
            if refit {
                self.list.ResourceBarrier(&[uav_barrier(&resource)]);
            }
            self.list.BuildRaytracingAccelerationStructure(&build, size.as_ref().map(std::slice::from_ref));
            self.list.ResourceBarrier(&[uav_barrier(&resource)]);
        }
        let result_bytes = plan.built.result_bytes;
        let payload = new.map(|(buffer, counted)| (Structure { buffer, serial, under }, counted));
        self.cache.finish_build(handle, plan.built, payload, &bottoms);
        self.cache.barrier(handle);

        match decision {
            Decision::Refit => {
                self.stats.one(Count::Refits);
                if past_refit_limit {
                    self.stats.one(Count::RefitsPastLimit);
                }
            }
            Decision::Rebuild(Why::NotBuilt) => {
                self.stats.one(if level == Level::Bottom { Count::BottomBuilds } else { Count::TopBuilds });
            }
            Decision::Rebuild(why) => {
                self.stats.one(Count::Rebuilds);
                if let Some(count) = Count::rebuild(why) {
                    self.stats.one(count);
                }
            }
        }
        Ok(Outcome { handle, decision, result_bytes, scratch_bytes })
    }

    /// A slot for the build `serial` of `handle` to report its compacted
    /// size in; `None` if there is none (the list's slots are used, the
    /// last list's sizes are not read yet, or the buffers cannot be made).
    fn size_slot(
        &mut self,
        handle: Handle,
        serial: u64,
    ) -> Option<D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC> {
        if self.sizes.is_none() {
            let bytes = (SIZE_SLOTS * 8) as u64;
            let buffer = make_buffer(&self.device, bytes, Place::Unordered, Owner::RtScratch).ok()?;
            let readback = make_buffer(&self.device, bytes, Place::Readback, Owner::RtBuffers).ok()?;
            self.sizes = Some(Sizes { buffer, readback, open: Vec::new(), pending: None, known: HashMap::new() });
        }
        let sizes = self.sizes.as_mut()?;
        if sizes.pending.is_some() || sizes.open.len() >= SIZE_SLOTS {
            return None;
        }
        let slot = sizes.open.len() as u64;
        sizes.open.push((handle, serial));
        Some(D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC {
            DestBuffer: sizes.buffer.address + 8 * slot,
            InfoType: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_COMPACTED_SIZE,
        })
    }

    /// Copies a structure into the memory its build said it compacts to and
    /// gives up the larger one (released once nothing can read it). Only
    /// once the build has run, since the size comes from it, and only while
    /// the structure is as that build left it: one refitted since is not
    /// compacted. Top levels built over a compacted bottom level are stale:
    /// its address changed.
    pub fn compact(&mut self, handle: Handle) -> Result<Compacted, Error> {
        self.collect();
        let Some(built) = self.cache.built(handle).cloned() else {
            return Err(self.refused(Error::State(Refusal::Empty)));
        };
        match update::compactable(&built) {
            Ok(()) => {}
            Err(Uncompacted::NotAsked) => return Ok(Compacted::NotAsked),
            Err(Uncompacted::Refitted) => {
                self.stats.one(Count::CompactionsRefusedRefitted);
                return Ok(Compacted::Refitted);
            }
        }
        match self.cache.state(handle) {
            Some(State::Ready) => {}
            Some(State::Building) => return Err(self.refused(Error::State(Refusal::NotRun))),
            Some(State::Stale) => return Err(self.refused(Error::State(Refusal::Stale))),
            _ => return Err(self.refused(Error::State(Refusal::Unknown))),
        }
        let payload = self.cache.payload(handle).map(|s| (s.buffer.address, s.serial, s.under.clone()));
        let Some((source, serial, under)) = payload else {
            return Err(self.refused(Error::State(Refusal::Empty)));
        };
        let known = self.sizes.as_ref().and_then(|s| s.known.get(&handle)).filter(|(of, _)| *of == serial);
        let Some(&(_, size)) = known else { return Ok(Compacted::SizeUnknown) };
        let after = size.next_multiple_of(GRANULE);
        if size == 0 || after >= built.result_bytes {
            return Ok(Compacted::NotSmaller);
        }
        if let Err(over) = budget::admit(self.cache.bytes(), after, self.limits.structures) {
            return Err(self.refused(Error::Budget(over)));
        }
        self.begin()?;
        let buffer = make_buffer(&self.device, size, Place::Structure, Owner::RtStructures)
            .map_err(failed("no memory for the compacted structure"))
            .map_err(|e| self.refused(e))?;
        let generation = self.cache.generation(handle).unwrap_or(0);
        let bottoms = self.cache.bottoms(handle).to_vec();
        if let Err(refusal) = self.cache.begin_build(handle, generation) {
            return Err(self.refused(Error::State(refusal)));
        }
        // SAFETY: an open list; both structures are ours, the source built
        // and run, and both live until the list has run. The barrier orders
        // the copy before every later read.
        unsafe {
            self.list.CopyRaytracingAccelerationStructure(
                buffer.address,
                source,
                D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT,
            );
            self.list.ResourceBarrier(&[uav_barrier(&buffer.resource)]);
        }
        let before = built.result_bytes;
        let built = Built { compacted: true, result_bytes: after, ..built };
        self.cache.finish_build(handle, built, Some((Structure { buffer, serial, under }, after)), &bottoms);
        self.cache.barrier(handle);
        self.stats.one(Count::Compactions);
        self.stats.add(Count::CompactionBytesSaved, before - after);
        Ok(Compacted::Done { before, after })
    }

    /// Gives a structure up: its memory is released once every list that
    /// may read it has run, and no top level built over it is left.
    pub fn free(&mut self, handle: Handle) -> bool {
        if let Some(sizes) = &mut self.sizes {
            sizes.known.remove(&handle);
        }
        let was = self.cache.retire(handle);
        if was {
            self.stats.one(Count::Retired);
        }
        was
    }

    /// The address to bind a top level by, for the list being recorded,
    /// which holds the structure (and the bottom levels under it) from now
    /// until it has run. Refused, and counted, if the structure may not be
    /// read now: not built, no barrier after its build, stale, or over a
    /// bottom level that is.
    pub fn top_address(&mut self, handle: Handle) -> Result<u64, Error> {
        let readable = match self.cache.level(handle) {
            Some(Level::Top) => self.cache.pin(handle),
            Some(Level::Bottom) => Err(Refusal::Bottom),
            None => Err(Refusal::Unknown),
        };
        let address =
            readable.and_then(|()| self.cache.payload(handle).map(|s| s.buffer.address).ok_or(Refusal::Empty));
        address.map_err(|refusal| {
            self.stats.one(Count::ReadsRefused);
            Error::State(refusal)
        })
    }

    /// Binds a top level as the shader resource view at root parameter
    /// `root` of the compute root signature the caller has set on the list.
    pub fn bind_top(&mut self, root: u32, handle: Handle) -> Result<(), Error> {
        self.begin()?;
        let address = self.top_address(handle)?;
        // SAFETY: an open list with the caller's root signature set; the
        // structure is held until the list has run.
        unsafe { self.list.SetComputeRootShaderResourceView(root, address) };
        Ok(())
    }

    /// Makes the timestamp heap and its read-back buffer, on first use.
    fn make_timer(&self) -> Option<Timer> {
        let frequency = self.queue.frequency().ok()?;
        let mut heap: Option<ID3D12QueryHeap> = None;
        let description =
            D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: 2 * REGIONS, NodeMask: 0 };
        // SAFETY: plain object creation.
        unsafe { self.device.CreateQueryHeap(&description, &mut heap) }.ok()?;
        let readback = make_buffer(&self.device, u64::from(2 * REGIONS) * 8, Place::Readback, Owner::RtBuffers).ok()?;
        let regions = Regions::new(REGIONS);
        Some(Timer { heap: heap?, readback, frequency, regions, pending: None, done: Results::new(TIMINGS_KEPT) })
    }

    /// Begins a timed region at this point of the list; `None`, counted,
    /// where it cannot be timed.
    fn begin_region(&mut self, label: &str) -> Option<Region> {
        self.begin().ok()?;
        if self.timer.is_none() {
            self.timer = self.make_timer();
        }
        // The last timed list's results are read before this one's are
        // resolved over them.
        if let Some(fence) = self.timer.as_ref().and_then(|t| t.pending.as_ref()).map(|(fence, _)| *fence) {
            let _ = self.wait(fence);
        }
        let began = self.timer.as_mut().filter(|t| t.pending.is_none()).and_then(|timer| {
            let (region, query) = timer.regions.begin(label)?;
            // SAFETY: an open list and a query of our heap.
            unsafe { self.list.EndQuery(&timer.heap, D3D12_QUERY_TYPE_TIMESTAMP, query) };
            Some(region)
        });
        if began.is_none() {
            self.stats.one(Count::RegionsUntimed);
        }
        began
    }

    fn end_region(&mut self, region: Region) {
        if let Some(timer) = &mut self.timer
            && let Some(query) = timer.regions.end(region)
        {
            // SAFETY: an open list and a query of our heap.
            unsafe { self.list.EndQuery(&timer.heap, D3D12_QUERY_TYPE_TIMESTAMP, query) };
        }
    }

    /// Records what `commands` records as a region whose GPU time is
    /// measured: builds made through `self`, or a dispatch recorded with
    /// [`Rt::record`]. The time is in [`Rt::timings`] once the list has
    /// run; `super::timing` says what it is and is not. A region lives in
    /// one list: if `commands` submits (`submit`, `flush`, `read_back`),
    /// the region is cut there, has no time, and is counted.
    pub fn timed<R>(&mut self, label: &str, commands: impl FnOnce(&mut Rt) -> R) -> R {
        let region = self.begin_region(label);
        let result = commands(self);
        if let Some(region) = region {
            self.end_region(region);
        }
        result
    }

    /// The timed regions of lists that have run, since the last call:
    /// the latest [`TIMINGS_KEPT`] of them, older ones having been dropped
    /// and counted.
    pub fn timings(&mut self) -> Vec<Timing> {
        self.collect();
        self.timer.as_mut().map(|t| t.done.take()).unwrap_or_default()
    }

    /// Closes the list and hands it to the queue: the fence value that is
    /// complete once it has run. Does not wait. With nothing recorded, the
    /// value of the last list submitted.
    pub fn submit(&mut self) -> Result<u64, Error> {
        if !self.open {
            return Ok(self.last);
        }
        // Timestamps out of the heap, and compacted sizes where the CPU
        // reads them.
        let timed = match &mut self.timer {
            Some(timer) if !timer.regions.is_empty() => {
                // A region still open ends here, cut: its end is recorded
                // so that every query resolved below was written.
                let cut = timer.regions.cut_open();
                for &query in &cut {
                    // SAFETY: an open list and a query of our heap.
                    unsafe { self.list.EndQuery(&timer.heap, D3D12_QUERY_TYPE_TIMESTAMP, query) };
                }
                self.stats.add(Count::RegionsCut, cut.len() as u64);
                // SAFETY: an open list; every query resolved was recorded
                // in it, and the buffer holds them all.
                unsafe {
                    self.list.ResolveQueryData(
                        &timer.heap,
                        D3D12_QUERY_TYPE_TIMESTAMP,
                        0,
                        timer.regions.queries(),
                        &timer.readback.resource,
                        0,
                    );
                }
                true
            }
            _ => false,
        };
        let sized = match &self.sizes {
            Some(sizes) if !sizes.open.is_empty() => {
                let (from, to) = (&sizes.buffer, &sizes.readback);
                let bytes = sizes.open.len() as u64 * 8;
                // SAFETY: an open list and two buffers of ours, the source
                // moved to the copy-source state for the copy and back.
                unsafe {
                    self.list.ResourceBarrier(&[crate::host::transition(
                        &from.resource,
                        from.state,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                    )]);
                    self.list.CopyBufferRegion(&to.resource, 0, &from.resource, 0, bytes);
                    self.list.ResourceBarrier(&[crate::host::transition(
                        &from.resource,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        from.state,
                    )]);
                }
                true
            }
            _ => false,
        };
        self.open = false;
        // SAFETY: our open list.
        if let Err(e) = unsafe { self.list.Close() } {
            // The list never runs: nothing it recorded happened.
            self.cache.abandon();
            self.scratch.abandoned();
            self.instances.abandoned();
            self.kept_open.clear();
            if let Some(timer) = &mut self.timer {
                drop(timer.regions.next_list());
            }
            if let Some(sizes) = &mut self.sizes {
                sizes.open.clear();
            }
            return Err(failed("the list did not close: a command recorded into it was invalid")(e));
        }
        let (fence, failure) = match self.queue.submit(&self.list) {
            Ok(fence) => (fence, None),
            // The queue may have taken the list without a fence value being
            // set for it. What it uses then waits for a value that no
            // submission has: it is never released, which is the only safe
            // thing to do with memory a list may be running on.
            Err(e) => {
                self.lost = true;
                (u64::MAX, Some(failed("the list was handed to the queue without a fence value")(e)))
            }
        };
        self.allocators[self.current].1 = fence;
        self.cache.seal(fence);
        self.scratch.submitted(fence);
        self.instances.submitted(fence);
        self.kept.retire(std::mem::take(&mut self.kept_open), fence, 0);
        if timed && let Some(timer) = &mut self.timer {
            timer.pending = Some((fence, timer.regions.next_list()));
        }
        if sized && let Some(sizes) = &mut self.sizes {
            sizes.pending = Some((fence, std::mem::take(&mut sizes.open)));
        }
        if let Some(failure) = failure {
            return Err(failure);
        }
        self.last = fence;
        self.stats.one(Count::Submissions);
        Ok(fence)
    }

    /// Waits until the submission with fence value `fence` has run, then
    /// does what [`Rt::collect`] does. The time spent waiting: the cost of
    /// synchronising with the GPU here, counted apart from GPU time.
    pub fn wait(&mut self, fence: u64) -> Result<Duration, Error> {
        if fence == u64::MAX {
            // The value of a list that got none (see `submit`): no
            // submission completes it.
            return Err(Error::Device("a wait for a list that was submitted without a fence value".into()));
        }
        let started = Instant::now();
        self.queue.wait(fence).map_err(failed("the wait for a submission failed"))?;
        let waited = started.elapsed();
        self.stats.one(Count::Waits);
        self.stats.add(Count::WaitMicros, waited.as_micros() as u64);
        self.collect();
        Ok(waited)
    }

    /// [`Rt::submit`], then [`Rt::wait`] for it.
    pub fn flush(&mut self) -> Result<Duration, Error> {
        let fence = self.submit()?;
        self.wait(fence)
    }

    /// Takes note of the submissions that have run, without waiting:
    /// builds among them are done, memory nothing can read any more is
    /// released, idle pool memory beyond what is kept is let go, and the
    /// timestamps and compacted sizes they wrote are read.
    pub fn collect(&mut self) {
        let completed = self.queue.completed();
        let released = self.cache.complete(completed);
        self.stats.add(Count::Released, released.len() as u64);
        drop(released);
        drop(self.kept.release(completed));
        self.scratch.trim(completed);
        self.instances.trim(completed);
        if let Some(timer) = &mut self.timer
            && let Some((_, regions)) = timer.pending.take_if(|(fence, _)| *fence <= completed)
        {
            let queries = regions.queries() as usize;
            let stamps: Vec<u64> = read(&timer.readback.resource, 0, queries * 8)
                .map(|bytes| bytes.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect())
                .unwrap_or_default();
            let dropped = timer.done.add(regions.resolve(&stamps, timer.frequency));
            self.stats.add(Count::TimingsDropped, dropped as u64);
        }
        if let Some(sizes) = &mut self.sizes
            && let Some((_, asked)) = sizes.pending.take_if(|(fence, _)| *fence <= completed)
            && let Ok(bytes) = read(&sizes.readback.resource, 0, asked.len() * 8)
        {
            for ((handle, serial), size) in asked.into_iter().zip(bytes.as_chunks::<8>().0) {
                // A structure given up since its build has no use for the
                // size, and nothing would take it out again.
                if self.cache.state(handle).is_some_and(|state| state != State::Retired) {
                    sizes.known.insert(handle, (serial, u64::from_le_bytes(*size)));
                }
            }
        }
    }

    /// The `bytes` at `offset` of `source`, as they are once everything
    /// recorded so far has run: records the copy, submits, waits and reads.
    /// `state` is the state `source` is in, and is left in. With the time
    /// spent waiting.
    pub fn read_back_resource(
        &mut self,
        source: &ID3D12Resource,
        state: D3D12_RESOURCE_STATES,
        offset: u64,
        bytes: u64,
    ) -> Result<(Vec<u8>, Duration), Error> {
        if bytes == 0 {
            return Ok((Vec::new(), Duration::ZERO));
        }
        self.begin()?;
        let readback = make_buffer(&self.device, bytes, Place::Readback, Owner::RtBuffers)
            .map_err(failed("no read-back buffer"))?;
        let moves = !state.contains(D3D12_RESOURCE_STATE_COPY_SOURCE);
        // SAFETY: an open list; the source is the caller's live buffer of
        // at least `offset + bytes` bytes in `state`, moved to the
        // copy-source state for the copy and back; the destination is ours.
        unsafe {
            if moves {
                self.list.ResourceBarrier(&[crate::host::transition(source, state, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            }
            self.list.CopyBufferRegion(&readback.resource, 0, source, offset, bytes);
            if moves {
                self.list.ResourceBarrier(&[crate::host::transition(source, D3D12_RESOURCE_STATE_COPY_SOURCE, state)]);
            }
        }
        self.keep(source);
        let waited = self.flush()?;
        let data = read(&readback.resource, 0, bytes as usize).map_err(failed("the read-back buffer did not map"))?;
        Ok((data, waited))
    }

    /// [`Rt::read_back_resource`] of a buffer of this backend's.
    pub fn read_back(&mut self, buffer: &Buffer, offset: u64, bytes: u64) -> Result<(Vec<u8>, Duration), Error> {
        if offset.checked_add(bytes).is_none_or(|end| end > buffer.bytes) {
            let have = buffer.bytes;
            return Err(Error::Device(format!("a read-back of {bytes} bytes at {offset} of a buffer of {have}")));
        }
        let (resource, state) = (buffer.resource.clone(), buffer.state);
        self.read_back_resource(&resource, state, offset, bytes)
    }
}

impl Drop for Rt {
    /// Nothing of ours is released while a list of ours may still run.
    fn drop(&mut self) {
        if self.last > self.queue.completed() {
            let _ = self.queue.wait(self.last);
        }
        // After a list with no fence value (see `submit`), what lists may
        // use is leaked instead: there is nothing to wait for.
        if self.lost && self.queue.completed() != u64::MAX {
            std::mem::forget(std::mem::take(&mut self.cache));
            std::mem::forget(std::mem::replace(&mut self.scratch, Pool::new(self.limits.scratch)));
            std::mem::forget(std::mem::replace(&mut self.instances, Pool::new(self.limits.instances)));
            std::mem::forget(std::mem::take(&mut self.kept));
            std::mem::forget((self.timer.take(), self.sizes.take()));
        }
    }
}

/// A checked geometry as D3D12's description of it.
fn geometry(geometry: &Geometry) -> D3D12_RAYTRACING_GEOMETRY_DESC {
    match *geometry {
        Geometry::Triangles(t) => D3D12_RAYTRACING_GEOMETRY_DESC {
            Type: D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES,
            Flags: D3D12_RAYTRACING_GEOMETRY_FLAGS(t.flags.bits() as i32),
            Anonymous: D3D12_RAYTRACING_GEOMETRY_DESC_0 {
                Triangles: D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC {
                    Transform3x4: t.transform.unwrap_or(0),
                    IndexFormat: DXGI_FORMAT(t.indices.map_or(0, |i| i.format.dxgi()) as i32),
                    VertexFormat: DXGI_FORMAT(t.vertex_format.dxgi() as i32),
                    IndexCount: t.indices.map_or(0, |i| i.count),
                    VertexCount: t.vertex_count,
                    IndexBuffer: t.indices.map_or(0, |i| i.address),
                    VertexBuffer: D3D12_GPU_VIRTUAL_ADDRESS_AND_STRIDE {
                        StartAddress: t.vertices,
                        StrideInBytes: t.vertex_stride,
                    },
                },
            },
        },
        Geometry::Boxes(b) => D3D12_RAYTRACING_GEOMETRY_DESC {
            Type: D3D12_RAYTRACING_GEOMETRY_TYPE_PROCEDURAL_PRIMITIVE_AABBS,
            Flags: D3D12_RAYTRACING_GEOMETRY_FLAGS(b.flags.bits() as i32),
            Anonymous: D3D12_RAYTRACING_GEOMETRY_DESC_0 {
                AABBs: D3D12_RAYTRACING_GEOMETRY_AABBS_DESC {
                    AABBCount: b.count,
                    AABBs: D3D12_GPU_VIRTUAL_ADDRESS_AND_STRIDE { StartAddress: b.address, StrideInBytes: b.stride },
                },
            },
        },
    }
}

/// Our process's private commit and working set now, bytes.
pub fn commit_and_working_set() -> (u64, u64) {
    crate::host::commit_and_working_set()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt::desc::{BuildFlags, GeometryFlags, IDENTITY, Inputs, Instance, Triangles, VertexFormat};

    /// The positions of `count` separate triangles in the plane z = 0, in
    /// rows two units apart, the whole moved along x by `shift`.
    fn triangles(count: u32, shift: f32) -> Vec<u8> {
        let side = (f64::from(count).sqrt().ceil() as u32).max(1);
        (0..count)
            .flat_map(|i| {
                let (x, y) = (shift + 2.0 * (i % side) as f32, 2.0 * (i / side) as f32);
                [x, y, 0.0, x + 1.0, y, 0.0, x, y + 1.0, 0.0]
            })
            .flat_map(f32::to_le_bytes)
            .collect()
    }

    /// A bottom level of the `count` triangles at `vertices`.
    fn bottom_of(vertices: &Buffer, count: u32, flags: BuildFlags) -> BottomDesc {
        BottomDesc {
            geometries: vec![Geometry::Triangles(Triangles {
                vertices: vertices.address(),
                vertex_stride: 12,
                vertex_count: 3 * count,
                vertex_format: VertexFormat::Float3,
                indices: None,
                transform: None,
                flags: GeometryFlags { opaque: true, ..Default::default() },
            })],
            flags,
            inputs: Inputs::Ready,
        }
    }

    /// The triangles of the bottom level that is compacted: enough that its
    /// memory is many 64 KiB units, so that a device that compacts at all
    /// saves whole units (one triangle's structure is under one unit before
    /// and after, and would never be copied).
    const MANY: u32 = 20_000;

    /// The whole path on the run's device, where it has what the backend
    /// needs: builds, a refit (with its barrier before), the rebuild the
    /// policy forces, a top level made stale and built again, a compaction
    /// refused after a refit, compaction with the top level built again
    /// over the copy, release. Whether the compacting copy ran depends on
    /// the device reporting a smaller size: the test says loudly when it
    /// did not. No ray is traced (that needs a shader, and is the
    /// validation tool's): this shows only that the device takes every
    /// call and the bookkeeping agrees with it.
    /// **Never run so far**: no machine it was built on had a GPU.
    #[test]
    fn structures_are_built_refitted_compacted_and_released_on_the_host() {
        let Some(host) = crate::host::host() else {
            eprintln!("no host D3D12 device; the ray-tracing structure test is skipped");
            return;
        };
        let limits = Limits::for_device(None, None).0;
        let policy = Policy { rebuild_after_refits: 1 };
        let mut rt = match Rt::with(&host.device, Queue::Shared(host), limits, policy) {
            Ok(rt) => rt,
            Err(Error::Unsupported(lacks)) => {
                eprintln!("the host device cannot run the hardware backend ({lacks:?})");
                eprintln!("the ray-tracing structure test is skipped");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let updatable = BuildFlags { allow_update: true, allow_compaction: true, ..Default::default() };
        let vertices = rt.upload(&triangles(1, 0.0)).unwrap();
        let fixed = rt.upload(&triangles(MANY, 5.0)).unwrap();

        // A bottom level that moves, a large one that does not and is
        // compacted, and a top level over both.
        let first = bottom_of(&vertices, 1, updatable);
        let moving = rt.timed("builds", |rt| rt.build_bottom(1, 1, &first, Change::Other)).unwrap();
        assert_eq!(moving.decision, Decision::Rebuild(Why::NotBuilt));
        let compactable = BuildFlags { allow_compaction: true, ..Default::default() };
        let still = rt.build_bottom(2, 1, &bottom_of(&fixed, MANY, compactable), Change::Other).unwrap();
        let instance =
            |bottom, id| Instance { transform: IDENTITY, id, mask: 0xff, hit_group: 0, flags: 0, bottom: Some(bottom) };
        let top_desc = |a, b| TopDesc {
            instances: vec![instance(a, 0), instance(b, 1)],
            flags: BuildFlags { allow_update: true, ..Default::default() },
            inputs: Inputs::Ready,
        };
        let top = rt.build_top(9, 1, &top_desc(moving.handle, still.handle), Change::Other).unwrap();
        assert!(rt.top_address(top.handle).is_ok(), "readable after its barrier, in the list that builds it");
        let waited = rt.flush().unwrap();
        eprintln!("first builds: waited {waited:?}; {}", rt.stats().line());
        assert_eq!(rt.state(top.handle), Some(State::Ready));
        let timings = rt.timings();
        assert!(timings.iter().any(|t| t.label == "builds" && t.gpu_ms.is_some() && !t.cut), "{timings:?}");

        // The moving one's positions change: a refit, which makes the top
        // level stale; then the top level's own refit. Once refitted it is
        // not compacted from the size its build reported.
        let moved = rt.upload(&triangles(1, 0.5)).unwrap();
        assert_eq!(rt.lookup(Key { level: Level::Bottom, id: 1 }, 2), Lookup::NeedsBuild(moving.handle));
        let refit = rt.build_bottom(1, 2, &bottom_of(&moved, 1, updatable), Change::Positions).unwrap();
        assert_eq!(refit.decision, Decision::Refit);
        assert_eq!(rt.top_address(top.handle), Err(Error::State(Refusal::Stale)));
        let again = rt.build_top(9, 2, &top_desc(moving.handle, still.handle), Change::Positions).unwrap();
        assert_eq!(again.decision, Decision::Refit);
        rt.flush().unwrap();
        assert_eq!(rt.compact(moving.handle).unwrap(), Compacted::Refitted);
        assert_eq!(rt.stats().get(Count::CompactionsRefusedRefitted), 1);

        // A second change passes the policy's one refit: a rebuild, into
        // new memory, so the top level is built again too.
        let rebuilt = rt.build_bottom(1, 3, &bottom_of(&vertices, 1, updatable), Change::Positions).unwrap();
        assert_eq!(rebuilt.decision, Decision::Rebuild(Why::RefitLimit));
        let over = rt.build_top(9, 3, &top_desc(moving.handle, still.handle), Change::Positions).unwrap();
        assert_eq!(over.decision, Decision::Rebuild(Why::ContentsChanged), "its instance's address moved");
        rt.flush().unwrap();

        // Compaction of the large one, whose size its build reported, and
        // the top level built again over the copy in the same list.
        match rt.compact(still.handle).unwrap() {
            Compacted::Done { before, after } => {
                assert!(after < before && before == still.result_bytes, "{before} to {after}");
                eprintln!("compacted {MANY} triangles from {before} to {after} bytes");
                assert_eq!(rt.stats().get(Count::Compactions), 1);
                assert_eq!(rt.built(still.handle).map(|b| b.compacted), Some(true));
                assert_eq!(
                    rt.top_address(top.handle),
                    Err(Error::State(Refusal::Stale)),
                    "the top level holds the address the bottom level had"
                );
                let over = rt.build_top(9, 4, &top_desc(moving.handle, still.handle), Change::Positions).unwrap();
                assert_eq!(over.decision, Decision::Rebuild(Why::ContentsChanged), "its instance's address moved");
                assert!(rt.top_address(top.handle).is_ok(), "over the copy, after the copy's barrier");
                rt.flush().unwrap();
                assert_eq!(rt.state(top.handle), Some(State::Ready));
                assert_eq!(rt.compact(still.handle).unwrap(), Compacted::NotAsked, "compacted already");
            }
            // A device may report a compacted size no smaller than the
            // structure. Then nothing below was exercised, and a pass of
            // this test says nothing of the compacting copy.
            Compacted::NotSmaller => {
                let bytes = still.result_bytes;
                eprintln!("COMPACTION NOT EXERCISED: the device reports no smaller size than {bytes} bytes");
                eprintln!("COMPACTION NOT EXERCISED: the copy, its barrier and the top level over it did not run");
            }
            other => panic!("the large bottom level was not compacted: {other:?}"),
        }
        rt.flush().unwrap();

        // Everything given up is released once its last list has run.
        for handle in [top.handle, moving.handle, still.handle] {
            assert!(rt.free(handle));
        }
        rt.flush().unwrap();
        assert_eq!(rt.memory().structures, 0, "{}", rt.stats().line());
        eprintln!("ray-tracing structures on the host: {}", rt.stats().line());
        drop((vertices, fixed, moved));
    }
}
