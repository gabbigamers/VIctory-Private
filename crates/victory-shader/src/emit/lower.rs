//! IR to DXIL, one machine instruction at a time (decision 0003).
//!
//! Every register dword is an `i32` (SCC an `i1`); float operations bitcast
//! their operands. Lane masks exist only as the per-lane booleans of
//! structurisation; branches and guards are built from them. A value the
//! emitter cannot express is `Missing` with a reason, and only fails the
//! program when something with an effect (an export, a branch) needs it.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::UserData;
use super::bitcode::{Attr, V};
use super::container::SigElement;
use super::shader::{Shader, flags};
use super::{
    HEAP_VIEWS, NULL_BUFFER_VIEW, ROOT_SRV_VIEW, ROOT_UAV_VIEW, ROOT_VIEW_OFFSET, VIEW_MARK_NONE,
    VIEW_READS, VIEW_WRITE_BITS, bitcode, loop_limit, loop_total_limit, shader, wave,
};
#[cfg(all(test, windows))]
use super::validator;
use crate::ShaderError;
use crate::gfx10::{Fields, NULL, Operand};
use crate::io::{Export, Interp, Io};
use crate::ir::{EXEC, Inst, Program, Reg, Value};
use crate::native::{HwStage, NativeProgram, StageIo};
use crate::resource::{Address, MEMORY, Resources, Role, Site, Source, USER_DATA};
use crate::structure::{Atom, Bool, LaneExit, Structured};

mod arith;
mod buffer;
mod bvh;
mod crosslane;
mod descriptors;
mod ds;
mod emulated;
mod flow;
mod half;
mod image;
mod limit;
mod masks;
mod memory;
mod operands;
mod scalar;
mod smem;
mod stage;
mod stopped;
#[cfg(test)]
mod thread_limit_tests;
mod valu;

type Why = &'static str;

#[derive(Debug, Clone, Copy)]
enum Val {
    I32(V),
    Bit(V),
    Missing(Why),
}

pub(crate) struct Lowered {
    pub dxil: Vec<u8>,
    /// Bit n: immediate sampler n is a comparison sampler.
    pub cmp_samplers: u64,
    pub root_cbvs: Vec<u8>,
    pub root_srvs: Vec<u8>,
    pub root_uavs: Vec<u8>,
    /// Reads title memory at computed addresses (the page table).
    pub memory: bool,
    /// Writes there (the written-pages bitmap).
    pub memory_writes: bool,
    /// Reads buffer views' addresses or texture views' sizes from
    /// descriptors (the view addresses).
    pub view_addresses: bool,
    /// Takes descriptors from the resource heap itself (root type 5, not
    /// a table), and whether as unordered access views.
    pub heap: bool,
    pub heap_writes: bool,
    /// Marks the pages it writes through buffer views (`emit`, "Write
    /// marks").
    pub view_marks: bool,
    pub user_data: Vec<UserData>,
    pub samplers: Vec<[u32; 4]>,
    /// Heap handles made with a non-uniform index.
    #[cfg(test)]
    pub nonuniform_handles: u32,
    /// Heap handles written.
    #[cfg(test)]
    pub heap_handles: u32,
    /// Whether the program uses wave operations.
    #[cfg(test)]
    pub wave_ops: bool,
    /// Whether it declares rasterizer-ordered views.
    #[cfg(test)]
    pub rovs: bool,
    /// Whether the module declares `dx.op.waveGetLaneIndex`.
    #[cfg(test)]
    pub uses_lane_index: bool,
    /// Whether the module contains the 32-lane half mask (`!31`).
    #[cfg(test)]
    pub has_half_wave_mask: bool,
    /// The function as the module writes it.
    #[cfg(test)]
    pub flow: super::bitcode::Flow,
    /// Where in [`Lowered::flow`] a lane stopped by [`LOOP_LIMIT`] or by
    /// [`LOOP_TOTAL_LIMIT`] ends the program at once, if any latch sends
    /// one there (a pixel's stopped lanes mostly go on instead: the module
    /// `stopped`).
    #[cfg(test)]
    pub limit_stop: Option<usize>,
    /// The wave size the program declares (`WaveSize`), if any.
    pub wave_size: Option<u32>,
    /// Why a compute program's lanes must be the console's: bits of
    /// [`wave_need`]; 0 if they need not be.
    pub wave_needs: u32,
    /// The thread group carries the console's wave itself (`emulated`).
    pub emulated: bool,
    /// The meetings of an emulated wave's lanes that were written.
    #[cfg(test)]
    pub meetings: u32,
}

/// Why a compute program depends on the console's wave: its size and which
/// thread is which lane. A program with any of these declares the console's
/// wave size, unless its thread group carries the wave itself (`emulated`).
pub mod wave_need {
    /// `ds_swizzle_b32`.
    pub const SWIZZLE: u32 = 1;
    /// A lane permute through shared-memory addresses (`ds_bpermute_b32`).
    pub const PERMUTE: u32 = 1 << 1;
    /// The count of a mask's lanes below each lane (`v_mbcnt`).
    pub const COUNT_BELOW: u32 = 1 << 2;
    /// A read of one lane by number (`v_readlane_b32`), other than
    /// [`RESTORED`].
    pub const LANE_READ: u32 = 1 << 3;
    /// A read of a neighbouring lane (DPP).
    pub const DPP: u32 = 1 << 4;
    /// A lane mask read as a number.
    pub const MASK_READ: u32 = 1 << 5;
    /// A lane mask made from a number or a constant lane set.
    pub const MASK_MADE: u32 = 1 << 6;
    /// Shared memory in a thread group of one wave, which the console
    /// compiler orders without barriers.
    pub const SHARED_MEMORY: u32 = 1 << 7;
    /// Cross-lane steps of a whole-wave section only (a wave reduction or
    /// prefix the console compiler expanded), which are lowered to the
    /// host's wave operations.
    pub const SECTION: u32 = 1 << 8;
    /// A write of one lane by number (`v_writelane_b32`): the lane named
    /// takes a scalar. Like a mask made from a number, it needs a lane's
    /// own number and nothing of the other lanes.
    pub const LANE_WRITE: u32 = 1 << 9;
    /// A `v_readlane_b32` that gives back the scalar a `v_writelane_b32`
    /// put in that lane (`Structured::restored`: a scalar the console
    /// compiler parked in a vector register). It is lowered as that
    /// scalar and reads no lane; it is named apart so that a program whose
    /// lane reads are all of this kind can be told from one that reads
    /// other lanes. The write that parked the scalar is a [`LANE_WRITE`],
    /// so a program with this reason always has that one too.
    pub const RESTORED: u32 = 1 << 10;

