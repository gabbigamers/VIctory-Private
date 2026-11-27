//! Synthetic traversal programs, assembled here by hand: what the
//! recogniser is tested on, and one software path for the validation probe.
//! **No title program was read to write them**; they are one way a
//! compiler could lay such a loop out, and nothing says a title's loops
//! look like them.
//!
//! Every [`Walk`] but two is one program with small differences: a compute
//! program of [`GROUP_THREADS`] threads a group, 32-lane waves, in which
//! thread `n` of the dispatch reads ray record `n`, walks one tree of
//! console-format box and triangle nodes with a stack of its own in
//! group-shared memory, and writes hit record `n`. A thread whose `n` is
//! not below the ray count does nothing.
//!
//! * **Bindings** (the title-side root signature the program is written
//!   for; [`SoftwareWalk::shader`] says what the host binds): root
//!   parameter [`ROOT_TREE`], a root SRV at the tree's first byte, 256-byte
//!   aligned, whose node 0 is the root box node; root parameter
//!   [`ROOT_TABLE`], a descriptor table whose descriptor 0 is the ray
//!   buffer (a raw SRV) and descriptor 1 the hit buffer (a raw UAV); root
//!   parameter [`ROOT_COUNT`], one root constant, the ray count.
//! * **Records** are `emit::rayquery`'s, so one set of rays serves both: a
//!   ray is [`RAY_BYTES`] (origin, tmin, direction, tmax as floats; the
//!   mask and flag words are not read), a hit [`HIT_BYTES`] (t, u, v, kind,
//!   instance index, instance id, geometry index, primitive index; a miss
//!   is eight zero words).
//! * **The walk.** The stack holds node pointers, [`STACK_ENTRIES`] to a
//!   thread and **unchecked**: the tree must need no more (`victory-rt`
//!   refuses to build one that does). Each step pops a pointer and runs
//!   the node instruction on it with the ray's origin, direction and
//!   inverse direction (`v_rcp_f32` of each component) and the extent. A
//!   box node's valid children are pushed last to first. A triangle gives
//!   `(tn, d, i, j)`; with `r = 1 / d` (`v_rcp_f32`, which the translation
//!   divides exactly) the candidate is `t = tn * r`, `u = i * r`,
//!   `v = j * r`, a front face when `d > 0`, and it is accepted when
//!   `tmin < t` and `t` passes the walk's extent test.
//!
//! Loop limits apply as to any translated loop: a thread that would go
//! round more than `emit::loop_limit` times is stopped and writes nothing.

use super::asm::{Asm, EXEC, Src, VCC};
use crate::dxbc::Container;
use crate::emit::{self, HostShader};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{ShaderError, io, ir, resource, structure};

/// Threads in a group (x; y and z are 1).
pub const GROUP_THREADS: u32 = 64;
/// Node pointers a thread's stack holds.
pub const STACK_ENTRIES: u32 = 64;
/// Bytes of a ray record and of a hit record (`emit::rayquery`'s).
pub const RAY_BYTES: u32 = emit::rayquery::RAY_BYTES;
pub const HIT_BYTES: u32 = emit::rayquery::HIT_BYTES;
/// The title-side root parameters (module notes).
pub const ROOT_TREE: u8 = 0;
pub const ROOT_TABLE: u8 = 1;
pub const ROOT_COUNT: u8 = 2;
/// The node pointer the walk starts from: the 32-bit box node in slot 0.
pub const ROOT_POINTER: u32 = 5;
/// The iterations [`Walk::Capped`] allows a thread.
pub const CAP: u32 = 48;
/// Where [`Walk::RawBase`] reads its tree: a 256-byte aligned address
/// below 2^40, written into the program.
pub const RAW_TREE: u64 = 0x12_3450_0000;

