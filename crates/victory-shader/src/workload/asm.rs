//! A small GFX10 assembler for the synthetic programs of this module: each
//! instruction by AMD's name, encoded as `gfx10` decodes it, with labels
//! for branches. It knows only the forms those programs use.

use std::collections::HashMap;

use crate::gfx10::opcodes;

/// Scalar operand codes.
pub(crate) const VCC: u16 = 106;
pub(crate) const EXEC: u16 = 126;
/// "The 32-bit literal that follows".
const LITERAL: u16 = 255;

/// A source operand of a 9-bit field: an SGPR or special register by its
/// code, an inline constant, a VGPR, or a literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Src {
    /// `s0`-`s105`, [`VCC`], [`EXEC`].
    S(u16),
    V(u8),
    /// An integer the instruction set has inline: 0 to 64, -1 to -16.
    K(i8),
    /// Any 32 bits, in a word after the instruction.
    Lit(u32),
}

impl Src {
    fn code(self) -> u16 {
        match self {
            Src::S(c) => c,
            Src::V(r) => 256 + u16::from(r),
            Src::K(n @ 0..=64) => 128 + n as u16,
            Src::K(n @ -16..=-1) => (192 - i16::from(n)) as u16,
            Src::K(n) => panic!("{n} is no inline constant"),
            Src::Lit(_) => LITERAL,
        }
    }

    fn literal(self) -> Option<u32> {
        match self {
            Src::Lit(x) => Some(x),
            _ => None,
        }
    }
}

fn op(table: &[(u16, &str)], name: &str) -> u32 {
    let found = table.iter().find(|e| e.1 == name);
    u32::from(found.unwrap_or_else(|| panic!("no opcode {name}")).0)
}

/// A program being written.
#[derive(Debug, Default)]
pub(crate) struct Asm {
    words: Vec<u32>,
    labels: HashMap<&'static str, usize>,
    /// Branches to patch: the word, the label.
    branches: Vec<(usize, &'static str)>,
}

impl Asm {
    pub fn new() -> Self {
        Asm::default()
    }

    fn push(&mut self, words: &[u32], literals: &[Src]) {
        self.words.extend_from_slice(words);
        let mut literal = literals.iter().filter_map(|s| s.literal());
        if let Some(x) = literal.next() {
            self.words.push(x);
        }
        assert!(literal.next().is_none(), "one literal an instruction");
    }

    /// The next instruction is `name`'s target.
    pub fn label(&mut self, name: &'static str) {
        assert!(
            self.labels.insert(name, self.words.len()).is_none(),
            "label {name} twice"
        );
    }

    /// `s_branch`, `s_cbranch_*` to `label`.
    pub fn branch(&mut self, name: &str, label: &'static str) {
        self.branches.push((self.words.len(), label));
        self.words.push(0xBF80_0000 | op(opcodes::SOPP, name) << 16);
    }

    pub fn endpgm(&mut self) {
        self.words
            .push(0xBF80_0000 | op(opcodes::SOPP, "s_endpgm") << 16);
    }

    pub fn sop1(&mut self, name: &str, sdst: u16, src: Src) {
        let w = 0xBE80_0000
            | u32::from(sdst) << 16
            | op(opcodes::SOP1, name) << 8
            | u32::from(src.code());
        self.push(&[w], &[src]);
    }

    pub fn sop2(&mut self, name: &str, sdst: u16, src0: Src, src1: Src) {
        let (a, b) = (u32::from(src0.code()), u32::from(src1.code()));
        let w = 0x8000_0000 | op(opcodes::SOP2, name) << 23 | u32::from(sdst) << 16 | b << 8 | a;
        self.push(&[w], &[src0, src1]);
    }

    pub fn sopc(&mut self, name: &str, src0: Src, src1: Src) {
        let (a, b) = (u32::from(src0.code()), u32::from(src1.code()));
        self.push(
            &[0xBF00_0000 | op(opcodes::SOPC, name) << 16 | b << 8 | a],
            &[src0, src1],
        );
    }

    /// `s_load_dwordx4 s[sdst..], s[base:base+1], bytes`.
    pub fn s_load_x4(&mut self, sdst: u16, base: u16, bytes: u32) {
        assert!(base.is_multiple_of(2) && bytes < 1 << 20);
        let w =
            0x3D << 26 | op(opcodes::SMEM, "s_load_dwordx4") << 18 | 1 << 17 | u32::from(sdst) << 6;
        // An immediate offset; no offset register.
        self.words
            .extend([w | u32::from(base) >> 1, bytes | 125 << 25]);
    }

    pub fn vop1(&mut self, name: &str, vdst: u8, src: Src) {
        let w = 0x7E00_0000
            | u32::from(vdst) << 17
            | op(opcodes::VOP1, name) << 9
            | u32::from(src.code());
        self.push(&[w], &[src]);
    }

