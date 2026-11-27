//! DXIL emission (decision 0003, step 3): a console vertex, pixel, compute
//! or mesh shader as a PC DXIL container (SM 6.6) that host D3D12 accepts.
//! Compute shaders keep the console's thread-group size; one that depends
//! on the lane layout (cross-lane operations) declares the console's wave
//! size (`WaveSize`), so the host device must support it, unless its thread
//! group carries the console's wave itself ("Emulated waves" below).
//! [`HostShader::wave`] says which, and why.
//!
//! What the host binds for a translated shader ([`HostShader`]):
//!
//! * **User data** at `b0, space 0` (32-bit root constants or a constant
//!   buffer, per stage): dword `index` holds a root parameter's value, as
//!   [`UserData`] lists. A root-constant dword is the constant. A
//!   descriptor table is the byte offset of its first descriptor in its
//!   heap: the descriptor index times 32 for CBV/SRV/UAV tables, times 16
//!   for sampler tables (the console's descriptor sizes; the code adds
//!   offsets in those units). The heap bases of dynamic resources are 0
//!   and not listed.
//! * **Descriptors** through SM 6.6 `ResourceDescriptorHeap[]` and
//!   `SamplerDescriptorHeap[]`, at the index the console address gives, so
//!   the host heaps keep the title's descriptor order. The root signature
//!   needs `CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED` and
//!   `SAMPLER_HEAP_DIRECTLY_INDEXED`.
//! * **Immediate samplers** as static samplers `s<n>, space 0`, `n` the
//!   position in [`HostShader::samplers`] (the console sampler words) and
//!   [`HostShader::static_samplers`] (the same decoded into a D3D12 static
//!   sampler description). Comparison sampling (added 2026-11-23): an
//!   immediate sampler it uses has a comparison filter there (bit 0x80);
//!   a heap sampler it uses must be a comparison sampler on the host. One
//!   of two immediate samplers chosen by uniform control flow takes two
//!   entries; the sampling runs with both and the result is chosen.
//! * **Root CBVs** as root descriptors `b<i>` in [`ROOT_CBV_SPACE`], `i`
//!   the title's root parameter index ([`HostShader::root_cbvs`]); the
//!   address the title sets is the buffer's.
//! * **Root SRVs and root UAVs** (raw buffers; changed 2026-11-24) are
//!   resource-heap views the host makes: a raw SRV (UAV) of the whole host
//!   copy holding the root address, at the heap index the host writes into
//!   the [`HostShader::user_data`] entry marked [`ROOT_SRV_VIEW`]
//!   ([`ROOT_UAV_VIEW`]) (dword `index` = the root descriptor's own first
//!   user-data dword, which such a program never reads as data), and the
//!   root address's byte offset in that copy into the entry marked
//!   [`ROOT_VIEW_OFFSET`] (its second); the shader adds that offset to
//!   every offset it reads at. Host root descriptors have no bounds, so a
//!   shader reading past the copy read other memory or faulted; through
//!   the view it reads 0 and its writes there are dropped. One view per
//!   host copy (and kind) serves every draw of a list. In programs that
//!   use title memory (below) root SRVs and UAVs are title memory instead;
//!   elsewhere [`HostShader::root_srvs`] and [`HostShader::root_uavs`] now
//!   hold only the host's own tables (`VIEW_STRIDES`).
//! * **Vertex fetch** (added 2026-11-23). Grand Theft Auto VI's mesh and
//!   vertex programs fetch vertices themselves with typed-format loads
//!   (`tbuffer_load_format_*`): the format (8_8_8_8 unorm, 16_16 snorm,
//!   2_10_10_10 unorm, 8_8_8_8 and 16_16_16_16 uint...) is in the
//!   instruction, so the translation reads raw bytes and converts them
//!   itself. Their descriptors are built around a root SRV (stride 1, no
//!   record limit): the shader reads it as the root SRV view above. The
//!   vertex index, the stride and the attribute offset are the shader's
//!   own arithmetic on data in that buffer (per-stream headers), not
//!   descriptor fields.
//! * **Buffer views in the heap** are declared by how the shader reads
//!   them: raw (ByteAddressBuffer) for dword loads and stores, atomics and
//!   typed-format loads without an index; structured with the used-resource
//!   entry's stride when indexed; typed with four 32-bit float channels for
//!   `buffer_load_format_*` and `buffer_store_format_*`. The host's view at
//!   that index must be of that kind and start at the console descriptor's
//!   base address (raw: `R32_TYPELESS` with the raw flag), whatever kind the
//!   title's view description says. Typed-format loads from heap
//!   descriptors (1,024 world compute programs, ray-tracing material
//!   resolves) read raw views this way. Buffers read by element through
//!   the resource heap (no used-resource entry, so no stride) are raw too,
//!   addressed at element × the view's stride + offset, the stride from
//!   the view-stride table: root SRV `t`[`VIEW_STRIDES`] in
//!   [`ROOT_CBV_SPACE`], listed in [`HostShader::root_srvs`], whose dword
//!   `i` the host keeps at the byte stride of the buffer view at heap
//!   index `i`.
//! * **Constant buffers in the heap** (added 2026-11-24): scalar buffer
//!   loads through a descriptor taken from the resource heap itself (root
//!   type 5, so no used-resource entry) read a raw buffer view, not a
//!   constant buffer view: the title's constant buffers are 16-byte
//!   aligned, the host's views 256. The host makes a raw view at the
//!   constant buffer's address for each constant buffer view in the heap.
//!   Constant buffers in descriptor tables are read the same way (scalar
//!   loads and per-lane rows), so tables are never copied per draw.
//! * **Rectangle lists** ([`rectangle_gs`]): a geometry shader made from a
//!   vertex shader's outputs completes each rectangle with its fourth
//!   corner opposite the right angle ([`right_angle`]), as the console's
//!   hardware does.
//! * **Heap indices** (added 2026-11-24) are clamped to [`HEAP_VIEWS`] − 1
//!   (samplers [`HEAP_SAMPLERS`] − 1): the host's heaps have at least that
//!   many slots, those past its own null, so an index past the heap (data
//!   still wrong; the console would read whatever memory follows its heap)
//!   reads nothing instead of faulting the device.
//! * **Buffer views at computed heap indices** (added 2026-11-24): a
//!   buffer read through a descriptor the shader takes from the resource
//!   heap itself first looks up the view-stride table (root SRV
//!   `t`[`VIEW_STRIDES`], which then covers every index below
//!   [`HEAP_VIEWS`], dynamic slots too); where it holds 0 (no buffer view
//!   there) the shader reads heap index [`NULL_BUFFER_VIEW`] instead (for
//!   unordered access the index below), which the host keeps a null raw
//!   buffer SRV (UAV). The index comes from data, and a texture descriptor
//!   read as a buffer one is a wild address (the texture's shifted 8 bits
//!   down: GPU faults at low addresses).
//! * **Heap access** ([`HostShader::heap`], [`HostShader::heap_writes`],
//!   added 2026-11-24): whether the shader takes descriptors from the
//!   resource heap itself, and unordered access views among them; the
//!   host then keeps everything the heap's views reach current, and
//!   copies back what such views may have written.
//!
//! * **Texture atomics** (added 2026-11-23): a texture UAV the shader uses
//!   atomically is declared a 32-bit unsigned single-channel view; the
//!   host's view at that index must be an R32 integer view. Global loads
//!   from a root SRV read it raw. 64-bit texture atomics (two dwords per
//!   value; added 2026-11-24) are SM 6.6's on typed heap resources: the
//!   host's view must be `R32G32_UINT` (low dword in R) and the device must
//!   report 64-bit atomics on typed resources (options 9) and on descriptor
//!   heap resources (options 11), else the pipeline is refused.
//!
//! * **Root parameters past the user SGPRs** (added 2026-11-24): a
//!   compute program whose root signature does not fit its 16 user SGPRs
//!   loads the rest from memory whose 40-bit address is in user data 0-1;
//!   the table starts with user-data dword 16. They are user data as any
//!   other: the host fills dword `index` as above.
//! * What shaders write through a root UAV is the title's to see.
//! * **Title memory** (added 2026-11-24, decision 0005's unified-memory
//!   item): accesses at addresses the program computes (pointers in root
//!   constants or loaded from memory, BVH nodes) go through a page table,
//!   the root SRV `t0` in [`MEMORY_SPACE`] ([`HostShader::memory`]), dwords:
//!   4096 level-1 entries for address bits 28-39 (a level-2 block number,
//!   0 for none), then blocks of 4096 entries for bits 16-27 (64 KiB pages),
//!   two dwords each: the resource-heap index of a raw SRV of the host
//!   memory holding the page, whose raw UAV is the next index, and the low
//!   dword of the title address that memory starts at (byte offset =
//!   address low dword - it). Block 0 names null views (its entry 0 is
//!   how the shader tells a miss); addresses at or above 2^40 use it. The
//!   root UAV `u0` in [`MEMORY_SPACE`] ([`HostShader::memory_writes`]) takes
//!   bit `block × 4096 + page` for each page a shader writes (pages of an
//!   access's first and last byte: what the host copies back) and for each
//!   page with no copy an access reached, or for an address whose 256 MiB
//!   region has no block, bit `level-1 index` (block 0's bits). Reads there
//!   give 0, writes are dropped. In such a program (and one with root
//!   parameters past its user SGPRs) root SRVs and UAVs are title memory
//!   too: no root views; the shader reads their addresses as user data
//!   (root type [`RootType::Descriptor`] in [`HostShader::user_data`],
//!   dword 0 the address's low dword, 1 the high), and the host must have
//!   a copy holding each such address.
//! * **View addresses** ([`HostShader::view_addresses`]): a buffer
//!   descriptor's dwords 0-1 read as numbers (the BVH SRV's base address)
//!   and an image descriptor's dwords 1-3 read only for sizes come from the
//!   root SRV `t1` in [`MEMORY_SPACE`]: per resource-heap index, four
//!   dwords, a console descriptor's first four as far as the host knows
//!   them: a buffer view's title address (high dword bits 0-15; stride bits
//!   0), a texture view's width and height - 1 (dword 1 bits 30-31, dword 2
//!   bits 0-13 and 14-29), first and last level (dword 3 bits 12-15, 16-19)
//!   and type (bits 28-31) with an identity swizzle; zeros for anything
//!   else.
//! * **Write marks** ([`HostShader::view_marks`], with
//!   [`Options::view_marks`]; added 2026-11-25): every store or atomic
//!   through an unordered buffer view from the heap (tables, the heap
//!   itself, root UAVs as their heap views) also sets bits in the root UAV
//!   `u`[`VIEW_WRITES`] in [`MEMORY_SPACE`], a bitmap of
//!   [`VIEW_WRITE_BITS`] bits, so the host copies back only what shaders
//!   wrote. The root SRV `t`[`VIEW_MARKS`] there holds two dwords per heap
//!   index: a base bit and a phase (0-65535); a write of byte `x` of the
//!   view (its offset in the view, an element times the stride plus the
//!   offset) sets bit `base + ((phase + x) >> 16)`, for the access's first
//!   and last byte, at most [`VIEW_WRITE_BITS`] − 1. Base
//!   [`VIEW_MARK_NONE`]: nothing is marked. The host gives each copy of
//!   title memory a run of bits, one per 64 KiB title page it touches, and
//!   a view the bit of its first byte's page as base with that byte's
//!   offset in its page as phase. Writes at computed addresses mark the
//!   page-table bitmap instead ("Title memory").
//! * **Read marks** (with [`Options::view_marks`]; added 2026-11-25):
//!   every image instruction that reads a texture shader view the program
//!   takes from the resource heap itself (not through a table, whose slots
//!   the host makes for the draw) also sets bit
//!   [`VIEW_READS`] + the view's heap index in the same bitmap, after the
//!   written pages, where still clear: the host makes views it left null
//!   until a shader reads them (textures the title placed itself).
//! * **Thread limits** (with [`Options::thread_limits`]; added
//!   2026-11-27, off unless the host asks): the console can count a
//!   dispatch in threads instead of thread groups, and then starts only
//!   the threads asked for, the last group of a dimension being partial
//!   (the dispatch initiator's `USE_THREAD_DIMENSIONS` and `PARTIAL_TG_EN`
//!   in AMD's public register headers). A host dispatch is whole groups.
//!   So a compute program reads, for each dimension its group is wider
//!   than one thread, a user-data dword the host sets
//!   ([`RootType::Other`]`(`[`USER_THREAD_LIMIT`]`)` in
//!   [`HostShader::user_data`]: `slot` the dimension, `dword` the group's
//!   size along it, `index` past every root parameter's dwords): the
//!   number of threads that run along that dimension. A thread whose
//!   number in the dispatch (group id times group size, plus its id in the
//!   group) is not below it returns before anything else. That number
//!   is 32 bits and wraps past them: the host must not count a dispatch
//!   in threads whose groups times the group's size exceed
//!   [`THREAD_LIMIT_NONE`] along a dimension.
//!   [`THREAD_LIMIT_NONE`] stops none: the host sets it for every
//!   dispatch counted in groups. Programs with a group barrier, programs
//!   that depend on the lane layout in a group of more than one
//!   dimension, and programs that themselves read user data where the
//!   limits would go (past what the root parameters say they hold) get no
//!   such dwords and are translated as without the option; neither do
//!   groups of one thread, which need none, nor emulated waves
//!   ([`WaveUse`]), whose lanes meet at group barriers.
//!
//! Entry points: [`translate`] (container bytes as the title passes them
//! at pipeline creation → [`HostShader`]), [`emit`] (the same with the
//! earlier stages already run). The root constants of each stage must
//! cover [`HostShader::user_data_dwords`]. Stability: these types and the
//! binding scheme are what pipeline creation builds on; changes are
//! announced.
//! * **Signatures** as the console container has them (same semantics,
//!   registers and masks), so the input layout and stage linking are the
//!   title's; pixel outputs are plain 32-bit values. A pixel shader
//!   declares the outputs its program exports (changed 2026-11-27): one
//!   the container lists and no export writes is left out, so the host
//!   keeps its own depth, coverage and stencil reference and does not
//!   modify that render target. A program that exports an output on some
//!   paths or in some channels only translates with the stores it has,
//!   and one that exports a channel its output does not declare with the
//!   declared channels alone. The shader names each such output with the
//!   reason ([`HostShader::output_notes`]) for the caller to log and
//!   count.
//! * **Mesh shaders** ([`HwStage::Ms`], `ms_6_6`): user data and
//!   descriptors as above (the root constants visible to the mesh stage);
//!   the pipeline is a mesh pipeline (pipeline-state stream, no input
//!   layout) with the title's pixel shader. The thread group is the
//!   console's in lanes (`[numthreads]` in the DXIL), the vertex and
//!   primitive maxima its own, triangles only, no wave size required. The
//!   console numbers groups in x only: the draw path passes the title's
//!   count as `DispatchMesh(x, 1, 1)`. Per-primitive outputs (`PSG1`) are
//!   the console's, without `SV_PrimitiveID` and `SV_CullPrimitive` (the
//!   console never exports them).

mod bitcode;
#[cfg(test)]
mod census_tests;
#[cfg(all(test, windows))]
mod bvh_tests;
mod container;
#[cfg(all(test, windows))]
mod crosslane_tests;
#[cfg(test)]
mod emulated_tests;
#[cfg(all(test, windows))]
mod diff_tests;
#[cfg(all(test, windows))]
mod flow_tests;
#[cfg(all(windows, any(test, feature = "host")))]
pub mod host;
mod lower;
pub mod rayquery;
#[cfg(all(test, windows))]
mod rayquery_gpu_tests;
#[cfg(test)]
mod rayquery_tests;
mod rect;
pub use rect::{rectangle_gs, right_angle};
#[cfg(all(test, windows))]
mod memory_tests;
mod sampler;
#[cfg(all(test, windows))]
mod semantics_tests;
mod shader;
#[cfg(all(test, windows))]
mod stride_tests;
#[cfg(all(test, windows))]
mod synthetic_tests;
#[cfg(windows)]
pub mod validator;
#[cfg(all(test, windows))]
mod valu_tests;
#[cfg(all(test, windows))]
mod waterfall_tests;
mod wave;

use crate::dxbc::{Container, FourCC};
use crate::native::{HwStage, NativeProgram, RootType};
use crate::{ShaderError, io, ir, resource, structure};

