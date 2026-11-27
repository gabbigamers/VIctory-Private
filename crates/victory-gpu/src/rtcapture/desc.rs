//! The guest-memory form of a D3D12 acceleration-structure build
//! description, read into an owned one.
//!
//! The layouts are the public x64 ones of `d3d12.h` (field order as
//! declared there; sizes follow from 8-byte pointers and addresses and
//! 4-byte enums, flags and counts). A build for Windows checks every size,
//! offset and value used here against the declarations in the host's
//! bindings (the `windows` crate), at compile time: see the end of this
//! file's code.
//!
//! ```text
//! D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC, 0x30 bytes
//!   0x00 u64 DestAccelerationStructureData
//!   0x08     Inputs (D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS, 0x18)
//!   0x08 u32   Type         0 top level, 1 bottom level
//!   0x0c u32   Flags
//!   0x10 u32   NumDescs
//!   0x14 u32   DescsLayout  0 array, 1 array of pointers
//!   0x18 u64   InstanceDescs (a GPU address) | pGeometryDescs | ppGeometryDescs
//!   0x20 u64 SourceAccelerationStructureData
//!   0x28 u64 ScratchAccelerationStructureData
//!
//! D3D12_RAYTRACING_GEOMETRY_DESC, 0x38 bytes
//!   0x00 u32 Type   0 triangles, 1 procedural boxes
//!   0x04 u32 Flags  1 opaque, 2 no duplicate any-hit invocation
//!   triangles (D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC):
//!   0x08 u64 Transform3x4 (a GPU address, or null)
//!   0x10 u32 IndexFormat   0 none, 42 R32_UINT, 57 R16_UINT
//!   0x14 u32 VertexFormat  a DXGI_FORMAT
//!   0x18 u32 IndexCount
//!   0x1c u32 VertexCount
//!   0x20 u64 IndexBuffer
//!   0x28 u64 VertexBuffer.StartAddress
//!   0x30 u64 VertexBuffer.StrideInBytes
//!   boxes (D3D12_RAYTRACING_GEOMETRY_AABBS_DESC):
//!   0x08 u64 AABBCount
//!   0x10 u64 AABBs.StartAddress
//!   0x18 u64 AABBs.StrideInBytes
//!
//! D3D12_RAYTRACING_INSTANCE_DESC, 0x40 bytes
//!   0x00 f32[3][4] Transform, rows
//!   0x30 u32 InstanceID in bits 0-23, InstanceMask in bits 24-31
//!   0x34 u32 InstanceContributionToHitGroupIndex in bits 0-23, Flags in 24-31
//!   0x38 u64 AccelerationStructure (the bottom level's GPU address)
//!
//! D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC, 0x10 bytes
//!   0x00 u64 DestBuffer
//!   0x08 u32 InfoType
//! ```
//!
//! What agrees with the console so far: a disassembly of the builder DLL
//! read by another project gives the description 0x30 bytes and its flags
//! at 0x0c (xbox-hle-lab, docs/GHIDRA-BVH.md, sections 2.4 and 4). The
//! geometry and instance layouts are PC's and unverified for the console
//! until a title run: so every enumeration and flag word is checked, and a
//! value PC does not have refuses the description ([`Refusal`]) instead of
//! being read as if it had PC's meaning.
//!
//! Reading takes a callback, so that tests pass synthetic bytes; it reads
//! the description, the arrays the description names, and nothing else.
//! Index, vertex, box and transform data are named by address and not read.

/// Reads guest memory at an address into the slice; false if any of it is
/// not readable (what was written to the slice is then not used).
pub type Read<'a> = dyn FnMut(u64, &mut [u8]) -> bool + 'a;

pub const DESC_BYTES: u64 = 0x30;
pub const INPUTS_OFFSET: u64 = 0x08;
pub const GEOMETRY_BYTES: u64 = 0x38;
pub const INSTANCE_BYTES: u64 = 0x40;
pub const POSTBUILD_BYTES: u64 = 0x10;

/// `D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAGS`.
pub mod build_flags {
    pub const ALLOW_UPDATE: u32 = 0x01;
    pub const ALLOW_COMPACTION: u32 = 0x02;
    pub const PREFER_FAST_TRACE: u32 = 0x04;
    pub const PREFER_FAST_BUILD: u32 = 0x08;
    pub const MINIMIZE_MEMORY: u32 = 0x10;
    pub const PERFORM_UPDATE: u32 = 0x20;
    /// Every flag PC has.
    pub const PUBLIC: u32 = 0x3f;
    /// The console's flag for a structure kept packed ("dehydrated"). Its
    /// value and name are from xbox-hle-lab's reading of the builder's
    /// validator (docs/GHIDRA-BVH.md, section 2.4), not from a header we
    /// have. A description with it is recorded and marked.
    pub const CONSOLE_DEHYDRATE: u32 = 0x0100_0000;
    /// What a description may carry and still be read.
    pub const KNOWN: u32 = PUBLIC | CONSOLE_DEHYDRATE;
}

/// `D3D12_RAYTRACING_GEOMETRY_FLAGS`.
pub mod geometry_flags {
    pub const OPAQUE: u32 = 1;
    pub const NO_DUPLICATE_ANYHIT_INVOCATION: u32 = 2;
    pub const KNOWN: u32 = 3;
}

/// `D3D12_RAYTRACING_INSTANCE_FLAGS`, the eight bits an instance has.
pub mod instance_flags {
    pub const TRIANGLE_CULL_DISABLE: u8 = 1;
    pub const TRIANGLE_FRONT_COUNTERCLOCKWISE: u8 = 2;
    pub const FORCE_OPAQUE: u8 = 4;
    pub const FORCE_NON_OPAQUE: u8 = 8;
    /// DXR 1.0 and 1.1; anything else is counted where instances are read.
    pub const KNOWN: u8 = 0x0f;
}

/// How an array of descriptions is laid out (`D3D12_ELEMENTS_LAYOUT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Array,
    ArrayOfPointers,
}

/// `DXGI_FORMAT` of an index buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFormat {
    /// `DXGI_FORMAT_UNKNOWN`: the geometry is not indexed.
    None,
    /// `DXGI_FORMAT_R16_UINT` (57).
    U16,
    /// `DXGI_FORMAT_R32_UINT` (42).
    U32,
}

