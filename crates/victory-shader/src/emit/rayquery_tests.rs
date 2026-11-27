//! Host tests of `rayquery`: the operation table against DXC's, the three
//! shaders' structure and containers, and the shaders themselves run one
//! thread at a time by the module interpreter ([`Flow::run`]) against a
//! model of a device's inline ray query. The model is this file's reading
//! of the DXR specification, not a device: what it shows is that the
//! emitted instructions do what the module's notes say, word for word.

use super::bitcode::{Arg, Flow, FlowOp, FlowTerm};
use super::container;
use super::rayquery::{
    Binding, Class, EXACT_COORDINATE, Form, GROUP_THREADS, HIT_BACK, HIT_BYTES, HIT_FRONT,
    HIT_MISS, HIT_PROCEDURAL, HIT_REFUSED, MAX_RAYS, OPS, OutOfRange, RAY_BYTES, RAY_FLAGS,
    REFUSED_GEOMETRY, REFUSED_RANGE, Resource, TriangleHit, Type, alpha_accepts, assemble, build,
    groups, op, ray_refusal, triangle_test,
};
use super::shader::{ACCEL_SPACE, ROOT_CBV_SPACE, Shader, USER_DATA_SPACE};
use crate::dxbc::{Container, FourCC};

/// The inline ray-query operations as DirectXShaderCompiler declares them:
/// name and opcode (`DxilConstants.h`, `enum class OpCode`), then the
/// declaration its `OP::GetOpFunc` makes (`DxilOperations.cpp`: the class
/// name and attribute of the property table, the overload type of its mask
/// (0x2 float, 0x8 i1, 0x40 i32; none: no suffix), the types of the
/// `GetOpFunc` switch, `nounwind` always). Listed by a script from those
/// two files as fetched 2026-11-27, not typed.
const DXC: [(&str, u32, &str); 37] = [
    (
        "AllocateRayQuery",
        178,
        "i32 @dx.op.allocateRayQuery(i32, i32) nounwind",
    ),
    (
        "RayQuery_TraceRayInline",
        179,
        "void @dx.op.rayQuery_TraceRayInline(i32, i32, %dx.types.Handle, i32, i32, float, float, float, float, float, float, float, float) nounwind",
    ),
    (
        "RayQuery_Proceed",
        180,
        "i1 @dx.op.rayQuery_Proceed.i1(i32, i32) nounwind",
    ),
    (
        "RayQuery_Abort",
        181,
        "void @dx.op.rayQuery_Abort(i32, i32) nounwind",
    ),
    (
        "RayQuery_CommitNonOpaqueTriangleHit",
        182,
        "void @dx.op.rayQuery_CommitNonOpaqueTriangleHit(i32, i32) nounwind",
    ),
    (
        "RayQuery_CommitProceduralPrimitiveHit",
        183,
        "void @dx.op.rayQuery_CommitProceduralPrimitiveHit(i32, i32, float) nounwind",
    ),
    (
        "RayQuery_CommittedStatus",
        184,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateType",
        185,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateObjectToWorld3x4",
        186,
        "float @dx.op.rayQuery_StateMatrix.f32(i32, i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CandidateWorldToObject3x4",
        187,
        "float @dx.op.rayQuery_StateMatrix.f32(i32, i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CommittedObjectToWorld3x4",
        188,
        "float @dx.op.rayQuery_StateMatrix.f32(i32, i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CommittedWorldToObject3x4",
        189,
        "float @dx.op.rayQuery_StateMatrix.f32(i32, i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CandidateProceduralPrimitiveNonOpaque",
        190,
        "i1 @dx.op.rayQuery_StateScalar.i1(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateTriangleFrontFace",
        191,
        "i1 @dx.op.rayQuery_StateScalar.i1(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CommittedTriangleFrontFace",
        192,
        "i1 @dx.op.rayQuery_StateScalar.i1(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateTriangleBarycentrics",
        193,
        "float @dx.op.rayQuery_StateVector.f32(i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CommittedTriangleBarycentrics",
        194,
        "float @dx.op.rayQuery_StateVector.f32(i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_RayFlags",
        195,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_WorldRayOrigin",
        196,
        "float @dx.op.rayQuery_StateVector.f32(i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_WorldRayDirection",
        197,
        "float @dx.op.rayQuery_StateVector.f32(i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_RayTMin",
        198,
        "float @dx.op.rayQuery_StateScalar.f32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateTriangleRayT",
        199,
        "float @dx.op.rayQuery_StateScalar.f32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CommittedRayT",
        200,
        "float @dx.op.rayQuery_StateScalar.f32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateInstanceIndex",
        201,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateInstanceID",
        202,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateGeometryIndex",
        203,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidatePrimitiveIndex",
        204,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CandidateObjectRayOrigin",
        205,
        "float @dx.op.rayQuery_StateVector.f32(i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CandidateObjectRayDirection",
        206,
        "float @dx.op.rayQuery_StateVector.f32(i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CommittedInstanceIndex",
        207,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CommittedInstanceID",
        208,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CommittedGeometryIndex",
        209,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CommittedPrimitiveIndex",
        210,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CommittedObjectRayOrigin",
        211,
        "float @dx.op.rayQuery_StateVector.f32(i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CommittedObjectRayDirection",
        212,
        "float @dx.op.rayQuery_StateVector.f32(i32, i32, i8) nounwind readonly",
    ),
    (
        "RayQuery_CandidateInstanceContributionToHitGroupIndex",
        214,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
    (
        "RayQuery_CommittedInstanceContributionToHitGroupIndex",
        215,
        "i32 @dx.op.rayQuery_StateScalar.i32(i32, i32) nounwind readonly",
    ),
];

/// Every operation, called once, declares the function DXC declares for it.
#[test]
fn the_operations_are_declared_as_dxc_declares_them() {
    let mut s = Shader::new(false);
    let b = s.entry;
    let scene = s.accel_handle(0);
    let none = s.m.i32(0);
    let q = s.ray_op(b, &op::ALLOCATE, &[none]);
    for (op, (name, opcode, declaration)) in OPS.iter().zip(DXC) {
        assert_eq!((op.name, op.opcode), (name, opcode));
        if op.opcode != op::ALLOCATE.opcode {
            let mut args = vec![q];
            for &p in &op.params[1..] {
                args.push(match p {
                    Type::I8 => s.m.i8(0),
                    Type::I32 => s.m.i32(0),
                    Type::F32 => s.m.f32(0),
                    Type::Handle => scene,
                    Type::Void | Type::I1 => unreachable!(),
                });
            }
            s.ray_op(b, op, &args);
        }
        let declared = s.m.declaration(&format!("dx.op.{}", op.function));
        assert_eq!(declared.as_deref(), Some(declaration), "{name}");
    }
    // The opcodes rise, and are the group's 178 to 215 but GeometryIndex.
    let opcodes: Vec<u32> = OPS.iter().map(|op| op.opcode).collect();
    let group: Vec<u32> = (178..=215).filter(|&n| n != 213).collect();
    assert_eq!(opcodes, group);
}

#[test]
#[should_panic(expected = "the arguments of RayQuery_CommitProceduralPrimitiveHit")]
fn an_operation_refuses_arguments_of_other_types() {
    let mut s = Shader::new(false);
    let none = s.m.i32(0);
    let q = s.ray_op(s.entry, &op::ALLOCATE, &[none]);
    // The hit's t as an integer.
    s.ray_op(s.entry, &op::COMMIT_PROCEDURAL_PRIMITIVE_HIT, &[q, none]);
}

#[test]
#[should_panic(expected = "constant ray flags")]
fn a_query_is_allocated_with_constant_flags_only() {
    let mut s = Shader::new(false);
    let zero = s.m.i32(0);
    let i32t = s.t.i32;
    let thread = s.op(
        s.entry,
        "threadId.i32",
        super::bitcode::Attr::ReadNone,
        i32t,
        93,
        &[zero],
    );
    s.ray_op(s.entry, &op::ALLOCATE, &[thread]);
}

const FORMS: [Form; 3] = [Form::Triangles, Form::Procedural, Form::Null];

fn binding(resource: Resource, class: Class, register: u32, space: u32) -> Binding {
    Binding {
        resource,
        class,
        register,
        space,
    }
}

#[test]
fn the_bindings_are_what_each_form_reads() {
    let constants = binding(Resource::Constants, Class::Constants, 0, USER_DATA_SPACE);
    let rays = binding(Resource::Rays, Class::RawBuffer, 0, ROOT_CBV_SPACE);
    let triangles = binding(Resource::Triangles, Class::RawBuffer, 1, ROOT_CBV_SPACE);
    let scene = binding(
        Resource::Scene,
        Class::AccelerationStructure,
        0,
        ACCEL_SPACE,
    );
    let hits = binding(Resource::Hits, Class::RawBufferWritten, 0, ROOT_CBV_SPACE);
    assert_eq!(
        build(Form::Triangles).bindings,
        [constants, rays, scene, hits]
    );
    assert_eq!(
        build(Form::Procedural).bindings,
        [constants, rays, triangles, scene, hits]
    );
    assert_eq!(build(Form::Null).bindings, [constants, hits]);
    for form in FORMS {
        assert_eq!(build(form).threads, [GROUP_THREADS, 1, 1]);
    }
    assert_eq!(
        [groups(0), groups(1), groups(64), groups(65)],
        [Some(0), Some(1), Some(1), Some(2)]
    );
    // D3D12's 65,535 groups a dimension: a longer list is not one dispatch.
    assert_eq!(MAX_RAYS, 4_194_240);
    assert_eq!(groups(MAX_RAYS - 1), Some(65_535));
    assert_eq!(groups(MAX_RAYS), Some(65_535));
    assert_eq!([groups(MAX_RAYS + 1), groups(u32::MAX)], [None, None]);
}

/// The same bytes each time; the forms differ.
#[test]
fn the_shaders_are_written_the_same_each_time() {
    let all = FORMS.map(|form| build(form).dxil);
    assert_eq!(all, FORMS.map(|form| build(form).dxil));
    assert!(all[0] != all[1] && all[1] != all[2] && all[0] != all[2]);
}

fn dword(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// The container: its parts in DXC's order, the hash D3D12 checks, a
/// compute program of shader model 6.6, the feature bit of ray-tracing
/// tier 1.1 where a query is allocated, and in `PSV0` the stage, the group
/// and each binding with its resource type and kind.
#[test]
fn the_containers_declare_the_stage_the_features_and_the_bindings() {
    for form in FORMS {
        let shader = build(form);
        let c = Container::parse(&shader.dxil).unwrap_or_else(|e| panic!("{form:?}: {e}"));
        let names: Vec<[u8; 4]> = c.parts.iter().map(|p| p.fourcc.0).collect();
        assert_eq!(
            names,
            [*b"SFI0", *b"ISG1", *b"OSG1", *b"PSV0", *b"HASH", *b"DXIL"]
        );
        assert_eq!(shader.dxil[4..20], container::hash(&shader.dxil[20..]));
        let part = |fourcc| c.part(fourcc).unwrap().data;
        let features = u64::from_le_bytes(part(FourCC::SFI0).try_into().unwrap());
        let traces = form != Form::Null;
        assert_eq!(features, if traces { 0x10_0000 } else { 0 }, "{form:?}");
        // No signature elements.
        assert_eq!(part(FourCC::ISG1), [0, 0, 0, 0, 8, 0, 0, 0]);
        assert_eq!(part(FourCC::OSG1), [0, 0, 0, 0, 8, 0, 0, 0]);
        let dxil = part(FourCC::DXIL);
        // Compute (5), 6.6; the size in dwords; DXIL 1.6; the bitcode.
        assert_eq!(dword(dxil, 0), 5 << 16 | 0x66);
        assert_eq!(dword(dxil, 4) as usize * 4, dxil.len());
        assert_eq!(&dxil[8..12], b"DXIL");
        assert_eq!([dword(dxil, 12), dword(dxil, 16)], [0x106, 16]);
        assert_eq!(dword(dxil, 20) as usize, dxil.len() - 24);
        assert_eq!(&dxil[24..28], b"BC\xC0\xDE");
        let psv = part(FourCC::PSV0);
        // Runtime info of 48 bytes: the stage at 24, the group at 36.
        assert_eq!((dword(psv, 0), psv[4 + 24]), (48, 5));
        let group: [u32; 3] = std::array::from_fn(|k| dword(psv, 4 + 36 + 4 * k));
        assert_eq!(group, [GROUP_THREADS, 1, 1]);
        // Resources of 24 bytes: type, space, first and last register, kind,
        // flags; constant buffers, then SRVs, then UAVs.
        let records: Vec<[u32; 6]> = (0..dword(psv, 52) as usize)
            .map(|n| std::array::from_fn(|k| dword(psv, 60 + 24 * n + 4 * k)))
            .collect();
        assert_eq!(dword(psv, 56), 24);
        let want: Vec<[u32; 6]> = shader
            .bindings
            .iter()
            .map(|b| {
                // PSV types: 2 CBV, 4 SRV raw, 7 UAV raw; DXIL kinds: 13
                // constant buffer, 11 raw buffer, 16 acceleration structure.
                let (ty, kind) = match b.class {
                    Class::Constants => (2, 13),
                    Class::RawBuffer => (4, 11),
                    Class::AccelerationStructure => (4, 16),
                    Class::RawBufferWritten => (7, 11),
                };
                [ty, b.space, b.register, b.register, kind, 0]
            })
            .collect();
        assert_eq!(records, want, "{form:?}");
    }
}

/// The tier 1.1 flag follows the written `AllocateRayQuery` call, as DXC
/// collects it (`DxilShaderFlags.cpp`): the tracing forms allocate one
/// query each and trace it once, the null form none.
#[test]
fn the_ray_tracing_flag_follows_the_allocation() {
    assert!(assemble(Form::Triangles).s.m.uses().ray_query);
    assert!(assemble(Form::Procedural).s.m.uses().ray_query);
    assert!(!assemble(Form::Null).s.m.uses().ray_query);
    for (form, queries) in [(Form::Triangles, 1), (Form::Procedural, 1), (Form::Null, 0)] {
        let m = assemble(form).s.m;
        assert_eq!(m.written_calls("dx.op.allocateRayQuery"), queries);
        assert_eq!(m.written_calls("dx.op.rayQuery_TraceRayInline"), queries);
        assert_eq!(m.written_calls("dx.op.rayQuery_Proceed.i1"), 2 * queries);
    }
}

/// The written function of each form passes the structural checker, calls
/// only operations the notes name, and gives every ray-query operation the
/// one handle `AllocateRayQuery` returned, directly: no phi and no select
/// carries a query.
#[test]
fn the_functions_are_well_formed_and_keep_the_query_in_one_value() {
    for form in FORMS {
        let flow = assemble(form).s.m.flow();
        flow.check().unwrap_or_else(|e| panic!("{form:?}: {e}"));
        let mut query = None;
        for inst in flow.blocks.iter().flat_map(|b| &b.insts) {
            let FlowOp::Call(name, args) = &inst.op else {
                continue;
            };
            let name = name.strip_prefix("dx.op.").expect("a dx.op call");
            let Arg::Int(opcode) = args[0] else {
                panic!("{form:?}: {name} without a constant opcode")
            };
            let Some(op) = OPS.iter().find(|op| u64::from(op.opcode) == opcode) else {
                let known = [
                    ("createHandleFromBinding", 217),
                    ("annotateHandle", 216),
                    ("threadId.i32", 93),
                    ("cbufferLoadLegacy.i32", 59),
                    ("rawBufferLoad.f32", 139),
                    ("rawBufferStore.i32", 140),
                    ("unary.f32", 27),
                ];
                assert!(known.contains(&(name, opcode)), "{form:?}: {name} {opcode}");
                continue;
            };
            assert_eq!(
                (name, args.len()),
                (op.function, 1 + op.params.len()),
                "{form:?}"
            );
            if op.opcode == op::ALLOCATE.opcode {
                assert_eq!(
                    (query, args[1]),
                    (None, Arg::Int(0)),
                    "one query, no constant flags"
                );
                query = Some(Arg::Inst(inst.id));
            } else {
                assert_eq!(Some(args[1]), query, "{form:?}: {name}");
            }
        }
        assert_eq!(query.is_some(), form != Form::Null);
    }
}

/// The candidate loop is `while (q.Proceed())` rotated: a guard, a
/// preheader of its own, the second `Proceed` in the one latch, which is
/// the exiting block, and an exit block of its own.
#[test]
fn the_candidate_loop_has_a_preheader_a_testing_latch_and_its_own_exit() {
    for form in [Form::Triangles, Form::Procedural] {
        let flow = assemble(form).s.m.flow();
        let proceeds: Vec<usize> = (0..flow.blocks.len())
            .filter(|&b| flow.calls(b).contains(&"dx.op.rayQuery_Proceed.i1"))
            .collect();
        let [guard, latch] = proceeds[..] else {
            panic!("{form:?}: {proceeds:?}")
        };
        let (FlowTerm::CondBr(_, pre, done), FlowTerm::CondBr(_, head, out)) =
            (flow.blocks[guard].term, flow.blocks[latch].term)
        else {
            panic!("{form:?}: the guard and the latch branch on Proceed")
        };
        assert!(guard < pre && pre < head && head < latch && latch < out && out < done);
        // The preheader and the exit hold nothing and have one way in.
        for (b, from, to) in [(pre, guard, head), (out, latch, done)] {
            assert_eq!(flow.blocks[b].term, FlowTerm::Br(to));
            assert_eq!(flow.preds(b), [from]);
            assert!(flow.blocks[b].insts.is_empty());
        }
        assert_eq!(flow.preds(head), [pre, latch]);
        // Nothing leaves the loop but through the latch.
        for b in head..latch {
            assert!(
                flow.succs(b).iter().all(|&s| (head..=latch).contains(&s)),
                "{form:?}: {b}"
            );
        }
        // The loop reaches the exit without the latch never.
        assert!(!flow.reaches(head, done, &[latch]));
    }
}

// ---- the arithmetic's reference ----

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    /// In [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1 << 24) as f32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }

    fn within(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn point(&mut self, range: f32) -> [f32; 3] {
        std::array::from_fn(|_| self.within(-range, range))
    }
}

/// [`triangle_test`] where the arithmetic stays in range, as every test of
/// ordinary coordinates expects it to.
fn met(origin: [f32; 3], dir: [f32; 3], p: [[f32; 3]; 3]) -> Option<TriangleHit> {
    triangle_test(origin, dir, p).expect("coordinates in range")
}

/// D3D12's front face: clockwise from the ray's origin in a left-handed
/// coordinate system. Seen from the origin along +z with x to the right and
/// y up, (0,0) → (0,1) → (1,0) turns clockwise.
#[test]
fn clockwise_from_the_origin_is_the_front_face() {
    let (origin, dir) = ([0.25, 0.25, -1.0], [0.0, 0.0, 2.0]);
    let clockwise = [[0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 0.0, 1.0]];
    let hit = met(origin, dir, clockwise).unwrap();
    assert_eq!(
        hit,
        TriangleHit {
            t: 1.0,
            u: 0.25,
            v: 0.25,
            clockwise: true
        }
    );
    let counter = [clockwise[0], clockwise[2], clockwise[1]];
    let hit = met(origin, dir, counter).unwrap();
    assert_eq!(
        hit,
        TriangleHit {
            t: 1.0,
            u: 0.25,
            v: 0.25,
            clockwise: false
        }
    );
    // From behind, the same vertices turn the other way.
    let hit = met([0.25, 0.25, 3.0], [0.0, 0.0, -1.0], clockwise).unwrap();
    assert_eq!((hit.t, hit.clockwise), (2.0, false));
    // The line, not the ray: behind the origin t is negative.
    let hit = met([0.25, 0.25, 3.0], [0.0, 0.0, 1.0], clockwise).unwrap();
    assert_eq!((hit.t, hit.clockwise), (-2.0, true));
    // u weighs the second vertex, v the third.
    let hit = met([0.125, 0.5, -1.0], dir, clockwise).unwrap();
    assert_eq!((hit.u, hit.v), (0.5, 0.125));
}

#[test]
fn degenerate_triangles_and_parallel_rays_miss() {
    let v = [[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]];
    // In the triangle's plane, through it.
    assert_eq!(
        triangle_test([-1.0, 0.25, 2.0], [1.0, 0.0, 0.0], v),
        Ok(None)
    );
    let (origin, dir) = ([0.0; 3], [0.0, 0.0, 1.0]);
    let line = [[-1.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 0.0, 2.0]];
    assert_eq!(triangle_test(origin, dir, line), Ok(None));
    assert_eq!(triangle_test(origin, dir, [[0.0, 0.0, 2.0]; 3]), Ok(None));
}

/// A NaN, an infinity, a direction of zero length or one too short for its
/// reciprocal: nothing is decided, which is not a miss.
#[test]
fn inputs_without_an_answer_are_out_of_range() {
    let v = [[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]];
    let (origin, dir) = ([0.25, 0.25, 0.0], [0.0, 0.0, 1.0]);
    assert!(matches!(triangle_test(origin, dir, v), Ok(Some(_))));
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for k in 0..9 {
            let mut p = v;
            p[k / 3][k % 3] = bad;
            assert_eq!(triangle_test(origin, dir, p), Err(OutOfRange), "{bad} {k}");
        }
        for k in 0..3 {
            let (mut o, mut d) = (origin, dir);
            (o[k], d[k]) = (bad, bad);
            assert_eq!(triangle_test(o, dir, v), Err(OutOfRange), "{bad} {k}");
            assert_eq!(triangle_test(origin, d, v), Err(OutOfRange), "{bad} {k}");
        }
    }
    assert_eq!(triangle_test(origin, [0.0; 3], v), Err(OutOfRange));
    assert_eq!(triangle_test(origin, [0.0, -0.0, 0.0], v), Err(OutOfRange));
    // The reciprocal of the smallest float is infinite; that of the
    // smallest normal one, 2^126, is not, and t is 2 × 2^126.
    assert_eq!(triangle_test(origin, [0.0, 0.0, 1e-45], v), Err(OutOfRange));
    let short = met(origin, [0.0, 0.0, f32::MIN_POSITIVE], v).unwrap();
    assert_eq!((short.t, short.u, short.v), (2f32.powi(127), 0.25, 0.25));
    // Any one input of a random ray and triangle made NaN or infinite.
    let mut rng = Rng(0xBAD_F00D);
    for _ in 0..20_000 {
        let mut x: [f32; 15] = std::array::from_fn(|_| rng.within(-4.0, 4.0));
        x[rng.below(15) as usize] =
            [f32::NAN, f32::INFINITY, f32::NEG_INFINITY][rng.below(3) as usize];
        let p = [
            [x[6], x[7], x[8]],
            [x[9], x[10], x[11]],
            [x[12], x[13], x[14]],
        ];
        let got = triangle_test([x[0], x[1], x[2]], [x[3], x[4], x[5]], p);
        assert_eq!(got, Err(OutOfRange), "{x:?}");
    }
}

/// A triangle the ray along +z from the origin crosses at t = 5, `s` wide.
fn wide(s: f32) -> [[f32; 3]; 3] {
    [
        [-1.0, -1.0, 5.0],
        [-1.0, 3.0 * s, 5.0],
        [3.0 * s, -1.0, 5.0],
    ]
}

/// A triangle around the +z axis as far along it as a float goes: the
/// distance's numerator and denominator stay finite and the distance, their
/// quotient, rounds to infinity.
const FARTHEST: [[f32; 3]; 3] = [
    [0.863_302_95, -1.342_596, f32::MAX],
    [-0.436_632_87, 0.834_815_5, f32::MAX],
    [-0.860_472_7, 1.137_987_1, f32::MAX],
];

/// Coordinates so large that their products leave single precision are out
/// of range, never a miss and never a hit holding a NaN; up to
/// [`EXACT_COORDINATE`] nothing is.
#[test]
fn coordinates_past_single_precision_are_out_of_range() {
    let (origin, dir) = ([0.0; 3], [0.0, 0.0, 1.0]);
    // The ray crosses each of these at t = 5.
    for s in [1.0, 1e9, 1e18] {
        let hit = met(origin, dir, wide(s)).unwrap();
        assert_eq!((hit.t, hit.clockwise), (5.0, true), "{s}");
    }
    for s in [3e19, 1e20, 1e25] {
        assert_eq!(triangle_test(origin, dir, wide(s)), Err(OutOfRange), "{s}");
    }
    // One whose barycentric came out NaN beside a positive denominator.
    let was_nan = [
        [1.563_227_2e19, 7.216_759e19, 5.0],
        [4.515_855_3e18, 0.652_049_8, 5.0],
        [-8.968_067e18, -7.463_698e18, 5.0],
    ];
    assert_eq!(triangle_test(origin, dir, was_nan), Err(OutOfRange));
    // All three axes large under a direction of length 1: the distance's
    // numerator is a product of three.
    // (At s = 2^41 the largest product is 8 s^3 = 2^126.)
    let deep = |s: f32| [[-s, -s, s], [-s, 3.0 * s, s], [3.0 * s, -s, s]];
    let (near, far) = (2f32.powi(41), 2f32.powi(42));
    assert_eq!(met(origin, dir, deep(near)).unwrap().t, near);
    assert_eq!(triangle_test(origin, dir, deep(far)), Err(OutOfRange));
    // Met, numerator and denominator finite, and their quotient not.
    assert_eq!(triangle_test(origin, dir, FARTHEST), Err(OutOfRange));
    // The small end is not refused. Across 1e-23 the products keep one bit:
    // a hit whose weights, a quarter each, are lost. Across 1e-24 they
    // round to 0: zero-area, though the ray crosses.
    let small = |s: f32| wide(1.0).map(|v| [v[0] * s, v[1] * s, v[2]]);
    let hit = met(origin, dir, small(1.0)).unwrap();
    assert_eq!((hit.t, hit.u, hit.v), (5.0, 0.25, 0.25));
    let hit = met(origin, dir, small(1e-23)).unwrap();
    assert_eq!((hit.t, hit.u, hit.v), (5.0, 0.0, 0.0));
    assert_eq!(triangle_test(origin, dir, small(1e-24)), Ok(None));
    // Whatever the magnitudes, a hit holds finite numbers and weights from
    // 0 to 1, and every outcome occurs.
    let mut rng = Rng(77);
    let (mut hits, mut misses, mut refused) = (0, 0, 0);
    for _ in 0..200_000 {
        let mut m =
            || [1.0, 1e12, 1e19, 1e20, -1e20, 3e20][rng.below(6) as usize] * rng.within(-1.0, 1.0);
        let p: [[f32; 3]; 3] = std::array::from_fn(|_| [m(), m(), 5.0]);
        match triangle_test(origin, dir, p) {
            Ok(Some(h)) => {
                hits += 1;
                assert!(h.t.is_finite(), "{p:?} {h:?}");
                assert!(
                    (0.0..=1.0).contains(&h.u) && (0.0..=1.0).contains(&h.v),
                    "{p:?} {h:?}"
                );
            }
            Ok(None) => misses += 1,
            Err(OutOfRange) => refused += 1,
        }
    }
    assert!(
        hits > 1000 && misses > 10_000 && refused > 10_000,
        "{hits} {misses} {refused}"
    );
    // In the stated range nothing is refused.
    for _ in 0..200_000 {
        let scale = [1.0, 4096.0, EXACT_COORDINATE][rng.below(3) as usize];
        let p: [[f32; 3]; 3] = std::array::from_fn(|_| rng.point(scale));
        let o = rng.point(scale);
        // The largest component anywhere from 2^-24 to 2^24, either sign.
        let exponent = rng.below(49) as i32 - 24;
        let mut d = rng.point(2f32.powi(exponent));
        let k = rng.below(3) as usize;
        d[k] = 2f32.powi(exponent).copysign(d[k]);
        assert!(triangle_test(o, d, p).is_ok(), "{o:?} {d:?} {p:?}");
    }
}

/// Random points on the edge two triangles share are in at least one.
#[test]
fn a_shared_edge_leaves_no_gap() {
    let (p, q) = ([0.3f32, -0.7, 1.0], [-0.9f32, 0.6, 1.3]);
    let (s, t) = ([1.1f32, 0.9, 0.7], [-1.2f32, -0.8, 1.1]);
    let mut rng = Rng(0x1234_5678);
    for _ in 0..20_000 {
        let k = rng.unit();
        let at: [f32; 3] = std::array::from_fn(|i| p[i] + k * (q[i] - p[i]));
        let o = [rng.unit() - 0.5, rng.unit() - 0.5, -2.0];
        let d: [f32; 3] = std::array::from_fn(|i| at[i] - o[i]);
        let hits = [met(o, d, [p, q, s]), met(o, d, [q, p, t])];
        assert!(hits.iter().any(Option::is_some), "{o:?} {d:?}");
    }
}

/// Against Möller-Trumbore in double precision: the same hits, distances,
/// barycentrics and facing wherever the answer is not within rounding of an
/// edge or the plane.
#[test]
fn the_test_agrees_with_moller_trumbore() {
    let mut rng = Rng(42);
    let (mut hits, mut fronts) = (0, 0);
    for _ in 0..40_000 {
        let v: [[f32; 3]; 3] = std::array::from_fn(|_| rng.point(2.0));
        let (o, d) = (rng.point(2.0), rng.point(1.0));
        let ours = met(o, d, v);
        let f = |a: [f32; 3]| a.map(f64::from);
        let (v0, v1, v2, o, d) = (f(v[0]), f(v[1]), f(v[2]), f(o), f(d));
        let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let (e1, e2, e3) = (sub(v1, v0), sub(v2, v0), sub(o, v0));
        let (s1, s2) = (cross(d, e2), cross(e3, e1));
        // Positive where the ray meets e1 × e2 head on: clockwise.
        let det = dot(s1, e1);
        let (t, u, w) = (dot(e2, s2) / det, dot(e3, s1) / det, dot(d, s2) / det);
        let inside = u > 1e-4 && w > 1e-4 && u + w < 1.0 - 1e-4 && det.abs() > 1e-3;
        let outside = u < -1e-4 || w < -1e-4 || u + w > 1.0 + 1e-4;
        if inside {
            let ours = ours.unwrap_or_else(|| panic!("{v:?} {o:?} {d:?}"));
            hits += 1;
            fronts += u32::from(ours.clockwise);
            assert_eq!(ours.clockwise, det > 0.0);
            for (a, b) in [(ours.t, t), (ours.u, u), (ours.v, w)] {
                assert!((f64::from(a) - b).abs() < 1e-3 * (1.0 + b.abs()), "{a} {b}");
            }
        } else if outside {
            assert_eq!(ours, None, "{v:?} {o:?} {d:?}");
        }
    }
    assert!(
        hits > 1000 && fronts > 300 && hits - fronts > 300,
        "{hits} {fronts}"
    );
}

#[test]
fn the_alpha_rule_is_a_checkerboard_of_eighths_shifted_by_the_primitive() {
    assert!(alpha_accepts(0.0, 0.0, 0));
    assert!(!alpha_accepts(0.0, 0.0, 1));
    // Cells change at multiples of 1/8, the lower bound included.
    assert!(alpha_accepts(0.124_999, 0.0, 0));
    assert!(!alpha_accepts(0.125, 0.0, 0));
    assert!(alpha_accepts(0.125, 0.125, 0));
    assert!(alpha_accepts(0.3, 0.6, 0) && !alpha_accepts(0.3, 0.6, 7));
    assert!(alpha_accepts(1.0, 0.0, 0));
    // A weight a rounding below 0 is in the cell below.
    assert!(!alpha_accepts(-1e-9, 0.0, 0));
    // The sum is taken modulo 2^32.
    assert!(!alpha_accepts(0.0, 0.0, u32::MAX));
    assert!(alpha_accepts(0.125, 0.0, u32::MAX));
}

fn ray_words(
    origin: [f32; 3],
    tmin: f32,
    dir: [f32; 3],
    tmax: f32,
    mask: u32,
    flags: u32,
) -> [u32; 12] {
    let f = [
        origin[0], origin[1], origin[2], tmin, dir[0], dir[1], dir[2], tmax,
    ]
    .map(f32::to_bits);
    // The two words past the flags are not read.
    [
        f[0],
        f[1],
        f[2],
        f[3],
        f[4],
        f[5],
        f[6],
        f[7],
        mask,
        flags,
        0xDEAD_BEEF,
        0x7FC0_0000,
    ]
}

#[test]
fn rays_dxr_leaves_undefined_are_refused() {
    let ray = |flags| ray_words([0.0; 3], 0.0, [0.0, 0.0, 1.0], f32::INFINITY, 0xFF, flags);
    for flags in [
        0, 0x01, 0x02, 0x04, 0x10, 0x20, 0x40, 0x80, 0x100, 0x200, 0x15, 0x2A4, 0x140,
    ] {
        assert_eq!(ray_refusal(&ray(flags)), None, "{flags:#x}");
    }
    // SKIP_CLOSEST_HIT_SHADER, the micromap flag, an unknown bit; two
    // opacity flags; both faces; triangles skipped with a face cull or with
    // procedural primitives.
    for flags in [
        0x08,
        0x400,
        0x8000_0000,
        0x03,
        0x41,
        0xC0,
        0x82,
        0x30,
        0x110,
        0x120,
        0x300,
    ] {
        assert!(ray_refusal(&ray(flags)).is_some(), "{flags:#x}");
    }
    assert_eq!(RAY_FLAGS, 0x3F7);
    let mut bad = ray(0);
    bad[5] = f32::NAN.to_bits();
    assert_eq!(ray_refusal(&bad), Some("a NaN in the ray"));
    for (tmin, tmax) in [(-0.0, 1.0), (0.0, 0.0), (1.0, 1.0), (0.5, f32::INFINITY)] {
        let ok = ray_words([0.0; 3], tmin, [0.0, 0.0, 1.0], tmax, 1, 0);
        assert_eq!(ray_refusal(&ok), None, "{tmin} {tmax}");
    }
    for (tmin, tmax) in [(-1.0, 1.0), (2.0, 1.0), (0.0, f32::NAN), (f32::NAN, 1.0)] {
        let bad = ray_words([0.0; 3], tmin, [0.0, 0.0, 1.0], tmax, 1, 0);
        assert!(ray_refusal(&bad).is_some(), "{tmin} {tmax}");
    }
    let far = ray_words([f32::INFINITY, 0.0, 0.0], 0.0, [0.0, 0.0, 1.0], 1.0, 1, 0);
    assert!(ray_refusal(&far).is_some());
    // No direction at all, of either sign of zero; the shortest there is,
    // is one.
    for dir in [[0.0; 3], [-0.0, 0.0, -0.0]] {
        let still = ray_words([1.0, 2.0, 3.0], 0.0, dir, 1.0, 1, 0);
        assert_eq!(ray_refusal(&still), Some("a direction of zero length"));
    }
    let short = ray_words([0.0; 3], 0.0, [0.0, 1e-45, 0.0], 1.0, 1, 0);
    assert_eq!(ray_refusal(&short), None);
    // The mask's high bits and the last two words are nobody's business.
    assert_eq!(
        ray_refusal(&ray_words([0.0; 3], 0.0, [1.0; 3], 1.0, u32::MAX, 0)),
        None
    );
}

// ---- the shaders, run against a model of the device ----

const FORCE_OPAQUE: u32 = 0x01;
const FORCE_NON_OPAQUE: u32 = 0x02;
const ACCEPT_FIRST_HIT: u32 = 0x04;
const CULL_BACK: u32 = 0x10;
const CULL_FRONT: u32 = 0x20;
const CULL_OPAQUE: u32 = 0x40;
const CULL_NON_OPAQUE: u32 = 0x80;
const SKIP_TRIANGLES: u32 = 0x100;
const SKIP_PROCEDURAL: u32 = 0x200;
/// `D3D12_RAYTRACING_INSTANCE_FLAGS`.
const INSTANCE_CULL_DISABLE: u32 = 1;
const INSTANCE_COUNTERCLOCKWISE: u32 = 2;
const INSTANCE_FORCE_OPAQUE: u32 = 4;
const INSTANCE_FORCE_NON_OPAQUE: u32 = 8;

#[derive(Debug, Clone)]
struct Geometry {
    /// `D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE`.
    opaque: bool,
    triangles: Vec<[[f32; 3]; 3]>,
}

/// An instance of a bottom-level structure of its own, moved by `offset`.
#[derive(Debug, Clone)]
struct Instance {
    id: u32,
    mask: u32,
    flags: u32,
    offset: [f32; 3],
    geometries: Vec<Geometry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    instance: usize,
    geometry: usize,
    primitive: usize,
}

fn places(scene: &[Instance]) -> Vec<Place> {
    let mut all = Vec::new();
    for (instance, i) in scene.iter().enumerate() {
        for (geometry, g) in i.geometries.iter().enumerate() {
            for primitive in 0..g.triangles.len() {
                all.push(Place {
                    instance,
                    geometry,
                    primitive,
                });
            }
        }
    }
    all
}

/// The triangle data of [`Resource::Triangles`], with unread words between
/// its tables: nothing may assume they follow one another.
fn triangle_data(scene: &[Instance]) -> Vec<u32> {
    let mut d = vec![0; 2 * scene.len()];
    for (i, instance) in scene.iter().enumerate() {
        d.extend([0x7FC0_0001; 3]);
        let table = d.len();
        (d[2 * i], d[2 * i + 1]) = (4 * table as u32, instance.flags);
        d.resize(table + instance.geometries.len(), 0);
        for (g, geometry) in instance.geometries.iter().enumerate() {
            d.push(0x7FC0_0002);
            d[table + g] = 4 * d.len() as u32;
            d.extend(
                geometry
                    .triangles
                    .iter()
                    .flatten()
                    .flatten()
                    .map(|x| x.to_bits()),
            );
        }
    }
    d
}

struct Ray {
    origin: [f32; 3],
    tmin: f32,
    dir: [f32; 3],
    tmax: f32,
    mask: u32,
    flags: u32,
}

/// A triangle the ray's line meets, and how DXR has the device treat it.
#[derive(Debug, Clone, Copy)]
struct Seen {
    place: Place,
    hit: TriangleHit,
    front: bool,
    opaque: bool,
}

/// Whether the ray takes the instance at all, and whether it has the
/// geometry opaque (`None`: culled for its opacity): the geometry's flag,
/// overridden by the instance's, overridden by the ray's.
fn opacity(scene: &[Instance], ray: &Ray, place: Place) -> Option<bool> {
    let instance = &scene[place.instance];
    if ray.mask & instance.mask & 0xFF == 0 {
        return None;
    }
    let mut opaque = instance.geometries[place.geometry].opaque;
    for (flags, force, force_non) in [
        (
            instance.flags,
            INSTANCE_FORCE_OPAQUE,
            INSTANCE_FORCE_NON_OPAQUE,
        ),
        (ray.flags, FORCE_OPAQUE, FORCE_NON_OPAQUE),
    ] {
        opaque = (opaque || flags & force != 0) && flags & force_non == 0;
    }
    let culled = ray.flags & if opaque { CULL_OPAQUE } else { CULL_NON_OPAQUE } != 0;
    (!culled).then_some(opaque)
}

/// The triangle at `place` as a ray with DXR's triangle rules sees it,
/// before any range test: the object-space test, then face culling.
fn triangle_seen(scene: &[Instance], ray: &Ray, place: Place) -> Option<Seen> {
    let opaque = opacity(scene, ray, place)?;
    if ray.flags & SKIP_TRIANGLES != 0 {
        return None;
    }
    let instance = &scene[place.instance];
    let origin: [f32; 3] = std::array::from_fn(|k| ray.origin[k] - instance.offset[k]);
    let triangle = instance.geometries[place.geometry].triangles[place.primitive];
    let hit = met(origin, ray.dir, triangle)?;
    let front = hit.clockwise != (instance.flags & INSTANCE_COUNTERCLOCKWISE != 0);
    let cull = if front { CULL_FRONT } else { CULL_BACK };
    if instance.flags & INSTANCE_CULL_DISABLE == 0 && ray.flags & cull != 0 {
        return None;
    }
    Some(Seen {
        place,
        hit,
        front,
        opaque,
    })
}

/// Every hit a ray may report with the alpha rule in force: inside the
/// ray's extent, both ends excluded.
fn acceptable(scene: &[Instance], ray: &Ray) -> Vec<Seen> {
    places(scene)
        .into_iter()
        .filter_map(|place| triangle_seen(scene, ray, place))
        .filter(|s| s.hit.t > ray.tmin && s.hit.t < ray.tmax)
        .filter(|s| s.opaque || alpha_accepts(s.hit.u, s.hit.v, s.place.primitive as u32))
        .collect()
}

fn record(scene: &[Instance], form: Form, s: &Seen) -> [u32; 8] {
    let kind = match (form, s.front) {
        (Form::Procedural, _) => HIT_PROCEDURAL,
        (_, true) => HIT_FRONT,
        (_, false) => HIT_BACK,
    };
    let p = s.place;
    [
        s.hit.t.to_bits(),
        s.hit.u.to_bits(),
        s.hit.v.to_bits(),
        kind,
        p.instance as u32,
        scene[p.instance].id,
        p.geometry as u32,
        p.primitive as u32,
    ]
}

struct Candidate {
    place: Place,
    /// A triangle the device found (the triangles form); a box otherwise.
    triangle: Option<Seen>,
    non_opaque: bool,
}

struct Committed {
    t: f32,
    place: Place,
    triangle: Option<Seen>,
}

/// A device's inline ray query over `scene`, as triangles or (`boxes`) as
/// one box per triangle, enumerating primitives in `order`. It panics when
/// the shader does what DXR does not allow.
struct Device<'a> {
    scene: &'a [Instance],
    boxes: bool,
    order: Vec<Place>,
    next: usize,
    ray: Option<Ray>,
    candidate: Option<Candidate>,
    committed: Option<Committed>,
    ended: bool,
    allocated: u32,
    commits: u32,
    /// The ray flags the shader traced may pass: the triangles form's on
    /// triangles, the procedural form's on boxes.
    allowed: u32,
}

