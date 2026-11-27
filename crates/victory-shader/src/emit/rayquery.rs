//! Inline ray queries (DXR 1.1 `RayQuery`, shader model 6.5 on) in the
//! emitter, and the compute shaders of the hardware ray-tracing prototype:
//! synthetic programs written here, which no translated title program
//! reaches. Experimental: nothing in the translation calls this module.
//!
//! The operations ([`OPS`]) are `dx.op` calls on a query handle, a plain
//! `i32` that `AllocateRayQuery` returns; their opcodes, functions,
//! attributes and signatures are DirectXShaderCompiler's (`DxilConstants.h`,
//! `DxilOperations.cpp`, `hctdb.py`; `docs/dxr-shader-emission.md` lists
//! which file gave which fact). A module that allocates a query carries
//! shader flag bit 25 and feature bit `0x100000`; the acceleration structure
//! is an SRV of resource kind 16 (`Shader::accel_handle`).
//!
//! The shaders ([`build`]) take one ray per thread: thread `n` of a
//! dispatch (`SV_DispatchThreadID.x`) reads ray record `n` and writes hit
//! record `n`, and does nothing when `n` is not below the ray count in the
//! constants. The group is [`GROUP_THREADS`] × 1 × 1.
//!
//! Records, little-endian 32-bit words:
//!
//! * **Ray**, [`RAY_BYTES`]: origin x y z, tmin, direction x y z (any
//!   length), tmax (floats); instance mask (low 8 bits used); ray flags
//!   (`D3D12_RAY_FLAG` bits); two words not read.
//! * **Hit**, [`HIT_BYTES`]: t, u, v (floats; u and v the weights of the
//!   triangle's second and third vertex, DXR's barycentrics); kind
//!   ([`HIT_MISS`], [`HIT_FRONT`], [`HIT_BACK`], [`HIT_PROCEDURAL`]);
//!   instance index (place in the top-level list); instance id; geometry
//!   index; primitive index. A miss is eight zero words. A kind with
//!   [`HIT_REFUSED`] says the shader could not decide the ray; its low bits
//!   say why ([`REFUSED_GEOMETRY`], [`REFUSED_RANGE`]) and the other seven
//!   words are 0. The host counts such records and traces their rays
//!   another way: a refusal is never a miss.
//!
//! What a ray may ask for ([`ray_refusal`]): every ray flag is dynamic, an
//! argument of `TraceRayInline`; the flags `AllocateRayQuery` takes must be
//! constants (the validator's rule) and are 0 here, so no driver
//! specialises on them. Not passed on: `SKIP_CLOSEST_HIT_SHADER` (not
//! allowed inline), the opacity-micromap flag (needs SM 6.9's
//! `AllocateRayQuery2`) and unknown bits; the shaders clear them, and the
//! host refuses such rays beforehand, as it does the combinations DXR calls
//! undefined and a direction of zero length.

use super::bitcode::{Attr, Bin, Cast, Term, V, pred};
use super::shader::{ACCEL_SPACE, ROOT_CBV_SPACE, Shader, USER_DATA_SPACE};
use Type::{F32, Handle, I1, I8, I32, Void};

/// A result or parameter type of a ray-query operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Void,
    I1,
    I8,
    I32,
    F32,
    /// `%dx.types.Handle`.
    Handle,
}

/// One `dx.op` of DXIL's inline ray query group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RayOp {
    /// DXIL's name of the operation.
    pub name: &'static str,
    pub opcode: u32,
    /// The function called, after `dx.op.`: the operation class, then the
    /// overload type unless that is void.
    pub function: &'static str,
    /// Declared `readonly` (the state accessors); all are `nounwind`.
    pub reads: bool,
    pub result: Type,
    /// The parameters after the opcode. The query handle is an `i32`.
    pub params: &'static [Type],
}

/// An operation that changes the query's state.
const fn changes(
    name: &'static str,
    opcode: u32,
    function: &'static str,
    result: Type,
    params: &'static [Type],
) -> RayOp {
    RayOp {
        name,
        opcode,
        function,
        reads: false,
        result,
        params,
    }
}

/// An accessor returning one scalar of the query's state.
const fn scalar(name: &'static str, opcode: u32, result: Type) -> RayOp {
    let function = match result {
        I1 => "rayQuery_StateScalar.i1",
        F32 => "rayQuery_StateScalar.f32",
        _ => "rayQuery_StateScalar.i32",
    };
    RayOp {
        name,
        opcode,
        function,
        reads: true,
        result,
        params: &[I32],
    }
}

/// An accessor returning component 0, 1 or 2 (a constant `i8`) of a vector.
const fn vector(name: &'static str, opcode: u32) -> RayOp {
    RayOp {
        name,
        opcode,
        function: "rayQuery_StateVector.f32",
        reads: true,
        result: F32,
        params: &[I32, I8],
    }
}

/// An accessor returning the element at row 0 to 2 (`i32`) and column 0 to
/// 3 (`i8`) of a 3 × 4 matrix.
const fn matrix(name: &'static str, opcode: u32) -> RayOp {
    RayOp {
        name,
        opcode,
        function: "rayQuery_StateMatrix.f32",
        reads: true,
        result: F32,
        params: &[I32, I32, I8],
    }
}

/// The inline ray-query operations, opcodes 178 to 215 (213 is
/// `GeometryIndex`, of the other ray-tracing shaders).
pub mod op {
    use super::Type::{F32, Handle, I1, I32, Void};
    use super::{RayOp, changes, matrix, scalar, vector};