/// The vertex formats DXR 1.1 takes for triangle positions, by
/// `DXGI_FORMAT` number. A description with another format is still
/// recorded (the layout does not depend on it); a later component has to
/// leave such a geometry to the software path.
pub const fn dxr_vertex_format(format: u32) -> bool {
    matches!(
        format,
        6 // R32G32B32_FLOAT
        | 16 // R32G32_FLOAT
        | 10 // R16G16B16A16_FLOAT
        | 34 // R16G16_FLOAT
        | 13 // R16G16B16A16_SNORM
        | 37 // R16G16_SNORM
        | 11 // R16G16B16A16_UNORM
        | 35 // R16G16_UNORM
        | 24 // R10G10B10A2_UNORM
        | 28 // R8G8B8A8_UNORM
        | 49 // R8G8_UNORM
        | 31 // R8G8B8A8_SNORM
        | 51 // R8G8_SNORM
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Triangles {
    /// GPU address of a 3x4 transform applied at build, or null.
    pub transform: u64,
    pub index_format: IndexFormat,
    /// A `DXGI_FORMAT`, as the title gave it ([`dxr_vertex_format`]).
    pub vertex_format: u32,
    pub index_count: u32,
    pub vertex_count: u32,
    pub index_buffer: u64,
    pub vertex_buffer: u64,
    pub vertex_stride: u64,
}

/// Procedural primitives: axis-aligned boxes of six floats each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boxes {
    pub count: u64,
    pub address: u64,
    pub stride: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryKind {
    Triangles(Triangles),
    Boxes(Boxes),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// [`geometry_flags`].
    pub flags: u32,
    pub kind: GeometryKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BottomLevel {
    pub layout: Layout,
    /// Where the title's geometry descriptions were (CPU memory, read
    /// during the call, as D3D12 does).
    pub address: u64,
    pub geometries: Vec<Geometry>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    /// Rows of the 3x4 object-to-world transform.
    pub transform: [f32; 12],
    /// 24 bits.
    pub id: u32,
    pub mask: u8,
    /// 24 bits.
    pub hit_group_offset: u32,
    /// [`instance_flags`].
    pub flags: u8,
    /// The bottom level's GPU address; null for an instance left out.
    pub bottom_level: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopLevel {
    pub layout: Layout,
    /// The GPU address of the instance descriptions, or of the pointers to
    /// them.
    pub address: u64,
    /// The instance descriptions as they were when the build was recorded.
    /// D3D12 reads them when the list runs, so a title may still change
    /// them in between: whoever builds from this reads them again then.
    pub instances: Vec<Instance>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Level {
    Top(TopLevel),
    Bottom(BottomLevel),
}

/// `D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Postbuild {
    /// Where the builder writes the information.
    pub destination: u64,
    /// 0 compacted size, 1 tools visualisation, 2 serialisation, 3 current
    /// size; as given.
    pub kind: u32,
}

/// One build, as the title described it.
#[derive(Clone, Debug, PartialEq)]
pub struct Description {
    pub destination: u64,
    /// The structure an update starts from; null otherwise.
    pub source: u64,
    pub scratch: u64,
    /// [`build_flags`], the console's included.
    pub flags: u32,
    pub level: Level,
}

impl Description {
    pub fn is_update(&self) -> bool {
        self.flags & build_flags::PERFORM_UPDATE != 0
    }

    pub fn is_top(&self) -> bool {
        matches!(self.level, Level::Top(_))
    }

    /// The heap bytes this holds, for the registry's bound.
    pub fn heap_bytes(&self) -> u64 {
        (match &self.level {
            Level::Top(top) => top.instances.capacity() * size_of::<Instance>(),
            Level::Bottom(bottom) => bottom.geometries.capacity() * size_of::<Geometry>(),
        }) as u64
    }
}

/// The start of the inputs, which the size query takes without the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputsHead {
    pub top: bool,
    pub flags: u32,
    pub count: u32,
    pub layout: Layout,
    pub address: u64,
}

/// The most elements one description may name. D3D12 allows 2^24 of each;
/// these keep what one call reads and holds bounded: 18 MiB read and 18 MiB
/// made of it for the largest top level, under 9 MiB for the largest
/// bottom level. More is refused and counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub instances: u32,
    pub geometries: u32,
    pub postbuild: u32,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        instances: 1 << 18,
        geometries: 1 << 16,
        postbuild: 16,
    };
}

/// Which array of a description a refusal is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum What {
    Description,
    Geometries,
    GeometryPointers,
    Instances,
    InstancePointers,
    Postbuild,
}

impl What {
    pub const fn name(self) -> &'static str {
        match self {
            What::Description => "description",
            What::Geometries => "geometry descriptions",
            What::GeometryPointers => "geometry description pointers",
            What::Instances => "instance descriptions",
            What::InstancePointers => "instance description pointers",
            What::Postbuild => "post-build descriptions",
        }
    }
}

/// Why a description was not read. Nothing of a refused description is
/// kept: the existing path builds it as before, and the registry does not
/// know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refusal {
    /// The description itself is at null.
    NullDescription,
    /// No destination: not a build.
    NullDestination,
    /// Elements are counted and their array is at null.
    NullArray(What),
    /// Guest memory the description names could not be read.
    Unreadable(What),
    /// An address plus a length passes the end of the address space.
    Overflow(What),
    /// More elements than [`Limits`] allows.
    TooMany(What, u32),
    /// A level PC does not have.
    Type(u32),
    /// Build flags outside [`build_flags::KNOWN`]: the bits not known.
    Flags(u32),
    /// A layout PC does not have.
    Layout(u32),
    /// A geometry of a kind PC does not have.
    GeometryType(u32),
    /// Geometry flags outside [`geometry_flags::KNOWN`]: the bits not known.
    GeometryFlags(u32),
    /// An index format that is not none, 16 or 32 bits.
    IndexFormat(u32),
}

impl Refusal {
    /// The value that was not understood, where there is one: a log line
    /// is written once for each refusal with its value.
    pub const fn value(self) -> Option<u32> {
        match self {
            Refusal::TooMany(_, v)
            | Refusal::Type(v)
            | Refusal::Flags(v)
            | Refusal::Layout(v)
            | Refusal::GeometryType(v)
            | Refusal::GeometryFlags(v)
            | Refusal::IndexFormat(v) => Some(v),
            _ => None,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Refusal::NullDescription => f.write_str("no description"),
            Refusal::NullDestination => f.write_str("no destination"),
            Refusal::NullArray(what) => write!(f, "{} counted but at null", what.name()),
            Refusal::Unreadable(what) => write!(f, "{} not readable", what.name()),
            Refusal::Overflow(what) => write!(f, "{} pass the end of memory", what.name()),
            Refusal::TooMany(what, count) => write!(f, "{count} {}: over the limit", what.name()),
            Refusal::Type(v) => write!(f, "unknown structure type {v}"),
            Refusal::Flags(v) => write!(f, "unknown build flags {v:#x}"),
            Refusal::Layout(v) => write!(f, "unknown elements layout {v}"),
            Refusal::GeometryType(v) => write!(f, "unknown geometry type {v}"),
            Refusal::GeometryFlags(v) => write!(f, "unknown geometry flags {v:#x}"),
            Refusal::IndexFormat(v) => write!(f, "unknown index format {v}"),
        }
    }
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32_at(bytes, at)) | u64::from(u32_at(bytes, at + 4)) << 32
}