    /// Each bit's name, in bit order.
    pub const NAMES: [&str; 11] = [
        "swizzle",
        "lane permute",
        "count below a lane",
        "lane read",
        "DPP",
        "mask read as a number",
        "mask made from a number",
        "shared memory within one wave",
        "whole-wave section",
        "lane write",
        "scalar read back from its lane",
    ];
}

pub(crate) struct Parts<'a> {
    pub native: &'a NativeProgram,
    pub p: &'a Program,
    pub st: &'a Structured,
    pub io: &'a Io,
    pub res: &'a Resources,
    pub inputs: Vec<SigElement>,
    pub outputs: Vec<SigElement>,
    pub prims: Vec<SigElement>,
    /// Mesh: what the group id arrives multiplied by.
    pub mesh_stride: u32,
    /// Writes through buffer views mark their pages (`emit`, "Write marks").
    pub view_marks: bool,
}

enum Stop {
    /// A phi whose incoming value has no DXIL form was read: redo with it missing.
    Retry(Value, Why),
    /// The program reads user data where its thread limits were put: redo
    /// without them.
    Unlimited,
    Fail(ShaderError),
}

#[cfg(any(test, all(windows, feature = "host")))]
pub(crate) fn lower(parts: &Parts) -> Result<Lowered, ShaderError> {
    lower_with(parts, false)
}

/// What a lane's back edges are counted against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// [`LOOP_LIMIT`]: the iterations of one loop, per entry into it.
    pub each: u32,
    /// [`LOOP_TOTAL_LIMIT`]: the back edges a lane takes in all.
    pub total: u32,
}

impl Limits {
    /// The limits every translation uses ([`loop_limit`],
    /// [`loop_total_limit`]).
    fn in_force() -> Self {
        Limits {
            each: loop_limit(),
            total: loop_total_limit(),
        }
    }
}

/// [`lower`]; with `thread_limits`, a compute program's threads past the
/// host's limits leave at entry ([`Lower::limit_threads`]). A program
/// that cannot take them is lowered exactly as without.
pub(crate) fn lower_with(parts: &Parts, thread_limits: bool) -> Result<Lowered, ShaderError> {
    lower_limited(parts, Limits::in_force(), thread_limits)
}

/// [`lower_with`] with the loop limits given: the tests of the counting
/// run loops against small ones.
pub(crate) fn lower_limited(parts: &Parts, limits: Limits, mut thread_limits: bool) -> Result<Lowered, ShaderError> {
    let mut bad = HashMap::new();
    loop {
        match Lower::new(parts, &bad, limits).run(thread_limits) {
            Ok(l) => return Ok(l),
            Err(Stop::Unlimited) => {
                thread_limits = false;
                bad.clear();
            }
            Err(Stop::Retry(v, why)) => {
                if bad.insert(v, why).is_some() {
                    return Err(ShaderError::Unsupported {
                        offset: 0,
                        what: "a phi that does not settle",
                    });
                }
            }
            Err(Stop::Fail(e)) => return Err(e),
        }
    }
}

/// `s_sendmsg` id of a mesh or NGG stage's output allocation request.
const GS_ALLOC_REQ: u16 = 9;

/// A descriptor load's address given by root parameters alone: root
/// parameter and byte offset.
type EntryAddress = (usize, u32);

struct Lower<'a> {
    s: Shader,
    x: &'a Parts<'a>,
    bad: &'a HashMap<Value, Why>,
    lane_fail: HashMap<Value, Why>,
    vals: HashMap<Value, Val>,
    floats: HashMap<Value, V>,
    /// Values (missing in `vals`) whose low 16 bits are known: a 16-bit
    /// result written over a value with no DXIL form keeps its high half.
    low16: HashMap<Value, V>,
    /// DXIL block per IR block (unreachable: none).
    block: Vec<Option<usize>>,
    cur: usize,
    bools: HashMap<(usize, Bool), V>,
    lane_bits: HashMap<Value, V>,
    /// IR phi location by value.
    phi_at: HashMap<Value, (usize, usize)>,
    mask_phis: HashMap<Value, V>,
    pending_masks: Vec<(V, usize, usize)>,
    data_phis: Vec<(V, usize, usize)>,
    read_phis: HashSet<Value>,
    params: HashMap<Value, (usize, u32)>,
    user_data: Option<V>,
    user_reads: BTreeMap<u32, UserData>,
    /// Handles of root SRVs and UAVs read as bounded views, by user-data
    /// dword ([`Lower::root_view`]).
    root_views: HashMap<u32, V>,
    samplers: Vec<[u32; 4]>,
    descriptors: HashMap<(Site, bool), &'a Source>,
    scalar: HashMap<Site, Address>,
    exports: HashMap<Site, &'a Export>,
    interps: HashMap<Site, &'a Interp>,
    handles: HashMap<(usize, Site, u8, bool, u32), V>,
    /// Handles made in the entry block for descriptors at addresses the
    /// root parameters alone give (table and heap loads with no other
    /// term), by base value, byte offset, dword, offset register, sampler
    /// heap and properties ([`Lower::entry_address`]).
    entry_handles: HashMap<(EntryAddress, u8, bool, u32), V>,
    /// Handles of descriptors chosen by control flow, made in their join
    /// block, by the choice (its `Debug` text), sampler heap and properties.
    join_handles: HashMap<(String, bool, u32), V>,
    /// Where the edge from one IR block to another arrives from, if not
    /// from the end of the first: the block splitting a critical edge, or
    /// what [`Lower::limit_exit`] puts on a latch's other edge.
    edges: HashMap<(usize, usize), usize>,
    /// Per IR block: a `done` export is behind every lane entering it.
    exported: Vec<bool>,
    /// Reverse-postorder index per IR block.
    rpo: Vec<usize>,
    /// DXIL block each IR block ends in (guarded stores split blocks).
    tail: Vec<Option<usize>>,
    /// The program writes UAVs: barriers fence them too.
    writes_uavs: bool,
    /// [`Lowered::heap`], [`Lowered::heap_writes`].
    heap: [bool; 2],
    /// src0 of the current instruction as DPP fetched it.
    src0: Option<V>,
    /// Compute code that reads other lanes: thread ids follow the host
    /// lanes, so lane n is console lane n.
    console_lanes: bool,
    /// Why ([`wave_need`]), with the need for shared memory in one wave.
    wave_needs: u32,
    /// The thread group carries the console's wave itself (`emulated`): the
    /// program is in its wave form, a thread's lane is its place in the
    /// group, and no host wave operation is used.
    emu: Option<emulated::Emulated>,
    /// Why the wave form cannot be lowered, found before lowering starts.
    emu_refused: Option<Why>,
    /// How deep the code being written is in parts only some lanes run
    /// ([`Lower::begin_guarded`]).
    in_guarded: u32,
    /// Pixel: an ordered section (POPS), whose UAVs the host makes
    /// rasterizer-ordered views.
    pops: bool,
    /// The program uses title memory (addresses it computes, the BVH
    /// instruction, root parameters through the user-data pointer): its root
    /// SRVs and UAVs are read there too ([`Lower::raw_view`]).
    root_memory: bool,
    /// Mesh: the thread index in the group (made in the entry block).
    flat: Option<V>,
    /// Mesh: IR blocks whose `s_barrier` is left out.
    no_barrier: HashSet<usize>,
    /// Vertex and mesh outputs holding 16-bit values: row, whether two per
    /// dword ([`packed_half`]).
    half_outputs: HashMap<u8, bool>,
    /// How lane masks become data.
    mask_data: MaskData,
    /// The instruction being lowered may read lane masks as data.
    mask_ok: bool,
    /// Ballots of masks read as data, by low dword: (low, high).
    ballots: HashMap<Value, (V, V)>,
    /// Masks whose ballot is being built: a mask whose lane bit needs its
    /// own value as a number (an opaque mask keyed by a dword it shares
    /// with another) would otherwise recurse until the stack overflows.
    balloting: HashSet<Value>,
    /// Block writing each value.
    def_block: HashMap<Value, usize>,
    /// IR block being lowered.
    cur_ir: usize,
    /// Whole-wave sections by block and start index.
    sections: HashMap<usize, HashMap<usize, super::wave::Section>>,
    /// Values written in a section that code outside it reads.
    section_out: HashSet<Value>,
    /// The program's constant table (the code from the first byte read
    /// through `s_getpc_b64` on): global, first byte, dwords.
    code_table: Option<(V, u32, u32)>,
    /// What back edges are counted against.
    limits: Limits,
    /// Loop headers' iteration counts (see [`LOOP_LIMIT`]), by IR block.
    counters: HashMap<usize, limit::Header>,
    /// The lane's count of all its back edges (see [`LOOP_TOTAL_LIMIT`])
    /// as each IR block holds it from its start to its end: a phi, or
    /// nothing where the lane has taken none. Only set for blocks a back
    /// edge can still follow ([`Lower::count_loops`]).
    total: Vec<Option<V>>,
    /// The phis of that count, by IR block, each with its incoming (value,
    /// DXIL block) pairs.
    totals: HashMap<usize, limit::Counter>,
    /// The block a lane stopped by [`LOOP_LIMIT`] or [`LOOP_TOTAL_LIMIT`]
    /// ends the program in (`stopped` says which lanes do).
    abort: Option<usize>,
    /// Pixel: the lane's count of its back edges along the edge on which a
    /// latch leaves its loop, by the edge's IR blocks, where it is not what
    /// the latch holds: a stopped lane's is used up there
    /// ([`Lower::leave_stopped`]).
    left: HashMap<(usize, usize), V>,
    /// The 16/64-bit round mode (MODE bits 3:2) each block starts with, and
    /// the one in force at the instruction being lowered (see [`round16`]).
    round16: Vec<Option<u8>>,
    mode16: Option<u8>,
}

