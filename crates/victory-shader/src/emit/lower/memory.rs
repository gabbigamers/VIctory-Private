//! Memory at addresses: global accesses (`global_*`), the program's
//! constant table, title memory through the page table, and the marks
//! of pages written through buffer views.

use super::{Lower, MemoryEntry, Why};
use crate::emit::bitcode::{Attr, Bin, Cast, V, pred};
use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{Inst, Reg};
use crate::resource::{CODE, MEMORY, Source};
#[cfg(doc)]
use super::Parts;

/// Where a program's constant table starts in its code: where its
/// instructions end, unless a load at a fixed offset reads below that (then
/// at `lowest`, the lowest offset any load or descriptor names). The
/// program's constants follow its instructions, and a load at a computed
/// offset reads them, not instruction bytes.
fn code_table_start(p: &crate::ir::Program, addresses: &[&crate::resource::Address], lowest: u32) -> u32 {
    let code_end = p
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .map(|i| i.machine.offset + 4 * i.machine.words.len())
        .max()
        .unwrap_or(0) as u32;
    let fixed_below = addresses.iter().any(|a| a.dynamic.is_none() && a.offset < code_end);
    if fixed_below { lowest } else { lowest.max(code_end & !3) }
}

impl<'a> Lower<'a> {
    /// The dword of the program's code at byte `byte` (a constant table
    /// the compiler appends to the code), as an internal constant array.
    // shortcut: an index outside the table reads its last dword (the
    // console would read whatever memory follows the code).
    pub(super) fn code_dword(&mut self, byte: V) -> Result<V, Why> {
        let (g, base, len) = match self.code_table {
            Some(t) => t,
            None => {
                let addresses: Vec<&crate::resource::Address> = self
                    .x
                    .res
                    .scalar
                    .iter()
                    .map(|s| &s.1)
                    .chain(self.x.res.descriptors.iter().flat_map(|d| {
                        d.source.leaves().into_iter().filter_map(|l| match l {
                            Source::Built { address, .. } => Some(address),
                            _ => None,
                        })
                    }))
                    .filter(|a| a.param == CODE)
                    .collect();
                let lowest = addresses.iter().map(|a| a.offset & !3).min().ok_or("a code table")?;
                let first = code_table_start(self.x.p, &addresses, lowest);
                let code = &self.x.native.code;
                let words: Vec<u32> = code
                    .get(first as usize..)
                    .ok_or("a code table past the code")?
                    .chunks(4)
                    .map(|c| {
                        let mut w = [0u8; 4];
                        w[..c.len()].copy_from_slice(c);
                        u32::from_le_bytes(w)
                    })
                    .collect();
                if words.is_empty() {
                    return Err("a code table past the code");
                }
                let elems: Vec<V> = words.iter().map(|&w| self.s.m.i32(w)).collect();
                let arr = self.s.m.array(words.len() as u32, self.s.t.i32);
                let init = self.s.m.aggregate(arr, &elems);
                let g = self.s.m.constant_global("code", init);
                let t = (g, first, words.len() as u32);
                self.code_table = Some(t);
                t
            }
        };
        let c = self.cur;
        let k = self.s.m.i32(base);
        let rel = self.s.m.bin(c, Bin::Sub, byte, k);
        let two = self.s.m.i32(2);
        let index = self.s.m.bin(c, Bin::LShr, rel, two);
        let last = self.s.m.i32(len - 1);
        let over = self.s.m.cmp(c, pred::IUGT, index, last);
        let index = self.s.m.select(c, over, last, index);
        let at = self.s.m.element(c, g, index);
        Ok(self.s.m.load(c, at))
    }