/// Reads `count` elements of `size` bytes at `address`, whole.
fn read_array(
    read: &mut Read<'_>,
    address: u64,
    count: u32,
    size: u64,
    what: What,
) -> Result<Vec<u8>, Refusal> {
    let bytes = u64::from(count) * size;
    if address.checked_add(bytes).is_none() {
        return Err(Refusal::Overflow(what));
    }
    let mut out = vec![0; bytes as usize];
    if !read(address, &mut out) {
        return Err(Refusal::Unreadable(what));
    }
    Ok(out)
}

/// Reads `count` elements of `size` bytes, each at the address the array of
/// pointers at `address` gives it. Elements that follow each other in
/// memory are read together.
fn read_through_pointers(
    read: &mut Read<'_>,
    address: u64,
    count: u32,
    size: u64,
    (pointers, elements): (What, What),
) -> Result<Vec<u8>, Refusal> {
    let table = read_array(read, address, count, 8, pointers)?;
    let at = |i: usize| u64_at(&table, i * 8);
    let (count, size) = (count as usize, size as usize);
    let mut out = vec![0; count * size];
    let mut first = 0;
    while first < count {
        let start = at(first);
        if start == 0 {
            return Err(Refusal::NullArray(elements));
        }
        let mut next = first + 1;
        while next < count && start.checked_add(((next - first) * size) as u64) == Some(at(next)) {
            next += 1;
        }
        let run = &mut out[first * size..next * size];
        if start.checked_add(run.len() as u64).is_none() {
            return Err(Refusal::Overflow(elements));
        }
        if !read(start, run) {
            return Err(Refusal::Unreadable(elements));
        }
        first = next;
    }
    Ok(out)
}

fn layout(value: u32) -> Result<Layout, Refusal> {
    match value {
        0 => Ok(Layout::Array),
        1 => Ok(Layout::ArrayOfPointers),
        v => Err(Refusal::Layout(v)),
    }
}

/// The first 0x18 bytes of the inputs at `bytes[at..]`, checked.
fn inputs_head(bytes: &[u8], at: usize) -> Result<InputsHead, Refusal> {
    let top = match u32_at(bytes, at) {
        0 => true,
        1 => false,
        v => return Err(Refusal::Type(v)),
    };
    let flags = u32_at(bytes, at + 4);
    if flags & !build_flags::KNOWN != 0 {
        return Err(Refusal::Flags(flags & !build_flags::KNOWN));
    }
    Ok(InputsHead {
        top,
        flags,
        count: u32_at(bytes, at + 8),
        layout: layout(u32_at(bytes, at + 12))?,
        address: u64_at(bytes, at + 16),
    })
}

/// Reads the inputs' start at `address` (a
/// `D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS`), and none of
/// the arrays it names: what the size query is asked about.
pub fn parse_inputs_head(read: &mut Read<'_>, address: u64) -> Result<InputsHead, Refusal> {
    if address == 0 {
        return Err(Refusal::NullDescription);
    }
    let bytes = read_array(read, address, 1, 0x18, What::Description)?;
    inputs_head(&bytes, 0)
}

fn geometry(bytes: &[u8]) -> Result<Geometry, Refusal> {
    let flags = u32_at(bytes, 4);
    if flags & !geometry_flags::KNOWN != 0 {
        return Err(Refusal::GeometryFlags(flags & !geometry_flags::KNOWN));
    }
    let kind = match u32_at(bytes, 0) {
        0 => GeometryKind::Triangles(Triangles {
            transform: u64_at(bytes, 0x08),
            index_format: match u32_at(bytes, 0x10) {
                0 => IndexFormat::None,
                57 => IndexFormat::U16,
                42 => IndexFormat::U32,
                v => return Err(Refusal::IndexFormat(v)),
            },
            vertex_format: u32_at(bytes, 0x14),
            index_count: u32_at(bytes, 0x18),
            vertex_count: u32_at(bytes, 0x1c),
            index_buffer: u64_at(bytes, 0x20),
            vertex_buffer: u64_at(bytes, 0x28),
            vertex_stride: u64_at(bytes, 0x30),
        }),
        1 => GeometryKind::Boxes(Boxes {
            count: u64_at(bytes, 0x08),
            address: u64_at(bytes, 0x10),
            stride: u64_at(bytes, 0x18),
        }),
        v => return Err(Refusal::GeometryType(v)),
    };
    Ok(Geometry { flags, kind })
}

fn instance(bytes: &[u8]) -> Instance {
    let mut transform = [0.0; 12];
    for (i, value) in transform.iter_mut().enumerate() {
        *value = f32::from_bits(u32_at(bytes, i * 4));
    }
    let (id, hit) = (u32_at(bytes, 0x30), u32_at(bytes, 0x34));
    Instance {
        transform,
        id: id & 0x00ff_ffff,
        mask: (id >> 24) as u8,
        hit_group_offset: hit & 0x00ff_ffff,
        flags: (hit >> 24) as u8,
        bottom_level: u64_at(bytes, 0x38),
    }
}

