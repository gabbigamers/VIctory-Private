//! The prototype's ray-query shaders on Windows: `dxil.dll` validation of
//! the three containers, pipeline creation on the hardware adapter and on
//! WARP where they report ray-tracing tier 1.1, and a dispatch with a null
//! acceleration structure, which every ray misses, over a hit buffer filled
//! beforehand so that a record written and one left alone differ. Tracing a
//! built scene is the GPU crate's part. Whatever cannot run here says so on
//! stderr, starting with `SKIPPED`.

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;

use super::host::{Dispatch, Host, Root};
use super::rayquery::{Class, Form, GROUP_THREADS, HIT_BYTES, RayQueryShader, Resource, build};
use super::validator::Validator;

const FORMS: [Form; 3] = [Form::Triangles, Form::Procedural, Form::Null];

#[test]
fn the_containers_validate() {
    let Ok(mut validator) = Validator::load(Validator::default_path().as_deref()) else {
        eprintln!("SKIPPED: no dxil.dll; the ray-query containers are not validated");
        return;
    };
    for form in FORMS {
        let mut dxil = build(form).dxil;
        validator
            .validate(&mut dxil)
            .unwrap_or_else(|e| panic!("{form:?}: DXIL validation failed: {e}"));
        eprintln!("{form:?}: validated");
    }
}

/// The device's `D3D12_RAYTRACING_TIER` (0 none, 10 tier 1.0, 11 tier 1.1).
fn raytracing_tier(host: &Host) -> i32 {
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS5::default();
    // SAFETY: the feature query writes the structure of the given size.
    let asked = unsafe {
        host.device.CheckFeatureSupport(
            D3D12_FEATURE_D3D12_OPTIONS5,
            (&raw mut options).cast(),
            size_of_val(&options) as u32,
        )
    };
    asked.map_or(0, |()| options.RaytracingTier.0)
}

/// A root signature with one root parameter per binding, in their order:
/// root constants for the constants, root descriptors for the rest.
fn root_signature(host: &Host, shader: &RayQueryShader) -> Result<ID3D12RootSignature, String> {
    let params: Vec<D3D12_ROOT_PARAMETER1> = shader
        .bindings
        .iter()
        .map(|b| {
            let (ty, anonymous) = match b.class {
                Class::Constants => (
                    D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                    D3D12_ROOT_PARAMETER1_0 {
                        Constants: D3D12_ROOT_CONSTANTS {
                            ShaderRegister: b.register,
                            RegisterSpace: b.space,
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
                            ShaderRegister: b.register,
                            RegisterSpace: b.space,
                            Flags: D3D12_ROOT_DESCRIPTOR_FLAG_NONE,
                        },
                    },
                ),
            };
            D3D12_ROOT_PARAMETER1 {
                ParameterType: ty,
                Anonymous: anonymous,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            }
        })
        .collect();
    let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
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
    // SAFETY: the description points at `params`, which outlives the call.
    let blob: ID3DBlob = unsafe {
        let mut blob = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)
            .map_err(|e| e.message().to_string())?;
        blob.ok_or("no root signature blob")?
    };
    // SAFETY: `blob` stays alive for the lifetime of `bytes`.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    host.root_signature_bytes(bytes)
}

/// What the hit buffer holds before a dispatch, at dword `k`: never 0, and
/// its own place, so a record written elsewhere shows.
fn untouched(k: u32) -> u32 {
    0xA5C3_0000 | k
}

/// Each form's pipeline is created where the device can run it, and runs:
/// 100 rays listed, two groups dispatched, the scene null. Every listed ray
/// misses, so records 0 to 99 become zeros, and the 28 threads past the
/// list write nothing, so theirs still hold what the buffer was filled
/// with. This shows that the driver compiles the query, that the candidate
/// loop ends and where a miss is written, not what a hit looks like.
#[test]
fn pipelines_are_created_and_a_null_scene_is_missed() {
    let _one = super::valu_tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    const RAYS: u32 = 100;
    // Along +z from (n, 0, 0), all of the line, every instance, no flags.
    let rays: Vec<u32> = (0..RAYS)
        .flat_map(|n| {
            let f = [n as f32, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, f32::INFINITY].map(f32::to_bits);
            [
                f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7], 0xFF, 0, 0, 0,
            ]
        })
        .collect();
    let (mut created, mut traced) = (0, 0);
    for (label, warp) in [("hardware", false), ("warp", true)] {
        let Ok(host) = Host::new(warp) else {
            eprintln!("SKIPPED: no SM 6.6 {label} device; no ray-query pipeline is created on it");
            continue;
        };
        let tier = raytracing_tier(&host);
        for form in FORMS {
            if form != Form::Null && tier < D3D12_RAYTRACING_TIER_1_1.0 {
                eprintln!(
                    "SKIPPED: the {label} device reports ray-tracing tier {tier}, below 1.1 \
                     (11); the {form:?} pipeline is not created"
                );
                continue;
            }
            let shader = build(form);
            let root = root_signature(&host, &shader)
                .unwrap_or_else(|e| panic!("{label}: {form:?}: root signature: {e}"));
            let pso = host
                .compute_pipeline_with(&root, &shader.dxil)
                .unwrap_or_else(|e| panic!("{label}: {form:?}: pipeline creation failed: {e}"));
            created += 1;
            let hit_dwords = 2 * GROUP_THREADS * HIT_BYTES / 4;
            let listed = (RAYS * HIT_BYTES / 4) as usize;
            let roots = shader
                .bindings
                .iter()
                .enumerate()
                .map(|(n, b)| {
                    let arg = match b.resource {
                        Resource::Constants => Root::Constants(vec![RAYS, 0, 0, 0]),
                        Resource::Rays => Root::View {
                            srv: true,
                            data: rays.clone(),
                        },
                        // Never read: no candidate comes of a null scene.
                        Resource::Triangles => Root::View {
                            srv: true,
                            data: vec![0; 16],
                        },
                        Resource::Scene => Root::NullSrv,
                        Resource::Hits => Root::UavOf((0..hit_dwords).map(untouched).collect()),
                    };
                    (n as u32, arg)
                })
                .collect();
            let dispatch = Dispatch {
                roots,
                views: &[],
                samplers: &[],
                groups: [2, 1, 1],
            };
            let out = host
                .dispatch(&root, &pso, &dispatch)
                .unwrap_or_else(|e| panic!("{label}: {form:?}: dispatch failed: {e}"));
            assert_eq!(out.len(), 1, "{label}: {form:?}: the hit buffer");
            assert_eq!(out[0].len(), hit_dwords as usize, "{label}: {form:?}");
            assert!(
                out[0][..listed].iter().all(|&w| w == 0),
                "{label}: {form:?}: a listed ray's record is not a miss: a null scene gave a \
                 hit, or the record was not written"
            );
            let past: Vec<u32> = (listed as u32..hit_dwords).map(untouched).collect();
            assert!(
                out[0][listed..] == past,
                "{label}: {form:?}: a thread past the list wrote"
            );
            traced += u32::from(form != Form::Null);
            eprintln!(
                "{label}: {form:?}: pipeline created; {RAYS} misses written, the records past \
                 them untouched"
            );
        }
    }
    assert!(
        created > 0,
        "neither hardware nor WARP SM 6.6 device available"
    );
    if traced == 0 {
        eprintln!(
            "SKIPPED: no device here reports ray-tracing tier 1.1; no shader with a ray query \
             was compiled or run, only the null form"
        );
    }
}