    /// The query handle of new state with these constant ray flags.
    pub const ALLOCATE: RayOp = changes("AllocateRayQuery", 178, "allocateRayQuery", I32, &[I32]);
    /// Query, acceleration structure, ray flags, instance mask, origin x y
    /// z, tmin, direction x y z, tmax.
    pub const TRACE_RAY_INLINE: RayOp = changes(
        "RayQuery_TraceRayInline",
        179,
        "rayQuery_TraceRayInline",
        Void,
        &[
            I32, Handle, I32, I32, F32, F32, F32, F32, F32, F32, F32, F32,
        ],
    );
    /// Whether a candidate waits for the shader.
    pub const PROCEED: RayOp = changes("RayQuery_Proceed", 180, "rayQuery_Proceed.i1", I1, &[I32]);
    pub const ABORT: RayOp = changes("RayQuery_Abort", 181, "rayQuery_Abort", Void, &[I32]);
    pub const COMMIT_NON_OPAQUE_TRIANGLE_HIT: RayOp = changes(
        "RayQuery_CommitNonOpaqueTriangleHit",
        182,
        "rayQuery_CommitNonOpaqueTriangleHit",
        Void,
        &[I32],
    );
    /// Query and the hit's t.
    pub const COMMIT_PROCEDURAL_PRIMITIVE_HIT: RayOp = changes(
        "RayQuery_CommitProceduralPrimitiveHit",
        183,
        "rayQuery_CommitProceduralPrimitiveHit",
        Void,
        &[I32, F32],
    );
    /// 0 nothing, 1 a triangle, 2 a procedural primitive.
    pub const COMMITTED_STATUS: RayOp = scalar("RayQuery_CommittedStatus", 184, I32);
    /// 0 a non-opaque triangle, 1 a procedural primitive.
    pub const CANDIDATE_TYPE: RayOp = scalar("RayQuery_CandidateType", 185, I32);
    pub const CANDIDATE_OBJECT_TO_WORLD: RayOp = matrix("RayQuery_CandidateObjectToWorld3x4", 186);
    pub const CANDIDATE_WORLD_TO_OBJECT: RayOp = matrix("RayQuery_CandidateWorldToObject3x4", 187);
    pub const COMMITTED_OBJECT_TO_WORLD: RayOp = matrix("RayQuery_CommittedObjectToWorld3x4", 188);
    pub const COMMITTED_WORLD_TO_OBJECT: RayOp = matrix("RayQuery_CommittedWorldToObject3x4", 189);
    pub const CANDIDATE_PROCEDURAL_PRIMITIVE_NON_OPAQUE: RayOp =
        scalar("RayQuery_CandidateProceduralPrimitiveNonOpaque", 190, I1);
    pub const CANDIDATE_TRIANGLE_FRONT_FACE: RayOp =
        scalar("RayQuery_CandidateTriangleFrontFace", 191, I1);
    pub const COMMITTED_TRIANGLE_FRONT_FACE: RayOp =
        scalar("RayQuery_CommittedTriangleFrontFace", 192, I1);
    pub const CANDIDATE_TRIANGLE_BARYCENTRICS: RayOp =
        vector("RayQuery_CandidateTriangleBarycentrics", 193);
    pub const COMMITTED_TRIANGLE_BARYCENTRICS: RayOp =
        vector("RayQuery_CommittedTriangleBarycentrics", 194);
    pub const RAY_FLAGS: RayOp = scalar("RayQuery_RayFlags", 195, I32);
    pub const WORLD_RAY_ORIGIN: RayOp = vector("RayQuery_WorldRayOrigin", 196);
    pub const WORLD_RAY_DIRECTION: RayOp = vector("RayQuery_WorldRayDirection", 197);
    pub const RAY_T_MIN: RayOp = scalar("RayQuery_RayTMin", 198, F32);
    pub const CANDIDATE_TRIANGLE_RAY_T: RayOp = scalar("RayQuery_CandidateTriangleRayT", 199, F32);
    /// The closest committed hit's t; tmax while nothing is committed.
    pub const COMMITTED_RAY_T: RayOp = scalar("RayQuery_CommittedRayT", 200, F32);
    pub const CANDIDATE_INSTANCE_INDEX: RayOp = scalar("RayQuery_CandidateInstanceIndex", 201, I32);
    pub const CANDIDATE_INSTANCE_ID: RayOp = scalar("RayQuery_CandidateInstanceID", 202, I32);
    pub const CANDIDATE_GEOMETRY_INDEX: RayOp = scalar("RayQuery_CandidateGeometryIndex", 203, I32);
    pub const CANDIDATE_PRIMITIVE_INDEX: RayOp =
        scalar("RayQuery_CandidatePrimitiveIndex", 204, I32);
    pub const CANDIDATE_OBJECT_RAY_ORIGIN: RayOp = vector("RayQuery_CandidateObjectRayOrigin", 205);
    pub const CANDIDATE_OBJECT_RAY_DIRECTION: RayOp =
        vector("RayQuery_CandidateObjectRayDirection", 206);
    pub const COMMITTED_INSTANCE_INDEX: RayOp = scalar("RayQuery_CommittedInstanceIndex", 207, I32);
    pub const COMMITTED_INSTANCE_ID: RayOp = scalar("RayQuery_CommittedInstanceID", 208, I32);
    pub const COMMITTED_GEOMETRY_INDEX: RayOp = scalar("RayQuery_CommittedGeometryIndex", 209, I32);
    pub const COMMITTED_PRIMITIVE_INDEX: RayOp =
        scalar("RayQuery_CommittedPrimitiveIndex", 210, I32);
    pub const COMMITTED_OBJECT_RAY_ORIGIN: RayOp = vector("RayQuery_CommittedObjectRayOrigin", 211);
    pub const COMMITTED_OBJECT_RAY_DIRECTION: RayOp =
        vector("RayQuery_CommittedObjectRayDirection", 212);
    pub const CANDIDATE_INSTANCE_CONTRIBUTION_TO_HIT_GROUP_INDEX: RayOp = scalar(
        "RayQuery_CandidateInstanceContributionToHitGroupIndex",
        214,
        I32,
    );
    pub const COMMITTED_INSTANCE_CONTRIBUTION_TO_HIT_GROUP_INDEX: RayOp = scalar(
        "RayQuery_CommittedInstanceContributionToHitGroupIndex",
        215,
        I32,
    );
}

/// Every operation of [`op`], by opcode.
pub const OPS: [RayOp; 37] = [
    op::ALLOCATE,
    op::TRACE_RAY_INLINE,
    op::PROCEED,
    op::ABORT,
    op::COMMIT_NON_OPAQUE_TRIANGLE_HIT,
    op::COMMIT_PROCEDURAL_PRIMITIVE_HIT,
    op::COMMITTED_STATUS,
    op::CANDIDATE_TYPE,
    op::CANDIDATE_OBJECT_TO_WORLD,
    op::CANDIDATE_WORLD_TO_OBJECT,
    op::COMMITTED_OBJECT_TO_WORLD,
    op::COMMITTED_WORLD_TO_OBJECT,
    op::CANDIDATE_PROCEDURAL_PRIMITIVE_NON_OPAQUE,
    op::CANDIDATE_TRIANGLE_FRONT_FACE,
    op::COMMITTED_TRIANGLE_FRONT_FACE,
    op::CANDIDATE_TRIANGLE_BARYCENTRICS,
    op::COMMITTED_TRIANGLE_BARYCENTRICS,
    op::RAY_FLAGS,
    op::WORLD_RAY_ORIGIN,
    op::WORLD_RAY_DIRECTION,
    op::RAY_T_MIN,
    op::CANDIDATE_TRIANGLE_RAY_T,
    op::COMMITTED_RAY_T,
    op::CANDIDATE_INSTANCE_INDEX,
    op::CANDIDATE_INSTANCE_ID,
    op::CANDIDATE_GEOMETRY_INDEX,
    op::CANDIDATE_PRIMITIVE_INDEX,
    op::CANDIDATE_OBJECT_RAY_ORIGIN,
    op::CANDIDATE_OBJECT_RAY_DIRECTION,
    op::COMMITTED_INSTANCE_INDEX,
    op::COMMITTED_INSTANCE_ID,
    op::COMMITTED_GEOMETRY_INDEX,
    op::COMMITTED_PRIMITIVE_INDEX,
    op::COMMITTED_OBJECT_RAY_ORIGIN,
    op::COMMITTED_OBJECT_RAY_DIRECTION,
    op::CANDIDATE_INSTANCE_CONTRIBUTION_TO_HIT_GROUP_INDEX,
    op::COMMITTED_INSTANCE_CONTRIBUTION_TO_HIT_GROUP_INDEX,
];

