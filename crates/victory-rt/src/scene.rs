//! A scene as DXR's build descriptions state one: bottom levels made of
//! triangle geometries, a top level of instances, the build flags, and for
//! a dynamic scene the changes each frame brings. Nothing here is built or
//! traced; the reference, the software model and (later, on Windows) the
//! native builder all start from this.

use crate::record::instance_flag;

/// A 3x4 transform in D3D12's layout: three rows of four, the fourth column
/// the translation. `p' = L p + T`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform(pub [f32; 12]);

impl Transform {
    pub const IDENTITY: Transform =
        Transform([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    pub fn translation(t: [f32; 3]) -> Self {
        let mut m = Transform::IDENTITY;
        (m.0[3], m.0[7], m.0[11]) = (t[0], t[1], t[2]);
        m
    }

    pub fn scale(s: [f32; 3]) -> Self {
        let mut m = Transform::IDENTITY;
        (m.0[0], m.0[5], m.0[10]) = (s[0], s[1], s[2]);
        m
    }

    /// A rotation about `axis` (0, 1 or 2) by the angle whose half-angle
    /// tangent is `s`: cosine `(1 - s^2) / (1 + s^2)`, sine `2s / (1 +
    /// s^2)`, from the four basic operations only. The matrix is orthonormal
    /// to rounding, which is all an instance transform needs.
    pub fn rotation(axis: usize, s: f32) -> Self {
        let q = 1.0 + s * s;
        let (c, n) = ((1.0 - s * s) / q, (2.0 * s) / q);
        let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
        let mut m = Transform::IDENTITY;
        m.0[4 * a + a] = c;
        m.0[4 * a + b] = -n;
        m.0[4 * b + a] = n;
        m.0[4 * b + b] = c;
        m
    }

    /// `self` after `first`: `p' = self(first(p))`, in `f32`.
    pub fn after(&self, first: &Transform) -> Transform {
        let (a, b) = (&self.0, &first.0);
        let mut m = [0f32; 12];
        for r in 0..3 {
            for c in 0..4 {
                let mut x = (a[4 * r] * b[c] + a[4 * r + 1] * b[4 + c]) + a[4 * r + 2] * b[8 + c];
                if c == 3 {
                    x += a[4 * r + 3];
                }
                m[4 * r + c] = x;
            }
        }
        Transform(m)
    }

    /// `L p + T` in `f32`, each row as `((l0 x + l1 y) + l2 z) + t`. The
    /// software model's builder uses exactly this.
    pub fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        std::array::from_fn(|r| {
            ((m[4 * r] * p[0] + m[4 * r + 1] * p[1]) + m[4 * r + 2] * p[2]) + m[4 * r + 3]
        })
    }

    /// The determinant of the linear part, in `f64` (its sign is exact
    /// unless the transform is nearly singular; the reference takes the sign
    /// from exact arithmetic).
    pub fn det(&self) -> f64 {
        let m = self.0.map(f64::from);
        m[0] * (m[5] * m[10] - m[6] * m[9]) - m[1] * (m[4] * m[10] - m[6] * m[8])
            + m[2] * (m[4] * m[9] - m[5] * m[8])
    }

    /// The inverse in `f64`: rows of four like the transform. `None` when
    /// the linear part is singular.
    pub fn inverse(&self) -> Option<[f64; 12]> {
        let m = self.0.map(f64::from);
        let det = self.det();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let l = |r: usize, c: usize| m[4 * r + c];
        let mut w = [0f64; 12];
        for r in 0..3 {
            for c in 0..3 {
                // The cofactor of (c, r), over the determinant.
                let (r1, r2) = ((c + 1) % 3, (c + 2) % 3);
                let (c1, c2) = ((r + 1) % 3, (r + 2) % 3);
                w[4 * r + c] = (l(r1, c1) * l(r2, c2) - l(r1, c2) * l(r2, c1)) / det;
            }
        }
        for r in 0..3 {
            w[4 * r + 3] = -(w[4 * r] * m[3] + w[4 * r + 1] * m[7] + w[4 * r + 2] * m[11]);
        }
        Some(w)
    }

    pub fn is_identity(&self) -> bool {
        self.0.map(f32::to_bits) == Transform::IDENTITY.0.map(f32::to_bits)
    }

    fn is_finite(&self) -> bool {
        self.0.iter().all(|x| x.is_finite())
    }
}

/// A geometry's index buffer.
#[derive(Clone, Debug, PartialEq)]
pub enum Indices {
    /// Three consecutive vertices per triangle.
    None,
    U16(Vec<u16>),
    U32(Vec<u32>),
}

impl Indices {
    pub fn len(&self) -> Option<usize> {
        match self {
            Indices::None => None,
            Indices::U16(i) => Some(i.len()),
            Indices::U32(i) => Some(i.len()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }

    fn get(&self, k: usize) -> usize {
        match self {
            Indices::None => k,
            Indices::U16(i) => usize::from(i[k]),
            Indices::U32(i) => i[k] as usize,
        }
    }
}

/// One triangle list of a bottom level
/// (`D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC` and its flags).
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Indices,
    /// Applied to the vertices at build time; part of object space, so one
    /// with a negative determinant flips the winding.
    pub transform: Option<Transform>,
    /// `D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE`. A geometry without it stands
    /// for an alpha-tested material ([`crate::alpha_accepts`]).
    pub opaque: bool,
}

impl Geometry {
    pub fn primitive_count(&self) -> usize {
        self.indices.len().unwrap_or(self.vertices.len()) / 3
    }

