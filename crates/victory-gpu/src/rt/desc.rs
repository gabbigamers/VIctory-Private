//! What a host ray-tracing structure is built from, as plain data: the
//! geometry of a bottom level and the instances of a top level, in the
//! terms of D3D12's build descriptions but with no D3D12 type in them, so
//! the rules are checked and tested on any host. `super::device` turns a
//! checked description into the D3D12 one.
//!
//! Addresses are host GPU virtual addresses. A description says nothing of
//! what the memory holds: a rule about contents (a triangle with a NaN x in
//! a corner is inactive, and may not become active in a refit) is the
//! caller's to keep, and is named where it matters ([`super::update`]).
//!
//! The limits and bit values are D3D12's, written here as numbers; the
//! Windows build checks each against the system's headers
//! (`super::device`). The rules are those of the DirectX Raytracing
//! functional specification; where a check here is stricter than D3D12 (a
//! box stride of 0, an empty level, a stride over 32 bits) it says so, and
//! what it refuses stays with the software backend.

use super::cache::Handle;

/// The alignment of a structure's and its scratch memory's address.
pub const STRUCTURE_ALIGN: u64 = 256;
/// The alignment of an array of instance descriptions.
pub const INSTANCE_ALIGN: u64 = 16;
/// The alignment of a geometry's transform.
pub const TRANSFORM_ALIGN: u64 = 16;
/// The alignment of an array of boxes.
pub const BOX_ALIGN: u64 = 8;
/// The most geometries in a bottom level, primitives in a bottom level and
/// instances in a top level.
pub const MAX_GEOMETRIES: u64 = 1 << 24;
pub const MAX_PRIMITIVES: u64 = 1 << 29;
pub const MAX_INSTANCES: u64 = 1 << 24;
/// The bytes of one instance description, one box (six floats: the least
/// corner, then the greatest) and one transform (three rows of four floats).
pub const INSTANCE_BYTES: usize = 64;
pub const BOX_BYTES: u64 = 24;
pub const TRANSFORM_BYTES: u64 = 48;
/// The greatest stride of vertices or boxes. D3D12's field is 64 bits wide
/// and only its low 32 are read, so a wider stride would be built from as
/// another one: it is refused here.
pub const MAX_STRIDE: u64 = u32::MAX as u64;

/// A vertex position format a bottom level takes. The six every device
/// with ray tracing takes; tier 1.1 adds 8- and 10-bit normalised ones,
/// which are refused here (as any other format) until a title needs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexFormat {
    /// Three 32-bit floats.
    Float3,
    /// Two 32-bit floats; z is 0.
    Float2,
    /// Four 16-bit floats; the fourth is not read, and may be left out of
    /// the stride.
    Half4,
    /// Two 16-bit floats; z is 0.
    Half2,
    /// Four signed normalised 16-bit values; the fourth as in `Half4`.
    Snorm16x4,
    /// Two signed normalised 16-bit values; z is 0.
    Snorm16x2,
}

impl VertexFormat {
    /// The format's `DXGI_FORMAT` number.
    pub const fn dxgi(self) -> u32 {
        match self {
            VertexFormat::Float3 => 6,
            VertexFormat::Float2 => 16,
            VertexFormat::Half4 => 10,
            VertexFormat::Half2 => 34,
            VertexFormat::Snorm16x4 => 13,
            VertexFormat::Snorm16x2 => 37,
        }
    }

    /// The bytes of a position a build reads (the least stride), and of
    /// one of its components (which the address and the stride are
    /// multiples of).
    pub const fn bytes(self) -> u64 {
        match self {
            VertexFormat::Float3 => 12,
            VertexFormat::Float2 => 8,
            VertexFormat::Half4 | VertexFormat::Snorm16x4 => 6,
            VertexFormat::Half2 | VertexFormat::Snorm16x2 => 4,
        }
    }