/// The 16/64-bit round mode (MODE bits 3:2: 0 nearest even, 3 toward zero)
/// after `inst`, from `mode` before it (`None`: not known).
fn round16_after(inst: &Inst, mode: Option<u8>) -> Option<u8> {
    match (inst.name(), &inst.fields) {
        ("s_round_mode", Fields::Sopp { imm }) => Some((imm >> 2 & 3) as u8),
        (
            "s_setreg_imm32_b32" | "s_setreg_b32",
            Fields::Sop {
                src, imm: Some(h), ..
            },
        ) => {
            let (id, off, size) = (h & 0x3F, h >> 6 & 0x1F, (h >> 11 & 0x1F) + 1);
            if id != 1 || off >= 4 || off + size <= 2 {
                return mode;
            }
            match src.first() {
                Some(&Operand::Literal(x)) if off <= 2 && off + size >= 4 => {
                    Some((x >> (2 - off) & 3) as u8)
                }
                _ => None,
            }
        }
        _ => mode,
    }
}

/// The 16/64-bit round mode each block starts with: nearest even at the
/// program's start, then as the MODE writes on the way set it; `None` where
/// paths that set it differently join (or a write is not a constant).
fn round16(p: &crate::ir::Program) -> Vec<Option<u8>> {
    // None: not reached yet; Some(m): the mode (m None: not known).
    let mut entry: Vec<Option<Option<u8>>> = vec![None; p.blocks.len()];
    entry[0] = Some(Some(0));
    let order = crate::ir::reverse_postorder(&p.blocks);
    loop {
        let mut changed = false;
        for &b in &order {
            let Some(mut m) = entry[b] else { continue };
            for inst in &p.blocks[b].insts {
                m = round16_after(inst, m);
            }
            for s in p.blocks[b].exit.successors() {
                let new = match entry[s] {
                    None => Some(m),
                    Some(old) if old == m => continue,
                    Some(_) => Some(None),
                };
                if entry[s] != new {
                    entry[s] = new;
                    changed = true;
                }
            }
        }
        if !changed {
            return entry.into_iter().map(Option::flatten).collect();
        }
    }
}

/// Iterations after which a loop is left, per lane and per entry into the
/// loop, unless [`super::loop_limit`] says otherwise. The console runs a
/// loop as long as its data says; data the host has not mirrored yet (a
/// null view reads zeros) can make that four billion iterations, which
/// hangs the host GPU until the driver resets it. A lane the limit stops
/// (it would have gone round its loop once more) ends as its stage
/// defines (the module `stopped`). A compute, vertex or mesh lane goes to
/// the abort block and runs nothing that follows the loop; a vertex or
/// mesh lane writes there the outputs that mark it. A latch with an
/// unconditional back edge branches there itself; one with a conditional
/// back edge leaves the loop first, and the block after it sends the
/// stopped lanes there ([`Lower::limit_exit`]). A pixel lane is discarded
/// in that block and goes on from it as a helper lane, which writes
/// nothing and takes no further back edge; at an unconditional back edge
/// it is discarded and ends. A lane whose own condition ends the loop,
/// also on the last iteration the limit allows, goes on as it would
/// without a limit.
///
/// Each loop has its own count ([`Lower::header_loops`] says which back
/// edges make one loop). It starts at 0 wherever the lane enters the loop
/// from outside it, goes up by one on each of the loop's back edges, and
/// is not moved by the back edges of the loops inside it. A loop in
/// another therefore starts again at 0 each time the outer loop comes
/// round to it, also where the two begin at the same block (an outer loop
/// whose body begins with its inner loop), and the outer loop's count
/// stays where it is while the inner loop goes round. What bounds a lane
/// in a nest is [`LOOP_TOTAL_LIMIT`].
///
/// The loops are read from the control-flow graph, and there a
/// conditional `continue` that falls through towards its loop's end is an
/// inner loop that begins at the same block (as is a conditional
/// `continue` of an outer loop taken from inside an inner one). It has a
/// count of its own, which starts again each time the loop's end is
/// taken, so a loop written that way is not left after this many
/// iterations: what bounds it is [`LOOP_TOTAL_LIMIT`], four times as many.
// shortcut: a genuine loop of more iterations than this is cut short too
// (its lanes are killed).
pub(super) const LOOP_LIMIT: u32 = 1 << 20;