/// The query handle the model hands out.
const QUERY: u64 = 0x51;

impl<'a> Device<'a> {
    fn new(scene: &'a [Instance], boxes: bool, order: Vec<Place>) -> Self {
        Device {
            scene,
            boxes,
            order,
            next: 0,
            ray: None,
            candidate: None,
            committed: None,
            ended: false,
            allocated: 0,
            commits: 0,
            allowed: if boxes { 0xC7 } else { RAY_FLAGS },
        }
    }

    fn closest(&self) -> f32 {
        let ray = self.ray.as_ref().expect("a traced ray");
        self.committed.as_ref().map_or(ray.tmax, |c| c.t)
    }

    fn commit(&mut self, t: f32, place: Place, triangle: Option<Seen>) {
        self.committed = Some(Committed { t, place, triangle });
        self.commits += 1;
        self.ended |= self.ray.as_ref().unwrap().flags & ACCEPT_FIRST_HIT != 0;
    }

    fn proceed(&mut self) -> bool {
        self.candidate = None;
        while !self.ended && self.next < self.order.len() {
            let place = self.order[self.next];
            self.next += 1;
            let ray = self.ray.as_ref().expect("a traced ray");
            if self.boxes {
                // Every box is offered, hit or not: enumeration is
                // conservative.
                let Some(opaque) = opacity(self.scene, ray, place) else {
                    continue;
                };
                // (The procedural form may not pass the flag: `allowed`.)
                if ray.flags & SKIP_PROCEDURAL != 0 {
                    continue;
                }
                let non_opaque = !opaque;
                self.candidate = Some(Candidate {
                    place,
                    triangle: None,
                    non_opaque,
                });
                return true;
            }
            let Some(seen) = triangle_seen(self.scene, ray, place) else {
                continue;
            };
            if !(seen.hit.t > ray.tmin && seen.hit.t < self.closest()) {
                continue;
            }
            if seen.opaque {
                self.commit(seen.hit.t, place, Some(seen));
            } else {
                self.candidate = Some(Candidate {
                    place,
                    triangle: Some(seen),
                    non_opaque: true,
                });
                return true;
            }
        }
        false
    }