    pub const fn component(self) -> u64 {
        match self {
            VertexFormat::Float3 | VertexFormat::Float2 => 4,
            _ => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFormat {
    U16,
    U32,
}

impl IndexFormat {
    pub const fn dxgi(self) -> u32 {
        match self {
            IndexFormat::U16 => 57,
            IndexFormat::U32 => 42,
        }
    }

    pub const fn bytes(self) -> u64 {
        match self {
            IndexFormat::U16 => 2,
            IndexFormat::U32 => 4,
        }
    }
}

/// A geometry's flags. Not opaque is what an alpha-tested geometry is: a
/// `RayQuery` stops at each candidate for the program to decide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeometryFlags {
    pub opaque: bool,
    /// A non-opaque primitive is offered to the program once a ray, at a
    /// cost in traversal.
    pub no_duplicate_any_hit: bool,
}

impl GeometryFlags {
    /// The `D3D12_RAYTRACING_GEOMETRY_FLAGS` bits.
    pub const fn bits(self) -> u32 {
        self.opaque as u32 | ((self.no_duplicate_any_hit as u32) << 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indices {
    pub address: u64,
    pub format: IndexFormat,
    pub count: u32,
}

impl Indices {
    /// The bytes the build reads at `address`.
    pub const fn bytes(&self) -> u64 {
        self.count as u64 * self.format.bytes()
    }
}

/// Triangles: every three indices one, or every three vertices without
/// indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Triangles {
    /// The first vertex's position.
    pub vertices: u64,
    pub vertex_stride: u64,
    pub vertex_count: u32,
    pub vertex_format: VertexFormat,
    pub indices: Option<Indices>,
    /// The address of a transform applied to the positions when built.
    pub transform: Option<u64>,
    pub flags: GeometryFlags,
}

impl Triangles {
    /// The bytes the build reads at `vertices`: up to the end of the last
    /// position, not of its stride. `None` where that is more than 64 bits
    /// count (no checked description: its stride fits 32).
    pub const fn vertex_bytes(&self) -> Option<u64> {
        match self.vertex_count {
            0 => Some(0),
            n => match (n as u64 - 1).checked_mul(self.vertex_stride) {
                Some(before) => before.checked_add(self.vertex_format.bytes()),
                None => None,
            },
        }
    }

    pub const fn primitives(&self) -> u64 {
        match self.indices {
            Some(indices) => indices.count as u64 / 3,
            None => self.vertex_count as u64 / 3,
        }
    }
}

/// Axis-aligned boxes around primitives the program intersects itself
/// (procedural primitives): what a hybrid of hardware traversal and
/// software intersection is built from. D3D12 takes a stride of 0 (one box
/// many times); here a stride is at least a box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boxes {
    pub address: u64,
    pub stride: u64,
    pub count: u64,
    pub flags: GeometryFlags,
}

impl Boxes {
    /// The bytes the build reads at `address`; `None` where that is more
    /// than 64 bits count (no checked description).
    pub const fn bytes(&self) -> Option<u64> {
        match self.count {
            0 => Some(0),
            n => match (n - 1).checked_mul(self.stride) {
                Some(before) => before.checked_add(BOX_BYTES),
                None => None,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    Triangles(Triangles),
    Boxes(Boxes),
}

/// What a build is asked to favour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Prefer {
    #[default]
    Neither,
    FastTrace,
    FastBuild,
}

/// A build's flags, without "perform update" (that is the decision of
/// [`super::update::decide`], not part of what a structure is).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildFlags {
    /// The structure can be refitted later.
    pub allow_update: bool,
    /// The structure can be copied into less memory once built.
    pub allow_compaction: bool,
    pub prefer: Prefer,
    pub minimize_memory: bool,
}

impl BuildFlags {
    /// The `D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAGS` bits.
    pub const fn bits(self) -> u32 {
        let prefer = match self.prefer {
            Prefer::Neither => 0,
            Prefer::FastTrace => 4,
            Prefer::FastBuild => 8,
        };
        self.allow_update as u32 | ((self.allow_compaction as u32) << 1) | prefer | ((self.minimize_memory as u32) << 4)
    }
}

/// The "perform update" bit a refit adds to [`BuildFlags::bits`].
pub const PERFORM_UPDATE: u32 = 0x20;

/// Whether the memory a build reads holds what it is to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inputs {
    /// Written already by the CPU, or by GPU commands that are submitted or
    /// recorded earlier in the list the build is recorded into.
    Ready,
    /// Something not yet submitted (and not in the build's list) is still
    /// to write it: the build cannot be ordered after that and is refused.
    Pending,
}

/// A bottom level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BottomDesc {
    pub geometries: Vec<Geometry>,
    pub flags: BuildFlags,
    pub inputs: Inputs,
}

/// Instance flag bits (`D3D12_RAYTRACING_INSTANCE_FLAGS`).
pub const INSTANCE_CULL_DISABLE: u8 = 1;
pub const INSTANCE_FRONT_COUNTERCLOCKWISE: u8 = 2;
pub const INSTANCE_FORCE_OPAQUE: u8 = 4;
pub const INSTANCE_FORCE_NON_OPAQUE: u8 = 8;

/// One instance of a bottom level in a top level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    /// Object to world: three rows of four.
    pub transform: [f32; 12],
    /// What a hit reports as the instance's id: 24 bits.
    pub id: u32,
    pub mask: u8,
    /// The hit group offset, 24 bits: not used by inline queries, kept so a
    /// description round-trips.
    pub hit_group: u32,
    pub flags: u8,
    /// The bottom level; `None` is an instance no ray meets.
    pub bottom: Option<Handle>,
}

/// A top level.
#[derive(Clone, Debug, PartialEq)]
pub struct TopDesc {
    pub instances: Vec<Instance>,
    pub flags: BuildFlags,
    pub inputs: Inputs,
}

/// Why a description is not built. Each is a case for the software backend
/// until it is handled, and is counted where it is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// A bottom level without a geometry, or a top level without an
    /// instance. Whether D3D12 takes either is not established here.
    Nothing,
    /// Triangles and boxes in one bottom level, which D3D12 does not take.
    Mixed,
    TooManyGeometries(u64),
    /// The primitives counted when the limit was passed (the most 64 bits
    /// hold, where the counts sum past that).
    TooManyPrimitives(u64),
    TooManyInstances(u64),
    /// [`Inputs::Pending`].
    InputsPending,
    /// A geometry (by its place) with no vertices, indices or boxes, or
    /// with a count that is not whole triangles.
    Count { geometry: usize, count: u64 },
    /// A geometry's memory (`what`) at address 0.
    Null { geometry: usize, what: &'static str },
    /// A geometry's memory (`what`) not at a multiple of `align`.
    Misaligned { geometry: usize, what: &'static str, align: u64 },
    /// A stride shorter than the element, not a multiple of what the
    /// element is aligned to, or over [`MAX_STRIDE`].
    Stride { geometry: usize, stride: u64 },
    /// An instance (by its place) whose id or hit group needs more than 24
    /// bits.
    Field { instance: usize, what: &'static str },
    /// An instance whose transform holds a value that is not finite.
    Transform { instance: usize },
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Invalid::Nothing => write!(f, "nothing to build from"),
            Invalid::Mixed => write!(f, "triangles and boxes in one bottom level"),
            Invalid::TooManyGeometries(n) => write!(f, "{n} geometries (at most {MAX_GEOMETRIES})"),
            Invalid::TooManyPrimitives(n) => write!(f, "{n} primitives (at most {MAX_PRIMITIVES})"),
            Invalid::TooManyInstances(n) => write!(f, "{n} instances (at most {MAX_INSTANCES})"),
            Invalid::InputsPending => write!(f, "the memory the build reads is not written yet"),
            Invalid::Count { geometry, count } => write!(f, "geometry {geometry}: a count of {count}"),
            Invalid::Null { geometry, what } => write!(f, "geometry {geometry}: no {what}"),
            Invalid::Misaligned { geometry, what, align } => {
                write!(f, "geometry {geometry}: {what} not aligned to {align} bytes")
            }
            Invalid::Stride { geometry, stride } => write!(f, "geometry {geometry}: a stride of {stride}"),
            Invalid::Field { instance, what } => write!(f, "instance {instance}: {what} over 24 bits"),
            Invalid::Transform { instance } => write!(f, "instance {instance}: a transform that is not finite"),
        }
    }
}

/// What of a structure a refit may not change: everything but where the
/// positions (or boxes, or transforms) are and what they hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    Bottom { flags: BuildFlags, geometries: Vec<GeometryShape> },
    Top { flags: BuildFlags, instances: u32 },
}