/// Back edges after which a lane is stopped, over all its loops and every
/// entry into each, unless [`super::loop_total_limit`] says otherwise: a
/// lane takes a back edge only while the back edges it has taken, that one
/// included, are fewer than this. The counts per loop alone would let a
/// nest run for the product of its loops' limits (2^40 iterations two
/// deep); this is what bounds a lane's work in all. A lane it stops ends
/// exactly as one [`LOOP_LIMIT`] stops: the same branch of the same latch
/// tests both, and the stopped lane goes the same way. A pixel lane that
/// goes on as a helper after either stop does so with this count used up,
/// so that this limit refuses it every further back edge.
///
/// The value is four times [`LOOP_LIMIT`], between two needs that no value
/// meets together. It has to be far enough above [`LOOP_LIMIT`] that a
/// genuine nest is not stopped: a nest whose loops each stay under their
/// own limit runs to its end while the lane goes round fewer than 2^22
/// times in all, which 2,048 by 2,048 iterations two deep just do. And it
/// is the most that any program can make one lane run, so it has to be
/// low: lanes running to [`LOOP_LIMIT`] made draws of 0.3 to 1.3 s where
/// that was seen ([`Lower::helper_reads_own`]), every doubling of this
/// value doubles what a program reaching it in as many lanes costs, and
/// Windows by default resets a device that does not answer for two
/// seconds. Four times keeps a nest's worst case a small multiple of a
/// single loop's. Either stop is a defect to look into
/// (`docs/progress.md`, C7), and unlike a device reset it leaves the run
/// alive to say which program it was.
///
/// It stops lanes that ran to their end while only the loops were
/// counted: any lane that goes round 2^22 times or more in all with each
/// of its loops under [`LOOP_LIMIT`], as in a nest of 2,048 by 2,049
/// iterations whose loops begin at blocks of their own, or in five long
/// loops one after another (four cannot reach it).
// shortcut: a genuine nest that goes round more often than this in one
// lane is cut short too (its lanes are killed).
pub(super) const LOOP_TOTAL_LIMIT: u32 = 4 * LOOP_LIMIT;