impl Shader {
    /// Calls ray-query operation `op` with `args` (those after the opcode),
    /// whose types must be the operation's; the flags of
    /// [`op::ALLOCATE`] must be a constant.
    pub(crate) fn ray_op(&mut self, block: usize, op: &RayOp, args: &[V]) -> V {
        let mut ty = |t: Type| match t {
            Void => self.t.void,
            I1 => self.t.i1,
            I8 => self.m.int(8),
            I32 => self.t.i32,
            F32 => self.t.f32,
            Handle => self.t.handle,
        };
        let want: Vec<_> = op.params.iter().map(|&p| ty(p)).collect();
        let ret = ty(op.result);
        let have: Vec<_> = args.iter().map(|&a| self.m.type_of(a)).collect();
        assert_eq!(have, want, "the arguments of {}", op.name);
        if op.opcode == op::ALLOCATE.opcode {
            assert!(self.m.int_value(args[0]).is_some(), "constant ray flags");
        }
        let attr = if op.reads {
            Attr::ReadOnly
        } else {
            Attr::NoUnwind
        };
        self.op(block, op.function, attr, ret, op.opcode, args)
    }
}

/// Bytes of a ray record.
pub const RAY_BYTES: u32 = 48;
/// Bytes of a hit record.
pub const HIT_BYTES: u32 = 32;

/// A hit record's kind: nothing was hit (and every other word is 0).
pub const HIT_MISS: u32 = 0;
/// A triangle hit on its front face: its vertices appear clockwise from the
/// ray's origin, in object space, in a left-handed coordinate system
/// (D3D12's default), or counter-clockwise in an instance with
/// `D3D12_RAYTRACING_INSTANCE_FLAG_TRIANGLE_FRONT_COUNTERCLOCKWISE`. The
/// device decides it (`CommittedTriangleFrontFace`).
pub const HIT_FRONT: u32 = 1;
/// A triangle hit on its back face.
pub const HIT_BACK: u32 = 2;
/// A hit the shader committed on a procedural primitive
/// ([`Form::Procedural`]: a triangle tested in the shader).
pub const HIT_PROCEDURAL: u32 = 3;
/// A hit record's kind with this bit: the shader refused the ray, for the
/// reasons in the low bits, and what it hit is not known. The record's
/// other words are 0.
pub const HIT_REFUSED: u32 = 0x8000_0000;
/// With [`HIT_REFUSED`]: the scene is not the form's. The device offered a
/// candidate, or committed a hit, of the other geometry type (a procedural
/// primitive to [`Form::Triangles`], a triangle to [`Form::Procedural`]),
/// which the shader leaves alone.
pub const REFUSED_GEOMETRY: u32 = 1;
/// With [`HIT_REFUSED`]: [`Form::Procedural`]'s test of a candidate left
/// single precision's range ([`triangle_test`] returned [`OutOfRange`]), so
/// that triangle is neither hit nor missed.
pub const REFUSED_RANGE: u32 = 2;

/// The thread group, 64 × 1 × 1. The rays are a list, so one dimension. 64
/// is a whole number of waves at the wave sizes 4, 8, 16, 32 and 64, so no
/// lane of a full group idles there (at 128, which D3D12 also allows, a
/// group is half a wave), and the last, partly listed group of a dispatch
/// has at most 63 idle threads. Nothing here depends on the lane layout: no
/// wave size is declared.
pub const GROUP_THREADS: u32 = 64;

/// The most rays one dispatch takes: D3D12 allows 65,535 thread groups a
/// dimension, so 4,194,240. The host splits a longer list into dispatches
/// (each with its own count and buffer addresses) and counts them.
pub const MAX_RAYS: u32 = 65_535 * GROUP_THREADS;

/// The thread groups (x; y and z are 1) of a dispatch over `rays` rays;
/// `None` above [`MAX_RAYS`], which no single dispatch can list.
pub fn groups(rays: u32) -> Option<u32> {
    (rays <= MAX_RAYS).then(|| rays.div_ceil(GROUP_THREADS))
}

/// The ray flags `TraceRayInline` is given by [`Form::Triangles`]: DXR 1.1's
/// but `SKIP_CLOSEST_HIT_SHADER` (0x08).
pub const RAY_FLAGS: u32 = 0x3F7;
const FORCE_OPAQUE: u32 = 0x01;
const FORCE_NON_OPAQUE: u32 = 0x02;
const ACCEPT_FIRST_HIT: u32 = 0x04;
const CULL_BACK: u32 = 0x10;
const CULL_FRONT: u32 = 0x20;
const CULL_OPAQUE: u32 = 0x40;
const CULL_NON_OPAQUE: u32 = 0x80;
const SKIP_TRIANGLES: u32 = 0x100;
const SKIP_PROCEDURAL: u32 = 0x200;
/// The ray flags the device sees in [`Form::Procedural`]: those that act
/// on procedural primitives as they do on triangles (opacity, the first
/// hit). The shader applies face culling and `SKIP_TRIANGLES` itself;
/// `SKIP_PROCEDURAL_PRIMITIVES` would skip the whole scene and the scene it
/// stands for has no procedural primitive, so it is dropped.
const PROCEDURAL_RAY_FLAGS: u32 =
    FORCE_OPAQUE | FORCE_NON_OPAQUE | ACCEPT_FIRST_HIT | CULL_OPAQUE | CULL_NON_OPAQUE;

/// `D3D12_RAYTRACING_INSTANCE_FLAGS` the procedural form reads from the
/// triangle data: face culling off, front faces counter-clockwise.
const INSTANCE_CULL_DISABLE: u32 = 1;
const INSTANCE_FRONT_COUNTERCLOCKWISE: u32 = 2;

/// Why the shaders cannot trace the ray record `ray` as DXR defines it,
/// if they cannot: the host counts such rays and traces them another way.
/// DXR leaves these undefined or does not mention them (a direction of
/// zero length), or the shaders would have to drop what was asked.
pub fn ray_refusal(ray: &[u32; 12]) -> Option<&'static str> {
    let f = |k: usize| f32::from_bits(ray[k]);
    if (0..8).any(|k| f(k).is_nan()) {
        return Some("a NaN in the ray");
    }
    if (0..3).chain(4..7).any(|k| f(k).is_infinite()) {
        return Some("an infinite origin or direction");
    }
    // DXR says nothing of it; the software test would divide by zero.
    if (4..7).all(|k| f(k) == 0.0) {
        return Some("a direction of zero length");
    }
    let (tmin, tmax) = (f(3), f(7));
    if tmin < 0.0 || tmin > tmax {
        return Some("tmin negative or above tmax");
    }
    let flags = ray[9];
    if flags & !RAY_FLAGS != 0 {
        return Some("ray flags outside DXR 1.1's inline set");
    }
    let opacity = FORCE_OPAQUE | FORCE_NON_OPAQUE | CULL_OPAQUE | CULL_NON_OPAQUE;
    if (flags & opacity).count_ones() > 1 {
        return Some("more than one opacity flag");
    }
    if flags & CULL_BACK != 0 && flags & CULL_FRONT != 0 {
        return Some("both faces culled");
    }
    if flags & SKIP_TRIANGLES != 0 && flags & (CULL_BACK | CULL_FRONT | SKIP_PROCEDURAL) != 0 {
        return Some("triangles skipped together with a face cull or procedural skip");
    }
    None
}

