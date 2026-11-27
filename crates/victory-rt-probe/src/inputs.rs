//! A scene as what a host build reads: the bytes of vertex, index,
//! transform and box buffers and the descriptions that name them
//! (`victory_gpu::rt::desc`), for the native path (triangles) and for the
//! hybrid path (one box around each triangle, and the triangles as the
//! procedural shader reads them). No device is needed for any of it.
//!
//! A bottom level's bytes are in two buffers. **Fixed** holds what a refit
//! may not move: indices and geometry transforms (the backend takes other
//! index memory as another shape, `victory_gpu::rt::update`). **Positions**
//! holds the vertices, three floats each, and is what a frame that moves
//! vertices uploads again. The hybrid path has one buffer of boxes per
//! bottom level, uploaded again with every change, and one buffer of
//! triangle data for the scene.

use victory_gpu::rt::desc::{
    BOX_BYTES, BottomDesc, Boxes, BuildFlags, Geometry, GeometryFlags, IndexFormat, Indices,
    Inputs, Prefer, TRANSFORM_ALIGN, Triangles, VertexFormat,
};

use crate::scene;

/// Bytes of a vertex position: three floats.
pub const VERTEX_BYTES: u64 = 12;
/// Bytes of a triangle in the hybrid path's triangle data: nine floats.
pub const TRIANGLE_BYTES: u64 = 36;

fn floats(out: &mut Vec<u8>, values: &[f32]) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

/// Zero bytes up to the next multiple of `align`; the offset reached.
fn pad(out: &mut Vec<u8>, align: u64) -> u64 {
    while !(out.len() as u64).is_multiple_of(align) {
        out.push(0);
    }
    out.len() as u64
}

/// The build flags of a structure: allowed to update or not, and what it
/// prefers. Compaction is not asked for: the probe does not compact.
pub fn build_flags(allow_update: bool, prefer: scene::Prefer) -> BuildFlags {
    BuildFlags {
        allow_update,
        allow_compaction: false,
        prefer: match prefer {
            scene::Prefer::Neither => Prefer::Neither,
            scene::Prefer::FastTrace => Prefer::FastTrace,
            scene::Prefer::FastBuild => Prefer::FastBuild,
        },
        minimize_memory: false,
    }
}

/// A geometry's flags: opaque as the scene says; one that is not opaque
/// is built so that a candidate is offered once a ray
/// (docs/dxr-architecture.md, "Structure construction").
pub fn geometry_flags(opaque: bool) -> GeometryFlags {
    GeometryFlags { opaque, no_duplicate_any_hit: !opaque }
}

/// Where one geometry's triangles lie in a bottom level's two buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriangleGeometry {
    /// In the positions buffer.
    pub vertex_offset: u64,
    pub vertex_count: u32,
    /// In the fixed buffer: offset, width and count.
    pub indices: Option<(u64, IndexFormat, u32)>,
    /// In the fixed buffer.
    pub transform: Option<u64>,
    pub flags: GeometryFlags,
}

/// A bottom level of triangles, for the native path.
#[derive(Clone, Debug, PartialEq)]
pub struct TriangleInputs {
    pub fixed: Vec<u8>,
    pub positions: Vec<u8>,
    pub geometries: Vec<TriangleGeometry>,
    pub flags: BuildFlags,
}