    /// A global memory access (`global_*`): loads from the program's
    /// constant table, from a root view at the address the title sets, or
    /// from title memory at an address the program computes; stores and
    /// atomics through a root view.
    pub(super) fn flat(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Flat { dst, .. } = &inst.fields else {
            unreachable!()
        };
        let name = inst.name();
        if !name.starts_with("global_load_") {
            return self.flat_write(inst, b, i);
        }
        let rest = &name["global_load_".len()..];
        let (n, sub) = match rest {
            "dword" => (1, None),
            "dwordx2" => (2, None),
            "dwordx3" => (3, None),
            "dwordx4" => (4, None),
            "ubyte" => (1, Some((8, false))),
            "sbyte" => (1, Some((8, true))),
            "ushort" => (1, Some((16, false))),
            "sshort" => (1, Some((16, true))),
            _ => return Err(name),
        };
        let a = *self
            .scalar
            .get(&(b, i))
            .ok_or("an untracked global address")?;
        let guard = self.guard(b, i)?;
        let Some(Operand::Vector { reg, .. }) = *dst else {
            return Err("a global load result");
        };
        let c = self.cur;
        let bytes = sub.map_or(4 * n, |(width, _)| width / 8);
        let (h, off) = if a.param == MEMORY {
            let (lo, hi) = self.global_address(inst)?;
            let (h, off) = self.memory(lo, hi, bytes, false, guard);
            (Some(h), off)
        } else {
            let mut off = self.s.m.i32(a.offset);
            if let Some(d) = a.dynamic {
                let d = self.i32_of(d)?;
                off = self.s.m.bin(c, Bin::Add, off, d);
            }
            if a.param == CODE {
                (None, off)
            } else {
                let src = Source::Built {
                    address: a,
                    bits: 0,
                    words: [0, 0],
                };
                let (h, off) = self.raw_view((b, i), &src, off, bytes, (11, 0), false, guard)?;
                (Some(h), off)
            }
        };
        let c = self.cur;
        let dword = |l: &mut Self, byte: V| -> Result<V, Why> {
            match h {
                None => l.code_dword(byte),
                Some(h) => {
                    let c = l.cur;
                    let (u, mask, al) = (l.s.m.undef(l.s.t.i32), l.s.m.i8(1), l.s.m.i32(4));
                    let r = l.s.op(
                        c,
                        "rawBufferLoad.f32",
                        Attr::ReadOnly,
                        l.s.t.res_ret,
                        139,
                        &[h, byte, u, mask, al],
                    );
                    let x = l.s.m.extract(c, r, 0);
                    Ok(l.as_int(x))
                }
            }
        };
        for k in 0..n {
            let x = match sub {
                None => {
                    let four = self.s.m.i32(4 * k);
                    let at = self.s.m.bin(c, Bin::Add, off, four);
                    dword(self, at)?
                }
                Some((width, signed)) => {
                    // shortcut: the field is taken not to cross a dword.
                    let (not3, three) = (self.s.m.i32(!3), self.s.m.i32(3));
                    let at = self.s.m.bin(c, Bin::And, off, not3);
                    let w = dword(self, at)?;
                    let byte = self.s.m.bin(c, Bin::And, off, three);
                    let sh = self.s.m.bin(c, Bin::Shl, byte, three);
                    let x = self.s.m.bin(c, Bin::LShr, w, sh);
                    let left = self.s.m.i32(32 - width);
                    let x = self.s.m.bin(c, Bin::Shl, x, left);
                    self.s
                        .m
                        .bin(c, if signed { Bin::AShr } else { Bin::LShr }, x, left)
                }
            };
            self.write(inst, Reg::V(reg + k as u8), x, guard);
        }
        Ok(())
    }

    /// The 64-bit address a `global_*` instruction accesses in each lane:
    /// its SGPR pair plus its VGPR offset, or its VGPR pair, plus its
    /// immediate; the high dword cut to address bits 32-47.
    fn global_address(&mut self, inst: &Inst) -> Result<(V, V), Why> {
        const WHY: Why = "a global address";
        let Fields::Flat {
            addr: Some(Operand::Vector { reg, .. }),
            saddr,
            offset,
            ..
        } = inst.fields
        else {
            return Err(WHY);
        };
        let read = |l: &mut Self, r: Reg| -> Result<V, Why> {
            let v = inst.read(r).ok_or(WHY)?;
            l.i32_of(v)
        };
        let (lo, hi) = match saddr {
            Some(Operand::Scalar { code, .. }) if code != NULL => {
                let (lo, hi) = (read(self, Reg::S(code))?, read(self, Reg::S(code + 1))?);
                let x = read(self, Reg::V(reg))?;
                self.add64(lo, hi, x, false)
            }
            _ => (read(self, Reg::V(reg))?, read(self, Reg::V(reg + 1))?),
        };
        let k = self.s.m.i32(offset as i32 as u32);
        let (lo, hi) = self.add64(lo, hi, k, offset < 0);
        let mask = self.s.m.i32(0xFFFF);
        Ok((lo, self.s.m.bin(self.cur, Bin::And, hi, mask)))
    }

