//! A scene written as console-format BVH nodes, for the software model to
//! traverse and for a later GPU run of the same traversal.
//!
//! What is the console's and what is ours:
//!
//! * The 64-byte triangle node and the 128-byte box node with 32-bit float
//!   bounds are laid out as the node test reads them ([`crate::node`]):
//!   vertices from byte 0 and the triangle id at byte 60; four child
//!   pointers, then four boxes as min xyz and max xyz.
//! * Everything else is **synthetic, a layout of our own, and not any
//!   title's**: the image header, the fields of a triangle node the test
//!   does not read (geometry index, opaque bit, primitive index), the whole
//!   top level and its instance records (the instruction refuses instance
//!   nodes, so no program's layout for them is implied by it), the shape of
//!   the trees and the padding of the boxes.
//!
//! The image, little-endian 32-bit words, regions aligned to 256 bytes (the
//! unit of the BVH descriptor's base address):
//!
//! ```text
//! header (256 bytes)   words: magic "VTRT", version 1, top-level base / 256,
//!                      top-level root pointer, instance count, bottom-level
//!                      count, image size / 256, stack entries needed, the
//!                      reach of ray origins as an f32 (+infinity if tight)
//! bottom level 0..n    each a region of 64-byte slots, its root box node in
//!                      slot 0; a pointer is slot * 8 + type
//! top level            a region like those; its leaves are instance records
//!                      (type 6), two slots each
//! ```
//!
//! A triangle node: words 0-8 the vertices (after the geometry's transform,
//! by [`Transform::apply`]), 9-11 zero, 12 the geometry index with the
//! opaque flag in bit 31, 13 the primitive index, 14 zero, 15 the triangle
//! id 9. One triangle per node, type 0.
//!
//! An instance record: words 0-11 world to object (the `f64` inverse
//! rounded to `f32`), 12 the id with the mask in bits 24-31, 13 the instance
//! flags, 14 the bottom level's base / 256, 15 the instance's index, 16-27
//! object to world, 28 the bottom level's root pointer, 29 its index, 30-31
//! zero.
//!
//! A tree: the leaves in order (triangles by geometry then primitive,
//! instances by index); a node with more than four is split in four runs of
//! as equal length as possible (the first runs one longer) after sorting by
//! centroid along the axis on which the centroids spread widest, ties by
//! leaf order; a run of one is the leaf itself. Unused child slots hold
//! `0xffffffff` and NaN bounds.
//!
//! Padding. The node test computes a box's slab distances in `f32` and has
//! no growth of its own (the emulation accepts descriptors with none), and
//! it decides a triangle by other arithmetic. Where a triangle's edge lies
//! on a face of its box, the two can disagree about a ray that passes within
//! rounding of the edge: the triangle test gives the hit to this triangle
//! and the box test has already turned the ray away from it. With tight
//! boxes the traversal then leaks through shared edges (tests/agreement.rs
//! shows it). Both errors are a few roundings of the distance from the
//! origin to the box, so every box is grown by [`GROWTH`] times the largest
//! such distance its region can see, the region's coordinate magnitude plus
//! the reach of the ray origins the image is built for, and then by one
//! step; an instance's box by that times the condition number of its
//! transform, since the ray reaches the bottom level through an inverse
//! rounded to `f32`. The traversal refuses a ray whose origin is beyond the
//! reach. [`BuildOptions::tight`] writes the tight boxes and no reach.

use crate::node::{INVALID, TRIANGLE_ID_PLAIN, node_type};
use crate::record::Ray;
use crate::scene::{Action, Actions, BottomLevel, Scene, Transform};

/// "VTRT".
pub const MAGIC: u32 = 0x5452_5456;
pub const VERSION: u32 = 1;
/// Words in a 64-byte slot.
pub const SLOT: usize = 16;
/// Words in a 256-byte region unit.
pub const UNIT: usize = 64;
/// The traversal's stack; a scene that needs more is refused.
pub const STACK_DEPTH: usize = 64;
/// The growth of a box relative to the largest distance from a ray origin
/// to it: 2^-20, sixteen unit roundoffs of `f32`. A slab distance carries
/// three roundings of that distance and the triangle test's view of an edge
/// about four; twice their sum.
pub const GROWTH: f32 = 1.0 / 1_048_576.0;
/// The header word that holds the reach.
pub const REACH_WORD: usize = 8;
/// The opaque flag in a triangle node's word 12.
pub const OPAQUE_BIT: u32 = 1 << 31;