/// The alpha rule of geometry not marked opaque (it stands in for an
/// alpha-tested material): a candidate at barycentrics `u`, `v` of
/// primitive `primitive` is accepted when `floor(8u) + floor(8v) +
/// primitive` is even. The floors are taken as floats, converted to signed
/// 32-bit integers and added, with the index, modulo 2^32.
pub fn alpha_accepts(u: f32, v: f32, primitive: u32) -> bool {
    let cell = |x: f32| (x * 8.0).floor() as i32;
    cell(u).wrapping_add(cell(v)).wrapping_add(primitive as i32) & 1 == 0
}

/// [`triangle_test`] could not decide: a difference, product or quotient on
/// the way was infinite or NaN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfRange;

/// Coordinates up to this magnitude (2^24), under a direction whose largest
/// component is between 2^-24 and 2^24, are always in [`triangle_test`]'s
/// range: the largest product on the way is below 2^105.
pub const EXACT_COORDINATE: f32 = 16_777_216.0;

/// What [`triangle_test`] finds where the ray's line meets the triangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriangleHit {
    /// Along the ray, in units of its direction; any sign.
    pub t: f32,
    /// The weights of the second and third vertex.
    pub u: f32,
    pub v: f32,
    /// The vertices appear clockwise from the ray's origin (a left-handed
    /// coordinate system): D3D12's front face unless the instance says
    /// counter-clockwise.
    pub clockwise: bool,
}

/// The ray's line against a triangle, exactly as [`Form::Procedural`]'s
/// shader computes it, in the same order of single-precision operations
/// (nothing fused): the watertight test of Woop, Benthin and Wald (JCGT
/// 2013) that the emulated triangle-node test uses (`lower::bvh`), the
/// same arithmetic written out again, followed by the three divisions by
/// the denominator. Edges and vertices are inside; a zero-area triangle and
/// a ray in the triangle's plane miss. No extent test: the caller compares
/// `t`.
///
/// The arithmetic is single precision's and has its range. Where the
/// direction's largest component, the denominator or the distance's
/// numerator is infinite or NaN (every such value on the way reaches one of
/// the three), or the triangle is met and the distance itself is infinite,
/// nothing was decided and the result is [`OutOfRange`]. A NaN or an
/// infinity anywhere in the inputs gives it, as does a direction of zero
/// length or too short for its reciprocal (below about 3e-39). So do finite
/// coordinates far enough from the origin that a product of two of them,
/// or of three over the direction's largest component, passes 2^128: about
/// 10^19 across the ray, about 10^12 on all three axes under a direction of
/// length 1. It is never given while the vertices' and the origin's
/// coordinates are at most [`EXACT_COORDINATE`] in magnitude and the
/// direction's largest between its reciprocal and it. A hit therefore
/// holds finite numbers, `u` and `v` from 0 to 1. The other end is not
/// refused: products below 2^-126 lose bits and below 2^-150 are 0, so a
/// triangle less than about 10^-19 across as the ray sees it can be
/// misjudged or lose its weights, and one below about 10^-23 reads as
/// zero-area and misses. The emulated node test has both ends too: it
/// computes the same products.
pub fn triangle_test(
    origin: [f32; 3],
    dir: [f32; 3],
    p: [[f32; 3]; 3],
) -> Result<Option<TriangleHit>, OutOfRange> {
    let ad = dir.map(f32::abs);
    let x_ge_y = ad[0] >= ad[1];
    let z0 = x_ge_y && ad[0] >= ad[2];
    let z1 = !x_ge_y && ad[1] >= ad[2];
    let by = |v: [f32; 3], k: [usize; 3]| {
        if z0 {
            v[k[0]]
        } else if z1 {
            v[k[1]]
        } else {
            v[k[2]]
        }
    };
    let neg = by(dir, [0, 1, 2]) < 0.0;
    let perm = |v: [f32; 3]| {
        let (x, y, z) = (by(v, [1, 2, 0]), by(v, [2, 0, 1]), by(v, [0, 1, 2]));
        if neg { [y, x, z] } else { [x, y, z] }
    };
    let d = perm(dir);
    let sz = 1.0 / d[2];
    let (sx, sy) = (d[0] * sz, d[1] * sz);
    let sheared = |v: [f32; 3]| {
        let r = perm(std::array::from_fn(|k| v[k] - origin[k]));
        [r[0] - sx * r[2], r[1] - sy * r[2], sz * r[2]]
    };
    let [a, b, c] = p.map(sheared);
    let edge = |p: [f32; 3], q: [f32; 3]| p[0] * q[1] - p[1] * q[0];
    let (u, v, w) = (edge(c, b), edge(a, c), edge(b, a));
    let inside = u >= 0.0 && v >= 0.0 && w >= 0.0 || u <= 0.0 && v <= 0.0 && w <= 0.0;
    let det = u + (v + w);
    let tnum = (u * a[2] + v * b[2]) + w * c[2];
    let finite = |x: f32| x.abs() < f32::INFINITY;
    if !(finite(d[2]) && finite(det) && finite(tnum)) {
        return Err(OutOfRange);
    }
    let (clockwise, counter) = (det > 0.0, det < 0.0);
    if !(inside && (clockwise || counter)) {
        return Ok(None);
    }
    let t = tnum / det;
    if !finite(t) {
        return Err(OutOfRange);
    }
    Ok(Some(TriangleHit {
        t,
        u: v / det,
        v: w / det,
        clockwise,
    }))
}