/// A synthetic program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Walk {
    /// Keeps the nearest accepted hit: its distance, both weights and its
    /// facing; `t < nearest so far`, which is also the extent of the box
    /// tests that follow.
    Nearest,
    /// [`Walk::Nearest`] with `t <= nearest so far`.
    NearestInclusive,
    /// [`Walk::Nearest`] with "not `t >= nearest so far`" (`v_cmp_nge_f32`),
    /// which a NaN passes.
    NearestUnordered,
    /// Yes or no: `t < tmax`, the hit record says only that there is a
    /// hit, and the walk ends at the first one accepted.
    Any,
    /// [`Walk::Nearest`] that also keeps the hit's node pointer and, after
    /// the walk, loads the geometry and primitive index from that node
    /// (words 12 and 13, `victory-rt`'s own layout). **The software path
    /// of the probe**, bottom level only ([`software`]).
    NearestIds,
    /// [`Walk::Nearest`] that keeps the node pointer and writes it to the
    /// hit record (as the primitive index) without loading through it.
    NearestNode,
    /// Keeps the last candidate inside `tmin < t < tmax`, whatever came
    /// before: the result follows the order of the visits.
    LastHit,
    /// [`Walk::Nearest`] that stops a thread after [`CAP`] steps.
    Capped,
    /// [`Walk::NearestNode`] where a candidate at exactly the nearest
    /// distance so far wins if its node pointer is the smaller.
    TieByKey,
    /// [`Walk::Nearest`] with the distances compared as integers
    /// (`v_cmp_lt_i32` on the floats' bits).
    BitsCompare,
    /// [`Walk::LastHit`] with no compare of `t` against any extent.
    NoExtent,
    /// [`Walk::Nearest`] with the inverse direction read from the ray
    /// record's last four words instead of computed.
    LoadedInverse,
    /// [`Walk::Nearest`] whose origin moves at every box node.
    MovingRay,
    /// [`Walk::Nearest`] that also counts the triangles it tests and
    /// writes the count to the hit record.
    Counting,
    /// A walk the wave makes as one: a node pointer in a scalar register,
    /// the lanes that hit kept as a mask that is saved as two numbers and
    /// comes back into EXEC. 64-lane waves, a group of one wave.
    WaveWalk,
    /// A loop with no node instruction: each thread counts to its ray
    /// count.
    NoNode,
    /// [`Walk::Nearest`] that runs the node instruction twice a round, the
    /// second result unused.
    TwoTests,
    /// [`Walk::Nearest`] whose node instruction says its directions are
    /// packed as 16-bit floats. **Not translated** (the lowering refuses
    /// the form), so it is no software path; it is here for the recogniser.
    PackedDirections,
    /// [`Walk::Nearest`] that marks, after the walk, the threads that are
    /// lanes 0 and 2 of their wave (a select by a constant lane mask): a
    /// walk with a pointer for each lane in a program that depends on
    /// which lane a thread is. **Not translated**: the per-lane lowering
    /// refuses such a select.
    LaneNumber,
    /// [`Walk::Nearest`] made twice over, the second walk starting from
    /// what the first found: a loop around the walk's loop.
    TwoPasses,
    /// [`Walk::Nearest`] whose descriptor is built around a constant
    /// address ([`RAW_TREE`]) instead of the root parameter's: the nodes
    /// are read as title memory at a raw address.
    RawBase,
    /// [`Walk::Nearest`] that never pushes a box node's fourth result.
    SkipsChild,
    /// [`Walk::Nearest`] that stores each candidate's distance to its hit
    /// record inside the loop (the record is written whole after it).
    StoresInLoop,
    /// [`Walk::Nearest`] that loads the minimum again for each candidate.
    LoadsInLoop,
    /// [`Walk::Nearest`] that ends at the first candidate accepted, with
    /// everything it keeps.
    EarlyExit,
    /// [`Walk::Nearest`] that accepts a candidate past the minimum *or*
    /// nearer than the nearest so far.
    AcceptEither,
}

impl Walk {
    pub const ALL: [Walk; 26] = [
        Walk::Nearest,
        Walk::NearestInclusive,
        Walk::NearestUnordered,
        Walk::Any,
        Walk::NearestIds,
        Walk::NearestNode,
        Walk::LastHit,
        Walk::Capped,
        Walk::TieByKey,
        Walk::BitsCompare,
        Walk::NoExtent,
        Walk::LoadedInverse,
        Walk::MovingRay,
        Walk::Counting,
        Walk::WaveWalk,
        Walk::NoNode,
        Walk::TwoTests,
        Walk::PackedDirections,
        Walk::LaneNumber,
        Walk::TwoPasses,
        Walk::RawBase,
        Walk::SkipsChild,
        Walk::StoresInLoop,
        Walk::LoadsInLoop,
        Walk::EarlyExit,
        Walk::AcceptEither,
    ];

    /// Whether the translation makes a container of it: the others have no
    /// per-lane form or a node instruction the lowering refuses.
    pub fn translates(self) -> bool {
        !matches!(
            self,
            Walk::WaveWalk | Walk::PackedDirections | Walk::LaneNumber
        )
    }

    /// Whether it walks a tree with a per-thread stack and writes a hit
    /// record: every program but the wave's walk and the count.
    pub fn stack(self) -> bool {
        !matches!(self, Walk::WaveWalk | Walk::NoNode)
    }

    /// The program's GFX10 words.
    pub fn words(self) -> Vec<u32> {
        match self {
            Walk::WaveWalk => wave_walk(),
            Walk::NoNode => no_node(),
            _ => stack_walk(self, None),
        }
    }

    /// The program as the console's container would describe it.
    pub fn native(self) -> NativeProgram {
        self.described(&self.words(), [GROUP_THREADS, 1, 1], 1)
    }

