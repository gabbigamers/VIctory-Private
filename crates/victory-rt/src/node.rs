//! The per-node test of the existing software path, in plain Rust: what
//! `victory-shader` emits for `image_bvh_intersect_ray`
//! (`crates/victory-shader/src/emit/lower/bvh.rs`, whose header names the
//! sources and states the semantics), operation for operation and in the
//! same order, so that the results are the same bits. One ray against one
//! node; the answer is four words.
//!
//! That crate's Windows tests have their own CPU reference of the same
//! lowering (`emit/bvh_tests.rs`), which they compare with the GPU word for
//! word. This module is a second copy of it, and
//! `the_validation_crates_node_test_equals_the_reference` there holds the
//! two together on random and on arbitrary nodes and rays: change one, and
//! the other has to follow before that test passes again. It runs on
//! Windows only, where those tests are built.
//!
//! * A pointer's low three bits are the node's type: 0-3 a triangle node,
//!   4 a box node with 16-bit float bounds, 5 one with 32-bit float bounds,
//!   6 an instance and 7 a procedural node, both refused (four
//!   `0xffffffff`). The node is `(pointer & !7) * 8` bytes past the base.
//! * Box nodes answer with their four child pointers, nearest first, misses
//!   last as `0xffffffff`.
//! * Triangle nodes answer with `t`'s numerator and denominator and two
//!   barycentric numerators, or `(+inf, 1.0, 0, 0)` on a miss.

pub const INVALID: u32 = u32::MAX;

/// The low three bits of a node pointer.
pub mod node_type {
    pub const TRIANGLE: u32 = 0;
    pub const BOX16: u32 = 4;
    pub const BOX32: u32 = 5;
    pub const INSTANCE: u32 = 6;
    pub const PROCEDURAL: u32 = 7;
}

/// The triangle id that returns the weights of vertices 1 and 2 for a type 0
/// triangle: selector 1 in bits 0-1, selector 2 in bits 2-3.
pub const TRIANGLE_ID_PLAIN: u32 = 9;

/// The instruction's ray operands.
#[derive(Clone, Copy, Debug)]
pub struct NodeRay {
    pub extent: f32,
    pub origin: [f32; 3],
    pub dir: [f32; 3],
    /// One over the direction, componentwise; the program computes it.
    pub inv: [f32; 3],
}

impl NodeRay {
    pub fn new(origin: [f32; 3], dir: [f32; 3], extent: f32) -> Self {
        NodeRay {
            extent,
            origin,
            dir,
            inv: dir.map(|d| 1.0 / d),
        }
    }
}

/// How many words of a node the test reads: 28 of a 32-bit box node, 16
/// otherwise.
pub fn words_read(pointer: u32) -> usize {
    if pointer & 7 == node_type::BOX32 {
        28
    } else {
        16
    }
}

/// The four result words for the node `w` (at least [`words_read`] words)
/// behind `pointer`.
pub fn intersect(w: &[u32], pointer: u32, ray: &NodeRay) -> [u32; 4] {
    let ty = pointer & 7;
    if ty < 4 {
        triangle(w, ty, ray)
    } else if ty & 6 == 4 {
        boxes(w, ty == node_type::BOX16, ray)
    } else {
        [INVALID; 4]
    }
}

/// A 16-bit float as the `f32` of the same value (the low 16 bits of `h`).
pub fn half_to_f32(h: u32) -> f32 {
    let (sign, e, m) = ((h & 0x8000) << 16, (h >> 10) & 0x1F, h & 0x3FF);
    let magnitude = match e {
        // Zero and subnormals: m * 2^-24, exact.
        0 => (m as f32 * (1.0 / 16_777_216.0)).to_bits(),
        0x1F => 0x7F80_0000 | m << 13,
        _ => (e + 112) << 23 | m << 13,
    };
    f32::from_bits(sign | magnitude)
}