impl Shape {
    pub fn flags(&self) -> BuildFlags {
        match self {
            Shape::Bottom { flags, .. } | Shape::Top { flags, .. } => *flags,
        }
    }
}

/// One geometry's part of a [`Shape`]. The index memory's address is in it:
/// a refit reads the indices the build read, so other memory is another
/// shape (the same memory with other contents is the caller's to say,
/// [`super::update::Change`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryShape {
    Triangles {
        format: VertexFormat,
        vertices: u32,
        stride: u64,
        indices: Option<Indices>,
        transformed: bool,
        flags: GeometryFlags,
    },
    Boxes {
        count: u64,
        stride: u64,
        flags: GeometryFlags,
    },
}

impl BottomDesc {
    /// The description's primitives, if D3D12's rules and this backend's
    /// limits allow building it.
    pub fn check(&self) -> Result<u64, Invalid> {
        if self.inputs == Inputs::Pending {
            return Err(Invalid::InputsPending);
        }
        if self.geometries.is_empty() {
            return Err(Invalid::Nothing);
        }
        if self.geometries.len() as u64 > MAX_GEOMETRIES {
            return Err(Invalid::TooManyGeometries(self.geometries.len() as u64));
        }
        let boxes = |g: &Geometry| matches!(g, Geometry::Boxes(_));
        if self.geometries.iter().any(boxes) != self.geometries.iter().all(boxes) {
            return Err(Invalid::Mixed);
        }
        let mut primitives = 0u64;
        for (geometry, g) in self.geometries.iter().enumerate() {
            let aligned = |what, address: u64, align: u64| match address {
                0 => Err(Invalid::Null { geometry, what }),
                a if a % align != 0 => Err(Invalid::Misaligned { geometry, what, align }),
                _ => Ok(()),
            };
            let count = match g {
                Geometry::Triangles(t) => {
                    let component = t.vertex_format.component();
                    aligned("vertices", t.vertices, component)?;
                    let stride = t.vertex_stride;
                    if stride < t.vertex_format.bytes() || stride % component != 0 || stride > MAX_STRIDE {
                        return Err(Invalid::Stride { geometry, stride });
                    }
                    if t.vertex_count == 0 {
                        return Err(Invalid::Count { geometry, count: 0 });
                    }
                    let corners = match t.indices {
                        Some(indices) => {
                            aligned("indices", indices.address, indices.format.bytes())?;
                            indices.count
                        }
                        None => t.vertex_count,
                    };
                    if corners == 0 || corners % 3 != 0 {
                        return Err(Invalid::Count { geometry, count: u64::from(corners) });
                    }
                    if let Some(transform) = t.transform {
                        aligned("transform", transform, TRANSFORM_ALIGN)?;
                    }
                    t.primitives()
                }
                Geometry::Boxes(b) => {
                    aligned("boxes", b.address, BOX_ALIGN)?;
                    if b.stride < BOX_BYTES || b.stride % BOX_ALIGN != 0 || b.stride > MAX_STRIDE {
                        return Err(Invalid::Stride { geometry, stride: b.stride });
                    }
                    if b.count == 0 {
                        return Err(Invalid::Count { geometry, count: 0 });
                    }
                    b.count
                }
            };
            // One geometry's count can be anything 64 bits hold: the sum
            // stops there and does not wrap back under the limit.
            primitives = primitives.saturating_add(count);
            if primitives > MAX_PRIMITIVES {
                return Err(Invalid::TooManyPrimitives(primitives));
            }
        }
        Ok(primitives)
    }

