//! `ds_*` instructions: LDS (thread-group shared memory), `ds_swizzle` and
//! `ds_bpermute`.

use super::{Lower, Why};
use crate::emit::bitcode::{Attr, Bin, V, pred};
use crate::gfx10::{Fields, Operand};
use crate::ir::{Inst, Reg};

impl<'a> Lower<'a> {
    /// The bit position of byte LDS address `a` + `offset` in its dword.
    fn lds_shift(&mut self, a: V, offset: u32) -> V {
        let c = self.cur;
        let (o, three) = (self.s.m.i32(offset), self.s.m.i32(3));
        let at = self.s.m.bin(c, Bin::Add, a, o);
        let byte = self.s.m.bin(c, Bin::And, at, three);
        self.s.m.bin(c, Bin::Shl, byte, three)
    }

    /// LDS (thread-group shared memory), `ds_swizzle` and `ds_bpermute`.
    pub(super) fn ds(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Ds {
            dst,
            addr,
            data,
            offset0,
            offset1,
            gds,
        } = &inst.fields
        else {
            unreachable!()
        };
        if *gds {
            return Err("GDS");
        }
        let name = inst.name();
        let guard = self.guard(b, i)?;
        let c = self.cur;
        let offset = u32::from(*offset0) | u32::from(*offset1) << 8;
        let dst_reg = match dst {
            Some(Operand::Vector { reg, .. }) => Some(*reg),
            _ => None,
        };
        if name == "ds_swizzle_b32" {
            // Bit 15 set: each lane of a quad picks a lane of it; else lanes
            // of each group of 32 combine their index with and/or/xor masks.
            let quad = offset & 0x8000 != 0
                || offset & 0x1C == 0x1C && offset & 0x380 == 0 && offset >> 10 & 0x1F < 4;
            // Mesh: host quads are threads 4n..4n+3 of a 1D group, as the
            // console's, so quad-local swizzles read through quad operations.
            let mesh_quad = self.s.mesh.is_some() && quad;
            if !mesh_quad {
                self.cross_lane(quad)?;
            }
            // The source sits in the address field.
            let x = self.vgprs(inst, addr.as_ref().ok_or("a swizzle source")?, 1)?[0];
            let lane = self.lane_index();
            let k = |l: &mut Self, v: u32| l.s.m.i32(v);
            let src = if offset & 0x8000 != 0 {
                let (three, one) = (k(self, 3), k(self, 1));
                let sel = self.s.m.bin(c, Bin::And, lane, three);
                let sh = self.s.m.bin(c, Bin::Shl, sel, one);
                let pattern = k(self, offset & 0xFF);
                let pick = self.s.m.bin(c, Bin::LShr, pattern, sh);
                let pick = self.s.m.bin(c, Bin::And, pick, three);
                let not3 = k(self, !3);
                let base = self.s.m.bin(c, Bin::And, lane, not3);
                self.s.m.bin(c, Bin::Or, base, pick)
            } else {
                let and = k(self, offset & 0x1F | !0x1F);
                let or = k(self, offset >> 5 & 0x1F);
                let xor = k(self, offset >> 10 & 0x1F);
                let t = self.s.m.bin(c, Bin::And, lane, and);
                let t = self.s.m.bin(c, Bin::Or, t, or);
                self.s.m.bin(c, Bin::Xor, t, xor)
            };
            let use_quad = mesh_quad || self.helpers_read(quad);
            let src_ok = self.src_lane_active(src, guard, mesh_quad);
            let mut r = if use_quad {
                self.quad_pick(x, src)
            } else {
                let safe_src = match src_ok {
                    Some(ok) => self.s.m.select(c, ok, src, lane),
                    None => src,
                };
                self.read_lane(x, safe_src)
            };
            if let Some(ok) = src_ok {
                let zero = k(self, 0);
                r = self.s.m.select(c, ok, r, zero);
            }
            let reg = dst_reg.ok_or("a swizzle result")?;
            self.write(inst, Reg::V(reg), r, guard);
            return Ok(());
        }
        if name == "ds_bpermute_b32" {
            // Each lane reads `data` of the lane its address plus the offset
            // names in dwords within its own 32-lane half (RDNA 2 ISA,
            // section 10.4.4: wave64 permutes lanes 0..31 and 32..63
            // independently); 0 from a lane EXEC leaves out.
            self.cross_lane(false)?;
            let a = self.vgprs(inst, addr.as_ref().ok_or("a permute address")?, 1)?[0];
            let x = self.vgprs(inst, data.first().ok_or("a permute source")?, 1)?[0];
            let k = |l: &mut Self, v: u32| l.s.m.i32(v);
            let (o, two, mask31) = (k(self, offset), k(self, 2), k(self, 31));
            let at = self.s.m.bin(c, Bin::Add, a, o);
            let lane = self.s.m.bin(c, Bin::LShr, at, two);
            let within_half = self.s.m.bin(c, Bin::And, lane, mask31);
            let lane = if self.x.native.wave64 {
                let me = self.lane_index();
                let not31 = k(self, !31);
                let base = self.s.m.bin(c, Bin::And, me, not31);
                self.s.m.bin(c, Bin::Or, base, within_half)
            } else {
                within_half
            };
            let r = self.read_lane(x, lane);
            let on = guard.unwrap_or_else(|| self.s.m.bool(true));
            let i32t = self.s.t.i32;
            let four = self.s.m.named_struct("dx.types.fouri32", vec![i32t; 4]);
            let ballot = self
                .s
                .op(c, "waveActiveBallot", Attr::NoUnwind, four, 116, &[on]);
            let (lo, hi) = (
                self.s.m.extract(c, ballot, 0),
                self.s.m.extract(c, ballot, 1),
            );
            let running = self.lane_bit_of(c, lane, lo, hi);
            let z = k(self, 0);
            let r = self.s.m.select(c, running, r, z);
            let reg = dst_reg.ok_or("a permute result")?;
            self.write(inst, Reg::V(reg), r, guard);
            return Ok(());
        }
        if self.s.lds_dwords == 0 {
            return Err("LDS the program does not allocate");
        }
        let a = self.vgprs(inst, addr.as_ref().ok_or("an LDS address")?, 1)?[0];
        // shortcut: LDS addresses are taken as aligned to what they access.
        // The address is `addr + offset` in 32 bits (it wraps: LLVM folds a
        // base below 0 and an offset on GFX7 and later). Each dword comes
        // with whether it lies inside the program's LDS:
        // out-of-bounds console accesses read 0 and drop writes, and the
        // host index is clamped so it never leaves the group-shared array.
        let dword = |l: &mut Self, bytes: u32| -> (V, V) {
            let c = l.cur;
            let o = l.s.m.i32(bytes);
            let at = l.s.m.bin(c, Bin::Add, a, o);
            let two = l.s.m.i32(2);
            let index = l.s.m.bin(c, Bin::LShr, at, two);
            let n = l.s.m.i32(l.s.lds_dwords);
            let inside = l.s.m.cmp(c, pred::IULT, index, n);
            let zero = l.s.m.i32(0);
            let index = l.s.m.select(c, inside, index, zero);
            let lds = l.s.lds();
            (l.s.m.element(c, lds, index), inside)
        };
        // A lane accesses when its guard holds and the dword is inside.
        let on = |l: &mut Self, inside: V| -> V {
            match guard {
                Some(g) => l.s.m.bin(l.cur, Bin::And, g, inside),
                None => inside,
            }
        };
        let width = |n: &str| match n.rsplit('_').next() {
            Some("b64") => 2,
            Some("b96") => 3,
            Some("b128") => 4,
            _ => 1,
        };
        // (byte offset, dwords) per element; read2/write2 scale their two
        // offsets by the element size (×64 for st64).
        let pair = |n: &str| -> Vec<(u32, u32)> {
            let w = width(n);
            let scale = 4 * w * if n.contains("st64") { 64 } else { 1 };
            vec![
                (u32::from(*offset0) * scale, w),
                (u32::from(*offset1) * scale, w),
            ]
        };
        if let Some(rest) = name.strip_prefix("ds_read") {
            let elements = if rest.starts_with('2') {
                pair(name)
            } else if matches!(rest, "_b32" | "_b64" | "_b96" | "_b128") {
                vec![(offset, width(name))]
            } else {
                // A byte or half, extended (`_d16`: into the low half,
                // `_d16_hi`: the high half, the other half kept).
                let (bits, signed) = match rest.trim_end_matches("_hi").trim_end_matches("_d16") {
                    "_u8" => (8, false),
                    "_i8" => (8, true),
                    "_u16" => (16, false),
                    "_i16" => (16, true),
                    _ => return Err("an LDS read of bytes or halves"),
                };
                let reg = dst_reg.ok_or("an LDS read result")?;
                let (p, inside) = dword(self, offset);
                let x = self.s.m.load(c, p);
                let zero = self.s.m.i32(0);
                let x = self.s.m.select(c, inside, x, zero);
                let sh = self.lds_shift(a, offset);
                let x = self.s.m.bin(c, Bin::LShr, x, sh);
                let left = self.s.m.i32(32 - bits);
                let x = self.s.m.bin(c, Bin::Shl, x, left);
                let x = self
                    .s
                    .m
                    .bin(c, if signed { Bin::AShr } else { Bin::LShr }, x, left);
                let x = if rest.contains("_d16") {
                    let old = match inst.read(Reg::V(reg)) {
                        Some(o) => self.i32_of(o)?,
                        None => self.s.m.i32(0),
                    };
                    let hi = rest.ends_with("_hi");
                    let keep = self.s.m.i32(if hi { 0xFFFF } else { 0xFFFF_0000 });
                    let old = self.s.m.bin(c, Bin::And, old, keep);
                    let x = if hi {
                        let k16 = self.s.m.i32(16);
                        self.s.m.bin(c, Bin::Shl, x, k16)
                    } else {
                        let low = self.s.m.i32(0xFFFF);
                        self.s.m.bin(c, Bin::And, x, low)
                    };
                    self.s.m.bin(c, Bin::Or, old, x)
                } else {
                    x
                };
                self.write(inst, Reg::V(reg), x, guard);
                return Ok(());
            };
            let reg = dst_reg.ok_or("an LDS read result")?;
            let mut n = 0;
            for (off, w) in elements {
                for k in 0..w {
                    let (p, inside) = dword(self, off + 4 * k);
                    let x = self.s.m.load(self.cur, p);
                    let zero = self.s.m.i32(0);
                    let x = self.s.m.select(self.cur, inside, x, zero);
                    self.write(inst, Reg::V(reg + n), x, guard);
                    n += 1;
                }
            }
            return Ok(());
        }
        if let Some(rest) = name.strip_prefix("ds_write") {
            let elements = if rest.starts_with('2') {
                pair(name)
            } else if matches!(rest, "_b32" | "_b64" | "_b96" | "_b128") {
                vec![(offset, width(name))]
            } else {
                // A byte or half (`_d16_hi`: the high half of the data) into
                // its dword through atomics, so lanes writing the other parts
                // of the dword keep theirs.
                // shortcut: lanes writing the same part in one instruction
                // leave the OR of theirs (the console keeps one).
                let bits = match rest.trim_end_matches("_d16_hi") {
                    "_b8" => 8,
                    "_b16" => 16,
                    _ => return Err("an LDS write of bytes or halves"),
                };
                let x = self.vgprs(inst, data.first().ok_or("an LDS write")?, 1)?[0];
                let x = if rest.ends_with("_d16_hi") {
                    let k16 = self.s.m.i32(16);
                    self.s.m.bin(c, Bin::LShr, x, k16)
                } else {
                    x
                };
                let mask = self.s.m.i32((1 << bits) - 1);
                let x = self.s.m.bin(c, Bin::And, x, mask);
                let sh = self.lds_shift(a, offset);
                let set = self.s.m.bin(c, Bin::Shl, x, sh);
                let part = self.s.m.bin(c, Bin::Shl, mask, sh);
                let ones = self.s.m.i32(u32::MAX);
                let keep = self.s.m.bin(c, Bin::Xor, part, ones);
                let (p, inside) = dword(self, offset);
                let g = on(self, inside);
                let at = self.begin_guarded(Some(g));
                use super::bitcode::Rmw;
                self.s.m.atomic(self.cur, Rmw::And, p, keep);
                self.s.m.atomic(self.cur, Rmw::Or, p, set);
                self.end_guarded(at, &[]);
                return Ok(());
            };
            let mut stores = Vec::new();
            for (e, (off, w)) in elements.into_iter().enumerate() {
                let vals = self.vgprs(inst, data.get(e).ok_or("an LDS write")?, w as u8)?;
                for (k, x) in vals.into_iter().enumerate() {
                    stores.push((off + 4 * k as u32, x));
                }
            }
            for (off, x) in stores {
                let (p, inside) = dword(self, off);
                let g = on(self, inside);
                let at = self.begin_guarded(Some(g));
                self.s.m.store(self.cur, p, x);
                self.end_guarded(at, &[]);
            }
            return Ok(());
        }
        // Atomics: the old value returns with `_rtn`.
        let op = name
            .strip_prefix("ds_")
            .map(|n| n.replace("_rtn", ""))
            .ok_or("an LDS operation")?;
        use super::bitcode::Rmw;
        let rmw = match op.as_str() {
            "add_u32" => Some(Rmw::Add),
            "sub_u32" => Some(Rmw::Sub),
            "and_b32" => Some(Rmw::And),
            "or_b32" => Some(Rmw::Or),
            "xor_b32" => Some(Rmw::Xor),
            "min_i32" => Some(Rmw::Min),
            "max_i32" => Some(Rmw::Max),
            "min_u32" => Some(Rmw::UMin),
            "max_u32" => Some(Rmw::UMax),
            "wrxchg_b32" => Some(Rmw::Xchg),
            "cmpst_b32" => None,
            _ => return Err("an LDS operation"),
        };
        let v0 = self.vgprs(inst, data.first().ok_or("an LDS atomic")?, 1)?[0];
        let v1 = match data.get(1) {
            Some(d) => Some(self.vgprs(inst, d, 1)?[0]),
            None => None,
        };
        let old_reg = dst_reg.and_then(|r| inst.read(Reg::V(r)).map(|v| (r, v)));
        let outside = match old_reg {
            Some((_, v)) => self.i32_of(v).unwrap_or(self.s.m.i32(0)),
            None => self.s.m.i32(0),
        };
        let (p, inside) = dword(self, offset);
        // Lanes the guard leaves out keep the old register; lanes outside
        // the LDS read 0.
        let zero = self.s.m.i32(0);
        let outside = match guard {
            Some(g) => self.s.m.select(self.cur, g, zero, outside),
            None => zero,
        };
        let g = on(self, inside);
        let at = self.begin_guarded(Some(g));
        let r = match rmw {
            Some(op) => self.s.m.atomic(self.cur, op, p, v0),
            // Compare with data0, store data1.
            None => self
                .s
                .m
                .cmpxchg(self.cur, p, v0, v1.ok_or("an LDS compare")?),
        };
        let r = self.end_guarded(at, &[(r, outside)])[0];
        if let Some(reg) = dst_reg {
            self.write(inst, Reg::V(reg), r, None);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{S_ENDPGM, lowers};
    use super::super::{Parts, lower};
    use crate::native::{ComputeIo, HwStage, NativeProgram, StageIo};
    use crate::{io, ir, resource, structure};

    #[test]
    fn ds_bpermute_wave64_preserves_32_lane_half() {
        // Without reading v0 (`thread_id`), only the wave64 half-wave base
        // (`lane_index & !31`) introduces `dx.op.waveGetLaneIndex`.
        let words = [
            0x7E02_02FF,
            0x0000_0104, // v_mov_b32 v1, 0x104
            0x7E04_0285, // v_mov_b32 v2, 5
            0xDACC_000C,
            0x0300_0201, // ds_bpermute_b32 v3, v1, v2 offset:12
            0x7E00_0503, // v_readfirstlane_b32 s0, v3
            0xBF07_8000, // s_cmp_lg_u32 s0, 0
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE83_0380, // s_mov_b32 s3, 0
            S_ENDPGM,
        ];
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        for wave64 in [false, true] {
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64,
                vgprs: 8,
                layout: vec![],
                used: vec![],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 0,
                    threads: [if wave64 { 64 } else { 32 }, 1, 1],
                }),
                code: code.clone(),
            };
            let p = ir::build(&native.code, wave64, native.vgprs).unwrap();
            let st = structure::structure(&p).unwrap();
            let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let mut lowered = lower(&Parts {
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
            .unwrap();
            assert!(lowered.wave_ops);
            assert_eq!(lowered.uses_lane_index, wave64, "wave64={wave64}");
            assert_eq!(lowered.has_half_wave_mask, wave64, "wave64={wave64}");
            #[cfg(windows)]
            if let Ok(mut v) = crate::emit::validator::Validator::load(
                crate::emit::validator::Validator::default_path().as_deref(),
            ) {
                v.validate(&mut lowered.dxil).unwrap();
            }
        }
    }

    #[test]
    fn ds_bpermute_refuses_gds() {
        // ds_bpermute_b32 with gds=1 (bit 17), consumed by a branch, must fail with GDS.
        let words = [
            0x7E02_0285,           // v_mov_b32 v1, 5
            0xDACC_0000 | 1 << 17, // ds_bpermute_b32 gds
            2 << 24 | 1 << 8,
            0x7E00_0502, // v_readfirstlane_b32 s0, v2
            0xBF07_8000, // s_cmp_lg_u32 s0, 0
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE83_0380, // s_mov_b32 s3, 0
            S_ENDPGM,
        ];
        assert!(matches!(
            lowers(&words, 0),
            Err(crate::ShaderError::Unsupported { what: "GDS", .. })
        ));
    }
}