    /// The three vertices of primitive `k`, untransformed.
    pub fn triangle(&self, k: usize) -> [[f32; 3]; 3] {
        std::array::from_fn(|j| self.vertices[self.indices.get(3 * k + j)])
    }
}

/// `PREFER_FAST_TRACE` or `PREFER_FAST_BUILD`; D3D12 allows one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preference {
    #[default]
    None,
    FastTrace,
    FastBuild,
}

/// `D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAGS`, the ones a build
/// or update decision depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BuildFlags {
    pub allow_update: bool,
    pub preference: Preference,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BottomLevel {
    pub geometries: Vec<Geometry>,
    pub flags: BuildFlags,
}

impl BottomLevel {
    pub fn primitive_count(&self) -> usize {
        self.geometries.iter().map(Geometry::primitive_count).sum()
    }
}

/// `D3D12_RAYTRACING_INSTANCE_DESC` without the hit-group offset, which an
/// inline query does not use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub transform: Transform,
    /// 24 bits, the title's to choose; need not be unique.
    pub id: u32,
    pub mask: u8,
    /// [`instance_flag`] bits.
    pub flags: u32,
    /// The bottom level's position in [`Scene::bottoms`].
    pub blas: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub bottoms: Vec<BottomLevel>,
    pub instances: Vec<Instance>,
    /// The top level's build flags.
    pub flags: BuildFlags,
}

impl Scene {
    /// What a build would refuse, and what this crate does not model: an
    /// index out of range, a count that is not whole triangles, a number
    /// that is not finite, an id over 24 bits, unknown or contradictory
    /// instance flags, a bottom-level reference out of range, an instance
    /// transform that cannot be inverted.
    pub fn validate(&self) -> Result<(), String> {
        for (b, bottom) in self.bottoms.iter().enumerate() {
            for (g, geo) in bottom.geometries.iter().enumerate() {
                let at = format!("bottom level {b} geometry {g}");
                let count = geo.indices.len().unwrap_or(geo.vertices.len());
                if count % 3 != 0 {
                    return Err(format!("{at}: {count} is not whole triangles"));
                }
                if (0..count).any(|k| geo.indices.get(k) >= geo.vertices.len()) {
                    return Err(format!("{at}: an index past the vertices"));
                }
                if matches!(geo.indices, Indices::U16(_)) && geo.vertices.len() > 0x1_0000 {
                    return Err(format!("{at}: more vertices than 16-bit indices reach"));
                }
                if !geo.vertices.iter().flatten().all(|x| x.is_finite())
                    || geo.transform.is_some_and(|t| !t.is_finite())
                {
                    return Err(format!("{at}: a number that is not finite"));
                }
            }
        }
        for (i, inst) in self.instances.iter().enumerate() {
            let at = format!("instance {i}");
            if inst.id >> 24 != 0 {
                return Err(format!("{at}: an id over 24 bits"));
            }
            if inst.flags & !instance_flag::ALL != 0 {
                return Err(format!("{at}: unknown flags"));
            }
            let force = instance_flag::FORCE_OPAQUE | instance_flag::FORCE_NON_OPAQUE;
            if inst.flags & force == force {
                return Err(format!("{at}: both force flags"));
            }
            if inst.blas as usize >= self.bottoms.len() {
                return Err(format!("{at}: no bottom level {}", inst.blas));
            }
            if !inst.transform.is_finite() || inst.transform.inverse().is_none() {
                return Err(format!("{at}: a transform that cannot be inverted"));
            }
        }
        Ok(())
    }