fn boxes(w: &[u32], box16: bool, ray: &NodeRay) -> [u32; 4] {
    let mut child = [INVALID; 4];
    let mut key = [0f32; 4];
    for i in 0..4 {
        // min xyz, max xyz.
        let b: [f32; 6] = std::array::from_fn(|j| {
            if box16 {
                // Pairs: minx|miny, minz|maxx, maxy|maxz.
                half_to_f32(w[4 + 3 * i + j / 2] >> (16 * (j % 2)))
            } else {
                f32::from_bits(w[4 + 6 * i + j])
            }
        });
        let (mut nan, mut near, mut far) = (false, 0f32, f32::INFINITY);
        for a in 0..3 {
            let (dlo, dhi) = (b[a] - ray.origin[a], b[3 + a] - ray.origin[a]);
            let (tlo, thi) = (dlo * ray.inv[a], dhi * ray.inv[a]);
            let pos = ray.inv[a] >= 0.0;
            let (mut n, mut x) = if pos { (tlo, thi) } else { (thi, tlo) };
            // A ray in the plane (0 * inf) is not bounded by it.
            let (dn, dx) = if pos { (dlo, dhi) } else { (dhi, dlo) };
            if dn == 0.0 && n.is_nan() {
                n = f32::NEG_INFINITY;
            }
            if dx == 0.0 && x.is_nan() {
                x = f32::INFINITY;
            }
            nan |= n.is_nan() || x.is_nan();
            if n > near {
                near = n;
            }
            if x < far {
                far = x;
            }
        }
        // A NaN extent bounds nothing.
        if ray.extent < far {
            far = ray.extent;
        }
        if near <= far && !nan {
            child[i] = w[i];
        }
        key[i] = near;
    }
    // A valid child moves before a farther or invalid one.
    for (a, b) in [(0, 2), (1, 3), (0, 1), (2, 3), (1, 2)] {
        if (child[b] != INVALID && key[b] < key[a]) || child[a] == INVALID {
            child.swap(a, b);
            key.swap(a, b);
        }
    }
    child
}