    /// `words` as a program of this walk's kind, its group `threads` wide,
    /// with the thread's id in the group in the first `ids` VGPRs (x, y, z).
    fn described(self, words: &[u32], threads: [u32; 3], ids: u32) -> NativeProgram {
        let wave = self == Walk::WaveWalk;
        let stack = self.stack();
        let table = |kind, table_offset_dwords| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 2,
            slot: ROOT_TABLE,
        };
        let root = |root, sgprs, user_data, slot| RootParam {
            root,
            table_offset_dwords: 0,
            sgprs,
            base_register: 0,
            user_data,
            slot,
        };
        NativeProgram {
            stage: HwStage::Cs,
            wave64: wave,
            vgprs: 48,
            layout: vec![
                root(RootType::Descriptor, 2, 0, ROOT_TREE),
                root(RootType::Table, 1, 2, ROOT_TABLE),
                root(RootType::Constants, 1, 3, ROOT_COUNT),
            ],
            used: vec![
                UsedResource {
                    kind: ResourceType::Srv,
                    root: RootType::Descriptor,
                    table_offset_dwords: 0,
                    lower_bound: 0,
                    stride: 0,
                    dwords: 4,
                    user_data: 0,
                    slot: ROOT_TREE,
                },
                table(ResourceType::Srv, 0),
                table(ResourceType::Uav, 8),
            ],
            io: StageIo::Cs(ComputeIo {
                // Four user SGPRs, the group's id along x after them, the
                // thread's id components asked for, and for a stack walk
                // the threads' stacks in group-shared memory (512-byte
                // granules).
                rsrc2: 4 << 1
                    | 1 << 7
                    | (ids - 1) << 11
                    | if stack {
                        (GROUP_THREADS * STACK_ENTRIES * 4 / 512) << 15
                    } else {
                        0
                    },
                threads,
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }
    }
}

/// A stack walk changed in one place so that it breaks **one** condition
/// the recogniser checks: the programs its refusals are tested on beyond
/// the one of [`Walk`] each reason has. A description of any of them would
/// be false (the comment of each says how; [`Variation::DescriptorInLoop`]
/// is the one whose refusal is caution alone), so each must be refused,
/// and for the reason of the check it breaks (the table is in the
/// recogniser's tests). None is run; several read memory nothing stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variation {
    /// The root is pushed only by the lanes with `tmin < tmax` (EXEC
    /// narrowed around the store, no branch): the others enter the loop
    /// and pop what nothing stored.
    GuardedRoot,
    /// The same with a branch around the store: it is not on the one path
    /// into the loop.
    BranchedRoot,
    /// The root is stored one entry above the thread's base, where the
    /// first pop does not read.
    RootAboveBase,
    /// The stack pointer starts at two entries with one pushed.
    DeepStart,
    /// A group of 8 by 8 threads: the thread's id along x is no longer the
    /// thread, and eight threads share each stack.
    Grid,
    /// The stack's base is made from the thread's id along z (the program
    /// is given all three components): one stack for the whole group.
    OtherAxis,
    /// The node tested is the one after the popped pointer's.
    NextNode,
    /// [`Walk::TwoPasses`] keeping the node pointer, the second pass
    /// loading through the pointer the first kept (the load is outside the
    /// walk's loop and reaches the pointer through the outer loop's join).
    LoadAtPass,
    /// [`Walk::Any`] that empties its stack at the first triangle past the
    /// minimum, accepted or not: no first-hit walk.
    ClearsOnMinimum,
    /// One weight is replaced for every front face, accepted or not.
    WeightOnFront,
    /// The last pointer popped is written to the hit record.
    LastPointer,
    /// The second half of the hit record is stored only by the lanes of
    /// the last candidate accepted: a lane mask of the loop decides a
    /// store after it.
    MaskedStore,
    /// Six more compares of the pointer, each picking a number the hit
    /// record gets: more compares than the recogniser enumerates.
    ManyCompares,
    /// The minimum a candidate must pass is the weight kept so far.
    MinimumMoves,
    /// [`Walk::TieByKey`] whose tie is settled against the kept facing.
    KeyIsFacing,
    /// The extent the node test and the compare get is replaced by a
    /// weight on acceptance (and the kept weight by the distance).
    ExtentIsWeight,
    /// Each push is guarded by one result word and stores its neighbour.
    WrongChild,
    /// The stack pointer is raised before each push, not after: an entry
    /// is skipped and the pop reads below what was pushed.
    PushesAbove,
    /// The last push does not raise the stack pointer.
    PushWithoutRaise,
    /// An accepted candidate sets the stack pointer to two entries.
    TriangleRaises,
    /// The loop goes round while the stack pointer was not 0 *before* the
    /// round's pushes.
    TestsEarlySp,
    /// The facing is taken from `0 <= d`, not `0 < d`.
    FacingInclusive,
    /// A cap tested at the top of the loop: a second way out.
    SecondExit,
    /// A word of the structure's descriptor is written again every round,
    /// with the value it had: the walk is [`Walk::Nearest`]'s and so is
    /// its container. Refused all the same: the recogniser does not look
    /// at what a round writes to the descriptor, only that none does.
    DescriptorInLoop,
    /// [`Walk::NearestNode`] that also loads the primitive index through
    /// the kept pointer, the address made with an add that has a carry
    /// out (an instruction whose result is known as nothing but itself).
    LoadWithCarry,
}

impl Variation {
    pub const ALL: [Variation; 25] = [
        Variation::GuardedRoot,
        Variation::BranchedRoot,
        Variation::RootAboveBase,
        Variation::DeepStart,
        Variation::Grid,
        Variation::OtherAxis,
        Variation::NextNode,
        Variation::LoadAtPass,
        Variation::ClearsOnMinimum,
        Variation::WeightOnFront,
        Variation::LastPointer,
        Variation::MaskedStore,
        Variation::ManyCompares,
        Variation::MinimumMoves,
        Variation::KeyIsFacing,
        Variation::ExtentIsWeight,
        Variation::WrongChild,
        Variation::PushesAbove,
        Variation::PushWithoutRaise,
        Variation::TriangleRaises,
        Variation::TestsEarlySp,
        Variation::FacingInclusive,
        Variation::SecondExit,
        Variation::DescriptorInLoop,
        Variation::LoadWithCarry,
    ];

    /// The walk it is a change of.
    pub fn of(self) -> Walk {
        match self {
            Variation::LoadAtPass => Walk::TwoPasses,
            Variation::ClearsOnMinimum => Walk::Any,
            Variation::KeyIsFacing => Walk::TieByKey,
            Variation::LoadWithCarry => Walk::NearestNode,
            _ => Walk::Nearest,
        }
    }

    /// The program as the console's container would describe it.
    pub fn native(self) -> NativeProgram {
        let threads = match self {
            Variation::Grid => [8, 8, 1],
            _ => [GROUP_THREADS, 1, 1],
        };
        let ids = if self == Variation::OtherAxis { 3 } else { 1 };
        let walk = self.of();
        walk.described(&stack_walk(walk, Some(self)), threads, ids)
    }
}