impl TriangleInputs {
    pub fn of(bottom: &scene::Bottom) -> TriangleInputs {
        let (mut fixed, mut positions) = (Vec::new(), Vec::new());
        let mut geometries = Vec::new();
        for geometry in &bottom.geometries {
            let vertex_offset = positions.len() as u64;
            geometry.vertices.iter().for_each(|v| floats(&mut positions, v));
            let indices = match &geometry.indices {
                scene::Indices::None => None,
                scene::Indices::U16(list) => {
                    let at = pad(&mut fixed, 2);
                    list.iter().for_each(|i| fixed.extend_from_slice(&i.to_le_bytes()));
                    Some((at, IndexFormat::U16, list.len() as u32))
                }
                scene::Indices::U32(list) => {
                    let at = pad(&mut fixed, 4);
                    list.iter().for_each(|i| fixed.extend_from_slice(&i.to_le_bytes()));
                    Some((at, IndexFormat::U32, list.len() as u32))
                }
            };
            let transform = geometry.transform.as_ref().map(|m| {
                let at = pad(&mut fixed, TRANSFORM_ALIGN);
                floats(&mut fixed, m);
                at
            });
            geometries.push(TriangleGeometry {
                vertex_offset,
                vertex_count: geometry.vertices.len() as u32,
                indices,
                transform,
                flags: geometry_flags(geometry.opaque),
            });
        }
        TriangleInputs {
            fixed,
            positions,
            geometries,
            flags: build_flags(bottom.allow_update, bottom.prefer),
        }
    }

    /// The build description, with the two buffers at these host
    /// addresses (`fixed` is not read where the level has no indices and
    /// no transform).
    pub fn desc(&self, fixed: u64, positions: u64) -> BottomDesc {
        let geometries = self
            .geometries
            .iter()
            .map(|g| {
                Geometry::Triangles(Triangles {
                    vertices: positions + g.vertex_offset,
                    vertex_stride: VERTEX_BYTES,
                    vertex_count: g.vertex_count,
                    vertex_format: VertexFormat::Float3,
                    indices: g.indices.map(|(offset, format, count)| Indices {
                        address: fixed + offset,
                        format,
                        count,
                    }),
                    transform: g.transform.map(|offset| fixed + offset),
                    flags: g.flags,
                })
            })
            .collect();
        BottomDesc { geometries, flags: self.flags, inputs: Inputs::Ready }
    }
}

/// Where one geometry's boxes lie in a bottom level's box buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxGeometry {
    pub offset: u64,
    pub count: u64,
    pub flags: GeometryFlags,
}

/// A bottom level of boxes, one around each triangle in its place, for
/// the hybrid path. A box is the least and the greatest of its triangle's
/// three vertices on each axis, in the bottom level's object space, with
/// nothing added: whether a device presents the candidate of a box with
/// no thickness, or of one a ray grazes, is among what the hybrid path
/// asks of it, and [`BoxInputs::flat`] counts the boxes of the first kind.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxInputs {
    pub boxes: Vec<u8>,
    pub geometries: Vec<BoxGeometry>,
    pub flags: BuildFlags,
    /// Boxes with no extent on at least one axis.
    pub flat: u32,
}

impl BoxInputs {
    pub fn of(bottom: &scene::Bottom) -> BoxInputs {
        let mut boxes = Vec::new();
        let mut geometries = Vec::new();
        let mut flat = 0;
        for geometry in &bottom.geometries {
            let offset = boxes.len() as u64;
            let count = geometry.primitive_count();
            for k in 0..count {
                let [a, b, c] = geometry.object_triangle(k);
                let least: [f32; 3] = std::array::from_fn(|x| a[x].min(b[x]).min(c[x]));
                let greatest: [f32; 3] = std::array::from_fn(|x| a[x].max(b[x]).max(c[x]));
                flat += u32::from((0..3).any(|x| least[x] == greatest[x]));
                floats(&mut boxes, &least);
                floats(&mut boxes, &greatest);
            }
            geometries.push(BoxGeometry {
                offset,
                count: count as u64,
                flags: geometry_flags(geometry.opaque),
            });
        }
        BoxInputs { boxes, geometries, flags: build_flags(bottom.allow_update, bottom.prefer), flat }
    }

