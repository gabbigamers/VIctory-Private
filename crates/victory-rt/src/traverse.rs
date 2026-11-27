//! The software traversal: a ray through an image of console-format nodes
//! ([`crate::console`]), every box and triangle decided by the node test
//! ([`crate::node`]). It is the CPU model a GPU run of the same traversal
//! is checked against, so it is specified to the last choice. All
//! arithmetic is `f32`.
//!
//! * **State.** The committed hit (none at first) and its `t`, which starts
//!   as the ray's `tmax`; the current ray and base (the world ray and the
//!   top level, or an instance's object ray and its bottom level); a stack
//!   of [`STACK_DEPTH`] 32-bit entries, holding the top level's root
//!   pointer at the start.
//! * **A step** pops one entry.
//!   * The return marker `0xfffffffe`: back to the world ray and the top
//!     level.
//!   * A box pointer: the node test, with the committed `t` as the ray's
//!     extent; the valid children are pushed last to first, so the nearest
//!     is popped next. Children already on the stack are not tested again
//!     when the committed `t` shrinks.
//!   * An instance pointer (top level only): the record is read. If `mask &
//!     ray mask & 0xff` is 0 it is skipped. Otherwise the object ray is
//!     `origin' = W origin + w`, `direction' = W direction` with each row
//!     as `((w0 x + w1 y) + w2 z) + w3` (no `w3` for the direction), the
//!     inverse direction is `1 / direction'` per component, and the marker
//!     and then the bottom level's root are pushed.
//!   * A triangle pointer: the node test gives `(tn, d, i, j)`; `tn` of
//!     +infinity is a miss. Otherwise `t = tn / d`, `u = i / d`, `v = j /
//!     d`, and the candidate is tested in this order: `tmin < t` and `t <
//!     committed t` (both strict, so of two candidates with the same `t`
//!     the one met first stays); facing (front when `d > 0`, swapped by the
//!     instance's counter-clockwise flag) against the ray's cull flags
//!     unless the instance disables culling; opacity (the node's opaque
//!     bit, then the instance's force flags, then the ray's) against the
//!     ray's opacity cull flags; for a non-opaque candidate the alpha test
//!     ([`crate::alpha_accepts`]). One that passes is committed, and with
//!     `ACCEPT_FIRST_HIT_AND_END_SEARCH` the traversal ends there.
//! * **The end**: an empty stack, or the first-hit stop. A ray with
//!   `SKIP_TRIANGLES` ends at once with a miss.
//! * **Refusals**, as errors and never as a quiet miss: a ray whose origin
//!   has a coordinate beyond the reach in the image's header (the boxes are
//!   not padded for it), a stack that would overflow, a pointer outside the
//!   image, a node type that has no place (a 16-bit box node, a procedural
//!   node, an instance inside a bottom level), more than [`MAX_STEPS`]
//!   steps.

use crate::alpha_accepts;
use crate::console::{OPAQUE_BIT, REACH_WORD, SLOT, STACK_DEPTH, SwScene, UNIT};
use crate::node::{INVALID, NodeRay, intersect, node_type};
use crate::record::{Hit, Ray, hit_kind, instance_flag, ray_flag};

/// Pushed under a bottom level's root; popped, it returns to the top level.
pub const RETURN: u32 = 0xFFFF_FFFE;
/// A traversal that takes more steps than this is refused.
pub const MAX_STEPS: u32 = 1 << 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    OutOfReach,
    StackOverflow,
    OutOfImage,
    NodeType(u32),
    Steps,
}

/// How box nodes are decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Boxes {
    /// By the node test.
    #[default]
    Tested,
    /// Every child with a pointer is visited, in slot order. For telling a
    /// miss the box test caused from any other.
    AllVisited,
}

/// What one traversal did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub box_tests: u32,
    pub triangle_tests: u32,
    pub instances_entered: u32,
    pub deepest_stack: u32,
}

/// The instance the traversal is inside.
#[derive(Clone, Copy)]
struct Inside {
    base: usize,
    index: u32,
    id: u32,
    flags: u32,
}

fn node(words: &[u32], base: usize, pointer: u32, slots: usize) -> Result<&[u32], Refusal> {
    let at = base + (pointer >> 3) as usize * SLOT;
    words.get(at..at + slots * SLOT).ok_or(Refusal::OutOfImage)
}