// Registers of the stack walk. Scalar: s[0:1] the tree's address, s2 the
// table, s3 the ray count, s4 the group's id.
const S_TABLE: u16 = 6;
const S_RAYS: u16 = 8;
const S_HITS: u16 = 12;
const S_FIRST: u16 = 16;
/// Lane masks: the lanes with a ray; the lanes in the loop; the triangle
/// lanes; scratch; the lanes of a box node.
const M_RAYS: u16 = 20;
const M_LOOP: u16 = 21;
const M_TRI: u16 = 19;
const M_A: u16 = 22;
const M_BOX: u16 = 23;
const M_B: u16 = 18;
const S_TREE: u16 = 24;
// Vector: v0 the thread's id in its group.
const V_N: u8 = 1;
const V_RAY_AT: u8 = 2;
const V_HIT_AT: u8 = 3;
const V_ORIGIN: u8 = 4;
const V_TMIN: u8 = 7;
const V_DIR: u8 = 8;
const V_TMAX: u8 = 11;
const V_INV: u8 = 12;
const V_BASE: u8 = 15;
const V_SP: u8 = 16;
const V_TMP: u8 = 17;
const V_BEST: u8 = 18;
const V_U: u8 = 19;
const V_V: u8 = 20;
const V_KIND: u8 = 21;
const V_NODE: u8 = 22;
const V_AT: u8 = 23;
const V_PTR: u8 = 24;
const V_TYPE: u8 = 25;
const V_RCP: u8 = 26;
const V_T: u8 = 27;
const V_RESULT: u8 = 28;
const V_CU: u8 = 32;
const V_CV: u8 = 33;
const V_FACE: u8 = 34;
const V_COUNT: u8 = 35;
const V_OUT: u8 = 36;
const V_OFF: u8 = 44;
/// [`Variation::OtherAxis`]: the thread's id along z, kept from v2 before
/// the walk uses that register.
const V_AXIS: u8 = 46;

/// The BVH descriptor around the root SRV in `s[0:1]`, into `s[at..at + 4]`:
/// sorted boxes, no growth, no node bound, barycentrics (what the
/// translation emulates).
fn tree_descriptor(a: &mut Asm, at: u16, raw: bool) {
    if raw {
        a.sop1("s_mov_b32", at, Src::Lit((RAW_TREE >> 8) as u32));
        a.sop1(
            "s_mov_b32",
            at + 1,
            Src::Lit(0x8000_0000 | (RAW_TREE >> 40) as u32),
        );
    } else {
        a.sop2("s_lshr_b64", at, Src::S(0), Src::K(8));
        a.sop2("s_or_b32", at + 1, Src::S(at + 1), Src::Lit(0x8000_0000));
    }
    a.sop1("s_mov_b32", at + 2, Src::K(-1));
    a.sop1(
        "s_mov_b32",
        at + 3,
        Src::Lit(8 << 28 | 1 << 27 | 1 << 24 | 0x3FF),
    );
}