fn triangle(w: &[u32], ty: u32, ray: &NodeRay) -> [u32; 4] {
    let vertex = |k: usize| -> [f32; 3] { std::array::from_fn(|j| f32::from_bits(w[3 * k + j])) };
    // The vertices each type tests.
    let t = ty as usize;
    let p = [[0, 1, 2, 2][t], [1, 3, 3, 4][t], [2, 2, 4, 0][t]].map(vertex);
    // kz: the axis of the largest |direction| (ties to the lower axis); kx
    // and ky the next two, swapped where the direction along kz is negative.
    let ad = ray.dir.map(f32::abs);
    let x_ge_y = ad[0] >= ad[1];
    let z0 = x_ge_y && ad[0] >= ad[2];
    let z1 = !x_ge_y && ad[1] >= ad[2];
    let by = |v: [f32; 3], k: [usize; 3]| {
        if z0 {
            v[k[0]]
        } else if z1 {
            v[k[1]]
        } else {
            v[k[2]]
        }
    };
    let neg = by(ray.dir, [0, 1, 2]) < 0.0;
    let perm = |v: [f32; 3]| -> [f32; 3] {
        let (x, y, z) = (by(v, [1, 2, 0]), by(v, [2, 0, 1]), by(v, [0, 1, 2]));
        if neg { [y, x, z] } else { [x, y, z] }
    };
    let d = perm(ray.dir);
    let sz = 1.0 / d[2];
    let (sx, sy) = (d[0] * sz, d[1] * sz);
    let sheared = |v: [f32; 3]| -> [f32; 3] {
        let r = perm(std::array::from_fn(|k| v[k] - ray.origin[k]));
        [r[0] - sx * r[2], r[1] - sy * r[2], sz * r[2]]
    };
    let [a, b, c] = p.map(sheared);
    let edge = |p: [f32; 3], q: [f32; 3]| p[0] * q[1] - p[1] * q[0];
    let (u, v, ww) = (edge(c, b), edge(a, c), edge(b, a));
    let inside = (u >= 0.0 && v >= 0.0 && ww >= 0.0) || (u <= 0.0 && v <= 0.0 && ww <= 0.0);
    let det = u + (v + ww);
    let tnum = (u * a[2] + v * b[2]) + ww * c[2];
    // t >= 0: the numerator has the denominator's sign (0 misses).
    let ahead = (det > 0.0 && tnum >= 0.0) || (det < 0.0 && tnum <= 0.0);
    if !(inside && ahead) {
        return [f32::INFINITY.to_bits(), 1.0f32.to_bits(), 0, 0];
    }
    // The barycentric numerators the triangle id's bits for this type pick.
    let id = w[15] >> (8 * ty);
    let pick = |selector: u32| match selector & 3 {
        0 => u.to_bits(),
        1 => v.to_bits(),
        2 => ww.to_bits(),
        _ => 0,
    };
    [tnum.to_bits(), det.to_bits(), pick(id), pick(id >> 2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box32(children: [u32; 4], boxes: [[f32; 6]; 4]) -> Vec<u32> {
        let mut w = children.to_vec();
        w.extend(boxes.iter().flatten().map(|x| x.to_bits()));
        w
    }

    fn slab(at: f32) -> [f32; 6] {
        [at, -1.0, -1.0, at + 1.0, 1.0, 1.0]
    }

    const NONE: [f32; 6] = [f32::NAN; 6];

    fn along_x() -> NodeRay {
        NodeRay::new([0.0; 3], [1.0, 0.0, 0.0], 100.0)
    }

    fn tri(v: [[f32; 3]; 3]) -> Vec<u32> {
        let mut w: Vec<u32> = v.iter().flatten().map(|x| x.to_bits()).collect();
        w.resize(15, 0);
        w.push(TRIANGLE_ID_PLAIN);
        w
    }

    #[test]
    fn halves_convert_exactly() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x0001), 1.0 / 16_777_216.0);
        assert_eq!(half_to_f32(0x03FF), 1023.0 / 16_777_216.0);
        assert_eq!(half_to_f32(0x0400), 1.0 / 16_384.0);
        assert_eq!(half_to_f32(0x7BFF), 65_504.0);
        assert_eq!(half_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(half_to_f32(0xFC00), f32::NEG_INFINITY);
        assert!(half_to_f32(0x7E00).is_nan());
        assert_eq!(half_to_f32(0x8000).to_bits(), 0x8000_0000);
        // Only the low 16 bits count.
        assert_eq!(half_to_f32(0xABCD_3C00), 1.0);
    }

    #[test]
    fn children_come_nearest_first_and_misses_last() {
        let w = box32([8, 16, 24, 32], [slab(30.0), NONE, slab(10.0), slab(-5.0)]);
        assert_eq!(intersect(&w, 5, &along_x()), [24, 8, INVALID, INVALID]);
        let w = box32([1, 2, 3, 4], [slab(3.0), slab(2.0), slab(1.0), slab(0.0)]);
        assert_eq!(intersect(&w, 5, &along_x()), [4, 3, 2, 1]);
        // Equal distances keep the network's order.
        let w = box32([1, 2, 3, 4], [slab(5.0); 4]);
        assert_eq!(intersect(&w, 5, &along_x()), [1, 2, 3, 4]);
        let w = box32([1, 2, 3, 4], [NONE, slab(5.0), slab(5.0), slab(5.0)]);
        assert_eq!(intersect(&w, 5, &along_x()), [3, 2, 4, INVALID]);
    }

    #[test]
    fn the_extent_is_inclusive_and_a_box_behind_misses() {
        let ray = NodeRay::new([0.0; 3], [1.0, 0.0, 0.0], 10.0);
        let w = box32(
            [1, 2, 3, 4],
            [slab(10.0), slab(10.5), slab(-1.0), slab(-2.0)],
        );
        assert_eq!(intersect(&w, 5, &ray), [3, 1, INVALID, INVALID]);
        // A ray in a bounding plane is inside; one beside the slab is not.
        let on = [2.0, 0.0, -1.0, 3.0, 1.0, 1.0];
        let off = [2.0, 0.5, -1.0, 3.0, 1.0, 1.0];
        let w = box32([1, 2, 3, 4], [on, off, NONE, NONE]);
        assert_eq!(intersect(&w, 5, &ray), [1, INVALID, INVALID, INVALID]);
    }

    #[test]
    fn a_16_bit_box_node_reads_pairs() {
        // minx|miny, minz|maxx, maxy|maxz with 2 = 0x4000, 3 = 0x4200,
        // -1 = 0xBC00, 1 = 0x3C00: the box [2, 3] x [-1, 1] x [-1, 1].
        let mut w = vec![7, 8, 9, 10];
        for k in 0..4 {
            w.extend(if k == 1 {
                [0xBC00_4000, 0x4200_BC00, 0x3C00_3C00]
            } else {
                [0x7E00_7E00; 3]
            });
        }
        w.resize(16, 0);
        assert_eq!(intersect(&w, 4, &along_x()), [8, INVALID, INVALID, INVALID]);
    }

    #[test]
    fn instance_and_procedural_nodes_are_refused() {
        let w = vec![0u32; 28];
        assert_eq!(intersect(&w, 6, &along_x()), [INVALID; 4]);
        assert_eq!(intersect(&w, 0x107, &along_x()), [INVALID; 4]);
    }

    #[test]
    fn a_triangle_answers_in_ratios_and_a_positive_denominator_is_a_front_face() {
        let ray = NodeRay::new([0.25, 0.25, -1.0], [0.0, 0.0, 1.0], 100.0);
        let v = [[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]];
        let r = intersect(&tri(v), 0, &ray).map(f32::from_bits);
        assert_eq!((r[0] / r[1], r[2] / r[1], r[3] / r[1]), (3.0, 0.25, 0.25));
        // The ray runs along (V1 - V0) x (V2 - V0): a back face.
        assert!(r[1] < 0.0);
        let r = intersect(&tri([v[0], v[2], v[1]]), 0, &ray).map(f32::from_bits);
        assert!(r[1] > 0.0);
        // Behind the origin, in the plane, without area: misses.
        let miss = [0x7F80_0000, 0x3F80_0000, 0, 0];
        let behind = NodeRay::new([0.25, 0.25, 3.0], [0.0, 0.0, 1.0], 100.0);
        assert_eq!(intersect(&tri(v), 0, &behind), miss);
        let flat = NodeRay::new([-1.0, 0.25, 2.0], [1.0, 0.0, 0.0], 100.0);
        assert_eq!(intersect(&tri(v), 0, &flat), miss);
        let line = [[-1.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 0.0, 2.0]];
        let down = NodeRay::new([0.0; 3], [0.0, 0.0, 1.0], 100.0);
        assert_eq!(intersect(&tri(line), 0, &down), miss);
        // No extent test: the program compares t.
        let short = NodeRay::new([0.25, 0.25, -1.0], [0.0, 0.0, 1.0], 1.0);
        let r = intersect(&tri(v), 0, &short).map(f32::from_bits);
        assert_eq!(r[0] / r[1], 3.0);
    }

    #[test]
    fn edges_and_vertices_count_as_inside() {
        let v = [[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]];
        for at in [
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
            [0.5, 0.0],
            [0.5, 0.5],
            [0.0, 0.25],
        ] {
            let ray = NodeRay::new([at[0], at[1], 0.0], [0.0, 0.0, 1.0], 100.0);
            let r = intersect(&tri(v), 0, &ray).map(f32::from_bits);
            assert_eq!(r[0] / r[1], 2.0, "{at:?}");
        }
        let ray = NodeRay::new([1.0, 1.0, 0.0], [0.0, 0.0, 1.0], 100.0);
        assert_eq!(intersect(&tri(v), 0, &ray)[0], f32::INFINITY.to_bits());
    }

    #[test]
    fn the_triangle_id_picks_the_weights() {
        let ray = NodeRay::new([0.25, 0.5, -1.0], [0.0, 0.0, 1.0], 100.0);
        let v = [[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]];
        let mut w = tri(v);
        let weights = |w: &[u32]| {
            let r = intersect(w, 0, &ray).map(f32::from_bits);
            (r[2] / r[1], r[3] / r[1])
        };
        // 9: vertex 1's weight, then vertex 2's.
        assert_eq!(weights(&w), (0.25, 0.5));
        // 0 | 1 << 2: vertex 0's, then vertex 1's; selector 3 reads 0.
        w[15] = 4;
        assert_eq!(weights(&w), (0.25, 0.25));
        w[15] = 0xF;
        assert_eq!(weights(&w), (0.0, 0.0));
        // Type 1 tests (V1, V3, V2) and reads the id's second byte.
        let mut w: Vec<u32> = [[9.0f32; 3], v[0], v[2], v[1]]
            .iter()
            .flatten()
            .map(|x| x.to_bits())
            .collect();
        w.resize(15, 0);
        w.push(9 << 8);
        let r = intersect(&w, 1, &ray).map(f32::from_bits);
        assert_eq!((r[0] / r[1], r[2] / r[1], r[3] / r[1]), (3.0, 0.25, 0.5));
    }
}