/// Lane masks read as numbers (`s_bcnt1`, `s_ff1`, `v_mbcnt`, scalar
/// compares, bit tests) are ballots of the lanes' booleans, taken where the
/// mask is written: structurisation's `active` masks hold only lanes that
/// reach that block, so the ballot misses none.
/// A page table lookup ([`Lower::memory`]).
struct MemoryEntry {
    handle: V,
    offset: V,
    /// Block × 4096 + page in the block.
    entry: V,
    block: V,
    /// The level-1 index: address bits 28-39.
    l1: V,
    /// No host copy holds the page.
    miss: V,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskData {
    /// Compute: host lane n is console lane n (the program declares the
    /// console's wave size), so ballots are the console's masks bit for bit
    /// and any use is exact.
    Console,
    /// Pixel and vertex programs: ballots over the host's wave, whatever its
    /// size. Only uses that mean the same on any wave are allowed: counts,
    /// the first lane, the count below a lane, equality (the HLSL wave
    /// operations the console compiler expands). Lane numbers stay host
    /// lanes throughout (`v_mbcnt`, `v_readlane` at a computed lane).
    Host,
    /// Mesh programs: their lanes and waves (LDS per console wave) are the
    /// console's but no host wave size can be declared. Only a mask's bit
    /// count is taken, over the host's wave (the console compiler's test of
    /// a value for uniformity, `WaveActiveAllEqual`): lane numbers stay the
    /// console's, so the first lane and the count below a lane are refused.
    None,
}

/// What a value holding a lane mask read as a number stops with.
const LANE_MASK: Why = "a lane mask read as data";

/// Instructions that read a lane mask (not EXEC implicitly, not a lane's
/// bit) as a number.
fn reads_mask_as_data(st: &Structured, i: &Inst) -> bool {
    let is_mask = |v: Value| st.masks.contains_key(&v) || st.highs.contains_key(&v);
    let ops: Vec<&Operand> = match &i.fields {
        // A mask operation (its result a mask) is no data use.
        Fields::Sop { .. }
            if i.writes
                .iter()
                .any(|&(r, w)| matches!(r, Reg::S(_)) && is_mask(w)) =>
        {
            return false;
        }
        Fields::Sop { src, .. } => src.iter().collect(),
        Fields::Valu(v)
            if !matches!(i.name(), "v_cndmask_b32") && !i.name().contains("_co_ci_") =>
        {
            v.src.iter().collect()
        }
        _ => return false,
    };
    ops.into_iter().any(|op| match *op {
        Operand::Scalar { code, .. } if code != NULL => i.read(Reg::S(code)).is_some_and(is_mask),
        _ => false,
    })
}

/// Where half `h` (dword h / 2, the high half when odd) of the parameter
/// whose first element is in row `a` comes from, when the console packs
/// 16-bit values two per dword: a row and channel of `elements`, none for
/// padding. The console fills its parameters with the 16-bit elements taken
/// by first channel, then row: each takes whole dwords of the first
/// parameter with room for it (four dwords each), its channels in order.
fn packed_half(elements: &[SigElement], a: u8, h: u8) -> Option<(u8, u8)> {
    let mut halves: Vec<&SigElement> = elements
        .iter()
        .filter(|e| e.half && e.kind == 0 && e.row.is_some())
        .collect();
    halves.sort_by_key(|e| (e.col, e.row));
    // (first element's row, (row, channel) per half)
    type Param = (Option<u32>, Vec<Option<(u8, u8)>>);
    let mut params: Vec<Param> = Vec::new();
    for e in halves {
        let need = usize::from(e.cols.div_ceil(2) * 2);
        let i = match params.iter().position(|p| p.1.len() + need <= 8) {
            Some(i) => i,
            None => {
                params.push((e.row, Vec::new()));
                params.len() - 1
            }
        };
        let row = u8::try_from(e.row?).ok()?;
        let slots = &mut params[i].1;
        slots.extend((0..e.cols).map(|c| Some((row, e.col + c))));
        slots.resize(slots.len().next_multiple_of(2), None);
    }
    // Two parameters starting in one row would leave the half ambiguous.
    let mut found = params.iter().filter(|p| p.0 == Some(u32::from(a)));
    match (found.next(), found.next()) {
        (Some(p), None) => p.1.get(usize::from(h)).copied().flatten(),
        _ => None,
    }
}

/// Vertex or mesh outputs of 16-bit values (see [`Lower::half_outputs`]),
/// by row: whether two per dword.
fn half_outputs(io: &StageIo) -> HashMap<u8, bool> {
    let entries = match io {
        StageIo::Vs { outputs, .. } => outputs,
        StageIo::Ms(ms) => &ms.outputs,
        _ => return HashMap::new(),
    };
    entries
        .iter()
        .filter(|o| o.param & 0x80 == 0 && o.fp16)
        .map(|o| (o.row, o.pack2))
        .collect()
}

fn unsupported(offset: usize, what: Why) -> Stop {
    Stop::Fail(ShaderError::Unsupported { offset, what })
}

impl<'a> Lower<'a> {
    fn new(x: &'a Parts<'a>, bad: &'a HashMap<Value, Why>, limits: Limits) -> Self {
        let p = x.p;
        let mut phi_at = HashMap::new();
        for (b, block) in p.blocks.iter().enumerate() {
            for (i, phi) in block.phis.iter().enumerate() {
                phi_at.insert(phi.value, (b, i));
            }
        }
        let mut s = Shader::new(x.native.stage == HwStage::Ps);
        s.inputs = x.inputs.clone();
        s.outputs = x.outputs.clone();
        let insts = || p.blocks.iter().flat_map(|b| &b.insts);
        let writes_uavs = insts().any(|i| {
            matches!(i.fields, Fields::Image { .. } | Fields::Buffer { .. })
                && (i.name().contains("store") || i.name().contains("atomic"))
        });
        let root_memory = x.res.scalar.iter().any(|s| matches!(s.1.param, MEMORY | USER_DATA))
            || insts().any(|i| i.name().starts_with("image_bvh"));
        let mut console_lanes = false;
        let mut wave_needs = 0;
        let (mut emu, mut emu_refused) = (None, None);
        if let StageIo::Cs(cs) = x.native.io {
            s.threads = Some(cs.threads);
            s.lds_dwords = cs.lds_bytes() / 4;
            // Code reading other lanes needs the console's wave size and
            // lane order; so do lane masks read as numbers, and LDS shared
            // within one wave, where the console compiler leaves out barriers.
            for (b, block) in p.blocks.iter().enumerate() {
                // Whole-wave sections' steps are named as such (found only
                // for a block that has such an instruction).
                let mut sections = None;
                for (n, i) in block.insts.iter().enumerate() {
                    let name = i.name();
                    let need = if name.starts_with("ds_swizzle") {
                        wave_need::SWIZZLE
                    } else if name.contains("permute") {
                        wave_need::PERMUTE
                    } else if name.starts_with("v_mbcnt") {
                        wave_need::COUNT_BELOW
                    } else if name == "v_readlane_b32" {
                        match i.writes.first() {
                            Some(w) if x.st.restored.contains_key(&w.1) => wave_need::RESTORED,
                            _ => wave_need::LANE_READ,
                        }
                    } else if name == "v_writelane_b32" {
                        wave_need::LANE_WRITE
                    } else if matches!(&i.fields, Fields::Valu(v) if v.dpp.is_some()) {
                        wave_need::DPP
                    } else if reads_mask_as_data(x.st, i) {
                        wave_need::MASK_READ
                    } else {
                        continue;
                    };
                    let inside = sections
                        .get_or_insert_with(|| wave::sections(p, b))
                        .iter()
                        .any(|(&start, sec)| n > start && n < sec.end);
                    wave_needs |= if inside { wave_need::SECTION } else { need };
                }
            }
            {
                // Masks held as numbers or constant lane sets: the lane
                // tests its own bit by its console lane.
                let whens = x.st.exits.iter().filter_map(|e| match *e {
                    LaneExit::Branch { when, .. } => Some(when),
                    _ => None,
                });
                let mut all: Vec<Bool> =
                    x.st.guards
                        .iter()
                        .flatten()
                        .flatten()
                        .copied()
                        .chain(whens)
                        .collect();
                all.sort_unstable();
                all.dedup();
                if all.into_iter().any(|g| {
                    x.st.bools
                        .support(g)
                        .iter()
                        .any(|a| matches!(a, Atom::Opaque(_) | Atom::Constant(_)))
                }) {
                    wave_needs |= wave_need::MASK_MADE;
                }
            }
            console_lanes = wave_needs != 0;
            let wave = if x.native.wave64 { 64 } else { 32 };
            let one_wave = cs.threads.iter().fold(1u32, |count, &n| count.saturating_mul(n)) <= wave;
            if one_wave && s.lds_dwords > 0 {
                wave_needs |= wave_need::SHARED_MEMORY;
            }
            if x.st.waves {
                // The group is the wave: nothing is asked of the host's.
                match emulated::Emulated::plan(x, s.lds_dwords) {
                    Ok(e) => {
                        s.lds_extra = emulated::dwords(e.wave);
                        emu = Some(e);
                        // A lane tests its own bit of a mask held as a
                        // number by its place in the group.
                        console_lanes = true;
                    }
                    Err(why) => emu_refused = Some(why),
                }
            } else if wave_needs != 0 {
                s.wave_size = Some(wave);
            }
        } else if x.st.waves {
            emu_refused = Some(emulated::NOT_COMPUTE);
        }
        let mut no_barrier = HashSet::new();
        if let StageIo::Ms(ms) = &x.native.io {
            s.prims = x.prims.clone();
            s.threads = Some([ms.threads, 1, 1]);
            s.lds_dwords = ms.lds_bytes.div_ceil(4);
            s.mesh = Some(super::shader::Mesh {
                max_vertices: ms.max_vertices,
                max_primitives: ms.max_primitives,
                topology: ms.topology,
            });
            // Barriers order the lanes' LDS accesses; the console's
            // allocation request needs them too, but the host does not. One
            // lane sends that request, and what it dominates (the path for
            // counts out of range) runs in that lane alone, so its barrier
            // would hang the host.
            let order = crate::ir::reverse_postorder(&p.blocks);
            let mut index = vec![usize::MAX; p.blocks.len()];
            for (n, &b) in order.iter().enumerate() {
                index[b] = n;
            }
            let idom = crate::ir::idoms(&p.blocks, &order, &index);
            let alloc: Vec<usize> = (0..p.blocks.len())
                .filter(|&b| {
                    p.blocks[b].insts.iter().any(|i| {
                        i.name() == "s_sendmsg"
                            && matches!(i.fields, Fields::Sopp { imm } if imm & 0xF == GS_ALLOC_REQ)
                    })
                })
                .collect();
            for &b in &order {
                let mut d = b;
                let dominated = loop {
                    if alloc.contains(&d) {
                        break true;
                    }
                    if idom[d] == d || idom[d] == usize::MAX {
                        break false;
                    }
                    d = idom[d];
                };
                if ms.lds_bytes == 0 || dominated {
                    no_barrier.insert(b);
                }
            }
        }
        Lower {
            s,
            x,
            bad,
            lane_fail: HashMap::new(),
            vals: HashMap::new(),
            floats: HashMap::new(),
            low16: HashMap::new(),
            block: vec![None; p.blocks.len()],
            cur: 0,
            bools: HashMap::new(),
            lane_bits: HashMap::new(),
            phi_at,
            mask_phis: HashMap::new(),
            pending_masks: Vec::new(),
            data_phis: Vec::new(),
            read_phis: HashSet::new(),
            params: x.res.params.iter().map(|&(v, p, d)| (v, (p, d))).collect(),
            user_data: None,
            user_reads: BTreeMap::new(),
            root_views: HashMap::new(),
            samplers: Vec::new(),
            descriptors: x
                .res
                .descriptors
                .iter()
                .map(|d| ((d.site, d.role == Role::Sampler), &d.source))
                .collect(),
            scalar: x.res.scalar.iter().copied().collect(),
            exports: x.io.exports.iter().map(|e| (e.site, e)).collect(),
            interps: x.io.interps.iter().map(|(s, i)| (*s, i)).collect(),
            handles: HashMap::new(),
            entry_handles: HashMap::new(),
            join_handles: HashMap::new(),
            edges: HashMap::new(),
            exported: vec![false; p.blocks.len()],
            rpo: vec![usize::MAX; p.blocks.len()],
            tail: vec![None; p.blocks.len()],
            writes_uavs,
            heap: [false; 2],
            src0: None,
            console_lanes,
            wave_needs,
            emu,
            emu_refused,
            in_guarded: 0,
            pops: matches!(x.native.io, StageIo::Ps { db_shader_control, .. }
                if db_shader_control & 1 << 16 != 0),
            root_memory,
            flat: None,
            no_barrier,
            half_outputs: half_outputs(&x.native.io),
            mask_data: match x.native.stage {
                HwStage::Cs => MaskData::Console,
                HwStage::Ps | HwStage::Vs => MaskData::Host,
                _ => MaskData::None,
            },
            mask_ok: false,
            ballots: HashMap::new(),
            balloting: HashSet::new(),
            def_block: p
                .blocks
                .iter()
                .enumerate()
                .flat_map(|(b, block)| {
                    block
                        .insts
                        .iter()
                        .flat_map(move |i| i.writes.iter().map(move |w| (w.1, b)))
                        .chain(block.phis.iter().map(move |phi| (phi.value, b)))
                })
                .collect(),
            cur_ir: 0,
            sections: HashMap::new(),
            section_out: HashSet::new(),
            code_table: None,
            limits,
            counters: HashMap::new(),
            total: vec![None; p.blocks.len()],
            totals: HashMap::new(),
            abort: None,
            left: HashMap::new(),
            round16: round16(p),
            mode16: Some(0),
        }
    }

