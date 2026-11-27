//! User data, root views and descriptors: the handle of the descriptor
//! an instruction reads and the heap index it is made from.

use super::{EntryAddress, Lower, Val, Why};
use crate::emit::UserData;
use crate::emit::bitcode::{Attr, Bin, V, pred};
use crate::emit::shader::{VIEW_STRIDE_BITS, VIEW_STRIDES, flags};
use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{Inst, Reg};
use crate::native::{ResourceType, RootType, UsedResource};
use crate::resource::{MEMORY, Site, Source};

impl<'a> Lower<'a> {
    /// A user-data dword: a root parameter's value.
    pub(super) fn user(&mut self, param: usize, dword: u32) -> Val {
        let rp = self.x.native.layout[param];
        match rp.root {
            RootType::ResourceHeap | RootType::SamplerHeap => Val::I32(self.s.m.i32(0)),
            RootType::Constants | RootType::Table => Val::I32(self.user_dword(param, dword)),
            // Its address, where root views are title memory.
            RootType::Descriptor if self.root_memory => Val::I32(self.user_dword(param, dword)),
            _ => Val::Missing("a root descriptor"),
        }
    }

    /// User-data dword `dword` of root parameter `param` as the host passes
    /// it (for a root descriptor: its GPU address, low dword first).
    fn user_dword(&mut self, param: usize, dword: u32) -> V {
        let rp = self.x.native.layout[param];
        let index = u32::from(rp.user_data) + dword;
        self.user_at(UserData {
            index,
            slot: rp.slot,
            root: rp.root,
            dword,
        })
    }

    /// A root SRV or UAV (`uav`) of root parameter `param`, in a program
    /// that does not read title memory: the resource-heap view the host
    /// makes of the host copy holding the buffer, whose index comes in the
    /// root descriptor's own first user-data dword (module notes, "Root SRVs
    /// and root UAVs"); offsets from the root address move by
    /// [`Lower::root_view_offset`]. Host root views have no bounds; a view's
    /// reads past the copy give 0 and its writes there are dropped.
    fn root_view(&mut self, param: usize, uav: bool) -> V {
        let rp = self.x.native.layout[param];
        // The descriptor's two user-data dwords hold its address on the
        // console; such a program never reads them as data (`user`).
        let index = u32::from(rp.user_data);
        if let Some(&h) = self.root_views.get(&index) {
            return h;
        }
        let i = self.user_at(UserData {
            index,
            slot: rp.slot,
            root: RootType::Descriptor,
            dword: if uav { super::ROOT_UAV_VIEW } else { super::ROOT_SRV_VIEW },
        });
        let props = if uav { (11 | 0x1000, 0) } else { (11, 0) };
        let h = self.s.heap_handle(self.s.entry, i, false, false, props);
        self.root_views.insert(index, h);
        h
    }

    /// For a root SRV or UAV read through the view of its host copy
    /// ([`Lower::root_view`]): the root address's byte offset in that view,
    /// from the descriptor's second user-data dword. None for other sources.
    pub(super) fn root_view_offset(&mut self, src: &Source) -> Option<V> {
        let Source::Built { address, .. } = src else {
            return None;
        };
        let rp = *self.x.native.layout.get(address.param)?;
        let cbv = self.used_kind(src).is_some_and(|u| u.kind == ResourceType::Cbv);
        if self.root_memory || rp.root != RootType::Descriptor || cbv {
            return None;
        }
        Some(self.user_at(UserData {
            index: u32::from(rp.user_data) + 1,
            slot: rp.slot,
            root: RootType::Descriptor,
            dword: super::ROOT_VIEW_OFFSET,
        }))
    }

    /// User-data dword `u.index`, which the host fills as `u` says, loaded
    /// in the entry block.
    pub(super) fn user_at(&mut self, u: UserData) -> V {
        let index = u.index;
        self.user_reads.insert(index, u);
        self.s.user_data = self.s.user_data.max(index + 1);
        let h = match self.user_data {
            Some(h) => h,
            None => {
                let h = self.s.user_data_handle();
                self.user_data = Some(h);
                h
            }
        };
        let entry = self.s.entry;
        let row = self.s.m.i32(index / 4);
        let r = self.s.op(
            entry,
            "cbufferLoadLegacy.i32",
            Attr::ReadOnly,
            self.s.t.cbuf_ret,
            59,
            &[h, row],
        );
        self.s.m.extract(entry, r, index % 4)
    }

    /// In a program that uses title memory, the address of the root SRV or
    /// UAV a descriptor `src` is built around: `(lo, hi)`, `hi` cut to bits
    /// 32-47. None for other descriptors and programs.
    pub(super) fn root_address(&mut self, src: &Source) -> Option<(V, V)> {
        let Source::Built { address, .. } = src else {
            return None;
        };
        let rp = self.x.native.layout.get(address.param)?;
        let cbv = self.used_kind(src).is_some_and(|u| u.kind == ResourceType::Cbv);
        if !self.root_memory || rp.root != RootType::Descriptor || cbv {
            return None;
        }
        let lo = self.user_dword(address.param, 0);
        let hi = self.user_dword(address.param, 1);
        let mask = self.s.m.i32(0xFFFF);
        Some((lo, self.s.m.bin(self.s.entry, Bin::And, hi, mask)))
    }