fn stack_walk(walk: Walk, vary: Option<Variation>) -> Vec<u32> {
    use Variation as V;
    use Walk::*;
    let varied = |v: Variation| vary == Some(v);
    let nearest = !matches!(walk, Any | LastHit | NoExtent);
    let node = matches!(walk, NearestIds | NearestNode | TieByKey) || varied(V::LoadAtPass);
    let mut a = Asm::new();
    if varied(V::OtherAxis) {
        a.vop1("v_mov_b32", V_AXIS, Src::V(2));
    }
    // The two descriptors of the table.
    a.sop1("s_mov_b32", S_TABLE, Src::S(2));
    a.sop1("s_mov_b32", S_TABLE + 1, Src::K(0));
    a.s_load_x4(S_RAYS, S_TABLE, 0);
    a.s_load_x4(S_HITS, S_TABLE, 32);
    // n, and the threads with a ray.
    a.sop2("s_lshl_b32", S_FIRST, Src::S(4), Src::K(6));
    a.vop2("v_add_nc_u32", V_N, Src::S(S_FIRST), 0);
    a.vopc("v_cmp_gt_u32", Src::S(3), V_N);
    a.sop1("s_and_saveexec_b32", M_RAYS, Src::S(VCC));
    a.branch("s_cbranch_execz", "end");
    // The ray.
    a.vop3(
        "v_mul_lo_u32",
        V_RAY_AT,
        Src::V(V_N),
        Src::K(RAY_BYTES as i8),
    );
    a.mubuf("buffer_load_dwordx4", V_ORIGIN, V_RAY_AT, S_RAYS, 0);
    a.mubuf("buffer_load_dwordx4", V_DIR, V_RAY_AT, S_RAYS, 16);
    if walk == LoadedInverse {
        a.mubuf("buffer_load_dwordx4", V_INV, V_RAY_AT, S_RAYS, 32);
    } else {
        for k in 0..3 {
            a.vop1("v_rcp_f32", V_INV + k, Src::V(V_DIR + k));
        }
    }
    tree_descriptor(&mut a, S_TREE, walk == RawBase);
    // Nothing hit; the thread's stack with the root on it.
    a.vop2(
        "v_lshlrev_b32",
        V_BASE,
        Src::K(8),
        if varied(V::OtherAxis) { V_AXIS } else { 0 },
    );
    a.vop1("v_mov_b32", V_BEST, Src::V(V_TMAX));
    for r in [V_U, V_V, V_KIND, V_NODE, V_COUNT] {
        a.vop1("v_mov_b32", r, Src::K(0));
    }
    if walk == StoresInLoop {
        a.vop2("v_lshlrev_b32", V_HIT_AT, Src::K(5), V_N);
    }
    a.label("pass");
    if varied(V::LoadAtPass) {
        // The primitive index of the node the pass before kept.
        a.vop2("v_and_b32", V_OFF, Src::K(-8), V_NODE);
        a.vop2("v_lshlrev_b32", V_OFF, Src::K(3), V_OFF);
        a.global_load(V_OFF + 1, V_OFF, Some(0), 52);
    }
    a.vop1("v_mov_b32", V_TMP, Src::K(ROOT_POINTER as i8));
    match vary {
        Some(V::GuardedRoot | V::BranchedRoot) => {
            a.vopc("v_cmp_lt_f32", Src::V(V_TMIN), V_TMAX);
            a.sop1("s_and_saveexec_b32", M_A, Src::S(VCC));
            if varied(V::BranchedRoot) {
                a.branch("s_cbranch_execz", "pushed");
            }
            a.ds("ds_write_b32", 0, V_BASE, V_TMP, 0);
            a.label("pushed");
            a.sop1("s_mov_b32", EXEC, Src::S(M_A));
        }
        Some(V::RootAboveBase) => {
            a.vop2("v_add_nc_u32", V_AT, Src::K(4), V_BASE);
            a.ds("ds_write_b32", 0, V_AT, V_TMP, 0);
        }
        _ => a.ds("ds_write_b32", 0, V_BASE, V_TMP, 0),
    }
    a.vop1(
        "v_mov_b32",
        V_SP,
        Src::K(if varied(V::DeepStart) { 8 } else { 4 }),
    );

    a.label("loop");
    if varied(V::SecondExit) {
        a.vop2("v_add_nc_u32", V_COUNT, Src::K(1), V_COUNT);
        a.vopc("v_cmpx_gt_u32", Src::K(CAP as i8), V_COUNT);
        a.branch("s_cbranch_execz", "out");
    }
    if varied(V::DescriptorInLoop) {
        a.sop1("s_mov_b32", S_TREE + 2, Src::K(-1));
    }
    a.vop2("v_add_nc_u32", V_SP, Src::K(-4), V_SP);
    if varied(V::TestsEarlySp) {
        a.vop1("v_mov_b32", V_TMP, Src::V(V_SP));
    }
    a.vop2("v_add_nc_u32", V_AT, Src::V(V_BASE), V_SP);
    a.ds("ds_read_b32", 0, V_AT, 0, V_PTR);
    if varied(V::NextNode) {
        a.vop2("v_add_nc_u32", V_PTR, Src::K(8), V_PTR);
    }
    let extent = if nearest { V_BEST } else { V_TMAX };
    let ray = [
        V_PTR,
        extent,
        V_ORIGIN,
        V_ORIGIN + 1,
        V_ORIGIN + 2,
        V_DIR,
        V_DIR + 1,
        V_DIR + 2,
        V_INV,
        V_INV + 1,
        V_INV + 2,
    ];
    match walk {
        PackedDirections => a.bvh_packed(V_RESULT, ray, S_TREE),
        _ => a.bvh(V_RESULT, ray, S_TREE),
    }
    if walk == TwoTests {
        a.bvh(V_OFF, ray, S_TREE);
    }
    if walk == Capped {
        a.vop2("v_add_nc_u32", V_COUNT, Src::K(1), V_COUNT);
    }
    a.vop2("v_and_b32", V_TYPE, Src::K(7), V_PTR);
    a.vopc_to("v_cmp_gt_u32", M_TRI, Src::K(4), Src::V(V_TYPE));
    a.sop1("s_and_saveexec_b32", M_LOOP, Src::S(M_TRI));
    a.branch("s_cbranch_execz", "boxes");

    // A triangle: the candidate, whether it is accepted, what is kept.
    a.vop1("v_rcp_f32", V_RCP, Src::V(V_RESULT + 1));
    a.vop2("v_mul_f32", V_T, Src::V(V_RESULT), V_RCP);
    a.vop2("v_mul_f32", V_CU, Src::V(V_RESULT + 2), V_RCP);
    a.vop2("v_mul_f32", V_CV, Src::V(V_RESULT + 3), V_RCP);
    if walk == Counting {
        a.vop2("v_add_nc_u32", V_COUNT, Src::K(1), V_COUNT);
    }
    if walk == StoresInLoop {
        a.mubuf("buffer_store_dword", V_T, V_HIT_AT, S_HITS, 0);
    }
    if walk == LoadsInLoop {
        a.mubuf("buffer_load_dword", V_TMP, V_RAY_AT, S_RAYS, 12);
        a.vopc("v_cmp_lt_f32", Src::V(V_TMP), V_T);
    } else if varied(V::MinimumMoves) {
        a.vopc("v_cmp_lt_f32", Src::V(V_U), V_T);
    } else {
        a.vopc("v_cmp_lt_f32", Src::V(V_TMIN), V_T);
    }
    match walk {
        NoExtent => a.sop1("s_mov_b32", M_A, Src::S(VCC)),
        _ => {
            let test = match walk {
                NearestInclusive => "v_cmp_le_f32",
                NearestUnordered => "v_cmp_nge_f32",
                BitsCompare => "v_cmp_lt_i32",
                _ => "v_cmp_lt_f32",
            };
            a.vopc_to(test, M_A, Src::V(V_T), Src::V(extent));
            if walk == TieByKey {
                // Or the same distance and the smaller pointer.
                a.vopc_to("v_cmp_eq_f32", M_B, Src::V(V_T), Src::V(V_BEST));
                let key = if varied(V::KeyIsFacing) {
                    V_KIND
                } else {
                    V_NODE
                };
                a.vopc_to("v_cmp_lt_u32", M_BOX, Src::V(V_PTR), Src::V(key));
                a.sop2("s_and_b32", M_B, Src::S(M_B), Src::S(M_BOX));
                a.sop2("s_or_b32", M_A, Src::S(M_A), Src::S(M_B));
            }
            let both = if walk == AcceptEither {
                "s_or_b32"
            } else {
                "s_and_b32"
            };
            a.sop2(both, M_A, Src::S(VCC), Src::S(M_A));
        }
    }
    if walk == Any {
        a.cndmask(V_KIND, Src::V(V_KIND), Src::K(1), M_A);
        // The first hit ends the walk: the stack is emptied. (VCC still
        // holds the minimum's compare.)
        let ends = if varied(V::ClearsOnMinimum) { VCC } else { M_A };
        a.cndmask(V_SP, Src::V(V_SP), Src::K(0), ends);
    } else {
        let front = if varied(V::FacingInclusive) {
            "v_cmp_le_f32"
        } else {
            "v_cmp_lt_f32"
        };
        a.vopc(front, Src::K(0), V_RESULT + 1);
        a.cndmask(V_FACE, Src::K(2), Src::K(1), VCC);
        let (best, u) = if varied(V::ExtentIsWeight) {
            (V_CU, V_T)
        } else {
            (V_T, V_CU)
        };
        a.cndmask(V_BEST, Src::V(V_BEST), Src::V(best), M_A);
        let weighed = if varied(V::WeightOnFront) { VCC } else { M_A };
        a.cndmask(V_U, Src::V(V_U), Src::V(u), weighed);
        a.cndmask(V_V, Src::V(V_V), Src::V(V_CV), M_A);
        a.cndmask(V_KIND, Src::V(V_KIND), Src::V(V_FACE), M_A);
        if node {
            a.cndmask(V_NODE, Src::V(V_NODE), Src::V(V_PTR), M_A);
        }
        if varied(V::TriangleRaises) {
            a.cndmask(V_SP, Src::V(V_SP), Src::K(8), M_A);
        }
        if varied(V::ManyCompares) {
            for k in 0..6 {
                a.vopc_to("v_cmp_lt_u32", M_B, Src::V(V_PTR), Src::K(8 + k));
                a.cndmask(V_COUNT, Src::V(V_COUNT), Src::K(k), M_B);
            }
        }
    }

    // A box node: its valid children, last to first.
    a.label("boxes");
    a.sop2("s_andn2_b32", EXEC, Src::S(M_LOOP), Src::S(M_TRI));
    a.branch("s_cbranch_execz", "next");
    for k in (0..if walk == SkipsChild { 3 } else { 4 }).rev() {
        a.vopc("v_cmp_ne_u32", Src::K(-1), V_RESULT + k);
        a.sop1("s_and_saveexec_b32", M_BOX, Src::S(VCC));
        if varied(V::PushesAbove) {
            a.vop2("v_add_nc_u32", V_SP, Src::K(4), V_SP);
        }
        a.vop2("v_add_nc_u32", V_AT, Src::V(V_BASE), V_SP);
        let pushed = if varied(V::WrongChild) { k ^ 1 } else { k };
        a.ds("ds_write_b32", 0, V_AT, V_RESULT + pushed, 0);
        if !(varied(V::PushesAbove) || varied(V::PushWithoutRaise) && k == 0) {
            a.vop2("v_add_nc_u32", V_SP, Src::K(4), V_SP);
        }
        a.sop1("s_mov_b32", EXEC, Src::S(M_BOX));
    }
    if walk == MovingRay {
        a.vop2("v_add_f32", V_ORIGIN, Src::V(V_ORIGIN), V_DIR);
    }

    // Round again while the stack holds a node.
    a.label("next");
    a.sop1("s_mov_b32", EXEC, Src::S(M_LOOP));
    if matches!(walk, Capped | EarlyExit) {
        a.vopc("v_cmp_ne_u32", Src::K(0), V_SP);
        match walk {
            Capped => a.vopc_to("v_cmp_gt_u32", M_A, Src::K(CAP as i8), Src::V(V_COUNT)),
            _ => a.vopc_to("v_cmp_eq_u32", M_A, Src::K(0), Src::V(V_KIND)),
        }
        a.sop2("s_and_b32", M_A, Src::S(VCC), Src::S(M_A));
        a.sop2("s_and_b32", EXEC, Src::S(EXEC), Src::S(M_A));
    } else {
        let sp = if varied(V::TestsEarlySp) { V_TMP } else { V_SP };
        a.vopc("v_cmpx_ne_u32", Src::K(0), sp);
    }
    a.branch("s_cbranch_execnz", "loop");
    a.label("out");
    a.sop1("s_mov_b32", EXEC, Src::S(M_RAYS));
    if walk == TwoPasses {
        a.vop2("v_add_nc_u32", V_COUNT, Src::K(1), V_COUNT);
        a.vopc("v_cmpx_gt_u32", Src::K(2), V_COUNT);
        a.branch("s_cbranch_execnz", "pass");
        a.sop1("s_mov_b32", EXEC, Src::S(M_RAYS));
    }
    if walk == LaneNumber {
        // The mask is the constant 5: lanes 0 and 2.
        a.cndmask(V_COUNT, Src::V(V_COUNT), Src::K(1), 128 + 5);
    }

    // The hit record: zeros for a miss.
    for k in 4..8 {
        a.vop1("v_mov_b32", V_OUT + k, Src::K(0));
    }
    a.vopc("v_cmp_ne_u32", Src::K(0), V_KIND);
    if walk == Any {
        a.vop1("v_mov_b32", V_OUT, Src::K(0));
    } else {
        a.cndmask(V_OUT, Src::K(0), Src::V(V_BEST), VCC);
    }
    a.vop1("v_mov_b32", V_OUT + 1, Src::V(V_U));
    a.vop1("v_mov_b32", V_OUT + 2, Src::V(V_V));
    a.vop1("v_mov_b32", V_OUT + 3, Src::V(V_KIND));
    match walk {
        NearestIds => {
            // Words 12 and 13 of the node hit: the geometry index under
            // the opaque bit, the primitive index.
            a.sop1("s_and_saveexec_b32", M_A, Src::S(VCC));
            a.vop2("v_and_b32", V_OFF, Src::K(-8), V_NODE);
            a.vop2("v_lshlrev_b32", V_OFF, Src::K(3), V_OFF);
            a.global_load(V_OUT + 6, V_OFF, Some(0), 48);
            a.global_load(V_OUT + 7, V_OFF, Some(0), 52);
            a.vop2("v_and_b32", V_OUT + 6, Src::Lit(0x7FFF_FFFF), V_OUT + 6);
            a.sop1("s_mov_b32", EXEC, Src::S(M_A));
        }
        NearestNode | TieByKey => a.vop1("v_mov_b32", V_OUT + 7, Src::V(V_NODE)),
        Counting | LaneNumber => a.vop1("v_mov_b32", V_OUT + 7, Src::V(V_COUNT)),
        _ => {}
    }
    match vary {
        Some(V::LoadAtPass) => {
            a.vop1("v_mov_b32", V_OUT + 6, Src::V(V_OFF + 1));
            a.vop1("v_mov_b32", V_OUT + 7, Src::V(V_NODE));
        }
        Some(V::LoadWithCarry) => {
            // In every lane, so that the sum is not also a pick of what
            // the register held (a miss loads through pointer 0).
            a.vop2("v_and_b32", V_OFF, Src::K(-8), V_NODE);
            a.vop2("v_lshlrev_b32", V_OFF, Src::K(3), V_OFF);
            a.add_co(V_OFF, Src::V(V_OFF), Src::K(52));
            a.global_load(V_OUT + 6, V_OFF, Some(0), 0);
        }
        Some(V::LastPointer) => a.vop1("v_mov_b32", V_OUT + 5, Src::V(V_PTR)),
        Some(V::ManyCompares) => a.vop1("v_mov_b32", V_OUT + 5, Src::V(V_COUNT)),
        _ => {}
    }
    a.vop2("v_lshlrev_b32", V_HIT_AT, Src::K(5), V_N);
    a.mubuf("buffer_store_dwordx4", V_OUT, V_HIT_AT, S_HITS, 0);
    if varied(V::MaskedStore) {
        a.sop2("s_and_b32", EXEC, Src::S(EXEC), Src::S(M_A));
    }
    a.mubuf("buffer_store_dwordx4", V_OUT + 4, V_HIT_AT, S_HITS, 16);
    a.label("end");
    a.endpgm();
    a.finish()
}