    /// The build description, with the box buffer at this host address.
    pub fn desc(&self, boxes: u64) -> BottomDesc {
        let geometries = self
            .geometries
            .iter()
            .map(|g| {
                Geometry::Boxes(Boxes {
                    address: boxes + g.offset,
                    stride: BOX_BYTES,
                    count: g.count,
                    flags: g.flags,
                })
            })
            .collect();
        BottomDesc { geometries, flags: self.flags, inputs: Inputs::Ready }
    }
}

/// The hybrid path's triangle data for a scene, as the procedural shader
/// reads it (`victory_shader::emit::rayquery::Resource::Triangles`),
/// offsets in bytes from its start: for instance `i` two dwords at
/// `8 * i`, the offset of its bottom level's geometry table and its
/// instance flags; in a geometry table a dword per geometry, the offset of
/// its triangle 0; a triangle is nine floats in the object space its box
/// was made in. Instances of one bottom level share its table. Refused
/// where an offset does not fit 32 bits.
pub fn triangle_data(scene: &scene::Scene) -> Result<Vec<u8>, String> {
    let instance_table = 8 * scene.instances.len() as u64;
    // Each bottom level's geometry table, then every triangle.
    let mut tables = Vec::with_capacity(scene.bottoms.len());
    let mut at = instance_table;
    for bottom in &scene.bottoms {
        tables.push(at);
        at += 4 * bottom.geometries.len() as u64;
    }
    let total = at + TRIANGLE_BYTES * scene.primitives() as u64;
    if total > u64::from(u32::MAX) {
        return Err(format!(
            "the triangle data would be {total} bytes: its offsets are 32 bits wide"
        ));
    }
    let mut out = Vec::with_capacity(total as usize);
    for instance in &scene.instances {
        let table = tables.get(instance.bottom as usize).ok_or_else(|| {
            format!("an instance of bottom level {}, which the scene does not have", instance.bottom)
        })?;
        out.extend_from_slice(&(*table as u32).to_le_bytes());
        out.extend_from_slice(&instance.flags.to_le_bytes());
    }
    for bottom in &scene.bottoms {
        for geometry in &bottom.geometries {
            out.extend_from_slice(&(at as u32).to_le_bytes());
            at += TRIANGLE_BYTES * geometry.primitive_count() as u64;
        }
    }
    for geometry in scene.bottoms.iter().flat_map(|b| &b.geometries) {
        for k in 0..geometry.primitive_count() {
            geometry.object_triangle(k).iter().for_each(|v| floats(&mut out, v));
        }
    }
    debug_assert_eq!(out.len() as u64, total);
    Ok(out)
}

/// One instance of a top level, its bottom level named by its place in the
/// scene: the device layer puts the structure built for it in that place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TopInstance {
    pub transform: [f32; 12],
    /// 24 bits.
    pub id: u32,
    pub mask: u8,
    /// `D3D12_RAYTRACING_INSTANCE_FLAGS`: four bits.
    pub flags: u8,
    pub bottom: u32,
}

/// The scene's instances in their order, which is the instance index a
/// hit reports. Refused where a field does not fit what D3D12 reads.
pub fn instances(scene: &scene::Scene) -> Result<Vec<TopInstance>, String> {
    scene
        .instances
        .iter()
        .enumerate()
        .map(|(i, instance)| {
            if instance.id >> 24 != 0 {
                return Err(format!("instance {i}: an id over 24 bits"));
            }
            let flags = u8::try_from(instance.flags)
                .ok()
                .filter(|flags| flags & !0xF == 0)
                .ok_or_else(|| format!("instance {i}: flags D3D12 does not define"))?;
            if instance.bottom as usize >= scene.bottoms.len() {
                return Err(format!("instance {i}: no bottom level {}", instance.bottom));
            }
            Ok(TopInstance {
                transform: instance.transform,
                id: instance.id,
                mask: instance.mask,
                flags,
                bottom: instance.bottom,
            })
        })
        .collect()
}

/// One bottom level's bytes for one path.
#[derive(Clone, Debug, PartialEq)]
pub enum BottomInputs {
    Triangles(TriangleInputs),
    Boxes(BoxInputs),
}