    /// A global store or atomic (`global_store_*`, `global_atomic_*`)
    /// through a root view at the address the title sets, or into title
    /// memory at an address the program computes.
    fn flat_write(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Flat {
            dst, data, cache, ..
        } = &inst.fields
        else {
            unreachable!()
        };
        let name = inst.name();
        let a = *self
            .scalar
            .get(&(b, i))
            .ok_or("an untracked global address")?;
        if a.param == CODE {
            return Err("a global store into the code");
        }
        let n = match name.strip_prefix("global_store_") {
            None => 1,
            Some("dword") => 1,
            Some("dwordx2") => 2,
            Some("dwordx3") => 3,
            Some("dwordx4") => 4,
            Some(_) => return Err(name),
        };
        // DXIL atomic operations: add, and, or, xor, signed and unsigned min
        // and max, exchange; subtraction adds the negation. A float minimum
        // or maximum is the signed integer one where the value is positive
        // and the reversed unsigned one where negative (IEEE order of the
        // bits), as two atomics, each a no-op in the lanes the other serves.
        let ops: Option<&[(u32, u32)]> = match name.strip_prefix("global_atomic_") {
            None => None,
            Some("add" | "sub") => Some(&[(0, 0)]),
            Some("and") => Some(&[(1, 0)]),
            Some("or") => Some(&[(2, 0)]),
            Some("xor") => Some(&[(3, 0)]),
            Some("smin") => Some(&[(4, 0)]),
            Some("smax") => Some(&[(5, 0)]),
            Some("umin") => Some(&[(6, 0)]),
            Some("umax") => Some(&[(7, 0)]),
            Some("swap") => Some(&[(8, 0)]),
            Some("fmin") if !cache.glc => Some(&[(4, i32::MAX as u32), (7, 0)]),
            Some("fmax") if !cache.glc => Some(&[(5, i32::MIN as u32), (6, u32::MAX)]),
            Some(_) => return Err(name),
        };
        if ops.is_none() && !name.starts_with("global_store_") {
            return Err(name);
        }
        let guard = self.guard(b, i)?;
        let c = self.cur;
        let d = data.as_ref().ok_or("global store data")?;
        let vals = self.vgprs(inst, d, n)?;
        let (memory, root) = if a.param == MEMORY {
            (Some(self.global_address(inst)?), None)
        } else {
            let mut off = self.s.m.i32(a.offset);
            if let Some(d) = a.dynamic {
                let d = self.i32_of(d)?;
                off = self.s.m.bin(c, Bin::Add, off, d);
            }
            let src = Source::Built {
                address: a,
                bits: 0,
                words: [0, 0],
            };
            match self.root_address(&src) {
                Some((lo, hi)) => {
                    // As `raw_view`, looked up inside the guard below.
                    let (lo, hi) = self.add64(lo, hi, off, false);
                    let mask = self.s.m.i32(0xFFFF);
                    (Some((lo, self.s.m.bin(self.cur, Bin::And, hi, mask))), None)
                }
                None => {
                    let h = self.handle((b, i), &src, false, (11 | 0x1000, 0))?;
                    let off = match self.root_view_offset(&src) {
                        Some(at) => self.s.m.bin(self.cur, Bin::Add, off, at),
                        None => off,
                    };
                    (None, Some((h, off)))
                }
            }
        };
        let (iu, void, i32t) = (self.s.m.undef(self.s.t.i32), self.s.t.void, self.s.t.i32);
        // Title memory is looked up (and its pages marked written) only in
        // the lanes that write.
        let at = self.begin_guarded(guard);
        let (h, off) = match (memory, root) {
            (Some((lo, hi)), _) => self.memory(lo, hi, 4 * u32::from(n), true, None),
            (_, Some(r)) => r,
            _ => unreachable!(),
        };
        if name.starts_with("global_store_") {
            let mut args = vec![h, off, iu];
            args.extend(&vals);
            args.resize(3 + 4, iu);
            args.push(self.s.m.i8((1u8 << n) - 1));
            args.push(self.s.m.i32(4));
            self.s
                .op(self.cur, "rawBufferStore.i32", Attr::NoUnwind, void, 140, &args);
            self.mark_view_write(h, off, 4 * u32::from(n), None);
            self.end_guarded(at, &[]);
            return Ok(());
        }
        let ops = ops.expect("an atomic");
        let mut x = vals[0];
        let c = self.cur;
        if name == "global_atomic_sub" {
            let zero = self.s.m.i32(0);
            x = self.s.m.bin(c, Bin::Sub, zero, x);
        }
        let negative = {
            let zero = self.s.m.i32(0);
            self.s.m.cmp(c, pred::SLT, x, zero)
        };
        let returned = match *dst {
            Some(Operand::Vector { reg, .. }) if cache.glc => Some(reg),
            _ => None,
        };
        let mut first = None;
        for (k, &(op, neutral)) in ops.iter().enumerate() {
            let value = if ops.len() == 1 {
                x
            } else {
                let n = self.s.m.i32(neutral);
                // The first serves positive values, the second negative ones.
                if k == 0 {
                    self.s.m.select(self.cur, negative, n, x)
                } else {
                    self.s.m.select(self.cur, negative, x, n)
                }
            };
            let op = self.s.m.i32(op);
            let r = self.s.op(
                self.cur,
                "atomicBinOp.i32",
                Attr::NoUnwind,
                i32t,
                78,
                &[h, op, off, iu, iu, value],
            );
            first.get_or_insert(r);
        }
        self.mark_view_write(h, off, 4, None);
        let zero = self.s.m.i32(0);
        let r = self.end_guarded(at, &[(first.expect("an atomic"), zero)])[0];
        if let Some(reg) = returned {
            self.write(inst, Reg::V(reg), r, guard);
        }
        Ok(())
    }