    /// A ray-query call's result; `None` for a function that is not one.
    fn call(&mut self, name: &str, args: &[u64]) -> Option<Vec<u64>> {
        let float = |x: f32| vec![u64::from(x.to_bits())];
        let f32_of = |x: u64| f32::from_bits(x as u32);
        let op = OPS.iter().find(|op| u64::from(op.opcode) == args[0])?;
        if name.strip_prefix("dx.op.") != Some(op.function) {
            return None;
        }
        if op.opcode == op::ALLOCATE.opcode {
            assert_eq!(args[1], 0, "no constant ray flags");
            self.allocated += 1;
            return Some(vec![QUERY]);
        }
        assert_eq!(args[1], QUERY, "{}: the query handle", op.name);
        let candidate = |d: &Self| d.candidate.as_ref().map(|c| c.place);
        let committed = |d: &Self| d.committed.as_ref().map(|c| c.place);
        let scene = self.scene;
        let place_word = |place: Option<Place>, what: u32| -> Vec<u64> {
            let p = place.unwrap_or_else(|| panic!("{}: nothing to read", op.name));
            vec![match what {
                0 => p.instance as u64,
                1 => u64::from(scene[p.instance].id),
                2 => p.geometry as u64,
                _ => p.primitive as u64,
            }]
        };
        Some(match op.opcode {
            179 => {
                assert!(self.ray.is_none(), "one trace a query");
                // The acceleration structure is checked by the caller.
                let x: [f32; 8] = std::array::from_fn(|k| f32_of(args[5 + k]));
                let allowed = u64::from(self.allowed);
                assert_eq!(args[3] & !allowed, 0, "ray flags {:#x}", args[3]);
                assert_eq!(args[4] & !0xFF, 0, "the mask's 8 bits");
                self.ray = Some(Ray {
                    origin: [x[0], x[1], x[2]],
                    tmin: x[3],
                    dir: [x[4], x[5], x[6]],
                    tmax: x[7],
                    mask: args[4] as u32,
                    flags: args[3] as u32,
                });
                Vec::new()
            }
            180 => vec![u64::from(self.proceed())],
            182 => {
                let c = self.candidate.as_ref().expect("a candidate to commit");
                let (place, seen) = (c.place, c.triangle.expect("a triangle candidate"));
                // A second commit of the same candidate changes nothing.
                if self.committed.as_ref().is_none_or(|k| k.place != place) {
                    self.commit(seen.hit.t, place, Some(seen));
                }
                Vec::new()
            }
            183 => {
                let c = self.candidate.as_ref().expect("a candidate to commit");
                assert!(c.triangle.is_none(), "a procedural candidate");
                let (place, t) = (c.place, f32_of(args[2]));
                let tmin = self.ray.as_ref().unwrap().tmin;
                assert!(
                    t >= tmin && t <= self.closest(),
                    "a commit outside the ray's extent"
                );
                self.commit(t, place, None);
                Vec::new()
            }
            184 => vec![match &self.committed {
                None => 0,
                Some(c) if c.triangle.is_some() => 1,
                Some(_) => 2,
            }],
            185 => {
                let c = self.candidate.as_ref().expect("a candidate");
                vec![u64::from(c.triangle.is_none())]
            }
            190 => {
                let c = self.candidate.as_ref().expect("a candidate");
                assert!(c.triangle.is_none(), "a procedural candidate");
                vec![u64::from(c.non_opaque)]
            }
            192 => {
                let c = self.committed.as_ref().expect("a committed hit");
                vec![u64::from(c.triangle.expect("a committed triangle").front)]
            }
            193 | 194 => {
                let triangle = match op.opcode {
                    193 => self.candidate.as_ref().and_then(|c| c.triangle),
                    _ => self.committed.as_ref().and_then(|c| c.triangle),
                };
                let hit = triangle
                    .unwrap_or_else(|| panic!("{}: no triangle", op.name))
                    .hit;
                match args[2] {
                    0 => float(hit.u),
                    1 => float(hit.v),
                    k => panic!("barycentric component {k}"),
                }
            }
            200 => float(self.closest()),
            201..=204 => place_word(candidate(self), op.opcode - 201),
            205 | 206 => {
                let place = candidate(self).expect("a candidate");
                let (ray, k) = (self.ray.as_ref().unwrap(), args[2] as usize);
                assert!(k < 3, "vector component {k}");
                float(match op.opcode {
                    205 => ray.origin[k] - scene[place.instance].offset[k],
                    _ => ray.dir[k],
                })
            }
            207..=210 => place_word(committed(self), op.opcode - 207),
            _ => panic!("{}: not expected of these shaders", op.name),
        })
    }
}