/// The hit of `ray` (which must pass [`Ray::validate`]) in `scene`.
pub fn trace(scene: &SwScene, ray: &Ray, boxes: Boxes) -> Result<(Hit, Work), Refusal> {
    let words = &scene.words[..];
    let mut work = Work::default();
    let mut hit = Hit::MISS;
    if ray.flags & ray_flag::SKIP_TRIANGLES != 0 {
        return Ok((hit, work));
    }
    let reach = f32::from_bits(words[REACH_WORD]);
    // Written so that a NaN reach refuses.
    if !ray.origin.iter().all(|x| x.abs() <= reach) {
        return Err(Refusal::OutOfReach);
    }
    let mut committed = ray.tmax;
    let world = NodeRay::new(ray.origin, ray.dir, committed);
    let mut current = world;
    let mut inside: Option<Inside> = None;
    let mut stack = [0u32; STACK_DEPTH];
    let mut depth = 0usize;
    let push = |stack: &mut [u32; STACK_DEPTH], depth: &mut usize, x: u32, work: &mut Work| {
        if *depth == STACK_DEPTH {
            return Err(Refusal::StackOverflow);
        }
        stack[*depth] = x;
        *depth += 1;
        work.deepest_stack = work.deepest_stack.max(*depth as u32);
        Ok(())
    };
    push(&mut stack, &mut depth, node_type::BOX32, &mut work)?;
    let mut steps = 0u32;
    while depth > 0 {
        steps += 1;
        if steps > MAX_STEPS {
            return Err(Refusal::Steps);
        }
        depth -= 1;
        let pointer = stack[depth];
        if pointer == RETURN {
            (inside, current) = (None, world);
            continue;
        }
        let base = inside.map_or(scene.top_base, |i| i.base);
        match (pointer & 7, inside) {
            (node_type::BOX32, _) => {
                let w = node(words, base, pointer, 2)?;
                work.box_tests += 1;
                let children = match boxes {
                    Boxes::Tested => {
                        current.extent = committed;
                        intersect(w, pointer, &current)
                    }
                    Boxes::AllVisited => [w[0], w[1], w[2], w[3]],
                };
                for &child in children.iter().rev() {
                    if child != INVALID {
                        push(&mut stack, &mut depth, child, &mut work)?;
                    }
                }
            }
            (node_type::INSTANCE, None) => {
                let w = node(words, base, pointer, 2)?;
                if (w[12] >> 24) & ray.mask & 0xFF == 0 {
                    continue;
                }
                let m: [f32; 12] = std::array::from_fn(|k| f32::from_bits(w[k]));
                let row = |r: usize, p: [f32; 3]| {
                    (m[4 * r] * p[0] + m[4 * r + 1] * p[1]) + m[4 * r + 2] * p[2]
                };
                let origin: [f32; 3] = std::array::from_fn(|r| row(r, ray.origin) + m[4 * r + 3]);
                let dir: [f32; 3] = std::array::from_fn(|r| row(r, ray.dir));
                current = NodeRay::new(origin, dir, committed);
                inside = Some(Inside {
                    base: w[14] as usize * UNIT,
                    index: w[15],
                    id: w[12] & 0x00FF_FFFF,
                    flags: w[13],
                });
                work.instances_entered += 1;
                push(&mut stack, &mut depth, RETURN, &mut work)?;
                push(&mut stack, &mut depth, w[28], &mut work)?;
            }
            (node_type::TRIANGLE, Some(at)) => {
                let w = node(words, base, pointer, 1)?;
                work.triangle_tests += 1;
                let r = intersect(w, pointer, &current);
                if r[0] == f32::INFINITY.to_bits() {
                    continue;
                }
                let [tn, d, i, j] = r.map(f32::from_bits);
                let (t, u, v) = (tn / d, i / d, j / d);
                // Written so that a NaN fails.
                if !(ray.tmin < t && t < committed) {
                    continue;
                }
                let mut front = d > 0.0;
                if at.flags & instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE != 0 {
                    front = !front;
                }
                if at.flags & instance_flag::TRIANGLE_CULL_DISABLE == 0 {
                    let cull_front = ray.flags & ray_flag::CULL_FRONT_FACING_TRIANGLES != 0;
                    let cull_back = ray.flags & ray_flag::CULL_BACK_FACING_TRIANGLES != 0;
                    if (front && cull_front) || (!front && cull_back) {
                        continue;
                    }
                }
                let opaque = if ray.flags & (ray_flag::FORCE_OPAQUE | ray_flag::FORCE_NON_OPAQUE)
                    != 0
                {
                    ray.flags & ray_flag::FORCE_OPAQUE != 0
                } else if at.flags & (instance_flag::FORCE_OPAQUE | instance_flag::FORCE_NON_OPAQUE)
                    != 0
                {
                    at.flags & instance_flag::FORCE_OPAQUE != 0
                } else {
                    w[12] & OPAQUE_BIT != 0
                };
                if ray.flags & ray_flag::CULL_OPAQUE != 0 && opaque {
                    continue;
                }
                if ray.flags & ray_flag::CULL_NON_OPAQUE != 0 && !opaque {
                    continue;
                }
                if !opaque && !alpha_accepts(u, v, w[13]) {
                    continue;
                }
                committed = t;
                hit = Hit {
                    t,
                    u,
                    v,
                    kind: if front {
                        hit_kind::FRONT
                    } else {
                        hit_kind::BACK
                    },
                    instance_index: at.index,
                    instance_id: at.id,
                    geometry_index: w[12] & !OPAQUE_BIT,
                    primitive_index: w[13],
                };
                if ray.flags & ray_flag::ACCEPT_FIRST_HIT_AND_END_SEARCH != 0 {
                    break;
                }
            }
            (ty, _) => return Err(Refusal::NodeType(ty)),
        }
    }
    Ok((hit, work))
}