    /// `(hi:lo) + x` with `x` zero-extended, or sign-extended when
    /// `negative` says its top bit means it.
    pub(super) fn add64(&mut self, lo: V, hi: V, x: V, negative: bool) -> (V, V) {
        let c = self.cur;
        let sum = self.s.m.bin(c, Bin::Add, lo, x);
        let carry = self.s.m.cmp(c, pred::IULT, sum, lo);
        let carry = self.s.m.cast(c, Cast::ZExt, carry, self.s.t.i32);
        let hi = self.s.m.bin(c, Bin::Add, hi, carry);
        let hi = if negative {
            let all = self.s.m.i32(u32::MAX);
            self.s.m.bin(c, Bin::Add, hi, all)
        } else {
            hi
        };
        (sum, hi)
    }

    /// The dword at byte `at` of raw buffer `h`.
    pub(super) fn raw_dword(&mut self, h: V, at: V) -> V {
        let c = self.cur;
        let (u, mask, al) = (self.s.m.undef(self.s.t.i32), self.s.m.i8(1), self.s.m.i32(4));
        let r = self.s.op(
            c,
            "rawBufferLoad.f32",
            Attr::ReadOnly,
            self.s.t.res_ret,
            139,
            &[h, at, u, mask, al],
        );
        let x = self.s.m.extract(c, r, 0);
        self.as_int(x)
    }