impl BottomInputs {
    fn flags(&self) -> BuildFlags {
        match self {
            BottomInputs::Triangles(t) => t.flags,
            BottomInputs::Boxes(b) => b.flags,
        }
    }

    /// Whether `self` can come of `built` by a refit: built to allow one,
    /// the same geometries in the same places, and for triangles the same
    /// indices and transforms byte for byte, so that only positions (or
    /// boxes) differ.
    fn refits(&self, built: &BottomInputs) -> bool {
        self.flags().allow_update
            && match (self, built) {
                (BottomInputs::Triangles(a), BottomInputs::Triangles(b)) => {
                    a.flags == b.flags && a.geometries == b.geometries && a.fixed == b.fixed
                }
                (BottomInputs::Boxes(a), BottomInputs::Boxes(b)) => {
                    a.flags == b.flags && a.geometries == b.geometries
                }
                _ => false,
            }
    }

    /// The bytes a build of it reads.
    pub fn bytes(&self) -> u64 {
        match self {
            BottomInputs::Triangles(t) => (t.fixed.len() + t.positions.len()) as u64,
            BottomInputs::Boxes(b) => b.boxes.len() as u64,
        }
    }
}

/// Everything one path's builds and shader read of a scene.
#[derive(Clone, Debug, PartialEq)]
pub struct PathInputs {
    pub bottoms: Vec<BottomInputs>,
    pub instances: Vec<TopInstance>,
    pub top_flags: BuildFlags,
    /// The hybrid path's triangle data; `None` for the native path.
    pub triangle_data: Option<Vec<u8>>,
    /// Boxes with no extent on an axis, over the bottom levels (hybrid).
    pub flat_boxes: u32,
}

impl PathInputs {
    /// Triangles, for the native path.
    pub fn triangles(scene: &scene::Scene) -> Result<PathInputs, String> {
        Ok(PathInputs {
            bottoms: scene
                .bottoms
                .iter()
                .map(|b| BottomInputs::Triangles(TriangleInputs::of(b)))
                .collect(),
            instances: instances(scene)?,
            top_flags: build_flags(scene.allow_update, scene.prefer),
            triangle_data: None,
            flat_boxes: 0,
        })
    }

    /// One box a triangle and the triangle data, for the hybrid path.
    pub fn boxes(scene: &scene::Scene) -> Result<PathInputs, String> {
        let boxes: Vec<BoxInputs> = scene.bottoms.iter().map(BoxInputs::of).collect();
        Ok(PathInputs {
            flat_boxes: boxes.iter().map(|b| b.flat).sum(),
            bottoms: boxes.into_iter().map(BottomInputs::Boxes).collect(),
            instances: instances(scene)?,
            top_flags: build_flags(scene.allow_update, scene.prefer),
            triangle_data: Some(triangle_data(scene)?),
        })
    }
}

/// What is done to each structure of a path, and where that is not what
/// the frame asked, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Steps {
    pub bottoms: Vec<scene::Action>,
    pub top: scene::Action,
    /// The triangle data is uploaded again.
    pub triangle_data: bool,
    pub notes: Vec<String>,
}