type Bounds = Option<([f32; 3], [f32; 3])>;

fn union(a: Bounds, b: Bounds) -> Bounds {
    match (a, b) {
        (Some((al, ah)), Some((bl, bh))) => Some((
            std::array::from_fn(|k| al[k].min(bl[k])),
            std::array::from_fn(|k| ah[k].max(bh[k])),
        )),
        (a, None) => a,
        (None, b) => b,
    }
}

fn magnitude(b: Bounds) -> f32 {
    b.map_or(0.0, |(lo, hi)| {
        lo.iter().chain(&hi).fold(0.0f32, |m, x| m.max(x.abs()))
    })
}

/// The six bound words of a child: grown by `pad` and a step, or tight for
/// `pad` 0; NaN for none.
fn bound_words(b: Bounds, pad: f32) -> [u32; 6] {
    let Some((lo, hi)) = b else {
        return [f32::NAN.to_bits(); 6];
    };
    std::array::from_fn(|j| {
        let x = if j < 3 { lo[j] } else { hi[j - 3] };
        let x = match (pad > 0.0, j < 3) {
            (false, _) => x,
            (true, true) => (x - pad).next_down(),
            (true, false) => (x + pad).next_up(),
        };
        x.to_bits()
    })
}

/// A leaf placed in a region, ready for a tree.
#[derive(Clone, Copy)]
struct Leaf {
    pointer: u32,
    bounds: Bounds,
    /// Stack entries the traversal needs once it has popped this leaf.
    stack: u32,
    order: u32,
}

fn centroid(l: &Leaf) -> [f32; 3] {
    l.bounds.map_or([0.0; 3], |(lo, hi)| {
        std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5)
    })
}

struct Region {
    words: Vec<u32>,
}

/// Rewrites a leaf (its words, its pointer) and gives its new bounds.
type LeafRefit<'a> = dyn FnMut(&mut [u32], u32) -> Result<Bounds, String> + 'a;

impl Region {
    fn alloc(&mut self, slots: usize) -> usize {
        let at = self.words.len() / SLOT;
        self.words.resize(self.words.len() + slots * SLOT, 0);
        at
    }

    /// Writes the box node in `slot` over `leaves` and returns its bounds
    /// and the stack entries the traversal needs once it has popped it.
    fn build(&mut self, slot: usize, leaves: &mut [Leaf], pad: f32) -> (Bounds, u32) {
        let mut children: Vec<(u32, Bounds, u32)> = Vec::new();
        if leaves.len() <= 4 {
            children.extend(leaves.iter().map(|l| (l.pointer, l.bounds, l.stack)));
        } else {
            let cs: Vec<[f32; 3]> = leaves.iter().map(centroid).collect();
            let spread = |k: usize| {
                let lo = cs.iter().fold(f32::INFINITY, |m, c| m.min(c[k]));
                let hi = cs.iter().fold(f32::NEG_INFINITY, |m, c| m.max(c[k]));
                hi - lo
            };
            let mut axis = 0;
            for k in 1..3 {
                if spread(k) > spread(axis) {
                    axis = k;
                }
            }
            leaves.sort_by(|a, b| {
                centroid(a)[axis]
                    .total_cmp(&centroid(b)[axis])
                    .then(a.order.cmp(&b.order))
            });
            let (base, more) = (leaves.len() / 4, leaves.len() % 4);
            let mut rest = leaves;
            for k in 0..4 {
                let (run, tail) = rest.split_at_mut(base + usize::from(k < more));
                rest = tail;
                if let [one] = run {
                    children.push((one.pointer, one.bounds, one.stack));
                } else {
                    let s = self.alloc(2);
                    let (bounds, stack) = self.build(s, run, pad);
                    children.push(((s as u32) << 3 | node_type::BOX32, bounds, stack));
                }
            }
        }
        let at = slot * SLOT;
        let mut all = None;
        let mut deepest = 0;
        for k in 0..4 {
            let (pointer, bounds) = children.get(k).map_or((INVALID, None), |c| (c.0, c.1));
            self.words[at + k] = pointer;
            self.words[at + 4 + 6 * k..at + 10 + 6 * k].copy_from_slice(&bound_words(bounds, pad));
            all = union(all, bounds);
            deepest = deepest.max(children.get(k).map_or(0, |c| c.2));
        }
        // Four pushed; one popped and walked while three wait.
        (all, 4.max(3 + deepest))
    }