    /// Marks the pages a write of `bytes` bytes at byte `x` of the unordered
    /// buffer view `h` touches, in the lanes `guard` lets through (module
    /// notes, "Write marks"): the bits of its first and last byte, each set
    /// only where still clear. Nothing for handles that are no heap view, or
    /// without [`Parts::view_marks`].
    pub(super) fn mark_view_write(&mut self, h: V, x: V, bytes: u32, guard: Option<V>) {
        if !self.x.view_marks {
            return;
        }
        let Some(&index) = self.s.view_index.get(&h) else {
            return;
        };
        let k = |l: &mut Self, v: u32| l.s.m.i32(v);
        let table = self.s.view_marks_handle();
        let c = self.cur;
        let (eight, four) = (k(self, 8), k(self, 4));
        let at = self.s.m.bin(c, Bin::Mul, index, eight);
        let base = self.raw_dword(table, at);
        let at = self.s.m.bin(c, Bin::Add, at, four);
        let phase = self.raw_dword(table, at);
        let none = k(self, super::VIEW_MARK_NONE);
        let valid = self.s.m.cmp(c, pred::NE, base, none);
        let active = match guard {
            Some(g) => self.s.m.bin(c, Bin::And, g, valid),
            None => valid,
        };
        let first = self.s.m.bin(c, Bin::Add, phase, x);
        let span = k(self, bytes.max(1) - 1);
        let last = self.s.m.bin(c, Bin::Add, first, span);
        let (k16, cap) = (k(self, 16), k(self, super::VIEW_WRITE_BITS - 1));
        let mut bits = Vec::new();
        for byte in [first, last] {
            let page = self.s.m.bin(c, Bin::LShr, byte, k16);
            let n = self.s.m.bin(c, Bin::Add, base, page);
            let over = self.s.m.cmp(c, pred::IUGT, n, cap);
            bits.push(self.s.m.select(c, over, cap, n));
        }
        let bitmap = self.s.view_writes_handle();
        let outer = self.begin_guarded(Some(active));
        for (i, &n) in bits.iter().enumerate() {
            let c = self.cur;
            let (k5, k2, k31, one, zero) = (k(self, 5), k(self, 2), k(self, 31), k(self, 1), k(self, 0));
            let word = self.s.m.bin(c, Bin::LShr, n, k5);
            let word = self.s.m.bin(c, Bin::Shl, word, k2);
            let bit = self.s.m.bin(c, Bin::And, n, k31);
            let bit = self.s.m.bin(c, Bin::Shl, one, bit);
            // Most writes find their page marked: a load, not an atomic.
            let old = self.raw_dword(bitmap, word);
            let set = self.s.m.bin(c, Bin::And, old, bit);
            let mut clear = self.s.m.cmp(c, pred::EQ, set, zero);
            if i == 1 {
                let other = self.s.m.cmp(c, pred::NE, n, bits[0]);
                clear = self.s.m.bin(c, Bin::And, clear, other);
            }
            let inner = self.begin_guarded(Some(clear));
            let iu = self.s.m.undef(self.s.t.i32);
            // Operation 2 is OR.
            self.s.op(self.cur, "atomicBinOp.i32", Attr::NoUnwind, self.s.t.i32, 78, &[bitmap, k2, word, iu, iu, bit]);
            self.end_guarded(inner, &[]);
        }
        self.end_guarded(outer, &[]);
    }