/// What one thread did.
struct Ran {
    /// The hit buffer's stores: byte offset and four dwords.
    stores: Vec<(u32, [u32; 4])>,
    commits: u32,
    traced: bool,
}

/// Runs thread `thread` of `form` with `count` rays listed, the ray
/// records `rays` and the triangle data `data`, against `device`.
fn run_thread(
    flow: &Flow,
    bindings: &[Binding],
    (thread, count): (u32, u32),
    rays: &[u32],
    data: &[u32],
    device: &mut Device,
) -> Ran {
    let mut handles = 0usize;
    let mut stores = Vec::new();
    let mut scene_passed = false;
    // The resource of a handle: they are numbered as they are made, which
    // is the bindings' order.
    let of = |h: u64| bindings[h as usize - 1].resource;
    let mut call = |name: &str, args: &[u64]| -> Vec<u64> {
        if let Some(result) = device.call(name, args) {
            if args[0] == u64::from(op::TRACE_RAY_INLINE.opcode) {
                assert_eq!(of(args[2]), Resource::Scene);
                scene_passed = true;
            }
            return result;
        }
        match (name, args[0]) {
            ("dx.op.createHandleFromBinding", 217) => {
                handles += 1;
                assert_eq!(args[2], u64::from(bindings[handles - 1].register));
                vec![handles as u64]
            }
            ("dx.op.annotateHandle", 216) => vec![args[1]],
            ("dx.op.threadId.i32", 93) => {
                assert_eq!(args[1], 0, "the x component");
                vec![u64::from(thread)]
            }
            ("dx.op.cbufferLoadLegacy.i32", 59) => {
                assert_eq!((of(args[1]), args[2]), (Resource::Constants, 0));
                vec![u64::from(count), 0xBAD, 0xBAD, 0xBAD]
            }
            ("dx.op.rawBufferLoad.f32", 139) => {
                let words = match of(args[1]) {
                    Resource::Rays => rays,
                    Resource::Triangles => data,
                    other => panic!("a load from {other:?}"),
                };
                assert_eq!((args[2] % 4, args[5]), (0, 4), "aligned dwords");
                let at = args[2] as usize / 4;
                let mut out: Vec<u64> = (0..4)
                    .map(|k| match args[4] >> k & 1 {
                        1 => u64::from(*words.get(at + k).expect("a load inside the buffer")),
                        _ => 0xBAD,
                    })
                    .collect();
                out.push(0);
                out
            }
            ("dx.op.rawBufferStore.i32", 140) => {
                assert_eq!((of(args[1]), args[8], args[9]), (Resource::Hits, 0xF, 4));
                let words: [u32; 4] = std::array::from_fn(|k| args[4 + k] as u32);
                stores.push((args[2] as u32, words));
                Vec::new()
            }
            // Round_ni.
            ("dx.op.unary.f32", 27) => {
                vec![u64::from(f32::from_bits(args[1] as u32).floor().to_bits())]
            }
            _ => panic!("{name} {args:?}"),
        }
    };
    flow.run(100_000, &mut call).unwrap();
    assert_eq!(handles, bindings.len(), "every binding has its handle");
    assert_eq!(device.ray.is_some(), scene_passed);
    Ran {
        stores,
        commits: device.commits,
        traced: device.ray.is_some(),
    }
}