    /// Rewrites the bounds below the box node behind `pointer`, the tree
    /// kept; `leaf` rewrites a leaf and gives its new bounds.
    fn refit(
        &mut self,
        pointer: u32,
        pad: f32,
        leaf: &mut LeafRefit<'_>,
    ) -> Result<Bounds, String> {
        if pointer & 7 != node_type::BOX32 {
            let at = (pointer >> 3) as usize * SLOT;
            let words = self
                .words
                .get_mut(at..)
                .ok_or("a pointer past its region")?;
            return leaf(words, pointer);
        }
        let at = (pointer >> 3) as usize * SLOT;
        let mut all = None;
        for k in 0..4 {
            let child = *self.words.get(at + k).ok_or("a pointer past its region")?;
            if child == INVALID {
                continue;
            }
            let bounds = self.refit(child, pad, leaf)?;
            self.words[at + 4 + 6 * k..at + 10 + 6 * k].copy_from_slice(&bound_words(bounds, pad));
            all = union(all, bounds);
        }
        Ok(all)
    }
}

/// What the image is built for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildOptions {
    /// The largest magnitude of a world coordinate of a ray origin the
    /// boxes are padded for; `None` for tight boxes.
    pub reach: Option<f32>,
}

impl BuildOptions {
    /// Tight boxes, any ray: for showing what padding is for.
    pub fn tight() -> Self {
        BuildOptions { reach: None }
    }

    pub fn reach(reach: f32) -> Self {
        BuildOptions { reach: Some(reach) }
    }

    /// Padded for these rays' origins.
    pub fn for_rays(rays: &[Ray]) -> Self {
        let far = rays
            .iter()
            .flat_map(|r| r.origin)
            .fold(0.0f32, |m, x| m.max(x.abs()));
        BuildOptions::reach(far)
    }
}

fn row_sum(m: &[f64; 12]) -> f64 {
    (0..3)
        .map(|r| m[4 * r].abs() + m[4 * r + 1].abs() + m[4 * r + 2].abs())
        .fold(0.0, f64::max)
}

/// The padding of bottom level `b` with tight bounds `bounds`: [`GROWTH`]
/// times its coordinate magnitude plus the farthest a ray origin can be
/// from its origin in its own space, over the instances that use it.
fn bottom_pad(scene: &Scene, b: usize, bounds: Bounds, options: BuildOptions) -> f32 {
    let Some(reach) = options.reach else {
        return 0.0;
    };
    let mut far = 0.0f64;
    for inst in scene.instances.iter().filter(|i| i.blas as usize == b) {
        let m = &inst.transform.0;
        let shift = m[3].abs().max(m[7].abs()).max(m[11].abs());
        let w = inst.transform.inverse().expect("a validated scene");
        far = far.max(row_sum(&w) * (f64::from(reach) + f64::from(shift)));
    }
    GROWTH * (magnitude(bounds) + (far as f32).next_up())
}

/// The top level's padding: [`GROWTH`] times the scene's world coordinate
/// magnitude plus the reach.
fn top_pad(scene: &Scene, bottoms: &[SwBottom], options: BuildOptions) -> f32 {
    let Some(reach) = options.reach else {
        return 0.0;
    };
    let mut all = None;
    for inst in &scene.instances {
        all = union(
            all,
            moved_bounds(bottoms[inst.blas as usize].bounds, &inst.transform),
        );
    }
    GROWTH * (magnitude(all) + reach)
}

/// Where a bottom level is in the image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwBottom {
    /// The region's first word.
    pub base: usize,
    pub words: usize,
    /// The tight bounds of its triangles in object space; `None` if empty.
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// Stack entries the traversal needs below its root.
    pub stack: u32,
    pub primitives: usize,
    /// What its boxes are grown by; 0 if tight.
    pub pad: f32,
}