    /// Title memory at a raw GPU address (decision 0005: the unified-memory
    /// item). Every access to an address the program computes goes through
    /// here: scalar and global loads, stores and atomics, the BVH node
    /// fetch, and in a program that uses title memory its root SRVs and UAVs
    /// ([`Self::raw_view`]). `(hi:lo)` is the 64-bit address of the access's
    /// first byte (`hi` holds bits 32-47), `bytes` its size, `write` whether
    /// it stores or does atomics, `active` the lanes that access (`None`:
    /// all; the others look up but mark nothing). Returns a raw buffer handle
    /// (read-write with `write`) and the address's byte offset in it; the
    /// access's dwords follow at `+ 4k`. Memory no host copy holds is the
    /// null view: reads give 0, writes are dropped. Contract (`emit` "Title
    /// memory"): a page table in the root SRV `t0` of
    /// [`super::shader::MEMORY_SPACE`]; the root UAV `u0` there takes a bit
    /// per page written and per page or 256 MiB region missed. May end in a
    /// new block: callers take [`Lower::cur`] afresh.
    // shortcut: one lookup per access, so its bytes are taken to lie in one
    // host copy of title memory (any access within a 64 KiB page does).
    pub(crate) fn memory(
        &mut self,
        lo: V,
        hi: V,
        bytes: u32,
        write: bool,
        active: Option<V>,
    ) -> (V, V) {
        let e = self.memory_entry(lo, hi, write);
        let c = self.cur;
        // Writes always mark (written or missed pages), reads only misses.
        let marks = match (write, active) {
            (true, Some(a)) => Some(a),
            (true, None) => None,
            (false, Some(a)) => Some(self.s.m.bin(c, Bin::And, e.miss, a)),
            (false, None) => Some(e.miss),
        };
        // A miss marks its page's entry, or with no block its region's
        // level-1 index (bits 0-4095, block 0's entries, which no copy has).
        let missed = self.memory_mark_index(e.entry, e.block, e.l1);
        let first = self.s.m.select(c, e.miss, missed, e.entry);
        let bitmap = self.s.memory_dirty_handle();
        let at = self.begin_guarded(marks);
        self.mark_memory_bit(bitmap, first);
        self.end_guarded(at, &[]);
        if write && bytes > 1 {
            // A multi-byte write crossing a 64 KiB boundary can enter another
            // 256 MiB region or cross 4 GiB, so look up its last byte in the
            // page table rather than offsetting `e.entry`.
            let c = self.cur;
            let k = |l: &mut Self, x: u32| l.s.m.i32(x);
            let (last, k16, mask) = (k(self, bytes - 1), k(self, 16), k(self, 0xFFFF));
            let (end_lo, end_hi) = self.add64(lo, hi, last, false);
            let end_hi = self.s.m.bin(c, Bin::And, end_hi, mask);
            let a = self.s.m.bin(c, Bin::LShr, lo, k16);
            let b = self.s.m.bin(c, Bin::LShr, end_lo, k16);
            let crossed = self.s.m.cmp(c, pred::NE, a, b);
            let t = self.s.m.bool(true);
            let not_miss = self.s.m.bin(c, Bin::Xor, e.miss, t);
            let mut cross = self.s.m.bin(c, Bin::And, not_miss, crossed);
            if let Some(a) = active {
                cross = self.s.m.bin(c, Bin::And, cross, a);
            }
            let at = self.begin_guarded(Some(cross));
            let (entry2, block2, l1_2) = self.memory_page(end_lo, end_hi);
            let second = self.memory_mark_index(entry2, block2, l1_2);
            self.mark_memory_bit(bitmap, second);
            self.end_guarded(at, &[]);
        }
        (e.handle, e.offset)
    }

    /// Bitmap bit for a page-table lookup: `l1` in block 0 when the 256 MiB
    /// region has no block, otherwise `entry`.
    fn memory_mark_index(&mut self, entry: V, block: V, l1: V) -> V {
        let c = self.cur;
        let zero = self.s.m.i32(0);
        let empty = self.s.m.cmp(c, pred::EQ, block, zero);
        self.s.m.select(c, empty, l1, entry)
    }

    /// Sets bit `n` in the title-memory dirty bitmap `bitmap`.
    fn mark_memory_bit(&mut self, bitmap: V, n: V) {
        let c = self.cur;
        let k = |l: &mut Self, x: u32| l.s.m.i32(x);
        let (k5, k31, k2, one) = (k(self, 5), k(self, 31), k(self, 2), k(self, 1));
        let iu = self.s.m.undef(self.s.t.i32);
        // Byte offset of the bitmap dword; operation 2 is OR.
        let word = self.s.m.bin(c, Bin::LShr, n, k5);
        let word = self.s.m.bin(c, Bin::Shl, word, k2);
        let bit = self.s.m.bin(c, Bin::And, n, k31);
        let bit = self.s.m.bin(c, Bin::Shl, one, bit);
        self.s.op(
            c,
            "atomicBinOp.i32",
            Attr::NoUnwind,
            self.s.t.i32,
            78,
            &[bitmap, k2, word, iu, iu, bit],
        );
    }

