//! Emulated waves (`lower/emulated.rs`) against the console's wave, on any
//! host: no GPU, no DXIL validator, no title data.
//!
//! Each test hand-assembles a small compute program for one console wave
//! (64 lanes, one of 32) and runs it twice:
//!
//! * on [`Console`], a model of the console's wave written from the
//!   instruction set's definitions: one program counter, scalar registers
//!   the wave shares, vector registers with a value per lane, EXEC and VCC
//!   as numbers. It shares the instruction decoder with the translator and
//!   nothing else: no control-flow analysis, no lane booleans. An
//!   instruction it does not know stops the test;
//! * as the translator's emulated wave, on [`Group`]: the written function
//!   run by every thread of a thread group, with group-shared memory and
//!   barriers, under several schedules ([`Schedule`]). The schedules stand
//!   for hosts whose waves have 4 to 128 lanes (a wave's threads advance
//!   together, the waves one after another), for a host that runs threads
//!   one at a time, last first, and for arbitrary interleavings.
//!
//! What each lane stores must be the same in all of them. A store through
//! a view is kept for the loads after it ([`host`]), in whatever order the
//! schedule makes them, so a load that comes too early or a store that
//! lands too late shows. [`Group`] also refuses what would hang or misread
//! on a real host: a barrier that some thread waits at while another has
//! ended, and a read of shared memory bits that nothing wrote.
//!
//! Not covered here, and so not claimed: that a driver accepts and runs
//! the modules (here they are structurally checked, `Flow::check`, and
//! nothing else; `scripts/validate-dxil.sh` puts what these tests emit
//! before Microsoft's validator), what the barriers cost, and anything
//! about programs unlike these (the title's are not here). Two lanes
//! storing at one address in the same instruction have no order in the
//! model either.

use std::collections::{BTreeMap, HashMap};

use super::bitcode::{Arg, Bin, Cast, Flow, FlowOp, FlowTerm, Rmw, pred};
use super::lower::{self, Limits, Lowered, Parts};
use super::{Options, USER_THREAD_LIMIT, emulate, wave_need};
use crate::gfx10::{self, Dpp, Fields, Operand, opcodes};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{ShaderError, io, ir, resource, structure};

// ---- assembling ----

const S_ENDPGM: u32 = 0xBF81_0000;
/// Scalar operand codes.
const VCC: u8 = 106;
const EXEC: u8 = 126;
/// The inline constant -1.
const M1: u16 = 193;
/// "The source is in the DPP word".
const DPP16: u16 = 0xFA;
const DPP8: u16 = 0xE9;

/// The inline constant `n` (0 to 64).
fn k(n: u8) -> u16 {
    assert!(n <= 64);
    128 + u16::from(n)
}

/// VGPR `n` as a source.
fn v(n: u8) -> u16 {
    256 + u16::from(n)
}

fn op(table: &[(u16, &str)], name: &str) -> u32 {
    let found = table.iter().find(|e| e.1 == name);
    u32::from(found.unwrap_or_else(|| panic!("no opcode {name}")).0)
}

fn sop1(name: &str, sdst: u8, src: u16) -> u32 {
    0xBE80_0000 | u32::from(sdst) << 16 | op(opcodes::SOP1, name) << 8 | u32::from(src)
}

fn sop2(name: &str, sdst: u8, src0: u16, src1: u16) -> u32 {
    let (a, b) = (u32::from(src0), u32::from(src1));
    0x8000_0000 | op(opcodes::SOP2, name) << 23 | u32::from(sdst) << 16 | b << 8 | a
}

fn sopc(name: &str, src0: u16, src1: u16) -> u32 {
    0xBF00_0000 | op(opcodes::SOPC, name) << 16 | u32::from(src1) << 8 | u32::from(src0)
}

/// A branch `words` instruction words forward (negative: back) of the next
/// instruction.
fn sopp(name: &str, words: i16) -> u32 {
    0xBF80_0000 | op(opcodes::SOPP, name) << 16 | u32::from(words as u16)
}

fn vop1(name: &str, dst: u8, src: u16) -> u32 {
    0x7E00_0000 | u32::from(dst) << 17 | op(opcodes::VOP1, name) << 9 | u32::from(src)
}

fn vop2(name: &str, vdst: u8, src0: u16, vsrc1: u8) -> u32 {
    op(opcodes::VOP2, name) << 25 | u32::from(vdst) << 17 | u32::from(vsrc1) << 9 | u32::from(src0)
}

/// A compare of `src0` with `vsrc1` into VCC.
fn vopc(name: &str, src0: u16, vsrc1: u8) -> u32 {
    0x7C00_0000 | op(opcodes::VOPC, name) << 17 | u32::from(vsrc1) << 9 | u32::from(src0)
}

/// The three-operand form: `code` is the opcode there (VOP3's own, or a
/// VOP2 opcode plus 0x100).
fn vop3(code: u32, dst: u8, src0: u16, src1: u16, src2: u16) -> [u32; 2] {
    let (a, b, c) = (u32::from(src0), u32::from(src1), u32::from(src2));
    [0xD400_0000 | code << 16 | u32::from(dst), a | b << 9 | c << 18]
}

fn vop3_own(name: &str, dst: u8, src0: u16, src1: u16, src2: u16) -> [u32; 2] {
    vop3(op(opcodes::VOP3, name), dst, src0, src1, src2)
}

/// The DPP word of a row pattern reading `vsrc0`.
fn dpp16(vsrc0: u8, ctrl: u16, bound_ctrl: bool, fi: bool) -> u32 {
    0xFF00_0000 | u32::from(bound_ctrl) << 19 | u32::from(fi) << 18 | u32::from(ctrl) << 8 | u32::from(vsrc0)
}

/// `buffer_store_dword vdata, vaddr, s[4:7], offset offen`.
fn store(vdata: u8, vaddr: u8, offset: u32) -> [u32; 2] {
    assert!(offset < 0x1000);
    [
        0x38 << 26 | op(opcodes::MUBUF, "buffer_store_dword") << 18 | 1 << 12 | offset,
        0x80 << 24 | (4 >> 2) << 16 | u32::from(vdata) << 8 | u32::from(vaddr),
    ]
}

/// `buffer_load_dword vdata, vaddr, s[4:7], offset offen`: the output
/// buffer read back.
fn load(vdata: u8, vaddr: u8, offset: u32) -> [u32; 2] {
    let [first, second] = store(vdata, vaddr, offset);
    let (stores, loads) = (op(opcodes::MUBUF, "buffer_store_dword"), op(opcodes::MUBUF, "buffer_load_dword"));
    [first ^ (stores ^ loads) << 18, second]
}

/// A shared-memory instruction on address `vaddr`: `ds_write_b32` of
/// `vdata`, or `ds_read_b32` into `vdst`.
fn ds(name: &str, vaddr: u8, vdata: u8, vdst: u8) -> [u32; 2] {
    ds_at(name, 0, vaddr, vdata, vdst)
}

/// [`ds`] with the instruction's 16 offset bits: a byte offset, or
/// `ds_swizzle_b32`'s pattern (whose source is in the address field).
fn ds_at(name: &str, offset: u16, vaddr: u8, vdata: u8, vdst: u8) -> [u32; 2] {
    [
        0x36 << 26 | op(opcodes::DS, name) << 18 | u32::from(offset),
        u32::from(vdst) << 24 | u32::from(vdata) << 8 | u32::from(vaddr),
    ]
}

/// `words` with the stores of `outputs` (VGPRs; output `n` of lane `l` at
/// byte `256 * n + 4 * l`) after them.
fn with_stores(mut words: Vec<u32>, outputs: &[u8]) -> Vec<u32> {
    // v15 = 4 * the lane.
    words.push(vop2("v_lshlrev_b32", 15, k(2), 0));
    for (n, &r) in outputs.iter().enumerate() {
        words.extend(store(r, 15, 256 * n as u32));
    }
    words
}

/// A compute program: `body` after the load of the output buffer's
/// descriptor (into s[4:7]) and of a constant buffer's (into s[12:15]).
struct Program {
    native: NativeProgram,
}

impl Program {
    fn new(body: &[u32], wave64: bool, threads: [u32; 3], lds: bool) -> Program {
        let table = |kind, slot, table_offset_dwords| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 0,
            slot,
        };
        let load = |dst: u32, bytes: u32| {
            [0x3D << 26 | 2 << 18 | 1 << 17 | dst << 6, bytes | 125 << 25]
        };
        let mut words = vec![sop1("s_mov_b32", 1, k(0))];
        words.extend(load(4, 0));
        words.extend(load(12, 32));
        words.extend_from_slice(body);
        words.push(S_ENDPGM);
        Program {
            native: NativeProgram {
                stage: HwStage::Cs,
                wave64,
                vgprs: 16,
                layout: vec![RootParam {
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                }],
                used: vec![table(ResourceType::Uav, 0, 0), table(ResourceType::Cbv, 1, 8)],
                io: StageIo::Cs(ComputeIo {
                    // One user SGPR; the thread id's y in v1 for a group of
                    // rows; a granule of shared memory if asked.
                    rsrc2: 1 << 1 | u32::from(threads[1] > 1) << 11 | u32::from(lds) << 15,
                    threads,
                }),
                code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            },
        }
    }

    fn wave(&self) -> u32 {
        if self.native.wave64 { 64 } else { 32 }
    }

    /// The program lowered: in its wave form (`waves`, the emulated wave)
    /// or as lanes of the host's wave.
    fn lowered(&self, waves: bool) -> Result<Lowered, ShaderError> {
        self.lowered_by(waves, lower::lower)
    }

    /// [`Program::lowered`] with the loop limits `limits` in place of the
    /// ones in force, and with thread limits asked for or not.
    fn lowered_limited(&self, waves: bool, limits: Limits, thread_limits: bool) -> Result<Lowered, ShaderError> {
        self.lowered_by(waves, |parts| lower::lower_limited(parts, limits, thread_limits))
    }

    fn lowered_by(
        &self,
        waves: bool,
        lower: impl FnOnce(&Parts) -> Result<Lowered, ShaderError>,
    ) -> Result<Lowered, ShaderError> {
        let native = &self.native;
        let p = ir::build(&native.code, native.wave64, native.vgprs)?;
        let st = if waves { structure::waves(&p)? } else { structure::structure(&p)? };
        let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
        let io = io::raise(&p, native)?;
        let l = lower(&Parts {
            native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![],
            prims: vec![],
            mesh_stride: 0,
            view_marks: false,
        })?;
        l.flow.check().unwrap();
        Ok(l)
    }
}

/// What a wave stored: the dword at each byte offset, with the lane that
/// stored it.
type Stored = BTreeMap<u32, (u32, u32)>;

// ---- the console's wave ----

/// The console's wave, by the instruction set's definitions.
struct Console {
    lanes: u32,
    /// Scalar registers by operand code (VCC at 106, EXEC at 126).
    s: [u32; 128],
    scc: bool,
    /// VGPRs: a value per lane.
    v: Vec<[u32; 64]>,
    lds: Vec<u32>,
    stored: Stored,
    /// A dword may be stored again (the later store stays); otherwise a
    /// second store at an offset stops the test.
    rewrites: bool,
    /// What a scalar load of the constant buffer at a byte offset gives.
    constants: fn(u32) -> u32,
}

impl Console {
    /// Runs `p` on one full wave: lane `l` is thread `l` of the group, and
    /// v0 holds its thread id.
    fn run(p: &Program, constants: fn(u32) -> u32) -> Stored {
        Console::run_with(p, constants, false)
    }

    /// [`Console::run`], with whether a dword may be stored again.
    fn run_with(p: &Program, constants: fn(u32) -> u32, rewrites: bool) -> Stored {
        let wave64 = p.native.wave64;
        let lanes = p.wave();
        let insts = gfx10::decode(&p.native.code).unwrap();
        let mut c = Console {
            lanes,
            s: [0; 128],
            scc: false,
            v: vec![[0; 64]; 16],
            lds: vec![0; 128],
            stored: Stored::new(),
            rewrites,
            constants,
        };
        for l in 0..lanes {
            c.v[0][l as usize] = l;
        }
        c.set64(EXEC, if wave64 { u64::MAX } else { u64::from(u32::MAX) });
        let mut at = 0;
        for _ in 0..100_000 {
            let inst = insts.iter().find(|i| i.offset == at).expect("an instruction");
            let name = inst.name().expect("a named instruction");
            let next = at + 4 * inst.words.len();
            let fields = inst.fields(wave64);
            if name == "s_endpgm" {
                return c.stored;
            }
            at = match (&fields, c.branches(name)) {
                (Fields::Sopp { imm }, Some(taken)) if taken => {
                    (next as i64 + 4 * i64::from(*imm as i16)) as usize
                }
                (_, Some(_)) => next,
                (_, None) => {
                    c.step(name, &fields);
                    next
                }
            };
        }
        panic!("the wave does not end");
    }

    fn get64(&self, code: u8) -> u64 {
        u64::from(self.s[usize::from(code)]) | u64::from(self.s[usize::from(code) + 1]) << 32
    }

    fn set64(&mut self, code: u8, x: u64) {
        self.s[usize::from(code)] = x as u32;
        self.s[usize::from(code) + 1] = (x >> 32) as u32;
    }