/// Reads the description at `address` and the geometry or instance
/// descriptions it names.
pub fn parse(read: &mut Read<'_>, address: u64, limits: Limits) -> Result<Description, Refusal> {
    if address == 0 {
        return Err(Refusal::NullDescription);
    }
    let head = read_array(read, address, 1, DESC_BYTES, What::Description)?;
    let inputs = inputs_head(&head, INPUTS_OFFSET as usize)?;
    let destination = u64_at(&head, 0x00);
    if destination == 0 {
        return Err(Refusal::NullDestination);
    }
    let (elements, pointers, limit, size) = if inputs.top {
        (
            What::Instances,
            What::InstancePointers,
            limits.instances,
            INSTANCE_BYTES,
        )
    } else {
        (
            What::Geometries,
            What::GeometryPointers,
            limits.geometries,
            GEOMETRY_BYTES,
        )
    };
    if inputs.count > limit {
        return Err(Refusal::TooMany(elements, inputs.count));
    }
    let bytes = if inputs.count == 0 {
        // Nothing is named: an empty structure, and nothing to read.
        Vec::new()
    } else if inputs.address == 0 {
        return Err(Refusal::NullArray(match inputs.layout {
            Layout::Array => elements,
            Layout::ArrayOfPointers => pointers,
        }));
    } else {
        match inputs.layout {
            Layout::Array => read_array(read, inputs.address, inputs.count, size, elements)?,
            Layout::ArrayOfPointers => {
                let what = (pointers, elements);
                read_through_pointers(read, inputs.address, inputs.count, size, what)?
            }
        }
    };
    let chunks = bytes.chunks_exact(size as usize);
    let level = if inputs.top {
        Level::Top(TopLevel {
            layout: inputs.layout,
            address: inputs.address,
            instances: chunks.map(instance).collect(),
        })
    } else {
        Level::Bottom(BottomLevel {
            layout: inputs.layout,
            address: inputs.address,
            geometries: chunks.map(geometry).collect::<Result<_, _>>()?,
        })
    };
    Ok(Description {
        destination,
        source: u64_at(&head, 0x20),
        scratch: u64_at(&head, 0x28),
        flags: inputs.flags,
        level,
    })
}

/// Reads the `count` post-build descriptions at `address`.
pub fn parse_postbuild(
    read: &mut Read<'_>,
    address: u64,
    count: u32,
    limits: Limits,
) -> Result<Vec<Postbuild>, Refusal> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > limits.postbuild {
        return Err(Refusal::TooMany(What::Postbuild, count));
    }
    if address == 0 {
        return Err(Refusal::NullArray(What::Postbuild));
    }
    let bytes = read_array(read, address, count, POSTBUILD_BYTES, What::Postbuild)?;
    Ok(bytes
        .as_chunks::<{ POSTBUILD_BYTES as usize }>()
        .0
        .iter()
        .map(|b| Postbuild {
            destination: u64_at(b, 0),
            kind: u32_at(b, 8),
        })
        .collect())
}

/// The layouts and values this file reads by, against the host bindings'
/// declarations of the same structures: a build for Windows fails if one
/// differs. This ties the offsets to PC's header; that the console's
/// structures are PC's is still an assumption (see the module text).
#[cfg(windows)]
const _: () = {
    use std::mem::offset_of;
    use windows::Win32::Graphics::Direct3D12 as d3d12;
    use windows::Win32::Graphics::Dxgi::Common as dxgi;

    type Desc = d3d12::D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC;
    type Inputs = d3d12::D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS;
    type Geometry = d3d12::D3D12_RAYTRACING_GEOMETRY_DESC;
    type Triangles = d3d12::D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC;
    type Boxes = d3d12::D3D12_RAYTRACING_GEOMETRY_AABBS_DESC;
    type Stride = d3d12::D3D12_GPU_VIRTUAL_ADDRESS_AND_STRIDE;
    type Instance = d3d12::D3D12_RAYTRACING_INSTANCE_DESC;
    type Postbuild = d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC;

    // The description: `parse` and `inputs_head`.
    assert!(size_of::<Desc>() as u64 == DESC_BYTES);
    assert!(offset_of!(Desc, DestAccelerationStructureData) == 0x00);
    assert!(offset_of!(Desc, Inputs) as u64 == INPUTS_OFFSET);
    assert!(offset_of!(Desc, SourceAccelerationStructureData) == 0x20);
    assert!(offset_of!(Desc, ScratchAccelerationStructureData) == 0x28);
    assert!(size_of::<Inputs>() == 0x18);
    assert!(offset_of!(Inputs, Type) == 0);
    assert!(offset_of!(Inputs, Flags) == 4);
    assert!(offset_of!(Inputs, NumDescs) == 8);
    assert!(offset_of!(Inputs, DescsLayout) == 12);
    assert!(offset_of!(Inputs, Anonymous) == 16);
    assert!(size_of::<d3d12::D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS_0>() == 8);
    assert!(d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL.0 == 0);
    assert!(d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL.0 == 1);
    assert!(d3d12::D3D12_ELEMENTS_LAYOUT_ARRAY.0 == 0);
    assert!(d3d12::D3D12_ELEMENTS_LAYOUT_ARRAY_OF_POINTERS.0 == 1);

    // Build flags.
    assert!(
        d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_ALLOW_UPDATE.0 as u32
            == build_flags::ALLOW_UPDATE
    );
    assert!(
        d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_ALLOW_COMPACTION.0 as u32
            == build_flags::ALLOW_COMPACTION
    );
    assert!(
        d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PREFER_FAST_TRACE.0 as u32
            == build_flags::PREFER_FAST_TRACE
    );
    assert!(
        d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PREFER_FAST_BUILD.0 as u32
            == build_flags::PREFER_FAST_BUILD
    );
    assert!(
        d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_MINIMIZE_MEMORY.0 as u32
            == build_flags::MINIMIZE_MEMORY
    );
    assert!(
        d3d12::D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PERFORM_UPDATE.0 as u32
            == build_flags::PERFORM_UPDATE
    );
    assert!(
        build_flags::PUBLIC
            == build_flags::ALLOW_UPDATE
                | build_flags::ALLOW_COMPACTION
                | build_flags::PREFER_FAST_TRACE
                | build_flags::PREFER_FAST_BUILD
                | build_flags::MINIMIZE_MEMORY
                | build_flags::PERFORM_UPDATE
    );

    // A geometry: `geometry`.
    assert!(size_of::<Geometry>() as u64 == GEOMETRY_BYTES);
    assert!(offset_of!(Geometry, Type) == 0);
    assert!(offset_of!(Geometry, Flags) == 4);
    let body = offset_of!(Geometry, Anonymous);
    assert!(body + offset_of!(Triangles, Transform3x4) == 0x08);
    assert!(body + offset_of!(Triangles, IndexFormat) == 0x10);
    assert!(body + offset_of!(Triangles, VertexFormat) == 0x14);
    assert!(body + offset_of!(Triangles, IndexCount) == 0x18);
    assert!(body + offset_of!(Triangles, VertexCount) == 0x1c);
    assert!(body + offset_of!(Triangles, IndexBuffer) == 0x20);
    let vertices = body + offset_of!(Triangles, VertexBuffer);
    assert!(vertices + offset_of!(Stride, StartAddress) == 0x28);
    assert!(vertices + offset_of!(Stride, StrideInBytes) == 0x30);
    assert!(body + offset_of!(Boxes, AABBCount) == 0x08);
    let boxes = body + offset_of!(Boxes, AABBs);
    assert!(boxes + offset_of!(Stride, StartAddress) == 0x10);
    assert!(boxes + offset_of!(Stride, StrideInBytes) == 0x18);
    assert!(size_of::<d3d12::D3D12_RAYTRACING_AABB>() == 24);
    assert!(d3d12::D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES.0 == 0);
    assert!(d3d12::D3D12_RAYTRACING_GEOMETRY_TYPE_PROCEDURAL_PRIMITIVE_AABBS.0 == 1);
    assert!(d3d12::D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE.0 as u32 == geometry_flags::OPAQUE);
    assert!(
        d3d12::D3D12_RAYTRACING_GEOMETRY_FLAG_NO_DUPLICATE_ANYHIT_INVOCATION.0 as u32
            == geometry_flags::NO_DUPLICATE_ANYHIT_INVOCATION
    );
    assert!(
        geometry_flags::KNOWN
            == geometry_flags::OPAQUE | geometry_flags::NO_DUPLICATE_ANYHIT_INVOCATION
    );
    assert!(dxgi::DXGI_FORMAT_UNKNOWN.0 == 0);
    assert!(dxgi::DXGI_FORMAT_R16_UINT.0 == 57);
    assert!(dxgi::DXGI_FORMAT_R32_UINT.0 == 42);

    // An instance: `instance`. The two bit-field words are declared as
    // such; which bits are which is the header's (24 low bits, 8 high).
    assert!(size_of::<Instance>() as u64 == INSTANCE_BYTES);
    assert!(offset_of!(Instance, Transform) == 0x00);
    assert!(offset_of!(Instance, _bitfield1) == 0x30);
    assert!(offset_of!(Instance, _bitfield2) == 0x34);
    assert!(offset_of!(Instance, AccelerationStructure) == 0x38);
    assert!(
        d3d12::D3D12_RAYTRACING_INSTANCE_FLAG_TRIANGLE_CULL_DISABLE.0 as u8
            == instance_flags::TRIANGLE_CULL_DISABLE
    );
    assert!(
        d3d12::D3D12_RAYTRACING_INSTANCE_FLAG_TRIANGLE_FRONT_COUNTERCLOCKWISE.0 as u8
            == instance_flags::TRIANGLE_FRONT_COUNTERCLOCKWISE
    );
    assert!(
        d3d12::D3D12_RAYTRACING_INSTANCE_FLAG_FORCE_OPAQUE.0 as u8 == instance_flags::FORCE_OPAQUE
    );
    assert!(
        d3d12::D3D12_RAYTRACING_INSTANCE_FLAG_FORCE_NON_OPAQUE.0 as u8
            == instance_flags::FORCE_NON_OPAQUE
    );

    // A post-build description: `parse_postbuild`.
    assert!(size_of::<Postbuild>() as u64 == POSTBUILD_BYTES);
    assert!(offset_of!(Postbuild, DestBuffer) == 0);
    assert!(offset_of!(Postbuild, InfoType) == 8);
};