/// The hit record a thread stored: two stores of four dwords, at the
/// thread's place.
fn stored(ran: &Ran, thread: u32) -> [u32; 8] {
    let at = thread * HIT_BYTES;
    let [(a, low), (b, high)] = ran.stores[..] else {
        panic!("{} stores", ran.stores.len())
    };
    assert_eq!((a, b), (at, at + 16));
    std::array::from_fn(|k| if k < 4 { low[k] } else { high[k - 4] })
}

/// A hit record without its kind, in which the forms differ.
fn but_kind(r: [u32; 8]) -> [u32; 7] {
    [r[0], r[1], r[2], r[4], r[5], r[6], r[7]]
}

struct Shaders {
    triangles: (Flow, Vec<Binding>),
    procedural: (Flow, Vec<Binding>),
}

fn shaders() -> Shaders {
    let of = |form| {
        let built = assemble(form);
        (built.s.m.flow(), built.bindings)
    };
    Shaders {
        triangles: of(Form::Triangles),
        procedural: of(Form::Procedural),
    }
}

/// Traces `ray` (record 0 of one listed) through both forms with the
/// device enumerating in `order`; returns the triangles form's record and
/// the procedural form's.
fn trace(sh: &Shaders, scene: &[Instance], ray: &[u32; 12], order: &[Place]) -> [[u32; 8]; 2] {
    assert_eq!(ray_refusal(ray), None);
    let data = triangle_data(scene);
    let mut out = [[0; 8]; 2];
    for (k, (boxes, (flow, bindings))) in [(false, &sh.triangles), (true, &sh.procedural)]
        .into_iter()
        .enumerate()
    {
        // The rays' list starts at a thread of its own: thread 3 of 4.
        let mut rays = vec![0x7FC0_0003; 3 * RAY_BYTES as usize / 4];
        rays.extend(ray);
        let mut device = Device::new(scene, boxes, order.to_vec());
        let ran = run_thread(flow, bindings, (3, 4), &rays, &data, &mut device);
        assert!(ran.traced && device.allocated == 1);
        out[k] = stored(&ran, 3);
        // A miss is all zeros, whatever was looked at on the way.
        assert_eq!(out[k][3] == HIT_MISS, ran.commits == 0);
        if out[k][3] == HIT_MISS {
            assert_eq!(out[k], [0; 8]);
        }
    }
    out
}