    /// EXEC, and VCC, as the wave's lanes.
    fn mask(&self, code: u8) -> u64 {
        if self.lanes == 64 { self.get64(code) } else { u64::from(self.s[usize::from(code)]) }
    }

    /// The lanes in EXEC.
    fn active_lanes(&self) -> Vec<u32> {
        let exec = self.mask(EXEC);
        (0..self.lanes).filter(|&l| exec >> l & 1 != 0).collect()
    }

    /// Whether a branch instruction is taken; `None` for other instructions.
    fn branches(&self, name: &str) -> Option<bool> {
        Some(match name {
            "s_branch" => true,
            "s_cbranch_scc0" => !self.scc,
            "s_cbranch_scc1" => self.scc,
            "s_cbranch_vccz" => self.mask(VCC) == 0,
            "s_cbranch_vccnz" => self.mask(VCC) != 0,
            "s_cbranch_execz" => self.mask(EXEC) == 0,
            "s_cbranch_execnz" => self.mask(EXEC) != 0,
            _ => return None,
        })
    }

    /// A 32-bit source as the wave (`lane` none) or one lane reads it.
    fn src(&self, o: &Operand, lane: Option<u32>) -> u32 {
        match *o {
            Operand::Scalar { code: gfx10::NULL, .. } => 0,
            Operand::Scalar { code, .. } => self.s[usize::from(code)],
            Operand::Vector { reg, .. } => self.v[usize::from(reg)][lane.expect("a lane") as usize],
            Operand::Const(c @ 128..=192) => u32::from(c) - 128,
            Operand::Const(c @ 193..=208) => (192 - i32::from(c)) as u32,
            Operand::Literal(x) => x,
            ref other => panic!("an operand the model does not know: {other:?}"),
        }
    }

    /// A 64-bit scalar source: inline constants sign-extend, a literal
    /// does not.
    fn src64(&self, o: &Operand) -> u64 {
        match *o {
            Operand::Scalar { code, .. } if code != gfx10::NULL => self.get64(code),
            Operand::Const(193..=208) => i64::from(self.src(o, None) as i32) as u64,
            _ => u64::from(self.src(o, None)),
        }
    }

    fn step(&mut self, name: &str, fields: &Fields) {
        match fields {
            Fields::Sop { dst, src, .. } => self.scalar(name, dst.as_ref(), src),
            Fields::Valu(valu) => self.vector(name, valu),
            // The descriptors: what they hold is not the wave's business.
            Fields::Smem { .. } if name == "s_load_dwordx4" => {}
            Fields::Smem { data: Some(Operand::Scalar { code, .. }), offset, .. }
                if name == "s_buffer_load_dword" =>
            {
                // One load for the wave.
                self.s[usize::from(*code)] = (self.constants)(*offset as u32);
            }
            Fields::Buffer { data: Some(Operand::Vector { reg: data, .. }), addr, offset, offen: true, .. }
                if name == "buffer_store_dword" =>
            {
                // Every lane's store of this instruction, in lane order;
                // the next instruction's come after all of them.
                for l in self.active_lanes() {
                    let at = self.src(addr, Some(l)) + u32::from(*offset);
                    let x = self.v[usize::from(*data)][l as usize];
                    let before = self.stored.insert(at, (x, l));
                    assert!(self.rewrites || before.is_none(), "two stores at {at:#x}");
                }
            }
            Fields::Buffer { data: Some(Operand::Vector { reg: data, .. }), addr, offset, offen: true, .. }
                if name == "buffer_load_dword" =>
            {
                // What the buffer holds once every earlier instruction is
                // done in every lane; 0 where nothing was stored.
                for l in self.active_lanes() {
                    let at = self.src(addr, Some(l)) + u32::from(*offset);
                    self.v[usize::from(*data)][l as usize] = self.stored.get(&at).map_or(0, |x| x.0);
                }
            }
            Fields::Ds { dst: Some(Operand::Vector { reg, .. }), addr: Some(addr), data, offset0, offset1, .. }
                if matches!(name, "ds_swizzle_b32" | "ds_bpermute_b32") =>
            {
                // Each lane in EXEC reads a lane of its 32: by a pattern
                // (the lanes of a quad permuted, or the lane's number
                // through and, or and xor masks), or by the dword its
                // address plus the offset names. A lane EXEC leaves out
                // gives 0.
                let offset = u32::from(*offset0) | u32::from(*offset1) << 8;
                let exec = self.mask(EXEC);
                let old = self.v[usize::from(*reg)];
                let mut new = old;
                for l in self.active_lanes() {
                    let (from, x) = if name == "ds_swizzle_b32" {
                        let from = if offset & 0x8000 != 0 {
                            (l & !3) | (offset >> (2 * (l & 3)) & 3)
                        } else {
                            ((l & (offset & 0x1F | !0x1F)) | (offset >> 5 & 0x1F)) ^ (offset >> 10 & 0x1F)
                        };
                        (from, self.src(addr, Some(from)))
                    } else {
                        let dword = self.src(addr, Some(l)).wrapping_add(offset) >> 2;
                        let from = (l & !31) | (dword & 31);
                        (from, self.src(data.first().expect("a source"), Some(from)))
                    };
                    new[l as usize] = if exec >> from & 1 != 0 { x } else { 0 };
                }
                self.v[usize::from(*reg)] = new;
            }
            Fields::Ds { dst, addr: Some(addr), data, .. } => {
                for l in self.active_lanes() {
                    let at = (self.src(addr, Some(l)) / 4) as usize;
                    match (name, dst, data.first()) {
                        ("ds_write_b32", _, Some(x)) => self.lds[at] = self.src(x, Some(l)),
                        ("ds_read_b32", Some(Operand::Vector { reg, .. }), _) => {
                            self.v[usize::from(*reg)][l as usize] = self.lds[at];
                        }
                        _ => panic!("{name} is not modelled"),
                    }
                }
            }
            _ => panic!("{name} is not modelled"),
        }
    }

    fn scalar(&mut self, name: &str, dst: Option<&Operand>, src: &[Operand]) {
        let code = match dst {
            Some(&Operand::Scalar { code, .. }) => code,
            _ => gfx10::NULL,
        };
        let (a, b) = (src.first(), src.get(1));
        let a32 = |c: &Self| c.src(a.expect("a source"), None);
        let b32 = |c: &Self| c.src(b.expect("two sources"), None);
        // The lane-mask width's operations, by their 64-bit names.
        let wide = name.replace("_b32", "_b64");
        let mask_op = self.lanes == 32 && name.ends_with("_b32");
        let m = |c: &Self, o: Option<&Operand>| {
            let x = c.src64(o.expect("a source"));
            if c.lanes == 32 { x & 0xFFFF_FFFF } else { x }
        };
        let all = if self.lanes == 64 { u64::MAX } else { 0xFFFF_FFFF };
        let logic = |x: u64, y: u64| -> Option<u64> {
            Some(match wide.trim_end_matches("_b64").trim_end_matches("_saveexec") {
                "s_and" => x & y,
                "s_or" => x | y,
                "s_xor" => x ^ y,
                "s_andn2" => x & !y,
                "s_orn2" => x | !y,
                "s_nand" => !(x & y),
                "s_nor" => !(x | y),
                "s_xnor" => !(x ^ y),
                _ => return None,
            } & all)
        };
        let put = |c: &mut Self, code: u8, x: u64| {
            if c.lanes == 64 { c.set64(code, x) } else { c.s[usize::from(code)] = x as u32 }
        };
        if name.ends_with("_b64") || mask_op {
            if wide.ends_with("_saveexec_b64") {
                // The old EXEC is saved, the new one is the operation on
                // the source and EXEC, and SCC says whether a lane is left.
                let old = self.mask(EXEC);
                let new = logic(m(self, a), old).unwrap_or_else(|| panic!("{name}"));
                put(self, code, old);
                put(self, EXEC, new);
                self.scc = new != 0;
                return;
            }
            match wide.as_str() {
                "s_mov_b64" => {
                    let x = m(self, a);
                    return put(self, code, x);
                }
                "s_not_b64" => {
                    let x = !m(self, a) & all;
                    put(self, code, x);
                    self.scc = x != 0;
                    return;
                }
                "s_bcnt1_i32_b64" => {
                    let n = m(self, a).count_ones();
                    self.s[usize::from(code)] = n;
                    self.scc = n != 0;
                    return;
                }
                "s_ff1_i32_b64" => {
                    let x = m(self, a);
                    self.s[usize::from(code)] = if x == 0 { u32::MAX } else { x.trailing_zeros() };
                    return;
                }
                "s_cselect_b64" => {
                    let x = if self.scc { m(self, a) } else { m(self, b) };
                    return put(self, code, x);
                }
                "s_lshl_b64" | "s_lshr_b64" if self.lanes == 64 && name.ends_with("_b64") => {
                    let n = b32(self) & 63;
                    let x = self.src64(a.expect("a source"));
                    let x = if name == "s_lshl_b64" { x << n } else { x >> n };
                    self.set64(code, x);
                    self.scc = x != 0;
                    return;
                }
                _ => {}
            }
            if let Some(x) = logic(m(self, a), m(self, b)) {
                put(self, code, x);
                self.scc = x != 0;
                return;
            }
        }
        let x = match name {
            "s_mov_b32" => {
                self.s[usize::from(code)] = a32(self);
                return;
            }
            "s_add_u32" => {
                let (x, carry) = a32(self).overflowing_add(b32(self));
                self.scc = carry;
                x
            }
            "s_sub_u32" => {
                let (x, borrow) = a32(self).overflowing_sub(b32(self));
                self.scc = borrow;
                x
            }
            "s_add_i32" => {
                let (x, over) = (a32(self) as i32).overflowing_add(b32(self) as i32);
                self.scc = over;
                x as u32
            }
            "s_and_b32" | "s_or_b32" | "s_xor_b32" | "s_lshl_b32" | "s_lshr_b32" => {
                let (x, y) = (a32(self), b32(self));
                let r = match name {
                    "s_and_b32" => x & y,
                    "s_or_b32" => x | y,
                    "s_xor_b32" => x ^ y,
                    "s_lshl_b32" => x << (y & 31),
                    _ => x >> (y & 31),
                };
                self.scc = r != 0;
                r
            }
            "s_bcnt1_i32_b32" => {
                let n = a32(self).count_ones();
                self.scc = n != 0;
                n
            }
            "s_ff1_i32_b32" => match a32(self) {
                0 => u32::MAX,
                x => x.trailing_zeros(),
            },
            "s_cmp_eq_u32" | "s_cmp_lg_u32" | "s_cmp_gt_u32" | "s_cmp_lt_u32" => {
                let (x, y) = (a32(self), b32(self));
                self.scc = match name {
                    "s_cmp_eq_u32" => x == y,
                    "s_cmp_lg_u32" => x != y,
                    "s_cmp_gt_u32" => x > y,
                    _ => x < y,
                };
                return;
            }
            "s_cmp_eq_u64" | "s_cmp_lg_u64" => {
                let equal = self.src64(a.expect("a source")) == self.src64(b.expect("two sources"));
                self.scc = equal == (name == "s_cmp_eq_u64");
                return;
            }
            _ => panic!("{name} is not modelled"),
        };
        self.s[usize::from(code)] = x;
    }

    /// The lane DPP makes lane `l` read, whether that lane exists for the
    /// pattern (a shift can leave the row), and whether lane `l` writes at
    /// all (row and bank masks are all ones in these tests).
    fn dpp_source(dpp: Dpp, l: u32) -> (u32, bool) {
        match dpp {
            Dpp::Lanes { sel, .. } => ((l & !7) | (sel >> (3 * (l & 7)) & 7), true),
            Dpp::Row { ctrl, .. } => {
                let (row, pos, n) = (l & !15, l & 15, u32::from(ctrl & 15));
                match ctrl {
                    0..=0xFF => ((l & !3) | (u32::from(ctrl) >> (2 * (l & 3)) & 3), true),
                    0x101..=0x10F => (l.wrapping_add(n), pos + n < 16),
                    0x111..=0x11F => (l.wrapping_sub(n), pos >= n),
                    0x121..=0x12F => (row | (pos.wrapping_sub(n) & 15), true),
                    0x140 => (l ^ 15, true),
                    0x141 => (l ^ 7, true),
                    0x150..=0x15F => (row | n, true),
                    0x160..=0x16F => (l ^ n, true),
                    _ => panic!("DPP pattern {ctrl:#x} is not modelled"),
                }
            }
        }
    }