pub use sampler::StaticSampler;
pub use shader::{
    ACCEL_SPACE, HEAP_SAMPLERS, HEAP_VIEWS, MEMORY_SPACE, NULL_BUFFER_VIEW, ROOT_CBV_SPACE, SAMPLER_SPACE,
    THREAD_LIMIT_NONE, USER_DATA_SPACE, USER_THREAD_LIMIT, VIEW_INTEGER, VIEW_MARK_NONE, VIEW_MARKS, VIEW_READS, VIEW_SIGNED, VIEW_STRIDE_BITS,
    VIEW_STRIDES, VIEW_WRITE_BITS, VIEW_WRITES,
};

/// The loop limit without an override: 2^20.
pub const DEFAULT_LOOP_LIMIT: u32 = lower::LOOP_LIMIT;

/// The limit on all of a lane's loops together without an override: 2^22.
pub const DEFAULT_LOOP_TOTAL_LIMIT: u32 = lower::LOOP_TOTAL_LIMIT;

/// The iteration count after which a translated loop is left, per lane and
/// per entry into the loop: [`DEFAULT_LOOP_LIMIT`]. A lane that would go
/// round once more is stopped there. A compute, vertex or mesh lane ends
/// at once, a vertex or mesh lane with a position that is not a number,
/// so that no primitive of its vertex is drawn. A pixel is discarded and,
/// where that branch has another side, runs on along it as a helper lane
/// of its quad that writes nothing and goes round no loop again (`lower`'s
/// module `stopped`). Each loop has its own count, which starts at
/// 0 whenever the lane enters that loop from outside it. A loop inside
/// another is counted afresh each time the outer loop comes round to it,
/// also where the two begin at the same instruction (an outer loop whose
/// body starts with its inner loop), and the inner loop's iterations are
/// not counted as the outer loop's. What a lane's loops may run together
/// is [`loop_total_limit`]. The loops are the control-flow graph's: a loop
/// with a conditional `continue` that falls through towards its end is two
/// loops there, the `continue` an inner one that is counted afresh each
/// time the loop's end is taken, so such a loop is bounded by
/// [`loop_total_limit`] and not by this limit.
///
/// With the `diagnostics` feature, `VICTORY_SHADER_LOOP_LIMIT` (decimal or
/// `0x` hex, at least 1) sets it instead. A loop that hits the limit in
/// every lane of a full-screen pass still runs for seconds (2^20 times a
/// 4K screen is about 10^13 iterations); a much lower limit shows whether
/// such loops are what makes a frame slow. The value is part of the
/// translation, so the shader cache must key on it (`victory-gpu`'s does).
pub fn loop_limit() -> u32 {
    static LIMIT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        if cfg!(feature = "diagnostics") {
            let value = std::env::var("VICTORY_SHADER_LOOP_LIMIT").ok();
            parse_limit(value.as_deref(), lower::LOOP_LIMIT)
        } else {
            lower::LOOP_LIMIT
        }
    })
}

/// The number of back edges after which a lane is stopped, over all its
/// loops and every entry into each: [`DEFAULT_LOOP_TOTAL_LIMIT`]. A lane
/// goes round a loop again only while the times it has gone round any
/// loop, that one included, are fewer than this; a lane that would go
/// round once more ends as one [`loop_limit`] stops does. It bounds what a
/// nest of loops can make a lane run, which the limit of each loop does
/// not: that alone allows the product of the loops' limits.
///
/// With the `diagnostics` feature, `VICTORY_SHADER_LOOP_TOTAL_LIMIT`
/// (decimal or `0x` hex, at least 1) sets it instead, read as
/// `VICTORY_SHADER_LOOP_LIMIT` is. Neither value is checked against the
/// other: a loop's count is never above the lane's, so with this limit at
/// or below the loop limit it is the one that stops a lane. The value is
/// part of the translation, so the shader cache must key on it
/// (`victory-gpu`'s does).
pub fn loop_total_limit() -> u32 {
    static LIMIT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        if cfg!(feature = "diagnostics") {
            let value = std::env::var("VICTORY_SHADER_LOOP_TOTAL_LIMIT").ok();
            parse_limit(value.as_deref(), lower::LOOP_TOTAL_LIMIT)
        } else {
            lower::LOOP_TOTAL_LIMIT
        }
    })
}

/// The value of `VICTORY_SHADER_LOOP_LIMIT` or
/// `VICTORY_SHADER_LOOP_TOTAL_LIMIT`; `default` when it is unset,
/// unreadable or 0. Pure, for testing.
fn parse_limit(value: Option<&str>, default: u32) -> u32 {
    let parsed = value
        .map(str::trim)
        .and_then(|v| match v.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => v.parse().ok(),
        });
    match parsed {
        Some(n) if n > 0 => n,
        _ => default,
    }
}

pub use lower::wave_need;

/// The names of the reasons in `needs` ([`wave_need`]'s bits), joined by
/// " + "; "none" for no bit.
pub fn wave_needs_text(needs: u32) -> String {
    let names: Vec<&str> = wave_need::NAMES
        .iter()
        .enumerate()
        .filter(|&(bit, _)| needs >> bit & 1 != 0)
        .map(|(_, &name)| name)
        .collect();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(" + ")
    }
}

/// What a translated compute program asks of the host's waves.
///
/// # Emulated waves
///
/// The console runs a compute wave of 64 (or 32) lanes; a host's waves may
/// have another size, and DXIL's wave operations work on the host's. A
/// program that only uses each lane's own values does not care. One that
/// reads other lanes, or reads a lane mask as a number, does
/// ([`wave_need`]), and is translated in one of two forms:
///
/// * **Host waves** (the default): the program declares the console's wave
///   size and its cross-lane operations are the host's wave operations.
///   Exact where the host runs waves of that size and packs a thread group
///   into them in order; a host that cannot run that size makes no pipeline.
/// * **Emulated waves** ([`Options::emulated_waves`]): the thread group
///   itself is the console's wave. Each thread is a lane, every thread
///   takes the wave's branches, and what one lane learns of another goes
///   through group-shared memory at a group barrier (`lower/emulated.rs`).
///   No wave operation and no wave size, so nothing depends on the host's
///   waves; the lanes' results are the console's on the programs of
///   `emulated_tests`, against a model of the console's wave (no GPU has
///   run one). It costs two barriers for every exchange and one where
///   memory order needs it. Only for a thread group of exactly one console
///   wave; a program outside what it covers is translated the first way
///   and says why ([`WaveUse::refusal`]).
///
/// A program is given its emulated wave where it would otherwise declare a
/// wave size asked for in [`Options::emulated_waves`], and also where it
/// has no per-lane form at all (its structure or its lowering is refused)
/// and its console wave's size is asked for: the wave form takes control
/// flow the per-lane form does not ([`WaveUse::per_lane`] keeps the
/// reason). Where neither form can be made, the per-lane form's refusal is
/// the translation's.
///
/// The loop limits count in an emulated wave as anywhere ([`loop_limit`],
/// [`loop_total_limit`]): each thread counts its own back edges, and since
/// every thread takes the wave's branches the counts are the same in all
/// of them, so a limit stops the whole wave at one back edge and no thread
/// is left waiting at a meeting (`emulated_tests`). An emulated wave takes
/// no thread limits ([`Options::thread_limits`]): a thread that left at
/// entry would never reach the meetings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaveUse {
    /// The wave size the program declares: the host must run waves of
    /// exactly this many lanes. 0: any wave will do.
    pub size: u32,
    /// Why the program's lanes must be the console's: bits of
    /// [`wave_need`]. 0: they need not be.
    pub needs: u32,
    /// The thread group carries the console's wave itself.
    pub emulated: bool,
    /// Why not, where that was asked for and could not be done; empty
    /// otherwise.
    pub refusal: String,
    /// Why the program has no per-lane form, where its emulated wave is
    /// the only translation there is; empty otherwise.
    pub per_lane: String,
}

/// A translated shader and what the host binds for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShader {
    pub stage: HwStage,
    /// The DXIL container, hashed as D3D12 expects.
    pub dxil: Vec<u8>,
    /// Pixel: the outputs whose declaration in the console container and
    /// whose exports in the program do not agree, each with how (module
    /// notes, "Signatures"), in the container's order. The program
    /// translates all the same. Whether a title program has any is not
    /// known: the caller logs them by shader and counts them by reason.
    pub output_notes: Vec<OutputNote>,
    /// The user-data dwords the shader reads.
    pub user_data: Vec<UserData>,
    /// Immediate samplers: the four sampler words of `s<n>`.
    pub samplers: Vec<[u32; 4]>,
    /// The same samplers decoded, `s<n>` at position n: the static sampler
    /// descriptions the host's root signature declares.
    pub static_samplers: Vec<StaticSampler>,
    /// Root CBVs the shader reads: root parameter indices, each bound as
    /// `b<index>` in [`ROOT_CBV_SPACE`].
    pub root_cbvs: Vec<u8>,
    /// Root SRVs (raw buffers) the host binds as root descriptors: `t<index>`
    /// in [`ROOT_CBV_SPACE`] (the host's own tables, and title root SRVs in
    /// programs using title memory; others are views, module notes).
    pub root_srvs: Vec<u8>,
    /// Root UAVs (raw buffers) the host binds as root descriptors:
    /// `u<index>` in [`ROOT_CBV_SPACE`] (likewise).
    pub root_uavs: Vec<u8>,
    /// Whether the shader reads title memory at addresses it computes: the
    /// host binds the page table `t0` in [`MEMORY_SPACE`] (module notes,
    /// "Title memory").
    pub memory: bool,
    /// Whether the host binds the page bitmap `u0` in [`MEMORY_SPACE`]
    /// (pages written and pages missed): whenever [`HostShader::memory`].
    pub memory_writes: bool,
    /// Whether it reads buffer views' title addresses or texture views'
    /// sizes from their descriptors: the host binds the view addresses `t1`
    /// in [`MEMORY_SPACE`] (module notes, "View addresses").
    pub view_addresses: bool,
    /// Whether it takes descriptors from the resource heap at indices it
    /// computes (root type 5, not through a table): the host cannot tell
    /// which views it reads, so every view in the heap must be current.
    pub heap: bool,
    /// Whether some of those are unordered access views: it may write
    /// through any of the heap's.
    pub heap_writes: bool,
    /// Whether it marks the pages it writes through buffer views: the host
    /// binds the view-mark table `t`[`VIEW_MARKS`] and the bitmap
    /// `u`[`VIEW_WRITES`] in [`MEMORY_SPACE`] (module notes, "Write
    /// marks").
    pub view_marks: bool,
    /// What a compute program asks of the host's waves.
    pub wave: WaveUse,
}

impl HostShader {
    /// Size of the user-data buffer the shader declares, in dwords (a
    /// multiple of 4; 0 when it reads none). Root constants must cover it.
    pub fn user_data_dwords(&self) -> u32 {
        let n = self
            .user_data
            .iter()
            .map(|u| u.index + 1)
            .max()
            .unwrap_or(0);
        n.next_multiple_of(4)
    }
}

/// A pixel output the container declares in one way and the program
/// exports in another ([`HostShader::output_notes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputNote {
    pub reason: OutputReason,
    /// The output's semantic name, a render target's with its number
    /// (`SV_Target1`, `SV_Depth`).
    pub output: String,
}

impl std::fmt::Display for OutputNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.output, self.reason.text())
    }
}

/// How a pixel output's exports differ from its declaration. None of them
/// stops the translation; what the shader does in each case is the
/// lowering's to say (`Lower::pixel_outputs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutputReason {
    /// No export a lane can run writes a channel the output declares: the
    /// shader does not declare it. The host leaves that render target
    /// unmodified and keeps its own depth, stencil reference or coverage,
    /// also where the console's stage data says it takes them from the
    /// program.
    NeverExported,
    /// An export writes a channel past the ones the output's element
    /// declares (four channels to a target declared with three): the
    /// shader stores the declared channels alone.
    PastElement,
    /// A declared channel that no export writes, in a lane that is not
    /// discarded: the shader stores the exported ones and leaves that
    /// channel without a value.
    NeverWritten,
    /// A lane that is not discarded can end without an export of the
    /// output that other paths have: the shader stores it where the
    /// program exports it and nowhere else.
    SomePaths,
}

impl OutputReason {
    /// Every reason, in the order of [`OutputReason::code`].
    pub const ALL: [OutputReason; 4] = [
        OutputReason::NeverExported,
        OutputReason::PastElement,
        OutputReason::NeverWritten,
        OutputReason::SomePaths,
    ];

    /// The reason as logs and tallies name it.
    pub fn text(self) -> &'static str {
        match self {
            OutputReason::NeverExported => "never exported",
            OutputReason::PastElement => "channels exported past the element",
            OutputReason::NeverWritten => "channels never written",
            OutputReason::SomePaths => "exported on some paths only",
        }
    }

    /// Its place in [`OutputReason::ALL`]: what a stored translation keeps.
    pub fn code(self) -> usize {
        self as usize
    }
}

/// [`UserData::dword`] of a root descriptor's entry that takes the heap
/// index of the host's raw SRV of it (module notes, "Root SRVs and root
/// UAVs"); dwords 0 and 1 are its address (programs using title memory).
pub const ROOT_SRV_VIEW: u32 = 2;
/// As [`ROOT_SRV_VIEW`], for the raw UAV of a root UAV.
pub const ROOT_UAV_VIEW: u32 = 3;
/// [`UserData::dword`] of a root descriptor's entry that takes its
/// address's byte offset in the view [`ROOT_SRV_VIEW`] or [`ROOT_UAV_VIEW`]
/// names.
pub const ROOT_VIEW_OFFSET: u32 = 4;

/// One dword of user data and the root parameter that fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserData {
    /// Dword in the user-data buffer (the console's user-data index).
    pub index: u32,
    /// Root parameter index in the title's root signature.
    pub slot: u8,
    /// `Constants`, `Table` or `Descriptor` (see the module notes for the
    /// value).
    pub root: RootType,
    /// Dword within the parameter; for a root descriptor
    /// [`ROOT_SRV_VIEW`], [`ROOT_UAV_VIEW`] or [`ROOT_VIEW_OFFSET`] name
    /// its view's heap index or its offset in that view instead.
    pub dword: u32,
}

/// How a translation is made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Writes through buffer views mark the pages they write (module notes,
    /// "Write marks"); the host must then bind the view-mark table and the
    /// bitmap ([`HostShader::view_marks`]).
    pub view_marks: bool,
    /// Compute programs read, per dimension their thread group is wider
    /// than one thread, how many threads of the dispatch run along it, and
    /// the others leave at entry (module notes, "Thread limits"): the host
    /// must set each [`USER_THREAD_LIMIT`] dword of
    /// [`HostShader::user_data`] before every dispatch.
    pub thread_limits: bool,
    /// The console wave sizes whose lane-dependent compute programs are
    /// translated as emulated waves, and whose compute programs without a
    /// per-lane form are given one where it can be made ([`WaveUse`]): a
    /// set of [`emulate`]'s bits. 0, the default: none, and every
    /// translation and refusal is as it was before the option existed.
    pub emulated_waves: u8,
}

/// [`Options::emulated_waves`]: the console's two wave sizes.
pub mod emulate {
    pub const WAVE32: u8 = 1;
    pub const WAVE64: u8 = 2;

    /// The bit of a wave of `lanes` lanes; 0 for a size the console has not.
    pub const fn bit(lanes: u32) -> u8 {
        match lanes {
            32 => WAVE32,
            64 => WAVE64,
            _ => 0,
        }
    }
}

/// Translates the vertex, pixel, compute or mesh program of a console
/// shader container, without write marks.
pub fn translate(container: &[u8]) -> Result<HostShader, ShaderError> {
    translate_with(container, Options::default())
}

/// [`translate`] made as `options` say.
pub fn translate_with(container: &[u8], options: Options) -> Result<HostShader, ShaderError> {
    let c = Container::parse(container)?;
    let dxil = c
        .part(FourCC::DXIL)
        .or(c.part(FourCC::ACCC))
        .ok_or(ShaderError::Malformed("no DXIL part"))?;
    let native = crate::native_programs(dxil.data)?
        .into_iter()
        .find(|p| {
            matches!(
                p.stage,
                HwStage::Vs | HwStage::Ps | HwStage::Cs | HwStage::Ms
            )
        })
        .ok_or(ShaderError::Unsupported {
            offset: 0,
            what: "a container without a vertex, pixel, compute or mesh program",
        })?;
    // Structurisation first: tracing per-lane descriptors needs it (its
    // failure is reported after the earlier stages').
    let (p, st) = crate::range::build(&native.patched_code(), native.wave64, native.vgprs)?;
    let res = resource::track(&p, &native.layout, native.stage, st.as_ref().ok());
    let io = io::raise(&p, &native);
    emit_stages(&c, &native, &p, st, io, res, options)
}