    fn run(mut self, thread_limits: bool) -> Result<Lowered, Stop> {
        let p = self.x.p;
        // First, so that the program's first block is the one after the test.
        let limits = if thread_limits { self.limit_threads() } else { Vec::new() };
        let order = crate::ir::reverse_postorder(&p.blocks);
        for (n, &b) in order.iter().enumerate() {
            self.rpo[b] = n;
        }
        for (n, &b) in order.iter().enumerate() {
            self.block[b] = Some(if n == 0 {
                self.s.entry
            } else {
                self.s.m.new_block()
            });
        }
        // Loop headers count iterations (`LOOP_LIMIT`), and the lane all
        // its back edges (`LOOP_TOTAL_LIMIT`).
        self.count_loops(&order);
        self.cur = self.s.entry;
        if let Some(why) = self.emu_refused {
            return Err(unsupported(0, why));
        }
        // An emulated wave runs a whole-wave section's steps as they are.
        if self.emu.is_none() {
            self.find_sections();
        }
        self.inputs()?;
        if let Some(mesh) = self.s.mesh {
            self.mesh_prologue(mesh).map_err(|w| unsupported(0, w))?;
            self.block[order[0]] = Some(self.cur);
        }
        for &b in &order {
            self.cur = self.block[b].expect("reachable");
            self.cur_ir = b;
            let preds: Vec<usize> = p.blocks[b]
                .preds
                .iter()
                .copied()
                .filter(|&q| self.block[q].is_some())
                .collect();
            self.exported[b] = !preds.is_empty() && preds.iter().all(|&q| self.exported_out(q));
            for (i, phi) in p.blocks[b].phis.iter().enumerate() {
                let val = match phi.reg {
                    Reg::S(c) if c == EXEC || c == EXEC + 1 => Val::Missing("a lane mask"),
                    _ if self.bad.contains_key(&phi.value) => Val::Missing(self.bad[&phi.value]),
                    Reg::Scc => {
                        let v = self.s.m.phi(self.cur, self.s.t.i1);
                        self.data_phis.push((v, b, i));
                        Val::Bit(v)
                    }
                    _ => {
                        let v = self.s.m.phi(self.cur, self.s.t.i32);
                        self.data_phis.push((v, b, i));
                        Val::I32(v)
                    }
                };
                self.vals.insert(phi.value, val);
            }
            let mut i = 0;
            self.mode16 = self.round16[b];
            while i < p.blocks[b].insts.len() {
                if let Some(&lanes) = self.x.st.discards.get(&(b, i)) {
                    // A pixel's discard: the lanes stay on as helpers.
                    let at = self.cur;
                    let offset = p.blocks[b].insts[i].machine.offset;
                    let kill = self
                        .boolean(lanes, at)
                        .map_err(|w| unsupported(offset, w))?;
                    self.s
                        .op(at, "discard", Attr::NoUnwind, self.s.t.void, 82, &[kill]);
                }
                self.inst(b, i)?;
                self.mode16 = round16_after(&p.blocks[b].insts[i], self.mode16);
                if let Some(end) = self.sections.get(&b).and_then(|s| s.get(&i)).map(|s| s.end) {
                    for j in i + 1..end {
                        self.section_inst(b, j, i);
                    }
                    i = end;
                    continue;
                }
                i += 1;
            }
            self.mask_ok = self.mask_data == MaskData::Console;
            self.tail[b] = Some(self.cur);
            self.exit(b)?;
        }
        // A vertex or mesh lane a loop limit stopped writes its outputs.
        self.end_stopped();
        self.fill_phis()?;
        // The limits sit past every dword the root parameters say they
        // hold; a read of the program's own that took one of their places
        // (a root descriptor's second dword where the container gives it
        // one) would be handed a limit as its value, or the host the
        // other way round. Such a program gets no limits: it is as
        // without the option, and the host does not run a dispatch of
        // its that would need them.
        if limits.iter().any(|u| self.user_reads.get(&u.index) != Some(u)) {
            return Err(Stop::Unlimited);
        }
        let user_data = self.user_reads.values().copied().collect();
        self.s.samplers = self.samplers.len() as u32;
        let root_cbvs = self.s.root_cbvs.iter().map(|r| r.0).collect();
        let root_srvs = self.s.root_srvs.iter().map(|r| r.0).collect();
        let root_uavs = self.s.root_uavs.iter().map(|r| r.0).collect();
        let memory = self.s.memory.is_some();
        let memory_writes = self.s.memory_dirty.is_some();
        let view_addresses = self.s.view_addresses.is_some();
        let cmp_samplers = self.s.cmp_samplers;
        #[cfg(test)]
        let flow = self.s.m.flow();
        Ok(Lowered {
            cmp_samplers,
            root_cbvs,
            root_srvs,
            root_uavs,
            memory,
            memory_writes,
            view_addresses,
            heap: self.heap[0],
            heap_writes: self.heap[1],
            view_marks: self.s.view_writes.is_some(),
            #[cfg(test)]
            nonuniform_handles: self.s.nonuniform_handles,
            #[cfg(test)]
            heap_handles: self.s.m.written_calls("dx.op.createHandleFromHeap") as u32,
            #[cfg(test)]
            wave_ops: self.s.flags & flags::WAVE_OPS != 0,
            #[cfg(test)]
            rovs: self.s.flags & flags::ROVS != 0,
            #[cfg(test)]
            uses_lane_index: self.s.m.has_function("dx.op.waveGetLaneIndex"),
            #[cfg(test)]
            has_half_wave_mask: self.s.m.has_i32_const(!31),
            #[cfg(test)]
            limit_stop: self.abort.and_then(|a| flow.at(a)),
            #[cfg(test)]
            flow,
            wave_size: self.s.wave_size,
            wave_needs: self.wave_needs,
            emulated: self.emu.is_some(),
            #[cfg(test)]
            meetings: self.emu.as_ref().map_or(0, |e| e.meetings),
            dxil: self.s.finish(),
            user_data,
            samplers: self.samplers,
        })
    }