    fn vector(&mut self, name: &str, valu: &gfx10::Valu) {
        assert!(valu.sdwa.is_none() && valu.abs == 0 && valu.neg == 0 && !valu.clamp && valu.omod == 0);
        let exec = self.mask(EXEC);
        let src = |c: &Self, n: usize, l: u32| c.src(&valu.src[n], Some(l));
        // Results a lane does not take part in: one for the wave.
        match (name, valu.dst) {
            ("v_readfirstlane_b32", Some(Operand::Scalar { code, .. })) => {
                // The lowest lane in EXEC; lane 0 when EXEC is empty.
                let first = if exec == 0 { 0 } else { exec.trailing_zeros() };
                self.s[usize::from(code)] = src(self, 0, first);
                return;
            }
            ("v_readlane_b32", Some(Operand::Scalar { code, .. })) => {
                let lane = self.src(&valu.src[1], None) & (self.lanes - 1);
                self.s[usize::from(code)] = src(self, 0, lane);
                return;
            }
            ("v_writelane_b32", Some(Operand::Vector { reg, .. })) => {
                // The lane named takes the scalar, whatever EXEC holds.
                let lane = self.src(&valu.src[1], None) & (self.lanes - 1);
                self.v[usize::from(reg)][lane as usize] = self.src(&valu.src[0], None);
                return;
            }
            _ => {}
        }
        if let Some(rest) = name.strip_prefix("v_cmp_").or(name.strip_prefix("v_cmpx_")) {
            // A lane outside EXEC has no bit in the result.
            let mut bits = 0u64;
            for l in (0..self.lanes).filter(|&l| exec >> l & 1 != 0) {
                let (x, y) = (src(self, 0, l), src(self, 1, l));
                let holds = match rest {
                    "eq_u32" => x == y,
                    "ne_u32" => x != y,
                    "lt_u32" => x < y,
                    "gt_u32" => x > y,
                    "ge_u32" => x >= y,
                    _ => panic!("{name} is not modelled"),
                };
                bits |= u64::from(holds) << l;
            }
            let code = match valu.dst {
                _ if name.starts_with("v_cmpx_") => EXEC,
                Some(Operand::Scalar { code, .. }) => code,
                _ => panic!("a compare without a result"),
            };
            if self.lanes == 64 {
                self.set64(code, bits);
            } else {
                self.s[usize::from(code)] = bits as u32;
            }
            return;
        }
        let Some(Operand::Vector { reg, .. }) = valu.dst else {
            panic!("{name} is not modelled");
        };
        let old = self.v[usize::from(reg)];
        let mut new = old;
        for l in (0..self.lanes).filter(|&l| exec >> l & 1 != 0) {
            // src0 as DPP fetches it: from another lane of the row; 0 from
            // a lane the pattern leaves the row for or EXEC leaves out
            // (unless inactive lanes are fetched), where the bound says so,
            // and no write at all where it does not.
            let a = match valu.dpp {
                None => src(self, 0, l),
                Some(dpp) => {
                    let (from, inside) = Self::dpp_source(dpp, l);
                    let (fi, bound) = match dpp {
                        Dpp::Lanes { fi, .. } => (fi, true),
                        Dpp::Row { fi, bound_ctrl, .. } => (fi, bound_ctrl),
                    };
                    if inside && (fi || exec >> from & 1 != 0) {
                        src(self, 0, from)
                    } else if bound {
                        0
                    } else {
                        continue;
                    }
                }
            };
            let b = |c: &Self| src(c, 1, l);
            new[l as usize] = match name {
                "v_mov_b32" => a,
                "v_add_nc_u32" => a.wrapping_add(b(self)),
                "v_sub_nc_u32" => a.wrapping_sub(b(self)),
                "v_and_b32" => a & b(self),
                "v_or_b32" => a | b(self),
                "v_xor_b32" => a ^ b(self),
                "v_lshlrev_b32" => b(self) << (a & 31),
                "v_lshrrev_b32" => b(self) >> (a & 31),
                "v_mul_lo_u32" => a.wrapping_mul(b(self)),
                // Lanes of the mask below this one, plus the second source.
                "v_mbcnt_lo_u32_b32" => {
                    let below = if l < 32 { (1u32 << l) - 1 } else { u32::MAX };
                    (a & below).count_ones() + b(self)
                }
                "v_mbcnt_hi_u32_b32" => {
                    let below = if l < 32 { 0 } else { (1u32 << (l - 32)) - 1 };
                    (a & below).count_ones() + b(self)
                }
                "v_cndmask_b32" => {
                    let Operand::Scalar { code, .. } = valu.src[2] else {
                        panic!("a select without a mask");
                    };
                    if self.mask(code) >> l & 1 != 0 { b(self) } else { a }
                }
                // The other row of the lane's half; the lane picked there
                // is nibble i of {src2, src1} for lane i of a row.
                "v_permlanex16_b32" => {
                    let sel = u64::from(self.src(&valu.src[1], None))
                        | u64::from(self.src(&valu.src[2], None)) << 32;
                    let pick = (sel >> (4 * (l & 15)) & 15) as u32;
                    src(self, 0, (l & !31) | ((l & 16) ^ 16) | pick)
                }
                _ => panic!("{name} is not modelled"),
            };
        }
        self.v[usize::from(reg)] = new;
    }
}

// ---- a thread group running the written function ----

/// How the threads of a group take turns between barriers.
#[derive(Debug, Clone, Copy)]
enum Schedule {
    /// A host whose waves have this many lanes: the group's threads in runs
    /// of that many, each run advancing an instruction at a time together
    /// until all of it waits or has ended, then the next run.
    Waves(usize),
    /// One thread at a time as far as it can go, the last thread first.
    LastFirst,
    /// Any thread, a few instructions at a time (a seeded sequence).
    Mixed(u64),
}

const SCHEDULES: [Schedule; 10] = [
    Schedule::Waves(4),
    Schedule::Waves(8),
    Schedule::Waves(16),
    Schedule::Waves(32),
    Schedule::Waves(64),
    Schedule::Waves(128),
    Schedule::LastFirst,
    Schedule::Mixed(1),
    Schedule::Mixed(0x9E37_79B9),
    Schedule::Mixed(0xDEAD_BEEF),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    /// At a barrier, until every thread is.
    Waiting,
    Ended,
}

struct Thread {
    block: usize,
    /// The next instruction of the block (its phis are done on entry).
    next: usize,
    values: Vec<u64>,
    fields: HashMap<u32, Vec<u64>>,
    state: State,
}

/// A dword of group-shared memory and which of its bits something wrote.
#[derive(Clone, Copy, Default)]
struct Cell {
    value: u32,
    written: u32,
}

/// What answers a thread's calls (all but the barrier): the thread, the
/// function's name, the arguments; the result's fields.
type Call<'a> = dyn FnMut(usize, &str, &[u64]) -> Vec<u64> + 'a;

/// A thread group: its threads, each running `flow`, and the memory they
/// share.
struct Group<'a> {
    flow: &'a Flow,
    threads: Vec<Thread>,
    memory: HashMap<u64, Cell>,
    barriers: u64,
}

impl<'a> Group<'a> {
    fn new(flow: &'a Flow, threads: usize) -> Self {
        let ids = flow.blocks.iter().flat_map(|b| &b.insts).map(|i| i.id as usize + 1);
        let values = vec![0u64; ids.max().unwrap_or(0)];
        let mut g = Group {
            flow,
            threads: (0..threads)
                .map(|_| Thread {
                    block: 0,
                    next: 0,
                    values: values.clone(),
                    fields: HashMap::new(),
                    state: State::Running,
                })
                .collect(),
            memory: HashMap::new(),
            barriers: 0,
        };
        for t in 0..threads {
            g.enter(t, 0, usize::MAX).unwrap();
        }
        g
    }

    fn get(&self, t: usize, a: Arg) -> u64 {
        match a {
            Arg::Inst(i) => self.threads[t].values[i as usize],
            Arg::Int(x) => x,
            // An address: the global, then the element.
            Arg::Global(g) => (u64::from(g) + 1) << 32,
            Arg::Other => 0,
        }
    }

    /// Thread `t` arrives in `block` from block `from`: its phis all read
    /// the values of the edge taken.
    fn enter(&mut self, t: usize, block: usize, from: usize) -> Result<(), String> {
        let mut joined = Vec::new();
        for inst in &self.flow.blocks[block].insts {
            let FlowOp::Phi(incoming) = &inst.op else { break };
            let Some(&(a, _)) = incoming.iter().find(|x| x.1 == from) else {
                return Err(format!("block {block}: phi {} has no entry from {from}", inst.id));
            };
            joined.push((inst.id, self.get(t, a)));
        }
        let thread = &mut self.threads[t];
        for &(id, x) in &joined {
            thread.values[id as usize] = x;
        }
        (thread.block, thread.next) = (block, joined.len());
        Ok(())
    }

    /// One instruction (or the branch closing a block) of thread `t`.
    fn step(&mut self, t: usize, call: &mut Call) -> Result<(), String> {
        let (b, n) = (self.threads[t].block, self.threads[t].next);
        let block = &self.flow.blocks[b];
        let Some(inst) = block.insts.get(n) else {
            let to = match block.term {
                FlowTerm::Missing => return Err(format!("block {b} has no terminator")),
                FlowTerm::Ret => {
                    self.threads[t].state = State::Ended;
                    return Ok(());
                }
                FlowTerm::Br(to) => to,
                FlowTerm::CondBr(c, yes, no) => {
                    if self.get(t, c) != 0 { yes } else { no }
                }
            };
            return self.enter(t, to, b);
        };
        self.threads[t].next += 1;
        let ones = |bits: u32| u64::MAX >> (64 - bits.clamp(1, 64));
        let signed = |x: u64, bits: u32| {
            let shift = 64 - bits.clamp(1, 64);
            ((x << shift) as i64) >> shift
        };
        let not_run = |what: &str| Err(format!("block {b}: {what} {} is not run", inst.id));
        let result = match &inst.op {
            FlowOp::Bin(..) if inst.float => return not_run("float instruction"),
            FlowOp::Bin(op, x, y) => {
                let (x, y, bits) = (self.get(t, *x), self.get(t, *y), inst.bits);
                let r = match op {
                    Bin::Add => x.wrapping_add(y),
                    Bin::Sub => x.wrapping_sub(y),
                    Bin::Mul => x.wrapping_mul(y),
                    Bin::UDiv | Bin::URem | Bin::SDiv if y == 0 => return not_run("division by 0 in"),
                    Bin::UDiv => x / y,
                    Bin::URem => x % y,
                    Bin::SDiv => signed(x, bits).wrapping_div(signed(y, bits)) as u64,
                    Bin::Shl | Bin::LShr | Bin::AShr if y >= u64::from(bits) => 0,
                    Bin::Shl => x << y,
                    Bin::LShr => x >> y,
                    Bin::AShr => (signed(x, bits) >> y) as u64,
                    Bin::And => x & y,
                    Bin::Or => x | y,
                    Bin::Xor => x ^ y,
                };
                r & ones(bits)
            }
            FlowOp::Cmp(p, bits, x, y) => {
                let (x, y) = (self.get(t, *x), self.get(t, *y));
                let (sx, sy) = (signed(x, *bits), signed(y, *bits));
                u64::from(match *p {
                    pred::EQ => x == y,
                    pred::NE => x != y,
                    pred::IUGT => x > y,
                    pred::IUGE => x >= y,
                    pred::IULT => x < y,
                    pred::IULE => x <= y,
                    pred::SGT => sx > sy,
                    pred::SGE => sx >= sy,
                    pred::SLT => sx < sy,
                    pred::SLE => sx <= sy,
                    _ => return not_run("float compare"),
                })
            }
            FlowOp::Select(c, yes, no) => {
                let pick = if self.get(t, *c) != 0 { *yes } else { *no };
                self.get(t, pick)
            }
            FlowOp::Cast(op, bits, x) => {
                let x = self.get(t, *x);
                match op {
                    Cast::Trunc | Cast::ZExt | Cast::Bitcast => x & ones(inst.bits),
                    Cast::SExt => signed(x, *bits) as u64 & ones(inst.bits),
                    _ => return not_run("float cast"),
                }
            }
            FlowOp::Extract(a, index) => {
                let Arg::Inst(a) = *a else {
                    return not_run("field of a constant read by");
                };
                match self.threads[t].fields.get(&a).and_then(|v| v.get(*index as usize)) {
                    Some(&x) => x,
                    None => return not_run("field no call gave, read by"),
                }
            }
            FlowOp::Call(name, args) => {
                let args: Vec<u64> = args.iter().map(|&a| self.get(t, a)).collect();
                if name == "dx.op.barrier" {
                    // A wait for the whole group (bit 0 of the mode).
                    if args[1] & 1 == 0 {
                        return not_run("barrier without a wait,");
                    }
                    self.threads[t].state = State::Waiting;
                    return Ok(());
                }
                let result = call(t, name, &args);
                let first = result.first().copied().unwrap_or(0);
                self.threads[t].fields.insert(inst.id, result);
                first
            }
            FlowOp::Element(array, index) => self.get(t, *array) + self.get(t, *index),
            FlowOp::Load(at) => {
                let cell = self.memory.get(&self.get(t, *at)).copied().unwrap_or_default();
                if cell.written != u32::MAX {
                    return Err(format!(
                        "thread {t} reads shared memory bits nothing wrote ({:#x} written), instruction {}",
                        cell.written, inst.id
                    ));
                }
                u64::from(cell.value)
            }
            FlowOp::Store(at, x) => {
                let (at, x) = (self.get(t, *at), self.get(t, *x) as u32);
                self.memory.insert(at, Cell { value: x, written: u32::MAX });
                0
            }
            FlowOp::Atomic(op, at, x) => {
                let (at, x) = (self.get(t, *at), self.get(t, *x) as u32);
                let cell = self.memory.entry(at).or_default();
                let old = cell.value;
                match op {
                    // A bit cleared or set is written whatever was there.
                    Rmw::And => {
                        cell.value &= x;
                        cell.written |= !x;
                    }
                    Rmw::Or => {
                        cell.value |= x;
                        cell.written |= x;
                    }
                    _ => return not_run("atomic operation the group does not model,"),
                }
                u64::from(old)
            }
            FlowOp::Other(_) => return not_run("memory instruction"),
            FlowOp::Phi(_) => return not_run("phi after another instruction,"),
        };
        self.threads[t].values[inst.id as usize] = result;
        Ok(())
    }

