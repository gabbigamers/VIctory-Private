//! Thread limits (`emit`, "Thread limits"): which threads of a compute
//! program lowered with them run, one lane at a time on this host, and what
//! the host is told to fill.

use super::tests::root;
use super::{Lowered, Parts, lower, lower_with};
use crate::emit::bitcode::FlowTerm;
use crate::emit::{THREAD_LIMIT_NONE, USER_THREAD_LIMIT, UserData};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;
const S_BARRIER: u32 = 0xBF8A_0000;

/// A compute program with thread groups of `threads` that stores one dword
/// through a raw buffer view from a descriptor table (root parameter 1,
/// user-data dword 4), after four root constants (dwords 0-3); `before`
/// runs ahead of the store. Lowered with thread limits if `limits`.
fn program(threads: [u32; 3], before: &[u32], limits: bool) -> Lowered {
    let mut words = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0xF408_0202, // s_load_dwordx4 s[8:11], s[4:5], 0
        0xFA00_0000,
    ];
    words.extend_from_slice(before);
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
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 256,
        layout: vec![constants, root(RootType::Table, 4, 1)],
        used: vec![buffer],
        io: StageIo::Cs(ComputeIo {
            // Five user SGPRs, no LDS.
            rsrc2: 5 << 1,
            threads,
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let p = ir::build(&native.code, false, 256).unwrap();
    let st = structure::structure(&p).unwrap();
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
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
    let lowered = if limits { lower_with(&parts, true) } else { lower(&parts) }.unwrap();
    lowered.flow.check().unwrap();
    // Where `dxil.dll` is there, the module is checked whole too.
    #[cfg(windows)]
    if let Ok(mut v) = crate::emit::validator::Validator::load(
        crate::emit::validator::Validator::default_path().as_deref(),
    ) {
        v.validate(&mut lowered.dxil.clone()).expect("DXIL validation failed");
    }
    lowered
}

/// The limit dwords a lowered program asks the host for.
fn limits_of(l: &Lowered) -> Vec<UserData> {
    let limit = |u: &&UserData| u.root == RootType::Other(USER_THREAD_LIMIT);
    l.user_data.iter().filter(limit).copied().collect()
}

/// Whether thread `thread` of group `group` stores, with the host's limit
/// dwords set to `limits` (by dimension).
fn stores(l: &Lowered, group: [u32; 3], thread: [u32; 3], limits: [u32; 3]) -> bool {
    let mut user = [0u32; 16];
    for u in limits_of(l) {
        user[u.index as usize] = limits[usize::from(u.slot)];
    }
    let mut stored = false;
    let mut call = |name: &str, args: &[u64]| match name {
        "dx.op.cbufferLoadLegacy.i32" => {
            let row = args[2] as usize * 4;
            user[row..row + 4].iter().map(|&v| u64::from(v)).collect()
        }
        "dx.op.groupId.i32" => vec![u64::from(group[args[1] as usize])],
        "dx.op.threadIdInGroup.i32" => vec![u64::from(thread[args[1] as usize])],
        "dx.op.rawBufferStore.i32" => {
            assert!(!stored, "one store");
            stored = true;
            Vec::new()
        }
        _ => vec![0],
    };
    l.flow.run(1 << 16, &mut call).unwrap();
    stored
}

#[test]
fn a_limit_dword_per_dimension_wider_than_one_thread() {
    let entry = |index, slot, dword| UserData {
        index,
        slot,
        root: RootType::Other(USER_THREAD_LIMIT),
        dword,
    };
    // After the root parameters' five dwords, in the order of the dimensions.
    assert_eq!(limits_of(&program([64, 1, 1], &[], true)), [entry(5, 0, 64)]);
    assert_eq!(limits_of(&program([8, 8, 1], &[], true)), [entry(5, 0, 8), entry(6, 1, 8)]);
    assert_eq!(limits_of(&program([1, 4, 2], &[], true)), [entry(5, 1, 4), entry(6, 2, 2)]);
    assert_eq!(
        limits_of(&program([4, 4, 4], &[], true)),
        [entry(5, 0, 4), entry(6, 1, 4), entry(7, 2, 4)]
    );
}

/// Without the option nothing changes, and with it a program that needs no
/// limit (one thread a group) or cannot take one (a group barrier) is the
/// same module as without.
#[test]
fn programs_without_limits_are_translated_as_before() {
    for (threads, before) in [([1, 1, 1], &[][..]), ([64, 1, 1], &[S_BARRIER][..])] {
        let (plain, asked) = (program(threads, before, false), program(threads, before, true));
        assert!(limits_of(&asked).is_empty(), "{threads:?}");
        assert_eq!(plain.dxil, asked.dxil, "{threads:?}");
        assert_eq!(plain.user_data, asked.user_data, "{threads:?}");
    }
    let plain = program([64, 1, 1], &[], false);
    assert!(limits_of(&plain).is_empty());
    assert_ne!(plain.dxil, program([64, 1, 1], &[], true).dxil);
}

/// The test is the first thing the function does: its entry block ends in
/// the two-way branch, one way a block that only returns, and the store is
/// only reached the other way.
#[test]
fn the_threads_left_out_return_before_anything_else() {
    let l = program([64, 1, 1], &[], true);
    let f = &l.flow;
    let FlowTerm::CondBr(_, body, out) = f.blocks[0].term else {
        panic!("the entry ends in {:?}", f.blocks[0].term);
    };
    assert_eq!(f.blocks[out].term, FlowTerm::Ret);
    assert!(f.blocks[out].insts.is_empty());
    assert!(!f.calls(0).iter().any(|c| c.contains("Store")), "{:?}", f.calls(0));
    let store = (0..f.blocks.len())
        .find(|&b| f.calls(b).contains(&"dx.op.rawBufferStore.i32"))
        .expect("a store");
    assert!(f.reaches(body, store, &[]));
    assert!(!f.reaches(0, store, &[body]));
}

/// One dimension: of the groups a dispatch of `limit` threads needs, exactly
/// the threads numbered below `limit` store, none more and none fewer.
#[test]
fn exactly_the_threads_asked_for_run_along_x() {
    const SIZE: u32 = 4;
    let l = program([SIZE, 1, 1], &[], true);
    for limit in 0..=3 * SIZE {
        let mut ran = Vec::new();
        for group in 0..limit.div_ceil(SIZE) {
            for thread in 0..SIZE {
                if stores(&l, [group, 0, 0], [thread, 0, 0], [limit, 0, 0]) {
                    ran.push(group * SIZE + thread);
                }
            }
        }
        assert_eq!(ran, (0..limit).collect::<Vec<_>>(), "limit {limit}");
    }
    // No limit: every thread of every group, as a dispatch counted in groups.
    for (group, thread) in [(0, 0), (0, 3), (7, 2), (65_534, 3)] {
        assert!(stores(&l, [group, 0, 0], [thread, 0, 0], [THREAD_LIMIT_NONE; 3]));
    }
    // The last group a host dispatch has, cut short after three threads.
    let last = 65_534;
    for thread in 0..SIZE {
        assert_eq!(stores(&l, [last, 0, 0], [thread, 0, 0], [last * SIZE + 3, 0, 0]), thread < 3);
    }
}

/// Three dimensions, each with its own limit: a thread runs when it is
/// inside all three.
#[test]
fn each_dimension_has_its_own_limit() {
    let size = [4u32, 2, 3];
    let l = program(size, &[], true);
    for limits in [[4u32, 2, 3], [5, 3, 4], [1, 1, 1], [7, 2, 5], [0, 4, 6], [8, 4, 6]] {
        let groups: [u32; 3] = std::array::from_fn(|c| limits[c].div_ceil(size[c]));
        let mut ran = 0u32;
        for gz in 0..groups[2] {
            for gy in 0..groups[1] {
                for gx in 0..groups[0] {
                    for t in 0..size.iter().product::<u32>() {
                        let thread = [t % size[0], t / size[0] % size[1], t / (size[0] * size[1])];
                        let group = [gx, gy, gz];
                        let inside = (0..3).all(|c| group[c] * size[c] + thread[c] < limits[c]);
                        assert_eq!(stores(&l, group, thread, limits), inside, "{limits:?} {group:?} {thread:?}");
                        ran += u32::from(inside);
                    }
                }
            }
        }
        assert_eq!(ran, limits.iter().product::<u32>(), "{limits:?}");
    }
}

/// A program that reads other lanes numbers its threads by host lane. In a
/// group of one dimension the limit is on those numbers, taken with every
/// lane still there: the wave's first lane is read in the entry block,
/// ahead of the branch. In a group of more dimensions it gets no limit.
#[test]
fn programs_reading_other_lanes_take_a_limit_in_one_dimension_only() {
    let other_lanes = [
        0x7E02_02FF,
        0x0000_0104, // v_mov_b32 v1, 0x104
        0x7E04_0285, // v_mov_b32 v2, 5
        0xDACC_000C,
        0x0300_0201, // ds_bpermute_b32 v3, v1, v2 offset:12
        0x7E00_0503, // v_readfirstlane_b32 s0, v3
        0xBF07_8000, // s_cmp_lg_u32 s0, 0
        0xBF85_0001, // s_cbranch_scc1 over the next instruction
        0xBE83_0380, // s_mov_b32 s3, 0
    ];
    let l = program([32, 1, 1], &other_lanes, true);
    assert!(l.wave_ops);
    assert_eq!(limits_of(&l).len(), 1);
    let entry = l.flow.calls(0);
    assert!(entry.contains(&"dx.op.waveReadLaneFirst.i32"), "{entry:?}");
    assert!(matches!(l.flow.blocks[0].term, FlowTerm::CondBr(..)));

    let (plain, asked) = (program([8, 4, 1], &other_lanes, false), program([8, 4, 1], &other_lanes, true));
    assert!(plain.wave_ops);
    assert!(limits_of(&asked).is_empty());
    assert_eq!(plain.dxil, asked.dxil);
}

/// A program that reads a user-data dword past what its root parameters
/// say they hold (a root SRV's second dword, where the container gives the
/// descriptor one) reads it where the limit would go. It is not refused
/// and is handed no limit in that place: it is lowered as without the
/// option. With the descriptor's two dwords declared, the limit follows.
#[test]
fn a_program_reading_user_data_where_the_limit_would_go_gets_none() {
    const fn s_mov_b32(sdst: u8, src0: u8) -> u32 {
        0xBE80_0000 | (sdst as u32) << 16 | 0x03 << 8 | src0 as u32
    }
    /// SMEM `s_load_dwordx4 s[sdata:sdata+3], s[sbase:sbase+1], 0`.
    const fn s_load_x4(sdata: u8, sbase: u8) -> [u32; 2] {
        [0x3D << 26 | 2 << 18 | (sdata as u32) << 6 | (sbase as u32 >> 1), 125 << 25]
    }
    const fn v_lshlrev_b32(vdst: u8, shift: u16, vsrc1: u8) -> u32 {
        0x1A << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (128 + shift) as u32
    }
    /// MUBUF `op` (offen) of `vdata` at `vaddr` through `s[srsrc:srsrc+3]`.
    const fn mubuf(op: u32, vdata: u8, vaddr: u8, srsrc: u8) -> [u32; 2] {
        [0x38 << 26 | op << 18 | 1 << 12, 0x80 << 24 | (srsrc as u32 >> 2) << 16 | (vdata as u32) << 8 | vaddr as u32]
    }
    // Reads a root SRV (its address in s[6:7]) through the buffer
    // descriptor it builds around it, a dword per thread, and stores what
    // it read through a view from a table (s4).
    let mut w = vec![
        s_mov_b32(5, 128), // s[4:5]: the table at s4
        s_mov_b32(0, 6),   // s[0:3]: the root SRV's buffer descriptor
        0x8801_FF07,
        0x0001_0000, // s_or_b32 s1, s7, stride 1
        0xBE82_03C1, // s_mov_b32 s2, -1
        0xBE83_03FF,
        0x1101_4FAC, // s_mov_b32 s3, (the console's buffer word 3)
    ];
    w.extend(s_load_x4(8, 4)); // s[8:11]: the result UAV
    w.push(v_lshlrev_b32(1, 6, 0)); // v1 = thread << 6
    w.extend(mubuf(0x0C, 2, 1, 0)); // buffer_load_dword v2, v1, s[0:3] offen
    w.push(v_lshlrev_b32(3, 2, 0)); // v3 = thread << 2
    w.push(0xBF8C_0000); // s_waitcnt
    w.extend(mubuf(0x1C, 2, 3, 8)); // buffer_store_dword v2, v3, s[8:11] offen
    w.push(S_ENDPGM);
    let used = |kind, root, stride, user_data, slot| UsedResource {
        kind,
        root,
        table_offset_dwords: 0,
        lower_bound: 0,
        stride,
        dwords: 4,
        user_data,
        slot,
    };
    let lowered = |descriptor_dwords: u32, limits: bool| {
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64: false,
            vgprs: 4,
            layout: vec![
                root(RootType::Table, 4, 1),
                RootParam {
                    sgprs: descriptor_dwords,
                    ..root(RootType::Descriptor, 6, 2)
                },
            ],
            used: vec![
                used(ResourceType::Uav, RootType::Table, 0, 4, 1),
                used(ResourceType::Srv, RootType::Descriptor, 1, 6, 2),
            ],
            io: StageIo::Cs(ComputeIo {
                rsrc2: 8 << 1,
                threads: [32, 1, 1],
            }),
            code: w.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = ir::build(&native.code, false, 4).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
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
        let l = lower_with(&parts, limits).unwrap();
        l.flow.check().unwrap();
        l
    };
    // The descriptor's second dword (the root address's offset in the
    // host's view) is user-data dword 7 either way.
    let offset = |l: &Lowered| l.user_data.iter().any(|u| u.index == 7 && u.root == RootType::Descriptor);
    // Declared whole: the limit comes after it.
    let whole = lowered(2, true);
    assert!(offset(&whole));
    assert_eq!(limits_of(&whole).iter().map(|u| u.index).collect::<Vec<_>>(), [8]);
    // Declared with one dword: dword 7 is where the limit would go.
    let (plain, asked) = (lowered(1, false), lowered(1, true));
    assert!(offset(&plain) && offset(&asked));
    assert!(limits_of(&asked).is_empty());
    assert_eq!(plain.dxil, asked.dxil);
    assert_eq!(plain.user_data, asked.user_data);
}

/// The program the host GPU test dispatches (`victory-gpu`,
/// `pipeline/thread_limit_tests.rs`; the same words), checked here where
/// no device is needed: with groups of 8 threads, each thread stores at the
/// texel of its number in the whole dispatch, group times 8 plus its
/// number in the group, so that which texels are written tells which
/// threads of which groups ran.
#[test]
fn the_host_tests_program_stores_at_its_thread_number_in_the_dispatch() {
    const GROUP: u32 = 8;
    let words: [u32; 13] = [
        0x8F06_8305, // s_lshl_b32 s6, s5, 3: the group id times 8
        0xBE85_0380, // s_mov_b32 s5, 0: high word of the table address
        0xF40C_0202,
        0xFA00_0000, // s_load_dwordx8 s[8:15], s[4:5], 0
        0x7E02_0200,
        0x7E04_0201,
        0x7E06_0202,
        0x7E08_0203, // v_mov_b32 v[1:4], s[0:3]
        0x4A0A_0006, // v_add_nc_u32 v5, s6, v0: plus the thread id in the group
        0x7E0C_0280, // v_mov_b32 v6, 0
        0xF020_1F08,
        0x0002_0105, // image_store v[1:4], v[5:6], s[8:15], dim:2D, dmask:0xf
        S_ENDPGM,
    ];
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 8,
        layout: vec![
            RootParam {
                sgprs: 4,
                ..root(RootType::Constants, 0, 0)
            },
            root(RootType::Table, 4, 1),
        ],
        used: vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 8,
            user_data: 4,
            slot: 1,
        }],
        io: StageIo::Cs(ComputeIo {
            // Five user SGPRs, then the group id along x in s5.
            rsrc2: 5 << 1 | 1 << 7,
            threads: [GROUP, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let p = ir::build(&native.code, false, native.vgprs).unwrap();
    let st = structure::structure(&p).unwrap();
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
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
    let l = lower_with(&parts, true).unwrap();
    l.flow.check().unwrap();
    let limits = limits_of(&l);
    assert_eq!(limits.len(), 1);
    // The texel thread `thread` of group `group` stores at, if it stores.
    let texel = |group: u32, thread: u32, limit: u32| {
        let mut user = [0u32; 8];
        user[limits[0].index as usize] = limit;
        let mut at = None;
        let mut call = |name: &str, args: &[u64]| match name {
            "dx.op.cbufferLoadLegacy.i32" => {
                let row = args[2] as usize * 4;
                user[row..row + 4].iter().map(|&v| u64::from(v)).collect()
            }
            "dx.op.groupId.i32" => vec![if args[1] == 0 { u64::from(group) } else { 0 }],
            "dx.op.threadIdInGroup.i32" => vec![if args[1] == 0 { u64::from(thread) } else { 0 }],
            "dx.op.textureStore.f32" => {
                assert!(at.is_none(), "one store");
                // After the opcode and the handle: x, y.
                assert_eq!(args[3], 0, "the row");
                at = Some(args[2] as u32);
                Vec::new()
            }
            _ => vec![0],
        };
        l.flow.run(1 << 16, &mut call).unwrap();
        at
    };
    for group in 0..4 {
        for thread in 0..GROUP {
            let number = group * GROUP + thread;
            assert_eq!(texel(group, thread, THREAD_LIMIT_NONE), Some(number));
            // 11 threads: one whole group and three threads of the next.
            assert_eq!(texel(group, thread, 11), (number < 11).then_some(number));
        }
    }
}