/// [`trace`] for each ray, by the node test.
pub fn trace_all(scene: &SwScene, rays: &[Ray]) -> Result<Vec<Hit>, Refusal> {
    rays.iter()
        .map(|r| trace(scene, r, Boxes::Tested).map(|(h, _)| h))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::BuildOptions;
    use crate::scene::{BottomLevel, BuildFlags, Geometry, Indices, Instance, Scene, Transform};

    /// Unit squares in the planes z = 0, 1, .., facing -z, two triangles
    /// each.
    fn layers(n: usize, opaque: bool) -> BottomLevel {
        BottomLevel {
            geometries: (0..n)
                .map(|k| Geometry {
                    vertices: vec![
                        [0.0, 0.0, k as f32],
                        [0.0, 1.0, k as f32],
                        [1.0, 0.0, k as f32],
                        [1.0, 1.0, k as f32],
                    ],
                    indices: Indices::U32(vec![0, 1, 2, 2, 1, 3]),
                    transform: None,
                    opaque,
                })
                .collect(),
            flags: BuildFlags::default(),
        }
    }

    fn one(bottom: BottomLevel, transform: Transform, flags: u32) -> SwScene {
        let scene = Scene {
            bottoms: vec![bottom],
            instances: vec![Instance {
                transform,
                id: 0x12_3456,
                mask: 0x0F,
                flags,
                blas: 0,
            }],
            flags: BuildFlags::default(),
        };
        SwScene::build(&scene, BuildOptions::reach(16.0)).unwrap()
    }

    fn down(x: f32, y: f32) -> Ray {
        Ray::new([x, y, -4.0], [0.0, 0.0, 2.0], 0.0, 100.0)
    }

    fn hit(scene: &SwScene, ray: &Ray) -> Hit {
        trace(scene, ray, Boxes::Tested).unwrap().0
    }

    #[test]
    fn the_nearest_layer_is_hit_with_its_ids_and_weights() {
        let sw = one(layers(5, true), Transform::IDENTITY, 0);
        let h = hit(&sw, &down(0.25, 0.5));
        // Vertex 1 is (0, 1), vertex 2 is (1, 0): u = y, v = x.
        assert_eq!((h.t, h.u, h.v), (2.0, 0.5, 0.25));
        // (v1 - v0) x (v2 - v0) = (0, 1, 0) x (1, 0, 0) = -z, against the
        // ray: a front face.
        assert_eq!(h.kind, hit_kind::FRONT);
        assert_eq!(h.ids(), [0, 0x12_3456, 0, 0]);
        // The second triangle of the square.
        let h = hit(&sw, &down(0.75, 0.75));
        assert_eq!(h.ids(), [0, 0x12_3456, 0, 1]);
        // From behind: the last layer, a back face.
        let up = Ray::new([0.25, 0.5, 9.0], [0.0, 0.0, -1.0], 0.0, 100.0);
        let h = hit(&sw, &up);
        assert_eq!((h.t, h.kind, h.geometry_index), (5.0, hit_kind::BACK, 4));
        assert!(hit(&sw, &down(1.5, 0.5)).is_miss());
        assert_eq!(hit(&sw, &down(1.5, 0.5)), Hit::MISS);
    }

    #[test]
    fn the_range_is_exclusive_at_both_ends() {
        let sw = one(layers(3, true), Transform::IDENTITY, 0);
        let mut ray = down(0.25, 0.5);
        ray.tmax = 2.0;
        assert!(hit(&sw, &ray).is_miss());
        ray.tmax = 2.0f32.next_up();
        assert_eq!(hit(&sw, &ray).t, 2.0);
        (ray.tmin, ray.tmax) = (2.0, 100.0);
        assert_eq!(hit(&sw, &ray).t, 2.5);
        ray.tmin = 2.0f32.next_down();
        assert_eq!(hit(&sw, &ray).t, 2.0);
    }

    #[test]
    fn masks_culling_and_winding_follow_the_flags() {
        let sw = one(layers(2, true), Transform::IDENTITY, 0);
        let mut ray = down(0.25, 0.5);
        ray.mask = 0xF0;
        assert!(hit(&sw, &ray).is_miss());
        ray.mask = 0x1_08;
        assert_eq!(hit(&sw, &ray).t, 2.0);
        ray.flags = ray_flag::CULL_FRONT_FACING_TRIANGLES;
        assert!(hit(&sw, &ray).is_miss());
        ray.flags = ray_flag::CULL_BACK_FACING_TRIANGLES;
        assert_eq!(hit(&sw, &ray).kind, hit_kind::FRONT);
        // Counter-clockwise front faces: the same triangles are back faces.
        let ccw = one(
            layers(2, true),
            Transform::IDENTITY,
            instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE,
        );
        assert!(hit(&ccw, &ray).is_miss());
        ray.flags = 0;
        assert_eq!(hit(&ccw, &ray).kind, hit_kind::BACK);
        // Culling disabled on the instance.
        let off = one(
            layers(2, true),
            Transform::IDENTITY,
            instance_flag::TRIANGLE_CULL_DISABLE,
        );
        ray.flags = ray_flag::CULL_FRONT_FACING_TRIANGLES;
        assert_eq!(hit(&off, &ray).t, 2.0);
        ray.flags = ray_flag::SKIP_TRIANGLES;
        assert!(hit(&off, &ray).is_miss());
    }

    #[test]
    fn a_mirrored_instance_keeps_its_facing() {
        // The instance transform does not enter the facing: mirrored in x,
        // the squares still show their front to a ray along +z.
        let mirror = Transform::scale([-1.0, 1.0, 1.0]);
        let sw = one(layers(1, true), mirror, 0);
        let h = hit(&sw, &down(-0.25, 0.5));
        assert_eq!((h.t, h.kind), (2.0, hit_kind::FRONT));
        assert_eq!((h.u, h.v), (0.5, 0.25));
    }

    #[test]
    fn alpha_rejects_let_the_ray_through_to_the_next_layer() {
        let sw = one(layers(4, false), Transform::IDENTITY, 0);
        // u = 0.3, v = 0.2: cells 2 and 1, primitive 0: odd, rejected on
        // every layer.
        assert!(hit(&sw, &down(0.2, 0.3)).is_miss());
        // u = 0.3, v = 0.3: cells 2 and 2: accepted on the first.
        assert_eq!(hit(&sw, &down(0.3, 0.3)).t, 2.0);
        let mut ray = down(0.2, 0.3);
        ray.flags = ray_flag::FORCE_OPAQUE;
        assert_eq!(hit(&sw, &ray).t, 2.0);
        ray.flags = ray_flag::CULL_NON_OPAQUE;
        assert!(hit(&sw, &ray).is_miss());
        // The instance forces opaque; the ray's flag wins over it.
        let forced = one(
            layers(4, false),
            Transform::IDENTITY,
            instance_flag::FORCE_OPAQUE,
        );
        assert_eq!(hit(&forced, &down(0.2, 0.3)).t, 2.0);
        ray.flags = ray_flag::FORCE_NON_OPAQUE;
        assert!(hit(&forced, &ray).is_miss());
        ray.flags = ray_flag::CULL_OPAQUE;
        assert!(hit(&forced, &ray).is_miss());
    }

    #[test]
    fn the_first_hit_flag_ends_the_search_at_an_accepted_hit() {
        let sw = one(layers(40, true), Transform::IDENTITY, 0);
        let mut ray = down(0.25, 0.5);
        let (full, full_work) = trace(&sw, &ray, Boxes::Tested).unwrap();
        ray.flags = ray_flag::ACCEPT_FIRST_HIT_AND_END_SEARCH;
        let (first, first_work) = trace(&sw, &ray, Boxes::Tested).unwrap();
        assert_eq!(full.t, 2.0);
        assert!(!first.is_miss() && first.t >= 2.0);
        assert!(first_work.triangle_tests <= full_work.triangle_tests);
        // Every box visited gives the same nearest hit, at more work.
        ray.flags = 0;
        let (all, all_work) = trace(&sw, &ray, Boxes::AllVisited).unwrap();
        assert_eq!(all, full);
        assert_eq!(all_work.triangle_tests, 80);
        assert!(full_work.triangle_tests < 80);
        assert!(full_work.deepest_stack <= sw.stack && all_work.deepest_stack <= sw.stack);
    }

    #[test]
    fn a_damaged_image_is_refused_not_missed() {
        let mut sw = one(layers(2, true), Transform::IDENTITY, 0);
        let ray = down(0.25, 0.5);
        // The image is padded for origins within 16.
        let mut far = ray;
        far.origin[1] = -16.5;
        assert_eq!(trace(&sw, &far, Boxes::Tested), Err(Refusal::OutOfReach));
        let b = sw.bottoms[0].base;
        let child = sw.words[b];
        // A child pointer past the image.
        sw.words[b] = 0x7FFF_FFF8;
        assert_eq!(
            trace(&sw, &ray, Boxes::AllVisited),
            Err(Refusal::OutOfImage)
        );
        // A procedural node, and an instance inside a bottom level.
        sw.words[b] = child | 7;
        assert_eq!(
            trace(&sw, &ray, Boxes::AllVisited),
            Err(Refusal::NodeType(7))
        );
        sw.words[b] = child | 6;
        assert_eq!(
            trace(&sw, &ray, Boxes::AllVisited),
            Err(Refusal::NodeType(6))
        );
    }

    #[test]
    fn a_loop_in_the_image_ends_at_the_stack_or_at_the_step_limit() {
        let mut sw = one(layers(2, true), Transform::IDENTITY, 0);
        let ray = down(0.25, 0.5);
        let b = sw.bottoms[0].base;
        // A box that is twice its own child: every step pops one entry and
        // pushes two, and the stack is full after as many steps as it has
        // entries.
        sw.words[b..b + 4].copy_from_slice(&[5, 5, INVALID, INVALID]);
        assert_eq!(
            trace(&sw, &ray, Boxes::AllVisited),
            Err(Refusal::StackOverflow)
        );
        // Once its own child, it pops one and pushes one: the stack never
        // grows and only the step limit ends it.
        sw.words[b + 1] = INVALID;
        assert_eq!(trace(&sw, &ray, Boxes::AllVisited), Err(Refusal::Steps));
        // By the node test too: the ray is inside the box it keeps
        // re-entering.
        let (lo, hi) = (b + 4, b + 7);
        for k in 0..3 {
            sw.words[lo + k] = (-8.0f32).to_bits();
            sw.words[hi + k] = 8.0f32.to_bits();
        }
        assert_eq!(trace(&sw, &ray, Boxes::Tested), Err(Refusal::Steps));
    }

    #[test]
    fn the_stack_holds_its_last_entry_and_refuses_the_next() {
        // A chain of 21 boxes written by hand after the image, hung under
        // the bottom level's root: each has the next box as its first child
        // and one triangle of the image three times as the others, so each
        // level leaves three entries waiting while the next is walked. With
        // the return marker under them, twenty levels are 62 entries when
        // the last box is popped (61) and pushes its own children.
        let built = one(layers(1, true), Transform::IDENTITY, 0);
        let chain = |last: usize| -> SwScene {
            let mut sw = built.clone();
            let base = sw.bottoms[0].base;
            let triangle = sw.words[base];
            assert_eq!(triangle & 7, node_type::TRIANGLE);
            let first = (sw.words.len() - base) / SLOT;
            for k in 0..21 {
                let at = sw.words.len();
                sw.words.resize(at + 2 * SLOT, 0);
                let next = ((first + 2 * (k + 1)) as u32) << 3 | node_type::BOX32;
                let children: [u32; 4] = std::array::from_fn(|c| match (k < 20, c) {
                    (true, 0) => next,
                    (true, _) => triangle,
                    (false, c) if c < last => triangle,
                    _ => INVALID,
                });
                sw.words[at..at + 4].copy_from_slice(&children);
            }
            let root = [(first as u32) << 3 | node_type::BOX32, INVALID, INVALID];
            sw.words[base..base + 3].copy_from_slice(&root);
            sw.words[base + 3] = INVALID;
            sw
        };
        let ray = down(0.25, 0.5);
        // Three children at the end: entry 64, the last there is.
        let (hit, work) = trace(&chain(3), &ray, Boxes::AllVisited).unwrap();
        assert_eq!(work.deepest_stack as usize, STACK_DEPTH);
        assert_eq!((hit.t, work.triangle_tests), (2.0, 63));
        // Four: the next one, refused and not dropped.
        assert_eq!(
            trace(&chain(4), &ray, Boxes::AllVisited),
            Err(Refusal::StackOverflow)
        );
    }
}