    fn exported_out(&self, b: usize) -> bool {
        self.exported[b] || self.has_done_export(b)
    }

    fn has_done_export(&self, b: usize) -> bool {
        (0..self.x.p.blocks[b].insts.len())
            .any(|i| self.exports.get(&(b, i)).is_some_and(|e| e.done))
    }

    // ---- instructions ----

    fn inst(&mut self, b: usize, i: usize) -> Result<(), Stop> {
        let inst: &'a Inst = &self.x.p.blocks[b].insts[i];
        let offset = inst.machine.offset;
        let name = inst.name();
        self.mask_ok = match self.mask_data {
            MaskData::Console => true,
            MaskData::Host => {
                name.starts_with("s_bcnt1_i32_")
                    || name.starts_with("s_ff1_i32_")
                    || name.starts_with("v_mbcnt_")
                    || matches!(
                        name,
                        "s_cmp_eq_u32" | "s_cmp_lg_u32" | "s_cmp_eq_u64" | "s_cmp_lg_u64"
                    )
            }
            MaskData::None => name == "s_bcnt1_i32_b64",
        };
        let effect = matches!(inst.fields, Fields::Exp { .. })
            || matches!(inst.fields, Fields::Flat { .. }) && !name.starts_with("global_load_")
            || name.contains("store")
            || name.contains("atomic")
            || name == "s_barrier"
            || matches!(inst.fields, Fields::Ds { .. })
                && !(name.starts_with("ds_read")
                    || name.starts_with("ds_swizzle")
                    || name.contains("permute"));
        // An emulated wave's lanes meet where memory must be as the
        // console's wave left it (`emulated`, "Memory order").
        if self.emu_meets_before(b, i) {
            // Every lane must get there: an instruction's own guard is
            // closed by the time the next one starts, unless its lowering
            // stopped half way.
            if self.in_guarded > 0 {
                return Err(unsupported(offset, emulated::SOME_LANES));
            }
            self.emu_meet(self.cur);
        }
        let r = match &inst.fields {
            Fields::Sop { .. } => self.sop(inst),
            Fields::Sopp { .. } if name == "s_barrier" && self.no_barrier.contains(&b) => Ok(()),
            Fields::Sopp { .. } if name == "s_barrier" => {
                // Thread-group sync with a shared-memory fence, and a UAV
                // fence when the program writes UAVs (the waits before an
                // s_barrier make those writes visible on the console).
                let mode = if self.writes_uavs { 1 | 2 | 8 } else { 1 | 8 };
                let mode = self.s.m.i32(mode);
                let void = self.s.t.void;
                self.s
                    .op(self.cur, "barrier", Attr::NoDuplicate, void, 80, &[mode]);
                Ok(())
            }
            Fields::Sopp { .. } => Ok(()),
            Fields::Smem { .. } => self.smem(inst, (b, i)).and_then(|()| self.emu_shared(inst, (b, i))),
            Fields::Valu(_) if name.starts_with("v_interp_") => self.interp(inst, (b, i)),
            Fields::Valu(_) => self.valu(inst, b, i),
            Fields::Vintrp { .. } => self.interp(inst, (b, i)),
            Fields::Image { .. } => self.image(inst, b, i),
            Fields::Buffer { .. } => self.buffer(inst, b, i),
            Fields::Exp { .. } => self.export(b, i),
            Fields::Ds { .. } => self.ds(inst, b, i),
            Fields::Flat { .. } if name.starts_with("global_") => self.flat(inst, b, i),
            Fields::Flat { .. } => Err("flat memory"),
        };
        match r {
            Ok(()) => Ok(()),
            Err(why) if effect => Err(unsupported(offset, why)),
            Err(why) => {
                for &(_, v) in &inst.writes {
                    self.vals.insert(v, Val::Missing(why));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Limits, Lowered, Parts, lower, lower_limited, packed_half};
    use crate::native::{
        ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
    };
    use crate::{io, ir, resource, structure};

    pub(super) const S_ENDPGM: u32 = 0xBF81_0000;

    /// Whether a synthetic compute program translates with these input registers.
    pub(super) fn lowers(words: &[u32], rsrc2: u32) -> Result<(), crate::ShaderError> {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64: false,
            vgprs: 256,
            layout: vec![],
            used: vec![],
            io: StageIo::Cs(ComputeIo {
                rsrc2,
                threads: [1, 1, 1],
            }),
            code,
        };
        let p = ir::build(&native.code, false, 256).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        lower(&Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![],
            prims: vec![],
            mesh_stride: 0,
            view_marks: false,
        })
        .map(|l| l.flow.check().unwrap())
    }

    /// Lowers a compute program with the root parameters `layout` (in user
    /// SGPRs from s0 on) and the used resources `used`.
    pub(super) fn lower_with(
        words: &[u32],
        layout: Vec<RootParam>,
        used: Vec<UsedResource>,
    ) -> Result<Lowered, crate::ShaderError> {
        lower_wave(words, layout, used, false)
    }

    /// [`lower_with`] in wave64 if `wave64`.
    pub(super) fn lower_wave(
        words: &[u32],
        layout: Vec<RootParam>,
        used: Vec<UsedResource>,
        wave64: bool,
    ) -> Result<Lowered, crate::ShaderError> {
        lower_wave_limited(words, layout, used, wave64, None)
    }

    /// [`lower_wave`] with the loop limits `limits`, if any.
    fn lower_wave_limited(
        words: &[u32],
        layout: Vec<RootParam>,
        used: Vec<UsedResource>,
        wave64: bool,
        limits: Option<Limits>,
    ) -> Result<Lowered, crate::ShaderError> {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let sgprs: u32 = layout.iter().map(|p| p.sgprs).sum();
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64,
            vgprs: 256,
            layout,
            used,
            io: StageIo::Cs(ComputeIo {
                // The layout's user SGPRs (at least two), 512 bytes of LDS.
                rsrc2: 1 << 15 | sgprs.max(2) << 1,
                threads: [1, 1, 1],
            }),
            code,
        };
        let p = ir::build(&native.code, wave64, 256).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
        let io = io::raise(&p, &native).unwrap();
        let parts = Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![],
            prims: vec![],
            mesh_stride: 0,
            view_marks: false,
        };
        let lowered = match limits {
            Some(limits) => lower_limited(&parts, limits, false),
            None => lower(&parts),
        }?;
        // Every program these tests lower is a well-formed function.
        lowered.flow.check().unwrap();
        Ok(lowered)
    }