    /// Runs the group to its end. An error is something a host would not
    /// forgive: a thread that ends while others wait at a barrier (they
    /// would wait for ever), or what [`Group::step`] refuses.
    fn run(&mut self, schedule: Schedule, call: &mut Call) -> Result<(), String> {
        let count = self.threads.len();
        let mut budget = 50_000_000u64;
        let mut seed = match schedule {
            Schedule::Mixed(seed) => seed | 1,
            _ => 1,
        };
        let mut random = move || {
            // xorshift64
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        loop {
            // Until every thread waits at a barrier or has ended.
            match schedule {
                Schedule::Waves(lanes) => {
                    for first in (0..count).step_by(lanes) {
                        let wave = first..count.min(first + lanes);
                        while wave.clone().any(|t| self.threads[t].state == State::Running) {
                            for t in wave.clone() {
                                if self.threads[t].state == State::Running {
                                    budget = budget.checked_sub(1).ok_or("the step budget ran out")?;
                                    self.step(t, call)?;
                                }
                            }
                        }
                    }
                }
                Schedule::LastFirst => {
                    for t in (0..count).rev() {
                        while self.threads[t].state == State::Running {
                            budget = budget.checked_sub(1).ok_or("the step budget ran out")?;
                            self.step(t, call)?;
                        }
                    }
                }
                Schedule::Mixed(_) => loop {
                    let running: Vec<usize> =
                        (0..count).filter(|&t| self.threads[t].state == State::Running).collect();
                    if running.is_empty() {
                        break;
                    }
                    let t = running[(random() % running.len() as u64) as usize];
                    for _ in 0..1 + random() % 5 {
                        if self.threads[t].state == State::Running {
                            budget = budget.checked_sub(1).ok_or("the step budget ran out")?;
                            self.step(t, call)?;
                        }
                    }
                },
            }
            let waiting = self.threads.iter().filter(|t| t.state == State::Waiting).count();
            if waiting == 0 {
                return Ok(());
            }
            if waiting != count {
                return Err(format!(
                    "{waiting} threads wait at a barrier that {} threads never reach",
                    count - waiting
                ));
            }
            self.barriers += 1;
            for thread in &mut self.threads {
                thread.state = State::Running;
            }
        }
    }
}

/// What the translated programs of these tests ask of a host, per thread:
/// ids, the root argument (a descriptor table at 0), handles (a view is
/// named by its heap index), a buffer's size, bit counts, and the views'
/// memory: a store is recorded in `stored` and kept, by view and byte
/// offset, for the loads after it, whichever thread they are of. A load of
/// what no thread stored (the constant buffer; the output buffer before a
/// store) gives `constants` of the byte offset and the thread. A second
/// store at an offset stops the test unless `rewrites`; any call not named
/// here does.
fn host<'a>(
    threads: [u32; 3],
    constants: &'a dyn Fn(u32, usize) -> u32,
    stored: &'a mut Stored,
    rewrites: bool,
) -> impl FnMut(usize, &str, &[u64]) -> Vec<u64> + 'a {
    let mut views: HashMap<(u64, u32), u32> = HashMap::new();
    move |t: usize, name: &str, args: &[u64]| -> Vec<u64> {
        let id = [
            t as u32 % threads[0],
            t as u32 / threads[0] % threads[1],
            t as u32 / (threads[0] * threads[1]),
        ];
        match name {
            "dx.op.threadIdInGroup.i32" => vec![u64::from(id[args[1] as usize])],
            "dx.op.flattenedThreadIdInGroup.i32" => vec![t as u64],
            "dx.op.cbufferLoadLegacy.i32" => vec![0; 4],
            "dx.op.createHandleFromBinding" => vec![u64::MAX],
            "dx.op.createHandleFromHeap" | "dx.op.annotateHandle" => vec![args[1]],
            "dx.op.getDimensions" => vec![1 << 16; 4],
            // Bit counts and the first set bit (-1 for none).
            "dx.op.unaryBits.i32" => vec![match (args[0], args[1] as u32) {
                (31, x) => u64::from(x.count_ones()),
                (32, 0) => u64::from(u32::MAX),
                (32, x) => u64::from(x.trailing_zeros()),
                other => panic!("unaryBits {other:?}"),
            }],
            "dx.op.rawBufferLoad.f32" | "dx.op.rawBufferLoad.i32" => {
                let (view, at) = (args[1], args[2] as u32);
                let held = views.get(&(view, at)).copied();
                vec![u64::from(held.unwrap_or_else(|| constants(at, t)))]
            }
            "dx.op.rawBufferStore.i32" => {
                let (view, at, x) = (args[1], args[2] as u32, args[4] as u32);
                views.insert((view, at), x);
                let before = stored.insert(at, (x, t as u32));
                assert!(rewrites || before.is_none(), "two stores at {at:#x}");
                Vec::new()
            }
            other => panic!("a call these tests do not answer: {other}"),
        }
    }
}

/// Whether the function calls a function whose name contains `part`.
fn calls(flow: &Flow, part: &str) -> bool {
    (0..flow.blocks.len()).any(|b| flow.calls(b).iter().any(|name| name.contains(part)))
}

/// Runs `body` (plus the stores of `outputs`) as the console's wave and as
/// the emulated wave under every schedule, and checks that every lane
/// stores the same in all. Returns what was stored and the lowering.
fn same_as_the_console(body: &[u32], outputs: &[u8], wave64: bool, lds: bool) -> (Stored, Lowered) {
    compared(body, outputs, wave64, lds, false)
}

/// [`same_as_the_console`], with whether a dword of the output buffer may
/// be stored more than once: then what each dword holds in the end, and
/// the lane that stored it last, must be the same.
fn compared(body: &[u32], outputs: &[u8], wave64: bool, lds: bool, rewrites: bool) -> (Stored, Lowered) {
    let lanes = if wave64 { 64 } else { 32 };
    let threads = [lanes, 1, 1];
    let p = Program::new(&with_stores(body.to_vec(), outputs), wave64, threads, lds);
    let want = Console::run_with(&p, |_| 0, rewrites);
    let l = p.lowered(true).unwrap();
    assert!(l.emulated && l.wave_size.is_none(), "an emulated wave declares no wave size");
    assert!(!calls(&l.flow, "wave") && !calls(&l.flow, "quad"), "no wave operation is left");
    let mut barriers = None;
    for schedule in SCHEDULES {
        let mut got = Stored::new();
        let mut group = Group::new(&l.flow, lanes as usize);
        group
            .run(schedule, &mut host(threads, &|_, _| 0, &mut got, rewrites))
            .unwrap_or_else(|e| panic!("{schedule:?}: {e}"));
        assert_eq!(got, want, "{schedule:?}");
        // The lanes meet as often however the host runs them.
        assert_eq!(*barriers.get_or_insert(group.barriers), group.barriers, "{schedule:?}");
    }
    (want, l)
}

/// Output `n` of each lane (0 where the lane stored none).
fn output(stored: &Stored, n: u32, lanes: u32) -> Vec<u32> {
    (0..lanes).map(|l| stored.get(&(256 * n + 4 * l)).map_or(0, |x| x.0)).collect()
}

// ---- the tests ----

/// A sum over the wave's lanes of each lane's number plus 3, by the kind
/// of steps the console compiler expands `WaveActiveSum` into: the lanes
/// outside the saved EXEC give 0, every lane adds the lanes a pattern
/// pairs it with (quads, half rows, rows), then the other row of its half.
/// A wave of 32 is done there; one of 64 adds its two halves as scalars.
fn reduction(wave64: bool) -> Vec<u32> {
    let width = if wave64 { "b64" } else { "b32" };
    let mut w = vec![vop2("v_add_nc_u32", 1, k(3), 0), sop1(&format!("s_or_saveexec_{width}"), 8, M1)];
    w.extend(vop3(0x101, 1, k(0), v(1), 8)); // v_cndmask_b32 v1, 0, v1, s8
    for ctrl in [0xB1, 0x4E, 0x141, 0x140] {
        w.push(vop2("v_add_nc_u32", 1, DPP16, 1));
        w.push(dpp16(1, ctrl, false, false));
    }
    w.extend(vop3_own("v_permlanex16_b32", 2, v(1), k(0), k(0)));
    w.push(vop2("v_add_nc_u32", 1, v(2), 1));
    w.extend(vop3_own("v_readlane_b32", 2, v(1), k(0), 0));
    if wave64 {
        w.extend(vop3_own("v_readlane_b32", 3, v(1), k(32), 0));
        w.push(sop2("s_add_u32", 2, 2, 3));
    }
    w.push(sop1(&format!("s_mov_{width}"), EXEC, 8));
    w.push(vop1("v_mov_b32", 3, 2));
    w
}

#[test]
fn a_wave_reduction_runs_step_by_step() {
    for wave64 in [false, true] {
        let lanes = if wave64 { 64 } else { 32 };
        let (stored, l) = same_as_the_console(&reduction(wave64), &[3], wave64, false);
        // Known without the model: the sum of lane + 3, in every lane.
        let sum = (0..lanes).sum::<u32>() + 3 * lanes;
        assert_eq!(output(&stored, 0, lanes), vec![sum; lanes as usize]);
        assert!(l.meetings > 0);
    }
    // As lanes of the host's wave, the wave of 32's sum is a whole-wave
    // section lowered to the host's wave operation, and the program asks
    // for the console's wave size for nothing else.
    let p = Program::new(&with_stores(reduction(false), &[3]), false, [32, 1, 1], false);
    let host = p.lowered(false).unwrap();
    assert_eq!(host.wave_size, Some(32));
    assert_eq!(host.wave_needs, wave_need::SECTION);
    assert!(calls(&host.flow, "waveActiveOp"));
}

#[test]
fn a_mask_read_as_a_number_is_the_waves() {
    for (compare, bound) in [("v_cmp_lt_u32", 5), ("v_cmp_lt_u32", 40), ("v_cmp_lt_u32", 63), ("v_cmp_ge_u32", 64)] {
        // The lanes above `bound` (none above 63; every lane is below 64):
        // their count, the first of them, and the mask's two dwords.
        let body = [
            vopc(compare, k(bound), 0),
            sop1("s_bcnt1_i32_b64", 2, u16::from(VCC)),
            sop1("s_ff1_i32_b64", 3, u16::from(VCC)),
            vop1("v_mov_b32", 1, 2),
            vop1("v_mov_b32", 2, 3),
            vop1("v_mov_b32", 3, u16::from(VCC)),
            vop1("v_mov_b32", 4, u16::from(VCC) + 1),
        ];
        let (stored, _) = same_as_the_console(&body, &[1, 2, 3, 4], true, false);
        let mask: u64 = (0..64u32)
            .filter(|&l| if compare == "v_cmp_lt_u32" { u32::from(bound) < l } else { u32::from(bound) >= l })
            .map(|l| 1u64 << l)
            .sum();
        let first = if mask == 0 { u32::MAX } else { mask.trailing_zeros() };
        assert_eq!(output(&stored, 0, 64), vec![mask.count_ones(); 64], "{compare} {bound}");
        assert_eq!(output(&stored, 1, 64), vec![first; 64], "{compare} {bound}");
        assert_eq!(output(&stored, 2, 64), vec![mask as u32; 64], "{compare} {bound}");
        assert_eq!(output(&stored, 3, 64), vec![(mask >> 32) as u32; 64], "{compare} {bound}");
    }
}

/// `if (lane > bound) { v1 += 7; s10 += 1 }`, as the console compiler
/// writes it: the wave skips the body when no lane is in it.
fn skip(bound: u8) -> Vec<u32> {
    vec![
        sop1("s_mov_b32", 10, k(0)),
        vop1("v_mov_b32", 1, v(0)),
        vopc("v_cmp_lt_u32", k(bound), 0),
        sop1("s_and_saveexec_b64", 8, u16::from(VCC)),
        sopp("s_cbranch_execz", 2),
        vop2("v_add_nc_u32", 1, k(7), 1),
        sop2("s_add_u32", 10, 10, k(1)),
        sop2("s_or_b64", EXEC, u16::from(EXEC), 8),
        vop1("v_mov_b32", 2, 10),
    ]
}

#[test]
fn a_branch_on_a_lane_mask_is_one_decision_for_the_wave() {
    for bound in [5, 31, 62, 63] {
        let (stored, _) = same_as_the_console(&skip(bound), &[1, 2], true, false);
        let own: Vec<u32> = (0..64).map(|l| if l > u32::from(bound) { l + 7 } else { l }).collect();
        assert_eq!(output(&stored, 0, 64), own, "{bound}");
        // The scalar the body counts in is the wave's: every lane sees the
        // body run, the lanes outside it too, unless no lane was in it.
        let ran = u32::from(bound < 63);
        assert_eq!(output(&stored, 1, 64), vec![ran; 64], "{bound}");
    }
}