    pub fn shape(&self) -> Shape {
        let geometries = self
            .geometries
            .iter()
            .map(|g| match *g {
                Geometry::Triangles(t) => GeometryShape::Triangles {
                    format: t.vertex_format,
                    vertices: t.vertex_count,
                    stride: t.vertex_stride,
                    indices: t.indices,
                    transformed: t.transform.is_some(),
                    flags: t.flags,
                },
                Geometry::Boxes(b) => GeometryShape::Boxes { count: b.count, stride: b.stride, flags: b.flags },
            })
            .collect();
        Shape::Bottom { flags: self.flags, geometries }
    }
}

impl TopDesc {
    /// Whether D3D12's rules and this backend's limits allow building the
    /// description. Says nothing of the bottom levels' state
    /// (`super::cache`).
    pub fn check(&self) -> Result<(), Invalid> {
        if self.inputs == Inputs::Pending {
            return Err(Invalid::InputsPending);
        }
        if self.instances.is_empty() {
            return Err(Invalid::Nothing);
        }
        if self.instances.len() as u64 > MAX_INSTANCES {
            return Err(Invalid::TooManyInstances(self.instances.len() as u64));
        }
        for (instance, i) in self.instances.iter().enumerate() {
            if i.id >> 24 != 0 {
                return Err(Invalid::Field { instance, what: "id" });
            }
            if i.hit_group >> 24 != 0 {
                return Err(Invalid::Field { instance, what: "hit group" });
            }
            if !i.transform.iter().all(|v| v.is_finite()) {
                return Err(Invalid::Transform { instance });
            }
        }
        Ok(())
    }

