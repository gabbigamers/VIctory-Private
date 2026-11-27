# Inline ray queries in the DXIL emitter

Experimental: nothing in the translation calls `emit::rayquery`. It gives
the emitter DXR 1.1's inline `RayQuery` and builds the three compute shaders
of the hardware ray-tracing prototype. The synthetic probe
(`crates/victory-rt-probe`; [dxr-validation.md](dxr-validation.md)) makes
its three pipelines from them and has not run. No driver, no device and no Windows
`dxil.dll` has seen these shaders. What has: this host's tests, and
Microsoft's validator and compiler for Linux (below). The last section says
what that leaves open.

## What was added

* `emit/shader.rs`: an acceleration structure as a declared resource
  (`Shader::accel_handle`, `t<n>` in `ACCEL_SPACE` = 3) with its metadata
  record and `PSV0` entry; shader flag bit 25 and feature bit `0x100000`
  when the written module allocates a query. Other shaders are unchanged.
* `emit/rayquery.rs`: the 37 operations (`OPS`; `Shader::ray_op` checks
  argument types and the constant-flags rule), the record layouts,
  `build(Form)` returning container and bindings, and what the host side
  needs: `ray_refusal`, `alpha_accepts`, `triangle_test`, `groups` and
  `MAX_RAYS`, the refusal kinds.
* Test code only (`cfg(test)`): `Flow::run` runs 32-bit float arithmetic,
  compares and conversions; `Module::declaration`; `host::Root::NullSrv`
  and `host::Root::UavOf` (a root UAV with given contents).

## The facts and where they are from

DirectXShaderCompiler, branch `main`, read 2026-11-27; DXR semantics from
DirectX-Specs `d3d/Raytracing.md`.

| Fact | File |
| --- | --- |
| Opcodes 178 to 215 (213 is `GeometryIndex`) | `DxilConstants.h`, `docs/DXIL.rst` |
| Class names, `readonly` on accessors only, overloads | `DxilOperations.cpp`, property table |
| `nounwind` on all; no name suffix for a void overload | same: `GetOpFunc`, `ConstructOverloadName` |
| Parameter types; the query handle is an `i32` | same: the `GetOpFunc` switch; `hctdb.py` |
| `AllocateRayQuery`'s flags must be constant | `DxilValidation.cpp` |
| Flag bit 25, set by `AllocateRayQuery`; validator 1.5 on | `DxilShaderFlags.h/.cpp`, `DxilModule.cpp` |
| Feature bit `0x100000` | `DxilConstants.h`, `DxilShaderFlags.cpp` |
| Resource kind 16, an SRV; properties (16, 0) | `DxilConstants.h`, `DxilResourceProperties.cpp` |
| It does not set the raw-buffer flag | `DxilModule.cpp` |
| `PSV0`: type 4 (SRV raw), kind 16 | `DxilPipelineStateValidation.cpp/.h` |
| SRV record: nine fields, shape 16, sample count 0 | `DxilMetadataHelper.cpp`, `EmitDxilSRV` |
| ... and an element-type tag, `!{i32 0, i32 4}` | same; `HlslTypes.cpp`; `DxilValidation.cpp` |

All of these were then seen in a module DXC compiled ("What was checked").

## The shaders

