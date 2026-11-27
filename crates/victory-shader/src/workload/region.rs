//! What a loop region does beyond its registers: the memory it touches,
//! and which of its instructions' register reads are data.

use crate::gfx10::{Fields, Operand};
use crate::ir::{EXEC, Inst, Program, Reg, Value};
use crate::resource::{MEMORY, Resources, Site};

/// Which memory an instruction touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Memory {
    /// The thread group's shared memory (`ds_*`): where a walk keeps its
    /// stack.
    Shared,
    /// Through a descriptor or an address traced to a root parameter.
    Tracked,
    /// At an address the program computed from data (title memory).
    Raw,
    /// The node instruction: nodes through a descriptor built around a
    /// root parameter's address (`tracked`), or at a raw address.
    Node { tracked: bool },
}

/// One instruction's access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub site: Site,
    pub memory: Memory,
    /// It writes (a store, an atomic); else it only reads.
    pub store: bool,
}

/// The memory accesses of `blocks` of `p`, in block and instruction order.
/// `res` is the program's resources: without them an address is not known
/// to be raw or tracked, and nothing is said.
pub fn touches(p: &Program, res: &Resources, blocks: &[usize]) -> Vec<Access> {
    let mut out = Vec::new();
    for &b in blocks {
        for (i, inst) in p.blocks[b].insts.iter().enumerate() {
            let (site, name) = ((b, i), inst.name());
            let raw = || {
                res.scalar
                    .iter()
                    .any(|s| s.0 == site && s.1.param == MEMORY)
            };
            let by_address = |raw: bool| if raw { Memory::Raw } else { Memory::Tracked };
            let memory = match &inst.fields {
                Fields::Ds { gds: false, .. }
                    if !matches!(name, "ds_swizzle_b32" | "ds_bpermute_b32") =>
                {
                    Memory::Shared
                }
                Fields::Image { .. } if name.starts_with("image_bvh") => Memory::Node {
                    tracked: res.descriptors.iter().any(|d| d.site == site),
                },
                Fields::Image { .. } => Memory::Tracked,
                Fields::Buffer { .. } if !name.ends_with("_inv") => Memory::Tracked,
                Fields::Smem { .. } if name.starts_with("s_buffer_") => Memory::Tracked,
                Fields::Smem { .. } if name.contains("_load_") || name.contains("_store_") => {
                    by_address(raw())
                }
                Fields::Flat { .. } if name.starts_with("global_") => by_address(raw()),
                _ => continue,
            };
            // Whatever `ds_*` does not only read (a write, an atomic) writes.
            let store = name.contains("_store")
                || name.contains("_atomic")
                || memory == Memory::Shared && !name.contains("_read");
            out.push(Access {
                site,
                memory,
                store,
            });
        }
    }
    out
}

/// The registers `inst` names as sources, as operands.
fn sources(inst: &Inst) -> Vec<Operand> {
    let name = inst.name();
    let writes = name.contains("_store") || name.contains("_atomic") || name.contains("_write");
    match &inst.fields {
        Fields::Sop { src, .. } => src.clone(),
        Fields::Valu(v) => v.src.clone(),
        Fields::Vintrp { src, .. } => vec![*src],
        Fields::Ds { addr, data, .. } => addr.iter().chain(data).copied().collect(),
        Fields::Buffer {
            data,
            addr,
            rsrc,
            soffset,
            ..
        } => {
            let data = data.iter().filter(|_| writes);
            [*addr, *rsrc, *soffset]
                .into_iter()
                .chain(data.copied())
                .collect()
        }
        Fields::Image {
            data,
            addr,
            rsrc,
            sampler,
            ..
        } => {
            let rest = [*rsrc]
                .into_iter()
                .chain(*sampler)
                .chain(writes.then_some(*data));
            addr.iter().copied().chain(rest).collect()
        }
        Fields::Flat {
            addr, data, saddr, ..
        } => [addr, data, saddr].into_iter().flatten().copied().collect(),
        Fields::Smem {
            data,
            base,
            soffset,
            ..
        } => {
            let data = data.iter().filter(|_| writes);
            [*base, *soffset].into_iter().chain(data.copied()).collect()
        }
        Fields::Exp { src, .. } => src.iter().flatten().copied().collect(),
        Fields::Sopp { .. } => Vec::new(),
    }
}

/// The values `inst` reads as data, where `guarded` says whether the
/// per-lane form runs it under a condition of its own. The IR lists every
/// register an instruction reads as the machine does; two of those reads
/// are not data in one lane's run: EXEC, which every vector instruction
/// reads (which lanes run is the per-lane form's control flow), and the
/// old value of a VGPR the instruction writes, which only a lane the
/// instruction does not run in keeps (none, unless it is guarded). A
/// register the instruction names as a source is always data.
pub fn data_reads(inst: &Inst, guarded: bool) -> Vec<(Reg, Value)> {
    let vector = !matches!(
        inst.fields,
        Fields::Sop { .. } | Fields::Sopp { .. } | Fields::Smem { .. }
    );
    // A move relative to M0 reads whichever register M0 picks.
    if !vector || inst.name().contains("rel") {
        return inst.reads.clone();
    }
    let sources = sources(inst);
    let named = |reg: Reg| {
        sources.iter().any(|s| match (*s, reg) {
            (Operand::Scalar { code, dwords }, Reg::S(r)) => {
                (code..code.saturating_add(dwords)).contains(&r)
            }
            (Operand::Vector { reg, dwords }, Reg::V(r)) => {
                (reg..reg.saturating_add(dwords)).contains(&r)
            }
            _ => false,
        })
    };
    let kept = |&(reg, _): &(Reg, Value)| {
        named(reg)
            || match reg {
                Reg::S(c) if c == EXEC || c == EXEC + 1 => false,
                Reg::V(_) => guarded || inst.write(reg).is_none(),
                _ => true,
            }
    };
    inst.reads.iter().copied().filter(kept).collect()
}

/// Whether `inst` reads, as data, a register it does not name as a source
/// (M0, a carry in SCC): its result is then not a function of its operands
/// alone.
pub fn reads_beyond_its_operands(inst: &Inst) -> bool {
    let sources = sources(inst);
    let named = |reg: Reg| {
        sources.iter().any(|s| match (*s, reg) {
            (Operand::Scalar { code, dwords }, Reg::S(r)) => {
                (code..code.saturating_add(dwords)).contains(&r)
            }
            (Operand::Vector { reg, dwords }, Reg::V(r)) => {
                (reg..reg.saturating_add(dwords)).contains(&r)
            }
            (Operand::Const(251), Reg::S(r)) => r == crate::ir::VCC || r == crate::ir::VCC + 1,
            (Operand::Const(252), Reg::S(r)) => r == EXEC || r == EXEC + 1,
            (Operand::Const(253), Reg::Scc) => true,
            (Operand::Const(254), Reg::S(r)) => r == crate::ir::M0,
            _ => false,
        })
    };
    data_reads(inst, false).iter().any(|&(reg, _)| !named(reg))
}

/// The operands `inst` forms a memory address of (a descriptor is not one).
pub fn addresses(inst: &Inst) -> Vec<Operand> {
    match &inst.fields {
        Fields::Ds { addr, .. } => addr.iter().copied().collect(),
        Fields::Buffer { addr, soffset, .. } => vec![*addr, *soffset],
        Fields::Image { addr, .. } => addr.clone(),
        Fields::Flat { addr, saddr, .. } => [addr, saddr].into_iter().flatten().copied().collect(),
        Fields::Smem { base, soffset, .. } => vec![*base, *soffset],
        _ => Vec::new(),
    }
}