    pub fn shape(&self) -> Shape {
        Shape::Top { flags: self.flags, instances: self.instances.len() as u32 }
    }
}

impl Instance {
    /// The instance as D3D12 reads it (`D3D12_RAYTRACING_INSTANCE_DESC`),
    /// with `bottom` the address of its bottom level (0 for none): the
    /// transform, the id below the mask, the hit group below the flags, the
    /// address. Of a checked description; wider fields are cut to their 24
    /// bits.
    pub fn encode(&self, bottom: u64) -> [u8; INSTANCE_BYTES] {
        let mut out = [0u8; INSTANCE_BYTES];
        for (i, value) in self.transform.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&value.to_le_bytes());
        }
        let id_mask = (self.id & 0x00ff_ffff) | (u32::from(self.mask) << 24);
        let group_flags = (self.hit_group & 0x00ff_ffff) | (u32::from(self.flags) << 24);
        out[48..52].copy_from_slice(&id_mask.to_le_bytes());
        out[52..56].copy_from_slice(&group_flags.to_le_bytes());
        out[56..64].copy_from_slice(&bottom.to_le_bytes());
        out
    }
}

/// The transform that changes nothing.
pub const IDENTITY: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];

#[cfg(test)]
mod tests {
    use super::*;

    fn triangles() -> Triangles {
        Triangles {
            vertices: 0x1_0000,
            vertex_stride: 12,
            vertex_count: 6,
            vertex_format: VertexFormat::Float3,
            indices: None,
            transform: None,
            flags: GeometryFlags { opaque: true, ..Default::default() },
        }
    }

    fn bottom(geometries: Vec<Geometry>) -> BottomDesc {
        BottomDesc { geometries, flags: BuildFlags::default(), inputs: Inputs::Ready }
    }

    fn boxes() -> Boxes {
        Boxes { address: 0x2_0000, stride: 24, count: 5, flags: GeometryFlags::default() }
    }

    fn instance() -> Instance {
        Instance { transform: IDENTITY, id: 7, mask: 0xff, hit_group: 0, flags: 0, bottom: None }
    }

    #[test]
    fn primitives_are_counted_by_indices_where_there_are_any() {
        assert_eq!(bottom(vec![Geometry::Triangles(triangles())]).check(), Ok(2));
        let indexed = Triangles {
            indices: Some(Indices { address: 0x3_0000, format: IndexFormat::U16, count: 12 }),
            vertex_count: 5,
            ..triangles()
        };
        assert_eq!(bottom(vec![Geometry::Triangles(indexed), Geometry::Triangles(triangles())]).check(), Ok(6));
        assert_eq!(bottom(vec![Geometry::Boxes(boxes())]).check(), Ok(5));
    }