/// A scene as an image of console-format nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct SwScene {
    /// The image as 32-bit words; [`SwScene::to_bytes`] is the buffer.
    pub words: Vec<u32>,
    pub bottoms: Vec<SwBottom>,
    /// The top level's first word.
    pub top_base: usize,
    pub top_words: usize,
    /// Stack entries the traversal needs for this scene.
    pub stack: u32,
    pub options: BuildOptions,
    /// The bottom level each instance refers to, for a refit to check.
    instance_bottoms: Vec<u32>,
}

/// A triangle's vertices as the builder stores them.
fn placed(bottom: &BottomLevel, geometry: usize, primitive: usize) -> [[f32; 3]; 3] {
    let geo = &bottom.geometries[geometry];
    let tri = geo.triangle(primitive);
    match &geo.transform {
        Some(m) => tri.map(|v| m.apply(v)),
        None => tri,
    }
}

fn tri_bounds(v: &[[f32; 3]; 3]) -> Bounds {
    Some((
        std::array::from_fn(|k| v[0][k].min(v[1][k]).min(v[2][k])),
        std::array::from_fn(|k| v[0][k].max(v[1][k]).max(v[2][k])),
    ))
}

fn bottom_bounds(bottom: &BottomLevel) -> Bounds {
    let mut all = None;
    for (g, geo) in bottom.geometries.iter().enumerate() {
        for p in 0..geo.primitive_count() {
            all = union(all, tri_bounds(&placed(bottom, g, p)));
        }
    }
    all
}

/// The bounds of `b` under `m`: its eight corners by [`Transform::apply`].
fn moved_bounds(b: Bounds, m: &Transform) -> Bounds {
    let (lo, hi) = b?;
    let mut all = None;
    for corner in 0..8 {
        let p: [f32; 3] = std::array::from_fn(|k| if corner >> k & 1 == 0 { lo[k] } else { hi[k] });
        let q = m.apply(p);
        all = union(all, Some((q, q)));
    }
    all
}

/// An instance's world bounds before the tree's own padding: grown by the
/// top level's padding times the transform's condition number.
fn instance_bounds(b: Bounds, m: &Transform, pad: f32) -> Bounds {
    let (lo, hi) = moved_bounds(b, m)?;
    if pad <= 0.0 {
        return Some((lo, hi));
    }
    let w = m.inverse().expect("a validated scene");
    let kappa = (row_sum(&m.0.map(f64::from)) * row_sum(&w)).max(1.0) as f32;
    let grow = pad * kappa;
    Some((lo.map(|x| x - grow), hi.map(|x| x + grow)))
}

fn instance_words(scene: &Scene, i: usize, bottom: &SwBottom) -> Result<[u32; 2 * SLOT], String> {
    let inst = &scene.instances[i];
    let w = inst
        .transform
        .inverse()
        .ok_or_else(|| format!("instance {i}: a transform that cannot be inverted"))?;
    let mut out = [0u32; 2 * SLOT];
    for k in 0..12 {
        out[k] = (w[k] as f32).to_bits();
        out[16 + k] = inst.transform.0[k].to_bits();
    }
    out[12] = inst.id | u32::from(inst.mask) << 24;
    out[13] = inst.flags;
    out[14] = (bottom.base / UNIT) as u32;
    out[15] = i as u32;
    out[28] = node_type::BOX32;
    out[29] = inst.blas;
    Ok(out)
}

impl SwScene {
    /// Writes `scene` (which must pass [`Scene::validate`]) as an image.
    /// Refuses a scene whose trees need more than [`STACK_DEPTH`] stack
    /// entries or whose pointers would not fit.
    pub fn build(scene: &Scene, options: BuildOptions) -> Result<SwScene, String> {
        SwScene::build_within(scene, options, STACK_DEPTH)
    }