    pub fn vop2(&mut self, name: &str, vdst: u8, src0: Src, vsrc1: u8) {
        let w = op(opcodes::VOP2, name) << 25
            | u32::from(vdst) << 17
            | u32::from(vsrc1) << 9
            | u32::from(src0.code());
        self.push(&[w], &[src0]);
    }

    /// A compare of `src0` with `vsrc1` into VCC, or with `x` into EXEC
    /// (`v_cmpx_*`).
    pub fn vopc(&mut self, name: &str, src0: Src, vsrc1: u8) {
        let w = 0x7C00_0000
            | op(opcodes::VOPC, name) << 17
            | u32::from(vsrc1) << 9
            | u32::from(src0.code());
        self.push(&[w], &[src0]);
    }

    /// A compare of `src0` with `src1` into the SGPR `sdst` (the
    /// three-operand form).
    pub fn vopc_to(&mut self, name: &str, sdst: u16, src0: Src, src1: Src) {
        let (a, b) = (u32::from(src0.code()), u32::from(src1.code()));
        let w = 0xD400_0000 | op(opcodes::VOPC, name) << 16 | u32::from(sdst);
        self.push(&[w, a | b << 9], &[src0, src1]);
    }

    /// An instruction of VOP3's own table with two sources.
    pub fn vop3(&mut self, name: &str, vdst: u8, src0: Src, src1: Src) {
        let (a, b) = (u32::from(src0.code()), u32::from(src1.code()));
        let w = 0xD400_0000 | op(opcodes::VOP3, name) << 16 | u32::from(vdst);
        self.push(&[w, a | b << 9], &[src0, src1]);
    }

    /// `v_add_co_u32 vdst, vcc, src0, src1`: the sum, its carry out in VCC.
    pub fn add_co(&mut self, vdst: u8, src0: Src, src1: Src) {
        let (a, b) = (u32::from(src0.code()), u32::from(src1.code()));
        let w = 0xD400_0000
            | op(opcodes::VOP3, "v_add_co_u32") << 16
            | u32::from(VCC) << 8
            | u32::from(vdst);
        self.push(&[w, a | b << 9], &[src0, src1]);
    }

    /// `v_cndmask_b32 vdst, otherwise, then, mask`: `then` where the lane's
    /// bit of the SGPR mask is set.
    pub fn cndmask(&mut self, vdst: u8, otherwise: Src, then: Src, mask: u16) {
        let (a, b) = (u32::from(otherwise.code()), u32::from(then.code()));
        let w = 0xD400_0000 | (0x100 + op(opcodes::VOP2, "v_cndmask_b32")) << 16 | u32::from(vdst);
        self.push(&[w, a | b << 9 | u32::from(mask) << 18], &[otherwise, then]);
    }

    /// A shared-memory instruction on the byte address in `vaddr` plus
    /// `offset`: `ds_write_b32` of `vdata`, or `ds_read_b32` into `vdst`.
    pub fn ds(&mut self, name: &str, offset: u16, vaddr: u8, vdata: u8, vdst: u8) {
        self.words.extend([
            0x36 << 26 | op(opcodes::DS, name) << 18 | u32::from(offset),
            u32::from(vdst) << 24 | u32::from(vdata) << 8 | u32::from(vaddr),
        ]);
    }

    /// A buffer load or store of `vdata` at the byte offset in `vaddr` plus
    /// `offset`, through the descriptor in `s[rsrc..rsrc + 4]`.
    pub fn mubuf(&mut self, name: &str, vdata: u8, vaddr: u8, rsrc: u16, offset: u32) {
        assert!(offset < 0x1000 && rsrc.is_multiple_of(4));
        let code = op(opcodes::MUBUF, name);
        self.words.extend([
            0x38 << 26 | (code & 0x7F) << 18 | (code >> 7) << 25 | 1 << 12 | offset,
            // No offset register (the constant 0).
            0x80 << 24 | u32::from(rsrc >> 2) << 16 | u32::from(vdata) << 8 | u32::from(vaddr),
        ]);
    }

    /// `global_load_dword vdst, vaddr, saddr, offset`: a load at an address
    /// the program computed: the 64-bit address in `v[vaddr:vaddr + 1]`, or
    /// with `saddr` the 64-bit base in `s[saddr:saddr + 1]` plus the 32-bit
    /// offset in `vaddr`.
    pub fn global_load(&mut self, vdst: u8, vaddr: u8, saddr: Option<u16>, offset: u16) {
        assert!(offset < 0x800);
        let saddr = saddr.unwrap_or(u16::from(crate::gfx10::NULL));
        self.words.extend([
            0x37 << 26
                | op(opcodes::GLOBAL, "global_load_dword") << 18
                | 2 << 14
                | u32::from(offset),
            u32::from(vdst) << 24 | u32::from(saddr) << 16 | u32::from(vaddr),
        ]);
    }