    #[test]
    fn what_d3d12_does_not_take_is_refused_with_its_reason() {
        let t = triangles();
        let one = |t: Triangles| bottom(vec![Geometry::Triangles(t)]).check();
        assert_eq!(bottom(vec![]).check(), Err(Invalid::Nothing));
        assert_eq!(
            bottom(vec![Geometry::Triangles(t), Geometry::Boxes(boxes())]).check(),
            Err(Invalid::Mixed),
            "one kind of geometry a bottom level"
        );
        assert_eq!(one(Triangles { vertices: 0, ..t }), Err(Invalid::Null { geometry: 0, what: "vertices" }));
        assert_eq!(
            one(Triangles { vertices: 0x1_0002, ..t }),
            Err(Invalid::Misaligned { geometry: 0, what: "vertices", align: 4 })
        );
        assert_eq!(
            one(Triangles { vertices: 0x1_0002, vertex_format: VertexFormat::Half4, vertex_stride: 8, ..t }),
            Ok(2),
            "16-bit positions sit on 2 bytes"
        );
        assert_eq!(one(Triangles { vertex_stride: 8, ..t }), Err(Invalid::Stride { geometry: 0, stride: 8 }));
        assert_eq!(one(Triangles { vertex_stride: 14, ..t }), Err(Invalid::Stride { geometry: 0, stride: 14 }));
        assert_eq!(one(Triangles { vertex_stride: 32, ..t }), Ok(2), "positions inside a larger vertex");
        let half = Triangles { vertices: 0x1_0000, vertex_format: VertexFormat::Snorm16x4, ..t };
        assert_eq!(one(Triangles { vertex_stride: 6, ..half }), Ok(2), "the fourth component left out");
        assert_eq!(one(Triangles { vertex_stride: 4, ..half }), Err(Invalid::Stride { geometry: 0, stride: 4 }));
        assert_eq!(one(Triangles { vertex_stride: 7, ..half }), Err(Invalid::Stride { geometry: 0, stride: 7 }));
        assert_eq!(one(Triangles { vertex_count: 0, ..t }), Err(Invalid::Count { geometry: 0, count: 0 }));
        assert_eq!(one(Triangles { vertex_count: 7, ..t }), Err(Invalid::Count { geometry: 0, count: 7 }));
        let indices = Indices { address: 0x3_0000, format: IndexFormat::U32, count: 9 };
        assert_eq!(one(Triangles { indices: Some(indices), vertex_count: 7, ..t }), Ok(3));
        assert_eq!(
            one(Triangles { indices: Some(Indices { count: 10, ..indices }), ..t }),
            Err(Invalid::Count { geometry: 0, count: 10 })
        );
        assert_eq!(
            one(Triangles { indices: Some(Indices { address: 0x3_0002, ..indices }), ..t }),
            Err(Invalid::Misaligned { geometry: 0, what: "indices", align: 4 })
        );
        assert_eq!(
            one(Triangles { transform: Some(0x4_0008), ..t }),
            Err(Invalid::Misaligned { geometry: 0, what: "transform", align: 16 })
        );
        assert_eq!(one(Triangles { transform: Some(0x4_0010), ..t }), Ok(2));
        let b = boxes();
        let one = |b: Boxes| bottom(vec![Geometry::Triangles(t), Geometry::Triangles(t), Geometry::Boxes(b)]).check();
        assert_eq!(one(b), Err(Invalid::Mixed));
        let one = |b: Boxes| bottom(vec![Geometry::Boxes(boxes()), Geometry::Boxes(b)]).check();
        assert_eq!(one(Boxes { stride: 16, ..b }), Err(Invalid::Stride { geometry: 1, stride: 16 }));
        assert_eq!(one(Boxes { stride: 28, ..b }), Err(Invalid::Stride { geometry: 1, stride: 28 }));
        assert_eq!(
            one(Boxes { address: 0x2_0004, ..b }),
            Err(Invalid::Misaligned { geometry: 1, what: "boxes", align: 8 })
        );
        assert_eq!(one(Boxes { count: 0, ..b }), Err(Invalid::Count { geometry: 1, count: 0 }));
        assert_eq!(one(Boxes { stride: 32, ..b }), Ok(10));
    }

    /// D3D12 reads the low 32 bits of a stride: a wider one would be built
    /// from as if it were its low half, so it is refused and not cut.
    #[test]
    fn a_stride_d3d12_would_cut_to_32_bits_is_refused() {
        let one = |t: Triangles| bottom(vec![Geometry::Triangles(t)]).check();
        let wide = (1u64 << 32) + 12;
        assert_eq!(
            one(Triangles { vertex_stride: wide, ..triangles() }),
            Err(Invalid::Stride { geometry: 0, stride: wide }),
            "its low 32 bits alone would pass"
        );
        let widest = Triangles { vertex_stride: MAX_STRIDE - 3, ..triangles() };
        assert_eq!(one(widest), Ok(2), "the widest that is read whole");
        let one = |b: Boxes| bottom(vec![Geometry::Boxes(b)]).check();
        let wide = (1u64 << 32) + 24;
        assert_eq!(one(Boxes { stride: wide, ..boxes() }), Err(Invalid::Stride { geometry: 0, stride: wide }));
        assert_eq!(one(Boxes { stride: MAX_STRIDE - 7, ..boxes() }), Ok(5));
    }