    pub fn triangle_instances(&self) -> usize {
        self.instances
            .iter()
            .map(|i| self.bottoms[i.blas as usize].primitive_count())
            .sum()
    }
}

/// One change a frame brings.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// New positions for a geometry's vertices, the same number of them and
    /// the same indices: what an update (refit) can take.
    Vertices {
        blas: u32,
        geometry: u32,
        vertices: Vec<[f32; 3]>,
    },
    /// A bottom level's geometries replaced: counts, indices or flags may
    /// differ, so it is built again.
    Geometries {
        blas: u32,
        geometries: Vec<Geometry>,
    },
    /// A new transform for one instance.
    InstanceTransform { instance: u32, transform: Transform },
    /// The instance list replaced.
    Instances(Vec<Instance>),
}

/// The changes between one frame and the next.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Frame {
    pub changes: Vec<Change>,
}

/// What a frame's changes ask of one acceleration structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Keep,
    /// An update in place: the structure was built with `allow_update` and
    /// only positions (or instance transforms) moved.
    Refit,
    Rebuild,
}

/// What applying a frame asks for: one action per bottom level, and the top
/// level's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actions {
    pub bottoms: Vec<Action>,
    pub top: Action,
}

impl Frame {
    /// Changes `scene` into the next frame and says what each structure
    /// needs. Positions moving in a bottom level built without
    /// `allow_update` is a rebuild, as D3D12 has it; the top level follows
    /// any bottom level that changed, since its bounds depend on them.
    pub fn apply(&self, scene: &mut Scene) -> Result<Actions, String> {
        let mut bottoms = vec![Action::Keep; scene.bottoms.len()];
        let mut top = Action::Keep;
        let raise = |a: &mut Action, to: Action| {
            if *a != Action::Rebuild {
                *a = to;
            }
        };
        for change in &self.changes {
            match change {
                Change::Vertices {
                    blas,
                    geometry,
                    vertices,
                } => {
                    let b = *blas as usize;
                    let bottom = scene.bottoms.get_mut(b).ok_or("no such bottom level")?;
                    let update = bottom.flags.allow_update;
                    let geo = bottom
                        .geometries
                        .get_mut(*geometry as usize)
                        .ok_or("no such geometry")?;
                    if geo.vertices.len() != vertices.len() {
                        return Err("a vertex change with a different vertex count".into());
                    }
                    geo.vertices.clone_from(vertices);
                    let to = if update {
                        Action::Refit
                    } else {
                        Action::Rebuild
                    };
                    raise(&mut bottoms[b], to);
                }
                Change::Geometries { blas, geometries } => {
                    let b = *blas as usize;
                    let bottom = scene.bottoms.get_mut(b).ok_or("no such bottom level")?;
                    bottom.geometries.clone_from(geometries);
                    bottoms[b] = Action::Rebuild;
                }
                Change::InstanceTransform {
                    instance,
                    transform,
                } => {
                    let inst = scene
                        .instances
                        .get_mut(*instance as usize)
                        .ok_or("no such instance")?;
                    inst.transform = *transform;
                    let to = if scene.flags.allow_update {
                        Action::Refit
                    } else {
                        Action::Rebuild
                    };
                    raise(&mut top, to);
                }
                Change::Instances(instances) => {
                    scene.instances.clone_from(instances);
                    top = Action::Rebuild;
                }
            }
        }
        if bottoms.iter().any(|a| *a != Action::Keep) {
            let to = if scene.flags.allow_update {
                Action::Refit
            } else {
                Action::Rebuild
            };
            raise(&mut top, to);
        }
        scene.validate()?;
        Ok(Actions { bottoms, top })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri() -> Geometry {
        Geometry {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
            indices: Indices::U16(vec![0, 1, 2, 2, 1, 3]),
            transform: None,
            opaque: true,
        }
    }

    fn scene(update: bool) -> Scene {
        Scene {
            bottoms: vec![BottomLevel {
                geometries: vec![tri()],
                flags: BuildFlags {
                    allow_update: update,
                    preference: Preference::FastTrace,
                },
            }],
            instances: vec![Instance {
                transform: Transform::IDENTITY,
                id: 5,
                mask: 0xFF,
                flags: 0,
                blas: 0,
            }],
            flags: BuildFlags {
                allow_update: update,
                preference: Preference::None,
            },
        }
    }

    #[test]
    fn indices_pick_the_vertices() {
        let g = tri();
        assert_eq!(g.primitive_count(), 2);
        assert_eq!(
            g.triangle(1),
            [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]
        );
        let mut plain = tri();
        plain.indices = Indices::None;
        assert_eq!(plain.primitive_count(), 1);
    }

    #[test]
    fn what_a_build_would_refuse_is_refused() {
        assert!(scene(false).validate().is_ok());
        let bad = |f: &dyn Fn(&mut Scene)| {
            let mut s = scene(false);
            f(&mut s);
            s.validate().is_err()
        };
        assert!(bad(
            &|s| s.bottoms[0].geometries[0].indices = Indices::U16(vec![0, 1, 4])
        ));
        assert!(bad(
            &|s| s.bottoms[0].geometries[0].indices = Indices::U32(vec![0, 1])
        ));
        assert!(bad(
            &|s| s.bottoms[0].geometries[0].vertices[1][2] = f32::NAN
        ));
        assert!(bad(&|s| s.instances[0].id = 1 << 24));
        assert!(bad(&|s| s.instances[0].flags = 0xC));
        assert!(bad(&|s| s.instances[0].flags = 0x10));
        assert!(bad(&|s| s.instances[0].blas = 1));
        assert!(bad(
            &|s| s.instances[0].transform = Transform::scale([1.0, 0.0, 1.0])
        ));
    }

    #[test]
    fn the_inverse_undoes_the_transform() {
        let m = Transform::translation([3.0, -2.0, 0.5])
            .after(&Transform::rotation(1, 0.5))
            .after(&Transform::scale([2.0, -0.5, 4.0]));
        assert!(m.det() < 0.0);
        let w = m.inverse().unwrap();
        let p = [0.25f32, -1.5, 3.0];
        let q = m.apply(p).map(f64::from);
        for r in 0..3 {
            let back = w[4 * r] * q[0] + w[4 * r + 1] * q[1] + w[4 * r + 2] * q[2] + w[4 * r + 3];
            assert!((back - f64::from(p[r])).abs() < 1e-5, "{back} {}", p[r]);
        }
        // A rotation by the angle whose half-angle tangent is 1 is a
        // quarter turn, exactly.
        let quarter = Transform::rotation(2, 1.0);
        assert_eq!(quarter.apply([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn a_frame_says_refit_or_rebuild() {
        let moved = Frame {
            changes: vec![Change::Vertices {
                blas: 0,
                geometry: 0,
                vertices: vec![
                    [0.0, 0.0, 1.0],
                    [1.0, 0.0, 1.0],
                    [0.0, 1.0, 1.0],
                    [1.0, 1.0, 1.0],
                ],
            }],
        };
        let mut s = scene(true);
        let a = moved.apply(&mut s).unwrap();
        assert_eq!((a.bottoms[0], a.top), (Action::Refit, Action::Refit));
        assert_eq!(s.bottoms[0].geometries[0].vertices[0][2], 1.0);
        // Without allow_update the same change is a rebuild.
        let a = moved.apply(&mut scene(false)).unwrap();
        assert_eq!((a.bottoms[0], a.top), (Action::Rebuild, Action::Rebuild));

        let regrown = Frame {
            changes: vec![Change::Geometries {
                blas: 0,
                geometries: vec![tri(), tri()],
            }],
        };
        let a = regrown.apply(&mut scene(true)).unwrap();
        assert_eq!((a.bottoms[0], a.top), (Action::Rebuild, Action::Refit));

        let slid = Frame {
            changes: vec![Change::InstanceTransform {
                instance: 0,
                transform: Transform::translation([1.0, 0.0, 0.0]),
            }],
        };
        let a = slid.apply(&mut scene(true)).unwrap();
        assert_eq!((a.bottoms[0], a.top), (Action::Keep, Action::Refit));
        let a = slid.apply(&mut scene(false)).unwrap();
        assert_eq!(a.top, Action::Rebuild);

        let short = Frame {
            changes: vec![Change::Vertices {
                blas: 0,
                geometry: 0,
                vertices: vec![[0.0; 3]],
            }],
        };
        assert!(short.apply(&mut scene(true)).is_err());
    }
}