#[test]
fn a_store_inside_a_branch_is_made_by_the_lanes_in_exec() {
    // Every lane of the wave is in the body's block; only the lanes in
    // EXEC store there.
    for bound in [5, 40, 63] {
        let mut body = vec![
            vop2("v_lshlrev_b32", 15, k(2), 0),
            vop2("v_add_nc_u32", 1, k(9), 0),
            vopc("v_cmp_lt_u32", k(bound), 0),
            sop1("s_and_saveexec_b64", 8, u16::from(VCC)),
            sopp("s_cbranch_execz", 2),
        ];
        body.extend(store(1, 15, 0));
        body.push(sop2("s_or_b64", EXEC, u16::from(EXEC), 8));
        let (stored, _) = same_as_the_console(&body, &[], true, false);
        let lanes: Vec<u32> = stored.values().map(|x| x.1).collect();
        assert_eq!(lanes, (u32::from(bound) + 1..64).collect::<Vec<u32>>(), "{bound}");
    }
}

#[test]
fn every_test_of_a_mask_is_the_waves() {
    // The lanes above a bound in VCC, then a branch over `v1 += 9` (which
    // every lane runs, EXEC being whole) on each wave-wide test there is:
    // VCC empty or not, SCC of a mask operation, and two masks compared.
    let tests: [(&str, Vec<u32>); 6] = [
        ("vccz", vec![sopp("s_cbranch_vccz", 1)]),
        ("vccnz", vec![sopp("s_cbranch_vccnz", 1)]),
        ("some lane", vec![sop2("s_and_b64", 10, u16::from(VCC), u16::from(EXEC)), sopp("s_cbranch_scc1", 1)]),
        ("no lane", vec![sop2("s_and_b64", 10, u16::from(VCC), u16::from(EXEC)), sopp("s_cbranch_scc0", 1)]),
        ("equal", vec![sopc("s_cmp_eq_u64", u16::from(VCC), u16::from(EXEC)), sopp("s_cbranch_scc1", 1)]),
        ("differ", vec![sopc("s_cmp_eq_u64", u16::from(VCC), u16::from(EXEC)), sopp("s_cbranch_scc0", 1)]),
    ];
    for (name, test) in &tests {
        // No lane above 63, some above 20, every lane at or below 64.
        for (compare, bound) in [("v_cmp_lt_u32", 63), ("v_cmp_lt_u32", 20), ("v_cmp_ge_u32", 64)] {
            let mut body = vec![vop1("v_mov_b32", 1, v(0)), vopc(compare, k(bound), 0)];
            body.extend(test);
            body.push(vop2("v_add_nc_u32", 1, k(9), 1));
            let (stored, _) = same_as_the_console(&body, &[1], true, false);
            let (none, all) = (bound == 63, bound == 64);
            let skipped = match *name {
                "vccz" | "no lane" => none,
                "vccnz" | "some lane" => !none,
                "equal" => all,
                _ => !all,
            };
            let want: Vec<u32> = (0..64).map(|l| if skipped { l } else { l + 9 }).collect();
            assert_eq!(output(&stored, 0, 64), want, "{name} {compare} {bound}");
        }
    }
}

#[test]
fn a_loop_runs_until_no_lane_is_left_and_scalars_count_for_the_wave() {
    // Each lane counts up to (its number & 7), at least once; the wave goes
    // round while a lane is left and counts its rounds in a scalar.
    let body = [
        sop1("s_mov_b32", 10, k(0)),
        vop1("v_mov_b32", 1, k(0)),
        vop2("v_and_b32", 2, k(7), 0),
        sop1("s_mov_b64", 8, u16::from(EXEC)),
        // the loop:
        vop2("v_add_nc_u32", 1, k(1), 1),
        sop2("s_add_u32", 10, 10, k(1)),
        vopc("v_cmp_lt_u32", v(1), 2),
        sop2("s_and_b64", EXEC, u16::from(EXEC), u16::from(VCC)),
        sopp("s_cbranch_scc1", -5),
        sop1("s_mov_b64", EXEC, 8),
        vop1("v_mov_b32", 3, 10),
    ];
    let (stored, _) = same_as_the_console(&body, &[1, 3], true, false);
    let own: Vec<u32> = (0..64).map(|l| (l & 7).max(1)).collect();
    assert_eq!(output(&stored, 0, 64), own);
    assert_eq!(output(&stored, 1, 64), vec![7; 64], "the wave's rounds, in every lane");
}

#[test]
fn a_mask_goes_through_numbers_and_back_into_exec() {
    // The even lanes' mask, shifted as two numbers into the lanes two up,
    // becomes EXEC: those lanes write.
    let body = [
        vop2("v_add_nc_u32", 1, k(40), 0),
        vop2("v_and_b32", 2, k(1), 0),
        vopc("v_cmp_eq_u32", k(0), 2),
        sop2("s_lshl_b32", 10, u16::from(VCC), k(3)),
        sop2("s_lshr_b32", 11, u16::from(VCC) + 1, k(2)),
        sop1("s_mov_b64", 8, u16::from(EXEC)),
        sop1("s_mov_b64", EXEC, 10),
        vop1("v_mov_b32", 1, k(9)),
        sop1("s_mov_b64", EXEC, 8),
    ];
    let (stored, _) = same_as_the_console(&body, &[1], true, false);
    let even = 0x5555_5555u32;
    let exec = u64::from(even << 3) | u64::from(even >> 2) << 32;
    let own: Vec<u32> = (0..64).map(|l| if exec >> l & 1 != 0 { 9 } else { l + 40 }).collect();
    assert_eq!(output(&stored, 0, 64), own);
    // As lanes of the host's wave too, a lane of the upper half tests its
    // bit of the high number (the one shifted right by 2), which another
    // instruction wrote than the low one.
    let p = Program::new(&with_stores(body.to_vec(), &[1]), true, [64, 1, 1], false);
    let host = p.lowered(false).unwrap();
    assert_eq!(host.wave_size, Some(64));
    let insts = || host.flow.blocks.iter().flat_map(|b| &b.insts);
    let by_2: Vec<u32> = insts()
        .filter(|i| matches!(i.op, FlowOp::Bin(Bin::And, Arg::Int(2), Arg::Int(31))))
        .map(|i| i.id)
        .collect();
    let shifts_right_by_2 =
        |i: &super::bitcode::FlowInst| matches!(i.op, FlowOp::Bin(Bin::LShr, _, Arg::Inst(n)) if by_2.contains(&n));
    assert!(insts().any(shifts_right_by_2), "the high number is read");
}

#[test]
fn the_first_lane_is_the_lowest_in_exec_or_lane_0() {
    for bound in [20, 45, 63] {
        let body = [
            vop2("v_add_nc_u32", 1, k(50), 0),
            vopc("v_cmp_lt_u32", k(bound), 0),
            sop1("s_and_saveexec_b64", 8, u16::from(VCC)),
            vop1("v_readfirstlane_b32", 2, v(1)),
            sop1("s_mov_b64", EXEC, 8),
            // With every lane in EXEC: lane 0's.
            vop1("v_readfirstlane_b32", 3, v(1)),
            vop1("v_mov_b32", 2, 2),
            vop1("v_mov_b32", 3, 3),
        ];
        let (stored, _) = same_as_the_console(&body, &[2, 3], true, false);
        let first = if bound < 63 { u32::from(bound) + 1 } else { 0 };
        assert_eq!(output(&stored, 0, 64), vec![first + 50; 64], "{bound}");
        assert_eq!(output(&stored, 1, 64), vec![50; 64], "{bound}");
    }
}

#[test]
fn lanes_are_read_and_written_by_number() {
    let mut body = vec![
        vop2("v_add_nc_u32", 1, k(60), 0),
        vop1("v_mov_b32", 3, k(0)),
    ];
    // Lane 41's value into lane 37 of another register; a lane found from
    // a mask (the first lane above 50) read too.
    body.extend(vop3_own("v_readlane_b32", 2, v(1), k(41), 0));
    body.extend(vop3_own("v_writelane_b32", 3, 2, k(37), 0));
    body.push(vopc("v_cmp_lt_u32", k(50), 0));
    body.push(sop1("s_ff1_i32_b64", 10, u16::from(VCC)));
    body.extend(vop3_own("v_readlane_b32", 11, v(1), 10, 0));
    body.push(vop1("v_mov_b32", 4, 11));
    let (stored, _) = same_as_the_console(&body, &[3, 4], true, false);
    let written: Vec<u32> = (0..64).map(|l| if l == 37 { 101 } else { 0 }).collect();
    assert_eq!(output(&stored, 0, 64), written);
    assert_eq!(output(&stored, 1, 64), vec![51 + 60; 64]);
}

#[test]
fn the_count_of_a_masks_lanes_below_each_lane() {
    // The lanes whose number is no multiple of 4, numbered in order: what
    // the console compiler makes of a compaction.
    let mut body = vec![vop2("v_and_b32", 2, k(3), 0), vopc("v_cmp_ne_u32", k(0), 2)];
    body.extend(vop3_own("v_mbcnt_lo_u32_b32", 3, u16::from(VCC), k(0), 0));
    body.extend(vop3_own("v_mbcnt_hi_u32_b32", 3, u16::from(VCC) + 1, v(3), 0));
    let (stored, _) = same_as_the_console(&body, &[3], true, false);
    let below: Vec<u32> = (0..64).map(|l| (0..l).filter(|x| x & 3 != 0).count() as u32).collect();
    assert_eq!(output(&stored, 0, 64), below);
}

#[test]
fn neighbouring_lanes_are_read_as_the_console_reads_them() {
    // Rows shifted right and left by one (with and without the bound, with
    // and without inactive lanes fetched), mirrored, rotated, quads
    // permuted: under EXEC of the lanes whose number is no multiple of 4,
    // and under every lane.
    let patterns = [
        (0x111, false, false),
        (0x111, true, false),
        (0x111, false, true),
        (0x101, true, false),
        (0x101, false, false),
        (0x140, false, false),
        (0x141, true, false),
        (0x123, false, false),
        (0x1B, false, false),
        (0x1B, true, true),
    ];
    for (ctrl, bound, fi) in patterns {
        for narrowed in [false, true] {
            let mut body = vec![
                vop2("v_add_nc_u32", 1, k(33), 0),
                vop2("v_add_nc_u32", 2, k(17), 0),
                sop1("s_mov_b64", 8, u16::from(EXEC)),
            ];
            if narrowed {
                body.push(vop2("v_and_b32", 4, k(3), 0));
                body.push(vopc("v_cmpx_ne_u32", k(0), 4));
            }
            body.push(vop1("v_mov_b32", 2, DPP16));
            body.push(dpp16(1, ctrl, bound, fi));
            body.push(sop1("s_mov_b64", EXEC, 8));
            same_as_the_console(&body, &[2], true, false);
        }
    }
    // Eight lanes at a time by their own table (DPP8).
    let sel = [1u32, 0, 3, 2, 7, 6, 5, 4];
    let word = sel.iter().enumerate().map(|(i, s)| s << (3 * i)).sum::<u32>() << 8 | 1;
    let body = [vop2("v_add_nc_u32", 1, k(33), 0), vop1("v_mov_b32", 2, DPP8), word];
    let (stored, _) = same_as_the_console(&body, &[2], true, false);
    let swapped: Vec<u32> = (0..64).map(|l| ((l & !7) | sel[(l & 7) as usize]) + 33).collect();
    assert_eq!(output(&stored, 0, 64), swapped);
}

#[test]
fn lanes_are_permuted_through_the_shared_memory_instructions() {
    // `ds_swizzle_b32` with a quad pattern (the quad reversed) and with a
    // mask pattern (the lane's number with its lowest bit flipped), and
    // `ds_bpermute_b32` by an address each lane computes (lane * 5 + 3 of
    // its half): under every lane, and under the lanes that are no
    // multiple of 4, where a lane left out gives 0.
    for narrowed in [false, true] {
        for pattern in [Some(0x8000 | 0x1B), Some(1 << 10 | 0x1F), None] {
            let mut body = vec![
                vop2("v_add_nc_u32", 1, k(33), 0),
                vop2("v_add_nc_u32", 2, k(17), 0),
                sop1("s_mov_b64", 8, u16::from(EXEC)),
            ];
            body.extend(vop3_own("v_mul_lo_u32", 6, v(0), k(5), 0));
            body.push(vop2("v_add_nc_u32", 6, k(3), 6));
            body.push(vop2("v_lshlrev_b32", 6, k(2), 6));
            if narrowed {
                body.push(vop2("v_and_b32", 4, k(3), 0));
                body.push(vopc("v_cmpx_ne_u32", k(0), 4));
            }
            match pattern {
                Some(pattern) => body.extend(ds_at("ds_swizzle_b32", pattern, 1, 0, 2)),
                None => body.extend(ds_at("ds_bpermute_b32", 0, 6, 1, 2)),
            }
            body.push(sop1("s_mov_b64", EXEC, 8));
            let (stored, _) = same_as_the_console(&body, &[2], true, false);
            if !narrowed && pattern == Some(1 << 10 | 0x1F) {
                let flipped: Vec<u32> = (0..64).map(|l| (l ^ 1) + 33).collect();
                assert_eq!(output(&stored, 0, 64), flipped);
            }
            if !narrowed && pattern.is_none() {
                let picked: Vec<u32> = (0..64).map(|l| ((l & 32) | (l * 5 + 3) & 31) + 33).collect();
                assert_eq!(output(&stored, 0, 64), picked);
            }
        }
    }
}