/// Which of the prototype's shaders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// The device intersects triangles. The top-level structure holds
    /// triangle geometry only; each candidate of geometry not marked
    /// opaque goes through [`alpha_accepts`] and is committed or left.
    /// (A procedural primitive the ray's flags do not skip makes the record
    /// a refusal, [`REFUSED_GEOMETRY`]; so does a triangle in the other
    /// form's scene.)
    Triangles,
    /// The hybrid: the device walks the structure, the shader intersects.
    /// The same scene with one axis-aligned box around each triangle
    /// (procedural geometry; same instances, geometry order and primitive
    /// order), the triangles themselves in the triangle data. The shader
    /// runs [`triangle_test`] on the candidate's triangle with the
    /// object-space ray the device gives, and commits a hit that lies
    /// strictly between tmin and the committed t (the triangle rule; DXR's
    /// procedural rule is inclusive), survives the ray's face-culling and
    /// skip flags as the instance's flags have them, and, when the device
    /// reports the candidate non-opaque, [`alpha_accepts`]. A candidate
    /// whose test is [`OutOfRange`] is not committed and the ray's record
    /// is a refusal ([`REFUSED_RANGE`]).
    Procedural,
    /// No trace: every listed thread writes a miss. It binds and reads no
    /// ray record, so its time is the dispatch, the count's read and the
    /// hit writes; a tracing form's time less this one's is the ray
    /// record's loads, the trace and the candidates together.
    Null,
}

/// What a binding of a prototype shader holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    /// Constants, 16 bytes (root constants or a constant buffer): dword 0
    /// the number of rays, the rest unread.
    Constants,
    /// The ray records, a raw buffer read: [`RAY_BYTES`] × the ray count.
    Rays,
    /// [`Form::Procedural`]'s triangle data, a raw buffer read, offsets in
    /// bytes from its start: for instance `i` of the top-level list two
    /// dwords at `8 × i`, the offset of its geometry table and its
    /// `D3D12_RAYTRACING_INSTANCE_FLAGS`; in a geometry table a dword per
    /// geometry, the offset of its triangle 0; a triangle is nine floats
    /// (three vertices, x y z) at 36 × its primitive index from there, in
    /// the object space its box was made in. Instances of one bottom-level
    /// structure can share a table.
    Triangles,
    /// The top-level acceleration structure.
    Scene,
    /// The hit records, a raw buffer written: [`HIT_BYTES`] × the ray
    /// count.
    Hits,
}

/// How the host binds a [`Resource`]: the register class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `b`: a constant buffer view or root constants.
    Constants,
    /// `t`: a raw buffer (`ByteAddressBuffer`).
    RawBuffer,
    /// `u`: a raw buffer written (`RWByteAddressBuffer`).
    RawBufferWritten,
    /// `t`: `RaytracingAccelerationStructure`. As a root SRV its address is
    /// the top-level structure's; as a descriptor it is an SRV of view
    /// dimension acceleration structure.
    AccelerationStructure,
}

/// One binding of a prototype shader. Each may be a root descriptor (a
/// root SRV, UAV or CBV, or root constants for [`Class::Constants`]) or a
/// descriptor in a table. A root descriptor has no bounds: the shaders
/// stay inside the ray count, and inside the triangle data only as far as
/// its own offsets do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub resource: Resource,
    pub class: Class,
    pub register: u32,
    pub space: u32,
}

/// A prototype shader and what the host binds for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RayQueryShader {
    pub form: Form,
    /// The DXIL container (`cs_6_6`), hashed as D3D12 expects. The device
    /// must report shader model 6.6, and for [`Form::Triangles`] and
    /// [`Form::Procedural`] `D3D12_RAYTRACING_TIER_1_1`.
    pub dxil: Vec<u8>,
    /// The thread-group size.
    pub threads: [u32; 3],
    /// The bindings, in the order the module makes their handles.
    pub bindings: Vec<Binding>,
}

/// The registers: rays `t0` and triangle data `t1` (space
/// [`ROOT_CBV_SPACE`]), hits `u0` there, the scene `t0` in [`ACCEL_SPACE`].
const RAYS: u8 = 0;
const TRIANGLES: u8 = 1;
const HITS: u8 = 0;
const SCENE: u32 = 0;

/// The shader `form`.
pub fn build(form: Form) -> RayQueryShader {
    let built = assemble(form);
    RayQueryShader {
        form,
        dxil: built.s.finish(),
        threads: [GROUP_THREADS, 1, 1],
        bindings: built.bindings,
    }
}

/// A shader before it is written.
pub(crate) struct Built {
    pub s: Shader,
    pub bindings: Vec<Binding>,
}

/// A block being filled: the instructions under short names.
struct At<'a> {
    s: &'a mut Shader,
    b: usize,
}

impl At<'_> {
    fn i(&mut self, v: u32) -> V {
        self.s.m.i32(v)
    }

    fn f(&mut self, v: f32) -> V {
        self.s.m.f32(v.to_bits())
    }

    fn bin(&mut self, op: Bin, a: V, b: V) -> V {
        self.s.m.bin(self.b, op, a, b)
    }

    fn cmp(&mut self, p: u8, a: V, b: V) -> V {
        self.s.m.cmp(self.b, p, a, b)
    }

    fn select(&mut self, c: V, t: V, f: V) -> V {
        self.s.m.select(self.b, c, t, f)
    }

    /// A float's bits.
    fn bits(&mut self, f: V) -> V {
        self.s.m.cast(self.b, Cast::Bitcast, f, self.s.t.i32)
    }

    /// The float of these bits.
    fn float(&mut self, i: V) -> V {
        self.s.m.cast(self.b, Cast::Bitcast, i, self.s.t.f32)
    }

    fn not(&mut self, c: V) -> V {
        let t = self.s.m.bool(true);
        self.bin(Bin::Xor, c, t)
    }

    /// A float without its sign.
    fn abs(&mut self, f: V) -> V {
        let i = self.bits(f);
        let i = self.masked(i, 0x7FFF_FFFF);
        self.float(i)
    }

    /// Whether a float is neither infinite nor NaN.
    fn finite(&mut self, f: V) -> V {
        let (a, inf) = (self.abs(f), self.f(f32::INFINITY));
        self.cmp(pred::OLT, a, inf)
    }

    fn masked(&mut self, x: V, mask: u32) -> V {
        let m = self.i(mask);
        self.bin(Bin::And, x, m)
    }

    /// Whether `x` has a bit of `mask`.
    fn has(&mut self, x: V, mask: u32) -> V {
        let (x, zero) = (self.masked(x, mask), self.i(0));
        self.cmp(pred::NE, x, zero)
    }

    fn ray(&mut self, op: &RayOp, args: &[V]) -> V {
        self.s.ray_op(self.b, op, args)
    }

    /// The three components of a vector the query `q` holds.
    fn ray_vector(&mut self, op: &RayOp, q: V) -> [V; 3] {
        std::array::from_fn(|k| {
            let k = self.s.m.i8(k as u8);
            self.ray(op, &[q, k])
        })
    }

    /// `n` (1 to 4) dwords at byte `offset` of the raw buffer `h`, as floats.
    fn load(&mut self, h: V, offset: V, n: u32) -> Vec<V> {
        let (u, align) = (self.s.m.undef(self.s.t.i32), self.i(4));
        let mask = self.s.m.i8((1u8 << n) - 1);
        let r = self.s.op(
            self.b,
            "rawBufferLoad.f32",
            Attr::ReadOnly,
            self.s.t.res_ret,
            139,
            &[h, offset, u, mask, align],
        );
        (0..n).map(|k| self.s.m.extract(self.b, r, k)).collect()
    }

    /// A hit record (`words`: its eight dwords) at byte `offset` of `hits`.
    fn store_hit(&mut self, hits: V, offset: V, words: [V; 8]) {
        let (u, align, void) = (self.s.m.undef(self.s.t.i32), self.i(4), self.s.t.void);
        let mask = self.s.m.i8(0xF);
        for (k, four) in words.chunks(4).enumerate() {
            let at = self.i(16 * k as u32);
            let at = self.bin(Bin::Add, offset, at);
            let args = [hits, at, u, four[0], four[1], four[2], four[3], mask, align];
            self.s.op(
                self.b,
                "rawBufferStore.i32",
                Attr::NoUnwind,
                void,
                140,
                &args,
            );
        }
    }

    fn end(&mut self, t: Term) {
        self.s.m.terminate(self.b, t);
    }
}