/// Synthetic guest memory and description images, for this module's tests
/// and the registry's.
#[cfg(test)]
pub(crate) mod image {
    use super::*;

    /// Ranges of bytes at guest addresses, and every read made of them.
    #[derive(Default)]
    pub(crate) struct Memory {
        ranges: Vec<(u64, Vec<u8>)>,
        pub(crate) reads: Vec<(u64, usize)>,
    }

    impl Memory {
        pub(crate) fn put(&mut self, address: u64, bytes: Vec<u8>) {
            self.ranges.push((address, bytes));
        }

        /// A reader of this memory: a read succeeds only inside one range.
        pub(crate) fn reader(&mut self) -> impl FnMut(u64, &mut [u8]) -> bool + '_ {
            |address, out| {
                self.reads.push((address, out.len()));
                for (start, bytes) in &self.ranges {
                    let Some(offset) = address.checked_sub(*start) else {
                        continue;
                    };
                    let Some(end) = (offset as usize).checked_add(out.len()) else {
                        continue;
                    };
                    if end <= bytes.len() {
                        out.copy_from_slice(&bytes[offset as usize..end]);
                        return true;
                    }
                }
                false
            }
        }

        /// The bytes read that no range passed to `named` covers.
        pub(crate) fn reads_outside(&self, named: &[(u64, u64)]) -> Vec<(u64, usize)> {
            let inside = |&(at, len): &(u64, usize)| {
                named
                    .iter()
                    .any(|&(start, bytes)| at >= start && at + len as u64 <= start + bytes)
            };
            self.reads.iter().copied().filter(|r| !inside(r)).collect()
        }
    }

    pub(crate) fn words(parts: &[(u32, u64)]) -> Vec<u8> {
        // Each part is a width in bytes (4 or 8) and a value.
        let mut out = Vec::new();
        for &(width, value) in parts {
            out.extend_from_slice(&value.to_le_bytes()[..width as usize]);
        }
        out
    }

    /// A description's 0x30 bytes.
    pub(crate) fn desc(
        destination: u64,
        (kind, flags, count, layout): (u32, u32, u32, u32),
        array: u64,
        source: u64,
        scratch: u64,
    ) -> Vec<u8> {
        let image = words(&[
            (8, destination),
            (4, kind.into()),
            (4, flags.into()),
            (4, count.into()),
            (4, layout.into()),
            (8, array),
            (8, source),
            (8, scratch),
        ]);
        assert_eq!(image.len() as u64, DESC_BYTES);
        image
    }

    pub(crate) fn triangles(flags: u32, t: &Triangles) -> Vec<u8> {
        let index_format = match t.index_format {
            IndexFormat::None => 0,
            IndexFormat::U16 => 57,
            IndexFormat::U32 => 42,
        };
        let image = words(&[
            (4, 0),
            (4, flags.into()),
            (8, t.transform),
            (4, index_format),
            (4, t.vertex_format.into()),
            (4, t.index_count.into()),
            (4, t.vertex_count.into()),
            (8, t.index_buffer),
            (8, t.vertex_buffer),
            (8, t.vertex_stride),
        ]);
        assert_eq!(image.len() as u64, GEOMETRY_BYTES);
        image
    }

    pub(crate) fn boxes(flags: u32, b: &Boxes) -> Vec<u8> {
        let mut image = words(&[
            (4, 1),
            (4, flags.into()),
            (8, b.count),
            (8, b.address),
            (8, b.stride),
        ]);
        // The union is as long as its longer member; the rest is whatever
        // the title left there.
        image.resize(GEOMETRY_BYTES as usize, 0xcc);
        image
    }

    pub(crate) fn instance_image(i: &Instance) -> Vec<u8> {
        let mut image = Vec::new();
        for value in i.transform {
            image.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        image.extend(words(&[
            (4, u64::from(i.id | u32::from(i.mask) << 24)),
            (4, u64::from(i.hit_group_offset | u32::from(i.flags) << 24)),
            (8, i.bottom_level),
        ]));
        assert_eq!(image.len() as u64, INSTANCE_BYTES);
        image
    }

    /// An instance of the bottom level at `bottom_level`, told apart by `n`.
    pub(crate) fn some_instance(n: u32, bottom_level: u64) -> Instance {
        let mut transform = [0.0; 12];
        for (i, value) in transform.iter_mut().enumerate() {
            *value = (n * 16 + i as u32) as f32 * 0.25 - 3.0;
        }
        Instance {
            transform,
            id: 0x00a0_0000 | n,
            mask: 0x80 | n as u8,
            hit_group_offset: 0x00ff_ff00 | n,
            flags: (n as u8) & instance_flags::KNOWN,
            bottom_level,
        }
    }

    pub(crate) fn some_triangles(n: u32) -> Triangles {
        Triangles {
            transform: if n.is_multiple_of(2) {
                0
            } else {
                0x40_0000_0000 + u64::from(n) * 48
            },
            index_format: [IndexFormat::U16, IndexFormat::U32, IndexFormat::None][n as usize % 3],
            vertex_format: 6,
            index_count: if n % 3 == 2 { 0 } else { 300 + n * 3 },
            vertex_count: 100 + n,
            index_buffer: if n % 3 == 2 {
                0
            } else {
                0x41_0000_0000 + u64::from(n) * 0x1000
            },
            vertex_buffer: 0x42_0000_0000 + u64::from(n) * 0x1000,
            vertex_stride: 12 + u64::from(n) * 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::image::*;
    use super::*;

    const DESC: u64 = 0x7ff0_1000;
    const ARRAY: u64 = 0x20_0000_0000;
    const DEST: u64 = 0x30_0000_0100;

    fn parse_in(memory: &mut Memory) -> Result<Description, Refusal> {
        parse(&mut memory.reader(), DESC, Limits::DEFAULT)
    }

    #[test]
    fn a_bottom_level_with_an_array_of_geometries_is_read_field_by_field() {
        let first = some_triangles(1);
        let boxed = Boxes {
            count: 77,
            address: 0x43_0000_0000,
            stride: 24,
        };
        let last = some_triangles(2);
        let mut array = triangles(geometry_flags::OPAQUE, &first);
        array.extend(boxes(
            geometry_flags::NO_DUPLICATE_ANYHIT_INVOCATION,
            &boxed,
        ));
        array.extend(triangles(0, &last));
        let flags = build_flags::ALLOW_UPDATE | build_flags::PREFER_FAST_TRACE;
        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (1, flags, 3, 0), ARRAY, 0, 0x31_0000_0000));
        memory.put(ARRAY, array);

        let d = parse_in(&mut memory).unwrap();
        assert_eq!(
            (d.destination, d.source, d.scratch),
            (DEST, 0, 0x31_0000_0000)
        );
        assert_eq!(d.flags, flags);
        assert!(!d.is_update() && !d.is_top());
        let Level::Bottom(bottom) = &d.level else {
            panic!("a bottom level");
        };
        assert_eq!((bottom.layout, bottom.address), (Layout::Array, ARRAY));
        assert_eq!(
            bottom.geometries,
            [
                Geometry {
                    flags: 1,
                    kind: GeometryKind::Triangles(first)
                },
                Geometry {
                    flags: 2,
                    kind: GeometryKind::Boxes(boxed)
                },
                Geometry {
                    flags: 0,
                    kind: GeometryKind::Triangles(last)
                },
            ]
        );
        // The description and its array, in two reads, and nothing else:
        // no index, vertex, box or transform data.
        assert_eq!(memory.reads, [(DESC, 0x30), (ARRAY, 3 * 0x38)]);
    }

    #[test]
    fn a_bottom_level_with_pointers_reads_each_geometry_where_it_is() {
        let geometries: Vec<Triangles> = (0..4).map(some_triangles).collect();
        // The first two follow each other; the others are elsewhere, in
        // the opposite order.
        let places = [
            0x50_0000_0000,
            0x50_0000_0038,
            0x52_0000_0000,
            0x51_0000_0000,
        ];
        let mut memory = Memory::default();
        let flags = build_flags::PERFORM_UPDATE | build_flags::ALLOW_UPDATE;
        memory.put(DESC, desc(DEST, (1, flags, 4, 1), ARRAY, 0x30_0000_0100, 0));
        memory.put(ARRAY, words(&places.map(|p| (8, p))));
        let mut pair = triangles(1, &geometries[0]);
        pair.extend(triangles(1, &geometries[1]));
        memory.put(places[0], pair);
        memory.put(places[2], triangles(1, &geometries[2]));
        memory.put(places[3], triangles(1, &geometries[3]));

        let d = parse_in(&mut memory).unwrap();
        assert!(d.is_update());
        assert_eq!(d.source, 0x30_0000_0100);
        let Level::Bottom(bottom) = &d.level else {
            panic!("a bottom level");
        };
        assert_eq!(bottom.layout, Layout::ArrayOfPointers);
        let read: Vec<Triangles> = bottom
            .geometries
            .iter()
            .map(|g| match g.kind {
                GeometryKind::Triangles(t) => t,
                GeometryKind::Boxes(_) => panic!("triangles"),
            })
            .collect();
        assert_eq!(read, geometries);
        assert_eq!(
            memory.reads,
            [
                (DESC, 0x30),
                (ARRAY, 32),
                (places[0], 0x70),
                (places[2], 0x38),
                (places[3], 0x38)
            ]
        );
    }

    #[test]
    fn a_top_level_with_an_array_of_instances_keeps_every_field() {
        let instances: Vec<Instance> = (0..5)
            .map(|n| some_instance(n, 0x30_0000_0000 + u64::from(n % 2) * 0x100))
            .collect();
        let mut memory = Memory::default();
        memory.put(
            DESC,
            desc(
                DEST,
                (0, build_flags::PREFER_FAST_BUILD, 5, 0),
                ARRAY,
                0,
                0x99,
            ),
        );
        memory.put(ARRAY, instances.iter().flat_map(instance_image).collect());

        let d = parse_in(&mut memory).unwrap();
        assert!(d.is_top());
        let Level::Top(top) = &d.level else {
            panic!("a top level");
        };
        assert_eq!((top.layout, top.address), (Layout::Array, ARRAY));
        assert_eq!(top.instances, instances);
        // The 24-bit fields and the 8-bit ones beside them do not mix.
        assert_eq!(top.instances[3].id, 0x00a0_0003);
        assert_eq!(top.instances[3].mask, 0x83);
        assert_eq!(top.instances[3].hit_group_offset, 0x00ff_ff03);
        assert_eq!(top.instances[3].flags, 3);
        assert_eq!(memory.reads, [(DESC, 0x30), (ARRAY, 5 * 0x40)]);
    }

    #[test]
    fn a_top_level_with_pointers_reads_each_instance_where_it_is() {
        let instances: Vec<Instance> = (0..3).map(|n| some_instance(n, 0x30_0000_0000)).collect();
        let places = [0x60_0000_0040, 0x60_0000_0000, 0x60_0000_0080];
        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (0, 0, 3, 1), ARRAY, 0, 0));
        memory.put(ARRAY, words(&places.map(|p| (8, p))));
        // In memory the second comes first.
        let mut block = instance_image(&instances[1]);
        block.extend(instance_image(&instances[0]));
        block.extend(instance_image(&instances[2]));
        memory.put(0x60_0000_0000, block);

        let d = parse_in(&mut memory).unwrap();
        let Level::Top(top) = &d.level else {
            panic!("a top level");
        };
        assert_eq!(top.layout, Layout::ArrayOfPointers);
        assert_eq!(top.instances, instances);
    }

    #[test]
    fn an_empty_structure_names_nothing_and_nothing_is_read_for_it() {
        for kind in [0, 1] {
            let mut memory = Memory::default();
            // Its array address is whatever the title left: not followed.
            memory.put(DESC, desc(DEST, (kind, 0, 0, 0), 0xdead_0000, 0, 0));
            let d = parse_in(&mut memory).unwrap();
            match &d.level {
                Level::Top(top) => assert!(top.instances.is_empty()),
                Level::Bottom(bottom) => assert!(bottom.geometries.is_empty()),
            }
            assert_eq!(memory.reads, [(DESC, 0x30)]);
        }
    }

    #[test]
    fn the_console_flag_for_packed_structures_is_kept_and_other_bits_refuse() {
        let mut memory = Memory::default();
        let flags = build_flags::CONSOLE_DEHYDRATE | build_flags::PREFER_FAST_TRACE;
        memory.put(DESC, desc(DEST, (1, flags, 0, 0), 0, 0, 0));
        assert_eq!(parse_in(&mut memory).unwrap().flags, flags);

        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (1, flags | 0x0200_0040, 0, 0), 0, 0, 0));
        assert_eq!(parse_in(&mut memory), Err(Refusal::Flags(0x0200_0040)));
    }

    #[test]
    fn what_pc_does_not_have_is_refused_and_not_guessed() {
        let refused = |image: Vec<u8>, array: Option<Vec<u8>>| {
            let mut memory = Memory::default();
            memory.put(DESC, image);
            if let Some(array) = array {
                memory.put(ARRAY, array);
            }
            parse_in(&mut memory).unwrap_err()
        };
        assert_eq!(
            refused(desc(DEST, (2, 0, 1, 0), ARRAY, 0, 0), None),
            Refusal::Type(2)
        );
        assert_eq!(
            refused(desc(DEST, (0, 0, 1, 2), ARRAY, 0, 0), None),
            Refusal::Layout(2)
        );
        assert_eq!(
            refused(desc(0, (0, 0, 1, 0), ARRAY, 0, 0), None),
            Refusal::NullDestination
        );
        assert_eq!(
            refused(desc(DEST, (1, 0, 1, 0), 0, 0, 0), None),
            Refusal::NullArray(What::Geometries)
        );
        assert_eq!(
            refused(desc(DEST, (0, 0, 1, 1), 0, 0, 0), None),
            Refusal::NullArray(What::InstancePointers)
        );

        let good = some_triangles(0);
        let mut bad_type = triangles(0, &good);
        bad_type[0] = 2;
        assert_eq!(
            refused(desc(DEST, (1, 0, 1, 0), ARRAY, 0, 0), Some(bad_type)),
            Refusal::GeometryType(2)
        );
        assert_eq!(
            refused(
                desc(DEST, (1, 0, 1, 0), ARRAY, 0, 0),
                Some(triangles(0x13, &good))
            ),
            Refusal::GeometryFlags(0x10)
        );
        let mut bad_index = triangles(0, &good);
        bad_index[0x10] = 41;
        assert_eq!(
            refused(desc(DEST, (1, 0, 1, 0), ARRAY, 0, 0), Some(bad_index)),
            Refusal::IndexFormat(41)
        );
        // One bad geometry refuses the whole description: the second here.
        let mut two = triangles(0, &good);
        two.extend(boxes(
            4,
            &Boxes {
                count: 1,
                address: 2,
                stride: 24,
            },
        ));
        assert_eq!(
            refused(desc(DEST, (1, 0, 2, 0), ARRAY, 0, 0), Some(two)),
            Refusal::GeometryFlags(4)
        );
    }

    #[test]
    fn counts_over_the_limit_are_refused_before_anything_is_read_for_them() {
        let limits = Limits {
            instances: 4,
            geometries: 2,
            postbuild: 1,
        };
        for (kind, count, what) in [(0, 5, What::Instances), (1, 3, What::Geometries)] {
            let mut memory = Memory::default();
            memory.put(DESC, desc(DEST, (kind, 0, count, 0), ARRAY, 0, 0));
            memory.put(ARRAY, vec![0; 0x1000]);
            let got = parse(&mut memory.reader(), DESC, limits);
            assert_eq!(got, Err(Refusal::TooMany(what, count)));
            assert_eq!(memory.reads, [(DESC, 0x30)]);
        }
        // The largest count a description can state allocates nothing.
        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (0, 0, u32::MAX, 1), ARRAY, 0, 0));
        assert_eq!(
            parse_in(&mut memory),
            Err(Refusal::TooMany(What::Instances, u32::MAX))
        );
    }

    #[test]
    fn a_count_exactly_at_the_limit_is_read() {
        let limits = Limits {
            instances: 4,
            geometries: 2,
            postbuild: 2,
        };
        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (0, 0, 4, 0), ARRAY, 0, 0));
        let instances = (0..4).map(|n| some_instance(n, 0x30_0000_0000));
        memory.put(ARRAY, instances.flat_map(|i| instance_image(&i)).collect());
        let d = parse(&mut memory.reader(), DESC, limits).unwrap();
        assert!(matches!(&d.level, Level::Top(top) if top.instances.len() == 4));

        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (1, 0, 2, 0), ARRAY, 0, 0));
        let sets = (0..2).flat_map(|n| triangles(1, &some_triangles(n)));
        memory.put(ARRAY, sets.collect());
        let d = parse(&mut memory.reader(), DESC, limits).unwrap();
        assert!(matches!(&d.level, Level::Bottom(bottom) if bottom.geometries.len() == 2));

        let mut memory = Memory::default();
        memory.put(0x9000, vec![0; 2 * POSTBUILD_BYTES as usize]);
        let read = &mut memory.reader();
        assert_eq!(parse_postbuild(read, 0x9000, 2, limits).unwrap().len(), 2);
        assert_eq!(
            parse_postbuild(read, 0x9000, 3, limits),
            Err(Refusal::TooMany(What::Postbuild, 3))
        );
    }

    #[test]
    fn memory_that_cannot_be_read_refuses() {
        let mut memory = Memory::default();
        assert_eq!(
            parse_in(&mut memory),
            Err(Refusal::Unreadable(What::Description))
        );
        assert_eq!(
            parse(&mut memory.reader(), 0, Limits::DEFAULT),
            Err(Refusal::NullDescription)
        );

        // An array one element short.
        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (0, 0, 2, 0), ARRAY, 0, 0));
        memory.put(ARRAY, vec![0; 0x40]);
        assert_eq!(
            parse_in(&mut memory),
            Err(Refusal::Unreadable(What::Instances))
        );

        // A pointer to nowhere, and a null one.
        for (pointer, refusal) in [
            (0x70_0000_0000, Refusal::Unreadable(What::Geometries)),
            (0, Refusal::NullArray(What::Geometries)),
        ] {
            let mut memory = Memory::default();
            memory.put(DESC, desc(DEST, (1, 0, 1, 1), ARRAY, 0, 0));
            memory.put(ARRAY, words(&[(8, pointer)]));
            assert_eq!(parse_in(&mut memory), Err(refusal));
        }

        // Addresses that would wrap.
        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (0, 0, 2, 0), u64::MAX - 0x40, 0, 0));
        assert_eq!(
            parse_in(&mut memory),
            Err(Refusal::Overflow(What::Instances))
        );
        let mut memory = Memory::default();
        memory.put(DESC, desc(DEST, (1, 0, 1, 1), ARRAY, 0, 0));
        memory.put(ARRAY, words(&[(8, u64::MAX - 8)]));
        assert_eq!(
            parse_in(&mut memory),
            Err(Refusal::Overflow(What::Geometries))
        );
    }

    #[test]
    fn the_inputs_alone_are_read_for_a_size_query() {
        let mut memory = Memory::default();
        let whole = desc(DEST, (1, build_flags::ALLOW_COMPACTION, 9, 1), ARRAY, 0, 0);
        memory.put(0x8000, whole[INPUTS_OFFSET as usize..].to_vec());
        let head = parse_inputs_head(&mut memory.reader(), 0x8000).unwrap();
        assert_eq!(
            head,
            InputsHead {
                top: false,
                flags: build_flags::ALLOW_COMPACTION,
                count: 9,
                layout: Layout::ArrayOfPointers,
                address: ARRAY,
            }
        );
        assert_eq!(memory.reads, [(0x8000, 0x18)]);
        assert_eq!(
            parse_inputs_head(&mut memory.reader(), 0),
            Err(Refusal::NullDescription)
        );
    }

    #[test]
    fn post_build_descriptions_are_read_and_bounded() {
        let mut memory = Memory::default();
        memory.put(
            0x9000,
            words(&[
                (8, 0x1234_5000),
                (4, 0),
                (4, 0xcccc_cccc),
                (8, 0x1234_6000),
                (4, 3),
                (4, 0),
            ]),
        );
        let limits = Limits::DEFAULT;
        assert_eq!(
            parse_postbuild(&mut memory.reader(), 0x9000, 2, limits).unwrap(),
            [
                Postbuild {
                    destination: 0x1234_5000,
                    kind: 0
                },
                Postbuild {
                    destination: 0x1234_6000,
                    kind: 3
                },
            ]
        );
        assert_eq!(
            parse_postbuild(&mut memory.reader(), 0, 0, limits),
            Ok(Vec::new())
        );
        assert_eq!(
            parse_postbuild(&mut memory.reader(), 0, 1, limits),
            Err(Refusal::NullArray(What::Postbuild))
        );
        assert_eq!(
            parse_postbuild(&mut memory.reader(), 0x9000, 17, limits),
            Err(Refusal::TooMany(What::Postbuild, 17))
        );
        assert_eq!(
            parse_postbuild(&mut memory.reader(), 0x9000, 3, limits),
            Err(Refusal::Unreadable(What::Postbuild))
        );
    }

    /// What docs/memory.md says an entry costs, and what a read can hold
    /// for the length of a call: the bytes read (16 MiB of instances, with
    /// 2 MiB of pointers to them) and the instances made of them.
    #[test]
    fn what_a_description_costs() {
        // The compiler lays these out; the figures are upper bounds.
        assert!(size_of::<Geometry>() <= 64 && size_of::<Instance>() <= 72);
        let limits = Limits::DEFAULT;
        let read = u64::from(limits.instances) * (INSTANCE_BYTES + 8);
        let kept = u64::from(limits.instances) * size_of::<Instance>() as u64;
        assert!(read <= 18 << 20 && kept <= 18 << 20);
        assert!(u64::from(limits.geometries) * (GEOMETRY_BYTES + 8 + 64) < 9 << 20);
    }

    #[test]
    fn the_formats_dxr_takes_for_positions() {
        for format in [6, 16, 10, 34, 13, 37, 11, 35, 24, 28, 49, 31, 51] {
            assert!(dxr_vertex_format(format), "{format}");
        }
        // Unknown, four floats, and a block-compressed format.
        for format in [0, 2, 71] {
            assert!(!dxr_vertex_format(format), "{format}");
        }
    }
}