    /// Root parameter `slot` in user SGPRs from `user_data` on.
    pub(super) fn root(root: RootType, user_data: u8, slot: u8) -> RootParam {
        RootParam {
            root,
            table_offset_dwords: 0,
            sgprs: if root == RootType::Descriptor { 2 } else { 1 },
            base_register: 0,
            user_data,
            slot,
        }
    }

    /// Lowers a one-thread compute program that copies four root constants
    /// (s0..s3) into v1..v4, runs `body`, and stores v0 to a raw buffer from a
    /// descriptor table, so the vector instructions in `body` have an effect.
    pub(super) fn lower_valu(body: &[u32]) -> Result<Lowered, crate::ShaderError> {
        lower_valu_limited(body, None)
    }

    /// [`lower_valu`] with the loop limits `limits`, if any.
    pub(super) fn lower_valu_limited(
        body: &[u32],
        limits: Option<Limits>,
    ) -> Result<Lowered, crate::ShaderError> {
        let mut words = vec![
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202, // s_load_dwordx4 s[8:11], s[4:5], 0
            0xFA00_0000,
        ];
        // v_mov_b32 v1..v4, s0..s3
        words.extend((0..4u32).map(|i| 0x7E00_0200 | (i + 1) << 17 | i));
        words.extend_from_slice(body);
        words.extend_from_slice(&[
            0xE070_0000, // buffer_store_dword v0, off, s[8:11], 0
            0x8002_0000,
            S_ENDPGM,
        ]);
        let constants = RootParam {
            sgprs: 4,
            ..root(RootType::Constants, 0, 0)
        };
        let buffer = UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 1,
        };
        let layout = vec![constants, root(RootType::Table, 4, 1)];
        lower_wave_limited(&words, layout, vec![buffer], false, limits)
    }

    #[test]
    fn packed_halves_fill_parameters_first_fit() {
        // 16-bit rows as a title's signature has them: row 0 holds two
        // elements (x y and z w), row 3 is 32-bit, row 4 has one channel at w.
        let el = |row: u32, col: u8, cols: u8, half: bool| super::SigElement {
            name: "TEXCOORD".into(),
            index: row,
            kind: 0,
            comp: 9,
            interp: 0,
            row: Some(row),
            col,
            cols,
            usage: 0,
            half,
        };
        let sig = [
            el(0, 0, 2, true),
            el(0, 2, 2, true),
            el(1, 0, 4, true),
            el(2, 0, 3, true),
            el(3, 0, 4, false),
            el(4, 0, 3, true),
            el(4, 3, 1, true),
            el(5, 0, 3, true),
        ];
        // Each half as row and channel ("-": padding or past the end).
        let param = |a| {
            (0..8)
                .map(|h| match packed_half(&sig, a, h) {
                    Some((r, c)) => format!("{r}{}", ["x", "y", "z", "w"][usize::from(c)]),
                    None => "-".into(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        // By first channel, then row: row 0's x y, all of row 1, then (last,
        // as it starts at z) row 0's z w.
        assert_eq!(param(0), "0x 0y 1x 1y 1z 1w 0z 0w");
        // Three channels take two dwords; row 4's w fills the first
        // parameter with room, after row 5.
        assert_eq!(param(2), "2x 2y 2z - 4x 4y 4z -");
        assert_eq!(param(5), "5x 5y 5z - 4w - - -");
        // No parameter starts in rows 1, 3 or 4.
        for a in [1, 3, 4] {
            assert_eq!(param(a), "- - - - - - - -");
        }
    }
}