    /// A raw view `src` at byte offset `off` (`props` as [`Self::handle`]
    /// takes them): its handle and the offset in it. A root view of a
    /// program that uses title memory is title memory at the root address
    /// plus the offset ([`Self::memory`]: `bytes` accessed, `write`, the
    /// `active` lanes), so the access is bounds-checked (host root views
    /// have no bounds) and sees the copies the program's other accesses see.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn raw_view(
        &mut self,
        site: Site,
        src: &Source,
        off: V,
        bytes: u32,
        props: (u32, u32),
        write: bool,
        active: Option<V>,
    ) -> Result<(V, V), Why> {
        let Some((lo, hi)) = self.root_address(src) else {
            let h = self.handle(site, src, false, props)?;
            let off = match self.root_view_offset(src) {
                Some(at) => self.s.m.bin(self.cur, Bin::Add, off, at),
                None => off,
            };
            return Ok((h, off));
        };
        // The hardware adds the 32-bit offset to the 48-bit base.
        let (lo, hi) = self.add64(lo, hi, off, false);
        let mask = self.s.m.i32(0xFFFF);
        let hi = self.s.m.bin(self.cur, Bin::And, hi, mask);
        Ok(self.memory(lo, hi, bytes, write, active))
    }

    /// The byte offset a buffer descriptor built around a root descriptor adds
    /// (constant and computed parts); none for other descriptors.
    pub(super) fn built_offset(&mut self, src: &Source) -> Result<(u32, Option<V>), Why> {
        match src {
            Source::Built { address, .. } => {
                let x = match address.dynamic {
                    Some(v) => Some(self.i32_of(v)?),
                    None => None,
                };
                Ok((address.offset, x))
            }
            _ => Ok((0, None)),
        }
    }

    /// The byte stride of the buffer view a heap-indexed descriptor names
    /// (descriptors from the resource heap only): dword `index` of the
    /// host's view-stride table, root SRV [`VIEW_STRIDES`].
    pub(super) fn view_stride(&mut self, src: &Source) -> Result<V, Why> {
        const WHY: Why = "a structured buffer of unknown stride";
        let heap = |load: &Site| {
            self.scalar
                .get(load)
                .and_then(|a| self.x.native.layout.get(a.param))
                .is_some_and(|p| p.root == RootType::ResourceHeap)
        };
        let leaves = src.leaves();
        if leaves.is_empty()
            || !leaves.iter().all(|l| match l {
                Source::Loaded { load, .. } | Source::Fetched { load, .. } => heap(load),
                _ => false,
            })
        {
            return Err(WHY);
        }
        let index = match src {
            Source::Loaded { load, first } => self.heap_index(*load, *first, false)?,
            Source::Phi { .. } => return Err(WHY),
            _ => self.lane_heap_index(src, false)?,
        };
        let c = self.cur;
        let four = self.s.m.i32(4);
        let at = self.s.m.bin(c, Bin::Mul, index, four);
        let table = self.s.root_srv_handle(VIEW_STRIDES);
        let iu = self.s.m.undef(self.s.t.i32);
        let mask = self.s.m.i8(1);
        let r = self.s.op(
            c,
            "rawBufferLoad.f32",
            Attr::ReadOnly,
            self.s.t.res_ret,
            139,
            &[table, at, iu, mask, four],
        );
        let x = self.s.m.extract(c, r, 0);
        let word = self.as_int(x);
        let bits = self.s.m.i32(VIEW_STRIDE_BITS);
        Ok(self.s.m.bin(c, Bin::And, word, bits))
    }

    /// The used-resource entry a descriptor loaded from a table falls in.
    pub(super) fn used_kind(&self, src: &Source) -> Option<&'a UsedResource> {
        if let Source::Phi { args, .. } | Source::Select { args, .. } | Source::Lanes { args, .. } =
            src
        {
            return self.used_kind(args.first()?);
        }
        if let Source::LanePhi { args, .. } = src {
            return args.iter().flatten().find_map(|a| self.used_kind(a));
        }
        if let Source::Built { address, .. } = src {
            let rp = self.x.native.layout.get(address.param)?;
            return self
                .x
                .native
                .used
                .iter()
                .find(|u| u.root == RootType::Descriptor && u.user_data == rp.user_data);
        }
        let (Source::Loaded { load, first } | Source::Fetched { load, first }) = src else {
            return None;
        };
        let a = self.scalar.get(load)?;
        let rp = self.x.native.layout.get(a.param)?;
        if rp.root != RootType::Table {
            return None;
        }
        let dword = (a.offset / 4) + u32::from(*first);
        self.x
            .native
            .used
            .iter()
            .filter(|u| {
                u.root == RootType::Table
                    && u.user_data == rp.user_data
                    && u.table_offset_dwords <= dword
            })
            .max_by_key(|u| u.table_offset_dwords)
    }

    /// A handle for the descriptor `src` read at `site`; the heap index of
    /// a texture shader view the program takes from the heap itself is noted
    /// for the instruction's read mark (handles are made once per block,
    /// often before the instruction). A view read through a table needs
    /// none: the host makes the slot for the draw.
    pub(super) fn handle(&mut self, site: Site, src: &Source, sampler: bool, props: (u32, u32)) -> Result<V, Why> {
        let h = self.handle_made(site, src, sampler, props)?;
        if let Some(&index) = self.s.texture_index.get(&h)
            && self.is_heap_direct(src)
        {
            self.s.texture_reads.push(index);
        }
        Ok(h)
    }

    fn handle_made(
        &mut self,
        site: Site,
        src: &Source,
        sampler: bool,
        props: (u32, u32),
    ) -> Result<V, Why> {
        let props = if self.pops && props.0 & 0x1000 != 0 {
            self.s.flags |= flags::ROVS;
            (props.0 | 0x2000, props.1)
        } else {
            props
        };
        // A descriptor from the resource heap itself: the host cannot tell
        // which slots the program reads, or writes through unordered access.
        if !sampler && self.is_heap_direct(src) {
            self.heap[0] = true;
            self.heap[1] |= props.0 & 0x1000 != 0;
        }
        match src {
            Source::Loaded { load, first }
                if let Some(address) = self.entry_address(*load) =>
            {
                // The same descriptor wherever it is loaded: the console
                // compiler reloads one before every use rather than keep it
                // (ActivateRays: 2,399 loads of one), and one handle made in
                // the entry block serves them all.
                let key = (address, *first, sampler, props.0);
                if let Some(&h) = self.entry_handles.get(&key) {
                    return Ok(h);
                }
                let here = std::mem::replace(&mut self.cur, self.s.entry);
                let index = self.entry_heap_index(address, *first, sampler);
                let index = self.buffer_guard(src, index, sampler, props);
                let h = self.s.heap_handle(self.cur, index, sampler, false, props);
                self.cur = here;
                self.entry_handles.insert(key, h);
                Ok(h)
            }
            Source::Loaded { load, first } => {
                let key = (self.cur, *load, *first, sampler, props.0);
                if let Some(&h) = self.handles.get(&key) {
                    return Ok(h);
                }
                let index = self.heap_index(*load, *first, sampler)?;
                let index = self.buffer_guard(src, index, sampler, props);
                // A waterfall loop's load at the first lane's offset reads
                // each lane's own entry (see `resource::Waterfall`).
                let per_lane = self.x.res.waterfall.loads.contains(load);
                let h = self
                    .s
                    .heap_handle(self.cur, index, sampler, per_lane, props);
                self.handles.insert(key, h);
                Ok(h)
            }
            Source::Constant(words) if sampler => {
                let words: [u32; 4] = words
                    .get(..4)
                    .and_then(|w| w.try_into().ok())
                    .ok_or("a sampler constant")?;
                let comparison = props.0 & 0x8000 != 0;
                let slot = self.samplers.iter().enumerate().position(|(n, w)| {
                    *w == words && (self.s.cmp_samplers >> n & 1 != 0) == comparison
                });
                let n = match slot {
                    Some(n) => n,
                    None => {
                        let n = self.samplers.len();
                        if n >= u64::BITS as usize {
                            return Err("more than 64 immediate sampler slots");
                        }
                        self.samplers.push(words);
                        self.s.cmp_samplers |= u64::from(comparison) << n;
                        n
                    }
                };
                Ok(self.s.sampler_handle(self.cur, n as u32))
            }
            Source::Constant(_) => Err("a constant resource descriptor"),
            Source::Built { address, .. } if address.param == MEMORY => {
                Err("a buffer descriptor built around a computed address")
            }
            Source::Built { address, .. } => {
                let rp = *self
                    .x
                    .native
                    .layout
                    .get(address.param)
                    .ok_or("a descriptor built around a table in the code")?;
                let kind = self
                    .x
                    .native
                    .used
                    .iter()
                    .find(|u| u.user_data == rp.user_data)
                    .map(|u| u.kind);
                match (rp.root, kind, props.0) {
                    (RootType::Descriptor, Some(ResourceType::Cbv) | None, 13) => {
                        Ok(self.s.root_cbv_handle(rp.slot))
                    }
                    (RootType::Descriptor, Some(ResourceType::Srv), 11) if !self.root_memory => {
                        Ok(self.root_view(address.param, false))
                    }
                    (RootType::Descriptor, Some(ResourceType::Srv), 11) => {
                        Ok(self.s.root_srv_handle(rp.slot))
                    }
                    (RootType::Descriptor, Some(ResourceType::Uav) | None, 0x100B)
                    | (RootType::Descriptor, Some(ResourceType::Uav), 11)
                        if !self.root_memory =>
                    {
                        Ok(self.root_view(address.param, true))
                    }
                    (RootType::Descriptor, Some(ResourceType::Uav) | None, 0x100B)
                    | (RootType::Descriptor, Some(ResourceType::Uav), 11) => {
                        Ok(self.s.root_uav_handle(rp.slot))
                    }
                    _ => Err("a root UAV or typed root view"),
                }
            }
            Source::Phi { block, args }
                if sampler && matches!(args[..], [Source::Constant(_), Source::Constant(_)]) =>
            {
                // One of two immediate samplers, chosen by uniform control
                // flow: what reads it runs with both, the edge taken picks
                // the result (static samplers cannot be indexed).
                let [a, b] = &args[..] else {
                    unreachable!()
                };
                let ha = self.handle(site, a, sampler, props)?;
                let hb = self.handle(site, b, sampler, props)?;
                let join = self.block[*block].ok_or("a descriptor chosen by control flow")?;
                let mut incoming = Vec::new();
                for (j, &q) in self.x.p.blocks[*block].preds.iter().enumerate() {
                    if self.block[q].is_none() {
                        continue;
                    }
                    let tail = self.tail[q].ok_or("a descriptor chosen by control flow")?;
                    let at = self.edges.get(&(q, *block)).copied().unwrap_or(tail);
                    incoming.push((self.s.m.bool(j == 0), at));
                }
                let phi = self.s.m.phi(join, self.s.t.i1);
                self.s.m.set_phi(phi, incoming);
                self.s.sampler_choices.push((ha, hb, phi));
                Ok(ha)
            }
            Source::Phi { block, .. } => {
                // One handle at the join for every access the choice reaches
                // (each is in a block the join dominates).
                let _ = site;
                let key = (format!("{src:?}"), sampler, props.0);
                if let Some(&h) = self.join_handles.get(&key) {
                    return Ok(h);
                }
                let join = self.block[*block].ok_or("a descriptor chosen by control flow")?;
                let here = std::mem::replace(&mut self.cur, join);
                let index = self.uniform_heap_index(src, sampler);
                let h = index.map(|index| {
                    let index = self.buffer_guard(src, index, sampler, props);
                    self.s.heap_handle(self.cur, index, sampler, false, props)
                });
                self.cur = here;
                let h = h?;
                self.join_handles.insert(key, h);
                Ok(h)
            }
            Source::Select { scc, args } => {
                // The heap index of each choice, picked by SCC (uniform).
                let [
                    Source::Loaded { load: a, first: fa },
                    Source::Loaded { load: b, first: fb },
                ] = args[..]
                else {
                    return Err("a descriptor chosen by a condition");
                };
                let cond = match self.get(*scc)? {
                    Val::Bit(x) => x,
                    _ => return Err("SCC without a value"),
                };
                let set = self.heap_index(a, fa, sampler)?;
                let clear = self.heap_index(b, fb, sampler)?;
                let index = self.s.m.select(self.cur, cond, set, clear);
                let index = self.buffer_guard(src, index, sampler, props);
                Ok(self.s.heap_handle(self.cur, index, sampler, false, props))
            }
            Source::Fetched { .. } | Source::Lanes { .. } | Source::LanePhi { .. } => {
                // A descriptor each lane has its own of (a waterfall loop
                // reads it from the first active lane): each lane's heap
                // index, non-uniform.
                let index = self.lane_heap_index(src, sampler)?;
                let index = self.buffer_guard(src, index, sampler, props);
                Ok(self.s.heap_handle(self.cur, index, sampler, true, props))
            }
        }
    }

    /// The heap index to read a buffer view at, for a descriptor `src` from
    /// the resource heap itself (module notes, "Buffer views at computed
    /// heap indices"): `index`, or the host's null raw buffer view when the
    /// view-stride table says the slot holds no buffer view. Indices come
    /// from data; one naming a texture slot read a texture descriptor as a
    /// buffer one (a wild address: the texture's address shifted 8 bits
    /// down) and faulted the device.
    fn buffer_guard(&mut self, src: &Source, index: V, sampler: bool, props: (u32, u32)) -> V {
        // Typed, raw and structured buffers (DXIL resource kinds 10-12).
        if sampler || !(10..=12).contains(&(props.0 & 0xFF)) || !self.is_heap_direct(src) {
            return index;
        }
        let c = self.cur;
        let last = self.s.m.i32(super::HEAP_VIEWS - 1);
        let over = self.s.m.cmp(c, pred::IUGT, index, last);
        let index = self.s.m.select(c, over, last, index);
        let four = self.s.m.i32(4);
        let at = self.s.m.bin(c, Bin::Mul, index, four);
        let table = self.s.root_srv_handle(VIEW_STRIDES);
        let (iu, mask) = (self.s.m.undef(self.s.t.i32), self.s.m.i8(1));
        let r = self.s.op(c, "rawBufferLoad.f32", Attr::ReadOnly, self.s.t.res_ret, 139, &[table, at, iu, mask, four]);
        let x = self.s.m.extract(c, r, 0);
        let stride = self.as_int(x);
        let zero = self.s.m.i32(0);
        let none = self.s.m.cmp(c, pred::EQ, stride, zero);
        // The null unordered access view sits just below the null SRV.
        let null = super::NULL_BUFFER_VIEW - u32::from(props.0 & 0x1000 != 0);
        let null = self.s.m.i32(null);
        self.s.m.select(c, none, null, index)
    }

    /// Whether a descriptor comes from the resource heap itself (root type
    /// 5, not a table) on some path.
    pub(super) fn is_heap_direct(&self, src: &Source) -> bool {
        src.leaves().iter().any(|l| match l {
            Source::Loaded { load, .. } | Source::Fetched { load, .. } => self
                .scalar
                .get(load)
                .and_then(|a| self.x.native.layout.get(a.param))
                .is_some_and(|p| p.root == RootType::ResourceHeap),
            _ => false,
        })
    }

    /// The address of the descriptor load at `load` when root parameters
    /// alone give it (a table or a heap, at a fixed offset: resource
    /// tracking found no other term), and not per lane.
    fn entry_address(&self, load: Site) -> Option<EntryAddress> {
        let a = self.scalar.get(&load)?;
        let fixed = a.dynamic.is_none() && !self.x.res.waterfall.loads.contains(&load);
        let root = self.x.native.layout.get(a.param)?.root;
        let heap = matches!(root, RootType::Table | RootType::ResourceHeap | RootType::SamplerHeap);
        (fixed && heap).then_some((a.param, a.offset))
    }

    /// [`Lower::heap_index`] of a descriptor at a fixed address
    /// ([`Lower::entry_address`]), in the entry block.
    fn entry_heap_index(&mut self, (param, offset): EntryAddress, first: u8, sampler: bool) -> V {
        let entry = self.s.entry;
        let base = match self.x.native.layout[param].root {
            RootType::Table => self.user_dword(param, 0),
            _ => self.s.m.i32(0),
        };
        let k = self.s.m.i32(offset.wrapping_add(4 * u32::from(first)));
        let addr = self.s.m.bin(entry, Bin::Add, base, k);
        let shift = self.s.m.i32(if sampler { 4 } else { 5 });
        self.s.m.bin(entry, Bin::LShr, addr, shift)
    }

    /// The heap index of a descriptor every lane shares, computed in the
    /// current block. One chosen by control flow is a phi at its join of the
    /// index each predecessor computes at its end; an edge may bring another
    /// join's (nested if/else).
    fn uniform_heap_index(&mut self, src: &Source, sampler: bool) -> Result<V, Why> {
        match src {
            Source::Loaded { load, first } => self.heap_index(*load, *first, sampler),
            Source::Phi { block, args } => {
                let mut incoming = Vec::new();
                for (&q, arg) in self.x.p.blocks[*block].preds.iter().zip(args) {
                    if self.block[q].is_none() {
                        continue;
                    }
                    let tail = self.tail[q].ok_or("a descriptor chosen by control flow")?;
                    let at = self.edges.get(&(q, *block)).copied().unwrap_or(tail);
                    // No lane comes this way (`resource` gave it another
                    // edge's source): any index will do.
                    if self.never_taken(q, *block) {
                        incoming.push((self.s.m.i32(0), at));
                        continue;
                    }
                    let here = std::mem::replace(&mut self.cur, at);
                    let index = self.uniform_heap_index(arg, sampler);
                    self.cur = here;
                    incoming.push((index?, at));
                }
                let join = self.block[*block].ok_or("a descriptor chosen by control flow")?;
                let phi = self.s.m.phi(join, self.s.t.i32);
                self.s.m.set_phi(phi, incoming);
                Ok(phi)
            }
            _ => Err("a descriptor chosen by control flow"),
        }
    }

    /// The heap index of a descriptor that may differ between lanes,
    /// computed in the current block (or, chosen by control flow, at the
    /// join).
    fn lane_heap_index(&mut self, src: &Source, sampler: bool) -> Result<V, Why> {
        match src {
            Source::Loaded { load, first } => self.heap_index(*load, *first, sampler),
            Source::Fetched { load, first } => self.fetched_index(*load, *first, sampler),
            Source::Lanes { mask, args } | Source::Select { scc: mask, args } => {
                let [a, b] = &args[..] else {
                    return Err("a descriptor chosen per lane");
                };
                let cond = if matches!(src, Source::Lanes { .. }) {
                    let &m = self
                        .x
                        .st
                        .masks
                        .get(mask)
                        .ok_or("a descriptor chosen per lane by an untracked mask")?;
                    self.boolean(m, self.cur)?
                } else {
                    match self.get(*mask)? {
                        Val::Bit(x) => x,
                        _ => return Err("SCC without a value"),
                    }
                };
                let set = self.lane_heap_index(a, sampler)?;
                let clear = self.lane_heap_index(b, sampler)?;
                Ok(self.s.m.select(self.cur, cond, set, clear))
            }
            Source::LanePhi { block, args } => {
                // Each predecessor computes its heap index at its end (what
                // it reads is there: the dwords are, on that edge); the join
                // picks the one of the edge the lane came by. An edge no
                // lane reading it takes gives index 0.
                let join =
                    self.block[*block].ok_or("a per-lane descriptor chosen by control flow")?;
                let mut incoming = Vec::new();
                for (&q, arg) in self.x.p.blocks[*block].preds.iter().zip(args) {
                    if self.block[q].is_none() {
                        continue;
                    }
                    let tail =
                        self.tail[q].ok_or("a per-lane descriptor joined from a later block")?;
                    let at = self.edges.get(&(q, *block)).copied().unwrap_or(tail);
                    let index = match arg {
                        Some(arg) => {
                            let here = std::mem::replace(&mut self.cur, at);
                            let index = self.lane_heap_index(arg, sampler);
                            self.cur = here;
                            index?
                        }
                        None => self.s.m.i32(0),
                    };
                    incoming.push((index, at));
                }
                let phi = self.s.m.phi(join, self.s.t.i32);
                self.s.m.set_phi(phi, incoming);
                Ok(phi)
            }
            _ => Err("a per-lane descriptor from neither a heap load nor a choice of them"),
        }
    }

    /// The heap index of the descriptor at dword `first` of what the global
    /// load at `load` reads, in each lane: its address (base plus offsets)
    /// over the descriptor size.
    fn fetched_index(&mut self, load: Site, first: u8, sampler: bool) -> Result<V, Why> {
        const WHY: Why = "a per-lane descriptor load";
        let a = *self
            .scalar
            .get(&load)
            .ok_or("an untracked descriptor load")?;
        match self.x.native.layout.get(a.param).map(|p| p.root) {
            Some(RootType::Table | RootType::ResourceHeap | RootType::SamplerHeap) => {}
            _ => return Err("a descriptor outside the heaps"),
        }
        let li: &Inst = &self.x.p.blocks[load.0].insts[load.1];
        let Fields::Flat {
            addr: Some(Operand::Vector { reg, .. }),
            saddr,
            offset,
            ..
        } = li.fields
        else {
            return Err(WHY);
        };
        let c = self.cur;
        // The low dword of the address: SGPR base plus VGPR offset, or the
        // VGPR pair's low half (heap addresses fit in 32 bits).
        let mut at = self.i32_of(li.read(Reg::V(reg)).ok_or(WHY)?)?;
        if let Some(Operand::Scalar { code, .. }) = saddr
            && code != NULL
        {
            let base = self.i32_of(li.read(Reg::S(code)).ok_or(WHY)?)?;
            at = self.s.m.bin(c, Bin::Add, base, at);
        }
        let k = self
            .s
            .m
            .i32((offset as i32 as u32).wrapping_add(4 * u32::from(first)));
        at = self.s.m.bin(c, Bin::Add, at, k);
        let shift = self.s.m.i32(if sampler { 4 } else { 5 });
        Ok(self.s.m.bin(c, Bin::LShr, at, shift))
    }

    /// The heap index of the descriptor at dword `first` of what the scalar
    /// load at `load` reads, computed in the current block.
    pub(super) fn heap_index(&mut self, load: Site, first: u8, sampler: bool) -> Result<V, Why> {
        let a = *self
            .scalar
            .get(&load)
            .ok_or("an untracked descriptor load")?;
        match self.x.native.layout.get(a.param).map(|p| p.root) {
            Some(RootType::Table | RootType::ResourceHeap | RootType::SamplerHeap) => {}
            _ => return Err("a descriptor outside the heaps"),
        }
        let li: &Inst = &self.x.p.blocks[load.0].insts[load.1];
        let Fields::Smem {
            base,
            offset,
            soffset,
            ..
        } = &li.fields
        else {
            return Err("a descriptor load");
        };
        let Operand::Scalar { code, .. } = *base else {
            return Err("a descriptor load");
        };
        let c = self.cur;
        let base = li.read(Reg::S(code)).ok_or("a descriptor load")?;
        let mut addr = self.i32_of(base)?;
        let k = self
            .s
            .m
            .i32((*offset as u32).wrapping_add(4 * u32::from(first)));
        addr = self.s.m.bin(c, Bin::Add, addr, k);
        if !matches!(soffset, Operand::Scalar { code: NULL, .. }) {
            let so = self.operand(li, soffset)?;
            addr = self.s.m.bin(c, Bin::Add, addr, so);
        }
        let shift = self.s.m.i32(if sampler { 4 } else { 5 });
        Ok(self.s.m.bin(c, Bin::LShr, addr, shift))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{S_ENDPGM, lower_wave, lower_with, root};
    use crate::native::{ResourceType, RootParam, RootType, UsedResource};

    #[test]
    fn a_descriptor_selected_by_scc_picks_its_heap_index() {
        // Two buffer descriptors loaded from the table at s0, one picked by
        // a root constant (s1) with s_cselect_b64 per half, and loaded
        // through by index and stored to LDS so the load is needed.
        let words = [
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], null
            0xF408_0302,
            0xFA00_0010, // s_load_dwordx4 s[12:15], s[4:5], 0x10
            0xBF06_8001, // s_cmp_eq_u32 s1, 0
            0x8590_0C08, // s_cselect_b64 s[16:17], s[8:9], s[12:13]
            0x8592_0E0A, // s_cselect_b64 s[18:19], s[10:11], s[14:15]
            0xE030_2000,
            0x8004_0100, // buffer_load_dword v1, v0, s[16:19], 0 idxen
            0xD834_0000,
            0x0000_0100, // ds_write_b32 v0, v1
            S_ENDPGM,
        ];
        let layout = || vec![root(RootType::Table, 0, 0), root(RootType::Constants, 1, 1)];
        // Both are structured buffers of dwords in the table's range.
        let used = || {
            vec![UsedResource {
                kind: ResourceType::Srv,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 4,
                dwords: 4,
                user_data: 0,
                slot: 0,
            }]
        };
        lower_with(&words, layout(), used()).unwrap();
    }

    /// Tables of 2D textures at s0 and samplers at s1.
    fn texture_tables() -> (Vec<RootParam>, Vec<UsedResource>) {
        let range = |kind, user_data, slot| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: if kind == ResourceType::Sampler { 4 } else { 8 },
            user_data,
            slot,
        };
        (
            vec![root(RootType::Table, 0, 0), root(RootType::Table, 1, 1)],
            vec![
                range(ResourceType::Srv, 0, 0),
                range(ResourceType::Sampler, 1, 1),
            ],
        )
    }

    #[test]
    fn a_descriptor_loaded_only_for_its_size_takes_the_kind_of_its_reload() {
        // The record count of the buffer at s0 (count - 1 as an index), then
        // the same descriptor loaded again for the indexed read.
        let words = [
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], null
            0x810C_C10A, // s_add_i32 s12, s10, -1
            0xF408_0402,
            0xFA00_0000, // s_load_dwordx4 s[16:19], s[4:5], null
            0x7E04_020C, // v_mov_b32 v2, s12
            0xE030_2000,
            0x8004_0102, // buffer_load_dword v1, v2, s[16:19], 0 idxen
            0xD834_0000,
            0x0000_0100, // ds_write_b32 v0, v1
            S_ENDPGM,
        ];
        let layout = vec![root(RootType::Table, 0, 0), root(RootType::Constants, 1, 1)];
        let used = vec![UsedResource {
            kind: ResourceType::Srv,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 4,
            dwords: 4,
            user_data: 0,
            slot: 0,
        }];
        lower_with(&words, layout, used).unwrap();
    }

    #[test]
    fn a_waterfall_loop_that_only_sizes_takes_the_kind_of_the_sampled_one() {
        // Each lane names table entry (lane & 1) by its byte offset in v7.
        // A first waterfall loop only reads the width bits of the entry
        // (`v_alignbit_b32`), a second one samples it; the width goes to the
        // buffer at dword 16 of the table.
        let words = [
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xBE86_0301, // s_mov_b32 s6, s1
            0xBE87_0380, // s_mov_b32 s7, 0
            0xF408_0203,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
            0xF408_0A02,
            0xFA00_0040, // s_load_dwordx4 s[40:43], s[4:5], 0x40
            0x7E04_0D00, // v_cvt_f32_u32 v2, v0
            0x7E06_0302, // v_mov_b32 v3, v2
            0x3420_0082, // v_lshlrev_b32 v16, 2, v0
            0x360E_0081, // v_and_b32 v7, 1, v0
            0x340E_0E85, // v_lshlrev_b32 v7, 5, v7
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0xBE97_037E, // s_mov_b32 s23, exec_lo
            0xBE95_037E, // s_mov_b32 s21, exec_lo
            0x7E28_0507, // L1: v_readfirstlane_b32 s20, v7
            0x7DA4_0E14, // v_cmpx_eq_u32 exec_lo, s20, v7
            0xF40C_0302,
            0x2800_0000, // s_load_dwordx8 s[12:19], s[4:5], s20
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0xD54E_0008,
            0x0278_1A0E, // v_alignbit_b32 v8, s14, s13, 30
            0xBE95_4715, // s_andn2_wrexec_b32 s21, s21
            0xBF89_FFF7, // s_cbranch_execnz L1
            0xBEFE_0317, // s_mov_b32 exec_lo, s23
            0xBE95_037E, // s_mov_b32 s21, exec_lo
            0x7E28_0507, // L2: v_readfirstlane_b32 s20, v7
            0x7DA4_0E14, // v_cmpx_eq_u32 exec_lo, s20, v7
            0xF40C_0302,
            0x2800_0000, // s_load_dwordx8 s[12:19], s[4:5], s20
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0xF09C_0F08,
            0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:2D
            0xBF8C_3F70, // s_waitcnt vmcnt(0)
            0xBE95_4715, // s_andn2_wrexec_b32 s21, s21
            0xBF89_FFF6, // s_cbranch_execnz L2
            0xBEFE_0317, // s_mov_b32 exec_lo, s23
            0xE070_1000,
            0x800A_0810, // buffer_store_dword v8, v16, s[40:43], 0 offen
            S_ENDPGM,
        ];
        let (layout, used) = texture_and_uav_tables();
        lower_wave(&words, layout, used, false).unwrap();
    }

    #[test]
    fn compute_spills_beside_a_descriptor_word_read_back() {
        // Dword 0 of a buffer descriptor (no host value) and a root constant
        // spilled to lanes 0 and 1 of v5 in a compute program; reading lane
        // 1 back is the constant (the descriptor's lane is never read).
        let words = [
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], null
            0xD761_0005,
            0x0001_0008, // v_writelane_b32 v5, s8, 0
            0xD761_0005,
            0x0001_0201, // v_writelane_b32 v5, s1, 1
            0xD760_000C,
            0x0001_0305, // v_readlane_b32 s12, v5, 1
            0xE030_2000,
            0x8002_0100, // buffer_load_dword v1, v0, s[8:11], 0 idxen
            0x7E06_020C, // v_mov_b32 v3, s12
            0xD834_0000,
            0x0000_0300, // ds_write_b32 v0, v3
            0xD834_0000,
            0x0000_0100, // ds_write_b32 v0, v1
            S_ENDPGM,
        ];
        let layout = vec![root(RootType::Table, 0, 0), root(RootType::Constants, 1, 1)];
        let used = vec![UsedResource {
            kind: ResourceType::Srv,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 4,
            dwords: 4,
            user_data: 0,
            slot: 0,
        }];
        lower_with(&words, layout, used).unwrap();
    }

    #[test]
    fn waterfall_loops_over_per_lane_descriptors_sample_with_each_lanes_own() {
        use crate::resource::tests::{
            WATERFALL_FETCHED_WAVE32, WATERFALL_FETCHED_WAVE64, WATERFALL_PICKED_WAVE32,
        };
        for (words, wave64) in [
            (WATERFALL_FETCHED_WAVE32, false),
            (WATERFALL_FETCHED_WAVE64, true),
            (WATERFALL_PICKED_WAVE32, false),
        ] {
            let (layout, used) = texture_tables();
            let l = lower_wave(words, layout, used, wave64).unwrap();
            // The loop's reads of the first lane became each lane's own
            // dwords: no wave operation is left.
            assert_eq!(l.nonuniform_handles, 1);
            assert!(!l.wave_ops);
        }
    }

    /// [`texture_tables`] with a raw buffer UAV at dword 16 of the first.
    pub(crate) fn texture_and_uav_tables() -> (Vec<RootParam>, Vec<UsedResource>) {
        let (layout, mut used) = texture_tables();
        used.push(UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 16,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 0,
            slot: 0,
        });
        (layout, used)
    }

    #[test]
    fn waterfall_loops_over_descriptors_joined_per_lane_sample_with_each_lanes_own() {
        use crate::resource::tests::{WATERFALL_JOINED_MIXED_WAVE32, WATERFALL_JOINED_WAVE32};
        for words in [WATERFALL_JOINED_WAVE32, WATERFALL_JOINED_MIXED_WAVE32] {
            let (layout, used) = texture_and_uav_tables();
            let l = lower_wave(words, layout, used, false).unwrap();
            // One image handle, from the join's per-lane heap index.
            assert_eq!(l.nonuniform_handles, 1);
            assert!(!l.wave_ops);
        }
    }

    #[test]
    fn an_address_half_set_where_only_lanes_that_never_run_skip_is_known() {
        // As `indirect_light_probes:CS_Alpha_Schedule` after its texture: a
        // table address whose high half (s25 = 0) is set inside the region
        // that only a skip no lane takes bypasses; the skip edge would bring
        // the stale input s25.
        let words = [
            0xBE9A_037E, // s_mov_b32 s26, exec_lo
            0xBEFE_097E, // s_wqm_b32 exec_lo, exec_lo
            0xBE98_0300, // s_mov_b32 s24, s0
            0xBE86_3C1A, // s_and_saveexec_b32 s6, s26
            0xBF88_0001, // s_cbranch_execz -> join
            0xBE99_0380, // s_mov_b32 s25, 0
            0xBEFE_0306, // join: s_mov_b32 exec_lo, s6
            0xF408_0A0C,
            0xFA00_0040, // s_load_dwordx4 s[40:43], s[24:25], 0x40
            0x3420_0082, // v_lshlrev_b32 v16, 2, v0
            0x7E06_0280, // v_mov_b32 v3, 0
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0xE070_1000,
            0x800A_0310, // buffer_store_dword v3, v16, s[40:43], 0 offen
            S_ENDPGM,
        ];
        let (_, mut used) = texture_and_uav_tables();
        used.retain(|u| u.user_data == 0);
        let layout = vec![root(RootType::Table, 0, 0)];
        lower_with(&words, layout, used).expect("the high half is 0 wherever a lane runs");
    }

    #[test]
    fn a_descriptor_skipped_only_by_lanes_that_never_run_needs_no_source() {
        // As `indirect_light_probes:CS_Alpha_Schedule`: the texture is
        // loaded under the lanes that ran before `s_wqm`, skipped when there
        // are none, and sampled with the helper lanes after the join. With
        // every lane running from the start nothing takes the skip, so the
        // registers it would bring (no descriptor) are never read.
        let words = [
            0xBE9A_037E, // s_mov_b32 s26, exec_lo
            0xBEFE_097E, // s_wqm_b32 exec_lo, exec_lo
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0A02,
            0xFA00_0040, // s_load_dwordx4 s[40:43], s[4:5], 0x40
            0xBE86_3C1A, // s_and_saveexec_b32 s6, s26
            0xBF88_0002, // s_cbranch_execz -> join
            0xF40C_0302,
            0xFA00_0000, // s_load_dwordx8 s[12:19], s[4:5], 0x0
            0xBEFE_0306, // join: s_mov_b32 exec_lo, s6
            0xBE88_03FF,
            0x0000_0092, // s_mov_b32 s8, 0x92 (an immediate sampler)
            0xBE89_0380, // s_mov_b32 s9, 0
            0xBE8A_0380, // s_mov_b32 s10, 0
            0xBE8B_0380, // s_mov_b32 s11, 0
            0x7E04_02F0, // v_mov_b32 v2, 0.5
            0x7E06_02F0, // v_mov_b32 v3, 0.5
            0x3420_0082, // v_lshlrev_b32 v16, 2, v0
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0xF09C_0F08,
            0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
            0xBF8C_3F70, // s_waitcnt vmcnt(0)
            0xE070_1000,
            0x800A_0C10, // buffer_store_dword v12, v16, s[40:43], 0 offen
            S_ENDPGM,
        ];
        let (_, mut used) = texture_and_uav_tables();
        used.retain(|u| u.user_data == 0);
        let layout = vec![root(RootType::Table, 0, 0)];
        let l = lower_with(&words, layout, used).unwrap();
        assert_eq!(l.nonuniform_handles, 0);
        #[cfg(windows)]
        if let Ok(mut v) = super::super::validator::Validator::load(
            super::super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut l.dxil.clone())
                .expect("DXIL validation failed");
        }
    }

    #[test]
    fn descriptors_joined_by_nested_control_flow_lower_to_one_uniform_handle() {
        // The view table at s0 (2D textures at entries 0 and 1, a raw UAV at
        // dword 16) and a root constant at s1: entry 0 if s1 is 0, else
        // entry 1 if it is 1, else entry 0 again, the inner if/else joined
        // before the outer one. The join's edges are a load and a join.
        let words = [
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0A02,
            0xFA00_0040, // s_load_dwordx4 s[40:43], s[4:5], 0x40
            0xBE88_03FF,
            0x0000_0092, // s_mov_b32 s8, 0x92 (an immediate sampler)
            0xBE89_0380, // s_mov_b32 s9, 0
            0xBE8A_0380, // s_mov_b32 s10, 0
            0xBE8B_0380, // s_mov_b32 s11, 0
            0xBF06_8001, // s_cmp_eq_u32 s1, 0
            0xBF84_0003, // s_cbranch_scc0 -> 0x38
            0xF40C_0302,
            0xFA00_0000, // s_load_dwordx8 s[12:19], s[4:5], 0x0
            0xBF82_0008, // s_branch -> 0x58
            0xBF06_8101, // 0x38: s_cmp_eq_u32 s1, 1
            0xBF84_0003, // s_cbranch_scc0 -> 0x4c
            0xF40C_0302,
            0xFA00_0020, // s_load_dwordx8 s[12:19], s[4:5], 0x20
            0xBF82_0002, // s_branch -> 0x54
            0xF40C_0302,
            0xFA00_0000, // 0x4c: s_load_dwordx8 s[12:19], s[4:5], 0x0
            0xBE86_0301, // 0x54: s_mov_b32 s6, s1
            0x7E04_02F0, // 0x58: v_mov_b32 v2, 0.5
            0x7E06_02F0, // v_mov_b32 v3, 0.5
            0x3420_0082, // v_lshlrev_b32 v16, 2, v0
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0xF09C_0F08,
            0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
            0xE070_1000,
            0x800A_0C10, // buffer_store_dword v12, v16, s[40:43], 0 offen
            S_ENDPGM,
        ];
        let (_, mut used) = texture_and_uav_tables();
        used.retain(|u| u.user_data == 0);
        let layout = vec![root(RootType::Table, 0, 0), root(RootType::Constants, 1, 1)];
        let l = lower_with(&words, layout, used).unwrap();
        // One image handle from the joins' heap index, the same in all lanes.
        assert_eq!(l.nonuniform_handles, 0);
        #[cfg(windows)]
        if let Ok(mut v) = super::super::validator::Validator::load(
            super::super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut l.dxil.clone())
                .expect("DXIL validation failed");
        }
    }

    #[test]
    fn descriptors_reloaded_from_a_root_table_share_one_handle() {
        // The view table at s0 (2D textures at entries 0 and 1, a raw UAV at
        // dword 16) and a root constant at s1. Entry 0 is loaded and sampled
        // in a branch, then loaded again and sampled after it (the console
        // compiler reloads a descriptor before each use rather than keep it).
        let words = [
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0A02,
            0xFA00_0040, // s_load_dwordx4 s[40:43], s[4:5], 0x40
            0xBE88_03FF,
            0x0000_0092, // s_mov_b32 s8, 0x92 (an immediate sampler)
            0xBE89_0380, // s_mov_b32 s9, 0
            0xBE8A_0380, // s_mov_b32 s10, 0
            0xBE8B_0380, // s_mov_b32 s11, 0
            0x7E04_02F0, // v_mov_b32 v2, 0.5
            0x7E06_02F0, // v_mov_b32 v3, 0.5
            0x3420_0082, // v_lshlrev_b32 v16, 2, v0
            0xBF06_8001, // s_cmp_eq_u32 s1, 0
            0xBF84_0005, // s_cbranch_scc0 -> the reload
            0xF40C_0302,
            0xFA00_0000, // s_load_dwordx8 s[12:19], s[4:5], 0x0
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0xF09C_0F08,
            0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
            0xF40C_0302,
            0xFA00_0000, // s_load_dwordx8 s[12:19], s[4:5], 0x0 (again)
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0xF09C_0F08,
            0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11]
            0xE070_1000,
            0x800A_0C10, // buffer_store_dword v12, v16, s[40:43], 0 offen
            S_ENDPGM,
        ];
        let (_, mut used) = texture_and_uav_tables();
        used.retain(|u| u.user_data == 0);
        let layout = vec![root(RootType::Table, 0, 0), root(RootType::Constants, 1, 1)];
        let l = lower_with(&words, layout, used).unwrap();
        // One image handle made in the entry block for both loads, and the
        // UAV's.
        assert_eq!(l.heap_handles, 2);
        #[cfg(windows)]
        if let Ok(mut v) = super::super::validator::Validator::load(
            super::super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut l.dxil.clone())
                .expect("DXIL validation failed");
        }
    }
}
