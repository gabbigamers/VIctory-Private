# Reference shaders for the inline ray-query emitter

Three HLSL compute shaders written for this project on 2026-11-27, taken
from no external source. They are not built, read or run by anything in the
repository. They are what
Microsoft's compiler was given so that its output could be set beside what
`emit/rayquery.rs` writes (`docs/dxr-shader-emission.md`, "What was
checked, and how").

Sources only. The compiled containers and their disassembly are not kept:
compile them again to look. The compiler is a tool the user supplies, never
part of this repository.

| File | What it is |
| --- | --- |
| `rq_tri.hlsl` | one ray a thread over triangles, a non-opaque candidate committed by a rule on its barycentrics |
| `rq_proc.hlsl` | one ray a thread over procedural (box) primitives, the triangle tested in the shader and committed with `CommitProceduralPrimitiveHit` |
| `all.hlsl` | every one of the 37 `RayQuery` operations called once, the acceleration structure in space 3 |

## The compiler

Microsoft DirectXShaderCompiler, the release for Linux of 2025-07-14
(`linux_dxc_2025_07_14.x86_64.tar.gz`, release v1.8.2505.1, sha256
`f2213da1fc99dc8778c8823078e16ba97c7f80f86a1d4520ab1adf4b462bc48c`). Its
`dxc --version` prints `libdxcompiler.so: 1.9(dev;4950-b106a961);
libdxil.so: 1.9`.

## The command

With the release unpacked at `<dxc>` (its `bin` and `lib` side by side):

    export LD_LIBRARY_PATH=<dxc>/lib
    <dxc>/bin/dxc -T cs_6_6 -E main rq_tri.hlsl -Fo rq_tri.dxil -Fc rq_tri.ll
    <dxc>/bin/dxv rq_tri.dxil

and the same for `rq_proc` and `all`. `-Fo` writes the container, `-Fc` its
disassembly; `dxv` is the validator and prints "Validation succeeded." for
all three. Write the outputs outside the repository.

## What the emitter was checked against

Read from the disassembly, and compared with the emitter's own containers:

* `rq_tri`, `rq_proc`: the acceleration structure's SRV record (nine
  fields, kind 16, sample count 0, the element-type tag `!{i32 0, i32 4}`),
  the handle's properties (16, 0), shader flags 33554448 (bit 25 and bit
  4), the opcodes and argument order of the operations these two use.
* `all`: the same SRV record with space 3 (`i32 3, i32 0, i32 1, i32 16,
  i32 0, !{i32 0, i32 4}`), the flags, `SFI0` `0x100000`, the `PSV0`
  resource record (4, 3, 0, 0, 16, 0); the 37 declarations with their
  opcodes and attributes, equal to the table `DXC` in
  `src/emit/rayquery_tests.rs` with no mismatch; and the shape of `while
  (q.Proceed())`, the same rotated loop the emitter writes.

The comparison was made by reading and by a script outside the repository,
with that compiler only (by the reviewer, by the corrector, and again when
the change was integrated). No test repeats it: the host test
`the_operations_are_declared_as_dxc_declares_them` holds the emitter to the
table, not to a compiler. The reference modules declare validator version
1.9; the emitter's declare 1.6.
