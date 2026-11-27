//! Image instructions (`image_*`): sampling, loads, stores, gathers and
//! size queries, and the read marks of texture views.

use std::collections::HashSet;

use super::{Lower, Why};
use crate::emit::bitcode::{Attr, Bin, Cast, V, pred};
use crate::emit::shader::flags;
use crate::gfx10::{Fields, Operand};
use crate::ir::{Inst, Reg, Value};
use crate::native::ResourceType;
use crate::resource::Source;
#[cfg(doc)]
use super::Parts;

impl<'a> Lower<'a> {
    /// An image instruction, its reads of texture views marked (module
    /// notes, "Read marks") after it.
    pub(super) fn image(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        self.s.texture_reads.clear();
        let lowered = self.image_op(inst, b, i);
        for index in std::mem::take(&mut self.s.texture_reads) {
            if lowered.is_ok() {
                self.mark_view_read(index);
            }
        }
        lowered
    }

    /// Sets the read mark of resource-heap index `index` (module notes,
    /// "Read marks") where it is still clear. Nothing without
    /// [`Parts::view_marks`].
    fn mark_view_read(&mut self, index: V) {
        if !self.x.view_marks {
            return;
        }
        let k = |l: &mut Self, v: u32| l.s.m.i32(v);
        let bitmap = self.s.view_writes_handle();
        let c = self.cur;
        let (base, k5, k2, k31, one, zero) =
            (k(self, super::VIEW_READS), k(self, 5), k(self, 2), k(self, 31), k(self, 1), k(self, 0));
        let n = self.s.m.bin(c, Bin::Add, index, base);
        let word = self.s.m.bin(c, Bin::LShr, n, k5);
        let word = self.s.m.bin(c, Bin::Shl, word, k2);
        let bit = self.s.m.bin(c, Bin::And, n, k31);
        let bit = self.s.m.bin(c, Bin::Shl, one, bit);
        // Most reads find their slot marked: a load, not an atomic.
        let old = self.raw_dword(bitmap, word);
        let set = self.s.m.bin(c, Bin::And, old, bit);
        let clear = self.s.m.cmp(c, pred::EQ, set, zero);
        let at = self.begin_guarded(Some(clear));
        let iu = self.s.m.undef(self.s.t.i32);
        // Operation 2 is OR.
        self.s.op(self.cur, "atomicBinOp.i32", Attr::NoUnwind, self.s.t.i32, 78, &[bitmap, k2, word, iu, iu, bit]);
        self.end_guarded(at, &[]);
    }