/// [`emit_with`] on the earlier stages' results, whose failures are the
/// translation's in this order: resources, I/O, structure. A compute
/// program whose per-lane structure is refused is given its emulated wave
/// where `options` ask for it and it can be made ([`WaveUse`]).
fn emit_stages(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: Result<structure::Structured, ShaderError>,
    io: Result<io::Io, ShaderError>,
    res: Result<resource::Resources, ShaderError>,
    options: Options,
) -> Result<HostShader, ShaderError> {
    match (res, io, st) {
        (Ok(res), Ok(io), Ok(st)) => emit_with(c, native, p, &st, &io, &res, options),
        // No per-lane form (the resources were then traced without it,
        // and may have failed for that).
        (res, Ok(io), Err(no_form)) => {
            if let ShaderError::Unsupported { what, .. } = no_form
                && options.emulates(native)
                && let Ok(mut shader) = emit_emulated(c, native, p, &io, options)
            {
                shader.wave.per_lane = what.into();
                return Ok(shader);
            }
            Err(res.err().unwrap_or(no_form))
        }
        (Err(e), ..) | (_, Err(e), _) => Err(e),
    }
}

/// Emits a program whose earlier stages already ran, without write marks.
pub fn emit(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
) -> Result<HostShader, ShaderError> {
    emit_with(c, native, p, st, io, res, Options::default())
}

/// [`emit`] made as `options` say.
pub fn emit_with(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
    options: Options,
) -> Result<HostShader, ShaderError> {
    let sigs = Signatures::of(c, native)?;
    let per_lane = sigs.per_lane(native, p, st, io, res, options);
    let mut l = match per_lane {
        Ok(l) => l,
        // No per-lane form: the wave form alone, where the caller wants
        // the console's wave of this size carried by the program.
        Err(ShaderError::Unsupported { offset, what }) if options.emulates(native) => {
            return match sigs.emulated(native, p, io, options) {
                Ok(e) => {
                    let wave = WaveUse {
                        needs: e.wave_needs,
                        emulated: true,
                        per_lane: what.into(),
                        ..WaveUse::default()
                    };
                    Ok(host_shader(native, e, wave))
                }
                Err(_) => Err(ShaderError::Unsupported { offset, what }),
            };
        }
        Err(e) => return Err(e),
    };
    let mut wave = WaveUse {
        size: l.wave_size.unwrap_or(0),
        needs: l.wave_needs,
        ..WaveUse::default()
    };
    // A program that would ask the host for the console's wave, where the
    // caller wants such programs to carry the wave themselves: its wave
    // form, or the form above with the reason there is no other.
    if wave.size != 0 && options.emulated_waves & emulate::bit(wave.size) != 0 {
        match sigs.emulated(native, p, io, options) {
            Ok(e) => {
                l = e;
                wave.size = 0;
                wave.emulated = true;
            }
            Err(ShaderError::Unsupported { what, .. }) => wave.refusal = what.into(),
            Err(e) => wave.refusal = e.to_string(),
        }
    }
    Ok(host_shader(native, l, wave))
}

/// The emulated wave of a compute program alone ([`WaveUse`]), whatever
/// wave sizes `options` name and whether or not the program has a per-lane
/// form: for a caller that reports what each form comes to, and for a
/// program whose per-lane structure is refused. The refusal is the wave
/// form's own.
pub fn emit_emulated(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    io: &io::Io,
    options: Options,
) -> Result<HostShader, ShaderError> {
    let l = Signatures::of(c, native)?.emulated(native, p, io, options)?;
    let wave = WaveUse {
        needs: l.wave_needs,
        emulated: true,
        ..WaveUse::default()
    };
    Ok(host_shader(native, l, wave))
}

impl Options {
    /// Whether these options ask for `native`'s console wave to be carried
    /// by the program: a compute program whose wave size is among
    /// [`Options::emulated_waves`].
    fn emulates(&self, native: &NativeProgram) -> bool {
        let lanes = if native.wave64 { 64 } else { 32 };
        native.stage == HwStage::Cs && self.emulated_waves & emulate::bit(lanes) != 0
    }
}

/// A container's signature parts as the lowering takes them.
struct Signatures {
    inputs: Vec<container::SigElement>,
    outputs: Vec<container::SigElement>,
    prims: Vec<container::SigElement>,
    mesh_stride: u32,
}

impl Signatures {
    fn of(c: &Container, native: &NativeProgram) -> Result<Signatures, ShaderError> {
        let sig = |fourcc| -> Result<Vec<container::SigElement>, ShaderError> {
            match c.part(fourcc) {
                Some(part) => signature(part.data),
                None => Ok(Vec::new()),
            }
        };
        let pixel = native.stage == HwStage::Ps;
        let mut inputs = sig(FourCC::ISG1)?;
        let mut outputs = sig(FourCC::OSG1)?;
        // Interpolation as DXC declares it by default; pixel inputs that are
        // read get the mode their barycentrics say.
        let interpolated = if pixel { &mut inputs } else { &mut outputs };
        for e in interpolated {
            e.interp = match (e.kind, e.comp) {
                (3, _) => 4,
                // Constant, as DXC declares it (the AMD compiler crashed on a
                // front face without a mode).
                (13, _) => 1,
                (0 | 6 | 7, 9) => 2,
                (0, _) => 1,
                _ => 0,
            };
        }
        // (With a conservative depth output the position is centroid: the
        // lowering sets that, by the outputs the shader ends up declaring.)
        // Per-primitive outputs are constant. The console never exports a
        // primitive id or cull flag a mesh shader writes (it culls with the
        // primitive export), so those rows are left out.
        let mut prims = sig(FourCC::PSG1)?;
        prims.retain(|e| !matches!(e.kind, 10 | 30));
        for e in &mut prims {
            e.interp = 1;
        }
        // A mesh group's id arrives multiplied by max(threads, vertices,
        // primitives) (the code divides by that); the thread count is in PSV0
        // from runtime info version 2 on.
        // shortcut: older containers fall back to the group's lane count, right
        // when the HLSL group fills its waves (Grand Theft Auto VI's do).
        let mesh_stride = match &native.io {
            crate::native::StageIo::Ms(ms) => {
                let psv = c.part(FourCC::PSV0).map(|p| p.data);
                let threads = psv
                    .filter(|d| crate::dxbc::u32_at(d, 0).is_ok_and(|n| n >= 48))
                    .and_then(|d| crate::dxbc::u32_at(d, 4 + 36).ok());
                threads.map_or(ms.threads, |t| {
                    t.max(ms.max_vertices).max(ms.max_primitives)
                })
            }
            _ => 0,
        };
        Ok(Signatures {
            inputs,
            outputs,
            prims,
            mesh_stride,
        })
    }

    /// What the lowering of one form of the program takes.
    fn parts<'a>(
        &self,
        native: &'a NativeProgram,
        p: &'a ir::Program,
        st: &'a structure::Structured,
        io: &'a io::Io,
        res: &'a resource::Resources,
        options: Options,
    ) -> lower::Parts<'a> {
        lower::Parts {
            native,
            p,
            st,
            io,
            res,
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
            prims: self.prims.clone(),
            mesh_stride: self.mesh_stride,
            view_marks: options.view_marks,
        }
    }

    /// The program lowered in its per-lane form: what [`emit_with`] makes
    /// its host shader of, where no wave form takes its place.
    fn per_lane(
        &self,
        native: &NativeProgram,
        p: &ir::Program,
        st: &structure::Structured,
        io: &io::Io,
        res: &resource::Resources,
        options: Options,
    ) -> Result<lower::Lowered, ShaderError> {
        let parts = |st, res| self.parts(native, p, st, io, res, options);
        match lower::lower_with(&parts(st, res), options.thread_limits) {
            // Discarded lanes kept on as helpers can read what no surviving
            // lane does (a register only killed lanes skipped writing); such a
            // program kills them as the console does (`structure::killing`).
            Err(_) if !st.discards.is_empty() => {
                let st = structure::killing(p)?;
                let res = resource::track(p, &native.layout, native.stage, Some(&st))?;
                lower::lower_with(&parts(&st, &res), options.thread_limits)
            }
            l => l,
        }
    }

    /// The program lowered in its wave form (`structure::waves`), with
    /// its resources traced on that form.
    fn emulated(
        &self,
        native: &NativeProgram,
        p: &ir::Program,
        io: &io::Io,
        options: Options,
    ) -> Result<lower::Lowered, ShaderError> {
        let st = structure::waves(p)?;
        let res = resource::track(p, &native.layout, native.stage, Some(&st))?;
        // (With thread limits asked for too, the wave form takes none: a
        // thread that left at entry would never reach the lanes' meetings.
        // `Lower::limit_threads` leaves such a program as it is.)
        let parts = self.parts(native, p, &st, io, &res, options);
        let l = lower::lower_with(&parts, options.thread_limits)?;
        debug_assert!(l.emulated && l.wave_size.is_none());
        Ok(l)
    }
}

/// The per-lane lowering [`emit_with`] makes its host shader of, for the
/// tests that read its function.
#[cfg(test)]
fn lowered(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
    options: Options,
) -> Result<lower::Lowered, ShaderError> {
    Signatures::of(c, native)?.per_lane(native, p, st, io, res, options)
}

/// A lowered program as its caller binds it.
fn host_shader(native: &NativeProgram, l: lower::Lowered, wave: WaveUse) -> HostShader {
    HostShader {
        stage: native.stage,
        dxil: l.dxil,
        output_notes: l.output_notes,
        user_data: l.user_data,
        static_samplers: l
            .samplers
            .iter()
            .enumerate()
            .map(|(n, &w)| {
                let mut s = StaticSampler::from_words(w);
                if l.cmp_samplers >> n & 1 != 0 {
                    s.filter |= 0x80;
                }
                s
            })
            .collect(),
        samplers: l.samplers,
        root_cbvs: l.root_cbvs,
        root_srvs: l.root_srvs,
        root_uavs: l.root_uavs,
        memory: l.memory,
        memory_writes: l.memory_writes,
        view_addresses: l.view_addresses,
        heap: l.heap,
        heap_writes: l.heap_writes,
        view_marks: l.view_marks,
        wave,
    }
}