#[test]
fn what_one_lane_writes_another_reads_after_they_met() {
    // Each lane writes its dword of shared memory and reads the next
    // lane's: on the console the write is done in every lane before any
    // lane reads. (The console compiler leaves out the barrier the source
    // had there when the group is one wave.)
    let mut body = vec![
        vop2("v_lshlrev_b32", 5, k(2), 0),
        vop2("v_add_nc_u32", 1, k(50), 0),
    ];
    body.extend(ds("ds_write_b32", 5, 1, 0));
    body.push(vop2("v_add_nc_u32", 6, k(1), 0));
    body.push(vop2("v_and_b32", 6, k(63), 6));
    body.push(vop2("v_lshlrev_b32", 6, k(2), 6));
    body.extend(ds("ds_read_b32", 6, 0, 7));
    let (stored, l) = same_as_the_console(&body, &[7], true, true);
    let next: Vec<u32> = (0..64).map(|l| (l + 1) % 64 + 50).collect();
    assert_eq!(output(&stored, 0, 64), next);
    assert_eq!(l.meetings, 1, "the lanes meet between the write and the read, and nowhere else");
}

#[test]
fn stores_keep_the_consoles_order() {
    // Two stores one after the other: the lanes meet between them, so a
    // lane's first store cannot land after another lane's second.
    let body = [vop2("v_add_nc_u32", 1, k(1), 0), vop2("v_add_nc_u32", 2, k(2), 0)];
    let (_, l) = same_as_the_console(&body, &[1, 2], true, false);
    assert_eq!(l.meetings, 1);
}

/// A scalar load, then a branch on what it loaded.
fn loads() -> Vec<u32> {
    vec![
        0x3D << 26 | op(opcodes::SMEM, "s_buffer_load_dword") << 18 | 1 << 17 | 2 << 6 | (12 >> 1),
        8 | 125 << 25,
        vop1("v_mov_b32", 1, 2),
        sopc("s_cmp_eq_u32", 2, k(0)),
        sopp("s_cbranch_scc1", 1),
        vop2("v_add_nc_u32", 1, k(64), 1),
    ]
}

#[test]
fn a_scalar_load_is_one_load_for_the_wave() {
    let threads = [64, 1, 1];
    let p = Program::new(&with_stores(loads(), &[1]), true, threads, false);
    // The memory changes while the threads load: each thread reads
    // something else, some of them 0 (the other side of the branch).
    let changing = |at: u32, t: usize| at + (t as u32 % 3) * 5;
    let l = p.lowered(true).unwrap();
    for schedule in SCHEDULES {
        let mut got = Stored::new();
        Group::new(&l.flow, 64)
            .run(schedule, &mut host(threads, &changing, &mut got, false))
            .unwrap_or_else(|e| panic!("{schedule:?}: {e}"));
        // What the console's wave stores when its one load gives what lane
        // 0 read.
        assert_eq!(got, Console::run(&p, |at| at), "{schedule:?}");
    }
    // A wave that needs nothing of its lanes but shares no load either
    // would take both sides of the branch: threads would end apart.
    let unshared = p.lowered(false).unwrap();
    assert_eq!(unshared.wave_size, None, "the program asks nothing of the host's wave");
}

#[test]
fn a_wave_of_32_lanes_is_emulated_too() {
    let body = [
        sop1("s_mov_b32", 10, k(0)),
        vop1("v_mov_b32", 1, v(0)),
        vopc("v_cmp_lt_u32", k(20), 0),
        sop1("s_bcnt1_i32_b32", 11, u16::from(VCC)),
        sop1("s_and_saveexec_b32", 8, u16::from(VCC)),
        sopp("s_cbranch_execz", 2),
        vop2("v_add_nc_u32", 1, k(7), 1),
        sop2("s_add_u32", 10, 10, k(1)),
        sop2("s_or_b32", EXEC, u16::from(EXEC), 8),
        vop1("v_mov_b32", 2, 10),
        vop1("v_mov_b32", 3, 11),
        vop1("v_readfirstlane_b32", 12, v(1)),
        vop1("v_mov_b32", 4, 12),
    ];
    let (stored, _) = same_as_the_console(&body, &[1, 2, 3, 4], false, false);
    let own: Vec<u32> = (0..32).map(|l| if l > 20 { l + 7 } else { l }).collect();
    assert_eq!(output(&stored, 0, 32), own);
    assert_eq!(output(&stored, 1, 32), vec![1; 32]);
    assert_eq!(output(&stored, 2, 32), vec![11; 32]);
    assert_eq!(output(&stored, 3, 32), vec![0; 32]);
}

/// A packet walk in small: the wave visits nodes 5 down to 1. A lane
/// "hits" node n when bit n of its number is set. The lanes that hit go
/// into the node (EXEC narrows, the wave skips a node no lane hits) and add
/// its number; the wave counts its hits in a scalar from the hit mask read
/// as a number, and keeps the last non-empty hit mask as two numbers, which
/// become EXEC once the walk is over. Its outputs are v1, v3 and v5.
fn walk() -> Vec<u32> {
    vec![
        sop1("s_mov_b32", 20, k(5)),
        sop1("s_mov_b32", 21, k(0)),
        sop1("s_mov_b32", 22, k(0)),
        sop1("s_mov_b32", 23, k(0)),
        vop1("v_mov_b32", 1, k(0)),
        vop1("v_mov_b32", 5, k(0)),
        // the walk:
        vop2("v_lshrrev_b32", 2, 20, 0),
        vop2("v_and_b32", 2, k(1), 2),
        vopc("v_cmp_ne_u32", k(0), 2),
        sop1("s_bcnt1_i32_b64", 24, u16::from(VCC)),
        sop2("s_add_u32", 21, 21, 24),
        sop1("s_and_saveexec_b64", 8, u16::from(VCC)),
        sopp("s_cbranch_execz", 3),
        vop2("v_add_nc_u32", 1, 20, 1),
        sop1("s_mov_b32", 22, u16::from(EXEC)),
        sop1("s_mov_b32", 23, u16::from(EXEC) + 1),
        sop2("s_or_b64", EXEC, u16::from(EXEC), 8),
        sop2("s_sub_u32", 20, 20, k(1)),
        sopc("s_cmp_lg_u32", 20, k(0)),
        sopp("s_cbranch_scc1", -14),
        // The popped mask as EXEC: those lanes mark themselves.
        sop1("s_mov_b64", 8, u16::from(EXEC)),
        sop1("s_mov_b64", EXEC, 22),
        vop1("v_mov_b32", 5, k(1)),
        sop1("s_mov_b64", EXEC, 8),
        vop1("v_mov_b32", 3, 21),
    ]
}

/// Why the walk has no per-lane form: the mask that comes back into EXEC
/// is two numbers joined with numbers at the loop's header.
const WALK_PER_LANE: &str = "a lane-mask join of an untracked mask";

#[test]
fn a_walk_pushes_and_pops_lane_masks_with_node_numbers() {
    let (stored, l) = same_as_the_console(&walk(), &[1, 3, 5], true, false);
    let sums: Vec<u32> = (0..64u32).map(|l| (1..=5).filter(|n| l >> n & 1 != 0).sum()).collect();
    assert_eq!(output(&stored, 0, 64), sums);
    assert_eq!(output(&stored, 1, 64), vec![5 * 32; 64], "32 lanes hit each of the 5 nodes");
    // Node 1 is the last: the lanes with bit 1 of their number set.
    let last: Vec<u32> = (0..64).map(|l| l >> 1 & 1).collect();
    assert_eq!(output(&stored, 2, 64), last);
    assert!(l.meetings > 0);
    // As lanes of the host's wave the walk does not translate at all.
    let p = Program::new(&with_stores(walk(), &[1, 3, 5]), true, [64, 1, 1], false);
    match p.lowered(false) {
        Err(ShaderError::Unsupported { what, .. }) => assert_eq!(what, WALK_PER_LANE),
        other => panic!("the walk as host lanes: {:?}", other.map(|l| l.wave_size)),
    }
}

#[test]
fn only_a_group_of_exactly_one_wave_is_emulated() {
    for (threads, why) in [
        ([128, 1, 1], "an emulated wave in a thread group of several waves"),
        ([8, 8, 2], "an emulated wave in a thread group of several waves"),
        ([32, 1, 1], "an emulated wave in a thread group that does not fill it"),
        ([1, 1, 1], "an emulated wave in a thread group that does not fill it"),
        // (A size no group has: counted without overflow.)
        ([u32::MAX, u32::MAX, 4], "an emulated wave in a thread group of several waves"),
    ] {
        let p = Program::new(&with_stores(skip(5), &[1, 2]), true, threads, false);
        match p.lowered(true) {
            Err(ShaderError::Unsupported { what, .. }) => assert_eq!(what, why, "{threads:?}"),
            other => panic!("{threads:?}: {:?}", other.map(|l| l.emulated)),
        }
    }
    // A group of 8 by 8 is one wave of 64, thread (x, y) its lane 8y + x:
    // every lane stores the count of the lanes above 40 at its own place.
    let mut body = vec![
        vop2("v_lshlrev_b32", 3, k(3), 1),
        vop2("v_add_nc_u32", 3, v(0), 3),
        vopc("v_cmp_lt_u32", k(40), 3),
        sop1("s_bcnt1_i32_b64", 2, u16::from(VCC)),
        vop1("v_mov_b32", 4, 2),
        vop2("v_lshlrev_b32", 15, k(2), 3),
    ];
    body.extend(store(4, 15, 0));
    let l = Program::new(&body, true, [8, 8, 1], false).lowered(true).unwrap();
    for schedule in SCHEDULES {
        let mut got = Stored::new();
        Group::new(&l.flow, 64).run(schedule, &mut host([8, 8, 1], &|_, _| 0, &mut got, false)).unwrap();
        let want: Stored = (0..64).map(|lane| (4 * lane, (23, lane))).collect();
        assert_eq!(got, want, "{schedule:?}");
    }
}

/// Every lane takes the count of the lanes above 5: a mask read as a
/// number.
fn counted() -> Vec<u32> {
    vec![
        vopc("v_cmp_lt_u32", k(5), 0),
        sop1("s_bcnt1_i32_b64", 2, u16::from(VCC)),
        vop1("v_mov_b32", 1, 2),
        vop1("v_mov_b32", 2, v(0)),
    ]
}

/// The translation as a caller gets it: `body` in a container with no
/// signature parts.
fn translated(body: &[u32], threads: [u32; 3], options: Options) -> super::HostShader {
    let p = Program::new(&with_stores(body.to_vec(), &[1, 2]), true, threads, false);
    emitted(&p, options).unwrap()
}

/// `p` through the translation's entry for a program whose earlier stages
/// ran ([`super::emit_with`]).
fn emitted(p: &Program, options: Options) -> Result<super::HostShader, ShaderError> {
    let native = &p.native;
    let bytes = crate::dxbc::build(&[]);
    let c = crate::dxbc::Container::parse(&bytes).unwrap();
    let ir = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
    let st = structure::structure(&ir).unwrap();
    let res = resource::track(&ir, &native.layout, native.stage, Some(&st)).unwrap();
    let io = io::raise(&ir, native).unwrap();
    super::emit_with(&c, native, &ir, &st, &io, &res, options)
}

#[test]
fn a_program_without_a_per_lane_form_is_given_its_emulated_wave_when_asked() {
    let asked = Options { emulated_waves: emulate::WAVE64, ..Options::default() };
    let p = Program::new(&with_stores(walk(), &[1, 3, 5]), true, [64, 1, 1], false);
    // By default, and asked for the other wave size only: the per-lane
    // form's refusal, as before the switch existed.
    let refusal = match emitted(&p, Options::default()) {
        Err(e @ ShaderError::Unsupported { what: WALK_PER_LANE, .. }) => e,
        other => panic!("{:?}", other.map(|h| h.wave)),
    };
    let other = Options { emulated_waves: emulate::WAVE32, ..Options::default() };
    assert_eq!(emitted(&p, other).unwrap_err(), refusal);
    // Asked for: the module the tests above run against the console's wave,
    // with the reason there is no other.
    let host = emitted(&p, asked).unwrap();
    assert_eq!((host.wave.size, host.wave.emulated), (0, true));
    assert_eq!(host.wave.needs, wave_need::MASK_READ);
    assert_eq!((host.wave.refusal.as_str(), host.wave.per_lane.as_str()), ("", WALK_PER_LANE));
    assert_eq!(host.dxil, p.lowered(true).unwrap().dxil);
    // Where the wave form cannot be made either (a group of two waves),
    // the translation's refusal is the per-lane form's.
    let two = Program::new(&with_stores(walk(), &[1, 3, 5]), true, [128, 1, 1], false);
    assert_eq!(emitted(&two, asked).unwrap_err(), refusal);
    // The wave form alone, for a caller that reports both: its own refusal.
    let alone = |p: &Program| {
        let native = &p.native;
        let bytes = crate::dxbc::build(&[]);
        let c = crate::dxbc::Container::parse(&bytes).unwrap();
        let ir = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
        let io = io::raise(&ir, native).unwrap();
        super::emit_emulated(&c, native, &ir, &io, Options::default())
    };
    let made = alone(&p).unwrap();
    assert_eq!((made.wave.emulated, made.wave.per_lane.as_str()), (true, ""));
    assert_eq!(made.dxil, host.dxil);
    assert_eq!(
        alone(&two).unwrap_err(),
        ShaderError::Unsupported { offset: 0, what: "an emulated wave in a thread group of several waves" }
    );
}

