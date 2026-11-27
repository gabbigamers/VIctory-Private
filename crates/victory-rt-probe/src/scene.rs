//! A scene as the probe hands it to a device: bottom levels of triangle
//! geometries, instances, build flags, and what each frame asks of each
//! structure. The same content as `victory-rt`'s scene, in types of the
//! probe's own, so that only [`crate::adapter`] names that crate.

/// A ray record: twelve 32-bit words (origin, tmin, direction, tmax,
/// instance mask, ray flags, two not read), the layout the shaders of
/// `victory_shader::emit::rayquery` read.
pub type RayWords = [u32; 12];

/// A hit record: eight 32-bit words (t, u, v, kind, instance index,
/// instance id, geometry index, primitive index).
pub type HitWords = [u32; 8];

/// A geometry's index buffer.
#[derive(Clone, Debug, PartialEq)]
pub enum Indices {
    /// Three consecutive vertices per triangle.
    None,
    U16(Vec<u16>),
    U32(Vec<u32>),
}

impl Indices {
    fn get(&self, k: usize) -> usize {
        match self {
            Indices::None => k,
            Indices::U16(i) => usize::from(i[k]),
            Indices::U32(i) => i[k] as usize,
        }
    }
}

/// One triangle list of a bottom level.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Indices,
    /// Three rows of four, applied to the vertices when the level is built.
    pub transform: Option<[f32; 12]>,
    /// Not opaque stands for an alpha-tested material.
    pub opaque: bool,
}

impl Geometry {
    pub fn primitive_count(&self) -> usize {
        let corners = match &self.indices {
            Indices::None => self.vertices.len(),
            Indices::U16(i) => i.len(),
            Indices::U32(i) => i.len(),
        };
        corners / 3
    }

    /// The three vertices of primitive `k` as stored, before the
    /// geometry's transform.
    pub fn triangle(&self, k: usize) -> [[f32; 3]; 3] {
        std::array::from_fn(|j| self.vertices[self.indices.get(3 * k + j)])
    }

    /// The three vertices of primitive `k` in the bottom level's object
    /// space: after the geometry's transform, each row computed in single
    /// precision as `((l0 x + l1 y) + l2 z) + t`. What the hybrid path's
    /// boxes are made around and its shader tests.
    pub fn object_triangle(&self, k: usize) -> [[f32; 3]; 3] {
        let stored = self.triangle(k);
        match &self.transform {
            None => stored,
            Some(m) => stored.map(|p| {
                std::array::from_fn(|r| {
                    ((m[4 * r] * p[0] + m[4 * r + 1] * p[1]) + m[4 * r + 2] * p[2]) + m[4 * r + 3]
                })
            }),
        }
    }
}

/// What a build is asked to favour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer {
    Neither,
    FastTrace,
    FastBuild,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bottom {
    pub geometries: Vec<Geometry>,
    /// Built so that it can be refitted.
    pub allow_update: bool,
    pub prefer: Prefer,
}

impl Bottom {
    pub fn primitive_count(&self) -> usize {
        self.geometries.iter().map(Geometry::primitive_count).sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    /// Object to world: three rows of four.
    pub transform: [f32; 12],
    /// 24 bits.
    pub id: u32,
    pub mask: u8,
    /// `D3D12_RAYTRACING_INSTANCE_FLAGS` bits.
    pub flags: u32,
    /// The bottom level's place in [`Scene::bottoms`].
    pub bottom: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub bottoms: Vec<Bottom>,
    pub instances: Vec<Instance>,
    /// The top level's build flags.
    pub allow_update: bool,
    pub prefer: Prefer,
}

impl Scene {
    /// Primitives over the bottom levels, each counted once.
    pub fn primitives(&self) -> usize {
        self.bottoms.iter().map(Bottom::primitive_count).sum()
    }
}

/// What a frame asks of one structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Keep,
    /// An update in place: only positions (or instance transforms) moved,
    /// in a structure built to allow it.
    Refit,
    /// Built again; the first frame's builds are this too.
    Rebuild,
}

/// One action per bottom level, and the top level's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actions {
    pub bottoms: Vec<Action>,
    pub top: Action,
}

impl Actions {
    /// Everything built: the first frame, and a loop's return to it.
    pub fn build_all(bottoms: usize) -> Actions {
        Actions { bottoms: vec![Action::Rebuild; bottoms], top: Action::Rebuild }
    }

    /// How many structures are asked to be refitted and to be built.
    pub fn asked(&self) -> (u32, u32) {
        let all = self.bottoms.iter().chain([&self.top]);
        let count = |a: Action| all.clone().filter(|&&x| x == a).count() as u32;
        (count(Action::Refit), count(Action::Rebuild))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_geometry_transform_is_applied_row_by_row_in_single_precision() {
        let geometry = Geometry {
            vertices: vec![[1.0, 2.0, 3.0], [0.0, 0.0, 0.0], [0.5, 0.25, -1.0]],
            indices: Indices::U16(vec![2, 0, 1]),
            // A mirror in x, a scale of y, z moved by 10.
            transform: Some([-1.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0]),
            opaque: true,
        };
        assert_eq!(geometry.primitive_count(), 1);
        assert_eq!(geometry.triangle(0), [[0.5, 0.25, -1.0], [1.0, 2.0, 3.0], [0.0; 3]]);
        assert_eq!(
            geometry.object_triangle(0),
            [[-0.5, 0.5, 9.0], [-1.0, 4.0, 13.0], [0.0, 0.0, 10.0]]
        );
    }

    #[test]
    fn what_a_frame_asks_is_counted_by_kind() {
        let actions = Actions {
            bottoms: vec![Action::Keep, Action::Refit, Action::Rebuild, Action::Refit],
            top: Action::Refit,
        };
        assert_eq!(actions.asked(), (3, 1));
        assert_eq!(Actions::build_all(2).asked(), (0, 3));
    }
}