    /// [`SwScene::build`] for a traversal with `stack_depth` stack entries:
    /// the limit as a parameter, so that a test can reach the refusal with
    /// a scene it can hold (the real limit needs trees some twenty levels
    /// deep).
    fn build_within(
        scene: &Scene,
        options: BuildOptions,
        stack_depth: usize,
    ) -> Result<SwScene, String> {
        scene.validate()?;
        let mut words = vec![0u32; UNIT];
        let mut bottoms = Vec::new();
        for (b, bottom) in scene.bottoms.iter().enumerate() {
            let bounds = bottom_bounds(bottom);
            let pad = bottom_pad(scene, b, bounds, options);
            let mut region = Region { words: Vec::new() };
            let root = region.alloc(2);
            let mut leaves = Vec::new();
            for (g, geo) in bottom.geometries.iter().enumerate() {
                for p in 0..geo.primitive_count() {
                    let v = placed(bottom, g, p);
                    let slot = region.alloc(1);
                    let at = slot * SLOT;
                    for (k, x) in v.iter().flatten().enumerate() {
                        region.words[at + k] = x.to_bits();
                    }
                    region.words[at + 12] = g as u32 | if geo.opaque { OPAQUE_BIT } else { 0 };
                    region.words[at + 13] = p as u32;
                    region.words[at + 15] = TRIANGLE_ID_PLAIN;
                    leaves.push(Leaf {
                        pointer: (slot as u32) << 3 | node_type::TRIANGLE,
                        bounds: tri_bounds(&v),
                        stack: 0,
                        order: leaves.len() as u32,
                    });
                }
            }
            let primitives = leaves.len();
            let (_, stack) = region.build(root, &mut leaves, pad);
            if region.words.len() / SLOT >= 1 << 29 {
                return Err("a bottom level too large for 32-bit pointers".into());
            }
            let base = words.len();
            words.extend_from_slice(&region.words);
            words.resize(words.len().next_multiple_of(UNIT), 0);
            bottoms.push(SwBottom {
                base,
                words: region.words.len(),
                bounds,
                stack,
                primitives,
                pad,
            });
        }

        let top_base = words.len();
        let (region, stack, instance_bottoms) = top_region(scene, &bottoms, options)?;
        let stack = stack.max(1);
        if stack as usize > stack_depth {
            return Err(format!(
                "the scene needs {stack} stack entries; the traversal has {stack_depth}"
            ));
        }
        let top_words = region.words.len();
        words.extend_from_slice(&region.words);
        words.resize(words.len().next_multiple_of(UNIT), 0);
        let header = [
            MAGIC,
            VERSION,
            (top_base / UNIT) as u32,
            node_type::BOX32,
            scene.instances.len() as u32,
            scene.bottoms.len() as u32,
            (words.len() / UNIT) as u32,
            stack,
            options.reach.unwrap_or(f32::INFINITY).to_bits(),
        ];
        words[..header.len()].copy_from_slice(&header);
        Ok(SwScene {
            words,
            bottoms,
            top_base,
            top_words,
            stack,
            options,
            instance_bottoms,
        })
    }

    /// Brings the image to `scene` after a frame that asked for `actions`
    /// ([`crate::scene::Frame::apply`]): refits where every structure can
    /// be kept, a build of the whole image where one cannot. A bottom level
    /// whose instances moved is refitted too if its padding changes.
    pub fn update(&mut self, scene: &Scene, actions: &Actions) -> Result<(), String> {
        if actions.top == Action::Rebuild || actions.bottoms.contains(&Action::Rebuild) {
            *self = SwScene::build(scene, self.options)?;
            return Ok(());
        }
        let mut any = actions.top == Action::Refit;
        for (b, action) in actions.bottoms.iter().enumerate() {
            let pad = bottom_pad(scene, b, bottom_bounds(&scene.bottoms[b]), self.options);
            if *action == Action::Refit || pad != self.bottoms[b].pad {
                self.refit_bottom(b, scene)?;
                any = true;
            }
        }
        if any {
            self.refit_top(scene)?;
        }
        Ok(())
    }