    #[test]
    fn the_limits_are_d3d12s() {
        let many = Boxes { count: MAX_PRIMITIVES, ..boxes() };
        assert_eq!(bottom(vec![Geometry::Boxes(many)]).check(), Ok(MAX_PRIMITIVES));
        assert_eq!(
            bottom(vec![Geometry::Boxes(many), Geometry::Boxes(boxes())]).check(),
            Err(Invalid::TooManyPrimitives(MAX_PRIMITIVES + 5))
        );
        // Counts that sum past 64 bits are over the limit too: the sum does
        // not wrap to a small number that passes.
        let most = Boxes { count: u64::MAX, ..boxes() };
        assert_eq!(5u64.wrapping_add(u64::MAX), 4, "what a wrapped sum would be");
        for geometries in [
            vec![Geometry::Boxes(boxes()), Geometry::Boxes(most)],
            vec![Geometry::Boxes(most), Geometry::Boxes(boxes())],
            vec![Geometry::Boxes(most), Geometry::Boxes(most)],
            vec![Geometry::Boxes(most)],
        ] {
            assert_eq!(bottom(geometries).check(), Err(Invalid::TooManyPrimitives(u64::MAX)));
        }
        let over = Boxes { count: MAX_PRIMITIVES + 1, ..boxes() };
        assert_eq!(bottom(vec![Geometry::Boxes(over)]).check(), Err(Invalid::TooManyPrimitives(MAX_PRIMITIVES + 1)));
    }

    #[test]
    fn a_build_whose_inputs_are_still_to_be_written_is_refused() {
        let pending = BottomDesc { inputs: Inputs::Pending, ..bottom(vec![Geometry::Triangles(triangles())]) };
        assert_eq!(pending.check(), Err(Invalid::InputsPending));
        let top = TopDesc { instances: vec![instance()], flags: BuildFlags::default(), inputs: Inputs::Pending };
        assert_eq!(top.check(), Err(Invalid::InputsPending));
    }

    #[test]
    fn the_bytes_a_build_reads_end_with_the_last_element() {
        let t = Triangles { vertex_stride: 32, vertex_count: 10, ..triangles() };
        assert_eq!(t.vertex_bytes(), Some(9 * 32 + 12));
        assert_eq!(Triangles { vertex_count: 0, ..t }.vertex_bytes(), Some(0));
        let packed = Triangles { vertex_format: VertexFormat::Half4, vertex_stride: 6, vertex_count: 4, ..t };
        assert_eq!(packed.vertex_bytes(), Some(24), "three components of the last position, not four");
        assert_eq!(Indices { address: 8, format: IndexFormat::U16, count: 9 }.bytes(), 18);
        assert_eq!(Boxes { stride: 40, count: 3, ..boxes() }.bytes(), Some(2 * 40 + 24));
        // What does not fit 64 bits has no count, and is not a wrapped one.
        assert_eq!(Boxes { stride: u64::MAX - 7, count: 3, ..boxes() }.bytes(), None);
        assert_eq!(Boxes { stride: u64::MAX - 31, count: 2, ..boxes() }.bytes(), Some(u64::MAX - 7));
        assert_eq!(Boxes { stride: u64::MAX - 15, count: 2, ..boxes() }.bytes(), None, "the last box does not fit");
        assert_eq!(Triangles { vertex_stride: 1 << 40, vertex_count: u32::MAX, ..t }.vertex_bytes(), None);
        let widest = Triangles { vertex_stride: MAX_STRIDE - 3, vertex_count: u32::MAX, ..t };
        assert_eq!(widest.vertex_bytes(), Some((u64::from(u32::MAX) - 1) * (MAX_STRIDE - 3) + 12), "every checked one");
    }

    #[test]
    fn flags_have_d3d12s_bits() {
        assert_eq!(BuildFlags::default().bits(), 0);
        let all =
            BuildFlags { allow_update: true, allow_compaction: true, prefer: Prefer::FastTrace, minimize_memory: true };
        assert_eq!(all.bits(), 1 | 2 | 4 | 0x10);
        assert_eq!(BuildFlags { prefer: Prefer::FastBuild, ..Default::default() }.bits(), 8);
        assert_eq!(all.bits() & PERFORM_UPDATE, 0);
        assert_eq!(GeometryFlags { opaque: true, no_duplicate_any_hit: false }.bits(), 1);
        assert_eq!(GeometryFlags { opaque: false, no_duplicate_any_hit: true }.bits(), 2);
    }