/// A triangle facing the origin side of z, at depth `z`, around (x, y).
fn facing(x: f32, y: f32, z: f32) -> [[f32; 3]; 3] {
    [
        [x - 1.0, y - 1.0, z],
        [x - 1.0, y + 3.0, z],
        [x + 3.0, y - 1.0, z],
    ]
}

fn instance(id: u32, flags: u32, geometries: Vec<Geometry>) -> Instance {
    Instance {
        id,
        mask: 0xFF,
        flags,
        offset: [0.0; 3],
        geometries,
    }
}

fn along_z(x: f32, y: f32, flags: u32) -> [u32; 12] {
    ray_words([x, y, 0.0], 0.0, [0.0, 0.0, 1.0], 100.0, 0xFF, flags)
}

#[test]
fn threads_past_the_ray_list_touch_nothing() {
    let scene = [instance(
        7,
        0,
        vec![Geometry {
            opaque: true,
            triangles: vec![facing(0.0, 0.0, 5.0)],
        }],
    )];
    let data = triangle_data(&scene);
    for form in FORMS {
        let built = assemble(form);
        let flow = built.s.m.flow();
        for (thread, count) in [(0, 0), (5, 5), (64, 3), (u32::MAX, 1)] {
            let mut device = Device::new(&scene, form == Form::Procedural, places(&scene));
            let ran = run_thread(
                &flow,
                &built.bindings,
                (thread, count),
                &[],
                &data,
                &mut device,
            );
            assert!(ran.stores.is_empty() && !ran.traced, "{form:?}");
            assert_eq!(device.allocated, 0);
        }
    }
}

#[test]
fn the_null_form_writes_misses_and_traces_nothing() {
    let built = assemble(Form::Null);
    let flow = built.s.m.flow();
    for (thread, count) in [(0, 1), (41, 42), (1000, 4000)] {
        let mut device = Device::new(&[], false, Vec::new());
        let ran = run_thread(
            &flow,
            &built.bindings,
            (thread, count),
            &[],
            &[],
            &mut device,
        );
        assert_eq!(stored(&ran, thread), [0; 8]);
        assert!(!ran.traced && device.allocated == 0);
    }
}

/// One opaque triangle: the record's words, face by face, and the ray's
/// words where `TraceRayInline` takes them.
#[test]
fn a_hit_record_holds_distance_barycentrics_face_and_place() {
    let sh = shaders();
    // Seen from the origin along +z, `facing` turns clockwise: a front face.
    let front = Geometry {
        opaque: true,
        triangles: vec![facing(0.0, 0.0, 5.0)],
    };
    let back = {
        let [a, b, c] = facing(0.0, 0.0, 5.0);
        Geometry {
            opaque: true,
            triangles: vec![[[0.0; 3]; 3]; 2]
                .into_iter()
                .chain([[a, c, b]])
                .collect(),
        }
    };
    let other = Geometry {
        opaque: true,
        triangles: vec![facing(50.0, 0.0, 5.0)],
    };
    let mut scene = vec![
        instance(0xAB_CDEF, 0, vec![other.clone(), front]),
        instance(0x12_3456, 0, vec![other.clone(), other, back]),
    ];
    scene[1].offset = [100.0, 0.0, 0.0];
    let order = places(&scene);
    let bits = |x: f32| x.to_bits();
    // Barycentrics: (x + 1) / 4 weighs the third vertex, (y + 1) / 4 the
    // second.
    let [tri, pro] = trace(&sh, &scene, &along_z(1.0, 0.0, 0), &order);
    assert_eq!(
        tri,
        [
            bits(5.0),
            bits(0.25),
            bits(0.5),
            HIT_FRONT,
            0,
            0xAB_CDEF,
            1,
            0
        ]
    );
    assert_eq!(
        pro,
        [
            bits(5.0),
            bits(0.25),
            bits(0.5),
            HIT_PROCEDURAL,
            0,
            0xAB_CDEF,
            1,
            0
        ]
    );
    // The second instance's triangle, wound the other way, in its own space.
    let [tri, pro] = trace(&sh, &scene, &along_z(101.0, 0.0, 0), &order);
    assert_eq!(
        tri,
        [
            bits(5.0),
            bits(0.5),
            bits(0.25),
            HIT_BACK,
            1,
            0x12_3456,
            2,
            2
        ]
    );
    assert_eq!(pro[..3], tri[..3]);
    assert_eq!(pro[3..], [HIT_PROCEDURAL, 1, 0x12_3456, 2, 2]);
    // An instance whose front faces are counter-clockwise.
    scene[1].flags = INSTANCE_COUNTERCLOCKWISE;
    let [tri, _] = trace(&sh, &scene, &along_z(101.0, 0.0, 0), &order);
    assert_eq!(tri[3], HIT_FRONT);
    // The extent excludes its ends: a hit at tmax or at tmin is none.
    for (tmin, tmax, hit) in [(0.0, 5.0, false), (5.0, 9.0, false), (4.999, 5.001, true)] {
        let ray = ray_words([1.0, 0.0, 0.0], tmin, [0.0, 0.0, 1.0], tmax, 0xFF, 0);
        let [tri, pro] = trace(&sh, &scene, &ray, &order);
        assert_eq!(
            [tri[3], pro[3]],
            if hit {
                [HIT_FRONT, HIT_PROCEDURAL]
            } else {
                [0, 0]
            }
        );
    }
    // A direction of another length: t is in its units.
    let ray = ray_words([1.0, 0.0, 1.0], 0.0, [0.0, 0.0, 0.5], 100.0, 0xFF, 0);
    assert_eq!(trace(&sh, &scene, &ray, &order)[1][0], bits(8.0));
    // The instance mask: its low 8 bits, against the instance's.
    scene[0].mask = 0x10;
    for (mask, hit) in [
        (0x10, true),
        (0xFFFF_FF10, true),
        (0xEF, false),
        (0x100, false),
    ] {
        let ray = ray_words([1.0, 0.0, 0.0], 0.0, [0.0, 0.0, 1.0], 100.0, mask, 0);
        let [tri, pro] = trace(&sh, &scene, &ray, &order);
        assert_eq!([tri[3] != 0, pro[3] != 0], [hit; 2], "{mask:#x}");
    }
}