/// What the shader's test of a triangle leaves ([`triangle_test`]).
struct Tested {
    /// The line meets the triangle (`Ok(Some)` there).
    meets: V,
    /// Nothing was decided ([`OutOfRange`] there).
    refused: V,
    t: V,
    u: V,
    v: V,
    clockwise: V,
}

impl At<'_> {
    /// [`triangle_test`] of the ray `origin`, `dir` and the triangle `p`,
    /// instruction for operation; where it finds no hit, `t`, `u` and `v`
    /// are 0, so nothing NaN or infinite leaves it.
    fn triangle_test(&mut self, origin: [V; 3], dir: [V; 3], p: [[V; 3]; 3]) -> Tested {
        let zero = self.f(0.0);
        let ad = dir.map(|d| self.abs(d));
        let x_ge_y = self.cmp(pred::OGE, ad[0], ad[1]);
        let x_ge_z = self.cmp(pred::OGE, ad[0], ad[2]);
        let y_ge_z = self.cmp(pred::OGE, ad[1], ad[2]);
        let z0 = self.bin(Bin::And, x_ge_y, x_ge_z);
        let y_gt_x = self.not(x_ge_y);
        let z1 = self.bin(Bin::And, y_gt_x, y_ge_z);
        let by = |at: &mut Self, v: [V; 3], k: [usize; 3]| {
            let x = at.select(z1, v[k[1]], v[k[2]]);
            at.select(z0, v[k[0]], x)
        };
        let dz = by(self, dir, [0, 1, 2]);
        let neg = self.cmp(pred::OLT, dz, zero);
        let perm = |at: &mut Self, v: [V; 3]| -> [V; 3] {
            let x = by(at, v, [1, 2, 0]);
            let y = by(at, v, [2, 0, 1]);
            let z = by(at, v, [0, 1, 2]);
            [at.select(neg, y, x), at.select(neg, x, y), z]
        };
        let d = perm(self, dir);
        let one = self.f(1.0);
        // (The float division shares the signed one's code.)
        let sz = self.bin(Bin::SDiv, one, d[2]);
        let sx = self.bin(Bin::Mul, d[0], sz);
        let sy = self.bin(Bin::Mul, d[1], sz);
        let sheared = |at: &mut Self, v: [V; 3]| -> [V; 3] {
            let rel: [V; 3] = std::array::from_fn(|k| at.bin(Bin::Sub, v[k], origin[k]));
            let r = perm(at, rel);
            let tx = at.bin(Bin::Mul, sx, r[2]);
            let ty = at.bin(Bin::Mul, sy, r[2]);
            [
                at.bin(Bin::Sub, r[0], tx),
                at.bin(Bin::Sub, r[1], ty),
                at.bin(Bin::Mul, sz, r[2]),
            ]
        };
        let [a, b, c] = p.map(|v| sheared(self, v));
        let edge = |at: &mut Self, p: [V; 3], q: [V; 3]| {
            let m = at.bin(Bin::Mul, p[0], q[1]);
            let n = at.bin(Bin::Mul, p[1], q[0]);
            at.bin(Bin::Sub, m, n)
        };
        let u = edge(self, c, b);
        let v = edge(self, a, c);
        let w = edge(self, b, a);
        let all = |at: &mut Self, p: u8| {
            let x = at.cmp(p, u, zero);
            let y = at.cmp(p, v, zero);
            let z = at.cmp(p, w, zero);
            let xy = at.bin(Bin::And, x, y);
            at.bin(Bin::And, xy, z)
        };
        let (ge, le) = (all(self, pred::OGE), all(self, pred::OLE));
        let inside = self.bin(Bin::Or, ge, le);
        let vw = self.bin(Bin::Add, v, w);
        let det = self.bin(Bin::Add, u, vw);
        let ua = self.bin(Bin::Mul, u, a[2]);
        let vb = self.bin(Bin::Mul, v, b[2]);
        let wc = self.bin(Bin::Mul, w, c[2]);
        let uv = self.bin(Bin::Add, ua, vb);
        let tnum = self.bin(Bin::Add, uv, wc);
        let clockwise = self.cmp(pred::OGT, det, zero);
        let counter = self.cmp(pred::OLT, det, zero);
        let either = self.bin(Bin::Or, clockwise, counter);
        let met = self.bin(Bin::And, inside, either);
        // In range: the direction's largest component and the two sums
        // finite, and the distance where the triangle is met. A quotient
        // where there is no hit (0/0, x/0, NaN) is compared at most, and
        // not read.
        let dz_finite = self.finite(d[2]);
        let det_finite = self.finite(det);
        let tnum_finite = self.finite(tnum);
        let sums = self.bin(Bin::And, dz_finite, det_finite);
        let sums = self.bin(Bin::And, sums, tnum_finite);
        let t = self.bin(Bin::SDiv, tnum, det);
        let t_finite = self.finite(t);
        let unmet = self.not(met);
        let t_decided = self.bin(Bin::Or, t_finite, unmet);
        let decided = self.bin(Bin::And, sums, t_decided);
        let meets = self.bin(Bin::And, met, decided);
        let over = |at: &mut Self, x: V| {
            let q = at.bin(Bin::SDiv, x, det);
            at.select(meets, q, zero)
        };
        Tested {
            meets,
            refused: self.not(decided),
            t: self.select(meets, t, zero),
            u: over(self, v),
            v: over(self, w),
            clockwise,
        }
    }

    /// [`alpha_accepts`].
    fn alpha_accepts(&mut self, u: V, v: V, primitive: V) -> V {
        let eight = self.f(8.0);
        let cell = |at: &mut Self, x: V| {
            let x = at.bin(Bin::Mul, x, eight);
            // Round_ni: to the integer below.
            let f32t = at.s.t.f32;
            let x = at.s.op(at.b, "unary.f32", Attr::ReadNone, f32t, 27, &[x]);
            at.s.m.cast(at.b, Cast::FpToSi, x, at.s.t.i32)
        };
        let (cu, cv) = (cell(self, u), cell(self, v));
        let sum = self.bin(Bin::Add, cu, cv);
        let sum = self.bin(Bin::Add, sum, primitive);
        let (odd, zero) = (self.masked(sum, 1), self.i(0));
        self.cmp(pred::EQ, odd, zero)
    }
}