/// [`Walk::WaveWalk`]: the wave tests nodes 5 down to 1 of the tree's
/// slots as triangles, one node for all lanes; the lanes that hit go on
/// with their own work (EXEC narrows, and the wave skips it where no lane
/// hits), and the last non-empty hit mask, kept as two numbers, becomes
/// EXEC once the walk is over. Each lane writes how many nodes it hit and
/// whether it was among the last to hit.
fn wave_walk() -> Vec<u32> {
    let mut a = Asm::new();
    a.sop1("s_mov_b32", S_TABLE, Src::S(2));
    a.sop1("s_mov_b32", S_TABLE + 1, Src::K(0));
    a.s_load_x4(S_RAYS, S_TABLE, 0);
    a.s_load_x4(S_HITS, S_TABLE, 32);
    a.vop3("v_mul_lo_u32", V_RAY_AT, Src::V(0), Src::K(RAY_BYTES as i8));
    a.mubuf("buffer_load_dwordx4", V_ORIGIN, V_RAY_AT, S_RAYS, 0);
    a.mubuf("buffer_load_dwordx4", V_DIR, V_RAY_AT, S_RAYS, 16);
    for k in 0..3 {
        a.vop1("v_rcp_f32", V_INV + k, Src::V(V_DIR + k));
    }
    tree_descriptor(&mut a, S_TREE, false);
    a.sop1("s_mov_b32", 28, Src::K(5));
    a.sop1("s_mov_b32", 30, Src::K(0));
    a.sop1("s_mov_b32", 31, Src::K(0));
    a.vop1("v_mov_b32", V_COUNT, Src::K(0));
    a.vop1("v_mov_b32", V_KIND, Src::K(0));
    a.label("walk");
    // The node's pointer: slot s28, a triangle.
    a.sop2("s_lshl_b32", 29, Src::S(28), Src::K(3));
    a.vop1("v_mov_b32", V_PTR, Src::S(29));
    let ray = [
        V_PTR,
        V_TMAX,
        V_ORIGIN,
        V_ORIGIN + 1,
        V_ORIGIN + 2,
        V_DIR,
        V_DIR + 1,
        V_DIR + 2,
        V_INV,
        V_INV + 1,
        V_INV + 2,
    ];
    a.bvh(V_RESULT, ray, S_TREE);
    a.vop1("v_rcp_f32", V_RCP, Src::V(V_RESULT + 1));
    a.vop2("v_mul_f32", V_T, Src::V(V_RESULT), V_RCP);
    a.vopc("v_cmp_lt_f32", Src::V(V_T), V_TMAX);
    a.sop1("s_and_saveexec_b64", M_LOOP, Src::S(VCC));
    a.branch("s_cbranch_execz", "skip");
    a.vop2("v_add_nc_u32", V_COUNT, Src::K(1), V_COUNT);
    a.sop1("s_mov_b32", 30, Src::S(EXEC));
    a.sop1("s_mov_b32", 31, Src::S(EXEC + 1));
    a.label("skip");
    a.sop2("s_or_b64", EXEC, Src::S(EXEC), Src::S(M_LOOP));
    a.sop2("s_sub_u32", 28, Src::S(28), Src::K(1));
    a.sopc("s_cmp_lg_u32", Src::S(28), Src::K(0));
    a.branch("s_cbranch_scc1", "walk");
    // The popped mask as EXEC: those lanes mark themselves.
    a.sop1("s_mov_b64", M_LOOP, Src::S(EXEC));
    a.sop1("s_mov_b64", EXEC, Src::S(30));
    a.vop1("v_mov_b32", V_KIND, Src::K(1));
    a.sop1("s_mov_b64", EXEC, Src::S(M_LOOP));
    a.vop2("v_lshlrev_b32", V_HIT_AT, Src::K(5), 0);
    a.mubuf("buffer_store_dword", V_COUNT, V_HIT_AT, S_HITS, 0);
    a.mubuf("buffer_store_dword", V_KIND, V_HIT_AT, S_HITS, 4);
    a.endpgm();
    a.finish()
}