/// The steps that bring a path's structures from `built` (what the device
/// holds; `None` before the first build) to `next`, where the frame asks
/// `asked`. What the frame asks is followed where the inputs allow it and
/// corrected where they do not, each correction with a note: nothing built
/// yet is built; a structure asked to be kept whose inputs differ is built
/// (it would be stale); a refit of more than positions, or of a structure
/// not built for updates, is a build; a top level kept over a bottom level
/// that changed is refitted (the backend makes that a build where an
/// address moved).
pub fn steps(built: Option<&PathInputs>, next: &PathInputs, asked: &scene::Actions) -> Steps {
    use scene::Action::{Keep, Rebuild, Refit};
    let mut notes = Vec::new();
    let Some(built) = built.filter(|b| b.bottoms.len() == next.bottoms.len()) else {
        return Steps {
            bottoms: vec![Rebuild; next.bottoms.len()],
            top: Rebuild,
            triangle_data: next.triangle_data.is_some(),
            notes,
        };
    };
    let bottoms: Vec<scene::Action> = next
        .bottoms
        .iter()
        .zip(&built.bottoms)
        .enumerate()
        .map(|(i, (next, built))| match asked.bottoms.get(i).copied().unwrap_or(Rebuild) {
            Keep if next == built => Keep,
            Keep => {
                notes.push(format!("bottom level {i}: asked to be kept with other inputs; built"));
                Rebuild
            }
            Refit if next.refits(built) => Refit,
            Refit => {
                notes.push(format!("bottom level {i}: a refit of more than positions; built"));
                Rebuild
            }
            Rebuild => Rebuild,
        })
        .collect();
    let same_top = next.instances == built.instances && next.top_flags == built.top_flags;
    let touched = bottoms.iter().any(|a| *a != Keep);
    let refits = next.top_flags.allow_update
        && next.top_flags == built.top_flags
        && next.instances.len() == built.instances.len()
        && next.instances.iter().zip(&built.instances).all(|(a, b)| a.bottom == b.bottom);
    let top = match asked.top {
        Keep if same_top && !touched => Keep,
        Keep if same_top => {
            notes.push("top level: asked to be kept over a bottom level that changed; refitted".into());
            if refits { Refit } else { Rebuild }
        }
        Keep => {
            notes.push("top level: asked to be kept with other instances; built".into());
            Rebuild
        }
        Refit if refits => Refit,
        Refit => {
            notes.push("top level: a refit of more than its instances' fields; built".into());
            Rebuild
        }
        Rebuild => Rebuild,
    };
    Steps { bottoms, top, triangle_data: next.triangle_data != built.triangle_data, notes }
}