    #[test]
    fn an_instance_is_laid_out_as_d3d12_reads_it() {
        let i = Instance {
            transform: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0],
            id: 0x00ab_cdef,
            mask: 0x81,
            hit_group: 0x0012_3456,
            flags: INSTANCE_FORCE_OPAQUE | INSTANCE_CULL_DISABLE,
            bottom: None,
        };
        let bytes = i.encode(0x0000_7fff_1234_5600);
        assert_eq!(bytes.len(), 64);
        for k in 0..12 {
            let v = f32::from_le_bytes(bytes[4 * k..4 * k + 4].try_into().unwrap());
            assert_eq!(v, (k + 1) as f32, "row-major, three rows of four");
        }
        assert_eq!(bytes[48..52], [0xef, 0xcd, 0xab, 0x81], "the id's 24 bits, then the mask");
        assert_eq!(bytes[52..56], [0x56, 0x34, 0x12, 0x05], "the hit group's 24 bits, then the flags");
        assert_eq!(u64::from_le_bytes(bytes[56..64].try_into().unwrap()), 0x0000_7fff_1234_5600);
        assert_eq!(instance().encode(0)[56..64], [0; 8], "no bottom level is address 0");
    }

    #[test]
    fn instances_that_do_not_fit_their_fields_are_refused() {
        let top = |instances| TopDesc { instances, flags: BuildFlags::default(), inputs: Inputs::Ready };
        assert_eq!(top(vec![]).check(), Err(Invalid::Nothing));
        assert_eq!(top(vec![instance(); 3]).check(), Ok(()));
        assert_eq!(top(vec![Instance { id: 0x00ff_ffff, ..instance() }]).check(), Ok(()));
        assert_eq!(
            top(vec![instance(), Instance { id: 0x0100_0000, ..instance() }]).check(),
            Err(Invalid::Field { instance: 1, what: "id" })
        );
        assert_eq!(
            top(vec![Instance { hit_group: 1 << 24, ..instance() }]).check(),
            Err(Invalid::Field { instance: 0, what: "hit group" })
        );
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut i = instance();
            i.transform[7] = bad;
            assert_eq!(top(vec![instance(), instance(), i]).check(), Err(Invalid::Transform { instance: 2 }));
        }
    }

    /// What a refit may not change is in the shape; where the positions are
    /// is not.
    #[test]
    fn a_shape_holds_everything_but_the_positions() {
        let t = triangles();
        let shape = |t: Triangles| bottom(vec![Geometry::Triangles(t)]).shape();
        let base = shape(t);
        assert_eq!(shape(Triangles { vertices: 0x9_0000, ..t }), base, "other vertex memory is the same shape");
        assert_eq!(shape(Triangles { transform: None, ..t }), base);
        for other in [
            Triangles { vertex_count: 9, ..t },
            Triangles { vertex_stride: 16, ..t },
            Triangles { vertex_format: VertexFormat::Float2, ..t },
            Triangles { transform: Some(0x4_0000), ..t },
            Triangles { flags: GeometryFlags::default(), ..t },
            Triangles { indices: Some(Indices { address: 0x3_0000, format: IndexFormat::U16, count: 6 }), ..t },
        ] {
            assert_ne!(shape(other), base, "{other:?}");
        }
        let indexed = |address| {
            shape(Triangles { indices: Some(Indices { address, format: IndexFormat::U16, count: 6 }), ..t })
        };
        assert_ne!(indexed(0x3_0000), indexed(0x5_0000), "other index memory is another shape");
        let moved = Triangles { transform: Some(0x4_0000), ..t };
        assert_eq!(shape(Triangles { transform: Some(0x8_0000), ..moved }), shape(moved));
        let updatable = BottomDesc {
            flags: BuildFlags { allow_update: true, ..Default::default() },
            ..bottom(vec![Geometry::Triangles(t)])
        };
        assert_ne!(updatable.shape(), base, "the flags are part of it");
        assert!(updatable.shape().flags().allow_update);
        let top = |n| TopDesc { instances: vec![instance(); n], flags: BuildFlags::default(), inputs: Inputs::Ready };
        assert_eq!(top(3).shape(), top(3).shape());
        assert_ne!(top(3).shape(), top(4).shape());
        assert_ne!(top(3).shape(), bottom(vec![Geometry::Boxes(boxes())]).shape());
    }
}