/// [`Walk::NoNode`]: `do { count += 1 } while (ray count > count)` in each
/// lane, the count written where a hit's first word goes.
fn no_node() -> Vec<u32> {
    let mut a = Asm::new();
    a.sop1("s_mov_b32", S_TABLE, Src::S(2));
    a.sop1("s_mov_b32", S_TABLE + 1, Src::K(0));
    a.s_load_x4(S_HITS, S_TABLE, 32);
    a.sop1("s_mov_b32", M_RAYS, Src::S(EXEC));
    a.vop1("v_mov_b32", V_COUNT, Src::K(0));
    a.label("loop");
    a.vop2("v_add_nc_u32", V_COUNT, Src::K(1), V_COUNT);
    a.vopc("v_cmpx_gt_u32", Src::S(3), V_COUNT);
    a.branch("s_cbranch_execnz", "loop");
    a.sop1("s_mov_b32", EXEC, Src::S(M_RAYS));
    a.vop2("v_lshlrev_b32", V_HIT_AT, Src::K(5), 0);
    a.mubuf("buffer_store_dword", V_COUNT, V_HIT_AT, S_HITS, 0);
    a.endpgm();
    a.finish()
}

/// A walk translated as any program is, with what the earlier stages made
/// of it.
pub struct Translated {
    pub native: NativeProgram,
    pub program: ir::Program,
    /// The program as one lane runs it.
    pub lanes: structure::Structured,
    pub resources: resource::Resources,
    pub shader: HostShader,
}