/// What a frame would have to ask to go from `built` to `next` with no
/// more known than the two: a structure whose inputs are the same is
/// kept, any other is built. For a loop's return from a case's last frame
/// to its first, which no frame of the case describes.
pub fn actions_between(built: &PathInputs, next: &PathInputs) -> scene::Actions {
    use scene::Action::{Keep, Rebuild};
    if built.bottoms.len() != next.bottoms.len() {
        return scene::Actions::build_all(next.bottoms.len());
    }
    let bottoms: Vec<scene::Action> =
        next.bottoms.iter().zip(&built.bottoms).map(|(a, b)| if a == b { Keep } else { Rebuild }).collect();
    let same = next.instances == built.instances && next.top_flags == built.top_flags;
    let top = if same && bottoms.iter().all(|a| *a == Keep) { Keep } else { Rebuild };
    scene::Actions { bottoms, top }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Bottom, Geometry as SceneGeometry, Indices as SceneIndices, Instance, Scene};
    use victory_gpu::rt::desc;

    fn f(bytes: &[u8], at: usize) -> f32 {
        f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn d(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    /// Two geometries: three loose triangles' worth of vertices without
    /// indices and not opaque; a quad with 16-bit indices, opaque, with a
    /// transform that moves it by (0, 0, 5).
    fn bottom() -> Bottom {
        let loose = SceneGeometry {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: SceneIndices::None,
            transform: None,
            opaque: false,
        };
        let quad = SceneGeometry {
            vertices: vec![[0.0, 0.0, 1.0], [2.0, 0.0, 1.0], [2.0, 2.0, 1.0], [0.0, 2.0, 3.0]],
            indices: SceneIndices::U16(vec![0, 1, 2, 0, 2, 3]),
            transform: Some([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 5.0]),
            opaque: true,
        };
        Bottom { geometries: vec![loose, quad], allow_update: true, prefer: scene::Prefer::FastBuild }
    }

    #[test]
    fn triangles_lie_in_two_buffers_and_the_description_names_them() {
        let inputs = TriangleInputs::of(&bottom());
        // Seven vertices of twelve bytes; six 16-bit indices, then the
        // transform at the next multiple of 16.
        assert_eq!(inputs.positions.len(), 7 * 12);
        assert_eq!(inputs.fixed.len(), 16 + 48);
        assert_eq!(f(&inputs.positions, 12), 1.0);
        assert_eq!(f(&inputs.positions, 3 * 12 + 2 * 12 + 4), 2.0, "vertex 2 of the quad, y");
        assert_eq!(u16::from_le_bytes([inputs.fixed[10], inputs.fixed[11]]), 3);
        assert_eq!(f(&inputs.fixed, 16 + 44), 5.0, "the transform's last number");

        let built = inputs.desc(0x4_0000, 0x8_0000);
        assert_eq!(built.check(), Ok(3), "D3D12's rules, and three primitives");
        assert_eq!(built.flags.bits(), 1 | 8, "allow update, prefer fast build");
        let [desc::Geometry::Triangles(loose), desc::Geometry::Triangles(quad)] =
            built.geometries[..]
        else {
            panic!("two triangle geometries")
        };
        assert_eq!((loose.vertices, loose.vertex_count, loose.vertex_stride), (0x8_0000, 3, 12));
        assert_eq!((loose.indices, loose.transform), (None, None));
        assert_eq!(loose.flags.bits(), 2, "not opaque: offered once a ray");
        assert_eq!((quad.vertices, quad.vertex_count), (0x8_0000 + 36, 4));
        let indices = quad.indices.unwrap();
        assert_eq!((indices.address, indices.count, indices.format), (0x4_0000, 6, IndexFormat::U16));
        assert_eq!(quad.transform, Some(0x4_0000 + 16));
        assert_eq!(quad.flags.bits(), 1);
        // The build reads inside what was written.
        assert!(loose.vertex_bytes().unwrap() + quad.vertex_bytes().unwrap() <= 7 * 12);
        assert!(indices.bytes() <= 16);
    }

    #[test]
    fn thirty_two_bit_indices_and_a_second_transform_keep_their_alignment() {
        let mut level = bottom();
        level.geometries[1].indices = SceneIndices::U32(vec![0, 1, 2, 0, 2, 3]);
        level.geometries.push(SceneGeometry {
            vertices: vec![[0.0; 3]; 3],
            indices: SceneIndices::U16(vec![0, 1, 2]),
            transform: Some(desc::IDENTITY),
            opaque: true,
        });
        let inputs = TriangleInputs::of(&level);
        let (first, second) = (inputs.geometries[1], inputs.geometries[2]);
        assert_eq!(first.indices, Some((0, IndexFormat::U32, 6)));
        assert_eq!(first.transform, Some(32));
        assert_eq!(second.indices, Some((80, IndexFormat::U16, 3)));
        assert_eq!(second.transform, Some(96));
        assert_eq!(inputs.desc(0x1_0000, 0x2_0000).check(), Ok(4));
    }

    #[test]
    fn a_box_is_the_least_and_greatest_of_its_triangle_after_the_transform() {
        let inputs = BoxInputs::of(&bottom());
        assert_eq!(inputs.boxes.len(), 3 * 24);
        let floats: Vec<f32> = (0..18).map(|k| f(&inputs.boxes, 4 * k)).collect();
        // The loose triangle lies in z = 0: no thickness there.
        assert_eq!(floats[..6], [0.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
        // The quad's first triangle, moved to z = 6; its second reaches 8.
        assert_eq!(floats[6..12], [0.0, 0.0, 6.0, 2.0, 2.0, 6.0]);
        assert_eq!(floats[12..18], [0.0, 0.0, 6.0, 2.0, 2.0, 8.0]);
        assert_eq!(inputs.flat, 2);
        let built = inputs.desc(0x10_0000);
        assert_eq!(built.check(), Ok(3));
        let [desc::Geometry::Boxes(loose), desc::Geometry::Boxes(quad)] = built.geometries[..]
        else {
            panic!("two box geometries")
        };
        assert_eq!((loose.address, loose.count, loose.stride), (0x10_0000, 1, 24));
        assert_eq!((quad.address, quad.count), (0x10_0000 + 24, 2));
        assert_eq!((loose.flags.bits(), quad.flags.bits()), (2, 1));
    }

    fn scene() -> Scene {
        let level = bottom();
        let other = Bottom {
            geometries: vec![level.geometries[1].clone()],
            allow_update: false,
            prefer: scene::Prefer::FastTrace,
        };
        let instance = |bottom, id, mask, flags| Instance {
            transform: [1.0, 0.0, 0.0, 7.0, 0.0, 1.0, 0.0, 8.0, 0.0, 0.0, 1.0, 9.0],
            id,
            mask,
            flags,
            bottom,
        };
        Scene {
            bottoms: vec![level, other],
            instances: vec![
                instance(1, 0xAB_CDEF, 0x0F, 0x2),
                instance(0, 5, 0xFF, 0x0),
                instance(1, 6, 0x80, 0x9),
            ],
            allow_update: true,
            prefer: scene::Prefer::FastTrace,
        }
    }

    #[test]
    fn the_triangle_data_is_read_as_the_procedural_shader_reads_it() {
        let scene = scene();
        let data = triangle_data(&scene).unwrap();
        // Three instances, tables of two and of one geometry, five
        // triangles.
        assert_eq!(data.len(), 24 + 8 + 4 + 5 * 36);
        // Instance 0 and 2 share bottom level 1's table; flags beside it.
        assert_eq!((d(&data, 0), d(&data, 4)), (32, 0x2));
        assert_eq!((d(&data, 8), d(&data, 12)), (24, 0x0));
        assert_eq!((d(&data, 16), d(&data, 20)), (32, 0x9));
        // Walk as the shader does: instance, geometry, primitive.
        let triangle = |instance: usize, geometry: usize, primitive: usize| -> Vec<f32> {
            let table = d(&data, 8 * instance) as usize;
            let first = d(&data, table + 4 * geometry) as usize;
            (0..9).map(|k| f(&data, first + 36 * primitive + 4 * k)).collect()
        };
        assert_eq!(triangle(1, 0, 0), [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        // The quad's second triangle, after its transform, in both levels.
        let second = [0.0, 0.0, 6.0, 2.0, 2.0, 6.0, 0.0, 2.0, 8.0];
        assert_eq!(triangle(1, 1, 1), second);
        assert_eq!(triangle(0, 0, 1), second);
        assert_eq!(triangle(2, 0, 0), [0.0, 0.0, 6.0, 2.0, 0.0, 6.0, 2.0, 2.0, 6.0]);
    }

    /// Every generated scene, as either path hands it to the backend,
    /// passes the backend's own check of D3D12's rules; and where the
    /// steps say a frame refits a structure, the backend's own decision
    /// for the two descriptions is a refit. Addresses are made up: the
    /// same ones for every frame, as the device layer keeps them.
    #[test]
    fn every_generated_scene_is_one_the_backend_takes_and_its_refits_are_refits() {
        use crate::adapter;
        use victory_gpu::rt::update::{Built, Change, Decision, Policy, decide};
        let bottom_desc = |bottom: &BottomInputs| match bottom {
            BottomInputs::Triangles(t) => t.desc(0x10_0000, 0x20_0000),
            BottomInputs::Boxes(b) => b.desc(0x30_0000),
        };
        let top_desc = |inputs: &PathInputs| desc::TopDesc {
            instances: inputs
                .instances
                .iter()
                .map(|i| desc::Instance {
                    transform: i.transform,
                    id: i.id,
                    mask: i.mask,
                    hit_group: 0,
                    flags: i.flags,
                    bottom: None,
                })
                .collect(),
            flags: inputs.top_flags,
            inputs: desc::Inputs::Ready,
        };
        let built = |shape| Built { shape, refits: 0, compacted: false, result_bytes: 1 << 16, update_scratch_bytes: 0 };
        let (mut refits, mut builds) = (0, 0);
        for class in adapter::classes() {
            let case = adapter::case(class, adapter::default_seed()).unwrap();
            for convert in [PathInputs::triangles, PathInputs::boxes] {
                let mut before: Option<PathInputs> = None;
                for (f, frame) in case.frames.iter().enumerate() {
                    let at = format!("{class} frame {f}");
                    let next = convert(&frame.scene).unwrap();
                    let primitives: u64 = next.bottoms.iter().map(|b| bottom_desc(b).check().expect(&at)).sum();
                    assert_eq!(primitives, frame.scene.primitives() as u64, "{at}");
                    assert_eq!(top_desc(&next).check(), Ok(()), "{at}");
                    let steps = steps(before.as_ref(), &next, &frame.actions);
                    assert!(steps.notes.is_empty(), "{at}: the scene's own actions are followed: {:?}", steps.notes);
                    for (i, action) in steps.bottoms.iter().enumerate() {
                        builds += u32::from(*action == scene::Action::Rebuild);
                        if *action != scene::Action::Refit {
                            continue;
                        }
                        refits += 1;
                        let was = bottom_desc(&before.as_ref().unwrap().bottoms[i]).shape();
                        let wanted = bottom_desc(&next.bottoms[i]).shape();
                        let decision = decide(Some(&built(was)), &wanted, Change::Positions, &Policy::default());
                        assert_eq!(decision, Decision::Refit, "{at}, bottom level {i}");
                    }
                    if steps.top == scene::Action::Refit {
                        let was = top_desc(before.as_ref().unwrap()).shape();
                        let decision =
                            decide(Some(&built(was)), &top_desc(&next).shape(), Change::Positions, &Policy::default());
                        assert_eq!(decision, Decision::Refit, "{at}, top level");
                    }
                    before = Some(next);
                }
            }
        }
        assert!(refits > 0 && builds > 0);
    }

    #[test]
    fn instances_keep_their_order_and_encode_as_d3d12_reads_them() {
        let scene = scene();
        let list = instances(&scene).unwrap();
        assert_eq!(list.iter().map(|i| i.bottom).collect::<Vec<_>>(), [1, 0, 1]);
        // Through the backend's own encoding: the transform, the id below
        // the mask, the flags above the hit group, the address.
        let encoded = desc::Instance {
            transform: list[0].transform,
            id: list[0].id,
            mask: list[0].mask,
            hit_group: 0,
            flags: list[0].flags,
            bottom: None,
        }
        .encode(0x1234_5678_9ABC);
        assert_eq!(f(&encoded, 12), 7.0);
        assert_eq!(f(&encoded, 44), 9.0);
        assert_eq!(d(&encoded, 48), 0x0FAB_CDEF);
        assert_eq!(d(&encoded, 52), 0x0200_0000);
        assert_eq!(u64::from_le_bytes(encoded[56..64].try_into().unwrap()), 0x1234_5678_9ABC);

        let mut wide = scene.clone();
        wide.instances[1].id = 1 << 24;
        assert!(instances(&wide).unwrap_err().contains("instance 1"));
        let mut unknown = scene.clone();
        unknown.instances[2].flags = 0x10;
        assert!(instances(&unknown).unwrap_err().contains("instance 2"));
        let mut nowhere = scene;
        nowhere.instances[0].bottom = 2;
        assert!(instances(&nowhere).is_err());
        assert!(triangle_data(&nowhere).is_err());
    }
}