    fn image_op(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Image {
            data,
            addr,
            dmask,
            dim,
            unorm,
            a16,
            d16,
            tfe,
            lwe,
            ..
        } = &inst.fields
        else {
            unreachable!()
        };
        if *tfe || *lwe {
            return Err("an image access with tfe or lwe");
        }
        let name = inst.name();
        if name.starts_with("image_bvh") {
            return self.bvh(inst, b, i);
        }
        // A load or sample with no channel enabled returns the first.
        let dmask = if *dmask == 0 { &1 } else { dmask };
        let site = (b, i);
        let guard = self.guard(b, i)?;
        let c = self.cur;
        let (kind, coords) = match dim {
            0 => (1, 1),
            1 => (2, 2),
            2 => (4, 3),
            // Sampled (face coordinates and face, see `cube_direction`) or
            // sized only.
            3 if name.starts_with("image_sample")
                || name.starts_with("image_gather")
                || name == "image_get_resinfo" =>
            {
                (5, 3)
            }
            4 => (6, 2),
            5 => (7, 3),
            _ => return Err("an image dimension (cube loads or MSAA)"),
        };
        // Address VGPRs in order.
        let mut regs = Vec::new();
        for a in addr {
            if let Operand::Vector { reg, dwords } = *a {
                regs.extend((0..dwords).map(|k| reg + k));
            }
        }
        // With a16, two 16-bit components per VGPR, low half first, after
        // the offset, bias and compare value, which keep a VGPR each (an f16
        // bias in its low half).
        let extra = name
            .strip_prefix("image_sample")
            .or_else(|| name.strip_prefix("image_gather4"))
            .map_or(0, |rest| {
                rest.split('_')
                    .filter(|t| matches!(*t, "o" | "b" | "c"))
                    .count()
            });
        let addr_val = |l: &mut Self, k: usize| -> Result<V, Why> {
            let packed = *a16 && k >= extra;
            let r = *regs
                .get(if packed { extra + (k - extra) / 2 } else { k })
                .ok_or("an image address")?;
            let v = inst.read(Reg::V(r)).ok_or("an image address")?;
            let x = l.i32_of(v)?;
            if !packed {
                return Ok(x);
            }
            let c = l.cur;
            let half = (k - extra) as u32 % 2;
            let (sh, mask) = (l.s.m.i32(16 * half), l.s.m.i32(0xFFFF));
            let x = l.s.m.bin(c, Bin::LShr, x, sh);
            Ok(l.s.m.bin(c, Bin::And, x, mask))
        };
        let src = self
            .descriptors
            .get(&(site, false))
            .copied()
            .ok_or("an untracked image")?;
        let store = name.starts_with("image_store");
        let uav = store
            || self
                .used_kind(src)
                .is_some_and(|u| u.kind == ResourceType::Uav);
        let props = (kind | if uav { 0x1000 } else { 0 }, 0x409);
        if let Some(op) = name.strip_prefix("image_atomic_") {
            // DXIL's atomic operations on a 32-bit integer texture UAV; the
            // old value returns with glc. Sub is the add of the negation.
            let code = match op {
                "add" | "sub" => 0,
                "and" => 1,
                "or" => 2,
                "xor" => 3,
                "smin" => 4,
                "smax" => 5,
                "umin" => 6,
                "umax" => 7,
                "swap" => 8,
                "cmpswap" => u32::MAX,
                _ => return Err(name),
            };
            let Fields::Image { cache, .. } = &inst.fields else {
                unreachable!()
            };
            // 16-bit coordinates (a16) are read as `addr_val` reads them.
            if *d16 {
                return Err("a 16-bit image atomic");
            }
            // dmask: the data dwords (cmpswap: twice the value's); two per
            // value is a 64-bit atomic (an R32G32_UINT view, low dword in R).
            let values = if code == u32::MAX { 2 } else { 1 };
            let wide = match dmask.count_ones() / values {
                1 => false,
                2 => true,
                _ => return Err("an image atomic of more than two dwords"),
            };
            let n = if wide { 2 } else { 1 };
            let iu = self.s.m.undef(self.s.t.i32);
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let Operand::Vector { reg, .. } = *data else {
                return Err("image atomic data");
            };
            let vals = self.vgprs(inst, data, values as u8 * n)?;
            let i64t = self.s.m.int(64);
            // Value k of the data: one dword, or two as an i64.
            let value_at = |l: &mut Self, k: usize| -> V {
                if !wide {
                    return vals[k];
                }
                let lo = l.s.m.cast(c, Cast::ZExt, vals[2 * k], i64t);
                let hi = l.s.m.cast(c, Cast::ZExt, vals[2 * k + 1], i64t);
                let k32 = l.s.m.int_const(i64t, 32);
                let hi = l.s.m.bin(c, Bin::Shl, hi, k32);
                l.s.m.bin(c, Bin::Or, lo, hi)
            };
            let mut value = value_at(self, 0);
            if op == "sub" {
                let z = if wide {
                    self.s.m.int_const(i64t, 0)
                } else {
                    self.s.m.i32(0)
                };
                value = self.s.m.bin(c, Bin::Sub, z, value);
            }
            let mut old = Vec::new();
            for k in 0..n {
                old.push(match inst.read(Reg::V(reg + k)) {
                    Some(v) if cache.glc => self.i32_of(v)?,
                    _ => self.s.m.i32(0),
                });
            }
            if wide {
                // SM 6.6's 64-bit atomics on typed resources in the heap: the
                // host device must support both (D3D12 options 9 and 11).
                self.s.flags |= flags::INT64 | flags::ATOMIC64_TYPED | flags::ATOMIC64_HEAP;
            }
            let h = self.handle(site, src, false, (kind | 0x1000, 0x105))?;
            let at = self.begin_guarded(guard);
            let (ty, suffix) = if wide {
                (i64t, "i64")
            } else {
                (self.s.t.i32, "i32")
            };
            let mut args = vec![h];
            let r = if code == u32::MAX {
                // cmpswap: data is (new, compare).
                args.extend(cs);
                let (new, compare) = (value, value_at(self, 1));
                args.extend([compare, new]);
                self.s.op(
                    self.cur,
                    &format!("atomicCompareExchange.{suffix}"),
                    Attr::NoUnwind,
                    ty,
                    79,
                    &args,
                )
            } else {
                args.push(self.s.m.i32(code));
                args.extend(cs);
                args.push(value);
                self.s.op(
                    self.cur,
                    &format!("atomicBinOp.{suffix}"),
                    Attr::NoUnwind,
                    ty,
                    78,
                    &args,
                )
            };
            let r = if wide {
                let (lo, hi) = self.split64(r);
                vec![lo, hi]
            } else {
                vec![r]
            };
            let inside: Vec<_> = r.into_iter().zip(old).collect();
            let r = self.end_guarded(at, &inside);
            if cache.glc {
                for (k, x) in (0..).zip(r) {
                    self.write(inst, Reg::V(reg + k), x, None);
                }
            }
            return Ok(());
        }
        if store {
            if name != "image_store" {
                return Err("an image store to a mip level");
            }
            // shortcut: stores are taken to write all four channels of a
            // float format (the console converts by the view's format).
            if *dmask != 0xF {
                return Err("an image store of some channels");
            }
            let iu = self.s.m.undef(self.s.t.i32);
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let Operand::Vector { reg, .. } = *data else {
                return Err("image store data");
            };
            let mut vals = Vec::new();
            for k in 0..4u8 {
                // d16: two f16 channels per VGPR, low half first.
                let r = if *d16 { k / 2 } else { k };
                let v = inst.read(Reg::V(reg + r)).ok_or("image store data")?;
                vals.push(if *d16 {
                    let mut x = self.i32_of(v)?;
                    if k % 2 == 1 {
                        let k16 = self.s.m.i32(16);
                        x = self.s.m.bin(self.cur, Bin::LShr, x, k16);
                    }
                    let f32t = self.s.t.f32;
                    self.s
                        .op(self.cur, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[x])
                } else {
                    self.float_of(v)?
                });
            }
            let h = self.handle(site, src, false, props)?;
            let at = self.begin_guarded(guard);
            let mut args = vec![h];
            args.extend(cs);
            args.extend(vals);
            args.push(self.s.m.i8(0xF));
            let void = self.s.t.void;
            self.s.op(
                self.cur,
                "textureStore.f32",
                Attr::NoUnwind,
                void,
                67,
                &args,
            );
            self.end_guarded(at, &[]);
            return Ok(());
        }
        let (op, sample) = match name {
            "image_sample" | "image_sample_o" => (60, true),
            "image_sample_lz" | "image_sample_l" => (62, true),
            "image_sample_b" => (61, true),
            "image_sample_c_lz" => (65, true),
            "image_sample_d" | "image_sample_d_cl" => (63, true),
            "image_get_lod" if self.s.pixel => (81, true),
            // Implicit derivatives: pixel shaders only.
            "image_sample_c" if self.s.pixel => (64, true),
            "image_gather4" | "image_gather4_lz" | "image_gather4_lz_o" | "image_gather4_l"
            | "image_gather4_l_o" => (73, true),
            "image_load" | "image_load_mip" => (66, false),
            "image_get_resinfo" => (72, false),
            _ => return Err(name),
        };
        if op == 72 {
            // Width, height, depth or layers, levels (per dmask) of the level
            // in the first address: DXIL's dimensions; a channel DXIL leaves
            // out reads 1 (UAVs: one level).
            let h = self.handle(site, src, false, props)?;
            let mip = if uav {
                self.s.m.undef(self.s.t.i32)
            } else {
                addr_val(self, 0)?
            };
            let i32t = self.s.t.i32;
            let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
            let d = self
                .s
                .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, mip]);
            let one = self.s.m.i32(1);
            let present = match kind {
                1 => [true, false, false, true],
                2 | 5 | 6 => [true, true, false, true],
                _ => [true, true, true, true],
            };
            let Operand::Vector { reg, .. } = *data else {
                return Err("an image result");
            };
            for (k, ch) in (0..4u32).filter(|ch| dmask >> ch & 1 != 0).enumerate() {
                let x = if ch == 3 && uav || !present[ch as usize] {
                    one
                } else {
                    self.s.m.extract(c, d, ch)
                };
                self.write(inst, Reg::V(reg + k as u8), x, guard);
            }
            return Ok(());
        }
        if uav && op != 66 {
            return Err("sampling a UAV");
        }
        let h = self.handle(site, src, false, props)?;
        let fu = self.s.m.undef(self.s.t.f32);
        let iu = self.s.m.undef(self.s.t.i32);
        let r = if sample {
            let ss = self
                .descriptors
                .get(&(site, true))
                .copied()
                .ok_or("an untracked sampler")?;
            if matches!(name, "image_gather4_l" | "image_gather4_l_o") {
                if *a16 || *d16 || *unorm || kind != 2 {
                    return Err(
                        "a gather at a level of 16-bit values, texel coordinates or not 2D",
                    );
                }
                let n = 3 + usize::from(name.ends_with("_o"));
                let a = (0..n)
                    .map(|k| addr_val(self, k))
                    .collect::<Result<Vec<_>, _>>()?;
                let lod = inst.read(Reg::V(regs[n - 1])).ok_or("an image address")?;
                return self.gather_at_level(inst, h, ss, &a, lod, guard);
            }
            // Comparison: the reference value is the first address after
            // the bias, and the sampler is a comparison sampler.
            let cmp = matches!(op, 64 | 65);
            if cmp && (*a16 || dmask.count_ones() != 1) {
                return Err("a comparison sample of 16-bit coordinates or several channels");
            }
            let sh = self.handle(site, ss, true, (14 | if cmp { 0x8000 } else { 0 }, 0))?;
            // Texel offsets (`_o`) come first: x in bits 0-5, y in 8-13,
            // signed (programmable offsets: gathers only).
            let offset_word = name.ends_with("_o");
            let first =
                usize::from(offset_word) + usize::from(name == "image_sample_b") + usize::from(cmp);
            // Coordinates the size divides (not the array slice); explicit
            // gradients (x then y) come before the coordinates.
            let spatial = coords - usize::from(kind == 6 || kind == 7);
            // (A cube's are two per direction, on the face.)
            let grads = match op {
                63 if kind == 5 => 4,
                63 => 2 * spatial,
                _ => 0,
            };
            let mut cs = vec![fu; 4];
            // With a16, sampling coordinates and LOD are f16.
            let to_float = |l: &mut Self, x: V| {
                if *a16 {
                    let f = l.s.t.f32;
                    l.s.op(l.cur, "legacyF16ToF32", Attr::ReadNone, f, 131, &[x])
                } else {
                    l.as_float(x)
                }
            };
            // A level query takes no array slice (the code may pass anything
            // there, e.g. an unwritten register).
            let read = if op == 81 { spatial } else { coords };
            for (k, slot) in cs.iter_mut().enumerate().take(read) {
                let x = addr_val(self, first + grads + k)?;
                *slot = to_float(self, x);
            }
            if kind == 5 {
                if *unorm || op == 81 {
                    return Err("a cube sample with texel coordinates or a level query");
                }
                let dir = self.cube_direction(cs[0], cs[1], cs[2]);
                cs[..3].copy_from_slice(&dir);
            }
            if *unorm {
                // Texel coordinates: divided by the size of level 0.
                let i32t = self.s.t.i32;
                let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
                let z = self.s.m.i32(0);
                let d = self
                    .s
                    .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, z]);
                for (k, slot) in cs.iter_mut().enumerate().take(spatial) {
                    let n = self.s.m.extract(c, d, k as u32);
                    let n = self.s.m.cast(c, Cast::UiToFp, n, self.s.t.f32);
                    *slot = self.s.m.bin(c, Bin::SDiv, *slot, n);
                }
            }
            if op == 81 {
                // The clamped then the unclamped level of detail.
                let Operand::Vector { reg, .. } = *data else {
                    return Err("an image result");
                };
                let f32t = self.s.t.f32;
                for (k, ch) in (0..2u32).filter(|ch| dmask >> ch & 1 != 0).enumerate() {
                    let clamped = self.s.m.bool(ch == 0);
                    let lod = self.s.op(
                        c,
                        "calculateLOD.f32",
                        Attr::ReadOnly,
                        f32t,
                        81,
                        &[h, sh, cs[0], cs[1], cs[2], clamped],
                    );
                    let x = self.as_int(lod);
                    self.write(inst, Reg::V(reg + k as u8), x, guard);
                }
                return Ok(());
            }
            let offsets = if kind == 5 {
                0
            } else {
                (coords - usize::from(kind == 6 || kind == 7)).min(3)
            };
            let zero = self.s.m.i32(0);
            let mut os = vec![iu; 3];
            for o in os.iter_mut().take(offsets) {
                *o = zero;
            }
            let mut args = vec![h, sh];
            args.extend(&cs);
            match op {
                60 => {
                    if offset_word {
                        // DXIL takes immediate offsets in -8..=7; the
                        // console's are 6-bit and may be computed.
                        let w = addr_val(self, 0)?;
                        let w = self
                            .s
                            .m
                            .int_value(w)
                            .ok_or("a sample offset that is not constant")?
                            as u32;
                        for (k, o) in os.iter_mut().enumerate().take(offsets) {
                            let x = (w >> (8 * k) & 0x3F) as i32;
                            let x = if x >= 32 { x - 64 } else { x };
                            if !(-8..=7).contains(&x) {
                                return Err("a sample offset past -8..7");
                            }
                            *o = self.s.m.i32(x as u32);
                        }
                    }
                    args.extend(&os);
                    args.push(fu);
                    self.s
                        .op(c, "sample.f32", Attr::ReadOnly, self.s.t.res_ret, 60, &args)
                }
                61 => {
                    let bias = addr_val(self, 0)?;
                    let bias = to_float(self, bias);
                    args.extend(&os);
                    args.extend([bias, fu]);
                    self.s.op(
                        c,
                        "sampleBias.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        61,
                        &args,
                    )
                }
                63 if kind == 5 => {
                    // A cube's gradients are the face's: ds, dt per step in x,
                    // then in y, in face coordinates (a face spans 1). The level
                    // the hardware picks from them, isotropically: log2 of the
                    // longer gradient in texels of a face, then the clamp.
                    // shortcut: sampled at that level, so a sampler's
                    // anisotropy is lost and its LOD bias comes after the
                    // clamp (the console adds it before); D3D12 cube sampling
                    // takes 3D gradients only. Upgrade if cube lookups blur or
                    // a biased sampler meets a binding clamp.
                    let mut g = [fu; 4];
                    for (k, x) in g.iter_mut().enumerate() {
                        let v = addr_val(self, first + k)?;
                        *x = to_float(self, v);
                    }
                    let i32t = self.s.t.i32;
                    let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
                    let z = self.s.m.i32(0);
                    let d = self
                        .s
                        .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, z]);
                    let w = self.s.m.extract(c, d, 0);
                    let w = self.s.m.cast(c, Cast::UiToFp, w, self.s.t.f32);
                    let mut len2 = [fu; 2];
                    for (k, l) in len2.iter_mut().enumerate() {
                        let a = self.s.m.bin(c, Bin::Mul, g[2 * k], g[2 * k]);
                        let b = self.s.m.bin(c, Bin::Mul, g[2 * k + 1], g[2 * k + 1]);
                        *l = self.s.m.bin(c, Bin::Add, a, b);
                    }
                    // FMax, Log (base 2): 0.5 log2(max(|dx|², |dy|²) w²).
                    let m = self.binary(35, len2[0], len2[1]);
                    let w2 = self.s.m.bin(c, Bin::Mul, w, w);
                    let m = self.s.m.bin(c, Bin::Mul, m, w2);
                    let half = self.s.m.f32(0.5f32.to_bits());
                    let l = self.unary(23, m);
                    let mut lod = self.s.m.bin(c, Bin::Mul, l, half);
                    if name.ends_with("_cl") {
                        let x = addr_val(self, first + grads + coords)?;
                        let clamp = to_float(self, x);
                        lod = self.binary(35, lod, clamp);
                    }
                    args.extend(&os);
                    args.push(lod);
                    self.s.op(
                        c,
                        "sampleLevel.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        62,
                        &args,
                    )
                }
                63 => {
                    // With a16 each direction's gradients start a VGPR.
                    if *a16 && spatial % 2 == 1 {
                        return Err("16-bit gradients of a 1D or 3D image");
                    }
                    let mut g = vec![fu; 6];
                    for k in 0..spatial {
                        let x = addr_val(self, first + k)?;
                        g[k] = to_float(self, x);
                        let y = addr_val(self, first + spatial + k)?;
                        g[3 + k] = to_float(self, y);
                    }
                    args.extend(&os);
                    args.extend(g);
                    // `_cl`: the least level of detail, after the coordinates.
                    let clamp = if name.ends_with("_cl") {
                        let x = addr_val(self, first + grads + coords)?;
                        to_float(self, x)
                    } else {
                        fu
                    };
                    args.push(clamp);
                    self.s.op(
                        c,
                        "sampleGrad.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        63,
                        &args,
                    )
                }
                64 | 65 => {
                    let reference = addr_val(self, first - 1)?;
                    let reference = self.as_float(reference);
                    args.extend(&os);
                    args.push(reference);
                    let name = if op == 64 {
                        args.push(fu);
                        "sampleCmp.f32"
                    } else {
                        "sampleCmpLevelZero.f32"
                    };
                    self.s
                        .op(c, name, Attr::ReadOnly, self.s.t.res_ret, op, &args)
                }
                62 => {
                    let lod = if name == "image_sample_l" {
                        let x = addr_val(self, coords)?;
                        to_float(self, x)
                    } else {
                        self.s.m.f32(0)
                    };
                    args.extend(&os);
                    args.push(lod);
                    self.s.op(
                        c,
                        "sampleLevel.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        62,
                        &args,
                    )
                }
                _ => {
                    if name == "image_gather4" {
                        return Err("a gather with implicit level");
                    }
                    if dmask.count_ones() != 1 {
                        return Err("a gather of several channels");
                    }
                    if offset_word {
                        let w = addr_val(self, 0)?;
                        for (k, at) in [(0, 26), (1, 18)] {
                            // Bits at..at+5 sign-extended: shift up, then down.
                            let left = self.s.m.i32(at);
                            let t = self.s.m.bin(c, Bin::Shl, w, left);
                            let k26 = self.s.m.i32(26);
                            os[k] = self.s.m.bin(c, Bin::AShr, t, k26);
                        }
                    }
                    args.extend(&os[..2]);
                    args.push(self.s.m.i32(dmask.trailing_zeros()));
                    self.s.op(
                        c,
                        "textureGather.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        73,
                        &args,
                    )
                }
            }
        } else {
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let mip = if name == "image_load_mip" {
                addr_val(self, coords)?
            } else if uav {
                // Our declarations have four channels.
                self.s.flags |= flags::TYPED_UAV_LOADS;
                iu
            } else {
                self.s.m.i32(0)
            };
            let mut args = vec![h, mip];
            args.extend(&cs);
            args.extend([iu, iu, iu]);
            self.s.op(
                c,
                "textureLoad.f32",
                Attr::ReadOnly,
                self.s.t.res_ret,
                66,
                &args,
            )
        };
        let Operand::Vector { reg, .. } = *data else {
            return Err("an image result");
        };
        let channels: Vec<u32> = if op == 73 {
            vec![0, 1, 2, 3]
        } else {
            (0..4).filter(|ch| dmask >> ch & 1 != 0).collect()
        };
        let mut xs = Vec::new();
        for ch in channels {
            let x = self.s.m.extract(c, r, ch);
            xs.push(if *d16 {
                // shortcut: d16 results are taken as floats (rounded to
                // nearest even); integer formats would keep 16 bits.
                let i32t = self.s.t.i32;
                self.s
                    .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[x])
            } else {
                self.as_int(x)
            });
        }
        if *d16 {
            // Two channels per VGPR, low half first (an odd last high half 0).
            let k16 = self.s.m.i32(16);
            xs = xs
                .chunks(2)
                .map(|p| match *p {
                    [lo, hi] => {
                        let hi = self.s.m.bin(c, Bin::Shl, hi, k16);
                        self.s.m.bin(c, Bin::Or, lo, hi)
                    }
                    _ => p[0],
                })
                .collect();
        }
        for (k, x) in xs.into_iter().enumerate() {
            self.write(inst, Reg::V(reg + k as u8), x, guard);
        }
        Ok(())
    }

    /// The direction a cube sample's face coordinates stand for: the
    /// console takes s and t in [1, 2] (`v_cubesc`/`v_cubetc` over
    /// `|v_cubema|`, plus 1.5) and the face id (`v_cubeid`), which the
    /// host's cube sampling turns back into the same texel.
    // shortcut: the view is taken as a cube, not a cube array (whose face
    // operand adds 8 × the slice).
    fn cube_direction(&mut self, s: V, t: V, face: V) -> [V; 3] {
        let c = self.cur;
        let k = |l: &mut Self, v: f32| l.s.m.f32(v.to_bits());
        let (two, three) = (k(self, 2.0), k(self, 3.0));
        let u = self.s.m.bin(c, Bin::Mul, s, two);
        let u = self.s.m.bin(c, Bin::Sub, u, three);
        let v = self.s.m.bin(c, Bin::Mul, t, two);
        let v = self.s.m.bin(c, Bin::Sub, v, three);
        let z = self.s.m.f32(0x8000_0000);
        let (nu, nv) = (
            self.s.m.bin(c, Bin::Sub, z, u),
            self.s.m.bin(c, Bin::Sub, z, v),
        );
        let (one, mone) = (k(self, 1.0), k(self, -1.0));
        let f = self.s.m.cast(c, Cast::FpToUi, face, self.s.t.i32);
        // Per face +x, -x, +y, -y, +z, -z.
        let table = [
            [one, mone, u, u, u, nu],
            [nv, nv, one, mone, nv, nv],
            [nu, u, v, nv, one, mone],
        ];
        table.map(|row| {
            let mut r = row[0];
            for (n, &x) in row.iter().enumerate().skip(1) {
                let kn = self.s.m.i32(n as u32);
                let is = self.s.m.cmp(c, pred::EQ, f, kn);
                r = self.s.m.select(c, is, x, r);
            }
            r
        })
    }

    /// `image_gather4_l(_o)`: DXIL gathers only from a view's first level,
    /// so the four texels are loaded from the level. Only what the title's
    /// culling shaders use: an immediate sampler without bias whose border
    /// is transparent black (a texel outside reads as a load outside: 0),
    /// and a level of detail proven whole (which level a fraction picks is
    /// not known; a whole one is that level, clamped to the sampler's and
    /// the view's).
    fn gather_at_level(
        &mut self,
        inst: &Inst,
        h: V,
        sampler: &Source,
        a: &[V],
        lod: Value,
        guard: Option<V>,
    ) -> Result<(), Why> {
        let Fields::Image { data, dmask, .. } = &inst.fields else {
            unreachable!()
        };
        let Source::Constant(w) = sampler else {
            return Err("a gather at a level through a sampler from memory");
        };
        let &[w0, w1, w2, w3, ..] = &w[..] else {
            return Err("a sampler constant");
        };
        let bits = |word: u32, at: u32, n: u32| word >> at & ((1 << n) - 1);
        // Clamp to a border (x, y) of transparent black; no anisotropy,
        // unnormalized or truncated coordinates, reduction or bias; whole
        // level limits.
        if w0 & 0x3F != 0x36
            || w0 & (7 << 9 | 1 << 15 | 1 << 19 | 1 << 27 | 3 << 29) != 0
            || bits(w3, 30, 2) != 0
            || bits(w2, 0, 20) != 0
            || w1 & 0x0F_F0FF != 0
        {
            return Err("a gather at a level through a sampler other than a black border's");
        }
        // No channel enabled: the first.
        let dmask = if *dmask == 0 { 1 } else { *dmask };
        if dmask.count_ones() != 1 {
            return Err("a gather of several channels");
        }
        let mip_filter = bits(w2, 26, 2) != 0;
        if mip_filter && self.whole(lod, &mut HashSet::new()) != Some(true) {
            return Err("a gather at a level that may not be whole");
        }
        let c = self.cur;
        let i32t = self.s.t.i32;
        let dims_t = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
        let zero = self.s.m.i32(0);
        let level = if mip_filter {
            // The sampler's limits, then the view's last level.
            let f = self.as_float(a[a.len() - 1]);
            let lo = self.s.m.f32((bits(w1, 0, 12) as f32 / 256.0).to_bits());
            let hi = self.s.m.f32((bits(w1, 12, 12) as f32 / 256.0).to_bits());
            let f = self.binary(35, f, lo);
            let f = self.binary(36, f, hi);
            let l = self.s.m.cast(c, Cast::FpToSi, f, i32t);
            let d = self
                .s
                .op(c, "getDimensions", Attr::ReadOnly, dims_t, 72, &[h, zero]);
            let levels = self.s.m.extract(c, d, 3);
            let one = self.s.m.i32(1);
            let last = self.s.m.bin(c, Bin::Sub, levels, one);
            let past = self.s.m.cmp(c, pred::SGT, l, last);
            self.s.m.select(c, past, last, l)
        } else {
            zero
        };
        let d = self
            .s
            .op(c, "getDimensions", Attr::ReadOnly, dims_t, 72, &[h, level]);
        let offset = (a.len() == 4).then(|| a[0]);
        let mut first = [zero; 2];
        for (k, at) in [(0, 26), (1, 18)] {
            let size = self.s.m.extract(c, d, k as u32);
            first[k] = self.gather_first(a[a.len() - 3 + k], size);
            if let Some(o) = offset {
                // Bits at..at+5 of the offset word sign-extended.
                let left = self.s.m.i32(at);
                let t = self.s.m.bin(c, Bin::Shl, o, left);
                let k26 = self.s.m.i32(26);
                let o = self.s.m.bin(c, Bin::AShr, t, k26);
                first[k] = self.s.m.bin(c, Bin::Add, first[k], o);
            }
        }
        // Texels (x, y + 1), (x + 1, y + 1), (x + 1, y), (x, y).
        let one = self.s.m.i32(1);
        let next = first.map(|f| self.s.m.bin(c, Bin::Add, f, one));
        let iu = self.s.m.undef(i32t);
        let Operand::Vector { reg, .. } = *data else {
            return Err("an image result");
        };
        for (k, (x, y)) in [
            (first[0], next[1]),
            (next[0], next[1]),
            (next[0], first[1]),
            (first[0], first[1]),
        ]
        .into_iter()
        .enumerate()
        {
            let args = [h, level, x, y, iu, iu, iu, iu];
            let r = self.s.op(
                c,
                "textureLoad.f32",
                Attr::ReadOnly,
                self.s.t.res_ret,
                66,
                &args,
            );
            let v = self.s.m.extract(c, r, dmask.trailing_zeros());
            let v = self.as_int(v);
            self.write(inst, Reg::V(reg + k as u8), v, guard);
        }
        Ok(())
    }

    /// The first texel of a gather's footprint along an axis of `size`
    /// texels at coordinate `u` (bits of a float): floor(u × size − 0.5)
    /// with the product exact and rounded to 1/256 of a texel, half up, as
    /// the hardware does (`gather_level_tests`), i.e.
    /// (floor(u × size × 512) − 255) >> 9, in 64-bit integers. NaN is 0;
    /// beyond ±2 every texel of the footprint is outside, as at ±2.
    fn gather_first(&mut self, u: V, size: V) -> V {
        let c = self.cur;
        let f = self.as_float(u);
        let num = self.s.m.cmp(c, pred::OEQ, f, f);
        let fz = self.s.m.f32(0);
        let f = self.s.m.select(c, num, f, fz);
        let (lo, hi) = (
            self.s.m.f32((-2.0f32).to_bits()),
            self.s.m.f32(2.0f32.to_bits()),
        );
        let f = self.binary(35, f, lo);
        let f = self.binary(36, f, hi);
        let x = self.as_int(f);
        // u = m × 2^(e − 150), e at least 1 (denormals).
        let k = |l: &mut Self, v: u32| l.s.m.i32(v);
        let (k23, kff) = (k(self, 23), k(self, 0xFF));
        let e = self.s.m.bin(c, Bin::LShr, x, k23);
        let e = self.s.m.bin(c, Bin::And, e, kff);
        let (kmant, zero, one) = (k(self, 0x7F_FFFF), k(self, 0), k(self, 1));
        let m = self.s.m.bin(c, Bin::And, x, kmant);
        let normal = self.s.m.cmp(c, pred::NE, e, zero);
        let implicit = k(self, 0x80_0000);
        let bit = self.s.m.select(c, normal, implicit, zero);
        let m = self.s.m.bin(c, Bin::Or, m, bit);
        let e = self.s.m.select(c, normal, e, one);
        // u × size × 512 = m × size × 2^(e − 141); |u| ≤ 2 keeps e ≤ 128.
        let k141 = k(self, 141);
        let shift = self.s.m.bin(c, Bin::Sub, k141, e);
        let k63 = k(self, 63);
        let far = self.s.m.cmp(c, pred::SGT, shift, k63);
        let shift = self.s.m.select(c, far, k63, shift);
        let i64t = self.s.m.int(64);
        self.s.flags |= flags::INT64;
        let m = self.s.m.cast(c, Cast::ZExt, m, i64t);
        let n = self.s.m.cast(c, Cast::ZExt, size, i64t);
        let p = self.s.m.bin(c, Bin::Mul, m, n);
        let z64 = self.s.m.int_const(i64t, 0);
        let neg = self.s.m.bin(c, Bin::Sub, z64, p);
        let negative = self.s.m.cmp(c, pred::SLT, x, zero);
        let p = self.s.m.select(c, negative, neg, p);
        let shift = self.s.m.cast(c, Cast::ZExt, shift, i64t);
        let g = self.s.m.bin(c, Bin::AShr, p, shift);
        let k255 = self.s.m.int_const(i64t, 255);
        let g = self.s.m.bin(c, Bin::Sub, g, k255);
        let k9 = self.s.m.int_const(i64t, 9);
        let g = self.s.m.bin(c, Bin::AShr, g, k9);
        self.s.m.cast(c, Cast::Trunc, g, self.s.t.i32)
    }

    /// Whether `v` holds, as a float, a whole number or an infinity in
    /// every lane: `Some(true)` and never NaN, `Some(false)` or NaN, `None`
    /// perhaps a fraction. Follows moves, selects, minimum and maximum,
    /// sums with constants, rounding and conversions from integers (a level
    /// of detail made with `ceil`), through phis and guarded writes' kept
    /// values.
    fn whole(&self, v: Value, seen: &mut HashSet<Value>) -> Option<bool> {
        if !seen.insert(v) {
            // Around a loop: what enters it decides.
            return Some(true);
        }
        let p = self.x.p;
        if let Some(&(b, i)) = self.phi_at.get(&v) {
            let mut never_nan = true;
            for &arg in &p.blocks[b].phis[i].args {
                never_nan &= self.whole(arg, seen)?;
            }
            return Some(never_nan);
        }
        let Some(&b) = self.def_block.get(&v) else {
            // A VGPR no one set reads zero.
            let vgpr = p
                .inputs
                .iter()
                .any(|&(r, x)| x == v && matches!(r, Reg::V(_)));
            let io = self.x.io.inputs.iter().any(|i| i.0 == v);
            return (vgpr && !io).then_some(true);
        };
        let (i, inst) = p.blocks[b]
            .insts
            .iter()
            .enumerate()
            .find(|(_, i)| i.writes.iter().any(|w| w.1 == v))?;
        let Fields::Valu(f) = &inst.fields else {
            return None;
        };
        if f.omod != 0 || f.sdwa.is_some() || f.dpp.is_some() {
            return None;
        }
        let mut never_nan = true;
        if self.x.st.guards[b][i].is_some() {
            let Some(Operand::Vector { reg, .. }) = f.dst else {
                return None;
            };
            never_nan = self.whole(inst.read(Reg::V(reg))?, seen)?;
        }
        // Per source: (never NaN, a finite constant).
        let mut src = |k: usize| -> Option<(bool, bool)> {
            let x = match *f.src.get(k)? {
                Operand::Vector { reg, .. } => {
                    return Some((self.whole(inst.read(Reg::V(reg))?, seen)?, false));
                }
                Operand::Const(c) => crate::resource::inline(c)?,
                Operand::Literal(x) => x,
                _ => return None,
            };
            let x = f32::from_bits(x);
            (x.is_infinite() || x.fract() == 0.0).then_some((true, x.is_finite()))
        };
        let r = match inst.name() {
            "v_cvt_f32_i32" | "v_cvt_f32_u32" => true,
            "v_floor_f32" | "v_ceil_f32" | "v_trunc_f32" | "v_rndne_f32" => false,
            "v_mov_b32" => src(0)?.0,
            "v_cndmask_b32" => src(0)?.0 & src(1)?.0,
            "v_max_f32" | "v_min_f32" => src(0)?.0 | src(1)?.0,
            "v_max3_f32" | "v_min3_f32" => src(0)?.0 | src(1)?.0 | src(2)?.0,
            "v_add_f32" | "v_sub_f32" | "v_subrev_f32" => {
                // Infinities of both signs make NaN.
                let (x, y) = (src(0)?, src(1)?);
                x.0 && y.0 && (x.1 || y.1)
            }
            _ => return None,
        };
        Some(r && never_nan)
    }
}