    /// The level-2 page-table index, block, and level-1 index of title
    /// address `(hi:lo)` (`block == 0` when the 256 MiB region has no block).
    fn memory_page(&mut self, lo: V, hi: V) -> (V, V, V) {
        let c = self.cur;
        let map = self.s.memory_handle();
        let k = |l: &mut Self, x: u32| l.s.m.i32(x);
        // Level 1: (hi:lo) >> 28, 4096 entries for 40-bit addresses.
        let (k4, k28, k256) = (k(self, 4), k(self, 28), k(self, 256));
        let top = self.s.m.bin(c, Bin::Shl, hi, k4);
        let low = self.s.m.bin(c, Bin::LShr, lo, k28);
        let l1 = self.s.m.bin(c, Bin::Or, top, low);
        let inside = self.s.m.cmp(c, pred::IULT, hi, k256);
        let zero = k(self, 0);
        let l1 = self.s.m.select(c, inside, l1, zero);
        let two = k(self, 2);
        let at = self.s.m.bin(c, Bin::Shl, l1, two);
        let block = self.raw_dword(map, at);
        let block = self.s.m.select(c, inside, block, zero);
        // Level 2: block b holds 4096 entries of two dwords after level 1.
        let (k16, k12, mask) = (k(self, 16), k(self, 12), k(self, 0xFFF));
        let page = self.s.m.bin(c, Bin::LShr, lo, k16);
        let page = self.s.m.bin(c, Bin::And, page, mask);
        let entry = self.s.m.bin(c, Bin::Shl, block, k12);
        let entry = self.s.m.bin(c, Bin::Or, entry, page);
        (entry, block, l1)
    }

    /// [`Self::memory`]'s lookup in the page table.
    fn memory_entry(&mut self, lo: V, hi: V, write: bool) -> MemoryEntry {
        let (entry, block, l1) = self.memory_page(lo, hi);
        let c = self.cur;
        let map = self.s.memory_handle();
        let k = |l: &mut Self, x: u32| l.s.m.i32(x);
        let (k3, first, four) = (k(self, 3), k(self, 4096 * 4), k(self, 4));
        let at = self.s.m.bin(c, Bin::Shl, entry, k3);
        let at = self.s.m.bin(c, Bin::Add, at, first);
        let view = self.raw_dword(map, at);
        let at = self.s.m.bin(c, Bin::Add, at, four);
        let start = self.raw_dword(map, at);
        // Block 0's first entry names the null pair: no copy holds the page.
        let null = self.raw_dword(map, first);
        let miss = self.s.m.cmp(c, pred::EQ, view, null);
        // Views come in pairs: the raw SRV, then the raw UAV of one copy.
        let (view, props) = if write {
            let one = k(self, 1);
            (self.s.m.bin(c, Bin::Add, view, one), (11 | 0x1000, 0))
        } else {
            (view, (11, 0))
        };
        let handle = self.s.heap_handle(c, view, false, true, props);
        // Pages written here are marked in the page table's bitmap.
        self.s.view_index.remove(&handle);
        MemoryEntry {
            handle,
            offset: self.s.m.bin(c, Bin::Sub, lo, start),
            entry,
            block,
            l1,
            miss,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::S_ENDPGM;
    use super::code_table_start;
    use crate::{ir, resource};

    /// A program's constant table starts after its instructions, unless a
    /// load at a fixed offset reads below that.
    #[test]
    fn the_code_table_starts_after_the_instructions() {
        // s_nop 0, s_endpgm, then 1,000 dwords of constants.
        let mut words = vec![0xBF80_0000, S_ENDPGM];
        words.extend(0..1000u32);
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let p = ir::build(&code, false, 256).unwrap();
        let at = |offset: u32, dynamic: bool| resource::Address {
            param: resource::CODE,
            offset,
            dynamic: dynamic.then_some(ir::Value(0)),
        };
        let (computed, fixed_after, fixed_inside) = (at(0, true), at(12, false), at(4, false));
        // A load at a computed offset from the code's start: the table is
        // the constants only, not the whole program.
        assert_eq!(code_table_start(&p, &[&computed], 0), 8);
        assert_eq!(code_table_start(&p, &[&computed, &fixed_after], 0), 8);
        // A load at a fixed offset inside the instructions keeps the start.
        assert_eq!(code_table_start(&p, &[&computed, &fixed_inside], 0), 0);
        // Loads past the instructions only: from the lowest of them.
        assert_eq!(code_table_start(&p, &[&fixed_after], 12), 12);
    }
}