/// `walk` through the translation every title program takes: the same
/// stages in the same order, the node instruction emulated, no option set.
/// [`Walk::WaveWalk`] has no per-lane form and fails here as such a title
/// program does.
pub fn translate(walk: Walk) -> Result<Translated, ShaderError> {
    translate_native(walk.native())
}

/// [`translate`] for a program as its container describes it.
pub fn translate_native(native: NativeProgram) -> Result<Translated, ShaderError> {
    let program = ir::build(&native.code, native.wave64, native.vgprs)?;
    let lanes = structure::structure(&program)?;
    let resources = resource::track(&program, &native.layout, native.stage, Some(&lanes))?;
    let io = io::raise(&program, &native)?;
    // A compute program has no signature parts to take from a container.
    let container = Container {
        digest: [0; 16],
        version: (1, 0),
        parts: Vec::new(),
    };
    let shader = emit::emit(&container, &native, &program, &lanes, &io, &resources)?;
    Ok(Translated {
        native,
        program,
        lanes,
        resources,
        shader,
    })
}

/// The software path of the validation probe: a translated walk and how a
/// dispatch of it is set up.
pub struct SoftwareWalk {
    /// The DXIL container and everything the host binds for it
    /// ([`HostShader`]): its user-data dwords name the title-side root
    /// parameters of this module ([`ROOT_TREE`]: the tree's address, read
    /// through the page table as title memory; [`ROOT_TABLE`]: the byte
    /// offset of the ray buffer's descriptor in the resource heap, the hit
    /// buffer's descriptor the next; [`ROOT_COUNT`]: the ray count).
    pub shader: HostShader,
    pub group_threads: u32,
    pub ray_bytes: u32,
    pub hit_bytes: u32,
    /// The deepest stack a tree may need.
    pub stack_entries: u32,
}

/// [`Walk::NearestIds`], translated: the nearest hit of each ray in one
/// bottom level of a `victory-rt` image (the root SRV at the bottom
/// level's first byte), with its geometry and primitive index.
///
/// Against `victory-rt`'s traversal this is **the bottom level only**.
/// Missing: the top level and its instance records (so no transform, no
/// instance mask, index, id or flags: the two instance words of a hit are
/// 0), the ray's flags (no culling by facing or opacity, no first-hit
/// stop), the opaque bit and the alpha test (every triangle is taken as
/// opaque), and every refusal (a ray out of the image's reach, a stack
/// overflow, a node type with no place: the host must not dispatch what
/// the model refuses).
pub fn software() -> Result<SoftwareWalk, ShaderError> {
    Ok(SoftwareWalk {
        shader: translate(Walk::NearestIds)?.shader,
        group_threads: GROUP_THREADS,
        ray_bytes: RAY_BYTES,
        hit_bytes: HIT_BYTES,
        stack_entries: STACK_ENTRIES,
    })
}