/// The shader `form`, unwritten.
pub(crate) fn assemble(form: Form) -> Built {
    let mut s = Shader::new(false);
    s.threads = Some([GROUP_THREADS, 1, 1]);
    // Dword 0: the number of rays.
    s.user_data = 1;
    let traces = form != Form::Null;
    let mut bindings = Vec::new();
    let mut bind = |resource, class, register, space| {
        bindings.push(Binding {
            resource,
            class,
            register,
            space,
        });
    };
    let constants = s.user_data_handle();
    bind(Resource::Constants, Class::Constants, 0, USER_DATA_SPACE);
    let rays = traces.then(|| s.root_srv_handle(RAYS));
    if traces {
        let register = u32::from(RAYS);
        bind(Resource::Rays, Class::RawBuffer, register, ROOT_CBV_SPACE);
    }
    let triangles = (form == Form::Procedural).then(|| s.root_srv_handle(TRIANGLES));
    if triangles.is_some() {
        let register = u32::from(TRIANGLES);
        bind(
            Resource::Triangles,
            Class::RawBuffer,
            register,
            ROOT_CBV_SPACE,
        );
    }
    let scene = traces.then(|| s.accel_handle(SCENE));
    if traces {
        bind(
            Resource::Scene,
            Class::AccelerationStructure,
            SCENE,
            ACCEL_SPACE,
        );
    }
    let hits = s.root_uav_handle(HITS);
    let register = u32::from(HITS);
    bind(
        Resource::Hits,
        Class::RawBufferWritten,
        register,
        ROOT_CBV_SPACE,
    );

    // The entry: threads past the ray list do nothing.
    let entry = s.entry;
    let (body, exit) = (s.m.new_block(), s.m.new_block());
    let zero = s.m.i32(0);
    let i32t = s.t.i32;
    let thread = s.op(entry, "threadId.i32", Attr::ReadNone, i32t, 93, &[zero]);
    let row = s.op(
        entry,
        "cbufferLoadLegacy.i32",
        Attr::ReadOnly,
        s.t.cbuf_ret,
        59,
        &[constants, zero],
    );
    let count = s.m.extract(entry, row, 0);
    let listed = s.m.cmp(entry, pred::IULT, thread, count);
    s.m.terminate(entry, Term::CondBr(listed, body, exit));
    s.m.terminate(exit, Term::Ret);

    let mut at = At { s: &mut s, b: body };
    let hit_bytes = at.i(HIT_BYTES);
    let hit_at = at.bin(Bin::Mul, thread, hit_bytes);
    let (Some(rays), Some(scene)) = (rays, scene) else {
        at.store_hit(hits, hit_at, [zero; 8]);
        at.end(Term::Br(exit));
        return Built { s, bindings };
    };

    // The ray, and the trace.
    let ray_bytes = at.i(RAY_BYTES);
    let ray_at = at.bin(Bin::Mul, thread, ray_bytes);
    let mut ray = Vec::new();
    for (k, n) in [(0, 4), (16, 4), (32, 2)] {
        let k = at.i(k);
        let from = at.bin(Bin::Add, ray_at, k);
        ray.extend(at.load(rays, from, n));
    }
    let tmin = ray[3];
    let mask = at.bits(ray[8]);
    let mask = at.masked(mask, 0xFF);
    let flags = at.bits(ray[9]);
    let passed = match form {
        Form::Procedural => PROCEDURAL_RAY_FLAGS,
        _ => RAY_FLAGS,
    };
    let passed = at.masked(flags, passed);
    let none = at.i(0);
    let q = at.ray(&op::ALLOCATE, &[none]);
    let mut trace = vec![q, scene, passed, mask];
    trace.extend(&ray[..8]);
    at.ray(&op::TRACE_RAY_INLINE, &trace);
    // What the procedural form decides itself, from the ray's flags.
    let culls = triangles.map(|_| {
        [
            at.has(flags, CULL_BACK),
            at.has(flags, CULL_FRONT),
            at.has(flags, SKIP_TRIANGLES),
        ]
    });

    // The candidates: `while (q.Proceed())` as a rotated loop, the shape
    // the module writer's notes ask of loops: a guard, a preheader, the
    // test in the latch (the one exiting block) and an exit of its own.
    let [
        pre,
        head,
        test,
        commit,
        latch,
        out,
        done,
        decided,
        hit,
        miss,
        refuse,
    ] = std::array::from_fn(|_| at.s.m.new_block());
    let first = at.ray(&op::PROCEED, &[q]);
    at.end(Term::CondBr(first, pre, done));
    at.b = pre;
    at.end(Term::Br(head));

    at.b = head;
    let f32t = at.s.t.f32;
    let fzero = at.f(0.0);
    // The procedural form keeps the committed hit's barycentrics: the
    // device keeps only its t.
    let kept: Option<[V; 2]> = triangles.map(|_| [at.s.m.phi(head, f32t), at.s.m.phi(head, f32t)]);
    // Why the ray is refused, so far: `REFUSED_*` bits, 0 while it is not.
    let why = at.s.m.phi(head, i32t);
    let candidate = at.ray(&op::CANDIDATE_TYPE, &[q]);
    let wanted = at.i(u32::from(triangles.is_some()));
    let ours = at.cmp(pred::EQ, candidate, wanted);
    // A candidate of the other geometry type is left, and remembered.
    let geometry = at.i(REFUSED_GEOMETRY);
    let why_other = at.bin(Bin::Or, why, geometry);
    at.end(Term::CondBr(ours, test, latch));

    at.b = test;
    let mut found = None;
    let mut why_tested = why;
    let accept = match (triangles, culls) {
        (Some(data), Some([cull_back, cull_front, skip])) => {
            // The candidate's triangle, through the instance's row and
            // its geometry table.
            let instance = at.ray(&op::CANDIDATE_INSTANCE_INDEX, &[q]);
            let geometry = at.ray(&op::CANDIDATE_GEOMETRY_INDEX, &[q]);
            let primitive = at.ray(&op::CANDIDATE_PRIMITIVE_INDEX, &[q]);
            let eight = at.i(8);
            let row = at.bin(Bin::Mul, instance, eight);
            let row = at.load(data, row, 2);
            let (table, instance_flags) = (at.bits(row[0]), at.bits(row[1]));
            let four = at.i(4);
            let entry = at.bin(Bin::Mul, geometry, four);
            let entry = at.bin(Bin::Add, table, entry);
            let first = at.load(data, entry, 1)[0];
            let first = at.bits(first);
            let bytes = at.i(36);
            let within = at.bin(Bin::Mul, primitive, bytes);
            let from = at.bin(Bin::Add, first, within);
            let p: [[V; 3]; 3] = std::array::from_fn(|k| {
                let k = at.i(12 * k as u32);
                let from = at.bin(Bin::Add, from, k);
                let v = at.load(data, from, 3);
                [v[0], v[1], v[2]]
            });
            let origin = at.ray_vector(&op::CANDIDATE_OBJECT_RAY_ORIGIN, q);
            let dir = at.ray_vector(&op::CANDIDATE_OBJECT_RAY_DIRECTION, q);
            let r = at.triangle_test(origin, dir, p);
            // Between tmin and the closest hit so far, both excluded.
            let closest = at.ray(&op::COMMITTED_RAY_T, &[q]);
            let after = at.cmp(pred::OGT, r.t, tmin);
            let before = at.cmp(pred::OLT, r.t, closest);
            let between = at.bin(Bin::And, after, before);
            // Face culling, unless the instance turns it off; a skip of
            // triangles regardless.
            let counter = at.has(instance_flags, INSTANCE_FRONT_COUNTERCLOCKWISE);
            let front = at.bin(Bin::Xor, r.clockwise, counter);
            let back = at.not(front);
            let back_culled = at.bin(Bin::And, cull_back, back);
            let front_culled = at.bin(Bin::And, cull_front, front);
            let face = at.bin(Bin::Or, back_culled, front_culled);
            let disabled = at.has(instance_flags, INSTANCE_CULL_DISABLE);
            let enabled = at.not(disabled);
            let face = at.bin(Bin::And, face, enabled);
            let culled = at.bin(Bin::Or, face, skip);
            let kept = at.not(culled);
            // The alpha rule where the device has the primitive non-opaque.
            let alpha = at.alpha_accepts(r.u, r.v, primitive);
            let non_opaque = at.ray(&op::CANDIDATE_PROCEDURAL_PRIMITIVE_NON_OPAQUE, &[q]);
            let opaque = at.not(non_opaque);
            let solid = at.bin(Bin::Or, opaque, alpha);
            let accept = at.bin(Bin::And, r.meets, between);
            let accept = at.bin(Bin::And, accept, kept);
            // A test out of range decides nothing: remembered.
            let range = at.i(REFUSED_RANGE);
            let range = at.select(r.refused, range, zero);
            why_tested = at.bin(Bin::Or, why, range);
            found = Some(r);
            at.bin(Bin::And, accept, solid)
        }
        _ => {
            let u = at.ray_vector(&op::CANDIDATE_TRIANGLE_BARYCENTRICS, q);
            let primitive = at.ray(&op::CANDIDATE_PRIMITIVE_INDEX, &[q]);
            at.alpha_accepts(u[0], u[1], primitive)
        }
    };
    at.end(Term::CondBr(accept, commit, latch));

    at.b = commit;
    match &found {
        Some(r) => at.ray(&op::COMMIT_PROCEDURAL_PRIMITIVE_HIT, &[q, r.t]),
        None => at.ray(&op::COMMIT_NON_OPAQUE_TRIANGLE_HIT, &[q]),
    };
    at.end(Term::Br(latch));

    at.b = latch;
    let next: Option<[V; 2]> = kept.map(|_| [at.s.m.phi(latch, f32t), at.s.m.phi(latch, f32t)]);
    let why_next = at.s.m.phi(latch, i32t);
    let more = at.ray(&op::PROCEED, &[q]);
    at.end(Term::CondBr(more, head, out));
    at.b = out;
    at.end(Term::Br(done));

    at.b = done;
    let last: Option<[V; 2]> = kept.map(|_| [at.s.m.phi(done, f32t), at.s.m.phi(done, f32t)]);
    let why_last = at.s.m.phi(done, i32t);
    at.s.m.set_phi(why, vec![(zero, pre), (why_next, latch)]);
    at.s.m.set_phi(
        why_next,
        vec![(why_other, head), (why_tested, test), (why_tested, commit)],
    );
    at.s.m
        .set_phi(why_last, vec![(zero, body), (why_next, out)]);
    if let (Some(kept), Some(next), Some(last), Some(r)) = (kept, next, last, &found) {
        for (k, new) in [r.u, r.v].into_iter().enumerate() {
            at.s.m
                .set_phi(kept[k], vec![(fzero, pre), (next[k], latch)]);
            let unchanged = [(kept[k], head), (kept[k], test)];
            at.s.m.set_phi(
                next[k],
                unchanged.into_iter().chain([(new, commit)]).collect(),
            );
            at.s.m.set_phi(last[k], vec![(fzero, body), (next[k], out)]);
        }
    }
    let status = at.ray(&op::COMMITTED_STATUS, &[q]);
    let committed = at.i(1 + u32::from(triangles.is_some()));
    let is_hit = at.cmp(pred::EQ, status, committed);
    // A hit of the other geometry type, which only the device commits.
    let something = at.cmp(pred::NE, status, zero);
    let not_ours = at.not(is_hit);
    let stray = at.bin(Bin::And, something, not_ours);
    let stray = at.select(stray, geometry, zero);
    let why = at.bin(Bin::Or, why_last, stray);
    let refused = at.cmp(pred::NE, why, zero);
    at.end(Term::CondBr(refused, refuse, decided));

    // Refused: the reasons in the kind, nothing else.
    at.b = refuse;
    let flag = at.i(HIT_REFUSED);
    let kind = at.bin(Bin::Or, why, flag);
    at.store_hit(
        hits,
        hit_at,
        [zero, zero, zero, kind, zero, zero, zero, zero],
    );
    at.end(Term::Br(exit));

    at.b = decided;
    at.end(Term::CondBr(is_hit, hit, miss));

    at.b = hit;
    let t = at.ray(&op::COMMITTED_RAY_T, &[q]);
    let (u, v, kind) = match last {
        Some([u, v]) => (u, v, at.i(HIT_PROCEDURAL)),
        None => {
            let u = at.ray_vector(&op::COMMITTED_TRIANGLE_BARYCENTRICS, q);
            let front = at.ray(&op::COMMITTED_TRIANGLE_FRONT_FACE, &[q]);
            let (yes, no) = (at.i(HIT_FRONT), at.i(HIT_BACK));
            (u[0], u[1], at.select(front, yes, no))
        }
    };
    let [t, u, v] = [t, u, v].map(|x| at.bits(x));
    let place = [
        op::COMMITTED_INSTANCE_INDEX,
        op::COMMITTED_INSTANCE_ID,
        op::COMMITTED_GEOMETRY_INDEX,
        op::COMMITTED_PRIMITIVE_INDEX,
    ]
    .map(|op| at.ray(&op, &[q]));
    let record = [t, u, v, kind, place[0], place[1], place[2], place[3]];
    at.store_hit(hits, hit_at, record);
    at.end(Term::Br(exit));

    at.b = miss;
    at.store_hit(hits, hit_at, [zero; 8]);
    at.end(Term::Br(exit));
    Built { s, bindings }
}