/// Geometry not marked opaque goes through the alpha rule, in both forms;
/// the ray's and the instance's opacity flags decide which geometry that
/// is.
#[test]
fn non_opaque_candidates_pass_the_alpha_rule_or_are_left() {
    let sh = shaders();
    // Primitive 0 behind primitive 1, which the rule holes at (1, 0):
    // u = 0.25, v = 0.5, cells 2 + 4, primitive 1: odd.
    let holed = Geometry {
        opaque: false,
        triangles: vec![facing(0.0, 0.0, 9.0), facing(0.0, 0.0, 5.0)],
    };
    let mut scene = vec![instance(1, 0, vec![holed])];
    let order = places(&scene);
    let reversed: Vec<Place> = order.iter().rev().copied().collect();
    let at = |scene: &[Instance], x: f32, flags: u32, order: &[Place]| {
        let [tri, pro] = trace(&sh, scene, &along_z(x, 0.0, flags), order);
        assert_eq!(but_kind(tri), but_kind(pro), "the forms agree");
        (f32::from_bits(tri[0]), tri[7])
    };
    for order in [&order, &reversed] {
        // Through the hole to the primitive behind (cells 2 + 4 + 0: even).
        assert_eq!(at(&scene, 1.0, 0, order), (9.0, 0));
        // u = 0.25, v = 0.75: cells 2 + 6 + 1, odd; behind it 2 + 6 + 0.
        assert_eq!(at(&scene, 2.0, 0, order), (9.0, 0));
        // u = 0.25, v = 0.375: cells 2 + 3 + 1, even: the nearer one.
        assert_eq!(at(&scene, 0.5, 0, order), (5.0, 1));
        assert_eq!(at(&scene, 1.0, FORCE_OPAQUE, order), (5.0, 1));
        assert_eq!(at(&scene, 1.0, CULL_NON_OPAQUE, order), (0.0, 0));
        assert_eq!(at(&scene, 1.0, CULL_OPAQUE, order), (9.0, 0));
    }
    // The instance has it opaque; the ray can still say otherwise.
    scene[0].flags = INSTANCE_FORCE_OPAQUE;
    assert_eq!(at(&scene, 1.0, 0, &order), (5.0, 1));
    assert_eq!(at(&scene, 1.0, FORCE_NON_OPAQUE, &order), (9.0, 0));
    assert_eq!(at(&scene, 1.0, CULL_OPAQUE, &order), (0.0, 0));
    // Opaque geometry an instance has non-opaque.
    scene[0].geometries[0].opaque = true;
    scene[0].flags = INSTANCE_FORCE_NON_OPAQUE;
    assert_eq!(at(&scene, 1.0, 0, &order), (9.0, 0));
}

/// Face culling and the skip flags, which the procedural form applies
/// itself: the device ignores them for boxes.
#[test]
fn culled_faces_and_skipped_triangles_are_not_hit() {
    let sh = shaders();
    // A front face at 5, a back face at 9.
    let [a, b, c] = facing(0.0, 0.0, 9.0);
    let pair = Geometry {
        opaque: true,
        triangles: vec![facing(0.0, 0.0, 5.0), [a, c, b]],
    };
    let mut scene = vec![instance(1, 0, vec![pair])];
    let order = places(&scene);
    let at = |scene: &[Instance], flags: u32| {
        let [tri, pro] = trace(&sh, scene, &along_z(1.0, 0.0, flags), &order);
        assert_eq!(but_kind(tri), but_kind(pro), "the forms agree");
        assert_eq!(pro[3], if tri[3] == 0 { 0 } else { HIT_PROCEDURAL });
        (f32::from_bits(tri[0]), tri[3])
    };
    assert_eq!(at(&scene, 0), (5.0, HIT_FRONT));
    assert_eq!(at(&scene, CULL_BACK), (5.0, HIT_FRONT));
    assert_eq!(at(&scene, CULL_FRONT), (9.0, HIT_BACK));
    assert_eq!(at(&scene, SKIP_TRIANGLES), (0.0, HIT_MISS));
    assert_eq!(at(&scene, SKIP_PROCEDURAL), (5.0, HIT_FRONT));
    assert_eq!(at(&scene, SKIP_PROCEDURAL | CULL_FRONT), (9.0, HIT_BACK));
    // Counter-clockwise fronts: the faces swap.
    scene[0].flags = INSTANCE_COUNTERCLOCKWISE;
    assert_eq!(at(&scene, CULL_BACK), (9.0, HIT_FRONT));
    assert_eq!(at(&scene, CULL_FRONT), (5.0, HIT_BACK));
    // Culling off in the instance; a skip of triangles is not culling.
    scene[0].flags = INSTANCE_CULL_DISABLE;
    assert_eq!(at(&scene, CULL_FRONT), (5.0, HIT_FRONT));
    assert_eq!(at(&scene, SKIP_TRIANGLES), (0.0, HIT_MISS));
}

/// Two primitives at one distance: either may be reported (the device's
/// order decides), and both forms report one of them.
#[test]
fn overlapping_primitives_report_one_of_them() {
    let sh = shaders();
    let twice = Geometry {
        opaque: true,
        triangles: vec![facing(0.0, 0.0, 5.0); 2],
    };
    let scene = [instance(1, 0, vec![twice])];
    let order = places(&scene);
    let reversed: Vec<Place> = order.iter().rev().copied().collect();
    let first = trace(&sh, &scene, &along_z(1.0, 0.0, 0), &order);
    let second = trace(&sh, &scene, &along_z(1.0, 0.0, 0), &reversed);
    // The first enumerated stays: a later one is not closer.
    assert_eq!([first[0][7], first[1][7]], [0, 0]);
    assert_eq!([second[0][7], second[1][7]], [1, 1]);
    assert_eq!(first[0][..3], second[0][..3]);
}