One ray per thread, groups of 64 × 1 × 1 (reason at `GROUP_THREADS`),
records and bindings as the module notes give them. One dispatch lists at
most `MAX_RAYS` (4,194,240: D3D12's 65,535 groups a dimension); `groups`
returns nothing for more, and the host splits the list and counts the
dispatches.

* **Triangles.** A non-opaque triangle candidate is committed when
  `alpha_accepts` it. The hit, face included, is the device's.
* **Procedural** (the hybrid). The device enumerates boxes; the shader
  loads the triangle by instance, geometry and primitive index, tests it in
  object space and commits. The test is the watertight one of the emulated
  triangle node (`lower/bvh.rs`; Woop, Benthin and Wald), written out again
  in the same order of operations: that code is a method of the lowering
  and could not be shared without touching it. The shader keeps the
  triangle rules: t strictly between tmin and the committed t, face culling
  by the instance's flags, `SKIP_TRIANGLES`, the alpha rule where the
  device reports the candidate non-opaque. Kind 3 carries no face: the
  shader knows it, the record has no place for it.
* **Null.** Writes misses; binds the constants and the hits only and reads
  no ray record. Its time is the dispatch, the count's read and the hit
  writes, so a tracing form's time less the null form's is the ray record's
  loads (three raw-buffer reads a thread), the trace and the candidates
  together. Separating the loads from the trace needs a further form.

Every ray flag is dynamic (an argument of `TraceRayInline`); only
`AllocateRayQuery`'s are constant, and are 0. Front face: D3D12's rule
(clockwise from the ray origin, left-handed, object space; reversed by the
instance's counter-clockwise flag), as `CommittedTriangleFrontFace` returns
it; `triangle_test` reports `clockwise` by the same rule.

### Refusals

A shader that cannot decide a ray does not write a miss. The record's kind
is `HIT_REFUSED` (bit 31) with the reasons in the low bits and the other
seven words 0. The host counts these records and traces their rays another
way. The reference crate judges them the same way: a refusal is an outcome
of its own, counted by reason, never a miss and never malformed; and a hit
of kind 3 is judged as the hit of its triangle with no face to compare
(`victory_rt::rule::judge`, `victory_rt::compare::judge`;
dxr-validation-reference.md, "Records").

* `REFUSED_GEOMETRY` (1): the scene is not the form's. The device offered a
  candidate or committed a hit of the other geometry type: a procedural
  primitive under the triangles form (unless the ray skips them), a
  triangle under the procedural form. The shader leaves such a candidate
  alone. The host should still choose the form by the geometry it built;
  this is the check behind it.
* `REFUSED_RANGE` (2), procedural form only: the test of a candidate left
  single precision (`triangle_test` returns `OutOfRange`). Before this the
  shader wrote a miss for a triangle the ray crosses once a product of two
  coordinates overflowed, and could convert a NaN to an integer. Now the
  test requires the direction's largest component, the denominator and the
  distance's numerator to be finite, and the distance of a met triangle;
  every infinity or NaN earlier in the arithmetic reaches one of them. A
  ray is refused whatever else it hit, since the undecided triangle may be
  nearer.

What is out of range, in the object space the device gives the ray in: any
NaN or infinity; a direction of zero length or with every component below
about 3e-39; finite coordinates so far from the ray's origin that a product
of two of them, or of three over the direction's largest component, passes
2^128 (about 10^19 across the ray; about 10^12 on all three axes under a
direction of length 1). Never out of range: coordinates up to 2^24
(`EXACT_COORDINATE`) under a direction whose largest component is between
2^-24 and 2^24. The small end is not refused: a triangle less than about
10^-19 across as the ray sees it can be misjudged or lose its barycentrics,
and below about 10^-23 it reads as zero-area and misses. The emulated node
test (`lower/bvh.rs`) computes the same products and has both limits, with
no refusal; a reference for comparison has to treat such scenes as outside
the comparison.

## What was checked, and how

* **Host tests** (`emit/rayquery_tests.rs`, 27): the operation table
  against declarations a script listed from DXC's files; `Flow::check`; one
  query handle used directly by every operation; the loop's shape; the
  containers (parts, hash, `SFI0`, `PSV0`); determinism. The shaders are
  then run by the module interpreter against a model of a device's query
  written from the specification: record words, mask, extents, opacity and
  culling flags, both windings, the alpha rule, overlapping primitives, and
  4,800 random rays in random enumeration order, word for word against a
  brute-force list of acceptable hits. Refusals: scenes of the other
  geometry type under each form; triangles at every magnitude (6,000
  random, the procedural form's record against `triangle_test` word for
  word, refusal for `OutOfRange`); `triangle_test` itself over 200,000
  random triangles of large magnitudes (a hit never holds a NaN) and
  200,000 in the stated safe range (none refused). Deliberate faults in the
  builder and the mirror, 38 in all and 17 of them in the refusal paths,
  were each caught. This is about the emitted instructions, not about any
  driver.
* **Unchanged output**: every container the crate's existing host tests
  emit (633, 356 distinct), hashed before and after the emitter change:
  identical, in two independent runs. Again when the change was
  integrated, on its new base: the 545 distinct containers the base's
  host tests finish are the same 545, by hash, with the change in.
* **Microsoft's validator, on Linux**: the three containers pass `dxv` of
  the DirectXShaderCompiler release for Linux of 2025-07-14 (its `dxc
  --version`: `libdxcompiler.so: 1.9(dev;4950-b106a961); libdxil.so:
  1.9`). The modules declare validator version 1.6. The validator does
  check them: with flag bit 25 left out it reports "Flags must match
  usage", and without the element-type tag "Invalid resource return type".
  It was seen to pass a scene handle annotated as a raw buffer, so it does
  not compare an annotation with the resource's kind; and it does not
  compare `PSV0` with the module at this validator version, so that record
  was compared with DXC's instead (next point).
* **Against DXC's output**: an HLSL compute shader calling all 37
  `RayQuery` methods, compiled by that `dxc` for `cs_6_6`, has the same SRV
  record (`i32 3, i32 0, i32 1, i32 16, i32 0, !{i32 0, i32 4}`), handle
  properties (16, 0), flags 33554448, `SFI0` `0x100000` and `PSV0`
  resource record (4, 3, 0, 0, 16, 0); its 37 declarations, opcodes and
  attributes equal the table in the tests with no mismatch; and its `while
  (q.Proceed())` is the same rotated loop (a guard, an empty preheader,
  `Proceed` again in the latch, an exit of its own).
* **Type-checked for Windows only, never run**
  (`emit/rayquery_gpu_tests.rs`): `dxil.dll` validation of the three
  containers; pipeline creation on the hardware adapter and WARP where the
  device reports tier 1.1; a dispatch with a null scene over a hit buffer
  filled beforehand (records 0 to 99 become zeros, the 28 past the list
  keep their contents). Skips print `SKIPPED`, also when no device ran a
  shader with a query.

The validator and compiler are tools outside the repository.
`scripts/validate-dxil.sh` dumps every container the host tests finish and
runs `dxv` over them (`VICTORY_DXV=<dxv> scripts/validate-dxil.sh`). When
this change was integrated it counted 548 containers, three of them these
shaders' (3,872, 5,104 and 2,068 bytes, each accepted and each byte for
byte the container validated before the integration), and rejected the two
it rejected before this change, both from `emit::census_tests`. The HLSL
given to `dxc` is kept as source, with the command, in
`crates/victory-shader/tests/dxc-reference/`; no compiled output is kept.

## Tolerances (proposed; a hardware run decides)

* Triangles form: t, u, v are the device's rounding. Proposed: t within
  2^-16 relative, u and v within 2^-14 absolute of the CPU reference.
* For the reference to flag, not fail: a ray within tolerance of an edge or
  vertex; 8u or 8v within 8 × the barycentric tolerance of an integer (the
  alpha cell); acceptable hits whose t are equal or closer than the
  tolerance (either may be reported); t within tolerance of tmin or tmax;
  `ACCEPT_FIRST_HIT` (any acceptable hit).
* Procedural form: bit for bit `triangle_test` on the object-space ray the
  device supplies, unless the driver fuses the multiply-subtracts. Until a
  device is shown to match exactly, the same tolerances apply.
* Software fallback, counted by the host: rays `ray_refusal` names before
  the dispatch (NaN, infinities, tmin negative or above tmax, a direction
  of zero length, flags outside DXR 1.1's inline set, exclusive pairs);
  records of kind `HIT_REFUSED` after it; scenes with procedural geometry
  or micromaps. The synthetic probe adapts two words on the CPU before it
  asks `ray_refusal`, and counts each: it clears `SKIP_CLOSEST_HIT_SHADER`,
  which changes no hit of an inline query, and cuts the instance mask to
  the low eight bits D3D12 reads (`crates/victory-rt-probe/src/rays.rs`).

## Open points for a Windows run

1. Any driver's compiler. The validator accepts the modules; pipeline
   creation, on hardware or WARP, is untested. Unlike DXC's output, the
   `AllocateRayQuery` call is not in the entry block but in the block after
   the thread-count branch (it still dominates every use).
2. Whether a query handle may flow through a phi or a select in driver
   compilers. Here it never does (one `AllocateRayQuery`, in a block that
   dominates every use), and the validator has no rule on it. Translated
   code is lane-masked SSA: a traversal region turned into a query should
   keep the handle one value in the same way.
3. A root SRV of address 0 as a null acceleration structure (the Windows
   test relies on it; the specification defines it as a miss).
4. Whether the device ends the search after a procedural commit under
   `ACCEPT_FIRST_HIT` (the specification's note 6). If not, the procedural
   form reports the closest hit, still an acceptable one.
5. WARP's reported tier; `dxil.dll` on Windows, which may be another
   version than the Linux library; `PSV0` against the module under a
   validator version of 1.8 or later (these modules declare 1.6).
6. Whether a driver keeps the finite tests (`|x| < inf`) and the IEEE
   results of the divisions they guard; no fast-math flag is set on any
   instruction.