    /// `image_bvh_intersect_ray v[vdst..vdst + 4], [node, extent, origin,
    /// direction, inverse direction], s[rsrc..rsrc + 4]`, each address in a
    /// VGPR of its own.
    pub fn bvh(&mut self, vdst: u8, addr: [u8; 11], rsrc: u16) {
        assert!(rsrc.is_multiple_of(4));
        let code = op(opcodes::MIMG, "image_bvh_intersect_ray");
        // All four result dwords, a 128-bit descriptor, three extra words
        // of address registers.
        self.words.extend([
            0x3C << 26 | (code & 0x7F) << 18 | (code >> 7) | 0xF << 8 | 1 << 15 | 3 << 1,
            u32::from(rsrc >> 2) << 16 | u32::from(vdst) << 8 | u32::from(addr[0]),
        ]);
        for extra in addr[1..].chunks(4) {
            let word = extra
                .iter()
                .enumerate()
                .map(|(j, &r)| u32::from(r) << (8 * j));
            self.words.push(word.fold(0, |a, b| a | b));
        }
    }

    /// [`Asm::bvh`] with the instruction's `a16` bit set: the directions
    /// packed as 16-bit floats. (The registers named stay the eleven; the
    /// decoder reads eight of them. No such program is run.)
    pub fn bvh_packed(&mut self, vdst: u8, addr: [u8; 11], rsrc: u16) {
        let at = self.words.len();
        self.bvh(vdst, addr, rsrc);
        self.words[at + 1] |= 1 << 30;
    }