#[test]
fn a_program_whose_per_lane_structure_is_refused_is_given_its_emulated_wave_when_asked() {
    // The stages before the lowering, as `translate_with` hands them on.
    // (No program of these tests has a wave form and no per-lane
    // structure, so the structure's refusal is put in by hand: what is
    // tested is what the translation does with one.)
    let asked = Options { emulated_waves: emulate::WAVE64, ..Options::default() };
    let no_structure = || ShaderError::Unsupported { offset: 0x40, what: "a structure refused for the test" };
    let no_resources = || ShaderError::Unsupported { offset: 0x20, what: "resources refused for the test" };
    let no_io = || ShaderError::Unsupported { offset: 0x30, what: "I/O refused for the test" };
    let stages = |threads: [u32; 3], failed: [bool; 3], options: Options| {
        let p = Program::new(&with_stores(counted(), &[1, 2]), true, threads, false);
        let native = &p.native;
        let bytes = crate::dxbc::build(&[]);
        let c = crate::dxbc::Container::parse(&bytes).unwrap();
        let ir = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
        let st = structure::structure(&ir).unwrap();
        let res = resource::track(&ir, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&ir, native).unwrap();
        let [no_res, no_i, no_st] = failed;
        super::emit_stages(
            &c,
            native,
            &ir,
            if no_st { Err(no_structure()) } else { Ok(st) },
            if no_i { Err(no_io()) } else { Ok(io) },
            if no_res { Err(no_resources()) } else { Ok(res) },
            options,
        )
    };
    let one = [64, 1, 1];
    // Every stage done: the translation as `emit_with` makes it.
    assert_eq!(stages(one, [false; 3], asked).unwrap(), translated(&counted(), one, asked));
    // No structure. Not asked: its refusal, after the resources' (which
    // are traced without it then). Asked: the emulated wave, with the
    // reason, whatever became of the per-lane form's resources.
    assert_eq!(stages(one, [false, false, true], Options::default()).unwrap_err(), no_structure());
    assert_eq!(stages(one, [true, false, true], Options::default()).unwrap_err(), no_resources());
    for no_res in [false, true] {
        let host = stages(one, [no_res, false, true], asked).unwrap();
        assert_eq!((host.wave.size, host.wave.emulated), (0, true));
        assert_eq!(host.wave.per_lane, "a structure refused for the test");
        assert_eq!(host.dxil, translated(&counted(), one, asked).dxil);
    }
    // Asked, and the wave form cannot be made either: as when not asked.
    let two = [128, 1, 1];
    assert_eq!(stages(two, [false, false, true], asked).unwrap_err(), no_structure());
    assert_eq!(stages(two, [true, false, true], asked).unwrap_err(), no_resources());
    // A failure that is no matter of the form stays what it is: the I/O's
    // (both forms need it), and the resources' where the structure stands.
    assert_eq!(stages(one, [false, true, true], asked).unwrap_err(), no_io());
    assert_eq!(stages(one, [true, true, true], asked).unwrap_err(), no_resources());
    assert_eq!(stages(one, [true, false, false], asked).unwrap_err(), no_resources());
}

#[test]
fn a_translation_says_what_it_asks_of_the_hosts_waves() {
    let asked = Options { emulated_waves: emulate::WAVE64, ..Options::default() };
    // By default: the console's wave size, and why.
    let host = translated(&counted(), [64, 1, 1], Options::default());
    assert_eq!((host.wave.size, host.wave.emulated), (64, false));
    assert_eq!(host.wave.needs, wave_need::MASK_READ);
    assert_eq!(super::wave_needs_text(host.wave.needs), "mask read as a number");
    assert_eq!(host.wave.refusal, "");
    // Asked for: no wave size; the reasons stay.
    let emulated = translated(&counted(), [64, 1, 1], asked);
    assert_eq!((emulated.wave.size, emulated.wave.emulated), (0, true));
    assert_eq!(emulated.wave.needs, wave_need::MASK_READ);
    assert_ne!(emulated.dxil, host.dxil);
    // Asked for the other wave size only: as by default.
    let other = Options { emulated_waves: emulate::WAVE32, ..Options::default() };
    assert_eq!(translated(&counted(), [64, 1, 1], other), host);
    // Asked for where it cannot be done: the host's wave, with the reason.
    let refused = translated(&counted(), [128, 1, 1], asked);
    assert_eq!((refused.wave.size, refused.wave.emulated), (64, false));
    assert_eq!(refused.wave.refusal, "an emulated wave in a thread group of several waves");
    assert_eq!(refused.dxil, translated(&counted(), [128, 1, 1], Options::default()).dxil);
    // A program whose lanes keep to themselves asks nothing either way.
    let own = [vop2("v_add_nc_u32", 1, k(1), 0), vop2("v_add_nc_u32", 2, k(2), 0)];
    for options in [Options::default(), asked] {
        let plain = translated(&own, [64, 1, 1], options);
        assert_eq!(plain.wave, super::WaveUse::default());
    }
    assert_eq!(super::wave_needs_text(0), "none");
    assert_eq!(
        super::wave_needs_text(wave_need::DPP | wave_need::SHARED_MEMORY),
        "DPP + shared memory within one wave"
    );
}

#[test]
fn the_group_refuses_what_a_host_would_not_forgive() {
    // A barrier only some threads reach, and a read of what nothing wrote:
    // the checks the other tests rely on do fire.
    use super::bitcode::{Attr, Module, Term};
    let mut m = Module::default();
    let (void, i32t) = (m.void(), m.int(32));
    let main_ty = m.func_ty(void, vec![]);
    m.ptr(main_ty);
    m.function("main", main_ty, None, true);
    let entry = m.new_block();
    let id_ty = m.func_ty(i32t, vec![i32t]);
    m.ptr(id_ty);
    let id = m.function("dx.op.flattenedThreadIdInGroup.i32", id_ty, Some(Attr::ReadNone), false);
    let barrier_ty = m.func_ty(void, vec![i32t, i32t]);
    m.ptr(barrier_ty);
    let barrier = m.function("dx.op.barrier", barrier_ty, Some(Attr::NoDuplicate), false);
    let (k96, k80, k9, zero) = (m.i32(96), m.i32(80), m.i32(9), m.i32(0));
    let t = m.call(entry, id, vec![k96]);
    let first = m.cmp(entry, pred::EQ, t, zero);
    let (waits, ends) = (m.new_block(), m.new_block());
    m.terminate(entry, Term::CondBr(first, ends, waits));
    m.call(waits, barrier, vec![k80, k9]);
    m.terminate(waits, Term::Br(ends));
    m.terminate(ends, Term::Ret);
    let flow = m.flow();
    flow.check().unwrap();
    let mut answer = |t: usize, _: &str, _: &[u64]| vec![t as u64];
    let hung = Group::new(&flow, 4).run(Schedule::Waves(2), &mut answer);
    assert_eq!(hung, Err("3 threads wait at a barrier that 1 threads never reach".into()));

    let mut m = Module::default();
    let (void, i32t) = (m.void(), m.int(32));
    let main_ty = m.func_ty(void, vec![]);
    m.ptr(main_ty);
    m.function("main", main_ty, None, true);
    let entry = m.new_block();
    let array = m.array(4, i32t);
    let shared = m.global("lds", array, 3);
    let (one, bit) = (m.i32(1), m.i32(4));
    let cell = m.element(entry, shared, one);
    m.atomic(entry, Rmw::Or, cell, bit);
    let x = m.load(entry, cell);
    m.store(entry, cell, x);
    m.terminate(entry, Term::Ret);
    let unwritten = Group::new(&m.flow(), 1).run(Schedule::LastFirst, &mut answer);
    assert!(unwritten.unwrap_err().contains("reads shared memory bits nothing wrote (0x4 written)"));
}

#[test]
fn what_one_lane_read_another_overwrites_after_they_met() {
    // An update in place: each lane writes its dword of shared memory,
    // reads the next lane's, and writes what it read plus 60 over its own.
    // On the console every lane has read before any lane overwrites, so
    // each reads what the first write left.
    let mut body = vec![
        vop2("v_lshlrev_b32", 5, k(2), 0),
        vop2("v_add_nc_u32", 1, k(50), 0),
    ];
    body.extend(ds("ds_write_b32", 5, 1, 0));
    body.push(vop2("v_add_nc_u32", 6, k(1), 0));
    body.push(vop2("v_and_b32", 6, k(63), 6));
    body.push(vop2("v_lshlrev_b32", 6, k(2), 6));
    body.extend(ds("ds_read_b32", 6, 0, 7));
    body.push(vop2("v_add_nc_u32", 8, k(60), 7));
    body.extend(ds("ds_write_b32", 5, 8, 0));
    body.extend(ds("ds_read_b32", 5, 0, 9));
    let (stored, l) = same_as_the_console(&body, &[7, 9], true, true);
    let next: Vec<u32> = (0..64).map(|l| (l + 1) % 64 + 50).collect();
    assert_eq!(output(&stored, 0, 64), next);
    let own: Vec<u32> = next.iter().map(|x| x + 60).collect();
    assert_eq!(output(&stored, 1, 64), own);
    // Before the first read, before the overwrite, before the last read;
    // and between the two stores of the outputs.
    assert_eq!(l.meetings, 4);
}

/// Where the tests of a buffer's memory order work in the output buffer:
/// past the outputs' places.
const WORK: u32 = 0x800;

#[test]
fn what_one_lane_stores_another_loads_after_they_met() {
    // As in shared memory, in a buffer: each lane stores at its own place
    // and loads from the next lane's.
    let mut body = vec![
        vop2("v_lshlrev_b32", 5, k(2), 0),
        vop2("v_add_nc_u32", 1, k(50), 0),
    ];
    body.extend(store(1, 5, WORK));
    body.push(vop2("v_add_nc_u32", 6, k(1), 0));
    body.push(vop2("v_and_b32", 6, k(63), 6));
    body.push(vop2("v_lshlrev_b32", 6, k(2), 6));
    body.extend(load(7, 6, WORK));
    let (stored, l) = same_as_the_console(&body, &[7], true, false);
    let next: Vec<u32> = (0..64).map(|l| (l + 1) % 64 + 50).collect();
    assert_eq!(output(&stored, 0, 64), next);
    // Before the load, and before the store of what was loaded.
    assert_eq!(l.meetings, 2);
}

#[test]
fn what_one_lane_loaded_another_stores_over_after_they_met() {
    // An update in place in a buffer: store, load the next lane's, store
    // that plus 60 over the lane's own, load it back.
    let mut body = vec![
        vop2("v_lshlrev_b32", 5, k(2), 0),
        vop2("v_add_nc_u32", 1, k(50), 0),
    ];
    body.extend(store(1, 5, WORK));
    body.push(vop2("v_add_nc_u32", 6, k(1), 0));
    body.push(vop2("v_and_b32", 6, k(63), 6));
    body.push(vop2("v_lshlrev_b32", 6, k(2), 6));
    body.extend(load(7, 6, WORK));
    body.push(vop2("v_add_nc_u32", 8, k(60), 7));
    body.extend(store(8, 5, WORK));
    body.extend(load(9, 5, WORK));
    let (stored, _) = compared(&body, &[7, 9], true, false, true);
    let next: Vec<u32> = (0..64).map(|l| (l + 1) % 64 + 50).collect();
    assert_eq!(output(&stored, 0, 64), next);
    let own: Vec<u32> = next.iter().map(|x| x + 60).collect();
    assert_eq!(output(&stored, 1, 64), own);
    // The buffer holds each lane's second store.
    let held: Vec<u32> = (0..64).map(|l| stored[&(WORK + 4 * l)].0).collect();
    assert_eq!(held, own);
}

#[test]
fn the_later_of_two_stores_at_one_place_stays() {
    // Each lane stores at its own place, then at the next lane's: every
    // place is stored twice, by two lanes, and holds the second store.
    let mut body = vec![
        vop2("v_lshlrev_b32", 5, k(2), 0),
        vop2("v_add_nc_u32", 1, k(50), 0),
        vop2("v_add_nc_u32", 2, k(60), 1),
        vop2("v_add_nc_u32", 2, k(60), 2),
    ];
    body.extend(store(1, 5, WORK));
    body.push(vop2("v_add_nc_u32", 6, k(1), 0));
    body.push(vop2("v_and_b32", 6, k(63), 6));
    body.push(vop2("v_lshlrev_b32", 6, k(2), 6));
    body.extend(store(2, 6, WORK));
    let (stored, l) = compared(&body, &[], true, false, true);
    for lane in 0..64u32 {
        let from = (lane + 63) % 64;
        assert_eq!(stored[&(WORK + 4 * lane)], (from + 170, from), "{lane}");
    }
    assert_eq!(l.meetings, 1);
}