    /// The image as the bytes of a buffer.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// An update in place of bottom level `b` of `scene` after its vertices
    /// moved: the tree is kept, the triangles and every box above them are
    /// rewritten. Refuses a different primitive count; a change of topology
    /// needs [`SwScene::build`].
    pub fn refit_bottom(&mut self, b: usize, scene: &Scene) -> Result<(), String> {
        let at = *self.bottoms.get(b).ok_or("no such bottom level")?;
        let bottom = scene.bottoms.get(b).ok_or("no such bottom level")?;
        let mut seen = 0usize;
        let bounds = bottom_bounds(bottom);
        let pad = bottom_pad(scene, b, bounds, self.options);
        let mut region = Region {
            words: self.words[at.base..at.base + at.words].to_vec(),
        };
        region.refit(node_type::BOX32, pad, &mut |w, _| {
            let (g, p) = ((w[12] & !OPAQUE_BIT) as usize, w[13] as usize);
            let geo = bottom
                .geometries
                .get(g)
                .ok_or("a refit with fewer geometries")?;
            if p >= geo.primitive_count() || (w[12] & OPAQUE_BIT != 0) != geo.opaque {
                return Err("a refit that changes more than positions".into());
            }
            let v = placed(bottom, g, p);
            for (k, x) in v.iter().flatten().enumerate() {
                w[k] = x.to_bits();
            }
            seen += 1;
            Ok(tri_bounds(&v))
        })?;
        if seen != bottom.primitive_count() || seen != at.primitives {
            return Err("a refit with a different primitive count".into());
        }
        self.words[at.base..at.base + at.words].copy_from_slice(&region.words);
        self.bottoms[b].bounds = bounds;
        self.bottoms[b].pad = pad;
        Ok(())
    }

    /// An update in place of the top level after instance transforms or
    /// bottom-level bounds changed: the tree is kept, the records and the
    /// boxes are rewritten. Refuses a different instance list.
    pub fn refit_top(&mut self, scene: &Scene) -> Result<(), String> {
        let bottoms: Vec<u32> = scene.instances.iter().map(|i| i.blas).collect();
        if bottoms != self.instance_bottoms {
            return Err("a top-level refit with a different instance list".into());
        }
        let pad = top_pad(scene, &self.bottoms, self.options);
        let mut region = Region {
            words: self.words[self.top_base..self.top_base + self.top_words].to_vec(),
        };
        let swb = &self.bottoms;
        region.refit(node_type::BOX32, pad, &mut |w, _| {
            let i = w[15] as usize;
            let inst = scene.instances.get(i).ok_or("no such instance")?;
            let bottom = &swb[inst.blas as usize];
            w[..2 * SLOT].copy_from_slice(&instance_words(scene, i, bottom)?);
            Ok(instance_bounds(bottom.bounds, &inst.transform, pad))
        })?;
        self.words[self.top_base..self.top_base + self.top_words].copy_from_slice(&region.words);
        Ok(())
    }
}