/// The rows of an `ISG1`/`OSG1` part.
fn signature(part: &[u8]) -> Result<Vec<container::SigElement>, ShaderError> {
    use crate::dxbc::u32_at;
    let count = u32_at(part, 0)? as usize;
    let base = u32_at(part, 4)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = base + 32 * i;
        let name_at = u32_at(part, at + 4)? as usize;
        let name = part
            .get(name_at..)
            .and_then(|s| s.split(|&b| b == 0).next())
            .ok_or(ShaderError::Malformed("signature name out of bounds"))?;
        let mask = *part
            .get(at + 24)
            .ok_or(ShaderError::Malformed("truncated"))?;
        if u32_at(part, at + 28)? != 0 {
            return Err(ShaderError::Unsupported {
                offset: 0,
                what: "a minimum-precision signature element",
            });
        }
        if u32_at(part, at)? != 0 {
            return Err(ShaderError::Unsupported {
                offset: 0,
                what: "a signature element of another stream",
            });
        }
        let kind = match u32_at(part, at + 12)? {
            0 => 0,
            1 => 3,
            2 => 6,
            3 => 7,
            4 => 4,
            5 => 5,
            6 => 1,
            7 => 10,
            8 => 2,
            9 => 13,
            10 => 12,
            64 => 16,
            65 => 17,
            66 => 14,
            67 => 19,
            68 => 18,
            69 => 20,
            70 => 15,
            25 => 30, // cull primitive
            _ => {
                return Err(ShaderError::Unsupported {
                    offset: 0,
                    what: "a system value in the signature",
                });
            }
        };
        // 16-bit elements are declared 32-bit; reading or writing one
        // converts (16-bit values travel packed on the console).
        let raw = u32_at(part, at + 16)?;
        let comp = match (kind, raw) {
            // DXIL declares the front face a uint (the validator insists).
            (13, _) => 5,
            (_, 1 | 4) => 5,
            (_, 2 | 5) => 4,
            (_, 3 | 6) => 9,
            (_, c) => {
                return Err(ShaderError::Unsupported {
                    offset: 0,
                    what: match c {
                        0 => "a signature element of unknown type",
                        _ => "a 64-bit signature element",
                    },
                });
            }
        };
        let register = u32_at(part, at + 20)?;
        out.push(container::SigElement {
            name: String::from_utf8_lossy(name).into_owned(),
            index: u32_at(part, at + 8)?,
            kind,
            comp,
            interp: 0,
            row: (register != u32::MAX).then_some(register),
            col: mask.trailing_zeros().min(3) as u8,
            cols: mask.count_ones().max(1) as u8,
            usage: 0,
            half: (4..=6).contains(&raw),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::bitcode::Attr;
    use super::lower::{LOOP_LIMIT, LOOP_TOTAL_LIMIT};
    use super::parse_limit;

    #[test]
    fn the_loop_limit_switches_parse_or_fall_back() {
        for default in [LOOP_LIMIT, LOOP_TOTAL_LIMIT] {
            assert_eq!(parse_limit(None, default), default);
            assert_eq!(parse_limit(Some("4096"), default), 4096);
            assert_eq!(parse_limit(Some(" 0x1000 "), default), 4096);
            assert_eq!(parse_limit(Some("1"), default), 1);
            for bad in ["", "0", "-5", "lots", "0x", "4294967296"] {
                assert_eq!(parse_limit(Some(bad), default), default, "{bad:?}");
            }
        }
    }

    /// The limits as their documentation and the README give them. The
    /// one on a lane's loops together is the larger: at or below the
    /// other, no loop could reach its own.
    #[test]
    fn the_loop_limits_are_the_documented_ones() {
        assert_eq!((LOOP_LIMIT, LOOP_TOTAL_LIMIT), (1 << 20, 1 << 22));
    }
    use super::bitcode::{Arg, FlowOp};
    use super::container::SigElement;
    use super::shader::Shader;
    use super::*;
    use crate::native::{ComputeIo, StageIo};

    #[test]
    fn indexed_move_initializes_an_input_before_its_first_use() {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Case {
            Complete,
            NonzeroBase,
            WrongStart,
            SkippedSlot,
            Decrement,
            Wrap,
            UnknownStart,
            ShortLoop,
            ClobberedScc,
            EarlyExit,
            DivergentExit,
            MaskedWrite,
            SourceIsOld,
            ReadBeforeWrite,
            ReadOnBackedge,
        }
        use Case::*;

        // A two-block do-while loop writes v[M0] from v2, then exports v1.
        // On the first two passes M0 selects v0 and v1 respectively. Both
        // registers initially contain raw perspective-center barycentrics.
        let words = |case| {
            let init = match case {
                WrongStart => 0xbe80_0382,   // s_mov_b32 s0, 2
                Wrap => 0xbe80_03c1,         // s_mov_b32 s0, -1
                UnknownStart => 0x8000_8080, // s_add_u32 s0, 0, 0
                _ => 0xbe80_0380,            // s_mov_b32 s0, 0
            };
            let mut w = vec![init, 0x7e04_0281, 0xbf82_0000]; // v_mov_b32 v2, 1; s_branch header
            let latch = w.len();
            if case == EarlyExit {
                w.extend([0xbf06_8000, 0xbf85_0000]); // s_cmp_eq_u32 s0, 0; s_cbranch_scc1 exit
            }
            w.push(0xbe80_0302); // s_mov_b32 s0, s2
            let header = w.len();
            w[2] |= (header - 3) as u32;
            w.push(0xbefc_0300); // s_mov_b32 m0, s0
            w.push(match case {
                SkippedSlot => 0x8102_8200, // s_add_i32 s2, s0, 2
                Decrement => 0x8102_c100,   // s_add_i32 s2, s0, -1
                _ => 0x8102_8100,           // s_add_i32 s2, s0, 1
            });
            if case == ReadBeforeWrite {
                w.push(0x7e06_0301);
            } // v_mov_b32 v3, v1
            if case == MaskedWrite {
                w.extend([0x7d84_0480, 0xbe84_3c6a]); // v_cmp_eq_u32 vcc, 0, v2; s_and_saveexec_b32 s4, vcc_lo
            }
            w.push(match case {
                SourceIsOld => 0x7e00_8501, // v_movreld_b32 v0, v1
                NonzeroBase => 0x7e02_8502, // v_movreld_b32 v1, v2
                _ => 0x7e00_8502,           // v_movreld_b32 v0, v2
            });
            if case == MaskedWrite {
                w.push(0x887e_047e);
            } // s_or_b32 exec_lo, exec_lo, s4
            if case == ReadOnBackedge {
                w.push(0x7e06_0301);
            } // v_mov_b32 v3, v1
            let branch = if case == DivergentExit {
                w.push(0x7d84_0480); // v_cmp_eq_u32 vcc, 0, v2
                0xbf87_0000 // s_cbranch_vccnz latch
            } else {
                w.push(if matches!(case, ShortLoop | NonzeroBase) {
                    0xbf06_8102
                } else {
                    0xbf06_8402
                }); // s_cmp_eq_u32 s2, 1/4
                if case == ClobberedScc {
                    w.push(0xbf06_8000); // s_cmp_eq_u32 s0, 0 exits on the first pass
                }
                0xbf84_0000 // s_cbranch_scc0 latch
            };
            let rel = latch as i32 - (w.len() as i32 + 1);
            w.push(branch | u32::from(rel as u16));
            let exit = w.len();
            if case == EarlyExit {
                w[latch + 1] |= (exit - latch - 2) as u32;
            }
            let output = if matches!(case, ReadBeforeWrite | ReadOnBackedge) {
                3
            } else {
                1
            };
            w.extend([0xf800_1801, output, 0xbf81_0000]); // exp mrt0.x done vm; s_endpgm
            w
        };
        let target = SigElement {
            name: "SV_Target".into(),
            index: 0,
            kind: 16,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 1,
            usage: 1,
            half: false,
        };
        let sig = super::container::signature(&[target], false);
        let bytes = crate::dxbc::build(&[(FourCC::OSG1, &sig)]);
        let c = Container::parse(&bytes).unwrap();
        for case in [
            Complete,
            NonzeroBase,
            WrongStart,
            SkippedSlot,
            Decrement,
            Wrap,
            UnknownStart,
            ShortLoop,
            ClobberedScc,
            EarlyExit,
            DivergentExit,
            MaskedWrite,
            SourceIsOld,
            ReadBeforeWrite,
            ReadOnBackedge,
        ] {
            let native = NativeProgram {
                stage: HwStage::Ps,
                wave64: false,
                vgprs: 4,
                layout: vec![],
                used: vec![],
                io: StageIo::Ps {
                    inputs: vec![],
                    input_addr: 2,
                    db_shader_control: 0,
                    patches: vec![],
                },
                code: words(case).iter().flat_map(|w| w.to_le_bytes()).collect(),
            };
            let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
            let st = structure::structure(&p).unwrap();
            let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let shader = emit(&c, &native, &p, &st, &io, &res);
            if matches!(case, Complete | NonzeroBase) {
                let shader = shader.unwrap();
                assert!(
                    Container::parse(&shader.dxil)
                        .unwrap()
                        .part(FourCC::DXIL)
                        .is_some()
                );
                #[cfg(windows)]
                if let Ok(mut validator) =
                    validator::Validator::load(validator::Validator::default_path().as_deref())
                {
                    let mut dxil = shader.dxil;
                    validator.validate(&mut dxil).unwrap();
                    eprintln!("synthetic {case:?} SDK valid bytes={}", dxil.len());
                }
            } else {
                eprintln!("synthetic {case:?} {:?}", shader.as_ref().err());
                assert!(
                    matches!(
                        shader,
                        Err(ShaderError::Unsupported {
                            what: "barycentrics read directly",
                            ..
                        })
                    ),
                    "{case:?}"
                );
            }
        }
    }

    /// A pixel shader writing SV_StencilRef (channel y of `exp mrtz`).
    /// DXC sets ShaderFlags.StencilRef (bit 11) for a pixel shader whose
    /// output signature has SV_StencilRef, and the feature bit
    /// D3D_SHADER_FEATURE_STENCIL_REF (0x200) in SFI0; the validator checks
    /// both against the module.
    #[test]
    fn a_stencil_export_declares_the_stencil_ref_feature() {
        let words = [
            0x7E00_02F2u32, // v_mov_b32 v0, 1.0
            0x7E02_0285,    // v_mov_b32 v1, 5
            0xF800_1883,
            0x0000_0100, // exp mrtz v0, v1, off, off done vm
            0xBF81_0000,
        ];
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 2,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let el = |name: &str, kind, comp| SigElement {
            name: name.into(),
            index: 0,
            kind,
            comp,
            interp: 0,
            row: None,
            col: 0,
            cols: 1,
            usage: 1,
            half: false,
        };
        let osg1 =
            container::signature(&[el("SV_Depth", 17, 9), el("SV_StencilRef", 20, 5)], false);
        let bytes = crate::dxbc::build(&[(FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let out = emit(&c, &native, &p, &st, &io, &res).unwrap();
        // dxil.dll, where there is one, checks the flags against the module.
        #[cfg(windows)]
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut out.dxil.clone()).unwrap();
        }
        let host = Container::parse(&out.dxil).unwrap();
        let sfi0 = host.part(FourCC::SFI0).unwrap().data;
        let features = u64::from_le_bytes(sfi0.try_into().unwrap());
        assert_ne!(features & 0x200, 0, "SFI0 {features:#x}: no StencilRef");
    }

    /// One-channel pixel outputs as a console container declares them.
    fn pixel_output(name: &str, kind: u8, row: Option<u32>) -> SigElement {
        SigElement {
            name: name.into(),
            index: row.unwrap_or(0),
            kind,
            comp: if matches!(kind, 14 | 20) { 5 } else { 9 },
            interp: 0,
            row,
            col: 0,
            cols: 1,
            usage: 0,
            half: false,
        }
    }

    /// The outputs a host shader declares, by name and register.
    type Declared = Vec<(String, Option<u32>)>;

    /// What `make` (as [`emit`] or [`lowered`]) gives for the pixel
    /// program `words` in a container declaring `inputs` and `outputs`,
    /// structurised with the discarded lanes kept as helpers or
    /// (`killing`) taken out.
    fn pixel_program<T>(
        words: &[u32],
        db_shader_control: u32,
        inputs: &[SigElement],
        outputs: &[SigElement],
        killing: bool,
        make: impl FnOnce(
            &Container,
            &NativeProgram,
            &ir::Program,
            &structure::Structured,
            &io::Io,
            &resource::Resources,
        ) -> T,
    ) -> T {
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![],
                input_addr: 0,
                db_shader_control,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let isg1 = container::signature(inputs, true);
        let osg1 = container::signature(outputs, false);
        let mut parts = vec![(FourCC::OSG1, osg1.as_slice())];
        if !inputs.is_empty() {
            parts.insert(0, (FourCC::ISG1, isg1.as_slice()));
        }
        let bytes = crate::dxbc::build(&parts);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = if killing {
            structure::killing(&p).unwrap()
        } else {
            structure::structure(&p).unwrap()
        };
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        make(&c, &native, &p, &st, &io, &res)
    }

    /// Emits the pixel program `words` ([`pixel_program`]).
    fn emit_pixel_program(
        words: &[u32],
        db_shader_control: u32,
        inputs: &[SigElement],
        outputs: &[SigElement],
        killing: bool,
    ) -> Result<HostShader, ShaderError> {
        let out = pixel_program(words, db_shader_control, inputs, outputs, killing, emit)?;
        // dxil.dll, where there is one, checks the module.
        #[cfg(windows)]
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut out.dxil.clone()).unwrap();
        }
        Ok(out)
    }

    /// A pixel program that translates, as the tests of the output notes
    /// read it.
    struct Stored {
        /// The notes, as [`notes`] gives them.
        notes: Vec<String>,
        /// Per output store the module writes, the output's id and the
        /// channel, in the order of the function's blocks.
        stores: Vec<(u64, u64)>,
        /// What one lane stores when the function is run
        /// ([`bitcode::Flow::run`]; the programs' registers start at 0):
        /// the output's id, the channel, the value's bits.
        lane: Vec<(u64, u64, u64)>,
        /// Whether that lane is discarded.
        discarded: bool,
        /// The outputs the host shader declares, with each one's channels.
        declared: Vec<(String, Option<u32>, u8)>,
        dxil: Vec<u8>,
    }

    /// The pixel program `words` with no inputs, emitted
    /// ([`emit_pixel_program`]: the validator sees it where there is one),
    /// with the function of its lowering.
    fn stored_pixel_outputs(
        words: &[u32],
        db_shader_control: u32,
        outputs: &[SigElement],
        killing: bool,
    ) -> Stored {
        let shader = emit_pixel_program(words, db_shader_control, &[], outputs, killing).unwrap();
        let l = pixel_program(
            words,
            db_shader_control,
            &[],
            outputs,
            killing,
            |c, native, p, st, io, res| lowered(c, native, p, st, io, res, Options::default()),
        )
        .unwrap();
        assert_eq!(l.dxil, shader.dxil);
        assert_eq!(l.output_notes, shader.output_notes);
        l.flow.check().unwrap();
        let calls = l.flow.blocks.iter().flat_map(|b| &b.insts);
        let stores = calls
            .filter_map(|i| match &i.op {
                FlowOp::Call(name, args) if name.starts_with("dx.op.storeOutput") => {
                    // The opcode, the output's id, its row, the channel, the value.
                    let [_, Arg::Int(id), Arg::Int(0), Arg::Int(col), _] = args[..] else {
                        panic!("{name} {args:?}");
                    };
                    Some((id, col))
                }
                _ => None,
            })
            .collect();
        let mut lane = Vec::new();
        let mut discarded = false;
        let mut call = |name: &str, args: &[u64]| {
            match name {
                "dx.op.storeOutput.f32" | "dx.op.storeOutput.i32" => {
                    lane.push((args[1], args[3], args[4]));
                }
                "dx.op.discard" => discarded |= args[1] != 0,
                name => panic!("{name}"),
            }
            Vec::new()
        };
        l.flow.run(1000, &mut call).unwrap();
        let host = Container::parse(&shader.dxil).unwrap();
        let declared = signature(host.part(FourCC::OSG1).unwrap().data).unwrap();
        Stored {
            notes: notes(&shader.output_notes),
            stores,
            lane,
            discarded,
            declared: declared
                .into_iter()
                .map(|e| (e.name, e.row, e.cols))
                .collect(),
            dxil: shader.dxil,
        }
    }

    /// [`emit_pixel_program`] of a program with no inputs: the outputs the host
    /// shader declares, with its feature flags.
    fn declared_pixel_outputs(
        words: &[u32],
        db_shader_control: u32,
        outputs: &[SigElement],
        killing: bool,
    ) -> Result<(Declared, u64), ShaderError> {
        let out = emit_pixel_program(words, db_shader_control, &[], outputs, killing)?;
        let host = Container::parse(&out.dxil).unwrap();
        let declared = signature(host.part(FourCC::OSG1).unwrap().data)?
            .into_iter()
            .map(|e| (e.name, e.row))
            .collect();
        let sfi0 = host.part(FourCC::SFI0).unwrap().data;
        Ok((declared, u64::from_le_bytes(sfi0.try_into().unwrap())))
    }

    /// A stored translation keeps a note's reason as its place in
    /// [`OutputReason::ALL`] (the shader cache of the gpu crate), and logs
    /// and tallies tell the reasons apart by their text.
    #[test]
    fn output_reasons_have_their_places_and_their_own_texts() {
        for (place, reason) in OutputReason::ALL.into_iter().enumerate() {
            assert_eq!(reason.code(), place);
            let same = OutputReason::ALL.iter().filter(|r| r.text() == reason.text());
            assert_eq!(same.count(), 1, "{}", reason.text());
        }
        let note = OutputNote {
            reason: OutputReason::SomePaths,
            output: "SV_Depth".into(),
        };
        assert_eq!(note.to_string(), "SV_Depth: exported on some paths only");
    }

    /// Output notes as logs give them: the output, then the reason.
    fn notes(notes: &[OutputNote]) -> Vec<String> {
        notes.iter().map(|n| n.to_string()).collect()
    }

    fn refusal(e: ShaderError) -> &'static str {
        match e {
            ShaderError::Unsupported { what, .. } => what,
            e => panic!("{e}"),
        }
    }

    /// The container's signature declares outputs whether or not the code
    /// exports them. One no export writes is undefined on the host if the
    /// shader declares it (and Microsoft's validator refuses a depth or a
    /// stencil reference that is declared and never written), so it is
    /// not declared: the host keeps its own depth and stencil reference
    /// and leaves the render target alone, as the console does where
    /// `DB_SHADER_CONTROL` does not take them from the program. (Where it
    /// does: [`an_output_the_stage_data_enables_and_no_export_writes_is_not_declared`].)
    /// Each is noted.
    #[test]
    fn a_pixel_output_no_export_writes_is_not_declared() {
        let depth = || pixel_output("SV_Depth", 17, None);
        let stencil = || pixel_output("SV_StencilRef", 20, None);
        let target = |n| pixel_output("SV_Target", 16, Some(n));
        let named = |names: &[(&str, Option<u32>)]| -> Declared {
            names.iter().map(|&(n, r)| (n.to_owned(), r)).collect()
        };
        // The program ends before its export (the opcode census puts
        // `s_endpgm` there): no lane exports, every lane is discarded.
        let ends_early = [
            0x7E02_0285u32, // v_mov_b32 v1, 5
            0xBF81_0000,    // s_endpgm
            0xF800_1883,
            0x0000_0100, // exp mrtz v0, v1, off, off done vm
            0xBF81_0000,
        ];
        let (declared, features) =
            declared_pixel_outputs(&ends_early, 0, &[depth(), stencil()], false).unwrap();
        assert_eq!(declared, []);
        assert_eq!(features & 0x200, 0, "SFI0 {features:#x}: StencilRef");

        // Only render target 0 is exported: the others, depth and stencil
        // reference go, wherever they stand in the signature.
        let color_only = [
            0xF800_180Fu32,
            0x0000_0000, // exp mrt0 v0, v0, v0, v0 done vm
            0xBF81_0000,
        ];
        let rgba = |n| SigElement {
            cols: 4,
            ..target(n)
        };
        let (declared, features) = declared_pixel_outputs(
            &color_only,
            0,
            &[depth(), rgba(1), rgba(0), stencil()],
            false,
        )
        .unwrap();
        assert_eq!(declared, named(&[("SV_Target", Some(0))]));
        assert_eq!(features & 0x200, 0, "SFI0 {features:#x}: StencilRef");
        // The shader names the ones it leaves out, in the signature's
        // order, for the caller to log.
        let outputs = [depth(), rgba(1), rgba(0), stencil()];
        let shader = emit_pixel_program(&color_only, 0, &[], &outputs, false).unwrap();
        assert_eq!(
            notes(&shader.output_notes),
            [
                "SV_Target1: never exported",
                "SV_Depth: never exported",
                "SV_StencilRef: never exported"
            ]
        );
        // And it is, byte for byte, the shader of a container that never
        // listed them.
        let plain = emit_pixel_program(&color_only, 0, &[], &[rgba(0)], false).unwrap();
        assert_eq!(plain.output_notes, []);
        assert_eq!(shader.dxil, plain.dxil);

        // Depth exported, stencil reference not.
        let depth_only = [
            0xF800_1881u32,
            0x0000_0000, // exp mrtz v0, off, off, off done vm
            0xBF81_0000,
        ];
        let (declared, _) =
            declared_pixel_outputs(&depth_only, 1, &[depth(), stencil()], false).unwrap();
        assert_eq!(declared, named(&[("SV_Depth", None)]));

        // A coverage output likewise.
        let coverage = pixel_output("SV_Coverage", 14, None);
        let (declared, _) =
            declared_pixel_outputs(&color_only, 0, &[rgba(0), coverage], false).unwrap();
        assert_eq!(declared, named(&[("SV_Target", Some(0))]));

        // Where no lane survives the console takes no value from the
        // program whatever the stage data says.
        let (declared, _) =
            declared_pixel_outputs(&ends_early, 0x103, &[depth(), stencil()], false).unwrap();
        assert_eq!(declared, []);
    }

    /// `DB_SHADER_CONTROL` can say the console takes depth, stencil
    /// reference or coverage from the program (Z_EXPORT_ENABLE,
    /// STENCIL_TEST_VAL_EXPORT_ENABLE, MASK_EXPORT_ENABLE) while the
    /// program exports none and a lane survives. What the console then
    /// does is not established and there is no value to store, so the
    /// output is not declared and the host keeps its own. The program
    /// translates (a refusal would lose every draw of the shader), to the
    /// bytes of a container that never listed the output, and the output
    /// is noted with the others no export writes.
    #[test]
    fn an_output_the_stage_data_enables_and_no_export_writes_is_not_declared() {
        let stencil = || pixel_output("SV_StencilRef", 20, None);
        let coverage = || pixel_output("SV_Coverage", 14, None);
        let color_only = [MRT0_DONE[0], MRT0_DONE[1], S_ENDPGM];
        let enabled = [
            (1, depth_output()),
            (2, stencil()),
            (0x100, coverage()),
            (0x103, depth_output()),
        ];
        for (bits, output) in enabled {
            for killing in [false, true] {
                let outputs = [rgba_target(0), output.clone()];
                let t = stored_pixel_outputs(&color_only, bits, &outputs, killing);
                assert_eq!(t.notes, [format!("{}: never exported", output.name)]);
                assert_eq!(t.declared, [("SV_Target".to_owned(), Some(0), 4)]);
                // The lane stores its colour and survives.
                assert_eq!(t.stores, [(0, 0), (0, 1), (0, 2), (0, 3)]);
                assert_eq!(t.lane, [(0, 0, 0), (0, 1, 0), (0, 2, 0), (0, 3, 0)]);
                assert!(!t.discarded);
                // The same bytes with the bit clear, and with the output
                // never listed.
                let clear = stored_pixel_outputs(&color_only, 0, &outputs, killing);
                assert_eq!(clear.notes, t.notes);
                assert_eq!(clear.dxil, t.dxil);
                let plain = stored_pixel_outputs(&color_only, bits, &outputs[..1], killing);
                assert_eq!(plain.notes, [""; 0]);
                assert_eq!(plain.dxil, t.dxil);
            }
        }
        // An output with nothing but the enabled ones: the shader declares
        // none (the colour export has no target in this container).
        for (bits, output) in [(1, depth_output()), (2, stencil())] {
            let alone = std::slice::from_ref(&output);
            let t = stored_pixel_outputs(&color_only, bits, alone, false);
            assert_eq!(t.notes, [format!("{}: never exported", output.name)]);
            assert_eq!((t.declared.len(), t.stores.len()), (0, 0));
        }

        // Depth and stencil reference both enabled, the depth alone
        // exported (4.0): it is stored, the stencil reference is noted.
        let depth_only = [
            0x7E02_02F6u32, // v_mov_b32 v1, 4.0
            0xF800_1881,
            0x0000_0001, // exp mrtz v1, off, off, off done vm
            S_ENDPGM,
        ];
        for killing in [false, true] {
            let outputs = [depth_output(), stencil()];
            let t = stored_pixel_outputs(&depth_only, 3, &outputs, killing);
            assert_eq!(t.notes, ["SV_StencilRef: never exported"]);
            assert_eq!(t.declared, [("SV_Depth".to_owned(), None, 1)]);
            assert_eq!(t.stores, [(0, 0)]);
            assert_eq!(t.lane, [(0, 0, u64::from(4f32.to_bits()))]);
        }
        // With the coverage exported as well (`mrtz`'s third channel), all
        // that is enabled is written: no note.
        let both = [
            0xF800_1885u32,
            0x0000_0000, // exp mrtz v0, off, v0, off done vm
            S_ENDPGM,
        ];
        let t = stored_pixel_outputs(&both, 0x101, &[depth_output(), coverage()], false);
        assert_eq!(t.notes, [""; 0]);
        assert_eq!(t.stores, [(0, 0), (1, 0)]);
    }

    /// An output exported on some of the paths a surviving lane takes has
    /// no value on the others. (Microsoft's validator only asks for a
    /// store somewhere in the function.) What the console writes for a
    /// lane on such a path is not established, so the shader stores the
    /// output where the program exports it and makes up no value for the
    /// other lanes; the output is noted. With the export on every path
    /// there is no note.
    #[test]
    fn a_pixel_output_exported_on_some_paths_only_is_stored_where_it_is_exported() {
        let outputs = [rgba_target(0), depth_output()];
        const NOTE: &str = "SV_Depth: exported on some paths only";
        let v1 = u64::from(4f32.to_bits());
        // if (v0 == k) { depth = 4.0 }; color. The registers start at 0:
        // with k = 0 the lane the function is run as exports its depth,
        // with k = 1 it takes the other path.
        let some = |k: u32| {
            let mut words = vec![
                0x7E02_02F6u32,  // v_mov_b32 v1, 4.0
                0x7D84_0080 | k, // v_cmp_eq_u32 vcc, k, v0
                0xBE80_3C6A,     // s_and_saveexec_b32 s0, vcc_lo
                0xBF88_0002,     // s_cbranch_execz -> join
            ];
            words.extend(MRTZ_V1);
            words.push(0x887E_007E); // join: s_or_b32 exec_lo, exec_lo, s0
            words.extend(MRT0_DONE);
            words.push(S_ENDPGM);
            words
        };
        let color = [(0, 0, 0), (0, 1, 0), (0, 2, 0), (0, 3, 0)];
        for killing in [false, true] {
            for k in [0, 1] {
                let t = stored_pixel_outputs(&some(k), 1, &outputs, killing);
                assert_eq!(t.notes, [NOTE], "killing {killing}, k {k}");
                assert_eq!(t.declared.len(), 2, "{:?}", t.declared);
                // One store of the depth in the function, before the
                // colour's four.
                assert_eq!(t.stores, [(1, 0), (0, 0), (0, 1), (0, 2), (0, 3)]);
                // The lane that takes the export's path stores its depth;
                // the other stores its colour and no depth: none is made
                // up for it.
                let mut lane = color.to_vec();
                if k == 0 {
                    lane.insert(0, (1, 0, v1));
                }
                assert_eq!(t.lane, lane, "killing {killing}, k {k}");
                assert!(!t.discarded);
            }
        }

        // if (v0 == k) { depth; color; end } else { depth; color; end }
        // (the depth is v0 here).
        let every = |k: u32| {
            let mut words = vec![
                0x7D84_0080u32 | k, // v_cmp_eq_u32 vcc, k, v0
                0xBE80_3C6A,        // s_and_saveexec_b32 s0, vcc_lo
                0xBF88_0005,        // s_cbranch_execz -> else
            ];
            words.extend(MRTZ);
            words.extend(MRT0_DONE);
            words.push(S_ENDPGM);
            words.push(0x887E_007E); // else: s_or_b32 exec_lo, exec_lo, s0
            words.extend(MRTZ);
            words.extend(MRT0_DONE);
            words.push(S_ENDPGM);
            words
        };
        for k in [0, 1] {
            let t = stored_pixel_outputs(&every(k), 1, &outputs, false);
            assert_eq!(t.notes, [""; 0]);
            assert_eq!(t.declared.len(), 2, "{:?}", t.declared);
            assert_eq!(t.lane[0], (1, 0, 0));
            assert_eq!(t.lane[1..], color);
        }
    }

    /// A render target declared with four channels and exported with
    /// fewer: the channels the export has are stored and the others are
    /// not. What the console writes to them is not established, so no
    /// value is made up; Microsoft's validator accepts the shader (it does
    /// not ask for a render target's stores), and the output is noted.
    #[test]
    fn a_pixel_output_with_a_channel_no_export_writes_stores_the_exported_ones() {
        let v1 = u64::from(4f32.to_bits());
        // Channels 0 and 1, then 0 to 2, then 1 and 3.
        for (mask, channels) in [(0x3u32, &[0u64, 1][..]), (0x7, &[0, 1, 2]), (0xA, &[1, 3])] {
            let words = [
                0x7E02_02F6u32,     // v_mov_b32 v1, 4.0
                0xF800_1800 | mask, // exp mrt0 (v1 in the channels of `mask`) done vm
                0x0101_0101,
                S_ENDPGM,
            ];
            for killing in [false, true] {
                let t = stored_pixel_outputs(&words, 0, &[rgba_target(0)], killing);
                assert_eq!(t.notes, ["SV_Target0: channels never written"]);
                assert_eq!(t.declared, [("SV_Target".to_owned(), Some(0), 4)]);
                let stores: Vec<(u64, u64)> = channels.iter().map(|&c| (0, c)).collect();
                assert_eq!(t.stores, stores, "mask {mask:#x}");
                let lane: Vec<(u64, u64, u64)> = channels.iter().map(|&c| (0, c, v1)).collect();
                assert_eq!(t.lane, lane, "mask {mask:#x}");
                assert!(!t.discarded);
            }
        }
        // The same export to a target declared with the two channels it
        // has: nothing to note.
        let two = [
            0xF800_1803u32,
            0x0000_0000, // exp mrt0 v0, v0, off, off done vm
            S_ENDPGM,
        ];
        let narrow = SigElement {
            cols: 2,
            ..pixel_output("SV_Target", 16, Some(0))
        };
        let t = stored_pixel_outputs(&two, 0, &[narrow], false);
        assert_eq!(t.notes, [""; 0]);
        assert_eq!(t.stores, [(0, 0), (0, 1)]);

        // Two channels on one path and four on the other, the lanes of
        // both surviving: every channel has an export, and two of them on
        // one path only.
        let paths = |k: u32| {
            let mut words = vec![
                0x7D84_0080u32 | k, // v_cmp_eq_u32 vcc, k, v0
                0xBE80_3C6A,        // s_and_saveexec_b32 s0, vcc_lo
                0xBF88_0003,        // s_cbranch_execz -> else
            ];
            words.extend(two);
            words.push(0x887E_007E); // else: s_or_b32 exec_lo, exec_lo, s0
            words.extend(MRT0_DONE);
            words.push(S_ENDPGM);
            words
        };
        for (k, stored) in [(0, 2), (1, 4)] {
            let t = stored_pixel_outputs(&paths(k), 0, &[rgba_target(0)], false);
            assert_eq!(t.notes, ["SV_Target0: exported on some paths only"]);
            assert_eq!(t.stores.len(), 6, "{:?}", t.stores);
            assert_eq!(t.lane.len(), stored, "k {k}: {:?}", t.lane);
        }
    }

    /// Four channels exported to a render target declared with three or
    /// fewer. DXIL has no store of a channel past its element's, and the
    /// host shader's output is that element: the declared channels are
    /// stored, the others dropped, and the output is noted. (Before
    /// 2026-11-27 the store was emitted as it stood. Microsoft's validator
    /// rejected that shader for a target of two channels and took it for
    /// one of three, where the store is exactly one past: its test is
    /// `Col > Cols`.)
    #[test]
    fn a_pixel_export_past_its_outputs_element_stores_the_declared_channels() {
        let narrow = |cols| {
            [SigElement {
                cols,
                ..pixel_output("SV_Target", 16, Some(0))
            }]
        };
        let v1 = u64::from(4f32.to_bits());
        let four = [
            0x7E02_02F6u32, // v_mov_b32 v1, 4.0
            0xF800_180F,
            0x0101_0101, // exp mrt0 v1, v1, v1, v1 done vm
            S_ENDPGM,
        ];
        const NOTE: &str = "SV_Target0: channels exported past the element";
        for cols in 1..=4u8 {
            for killing in [false, true] {
                let t = stored_pixel_outputs(&four, 0, &narrow(cols), killing);
                assert_eq!(
                    t.notes,
                    [NOTE; 1][usize::from(cols == 4)..],
                    "{cols} channels"
                );
                assert_eq!(t.declared, [("SV_Target".to_owned(), Some(0), cols)]);
                let stores: Vec<(u64, u64)> = (0..u64::from(cols)).map(|c| (0, c)).collect();
                assert_eq!(t.stores, stores, "{cols} channels");
                let lane: Vec<(u64, u64, u64)> = stores.iter().map(|&(o, c)| (o, c, v1)).collect();
                assert_eq!(t.lane, lane, "{cols} channels");
                assert!(!t.discarded);
            }
        }
        // A second target beside it, exported within its channels: the
        // note names the first alone, and each keeps its own stores.
        let mut two_targets = vec![
            0xF800_000Fu32,
            0x0000_0000, // exp mrt0 v0, v0, v0, v0
            0xF800_181F,
            0x0000_0000, // exp mrt1 v0, v0, v0, v0 done vm
        ];
        two_targets.push(S_ENDPGM);
        let outputs = [narrow(3)[0].clone(), rgba_target(1)];
        let t = stored_pixel_outputs(&two_targets, 0, &outputs, false);
        assert_eq!(t.notes, [NOTE]);
        let stores = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (1, 3)];
        assert_eq!(t.stores, stores);

        // An export of the fourth channel alone to a target of three
        // writes no channel the target declares: the target is not
        // declared (the host leaves it as it is), under both reasons.
        let fourth = [
            0xF800_1808u32,
            0x0000_0000, // exp mrt0 off, off, off, v0 done vm
            S_ENDPGM,
        ];
        let t = stored_pixel_outputs(&fourth, 0, &narrow(3), false);
        assert_eq!(t.notes, ["SV_Target0: never exported", NOTE]);
        assert_eq!((t.declared.len(), t.stores.len()), (0, 0));
        assert!(!t.discarded);
    }

    /// A discard's exit (no lane of the wave is left: a `done` export to
    /// `null` with no lane in EXEC, then the end) writes no output and
    /// needs none: every lane that takes it is discarded. The program
    /// translates with its outputs declared and nothing to note, with the
    /// discarded lanes kept as helpers and with them taken out. An exit
    /// that leaves the killed lanes in EXEC is, with them taken out, an
    /// export of those lanes that writes nothing: the outputs are noted.
    #[test]
    fn a_discard_exit_needs_no_pixel_outputs() {
        // `cmpx` kills the lanes it holds in. The function is run as a
        // lane whose registers are 0: `lt` leaves it alive, `eq` kills it.
        // `kill` is what the exit's block does before its export.
        const LT_F32: u32 = 0x7C22_0080; // v_cmpx_lt_f32 exec, 0, v0
        const LT: u32 = 0x7DA2_0080; // v_cmpx_lt_u32 exec, 0, v0
        const EQ: u32 = 0x7DA4_0080; // v_cmpx_eq_u32 exec, 0, v0
        let program = |cmpx: u32, kill: &[u32]| {
            let mut words = vec![
                0xBE88_037Eu32, // s_mov_b32 s8, exec_lo
                cmpx,           // (the lanes killed)
                0x8A08_7E08,    // s_andn2_b32 s8, s8, exec_lo
                0xBF84_0005,    // s_cbranch_scc0 -> kill
                0xBEFE_0308,    // s_mov_b32 exec_lo, s8
                0x7E02_0280,    // v_mov_b32 v1, 0
                0xF800_180F,
                0x0101_0101, // exp mrt0, v1, v1, v1, v1 done vm
                S_ENDPGM,
            ];
            words.extend(kill); // kill:
            words.extend([
                0xF800_1890,
                0x0000_0000, // exp null done vm
                S_ENDPGM,
            ]);
            words
        };
        let outputs = [rgba_target(0)];
        // The exit's block takes its lanes out of EXEC from the live mask,
        // which is empty there.
        let words = program(LT_F32, &[0xBEFE_0308]); // s_mov_b32 exec_lo, s8
        for killing in [false, true] {
            let shader = emit_pixel_program(&words, 0, &[], &outputs, killing).unwrap();
            assert_eq!(notes(&shader.output_notes), [""; 0], "killing {killing}");
            let (declared, _) = declared_pixel_outputs(&words, 0, &outputs, killing).unwrap();
            assert_eq!(declared.len(), 1, "killing {killing}: {declared:?}");
        }
        // The same with a compare the function can be run with, and with
        // EXEC made 0 at the exit.
        let color = [(0, 0, 0), (0, 1, 0), (0, 2, 0), (0, 3, 0)];
        for kill in [0xBEFE_0308u32, 0xBEFE_0380] {
            for killing in [false, true] {
                let t = stored_pixel_outputs(&program(LT, &[kill]), 0, &outputs, killing);
                assert_eq!(t.declared.len(), 1, "killing {killing}: {:?}", t.declared);
                assert_eq!(t.notes, [""; 0], "killing {killing}");
                // The lane that is not killed stores its colour.
                assert_eq!(t.lane, color);
                assert!(!t.discarded);
                // The killed lane is discarded: where it is killed, as a
                // helper that runs on to the export (the host drops what
                // it stores); or, taken out, at the exit.
                let t = stored_pixel_outputs(&program(EQ, &[kill]), 0, &outputs, killing);
                assert_eq!(t.notes, [""; 0], "killing {killing}");
                assert!(t.discarded);
                assert_eq!(t.lane, if killing { &[][..] } else { &color[..] });
            }
        }

        // The same exit without that instruction: the killed lanes reach
        // the `done` export in EXEC. With them kept as helpers the discard
        // is where they are killed, as above. With them taken out, the
        // lane that takes the exit is by the mask exported and not
        // discarded: the shader returns for it with no discard and no
        // store (as before 2026-11-27), and the target, which the other
        // path writes, is noted.
        for cmpx in [LT, EQ] {
            let t = stored_pixel_outputs(&program(cmpx, &[]), 0, &outputs, false);
            assert_eq!(t.notes, [""; 0]);
            assert_eq!((t.lane.len(), t.discarded), (4, cmpx == EQ));
        }
        let t = stored_pixel_outputs(&program(EQ, &[]), 0, &outputs, true);
        assert_eq!(t.notes, ["SV_Target0: exported on some paths only"]);
        assert_eq!((t.lane.len(), t.discarded), (0, false));
        let t = stored_pixel_outputs(&program(LT, &[]), 0, &outputs, true);
        assert_eq!(t.notes, ["SV_Target0: exported on some paths only"]);
        assert_eq!((&t.lane[..], t.discarded), (&color[..], false));
    }

    const S_ENDPGM: u32 = 0xBF81_0000;
    /// `exp mrt0 v0, v0, v0, v0 done vm`.
    const MRT0_DONE: [u32; 2] = [0xF800_180F, 0x0000_0000];
    /// `exp mrtz v0, off, off, off` (depth).
    const MRTZ: [u32; 2] = [0xF800_0081, 0x0000_0000];
    /// The same, `done vm`.
    const MRTZ_DONE: [u32; 2] = [0xF800_1881, 0x0000_0000];
    /// `exp mrtz v1, off, off, off`.
    const MRTZ_V1: [u32; 2] = [0xF800_0081, 0x0000_0001];

    fn rgba_target(n: u32) -> SigElement {
        SigElement {
            cols: 4,
            ..pixel_output("SV_Target", 16, Some(n))
        }
    }

    fn depth_output() -> SigElement {
        pixel_output("SV_Depth", 17, None)
    }

    /// An export in a block no lane enters writes nothing, and the module
    /// leaves out the blocks it can tell no lane enters (a branch on a
    /// constant), their stores with them. An output with no other export
    /// would be declared and have no store in the function, which
    /// Microsoft's validator refuses for a depth, a stencil reference and a
    /// coverage: it is not declared.
    #[test]
    fn a_pixel_output_exported_only_where_no_lane_enters_is_not_declared() {
        // A branch on "no lane in EXEC" at the entry, where every lane is:
        // no lane takes it, and the path the lanes take ends without an
        // export (every lane is discarded).
        let depth_there = [
            0xBF88_0001u32, // s_cbranch_execz -> there
            S_ENDPGM,
            MRTZ_DONE[0], // there: exp mrtz v0 done vm
            MRTZ_DONE[1],
            S_ENDPGM,
        ];
        let both_there = [
            0xBF88_0001u32, // s_cbranch_execz -> there
            S_ENDPGM,
            0xF800_1883, // there: exp mrtz v0, v0, off, off done vm
            0x0000_0000,
            S_ENDPGM,
        ];
        let stencil = pixel_output("SV_StencilRef", 20, None);
        for killing in [false, true] {
            let (declared, _) =
                declared_pixel_outputs(&depth_there, 0, &[depth_output()], killing).unwrap();
            assert_eq!(declared, [], "killing {killing}");
            let (declared, features) =
                declared_pixel_outputs(&both_there, 0, &[depth_output(), stencil.clone()], killing)
                    .unwrap();
            assert_eq!(declared, [], "killing {killing}");
            assert_eq!(features & 0x200, 0, "SFI0 {features:#x}: StencilRef");
        }
        // The same with a colour export on the path the lanes take: the
        // depth export is still one no lane runs.
        let color_here = [
            0xBF88_0003u32, // s_cbranch_execz -> there
            MRT0_DONE[0],
            MRT0_DONE[1],
            S_ENDPGM,
            MRTZ[0], // there: exp mrtz v0
            MRTZ[1],
            MRT0_DONE[0],
            MRT0_DONE[1],
            S_ENDPGM,
        ];
        for killing in [false, true] {
            let (declared, _) =
                declared_pixel_outputs(&color_here, 0, &[rgba_target(0), depth_output()], killing)
                    .unwrap();
            assert_eq!(declared, [("SV_Target".to_owned(), Some(0))]);
        }

        // A block that conditions structurisation holds possible lead to
        // and that the module leaves out all the same: EXEC is "some lane
        // of the quad is in an empty mask", which the lowering takes as
        // true, so its branch on "no lane in EXEC" is one on a constant.
        // The depth is declared at first, the function is then found
        // without a store of it, and the program is lowered again without
        // it ([`Lower::stored_outputs`]).
        let quad = [
            0xBE89_0380u32, // s_mov_b32 s9, 0
            0xBEFE_0909,    // s_wqm_b32 exec_lo, s9
            0xBF88_0001,    // s_cbranch_execz -> there
            S_ENDPGM,
            MRTZ_DONE[0], // there: exp mrtz v0 done vm
            MRTZ_DONE[1],
            S_ENDPGM,
        ];
        for killing in [false, true] {
            let shader = emit_pixel_program(&quad, 0, &[], &[depth_output()], killing).unwrap();
            assert_eq!(notes(&shader.output_notes), ["SV_Depth: never exported"]);
            let host = Container::parse(&shader.dxil).unwrap();
            let declared = signature(host.part(FourCC::OSG1).unwrap().data).unwrap();
            assert!(declared.is_empty(), "killing {killing}: {declared:?}");
        }
    }

    /// A block no lane enters has nothing to note: an end there is one no
    /// lane leaves through, whatever the paths to it have written, and an
    /// export there runs in no lane, whatever its channels.
    #[test]
    fn a_block_no_lane_enters_gives_no_pixel_output_note() {
        let narrow = SigElement {
            cols: 3,
            ..pixel_output("SV_Target", 16, Some(0))
        };
        // color (done); if (no lane in EXEC) { four channels of colour;
        // end }; depth; end. At the entry every lane is in EXEC, so no
        // lane takes the branch. Its block ends behind the `done` export
        // and without the depth, and exports a channel the target does
        // not declare.
        let words = [
            0xF800_1807u32,
            0x0000_0000, // exp mrt0 v0, v0, v0, off done vm
            0xBF88_0003, // s_cbranch_execz -> there
            MRTZ[0],     // exp mrtz v0
            MRTZ[1],
            S_ENDPGM,
            0xF800_000F, // there: exp mrt0 v0, v0, v0, v0
            0x0000_0000,
            S_ENDPGM,
        ];
        for killing in [false, true] {
            let t = stored_pixel_outputs(&words, 1, &[narrow.clone(), depth_output()], killing);
            assert_eq!(t.notes, [""; 0], "killing {killing}");
            assert_eq!(t.declared.len(), 2, "{:?}", t.declared);
            // The module leaves that block out, its stores with it.
            assert_eq!(t.stores, [(0, 0), (0, 1), (0, 2), (1, 0)]);
            assert_eq!(t.lane, [(0, 0, 0), (0, 1, 0), (0, 2, 0), (1, 0, 0)]);
            assert!(!t.discarded);
        }
    }

    /// The instructions inside a whole-wave section (EXEC made every lane,
    /// then restored) are not lowered one by one, so an export there is
    /// passed over. One that is the `done` export or writes a channel of a
    /// declared output would be lost without a word, and its output taken
    /// for one no export writes: refused. One the lowering does nothing
    /// for outside a section either (its target is `null` or not one the
    /// container lists) loses nothing: the program translates as it does
    /// without that export.
    #[test]
    fn a_pixel_export_a_whole_wave_section_would_lose_is_refused() {
        const WHY: &str = "a pixel export inside a whole-wave section";
        // The section with `inside` in it, then `after`.
        let section = |inside: &[u32], after: &[u32]| {
            let mut words = vec![0xBE89_3DC1u32]; // s_or_saveexec_b32 s9, -1
            words.extend(inside);
            words.push(0xBEFE_0309); // s_mov_b32 exec_lo, s9
            words.extend(after);
            words.push(S_ENDPGM);
            words
        };
        let words = section(&MRTZ, &MRT0_DONE);
        let outputs = [rgba_target(0), depth_output()];
        for killing in [false, true] {
            let e = declared_pixel_outputs(&words, 0, &outputs, killing).unwrap_err();
            assert_eq!(refusal(e), WHY);
        }
        // The same section without the export translates.
        let plain = section(&[], &MRT0_DONE);
        let (declared, _) = declared_pixel_outputs(&plain, 0, &outputs[..1], false).unwrap();
        assert_eq!(declared.len(), 1, "{declared:?}");

        // The `done` export in a section is refused whatever it writes:
        // where a lane ends exported goes by it ([`Lower::exit`]).
        const NULL_DONE: [u32; 2] = [0xF800_1890, 0x0000_0000]; // exp null done vm
        for done in [MRT0_DONE, NULL_DONE] {
            for killing in [false, true] {
                let words = section(&done, &[]);
                let e = declared_pixel_outputs(&words, 0, &outputs[..1], killing).unwrap_err();
                assert_eq!(refusal(e), WHY);
            }
        }

        // The depth export in a container that lists no depth, and an
        // export to `null`: outside a section the lowering has no store
        // for either, so nothing is lost inside one. The shader is, byte
        // for byte, the one of the section without the export.
        const NULL: [u32; 2] = [0xF800_0090, 0x0000_0000]; // exp null off, off, off, off
        for killing in [false, true] {
            let plain = stored_pixel_outputs(&plain, 0, &outputs[..1], killing);
            assert_eq!(plain.notes, [""; 0]);
            assert_eq!(plain.stores, [(0, 0), (0, 1), (0, 2), (0, 3)]);
            for inside in [MRTZ, NULL] {
                let words = section(&inside, &MRT0_DONE);
                let t = stored_pixel_outputs(&words, 0, &outputs[..1], killing);
                assert_eq!(t.notes, [""; 0], "{inside:x?}");
                assert_eq!(t.dxil, plain.dxil, "{inside:x?}");
                assert!(!t.discarded);
            }
        }

        // The fourth channel alone to a target declared with three: a
        // channel past the element, which the lowering stores nowhere
        // ([`a_pixel_export_past_its_outputs_element_stores_the_declared_channels`]).
        // Passed over, and noted as outside a section.
        let narrow = [SigElement {
            cols: 3,
            ..pixel_output("SV_Target", 16, Some(0))
        }];
        let fourth = [0xF800_0008u32, 0x0000_0000]; // exp mrt0 off, off, off, v0
        let three = [0xF800_1807u32, 0x0000_0000]; // exp mrt0 v0, v0, v0, off done vm
        for killing in [false, true] {
            let t = stored_pixel_outputs(&section(&fourth, &three), 0, &narrow, killing);
            assert_eq!(t.notes, ["SV_Target0: channels exported past the element"]);
            assert_eq!(t.stores, [(0, 0), (0, 1), (0, 2)]);
            // A channel the target declares, in the section: refused.
            let first = [0xF800_0001u32, 0x0000_0000]; // exp mrt0 v0, off, off, off
            let e = declared_pixel_outputs(&section(&first, &three), 0, &narrow, killing);
            assert_eq!(refusal(e.unwrap_err()), WHY);
        }
    }

    /// Which ends a lane leaves through without a discard decides where an
    /// output needs its value: a lane is behind its `done` export from that
    /// export on, in whatever block the program then ends.
    #[test]
    fn pixel_outputs_are_needed_where_a_lane_ends_behind_its_done_export() {
        let outputs = [rgba_target(0), depth_output()];
        // color (done); if (v0 == 0) { depth }; end. The end is in the join
        // block, behind the `done` export on both paths, and the depth is
        // behind one of them only.
        let mut after = MRT0_DONE.to_vec();
        after.extend([
            0x7D84_0080u32, // v_cmp_eq_u32 vcc, 0, v0
            0xBE89_3C6A,    // s_and_saveexec_b32 s9, vcc_lo
            0xBF88_0002,    // s_cbranch_execz -> join
        ]);
        after.extend(MRTZ);
        after.push(0x887E_097E); // join: s_or_b32 exec_lo, exec_lo, s9
        after.push(S_ENDPGM);
        // The depth is noted, and stored where the program exports it:
        // once, behind the colour's stores.
        for killing in [false, true] {
            let t = stored_pixel_outputs(&after, 1, &outputs, killing);
            assert_eq!(t.notes, ["SV_Depth: exported on some paths only"]);
            assert_eq!(t.stores, [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0)]);
            assert!(!t.discarded);
        }

        // if (v0 == 0) { color (done) }; end. The end is joined by a path
        // with the `done` export and one without: every lane that leaves
        // through it is discarded ([`Lower::exit`]), so no output is needed
        // there, and the target stays declared (a store of it is in the
        // function).
        let mut joined = vec![
            0x7D84_0080u32, // v_cmp_eq_u32 vcc, 0, v0
            0xBE89_3C6A,    // s_and_saveexec_b32 s9, vcc_lo
            0xBF88_0002,    // s_cbranch_execz -> join
        ];
        joined.extend(MRT0_DONE);
        joined.push(0x887E_097E); // join: s_or_b32 exec_lo, exec_lo, s9
        joined.push(S_ENDPGM);
        // The same with the end of the path without an export in a block
        // of its own: if (v0 == 0) { color (done); end }; end. The lanes of
        // the first end survive with their target written; those of the
        // second are discarded and need none.
        let mut apart = vec![
            0x7D84_0080u32, // v_cmp_eq_u32 vcc, 0, v0
            0xBE89_3C6A,    // s_and_saveexec_b32 s9, vcc_lo
            0xBF88_0003,    // s_cbranch_execz -> else
        ];
        apart.extend(MRT0_DONE);
        apart.push(S_ENDPGM);
        apart.push(0x887E_097E); // else: s_or_b32 exec_lo, exec_lo, s9
        apart.push(S_ENDPGM);
        for program in [&joined, &apart] {
            for killing in [false, true] {
                let t = stored_pixel_outputs(program, 0, &outputs[..1], killing);
                assert_eq!(t.declared, [("SV_Target".to_owned(), Some(0), 4)]);
                // Nothing to note: no surviving lane is without it.
                assert_eq!(t.notes, [""; 0]);
            }
        }
    }

    /// The interpolation mode a host container gives SV_Position: its
    /// record among PSV0's signature elements (one row, row 0, four
    /// channels from the first, kind 3, type, mode).
    fn position_mode(dxil: &[u8]) -> u8 {
        let host = Container::parse(dxil).unwrap();
        let psv = host.part(FourCC::PSV0).unwrap().data;
        let at = psv
            .windows(4)
            .position(|w| w == [1, 0, 0x44, 3])
            .expect("SV_Position");
        psv[at + 5]
    }

    /// With a conservative depth output DXIL wants the position centroid.
    /// That follows the outputs the shader declares: a conservative depth
    /// the container lists and no export writes is not declared, and the
    /// position is then the pixel centre's, as in a container that never
    /// listed it.
    #[test]
    fn the_position_is_centroid_only_with_a_declared_conservative_depth() {
        let position = SigElement {
            cols: 4,
            ..pixel_output("SV_Position", 3, Some(0))
        };
        let at_least = pixel_output("SV_DepthGreaterEqual", 19, None);
        let mut color = MRT0_DONE.to_vec();
        color.push(S_ENDPGM);
        let with = emit_pixel_program(
            &color,
            0,
            std::slice::from_ref(&position),
            &[rgba_target(0), at_least.clone()],
            false,
        )
        .unwrap();
        let without = emit_pixel_program(
            &color,
            0,
            std::slice::from_ref(&position),
            &[rgba_target(0)],
            false,
        )
        .unwrap();
        assert_eq!(position_mode(&without.dxil), 4);
        assert_eq!(position_mode(&with.dxil), 4);
        assert_eq!(with.dxil, without.dxil);

        // Exported, it is declared, and the position is centroid.
        let mut both = MRTZ.to_vec();
        both.extend(color);
        let exported =
            emit_pixel_program(&both, 1, &[position], &[rgba_target(0), at_least], false).unwrap();
        assert_eq!(position_mode(&exported.dxil), 5);
    }

    /// `s_barrier` waits for the waves of a thread group. Pixel and vertex
    /// waves belong to none, and DXIL has a group sync only in compute,
    /// mesh and amplification shaders (Microsoft's validator refuses it
    /// elsewhere): such a program is refused.
    #[test]
    fn a_barrier_outside_compute_and_mesh_programs_is_refused() {
        const WHY: &str = "a thread-group barrier outside compute and mesh programs";
        let pixel = [
            0xBF8A_0000u32, // s_barrier
            0xF800_180F,
            0x0000_0000, // exp mrt0 v0, v0, v0, v0 done vm
            0xBF81_0000,
        ];
        let target = SigElement {
            cols: 4,
            ..pixel_output("SV_Target", 16, Some(0))
        };
        let e = declared_pixel_outputs(&pixel, 0, &[target], false).unwrap_err();
        assert_eq!(refusal(e), WHY);

        let vertex = [
            0xBF8A_0000u32, // s_barrier
            0xF800_08CF,
            0x0000_0000, // exp pos0 v0, v0, v0, v0 done
            0xBF81_0000,
        ];
        let native = NativeProgram {
            stage: HwStage::Vs,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: StageIo::Vs {
                inputs: vec![],
                outputs: vec![],
            },
            code: vertex.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let position = SigElement {
            cols: 4,
            ..pixel_output("SV_Position", 3, Some(0))
        };
        let osg1 = container::signature(&[position], false);
        let bytes = crate::dxbc::build(&[(FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let translate = |native: &NativeProgram| {
            let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
            let st = structure::structure(&p).unwrap();
            let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
            let io = io::raise(&p, native).unwrap();
            emit(&c, native, &p, &st, &io, &res)
        };
        assert_eq!(refusal(translate(&native).unwrap_err()), WHY);
        // The same program without the barrier translates.
        let plain = NativeProgram {
            code: native.code[4..].to_vec(),
            ..native
        };
        translate(&plain).unwrap();
    }

    #[test]
    fn immediate_sampler_slots_distinguish_comparison_uses() {
        let words = [
            0xBE81_0380u32, // s_mov_b32 s1, 0
            0xF40C_0000,
            0xFA00_0000, // s_load_dwordx8 s[0:7], s[0:1], null
            0xBE88_0380, // s_mov_b32 s8, 0
            0xBE89_0380, // s_mov_b32 s9, 0
            0xBE8A_0380, // s_mov_b32 s10, 0
            0xBE8B_0380, // s_mov_b32 s11, 0
            0x7E00_02F0, // v_mov_b32 v0, 0.5 (comparison reference)
            0x7E02_02F0, // v_mov_b32 v1, 0.5 (u)
            0x7E04_02F0, // v_mov_b32 v2, 0.5 (v)
            0xBF8C_0070, // s_waitcnt vmcnt(0) lgkmcnt(0)
            0xF080_0108,
            0x0040_0301, // image_sample v3, v[1:2], s[0:7], s[8:11]
            0xF0A0_0108,
            0x0040_0400, // image_sample_c v4, v[0:2], s[0:7], s[8:11]
            0xF0BC_0108,
            0x0040_0500, // image_sample_c_lz v5, v[0:2], s[0:7], s[8:11]
            0xF080_0108,
            0x0040_0601, // image_sample v6, v[1:2], s[0:7], s[8:11]
            0xBF8C_3F70, // s_waitcnt vmcnt(0)
            0xF800_180F,
            0x0605_0403, // exp mrt0, v3, v4, v5, v6 done vm
            0xBF81_0000,
        ];
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 7,
            layout: vec![crate::native::RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 0,
                slot: 0,
            }],
            used: vec![crate::native::UsedResource {
                kind: crate::native::ResourceType::Srv,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 0,
                dwords: 8,
                user_data: 0,
                slot: 0,
            }],
            io: StageIo::Ps {
                inputs: vec![],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let output = SigElement {
            name: "SV_Target".into(),
            index: 0,
            kind: 16,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 15,
            half: false,
        };
        let osg1 = container::signature(&[output], false);
        let bytes = crate::dxbc::build(&[(FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let out = emit(&c, &native, &p, &st, &io, &res).unwrap();

        let sampler = StaticSampler::from_words([0; 4]);
        let comparison = StaticSampler {
            filter: sampler.filter | 0x80,
            ..sampler
        };
        assert_eq!(out.samplers, [[0; 4]; 2]);
        assert_eq!(out.static_samplers, [sampler, comparison]);
        #[cfg(windows)]
        if let Some(path) = super::validator::Validator::default_path() {
            let mut v = super::validator::Validator::load(Some(&path)).unwrap();
            v.validate(&mut out.dxil.clone()).unwrap();
        }
    }

    /// A wave64 compute program that writes only EXEC's high dword
    /// (`s_mov_b32 exec_hi, 0`) before a vector instruction. The new mask is
    /// opaque and keyed by the unchanged low dword, so building its lane bit
    /// needed the mask's own value: lowering recursed until the stack
    /// overflowed, which takes the whole process down (found by
    /// random-input testing). Now the instruction fails like any other that
    /// cannot be expressed: its result is missing, and only a side effect
    /// that uses it would stop the translation.
    #[test]
    fn an_exec_high_write_does_not_overflow_the_stack() {
        // s_mov_b32 exec_hi, 0; v_mov_b32 v1, v0; s_endpgm
        let words = [0xBEFF_0380u32, 0x7E02_0300, 0xBF81_0000];
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64: true,
            vgprs: 2,
            layout: vec![],
            used: vec![],
            io: StageIo::Cs(ComputeIo {
                rsrc2: 0,
                threads: [64, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let container = crate::dxbc::build(&[]);
        let c = Container::parse(&container).unwrap();
        if let Err(e) = emit(&c, &native, &p, &st, &io, &res) {
            assert!(e.to_string().contains("its own value"), "{e}");
        }
    }

    /// A pixel shader writing a constant colour.
    #[cfg(windows)]
    fn constant_color() -> Vec<u8> {
        let mut s = Shader::new(true);
        s.outputs.push(SigElement {
            name: "SV_Target".into(),
            index: 0,
            kind: 16,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 15,
            half: false,
        });
        let b = s.entry;
        for (c, v) in [1.0f32, 0.5, 0.25, 1.0].into_iter().enumerate() {
            let args = [
                s.m.i32(0),
                s.m.i32(0),
                s.m.i8(c as u8),
                s.m.f32(v.to_bits()),
            ];
            s.op(b, "storeOutput.f32", Attr::NoUnwind, s.t.void, 5, &args);
        }
        s.m.terminate(b, super::bitcode::Term::Ret);
        s.finish()
    }

    #[test]
    #[cfg(windows)]
    fn stub_pipeline_is_created() {
        let host = match super::host::Host::new(false) {
            Ok(host) => host,
            Err(e) => {
                eprintln!("no SM 6.6 device ({e}); skipped");
                return;
            }
        };
        let ps = super::HostShader {
            stage: crate::HwStage::Ps,
            dxil: constant_color(),
            output_notes: Vec::new(),
            user_data: Vec::new(),
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: Vec::new(),
            root_srvs: Vec::new(),
            root_uavs: Vec::new(),
            memory: false,
            memory_writes: false,
            view_addresses: false,
            heap: false,
            heap_writes: false,
            view_marks: false,
            wave: Default::default(),
        };
        let file = |var: &str, stage| {
            std::env::var(var).ok().map(|p| super::HostShader {
                stage,
                dxil: std::fs::read(p).unwrap(),
                output_notes: Vec::new(),
                user_data: Vec::new(),
                samplers: Vec::new(),
                static_samplers: Vec::new(),
                root_cbvs: Vec::new(),
                root_srvs: Vec::new(),
                root_uavs: Vec::new(),
                memory: false,
                memory_writes: false,
                view_addresses: false,
                heap: false,
                heap_writes: false,
                view_marks: false,
                wave: Default::default(),
            })
        };
        let ps = file("VICTORY_REF_PS", crate::HwStage::Ps).unwrap_or(ps);
        let vs = file("VICTORY_REF_VS", crate::HwStage::Vs)
            .unwrap_or_else(|| super::host::stub_vs(&ps).unwrap());
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut vs.dxil.clone()).unwrap();
        }
        host.pipeline(&vs, &ps).unwrap();
        // The runtime checks the container hash: no hash or a wrong one is
        // refused. (Recent runtimes also take sixteen 0x01 bytes, a bypass
        // value; ours works with older ones too.)
        let mut wrong = ps.dxil[4..20].to_vec();
        wrong[0] ^= 1;
        for digest in [&[0u8; 16][..], &wrong] {
            let mut other = ps.clone();
            other.dxil[4..20].copy_from_slice(digest);
            assert!(
                host.pipeline(&vs, &other).is_err(),
                "digest {digest:02x?} accepted"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn constant_color_validates() {
        let mut c = constant_color();
        let path = super::validator::Validator::default_path();
        let Ok(mut v) = super::validator::Validator::load(path.as_deref()) else {
            eprintln!("no dxil.dll; skipped");
            return;
        };
        let ours = c[4..20].to_vec();
        v.validate(&mut c).unwrap();
        assert_eq!(c[4..20], ours[..], "our hash differs from the validator's");
    }

    /// Triangle (0, 1, 2), each corner's 9-bit index in its 10-bit field.
    const TRIANGLE: u32 = 1 << 10 | 2 << 20;
    /// The edge flags of the three corners (bits 9, 19 and 29).
    const EDGE_FLAGS: u32 = 1 << 9 | 1 << 19 | 1 << 29;
    /// The culling bit.
    const CULL: u32 = 1 << 31;

    /// A one-wave (32-lane) triangle mesh program whose lanes all export
    /// primitive `prim` and, as vertex `lane`, the position
    /// (lane & 1 ? 3 : -1, lane & 2 ? 3 : -1, 0.5, 1): triangle (0, 1, 2)
    /// covers the whole target.
    fn triangle_mesh(prim: u32) -> Result<HostShader, ShaderError> {
        let words = [
            0xD765_0001u32, // v_mbcnt_lo_u32_b32 v1, -1, 0
            0x0001_00C1,
            0x3604_0281, // v_and_b32 v2, 1, v1
            0x3606_0282, // v_and_b32 v3, 2, v1
            0x7E04_0D02, // v_cvt_f32_u32 v2, v2
            0x7E06_0D03, // v_cvt_f32_u32 v3, v3
            0x1008_04F6, // v_mul_f32 v4, 4.0, v2
            0x0608_08F3, // v_add_f32 v4, -1.0, v4
            0x100A_06F4, // v_mul_f32 v5, 2.0, v3
            0x060A_0AF3, // v_add_f32 v5, -1.0, v5
            0x7E0C_02F0, // v_mov_b32 v6, 0.5
            0x7E0E_02F2, // v_mov_b32 v7, 1.0
            0x7E12_02FF, // v_mov_b32 v9, prim
            prim,
            0xF800_0941, // exp prim v9, off, off, off done
            0x0000_0009,
            0xF800_08CF, // exp pos0 v4, v5, v6, v7 done
            0x0706_0504,
            0xBF81_0000, // s_endpgm
        ];
        let native = NativeProgram {
            stage: HwStage::Ms,
            wave64: false,
            vgprs: 10,
            layout: vec![],
            used: vec![],
            io: StageIo::Ms(crate::native::MeshIo {
                outputs: vec![],
                clip_cntl: 0,
                threads: 32,
                max_vertices: 32,
                max_primitives: 32,
                topology: 2,
                lds_bytes: 0,
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let position = SigElement {
            name: "SV_Position".into(),
            index: 0,
            kind: 3,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 0xF,
            half: false,
        };
        let osg1 = container::signature(&[position], false);
        let bytes = crate::dxbc::build(&[(FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes)?;
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
        let st = structure::structure(&p)?;
        let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
        let io = io::raise(&p, &native)?;
        emit(&c, &native, &p, &st, &io, &res)
    }

    /// A primitive export's corners are 9-bit indices: the edge flag above
    /// each (bits 9, 19, 29) is no part of it. Masking ten bits made a
    /// flagged corner index 512 or more, past any mesh program's vertices
    /// (the draw test below shows the triangle). Flagged, unflagged and
    /// culled exports translate, and dxil.dll, where there is one, takes
    /// each module.
    #[test]
    fn primitive_export_indices_drop_the_edge_flags() {
        #[cfg(windows)]
        let mut validator = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        )
        .ok();
        for prim in [
            TRIANGLE,
            TRIANGLE | EDGE_FLAGS,
            TRIANGLE | EDGE_FLAGS | CULL,
        ] {
            let ms = triangle_mesh(prim).unwrap_or_else(|e| panic!("{prim:#x}: {e}"));
            #[cfg(windows)]
            if let Some(v) = validator.as_mut() {
                v.validate(&mut ms.dxil.clone())
                    .unwrap_or_else(|e| panic!("{prim:#x}: {e}"));
            }
        }
    }

    /// A triangle mesh program on the host GPU and on WARP (where they have
    /// mesh shaders): triangle (0, 1, 2) covers the target with its corners'
    /// edge flags set as without them; with the culling bit, nothing is lit.
    #[test]
    #[cfg(windows)]
    fn edge_flagged_triangles_draw_as_unflagged_ones() {
        use super::host::{Draw, Host, Root};
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_FEATURE_D3D12_OPTIONS7, D3D12_FEATURE_DATA_D3D12_OPTIONS7,
            D3D12_MESH_SHADER_TIER_NOT_SUPPORTED, D3D12_SHADER_VISIBILITY_MESH,
        };
        let _one = super::valu_tests::ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ps = constant_color();
        let mut ran = 0;
        for (label, warp) in [("hardware", false), ("warp", true)] {
            let Ok(host) = Host::new(warp) else {
                eprintln!("no SM 6.6 {label} device; skipped");
                continue;
            };
            let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
            // SAFETY: the query writes the structure of the given size.
            let queried = unsafe {
                host.device.CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS7,
                    (&raw mut options).cast(),
                    size_of_val(&options) as u32,
                )
            };
            if queried.is_err() || options.MeshShaderTier == D3D12_MESH_SHADER_TIER_NOT_SUPPORTED {
                eprintln!("no mesh shaders on the {label} device; skipped");
                continue;
            }
            for (prim, culled) in [
                (TRIANGLE, false),
                (TRIANGLE | EDGE_FLAGS, false),
                (TRIANGLE | EDGE_FLAGS | CULL, true),
            ] {
                let ms = triangle_mesh(prim).unwrap();
                let root = host
                    .root_signature_for(&[(&ms, D3D12_SHADER_VISIBILITY_MESH)])
                    .unwrap();
                let pso = host
                    .mesh_pipeline_with(&root, &ms.dxil, &ps)
                    .unwrap_or_else(|e| panic!("{label}: mesh pipeline: {e}"));
                let texels = host
                    .draw(
                        &root,
                        &pso,
                        &Draw {
                            roots: vec![(0, Root::Constants(vec![0]))],
                            textures: &[],
                            samplers: &[],
                            vertices: &[],
                            stride: 0,
                            size: (16, 16),
                            mesh_groups: 1,
                        },
                    )
                    .unwrap_or_else(|e| panic!("{label}: draw: {e}"));
                let lit = texels.iter().filter(|t| t[3] != 0.0).count();
                if culled {
                    assert_eq!(lit, 0, "{label}, {prim:#x}: a culled triangle lit texels");
                    continue;
                }
                assert!(
                    texels.iter().all(|t| *t == [1.0, 0.5, 0.25, 1.0]),
                    "{label}, {prim:#x}: {lit} of 256 texels lit"
                );
            }
            eprintln!("{label}: edge-flagged triangles drew");
            ran += 1;
        }
        if ran == 0 {
            eprintln!("no device with mesh shaders; skipped");
        }
    }

    /// A pixel shader interpolating at sample positions (PERSP_SAMPLE
    /// barycentrics, `SPI_PS_INPUT_ADDR` bit 0) runs at sample frequency:
    /// DXC's PSV0 says so in PSInfo.SampleFrequency (runtime info byte 1)
    /// whenever an input is interpolated per sample.
    #[test]
    fn sample_interpolated_inputs_set_psv_sample_frequency() {
        let words = [
            0xC808_0000u32, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001,    // v_interp_p2_f32 v2, v1, attr0.x
            0xF800_180F,
            0x0202_0202, // exp mrt0, v2, v2, v2, v2 done vm
            0xBF81_0000,
        ];
        let psv_of = |input_addr: u32| {
            let native = super::NativeProgram {
                stage: super::HwStage::Ps,
                wave64: false,
                vgprs: 4,
                layout: vec![],
                used: vec![],
                io: crate::native::StageIo::Ps {
                    inputs: vec![crate::native::PsInput {
                        usage: crate::native::Usage::Generic,
                        row: 0,
                        mask: 0b1111,
                        attr: 0,
                        flat: false,
                        fp16: false,
                        pack2: false,
                        row2: 0,
                    }],
                    input_addr,
                    db_shader_control: 0,
                    patches: vec![],
                },
                code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            };
            let el = |name: &str, kind, usage| SigElement {
                name: name.into(),
                index: 0,
                kind,
                comp: 9,
                interp: 0,
                row: Some(0),
                col: 0,
                cols: 4,
                usage,
                half: false,
            };
            let isg1 = super::container::signature(&[el("TEXCOORD", 0, 0)], true);
            let osg1 = super::container::signature(&[el("SV_Target", 16, 15)], false);
            let bytes =
                crate::dxbc::build(&[(super::FourCC::ISG1, &isg1), (super::FourCC::OSG1, &osg1)]);
            let c = super::Container::parse(&bytes).unwrap();
            let p = crate::ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
            let st = crate::structure::structure(&p).unwrap();
            let res = crate::resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
            let io = crate::io::raise(&p, &native).unwrap();
            let out = super::emit(&c, &native, &p, &st, &io, &res).unwrap();
            // dxil.dll, where there is one, checks PSV0 against the module.
            #[cfg(windows)]
            if let Ok(mut v) = super::validator::Validator::load(
                super::validator::Validator::default_path().as_deref(),
            ) {
                v.validate(&mut out.dxil.clone()).unwrap();
            }
            let host = super::Container::parse(&out.dxil).unwrap();
            host.part(super::FourCC::PSV0).unwrap().data.to_vec()
        };
        // Runtime info starts after its size; PSInfo is {DepthOutput, SampleFrequency}.
        let center = psv_of(1 << 1);
        assert_eq!(center[4 + 1], 0, "center interpolation is per pixel");
        let sample = psv_of(1 << 0);
        assert_eq!(sample[4 + 1], 1, "PSInfo.SampleFrequency");
    }

    /// Emits a pixel program of `words` with these inputs and signatures.
    fn emit_pixel(
        words: &[u32],
        inputs: Vec<crate::native::PsInput>,
        input_addr: u32,
        isg1: &[u8],
        osg1: &[u8],
    ) -> Result<HostShader, ShaderError> {
        let native = super::NativeProgram {
            stage: super::HwStage::Ps,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: crate::native::StageIo::Ps {
                inputs,
                input_addr,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let bytes = crate::dxbc::build(&[(FourCC::ISG1, isg1), (FourCC::OSG1, osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native)?;
        let out = emit(&c, &native, &p, &st, &io, &res)?;
        // dxil.dll, where there is one, checks the module.
        #[cfg(windows)]
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut out.dxil.clone()).unwrap();
        }
        Ok(out)
    }

    /// A float4 element per row (16-bit ones declared so in the part).
    fn rows(elements: &[(&str, u8, bool)], input: bool) -> Vec<u8> {
        let els: Vec<SigElement> = elements
            .iter()
            .enumerate()
            .map(|(row, &(name, kind, _))| SigElement {
                name: name.into(),
                index: if kind == 3 { 0 } else { row as u32 },
                kind,
                comp: 9,
                interp: 0,
                row: Some(row as u32),
                col: 0,
                cols: 4,
                usage: 0,
                half: false,
            })
            .collect();
        let mut part = container::signature(&els, input);
        for (row, &(.., half)) in elements.iter().enumerate() {
            if half {
                part[8 + 32 * row + 16] = 6;
            }
        }
        part
    }

    /// Attribute 0 in row 0, 16-bit (one value per dword) or not.
    fn texcoord(fp16: bool) -> crate::native::PsInput {
        crate::native::PsInput {
            usage: crate::native::Usage::Generic,
            row: 0,
            mask: 1,
            attr: 0,
            flat: false,
            fp16,
            pack2: false,
            row2: 0,
        }
    }

    /// A 16-bit result written over a first interpolation half keeps that
    /// half's high 16 bits, which have no host form: what reads only the low
    /// half translates, a read of the whole dword does not.
    #[test]
    fn a_half_over_a_first_interpolation_half_reads_as_a_half() {
        let program = |read: u32| {
            let words = [
                0xD742_0002u32,
                0x0002_0000, // v_interp_p1ll_f16 v2, v0, attr0.x
                0xD75A_0003,
                0x040A_0200, // v_interp_p2_f16 v3, v1, attr0.x, v2
                0xCC21_0002,
                0x0A01_E503, // v_fma_mixlo_f16 v2, sel_lo(v3), 1.0, 0
                read,        // v4 from v2
                0xF800_180F,
                0x0404_0404, // exp mrt0, v4, v4, v4, v4 done vm
                0xBF81_0000,
            ];
            let isg1 = rows(&[("TEXCOORD", 0, true)], true);
            let osg1 = rows(&[("SV_Target", 16, false)], false);
            emit_pixel(&words, vec![texcoord(true)], 1 << 1, &isg1, &osg1)
        };
        program(0x7E08_1702).unwrap(); // v_cvt_f32_f16 v4, v2
        program(0x3408_0490).unwrap(); // v_lshlrev_b32 v4, 16, v2
        let whole = program(0x7E08_0302).unwrap_err(); // v_mov_b32 v4, v2
        assert!(whole.to_string().contains("first half"), "{whole}");
        let part = program(0x3408_0488).unwrap_err(); // v_lshlrev_b32 v4, 8, v2
        assert!(part.to_string().contains("first half"), "{part}");
    }

    /// A pixel export before the `done` one, both under a lane guard (EXEC
    /// narrowed without a branch): the lanes outside it are discarded at the
    /// `done` export, so the first one stores in every lane. With EXEC
    /// restored before the `done` export those lanes would write only the
    /// second target, which stays refused.
    #[test]
    fn an_export_before_a_discarding_done_export_stores_unguarded() {
        let program = |restore: &[u32]| {
            let mut words = vec![
                0xBE80_037Eu32, // s_mov_b32 s0, exec_lo
                0x7C22_0080,    // v_cmpx_lt_f32 exec, 0, v0
                0xF800_000F,
                0x0000_0000, // exp mrt0, v0, v0, v0, v0
            ];
            words.extend(restore);
            words.extend([
                0xF800_181F,
                0x0000_0000, // exp mrt1, v0, v0, v0, v0 done vm
                0xBF81_0000,
            ]);
            let isg1 = rows(&[("SV_Position", 3, false)], true);
            let osg1 = rows(&[("SV_Target", 16, false), ("SV_Target", 16, false)], false);
            emit_pixel(&words, vec![], 1 << 8, &isg1, &osg1)
        };
        program(&[]).unwrap();
        let e = program(&[0xBEFE_0300]).unwrap_err(); // s_mov_b32 exec_lo, s0
        assert!(e.to_string().contains("a conditional export"), "{e}");
    }

    /// The compiler reuses the j register (`v_mov` under a kill's EXEC) and
    /// interpolates in the other lanes: those still hold j, which
    /// structurisation's lane conditions prove. Interpolating in the lanes
    /// the copy ran in stays refused.
    #[test]
    fn an_interpolation_after_j_is_reused_in_other_lanes_translates() {
        let program = |other_lanes: &[u32]| {
            let mut words = vec![
                0xBE80_037Eu32, // s_mov_b32 s0, exec_lo
                0x7C22_0480,    // v_cmpx_lt_f32 exec, 0, v2
                0x7E02_0280,    // v_mov_b32 v1, 0
            ];
            words.extend(other_lanes);
            words.extend([
                0xC80C_0000, // v_interp_p1_f32 v3, v0, attr0.x
                0xC80D_0001, // v_interp_p2_f32 v3, v1, attr0.x
                0xBEFE_0300, // s_mov_b32 exec_lo, s0
                0xF800_180F,
                0x0303_0303, // exp mrt0, v3, v3, v3, v3 done vm
                0xBF81_0000,
            ]);
            let isg1 = rows(&[("TEXCOORD", 0, false), ("SV_Position", 3, false)], true);
            let osg1 = rows(&[("SV_Target", 16, false)], false);
            emit_pixel(&words, vec![texcoord(false)], 1 << 1 | 1 << 8, &isg1, &osg1)
        };
        program(&[0x8A7E_7E00]).unwrap(); // s_andn2_b32 exec_lo, s0, exec_lo
        let e = program(&[]).unwrap_err();
        assert!(e.to_string().contains("computed barycentrics"), "{e}");
    }

    /// Two elements sharing a row, the one at the higher channels listed
    /// first (as console signatures have them): the signature part keeps
    /// them in element order within the row, as the validator does.
    #[test]
    fn elements_sharing_a_row_keep_their_order() {
        let el = |index, col, cols| SigElement {
            name: "TEXCOORD".into(),
            index,
            kind: 0,
            comp: 9,
            interp: 0,
            row: Some(0),
            col,
            cols,
            usage: 0,
            half: false,
        };
        let isg1 = container::signature(&[el(1, 1, 2), el(0, 0, 1)], true);
        let osg1 = rows(&[("SV_Target", 16, false)], false);
        let words = [
            0xC808_0000u32, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001,    // v_interp_p2_f32 v2, v1, attr0.x
            0xF800_180F,
            0x0202_0202, // exp mrt0, v2, v2, v2, v2 done vm
            0xBF81_0000,
        ];
        emit_pixel(&words, vec![texcoord(false)], 1 << 1, &isg1, &osg1).unwrap();
    }

    /// An unread float input packed beside the front face (as console
    /// signatures have it when the float was `nointerpolation`) takes the
    /// row's constant mode: DXIL wants one mode per row.
    #[test]
    fn a_row_keeps_one_interpolation_mode() {
        let el = |name: &str, kind, comp, row, col| SigElement {
            name: name.into(),
            index: 0,
            kind,
            comp,
            interp: 0,
            row: Some(row),
            col,
            cols: 1,
            usage: 0,
            half: false,
        };
        let isg1 = container::signature(
            &[
                el("TEXCOORD", 0, 9, 0, 0),
                el("SV_IsFrontFace", 13, 5, 0, 1),
                el("COLOR", 0, 9, 1, 0),
            ],
            true,
        );
        let osg1 = rows(&[("SV_Target", 16, false)], false);
        let words = [
            0xC808_0000u32, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001,    // v_interp_p2_f32 v2, v1, attr0.x
            0xF800_180F,
            0x0202_0202, // exp mrt0, v2, v2, v2, v2 done vm
            0xBF81_0000,
        ];
        let color = crate::native::PsInput {
            row: 1,
            ..texcoord(false)
        };
        emit_pixel(&words, vec![color], 1 << 1, &isg1, &osg1).unwrap();
    }

    /// The LLVM text `dxc -dumpbin` prints for a DXIL container, when dxc
    /// is installed.
    #[cfg(windows)]
    fn dump_dxil(container: &[u8]) -> Option<String> {
        let dxc = match std::env::var_os("VICTORY_PC_DXC") {
            Some(p) => std::path::PathBuf::from(p),
            None => super::validator::Validator::default_path()?.with_file_name("dxc.exe"),
        };
        if !dxc.exists() {
            return None;
        }
        let file = std::env::temp_dir().join(format!("victory-interp-{}.dxil", std::process::id()));
        std::fs::write(&file, container).ok()?;
        let out = std::process::Command::new(dxc)
            .arg("-dumpbin")
            .arg(&file)
            .output()
            .ok()?;
        let _ = std::fs::remove_file(&file);
        assert!(
            out.status.success(),
            "dxc -dumpbin failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    #[cfg(windows)]
    #[test]
    fn interpolated_f16_writes_the_op_sel_half_with_modifiers() {
        // p1_f32 v2, v0, attr0.x; then a p2_f16 completing it into v2.
        let p2 = |dst_high: bool, omod: u32, clamp: bool| {
            let vgpr = |n: u32| 256 + n;
            let mut w0 = 0x35u32 << 26 | 0x35au32 << 16 | 2;
            if dst_high {
                w0 |= 1 << 14; // op_sel[3]: the high destination half.
            }
            if clamp {
                w0 |= 1 << 15;
            }
            let w1 = vgpr(1) << 9 | vgpr(2) << 18;
            [w0, w1 | omod << 27]
        };
        let dump = |dst_high: bool, omod: u32, clamp: bool| {
            let [w0, w1] = p2(dst_high, omod, clamp);
            let words = [
                0xC808_0000u32, // v_interp_p1_f32 v2, v0, attr0.x
                w0,
                w1, // v_interp_p2_f16 v2, attr0, v1, v0
                0xF800_180F,
                0x0202_0202, // exp mrt0, v2, v2, v2, v2 done vm
                0xBF81_0000,
            ];
            let native = super::NativeProgram {
                stage: super::HwStage::Ps,
                wave64: false,
                vgprs: 4,
                layout: vec![],
                used: vec![],
                io: crate::native::StageIo::Ps {
                    inputs: vec![crate::native::PsInput {
                        usage: crate::native::Usage::Generic,
                        row: 0,
                        mask: 0b1111,
                        attr: 0,
                        flat: false,
                        fp16: true,
                        pack2: false,
                        row2: 0,
                    }],
                    input_addr: 2,
                    db_shader_control: 0,
                    patches: vec![],
                },
                code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            };
            let el = |name: &str, kind, usage, half| SigElement {
                name: name.into(),
                index: 0,
                kind,
                comp: 9,
                interp: 0,
                row: Some(0),
                col: 0,
                cols: 4,
                usage,
                half,
            };
            let isg1 = super::container::signature(&[el("TEXCOORD", 0, 0, true)], true);
            let osg1 = super::container::signature(&[el("SV_Target", 16, 15, false)], false);
            let bytes =
                crate::dxbc::build(&[(super::FourCC::ISG1, &isg1), (super::FourCC::OSG1, &osg1)]);
            let c = super::Container::parse(&bytes).unwrap();
            let p = crate::ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
            let st = crate::structure::structure(&p).unwrap();
            let res = crate::resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
            let io = crate::io::raise(&p, &native).unwrap();
            super::emit(&c, &native, &p, &st, &io, &res).unwrap().dxil
        };
        let (Some(low), Some(high)) = (
            dump_dxil(&dump(false, 0, false)),
            dump_dxil(&dump(true, 1, true)),
        ) else {
            eprintln!("no dxc.exe; DXIL check skipped");
            return;
        };
        // The low-half write keeps the destination's high half.
        assert!(low.contains("-65536"), "low write keeps the high half");
        // The high-half write shifts the result up by 16.
        assert!(high.contains("shl i32"), "op_sel[3] writes the high half");
        // omod mul:2 and clamp (dx.op.unary.f32, opcode 7) are applied.
        assert!(high.contains("2.000000e+00"), "omod multiplies by two");
        assert!(
            high.contains("dx.op.unary.f32(i32 7"),
            "clamp saturates the result"
        );
        assert!(!low.contains("dx.op.unary.f32(i32 7"));
    }

    /// A pixel shader reading channel z of an input whose element starts at
    /// w. Looking the channel up subtracted the element's first channel
    /// before checking the channel lies in it, so in a build with overflow
    /// checks (tests, debug) any read below an element that starts at z or
    /// w panicked, whichever row it was in (found by random-input testing).
    #[test]
    fn reading_a_channel_below_an_input_element_does_not_underflow() {
        // v_interp_mov_f32 v3, p0, attr0.z; s_endpgm
        let words = [0xC80E_0202u32, 0xBF81_0000];
        let native = super::NativeProgram {
            stage: super::HwStage::Ps,
            wave64: false,
            vgprs: 4,
            layout: vec![],
            used: vec![],
            io: crate::native::StageIo::Ps {
                inputs: vec![crate::native::PsInput {
                    usage: crate::native::Usage::Generic,
                    row: 0,
                    mask: 0b1000,
                    attr: 0,
                    flat: false,
                    fp16: false,
                    pack2: false,
                    row2: 0,
                }],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let w_only = SigElement {
            name: "TEXCOORD".into(),
            index: 0,
            kind: 0,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 3,
            cols: 1,
            usage: 0,
            half: false,
        };
        let isg1 = super::container::signature(&[w_only], true);
        let bytes = crate::dxbc::build(&[(super::FourCC::ISG1, &isg1)]);
        let c = super::Container::parse(&bytes).unwrap();
        let p = crate::ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = crate::structure::structure(&p).unwrap();
        let res = crate::resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = crate::io::raise(&p, &native).unwrap();
        // Translated or refused with a reason; it must not panic.
        let _ = super::emit(&c, &native, &p, &st, &io, &res);
    }
}