#[test]
fn memory_order_is_kept_round_a_loop() {
    // Two rounds of: every lane adds the next lane's dword of shared
    // memory to its own. The lanes have met when the loop is entered and
    // nothing is written since, so only the way round the loop puts a
    // round's read after a write (the round before's).
    let mut body = vec![
        vop2("v_lshlrev_b32", 5, k(2), 0),
        vop2("v_add_nc_u32", 6, k(1), 0),
        vop2("v_and_b32", 6, k(63), 6),
        vop2("v_lshlrev_b32", 6, k(2), 6),
        vop2("v_add_nc_u32", 1, k(1), 0),
        sop1("s_mov_b32", 10, k(2)),
    ];
    body.extend(ds("ds_write_b32", 5, 1, 0));
    body.extend(ds("ds_read_b32", 5, 0, 8));
    // the loop:
    body.extend(ds("ds_read_b32", 6, 0, 7));
    body.push(vop2("v_add_nc_u32", 8, v(7), 8));
    body.extend(ds("ds_write_b32", 5, 8, 0));
    body.push(sop2("s_sub_u32", 10, 10, k(1)));
    body.push(sopc("s_cmp_lg_u32", 10, k(0)));
    body.push(sopp("s_cbranch_scc1", -8));
    body.extend(ds("ds_read_b32", 5, 0, 9));
    let (stored, _) = same_as_the_console(&body, &[9], true, true);
    // Lane l starts at l + 1; a round makes it x[l] + x[l + 1].
    let mut x: Vec<u32> = (1..=64).collect();
    for _ in 0..2 {
        x = (0..64).map(|l| x[l] + x[(l + 1) % 64]).collect();
    }
    assert_eq!(output(&stored, 0, 64), x);
}

#[test]
fn a_scalar_read_back_from_its_lane_is_told_from_a_lane_read() {
    // A scalar put in lane 5 of a register and read back from there, as
    // the console compiler parks scalars: no lane is read.
    let mut parked = vec![sop1("s_mov_b32", 10, k(7)), vop1("v_mov_b32", 3, k(0))];
    parked.extend(vop3_own("v_writelane_b32", 3, 10, k(5), 0));
    parked.extend(vop3_own("v_readlane_b32", 11, v(3), k(5), 0));
    parked.push(vop1("v_mov_b32", 4, 11));
    let (stored, l) = same_as_the_console(&parked, &[3, 4], true, false);
    assert_eq!(output(&stored, 1, 64), vec![7; 64]);
    assert_eq!(l.wave_needs, wave_need::LANE_WRITE | wave_need::RESTORED);
    // As lanes of the host's wave it still declares the console's wave
    // size: the write takes the lane's number from the host's wave. The
    // reasons say that this is all it takes.
    let lowered = |body: &[u32]| {
        let p = Program::new(&with_stores(body.to_vec(), &[3, 4]), true, [64, 1, 1], false);
        p.lowered(false).unwrap()
    };
    let host = lowered(&parked);
    assert_eq!(host.wave_size, Some(64));
    assert_eq!(host.wave_needs, wave_need::LANE_WRITE | wave_need::RESTORED);
    assert!(!calls(&host.flow, "waveReadLaneAt"));
    assert_eq!(
        super::wave_needs_text(host.wave_needs),
        "lane write + scalar read back from its lane"
    );
    // Another lane of that register read is a lane read.
    let mut read = parked.clone();
    read.extend(vop3_own("v_readlane_b32", 12, v(3), k(6), 0));
    read.push(vop1("v_mov_b32", 4, 12));
    let host = lowered(&read);
    assert_eq!(host.wave_needs, wave_need::LANE_WRITE | wave_need::RESTORED | wave_need::LANE_READ);
    assert!(calls(&host.flow, "waveReadLaneAt"));
}

#[test]
fn a_low_dword_read_as_a_mask_with_two_high_dwords_is_refused() {
    // The even lanes' mask as two numbers becomes EXEC twice, the second
    // time with another high number beside the same low one. A mask held
    // as numbers is known by its low number, which then stands for two
    // masks: neither form translates it, and says why. (Before the high
    // number was followed, the lanes 32 to 63 tested the low one here.)
    let body = [
        vop2("v_add_nc_u32", 1, k(40), 0),
        vop2("v_and_b32", 2, k(1), 0),
        vopc("v_cmp_eq_u32", k(0), 2),
        sop2("s_lshl_b32", 10, u16::from(VCC), k(3)),
        sop2("s_lshr_b32", 11, u16::from(VCC) + 1, k(2)),
        sop1("s_mov_b64", 8, u16::from(EXEC)),
        sop1("s_mov_b64", EXEC, 10),
        vop1("v_mov_b32", 1, k(9)),
        sop2("s_lshr_b32", 11, u16::from(VCC) + 1, k(1)),
        sop1("s_mov_b64", EXEC, 10),
        vop1("v_mov_b32", 3, k(7)),
        sop1("s_mov_b64", EXEC, 8),
    ];
    let p = Program::new(&with_stores(body.to_vec(), &[1, 3]), true, [64, 1, 1], false);
    for waves in [false, true] {
        match p.lowered(waves) {
            Err(ShaderError::Unsupported { what, .. }) => {
                assert_eq!(what, "a lane mask's low dword read with two high dwords", "{waves}");
            }
            other => panic!("{waves}: {:?}", other.map(|l| l.wave_size)),
        }
    }
}

// ---- the loop limits and the thread limits ----

/// What the emulated wave of `p` stores when its loops are counted against
/// `limits`, under every schedule: the same in all of them, with no thread
/// left at a meeting the others never reach ([`Group::run`] refuses that).
/// With it, whether the lowering has a block the limits stop lanes through.
fn stored_under(p: &Program, limits: Limits) -> (Stored, bool) {
    let l = p.lowered_limited(true, limits, false).unwrap();
    assert!(l.emulated && l.wave_size.is_none());
    let (lanes, threads) = (p.wave(), [p.wave(), 1, 1]);
    let mut first: Option<(Stored, u64)> = None;
    for schedule in SCHEDULES {
        let mut got = Stored::new();
        let mut group = Group::new(&l.flow, lanes as usize);
        group
            .run(schedule, &mut host(threads, &|_, _| 0, &mut got, false))
            .unwrap_or_else(|e| panic!("{limits:?} {schedule:?}: {e}"));
        let ran = (got, group.barriers);
        // The same stores and as many meetings however the host runs it.
        assert_eq!(*first.get_or_insert_with(|| ran.clone()), ran, "{limits:?} {schedule:?}");
    }
    (first.expect("a schedule").0, l.limit_stop.is_some())
}

/// The loop of [`a_loop_runs_until_no_lane_is_left_and_scalars_count_for_the_wave`],
/// `rounds` times over: each lane counts up to (its number & 7), at least
/// once, in v1; v4 counts a lane's steps over all rounds and s10 the
/// wave's. The wave goes round the inner loop seven times a round, so it
/// takes six back edges there each round and `rounds - 1` of the outer
/// loop: for three rounds, twenty in all.
fn nest(rounds: u8) -> Vec<u32> {
    vec![
        sop1("s_mov_b32", 11, k(rounds)),
        sop1("s_mov_b32", 10, k(0)),
        vop1("v_mov_b32", 4, k(0)),
        vop2("v_and_b32", 2, k(7), 0),
        sop1("s_mov_b64", 8, u16::from(EXEC)),
        // the outer loop:
        vop1("v_mov_b32", 1, k(0)),
        // the inner loop:
        vop2("v_add_nc_u32", 1, k(1), 1),
        vop2("v_add_nc_u32", 4, k(1), 4),
        sop2("s_add_u32", 10, 10, k(1)),
        vopc("v_cmp_lt_u32", v(1), 2),
        sop2("s_and_b64", EXEC, u16::from(EXEC), u16::from(VCC)),
        sopp("s_cbranch_scc1", -6),
        sop1("s_mov_b64", EXEC, 8),
        sop2("s_sub_u32", 11, 11, k(1)),
        sopc("s_cmp_lg_u32", 11, k(0)),
        sopp("s_cbranch_scc1", -11),
        vop1("v_mov_b32", 3, 10),
    ]
}

/// The loop limits in an emulated wave: every thread counts its own back
/// edges, and they count alike, because every thread takes the wave's
/// branches. A loop that stays below both limits runs as on the console; at
/// a limit the whole wave is stopped at one back edge (no thread waits at
/// a meeting for one that left) and stores nothing after it.
#[test]
fn a_loop_in_an_emulated_wave_counts_against_both_limits() {
    let p = Program::new(&with_stores(nest(1), &[4, 3]), true, [64, 1, 1], false);
    let want = Console::run(&p, |_| 0);
    assert_eq!(output(&want, 1, 64), vec![7; 64], "seven rounds: six back edges");
    // Six back edges are taken below a limit of seven, of either kind.
    for limits in [Limits { each: 7, total: 1 << 22 }, Limits { each: 1 << 20, total: 7 }] {
        let (stored, can_stop) = stored_under(&p, limits);
        assert!(can_stop, "{limits:?}");
        assert_eq!(stored, want, "{limits:?}");
    }
    // At six, the sixth is not taken: every lane is stopped there, also
    // the lanes whose own count ended rounds before.
    for limits in [Limits { each: 6, total: 1 << 22 }, Limits { each: 1 << 20, total: 6 }] {
        let (stored, _) = stored_under(&p, limits);
        assert_eq!(stored, Stored::new(), "{limits:?}");
    }
}

/// A loop inside another, in an emulated wave: the inner loop's count
/// starts again each time the outer loop comes round to it, the outer
/// loop's count is its own, and the lane's count of all its back edges
/// goes on through both, as in the per-lane form (`lower/limit.rs`).
#[test]
fn a_nest_in_an_emulated_wave_counts_each_loop_and_the_lanes_total() {
    let p = Program::new(&with_stores(nest(3), &[4, 3]), true, [64, 1, 1], false);
    let want = Console::run(&p, |_| 0);
    let steps: Vec<u32> = (0..64).map(|l| 3 * (l & 7).max(1)).collect();
    assert_eq!(output(&want, 0, 64), steps);
    assert_eq!(output(&want, 1, 64), vec![21; 64], "the wave's rounds of the inner loop");
    // Six back edges an entry into the inner loop, two of the outer loop,
    // twenty in all: below seven a loop and twenty-one for the lane.
    let (stored, _) = stored_under(&p, Limits { each: 7, total: 21 });
    assert_eq!(stored, want);
    // (The inner loop's count did not go on from one round to the next:
    // eighteen back edges of it passed a limit of seven.)
    // The lane's total stops the wave at its twentieth back edge, the last
    // of the inner loop; a loop's own limit stops it in the first round.
    for limits in [Limits { each: 7, total: 20 }, Limits { each: 6, total: 1 << 22 }] {
        let (stored, _) = stored_under(&p, limits);
        assert_eq!(stored, Stored::new(), "{limits:?}");
    }
    // The outer loop's two back edges count against its own limit.
    let (stored, _) = stored_under(&p, Limits { each: 7, total: 1 << 22 });
    assert_eq!(stored, want);
    let two = Program::new(&with_stores(nest(8), &[4, 3]), true, [64, 1, 1], false);
    let (stored, _) = stored_under(&two, Limits { each: 7, total: 1 << 22 });
    assert_eq!(stored, Stored::new(), "seven back edges of the outer loop");
    let (stored, _) = stored_under(&two, Limits { each: 8, total: 1 << 22 });
    assert_eq!(stored, Console::run(&two, |_| 0));
}

/// Thread limits (`emit`, "Thread limits") and an emulated wave do not go
/// together: a thread that left at entry would never reach the lanes'
/// meetings. Asked for both, the wave form is lowered exactly as without
/// thread limits and the host is told of no limit to fill, so it does not
/// run a dispatch of the program that fills no whole groups.
#[test]
fn an_emulated_wave_takes_no_thread_limits() {
    let limit = |u: &super::UserData| u.root == RootType::Other(USER_THREAD_LIMIT);
    let p = Program::new(&with_stores(counted(), &[1, 2]), true, [64, 1, 1], false);
    let in_force = Limits { each: super::loop_limit(), total: super::loop_total_limit() };
    let plain = p.lowered(true).unwrap();
    let asked = p.lowered_limited(true, in_force, true).unwrap();
    assert!(asked.emulated && !asked.user_data.iter().any(limit));
    assert_eq!(asked.dxil, plain.dxil);
    // The per-lane form of the same program does take one, along x.
    let per_lane = p.lowered_limited(false, in_force, true).unwrap();
    assert_eq!(per_lane.user_data.iter().filter(|u| limit(u)).count(), 1);
    // Through the translation's entry: with both options the emulated
    // wave, without a limit; with thread limits alone the per-lane form
    // with its limit.
    let both = Options { thread_limits: true, emulated_waves: emulate::WAVE64, ..Options::default() };
    let host = emitted(&p, both).unwrap();
    assert!(host.wave.emulated && !host.user_data.iter().any(limit));
    assert_eq!(host.dxil, plain.dxil);
    let threads = Options { thread_limits: true, ..Options::default() };
    let host = emitted(&p, threads).unwrap();
    assert_eq!((host.wave.size, host.wave.emulated), (64, false));
    assert_eq!(host.user_data.iter().filter(|u| limit(u)).count(), 1);
}