fn top_region(
    scene: &Scene,
    bottoms: &[SwBottom],
    options: BuildOptions,
) -> Result<(Region, u32, Vec<u32>), String> {
    let pad = top_pad(scene, bottoms, options);
    let mut region = Region { words: Vec::new() };
    let root = region.alloc(2);
    let mut leaves = Vec::new();
    for (i, inst) in scene.instances.iter().enumerate() {
        let bottom = &bottoms[inst.blas as usize];
        let slot = region.alloc(2);
        region.words[slot * SLOT..(slot + 2) * SLOT]
            .copy_from_slice(&instance_words(scene, i, bottom)?);
        leaves.push(Leaf {
            pointer: (slot as u32) << 3 | node_type::INSTANCE,
            bounds: instance_bounds(bottom.bounds, &inst.transform, pad),
            // The return marker and the root pushed; the root popped.
            stack: 2.max(1 + bottom.stack),
            order: i as u32,
        });
    }
    let (_, stack) = region.build(root, &mut leaves, pad);
    Ok((
        region,
        stack,
        scene.instances.iter().map(|i| i.blas).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{BuildFlags, Geometry, Indices, Instance};

    fn quad(z: f32, opaque: bool) -> Geometry {
        Geometry {
            vertices: vec![[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z], [1.0, 1.0, z]],
            indices: Indices::U16(vec![0, 1, 2, 2, 1, 3]),
            transform: None,
            opaque,
        }
    }

    fn scene(quads: usize, instances: usize) -> Scene {
        Scene {
            bottoms: vec![BottomLevel {
                geometries: (0..quads).map(|k| quad(k as f32, k % 2 == 0)).collect(),
                flags: BuildFlags::default(),
            }],
            instances: (0..instances)
                .map(|k| Instance {
                    transform: Transform::translation([4.0 * k as f32, 0.0, 0.0]),
                    id: 100 + k as u32,
                    mask: 0xF0,
                    flags: 2,
                    blas: 0,
                })
                .collect(),
            flags: BuildFlags::default(),
        }
    }

    #[test]
    fn the_image_has_the_documented_layout() {
        let sw = SwScene::build(&scene(1, 1), BuildOptions::tight()).unwrap();
        let bytes = sw.to_bytes();
        assert_eq!(&bytes[0..4], b"VTRT");
        assert_eq!(bytes.len() % 256, 0);
        // Header, then the bottom level: a root (2 slots) and two triangles.
        let b = sw.bottoms[0];
        assert_eq!((b.base, b.words, b.primitives), (UNIT, 4 * SLOT, 2));
        let root = &sw.words[b.base..];
        assert_eq!(&root[..4], &[2 << 3, 3 << 3, INVALID, INVALID]);
        // The first child's tight box: (0, 0, 0) to (1, 1, 0).
        let floats: Vec<f32> = root[4..10].iter().map(|w| f32::from_bits(*w)).collect();
        assert_eq!(floats, [0.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
        assert!(f32::from_bits(root[16]).is_nan());
        // The second triangle: vertices 2, 1, 3; geometry 0, opaque; id 9.
        let tri = &root[3 * SLOT..4 * SLOT];
        assert_eq!(f32::from_bits(tri[1]), 1.0);
        assert_eq!(f32::from_bits(tri[3]), 1.0);
        assert_eq!((tri[12], tri[13], tri[15]), (OPAQUE_BIT, 1, 9));
        // The top level: a root and one record.
        assert_eq!(sw.top_base, 2 * UNIT);
        let top = &sw.words[sw.top_base..];
        assert_eq!(top[0], 2 << 3 | 6);
        let rec = &top[2 * SLOT..4 * SLOT];
        assert_eq!(rec[12], 100 | 0xF0 << 24);
        assert_eq!(
            (rec[13], rec[14], rec[15], rec[28], rec[29]),
            (2, 1, 0, 5, 0)
        );
        assert_eq!(f32::from_bits(rec[16]), 1.0);
        assert_eq!(&sw.words[..8], &[MAGIC, 1, 2, 5, 1, 1, 3, sw.stack]);
        assert_eq!(f32::from_bits(sw.words[REACH_WORD]), f32::INFINITY);
        let padded = SwScene::build(&scene(1, 1), BuildOptions::reach(64.0)).unwrap();
        assert_eq!(f32::from_bits(padded.words[REACH_WORD]), 64.0);
        // The squares reach to 1 and rays start within 64 (rounded up).
        assert_eq!(padded.bottoms[0].pad, GROWTH * (1.0 + 64.0f32.next_up()));
        // The top root's four slots pushed and the instance popped while
        // three wait (3); its marker pushed and its root pushed and popped
        // (1); that root's four pushed (4).
        assert_eq!(sw.stack, 3 + 1 + 4);
    }

    #[test]
    fn padding_grows_every_box_outward() {
        let tight = SwScene::build(&scene(3, 2), BuildOptions::tight()).unwrap();
        let padded = SwScene::build(&scene(3, 2), BuildOptions::reach(64.0)).unwrap();
        assert_eq!(tight.words.len(), padded.words.len());
        let (b, mut grown) = (tight.bottoms[0], 0);
        for k in 0..4 {
            for j in 0..6 {
                let at = b.base + 4 + 6 * k + j;
                let (t, p) = (
                    f32::from_bits(tight.words[at]),
                    f32::from_bits(padded.words[at]),
                );
                if t.is_nan() {
                    assert!(p.is_nan());
                    continue;
                }
                assert!(if j < 3 { p < t } else { p > t }, "{t} {p}");
                assert!((p - t).abs() < 1e-3);
                grown += 1;
            }
        }
        assert!(grown >= 12);
    }

    #[test]
    fn more_than_four_leaves_split_into_runs() {
        // 9 quads, 18 triangles, sorted along z: runs of 5, 5, 4, 4.
        let sw = SwScene::build(&scene(9, 1), BuildOptions::reach(64.0)).unwrap();
        let b = sw.bottoms[0];
        let root = &sw.words[b.base..b.base + 2 * SLOT];
        let mut counts = Vec::new();
        for &child in &root[..4] {
            assert_eq!(child & 7, 5);
            let node = &sw.words[b.base + (child >> 3) as usize * SLOT..];
            let inner: Vec<u32> = node[..4]
                .iter()
                .copied()
                .filter(|&c| c != INVALID)
                .collect();
            let mut n = 0;
            for c in inner {
                if c & 7 == 5 {
                    let leaf = &sw.words[b.base + (c >> 3) as usize * SLOT..];
                    n += leaf[..4].iter().filter(|&&c| c != INVALID).count();
                } else {
                    n += 1;
                }
            }
            counts.push(n);
        }
        assert_eq!(counts, [5, 5, 4, 4]);
        // The root, four boxes below it, and a box for the pair in each run
        // of five.
        assert_eq!(b.words, (18 + 2 * (1 + 4 + 2)) * SLOT);
    }

    #[test]
    fn a_refit_equals_a_build_of_the_moved_scene() {
        let mut s = scene(9, 6);
        let mut sw = SwScene::build(&s, BuildOptions::reach(64.0)).unwrap();
        for geo in &mut s.bottoms[0].geometries {
            for v in &mut geo.vertices {
                v[0] += 0.25 * v[2];
            }
        }
        s.instances[3].transform = Transform::translation([-7.0, 2.0, 1.0]);
        sw.refit_bottom(0, &s).unwrap();
        sw.refit_top(&s).unwrap();
        // The motion keeps every centroid's order along z and the instances'
        // along x apart from one, so only the top tree may differ: compare
        // the bottom level with a fresh build, and the records.
        let fresh = SwScene::build(&s, BuildOptions::reach(64.0)).unwrap();
        let b = sw.bottoms[0];
        assert_eq!(
            sw.words[b.base..b.base + b.words],
            fresh.words[b.base..b.base + b.words]
        );
        assert_eq!(sw.bottoms, fresh.bottoms);
        let record = |sw: &SwScene, i: u32| -> Vec<u32> {
            let top = &sw.words[sw.top_base..sw.top_base + sw.top_words];
            let at = (0..top.len() / SLOT)
                .find(|&s| {
                    top[s * SLOT + 15] == i && top[s * SLOT + 28] == 5 && top[s * SLOT + 14] == 1
                })
                .unwrap();
            top[at * SLOT..(at + 2) * SLOT].to_vec()
        };
        assert_eq!(record(&sw, 3), record(&fresh, 3));
        // A change of topology is refused.
        s.bottoms[0].geometries.pop();
        assert!(sw.refit_bottom(0, &s).is_err());
        s.instances.pop();
        assert!(sw.refit_top(&s).is_err());
    }

    #[test]
    fn the_stack_a_scene_needs_is_counted_level_by_level() {
        let sw = SwScene::build(&scene(40, 70), BuildOptions::reach(64.0)).unwrap();
        assert!(sw.stack as usize <= STACK_DEPTH);
        // 80 triangles: runs of 20, of 5, and a pair, four levels of boxes.
        // 70 instances: runs of 18 or 17, of 5 or 4, and a pair.
        assert_eq!(sw.bottoms[0].stack, 3 + 3 + 3 + 4);
        assert_eq!(sw.stack, 3 + 3 + 3 + 3 + (1 + 13));
        assert_eq!(sw.words[7], 26);
    }

    #[test]
    fn a_scene_that_needs_more_stack_than_the_traversal_has_is_refused() {
        // The real limit of 64 needs trees too deep for a test to hold, so
        // the limit is lowered to this scene's need, 26, and below it.
        let options = BuildOptions::reach(64.0);
        let fits = SwScene::build_within(&scene(40, 70), options, 26).unwrap();
        assert_eq!(fits, SwScene::build(&scene(40, 70), options).unwrap());
        assert_eq!(
            SwScene::build_within(&scene(40, 70), options, 25).unwrap_err(),
            "the scene needs 26 stack entries; the traversal has 25"
        );
        // A smaller scene still fits the lowered limit, and one entry less
        // than it needs refuses it too.
        let small = SwScene::build_within(&scene(1, 1), options, 8).unwrap();
        assert_eq!(small.stack, 8);
        assert!(SwScene::build_within(&scene(1, 1), options, 7).is_err());
        // An update that has to build again keeps the real limit.
        let mut sw = fits;
        let actions = Actions {
            bottoms: vec![Action::Rebuild],
            top: Action::Rebuild,
        };
        sw.update(&scene(40, 70), &actions).unwrap();
        assert_eq!(sw.stack, 26);
    }
}