/// Random scenes and rays, the device enumerating in a random order: both
/// forms report a closest acceptable hit (with `ACCEPT_FIRST_HIT`, any
/// acceptable hit), word for word, or a miss when there is none.
#[test]
fn random_scenes_and_rays_report_the_closest_acceptable_hit() {
    let sh = shaders();
    let mut rng = Rng(0x00C0_FFEE);
    let (mut hits, mut misses, mut fronts, mut firsts, mut alpha) = (0, 0, 0, 0, 0);
    for _ in 0..120 {
        let scene: Vec<Instance> = (0..1 + rng.below(4))
            .map(|_| Instance {
                id: rng.next() & 0xFF_FFFF,
                mask: [0xFF, 0xFF, 0x01, 0x82][rng.below(4) as usize],
                flags: [0, 0, 0, 1, 2, 3, 4, 8, 6, 9][rng.below(10) as usize],
                offset: rng.point(4.0),
                geometries: (0..1 + rng.below(3))
                    .map(|_| Geometry {
                        opaque: rng.below(2) == 0,
                        triangles: (0..1 + rng.below(5))
                            .map(|_| {
                                let centre = rng.point(3.0);
                                std::array::from_fn(|_| {
                                    let p = rng.point(2.5);
                                    std::array::from_fn(|k| centre[k] + p[k])
                                })
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        let all = places(&scene);
        for _ in 0..40 {
            // Towards a point of some triangle, or anywhere.
            let target = all[rng.below(all.len() as u32) as usize];
            let instance = &scene[target.instance];
            let [a, b, c] = instance.geometries[target.geometry].triangles[target.primitive];
            let (u, v) = (rng.unit(), rng.unit());
            let (u, v) = if u + v > 1.0 {
                (1.0 - u, 1.0 - v)
            } else {
                (u, v)
            };
            let origin = rng.point(8.0);
            let scale = [0.25, 1.0, 3.0][rng.below(3) as usize];
            let dir: [f32; 3] = std::array::from_fn(|k| {
                let at = a[k] + u * (b[k] - a[k]) + v * (c[k] - a[k]) + instance.offset[k];
                let aim = if rng.below(8) == 0 {
                    rng.within(-1.0, 1.0)
                } else {
                    0.0
                };
                (at - origin[k] + aim) * scale
            });
            // The target is at t = 1 / scale.
            let tmin = [0.0, 0.0, 0.5 / scale][rng.below(3) as usize];
            let tmax = [f32::INFINITY, 4.0 / scale, 0.9 / scale][rng.below(3) as usize];
            let flags = [0, 0, 0, 0x01, 0x02, 0x40, 0x80][rng.below(7) as usize]
                | [0, 0, 0x04][rng.below(3) as usize]
                | [0, 0, 0, 0x10, 0x20, 0x100, 0x200][rng.below(7) as usize];
            let mask = [0xFF, 0xFF, 0x01, 0x80, 0xFFFF_FF02][rng.below(5) as usize];
            let words = ray_words(origin, tmin, dir, tmax, mask, flags);
            let ray = Ray {
                origin,
                tmin,
                dir,
                tmax,
                mask,
                flags,
            };
            let mut order = all.clone();
            for k in (1..order.len()).rev() {
                order.swap(k, rng.below(k as u32 + 1) as usize);
            }
            let got = trace(&sh, &scene, &words, &order);
            let ok = acceptable(&scene, &ray);
            let nearest = ok.iter().map(|s| s.hit.t).fold(f32::INFINITY, f32::min);
            for (form, got) in [(Form::Triangles, got[0]), (Form::Procedural, got[1])] {
                if ok.is_empty() {
                    assert_eq!(got, [0; 8], "{form:?}: {words:x?}");
                    continue;
                }
                let first = flags & ACCEPT_FIRST_HIT != 0;
                let reported = ok
                    .iter()
                    .filter(|s| first || s.hit.t == nearest)
                    .find(|s| record(&scene, form, s) == got);
                assert!(
                    reported.is_some(),
                    "{form:?}: {got:x?} of {ok:?}, {words:x?}"
                );
            }
            // Where one hit is the answer, the forms differ in the kind only.
            if ok
                .iter()
                .filter(|s| flags & ACCEPT_FIRST_HIT != 0 || s.hit.t == nearest)
                .count()
                == 1
            {
                assert_eq!(but_kind(got[0]), but_kind(got[1]));
            }
            match ok
                .iter()
                .find(|s| record(&scene, Form::Triangles, s) == got[0])
            {
                Some(s) => {
                    hits += 1;
                    fronts += u32::from(s.front);
                    firsts += u32::from(flags & ACCEPT_FIRST_HIT != 0 && s.hit.t != nearest);
                    alpha += u32::from(!s.opaque);
                }
                None => misses += 1,
            }
        }
    }
    // The cases the test is for all occurred.
    assert!(hits > 1000 && misses > 1000, "{hits} {misses}");
    assert!(
        fronts > 300 && hits - fronts > 300 && alpha > 200 && firsts > 20,
        "{fronts} {alpha} {firsts}"
    );
}

// ---- refusals ----

/// Runs ray record 0, the one listed, through `shader` against `device`
/// and returns its hit record.
fn run_one(
    shader: &(Flow, Vec<Binding>),
    scene: &[Instance],
    ray: &[u32; 12],
    device: &mut Device,
) -> [u32; 8] {
    let data = triangle_data(scene);
    let ran = run_thread(&shader.0, &shader.1, (0, 1), ray, &data, device);
    assert!(ran.traced);
    stored(&ran, 0)
}

fn refusal(why: u32) -> [u32; 8] {
    [0, 0, 0, HIT_REFUSED | why, 0, 0, 0, 0]
}

/// The procedural form does not call a triangle missed when its test left
/// single precision: the ray's record is a refusal, whatever else the ray
/// hit, and no conversion of a NaN is reached.
#[test]
fn a_test_out_of_range_refuses_the_ray() {
    let sh = shaders();
    let one = |triangle, opaque| {
        let triangles = vec![triangle];
        [instance(3, 0, vec![Geometry { opaque, triangles }])]
    };
    let ray = along_z(0.0, 0.0, 0);
    for opaque in [true, false] {
        for s in [1.0, 1e9, 1e18] {
            let scene = one(wide(s), opaque);
            let mut device = Device::new(&scene, true, places(&scene));
            let got = run_one(&sh.procedural, &scene, &ray, &mut device);
            assert_eq!((got[0], got[3]), (5f32.to_bits(), HIT_PROCEDURAL), "{s}");
        }
        for s in [3e19, 1e20, 1e25] {
            let scene = one(wide(s), opaque);
            let mut device = Device::new(&scene, true, places(&scene));
            let got = run_one(&sh.procedural, &scene, &ray, &mut device);
            assert_eq!(got, refusal(REFUSED_RANGE), "{s}");
            assert_eq!(device.commits, 0);
        }
        // The triangle whose barycentric was NaN: with the alpha rule
        // (geometry not opaque) its conversion to an integer was reached.
        let was_nan = [
            [1.563_227_2e19, 7.216_759e19, 5.0],
            [4.515_855_3e18, 0.652_049_8, 5.0],
            [-8.968_067e18, -7.463_698e18, 5.0],
        ];
        let scene = one(was_nan, opaque);
        let mut device = Device::new(&scene, true, places(&scene));
        let got = run_one(&sh.procedural, &scene, &ray, &mut device);
        assert_eq!(got, refusal(REFUSED_RANGE));
    }
    // A nearer hit in range does not make the ray decided, in either order
    // of enumeration: the triangle out of range may be nearer still.
    let pair = Geometry {
        opaque: true,
        triangles: vec![facing(0.0, 0.0, 2.0), wide(1e25)],
    };
    let scene = [instance(3, 0, vec![pair])];
    let order = places(&scene);
    let reversed: Vec<Place> = order.iter().rev().copied().collect();
    for order in [order, reversed] {
        let mut device = Device::new(&scene, true, order);
        let got = run_one(&sh.procedural, &scene, &ray, &mut device);
        assert_eq!(got, refusal(REFUSED_RANGE));
        assert_eq!(device.commits, 1);
    }
    // The triangles form on the same triangles leaves the answer to the
    // device (here the model's, which is in range at this width).
    let scene = one(wide(1e18), true);
    let mut device = Device::new(&scene, false, places(&scene));
    let got = run_one(&sh.triangles, &scene, &ray, &mut device);
    assert_eq!((got[0], got[3]), (5f32.to_bits(), HIT_FRONT));
    // A direction too short for its reciprocal, which `ray_refusal` lets
    // through: the object-space ray is the device's to give.
    let short = ray_words([0.0; 3], 0.0, [0.0, 0.0, 1e-45], 100.0, 0xFF, 0);
    assert_eq!(ray_refusal(&short), None);
    let scene = one(facing(0.0, 0.0, 5.0), true);
    let mut device = Device::new(&scene, true, places(&scene));
    let got = run_one(&sh.procedural, &scene, &short, &mut device);
    assert_eq!(got, refusal(REFUSED_RANGE));
    // An infinite or absent object-space direction, which only a device's
    // transform could make of a ray the host let through.
    for dir in [
        [0.0, 0.0, f32::INFINITY],
        [f32::NEG_INFINITY, 0.0, 1.0],
        [0.0; 3],
    ] {
        let ray = ray_words([0.0; 3], 0.0, dir, 100.0, 0xFF, 0);
        let mut device = Device::new(&scene, true, places(&scene));
        let got = run_one(&sh.procedural, &scene, &ray, &mut device);
        assert_eq!(got, refusal(REFUSED_RANGE), "{dir:?}");
    }
    // A distance that is infinite beside a finite numerator and denominator.
    let scene = one(FARTHEST, true);
    let ray = ray_words([0.0; 3], 0.0, [0.0, 0.0, 1.0], f32::INFINITY, 0xFF, 0);
    let mut device = Device::new(&scene, true, places(&scene));
    let got = run_one(&sh.procedural, &scene, &ray, &mut device);
    assert_eq!(got, refusal(REFUSED_RANGE));
}

/// Random triangles of every magnitude, one a scene: the procedural form's
/// record is a refusal exactly where [`triangle_test`] is out of range,
/// and otherwise the mirror's hit or miss word for word.
#[test]
fn the_shader_and_the_mirror_agree_at_every_magnitude() {
    let sh = shaders();
    let mut rng = Rng(0x0BAD_CAFE);
    let (mut hits, mut misses, mut refused, mut holes) = (0, 0, 0, 0);
    for _ in 0..6000 {
        let m = |rng: &mut Rng| {
            [1.0, 1.0, 1e6, 1e12, 1e19, -1e20, 3e20][rng.below(7) as usize] * rng.within(-1.0, 1.0)
        };
        let depth = [5.0, 5.0, 1e12, 1e30][rng.below(4) as usize];
        // Around the ray more often than not.
        let mut p: [[f32; 3]; 3] = std::array::from_fn(|_| [m(&mut rng), m(&mut rng), depth]);
        if rng.below(2) == 0 {
            let s = m(&mut rng).abs();
            p = [[-s, -s, depth], [-s, 3.0 * s, depth], [3.0 * s, -s, depth]];
        }
        let dir = [0.0, 0.0, [1.0, 1.0, 1e-20, 1e20][rng.below(4) as usize]];
        let opaque = rng.below(2) == 0;
        let triangles = vec![p];
        let scene = [instance(9, 0, vec![Geometry { opaque, triangles }])];
        let ray = ray_words([0.0; 3], 0.0, dir, f32::INFINITY, 0xFF, 0);
        assert_eq!(ray_refusal(&ray), None);
        let mut device = Device::new(&scene, true, places(&scene));
        let got = run_one(&sh.procedural, &scene, &ray, &mut device);
        let want = match triangle_test([0.0; 3], dir, p) {
            Err(OutOfRange) => {
                refused += 1;
                refusal(REFUSED_RANGE)
            }
            Ok(Some(h)) if h.t > 0.0 && (opaque || alpha_accepts(h.u, h.v, 0)) => {
                hits += 1;
                let (t, u, v) = (h.t.to_bits(), h.u.to_bits(), h.v.to_bits());
                [t, u, v, HIT_PROCEDURAL, 0, 9, 0, 0]
            }
            Ok(found) => {
                holes += u32::from(found.is_some_and(|h| h.t > 0.0));
                misses += 1;
                [0; 8]
            }
        };
        assert_eq!(got, want, "{p:?} {dir:?} opaque {opaque}");
    }
    assert!(
        hits > 300 && misses > 300 && refused > 300 && holes > 30,
        "{hits} {misses} {refused} {holes}"
    );
}

/// A scene that is not the form's: the device offers or commits geometry of
/// the other type, the shader leaves it, and the record says so instead of
/// reading as a miss. A ray that meets none of it is still a miss.
#[test]
fn geometry_of_the_other_type_refuses_the_ray() {
    let sh = shaders();
    let through = along_z(1.0, 0.0, 0);
    for opaque in [true, false] {
        let triangles = vec![facing(0.0, 0.0, 5.0)];
        let scene = [instance(7, 0, vec![Geometry { opaque, triangles }])];
        // Boxes under the triangles form: a procedural candidate.
        let boxes = || {
            let mut device = Device::new(&scene, true, places(&scene));
            device.allowed = RAY_FLAGS;
            device
        };
        let mut device = boxes();
        let got = run_one(&sh.triangles, &scene, &through, &mut device);
        assert_eq!(got, refusal(REFUSED_GEOMETRY));
        assert_eq!(device.commits, 0);
        // The ray skips procedural primitives, or the instance: a miss.
        for ray in [
            along_z(1.0, 0.0, SKIP_PROCEDURAL),
            ray_words([1.0, 0.0, 0.0], 0.0, [0.0, 0.0, 1.0], 100.0, 0x100, 0),
        ] {
            let got = run_one(&sh.triangles, &scene, &ray, &mut boxes());
            assert_eq!(got, [0; 8]);
        }
        // Triangles under the procedural form: the device commits an opaque
        // one itself and offers one that is not.
        let mut device = Device::new(&scene, false, places(&scene));
        let got = run_one(&sh.procedural, &scene, &through, &mut device);
        assert_eq!(got, refusal(REFUSED_GEOMETRY));
        assert_eq!(device.commits, u32::from(opaque));
        let mut device = Device::new(&scene, false, places(&scene));
        let got = run_one(&sh.procedural, &scene, &along_z(50.0, 50.0, 0), &mut device);
        assert_eq!(got, [0; 8]);
    }
}