    /// The program's words, every branch resolved.
    pub fn finish(mut self) -> Vec<u32> {
        for (at, label) in &self.branches {
            let to = *self
                .labels
                .get(label)
                .unwrap_or_else(|| panic!("no label {label}"));
            let words = to as i64 - (*at as i64 + 1);
            let words = i16::try_from(words).expect("a branch within reach");
            self.words[*at] |= u32::from(words as u16);
        }
        self.words
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx10::{self, Fields, Operand};

    fn decoded(words: Vec<u32>) -> Vec<(String, Fields)> {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let list = gfx10::decode(&code).expect("decodes");
        list.iter()
            .map(|i| (i.name().unwrap_or("?").to_owned(), i.fields(false)))
            .collect()
    }

    /// Every form the assembler writes decodes to what was asked for.
    #[test]
    fn what_is_assembled_decodes_to_what_was_meant() {
        let mut a = Asm::new();
        a.sop1("s_mov_b32", 7, Src::Lit(0xDEAD_BEEF));
        a.sop2("s_lshr_b64", 24, Src::S(0), Src::K(8));
        a.s_load_x4(8, 6, 32);
        a.vop1("v_rcp_f32", 12, Src::V(8));
        a.vop2("v_add_nc_u32", 16, Src::K(-4), 16);
        a.vopc("v_cmp_lt_f32", Src::V(7), 27);
        a.vopc_to("v_cmp_lt_f32", 22, Src::V(27), Src::V(18));
        a.cndmask(18, Src::V(18), Src::V(27), 22);
        a.ds("ds_write_b32", 0, 23, 31, 0);
        a.ds("ds_read_b32", 4, 23, 0, 24);
        a.mubuf("buffer_load_dwordx4", 4, 2, 8, 16);
        a.mubuf("buffer_store_dwordx4", 36, 3, 12, 0);
        a.bvh(28, [24, 18, 4, 5, 6, 8, 9, 10, 12, 13, 14], 24);
        a.global_load(40, 38, None, 48);
        a.global_load(41, 38, Some(0), 52);
        a.vop3("v_mul_lo_u32", 2, Src::V(1), Src::K(48));
        a.add_co(44, Src::V(44), Src::K(52));
        a.label("end");
        a.branch("s_cbranch_execz", "end");
        a.endpgm();
        let d = decoded(a.finish());
        let names: Vec<&str> = d.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(
            names,
            [
                "s_mov_b32",
                "s_lshr_b64",
                "s_load_dwordx4",
                "v_rcp_f32",
                "v_add_nc_u32",
                "v_cmp_lt_f32",
                "v_cmp_lt_f32",
                "v_cndmask_b32",
                "ds_write_b32",
                "ds_read_b32",
                "buffer_load_dwordx4",
                "buffer_store_dwordx4",
                "image_bvh_intersect_ray",
                "global_load_dword",
                "global_load_dword",
                "v_mul_lo_u32",
                "v_add_co_u32",
                "s_cbranch_execz",
                "s_endpgm",
            ]
        );
        let s = |code, dwords| Operand::Scalar { code, dwords };
        let v = |reg, dwords| Operand::Vector { reg, dwords };
        assert_eq!(
            d[0].1,
            Fields::Sop {
                dst: Some(s(7, 1)),
                src: vec![Operand::Literal(0xDEAD_BEEF)],
                imm: None
            }
        );
        assert_eq!(
            d[1].1,
            Fields::Sop {
                dst: Some(s(24, 2)),
                src: vec![s(0, 2), Operand::Const(136)],
                imm: None
            }
        );
        let Fields::Smem {
            data, base, offset, ..
        } = &d[2].1
        else {
            panic!()
        };
        assert_eq!((*data, *base, *offset), (Some(s(8, 4)), s(6, 2), 32));
        let Fields::Valu(rcp) = &d[3].1 else { panic!() };
        assert_eq!((rcp.dst, &rcp.src[..]), (Some(v(12, 1)), &[v(8, 1)][..]));
        let Fields::Valu(add) = &d[4].1 else { panic!() };
        assert_eq!(add.src, [Operand::Const(196), v(16, 1)]);
        let Fields::Valu(cmp) = &d[5].1 else { panic!() };
        assert_eq!(
            (cmp.dst, &cmp.src[..]),
            (Some(s(106, 1)), &[v(7, 1), v(27, 1)][..])
        );
        let Fields::Valu(cmp) = &d[6].1 else { panic!() };
        assert_eq!(
            (cmp.dst, &cmp.src[..]),
            (Some(s(22, 1)), &[v(27, 1), v(18, 1)][..])
        );
        let Fields::Valu(pick) = &d[7].1 else {
            panic!()
        };
        assert_eq!(
            (pick.dst, &pick.src[..]),
            (Some(v(18, 1)), &[v(18, 1), v(27, 1), s(22, 1)][..])
        );
        let Fields::Ds {
            dst,
            addr,
            data,
            offset0,
            ..
        } = &d[8].1
        else {
            panic!()
        };
        assert_eq!(
            (*dst, *addr, &data[..], *offset0),
            (None, Some(v(23, 1)), &[v(31, 1)][..], 0)
        );
        let Fields::Ds {
            dst, addr, offset0, ..
        } = &d[9].1
        else {
            panic!()
        };
        assert_eq!((*dst, *addr, *offset0), (Some(v(24, 1)), Some(v(23, 1)), 4));
        let Fields::Buffer {
            data,
            addr,
            rsrc,
            offset,
            offen,
            idxen,
            ..
        } = &d[10].1
        else {
            panic!()
        };
        assert_eq!(
            (*data, *addr, *rsrc, *offset),
            (Some(v(4, 4)), v(2, 1), s(8, 4), 16)
        );
        assert!(*offen && !*idxen);
        let Fields::Buffer { data, rsrc, .. } = &d[11].1 else {
            panic!()
        };
        assert_eq!((*data, *rsrc), (Some(v(36, 4)), s(12, 4)));
        let Fields::Image {
            data,
            addr,
            rsrc,
            a16,
            ..
        } = &d[12].1
        else {
            panic!()
        };
        assert_eq!((*data, *rsrc, *a16), (v(28, 4), s(24, 4), false));
        let want: Vec<Operand> = [24, 18, 4, 5, 6, 8, 9, 10, 12, 13, 14]
            .map(|r| v(r, 1))
            .into();
        assert_eq!(*addr, want);
        let Fields::Flat {
            dst, addr, offset, ..
        } = &d[13].1
        else {
            panic!()
        };
        assert_eq!((*dst, *addr, *offset), (Some(v(40, 1)), Some(v(38, 2)), 48));
        let Fields::Flat {
            dst,
            addr,
            saddr,
            offset,
            ..
        } = &d[14].1
        else {
            panic!()
        };
        assert_eq!(
            (*dst, *addr, *saddr, *offset),
            (Some(v(41, 1)), Some(v(38, 1)), Some(s(0, 2)), 52)
        );
        let Fields::Valu(mul) = &d[15].1 else {
            panic!()
        };
        assert_eq!(
            (mul.dst, &mul.src[..2]),
            (Some(v(2, 1)), &[v(1, 1), Operand::Const(176)][..])
        );
        let Fields::Valu(add) = &d[16].1 else {
            panic!()
        };
        assert_eq!(
            (add.dst, &add.src[..], add.carry),
            (
                Some(v(44, 1)),
                &[v(44, 1), Operand::Const(180)][..],
                Some(s(106, 1))
            )
        );
        assert_eq!(d[17].1, Fields::Sopp { imm: 0xFFFF });
    }

    #[test]
    fn inline_constants_have_their_codes() {
        assert_eq!(Src::K(0).code(), 128);
        assert_eq!(Src::K(64).code(), 192);
        assert_eq!(Src::K(-1).code(), 193);
        assert_eq!(Src::K(-16).code(), 208);
    }
}
