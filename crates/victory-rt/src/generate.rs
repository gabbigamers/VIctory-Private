//! Seeded synthetic scenes and rays, one class per property under test.
//! Everything is drawn from [`Rng`] and built with exact conversions and the
//! four basic `f32` operations (rotations from a rational half-angle
//! tangent, shells that are cubes), so a class and a seed give the same
//! bytes on every platform. No geometry, shader or number here comes from a
//! title.

use crate::record::{Ray, instance_flag, ray_flag};
use crate::rng::Rng;
use crate::scene::{
    BottomLevel, BuildFlags, Change, Frame, Geometry, Indices, Instance, Preference, Scene,
    Transform,
};

/// The seed the host tests and the documented agreement table use.
pub const DEFAULT_SEED: u64 = 0x5EED_2026_1127;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    StaticSoup,
    DynamicRefit,
    DynamicRebuild,
    Instancing,
    AlphaTested,
    Overlapping,
    Transformed,
    HitOrdering,
    Precision,
    MasksAndFlags,
    Single,
    Closed,
    Far,
    Seam,
    SurfaceRays,
    Scale,
    SeamEdge,
    Edge,
    Thin,
    LargeThin,
    CullEdge,
    Extent,
    Tie,
    FarGraze,
    Flat,
    Degenerate,
    ClosedEdge,
}

/// What a class is to the proposed rule (docs/dxr-validation.md,
/// "Scenes").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    /// Rays that are not aimed at a difference: the bound on ambiguous
    /// rays applies, and the tolerance constant is measured over these.
    Random,
    /// Rays aimed at edges, seams, ties, slivers, boundaries: judged by
    /// admissibility, and never a validation alone.
    Directed,
}

impl Class {
    /// The ten classes this crate had before the rule's were added, in
    /// their order: the first ten of [`Class::ALL`].
    pub const FIRST: [Class; 10] = [
        Class::StaticSoup,
        Class::DynamicRefit,
        Class::DynamicRebuild,
        Class::Instancing,
        Class::AlphaTested,
        Class::Overlapping,
        Class::Transformed,
        Class::HitOrdering,
        Class::Precision,
        Class::MasksAndFlags,
    ];

    pub const ALL: [Class; 27] = [
        Class::StaticSoup,
        Class::DynamicRefit,
        Class::DynamicRebuild,
        Class::Instancing,
        Class::AlphaTested,
        Class::Overlapping,
        Class::Transformed,
        Class::HitOrdering,
        Class::Precision,
        Class::MasksAndFlags,
        Class::Single,
        Class::Closed,
        Class::Far,
        Class::Seam,
        Class::SurfaceRays,
        Class::Scale,
        Class::SeamEdge,
        Class::Edge,
        Class::Thin,
        Class::LargeThin,
        Class::CullEdge,
        Class::Extent,
        Class::Tie,
        Class::FarGraze,
        Class::Flat,
        Class::Degenerate,
        Class::ClosedEdge,
    ];

    pub fn kind(self) -> ClassKind {
        match self {
            Class::Overlapping
            | Class::Precision
            | Class::SeamEdge
            | Class::Edge
            | Class::Thin
            | Class::LargeThin
            | Class::CullEdge
            | Class::Extent
            | Class::Tie
            | Class::FarGraze
            | Class::Flat
            | Class::Degenerate
            | Class::ClosedEdge => ClassKind::Directed,
            _ => ClassKind::Random,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Class::StaticSoup => "static-soup",
            Class::DynamicRefit => "dynamic-refit",
            Class::DynamicRebuild => "dynamic-rebuild",
            Class::Instancing => "instancing",
            Class::AlphaTested => "alpha-tested",
            Class::Overlapping => "overlapping",
            Class::Transformed => "transformed",
            Class::HitOrdering => "hit-ordering",
            Class::Precision => "precision",
            Class::MasksAndFlags => "masks-and-flags",
            Class::Single => "single",
            Class::Closed => "closed",
            Class::Far => "far",
            Class::Seam => "seam",
            Class::SurfaceRays => "surface-rays",
            Class::Scale => "scale",
            Class::SeamEdge => "seam-edge",
            Class::Edge => "edge",
            Class::Thin => "thin",
            Class::LargeThin => "large-thin",
            Class::CullEdge => "cull-edge",
            Class::Extent => "extent",
            Class::Tie => "tie",
            Class::FarGraze => "far-graze",
            Class::Flat => "flat",
            Class::Degenerate => "degenerate",
            Class::ClosedEdge => "closed-edge",
        }
    }

    pub fn from_name(name: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What the class holds, in a line.
    pub fn describe(self) -> &'static str {
        match self {
            Class::StaticSoup => {
                "one bottom level of random triangles (no indices) and a 16-bit indexed grid, one instance"
            }
            Class::DynamicRefit => {
                "vertices move with the topology kept, in bottom levels built for update, and instances move: four frames"
            }
            Class::DynamicRebuild => {
                "triangle counts, geometry lists and the instance list change: four frames"
            }
            Class::Instancing => {
                "48 instances of two small bottom levels: rotations, non-uniform scales, mirrors, repeated ids"
            }
            Class::AlphaTested => {
                "non-opaque triangles large enough for the alpha pattern, before an opaque backdrop"
            }
            Class::Overlapping => {
                "the same triangle several times in a geometry, across geometries and across instances; coplanar overlaps"
            }
            Class::Transformed => {
                "per-geometry transforms (one mirrored) under rotated, scaled, sheared and mirrored instances; facing culls"
            }
            Class::HitOrdering => {
                "stacks of non-opaque layers along the ray, accepted and rejected candidates interleaved"
            }
            Class::Precision => {
                "rays exactly through edges and vertices of closed meshes, grazing rays, slivers, triangles of no area, coordinates near 2^20, tmin and tmax exactly on a hit"
            }
            Class::MasksAndFlags => "instance masks and flags against ray masks and every ray flag",
            Class::Single => "one triangle",
            Class::Closed => {
                "closed meshes of several surfaces, moved and scaled; random rays from outside, and from inside, where none may escape"
            }
            Class::Far => {
                "triangles with edges of 2^-4 to 2^4 units at 2^12, 2^14 and 2^16 from the origin, in the vertices and by an instance"
            }
            Class::Seam => {
                "closed surfaces split across bottom levels under one transform, across instances with different transforms, across a geometry transform, and across geometries; random rays"
            }
            Class::SurfaceRays => {
                "rays that leave a surface and rays whose extent ends on one, with biases of 0, 1, 16 and 256 units of 8 spacings"
            }
            Class::Scale => {
                "2,048 triangles; incoherent rays and a coherent bundle from one origin"
            }
            Class::SeamEdge => {
                "the seam class's scene; rays exactly on the seams and 1, 2 and 8 spacings either side"
            }
            Class::Edge => {
                "two triangles sharing an edge, a fan, a closed mesh; rays exactly on edges and vertices and 1, 2 and 8 spacings either side"
            }
            Class::Thin => {
                "slivers and needles to an aspect of 2^20; rays through their inside, edges and vertices"
            }
            Class::LargeThin => {
                "triangles a few spacings across at 2^16, 2^20 and 2^24 from the origin"
            }
            Class::CullEdge => {
                "an edge shared by a front and a back face, under each facing cull and none"
            }
            Class::Extent => {
                "hits at exactly t = 0, exactly at tmin and tmax and one spacing either side; tmax infinite and the largest float; tmin of negative zero"
            }
            Class::Tie => {
                "coplanar duplicates of one surface and of two; parallel layers 1, 2, 4 and 8 spacings apart"
            }
            Class::FarGraze => {
                "the edge class's shapes at 2^18 from the origin, met at grazing angles"
            }
            Class::Flat => {
                "triangles in axis planes; rays along an axis through grid lines and vertices, and rays lying in a face's plane"
            }
            Class::Degenerate => {
                "triangles of no area (collinear, two vertices equal, a point), plain, under a turned geometry transform and in a turned instance"
            }
            Class::ClosedEdge => {
                "the closed class's meshes; rays from inside one whose origins and directions are on a coarse lattice, so that many go exactly through an edge or a vertex of its faces"
            }
        }
    }
}

/// A class at a seed: the scene of frame 0, the changes of each later frame,
/// and the rays, which are traced against every frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub class: Class,
    pub seed: u64,
    pub scene: Scene,
    pub frames: Vec<Frame>,
    pub rays: Vec<Ray>,
}

impl Case {
    pub fn frame_count(&self) -> usize {
        1 + self.frames.len()
    }

    /// The scene at each frame.
    pub fn scenes(&self) -> Vec<Scene> {
        let mut out = vec![self.scene.clone()];
        for frame in &self.frames {
            let mut next = out.last().expect("frame 0").clone();
            frame.apply(&mut next).expect("a generated frame applies");
            out.push(next);
        }
        out
    }

    /// FNV-1a 64 over the case's numbers, bit for bit: for the scene of
    /// each frame the vertices, indices, geometry transforms and opaque
    /// flags of every geometry and the transform, id, mask, flags and
    /// bottom level of every instance, then the rays. Two machines whose
    /// generators give the same fingerprint traced the same rays against
    /// the same triangles; the result file carries it for that check
    /// ([`crate::results`]). Build flags are not in it: they change no hit.
    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xCBF2_9CE4_8422_2325u64;
        let mut eat = |w: u32| {
            for b in w.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3);
            }
        };
        for scene in self.scenes() {
            for bottom in &scene.bottoms {
                for geo in &bottom.geometries {
                    geo.vertices.iter().flatten().for_each(|x| eat(x.to_bits()));
                    (0..geo.primitive_count() * 3).for_each(|k| {
                        eat(match &geo.indices {
                            Indices::None => k as u32,
                            Indices::U16(i) => u32::from(i[k]),
                            Indices::U32(i) => i[k],
                        })
                    });
                    if let Some(t) = &geo.transform {
                        t.0.iter().for_each(|x| eat(x.to_bits()));
                    }
                    eat(u32::from(geo.opaque));
                }
            }
            for inst in &scene.instances {
                inst.transform.0.iter().for_each(|x| eat(x.to_bits()));
                [inst.id, u32::from(inst.mask), inst.flags, inst.blas]
                    .into_iter()
                    .for_each(&mut eat);
            }
        }
        self.rays
            .iter()
            .flat_map(|r| r.to_words())
            .for_each(&mut eat);
        h
    }
}

type P = [f32; 3];

fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn mul(a: P, k: f32) -> P {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn axis(k: usize, x: f32) -> P {
    let mut p = [0.0; 3];
    p[k] = x;
    p
}

fn area_is_zero(t: &[P; 3]) -> bool {
    let (a, b) = (sub(t[1], t[0]), sub(t[2], t[0]));
    a[1] * b[2] - a[2] * b[1] == 0.0
        && a[2] * b[0] - a[0] * b[2] == 0.0
        && a[0] * b[1] - a[1] * b[0] == 0.0
}

/// Vertices and triangle indices.
#[derive(Clone)]
struct Mesh {
    vertices: Vec<P>,
    indices: Vec<u32>,
}

#[derive(Clone, Copy)]
enum Idx {
    None,
    U16,
    U32,
}

impl Mesh {
    /// `nx` by `ny` cells from `origin`, two triangles a cell: (a, b, c)
    /// and (c, b, d) with a the cell's corner, b one step along `du`, c one
    /// along `dv`, d both. The triangles' `(v1 - v0) x (v2 - v0)` is along
    /// `du x dv`.
    fn grid(nx: u32, ny: u32, origin: P, du: P, dv: P) -> Mesh {
        let mut vertices = Vec::new();
        for j in 0..=ny {
            for i in 0..=nx {
                vertices.push(add(origin, add(mul(du, i as f32), mul(dv, j as f32))));
            }
        }
        let mut indices = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                let a = j * (nx + 1) + i;
                let (b, c, d) = (a + 1, a + nx + 1, a + nx + 2);
                indices.extend([a, b, c, c, b, d]);
            }
        }
        Mesh { vertices, indices }
    }

    /// The surface of a cube of half-width `half` about `center`, `n` by `n`
    /// cells a face, front faces outward. Faces do not share vertex
    /// entries, but where they meet the positions are the same bits.
    fn cube(n: u32, half: f32, center: P) -> Mesh {
        let mut mesh = Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
        };
        for k in 0..3 {
            for sign in [1.0, -1.0] {
                let (origin, du, dv, _) = face(k, sign, half, n, center);
                mesh.append(&Mesh::grid(n, n, origin, du, dv));
            }
        }
        mesh
    }

    /// Eight faces about the origin, front faces outward.
    fn octahedron(r: f32) -> Mesh {
        let mut mesh = Mesh {
            vertices: (0..6)
                .map(|k| axis(k / 2, if k % 2 == 0 { r } else { -r }))
                .collect(),
            indices: Vec::new(),
        };
        for s in 0..8u32 {
            let v = [s & 1, 2 + (s >> 1 & 1), 4 + (s >> 2 & 1)];
            // An odd number of negative axes turns the winding.
            if s.count_ones() % 2 == 0 {
                mesh.indices.extend(v);
            } else {
                mesh.indices.extend([v[0], v[2], v[1]]);
            }
        }
        mesh
    }

    fn append(&mut self, other: &Mesh) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    fn geometry(&self, idx: Idx, transform: Option<Transform>, opaque: bool) -> Geometry {
        let (vertices, indices) = match idx {
            Idx::None => (
                self.indices
                    .iter()
                    .map(|&i| self.vertices[i as usize])
                    .collect(),
                Indices::None,
            ),
            Idx::U16 => (
                self.vertices.clone(),
                Indices::U16(self.indices.iter().map(|&i| i as u16).collect()),
            ),
            Idx::U32 => (self.vertices.clone(), Indices::U32(self.indices.clone())),
        };
        Geometry {
            vertices,
            indices,
            transform,
            opaque,
        }
    }
}

/// One face of a cube: the corner its grid starts from, the two cell steps,
/// and the outward normal. `du x dv` is along the normal.
fn face(k: usize, sign: f32, half: f32, n: u32, center: P) -> (P, P, P, P) {
    let (b, c) = ((k + 1) % 3, (k + 2) % 3);
    let (ub, vc) = if sign > 0.0 { (b, c) } else { (c, b) };
    let step = 2.0 * half / n as f32;
    let mut origin = center;
    origin[k] += sign * half;
    origin[b] -= half;
    origin[c] -= half;
    (origin, axis(ub, step), axis(vc, step), axis(k, sign))
}

fn soup_geometry(vertices: Vec<P>, opaque: bool) -> Geometry {
    Geometry {
        vertices,
        indices: Indices::None,
        transform: None,
        opaque,
    }
}

fn bottom(geometries: Vec<Geometry>, allow_update: bool, preference: Preference) -> BottomLevel {
    BottomLevel {
        geometries,
        flags: BuildFlags {
            allow_update,
            preference,
        },
    }
}

fn instance(transform: Transform, id: u32, blas: u32) -> Instance {
    Instance {
        transform,
        id,
        mask: 0xFF,
        flags: 0,
        blas,
    }
}

/// Every triangle of every instance in world space, in `f32`: where rays
/// are aimed, not what they are judged against.
fn world_triangles(scene: &Scene) -> Vec<[P; 3]> {
    let mut out = Vec::new();
    for inst in &scene.instances {
        for geo in &scene.bottoms[inst.blas as usize].geometries {
            for k in 0..geo.primitive_count() {
                out.push(geo.triangle(k).map(|v| {
                    let v = geo.transform.map_or(v, |g| g.apply(v));
                    inst.transform.apply(v)
                }));
            }
        }
    }
    out
}

/// The centre and half-width of the cube around `tris`.
fn extent(tris: &[[P; 3]]) -> (P, f32) {
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for v in tris.iter().flatten() {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let center = mul(add(lo, hi), 0.5);
    let half = (0..3).fold(1.0f32, |m, k| m.max((hi[k] - lo[k]) * 0.5));
    (center, half)
}

struct Gen {
    r: Rng,
}

impl Gen {
    fn new(class: Class, seed: u64) -> Self {
        let salt = (class as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407);
        Gen {
            r: Rng::new(seed ^ salt),
        }
    }

    /// A point with coordinates that are multiples of 2^-`shift`, within
    /// `half` (in those units) of the origin.
    fn point(&mut self, half: i32, shift: u32) -> P {
        std::array::from_fn(|_| self.r.dyadic(-half, half, shift))
    }

    /// `count` triangles with area: a corner within `half` and two more
    /// within `size` of it, all in units of 2^-`shift`.
    fn soup(&mut self, count: usize, half: i32, size: i32, shift: u32) -> Vec<P> {
        let mut out = Vec::new();
        while out.len() < 3 * count {
            let a = self.point(half, shift);
            let t = [
                a,
                add(a, self.point(size, shift)),
                add(a, self.point(size, shift)),
            ];
            if !area_is_zero(&t) {
                out.extend(t);
            }
        }
        out
    }

    fn rotation(&mut self) -> Transform {
        let k = self.r.below(3) as usize;
        Transform::rotation(k, self.r.dyadic(-8, 8, 3))
    }

    /// A point on the surface of a cube about `center`, of half-width
    /// between `lo` and `hi`.
    fn shell(&mut self, center: P, lo: f32, hi: f32) -> P {
        let r = lo + (hi - lo) * self.r.unit();
        let k = self.r.below(3) as usize;
        let mut p: P = std::array::from_fn(|_| r * (2.0 * self.r.unit() - 1.0));
        p[k] = if self.r.chance(1, 2) { r } else { -r };
        add(center, p)
    }

    fn inside(&mut self, t: &[P; 3]) -> P {
        let (mut a, mut b) = (self.r.unit(), self.r.unit());
        if a + b > 1.0 {
            (a, b) = (1.0 - a, 1.0 - b);
        }
        add(t[0], add(mul(sub(t[1], t[0]), a), mul(sub(t[2], t[0]), b)))
    }

    /// A ray from `origin` through `target`, the direction scaled so that
    /// the target is at `t` between 1/4 and 4; one in eight ends before or
    /// near the target, one in eight starts before or near it.
    fn through(&mut self, origin: P, target: P) -> Ray {
        let k = self.r.dyadic(2, 32, 3);
        let dir = mul(sub(target, origin), k);
        let (tmin, tmax) = match self.r.below(8) {
            0 => (0.0, (0.25 + self.r.unit()) / k),
            1 => ((0.25 + self.r.unit()) / k, 1000.0),
            _ => (0.0, 1000.0),
        };
        if dir == [0.0; 3] {
            return Ray::new(origin, [0.0, 0.0, 1.0], 0.0, 1000.0);
        }
        Ray::new(origin, dir, tmin, tmax)
    }

    /// `count` rays, `aimed` in eight of them through a point of a triangle
    /// of `tris` and the rest through a point of the cube around them, all
    /// from a shell outside it.
    fn rays(&mut self, tris: &[[P; 3]], count: usize, aimed: u32) -> Vec<Ray> {
        let (center, half) = extent(tris);
        (0..count)
            .map(|_| {
                let origin = self.shell(center, 1.25 * half, 3.0 * half);
                let target = if self.r.chance(aimed, 8) {
                    let t = *self.r.pick(tris);
                    self.inside(&t)
                } else {
                    add(
                        center,
                        std::array::from_fn(|_| half * (2.0 * self.r.unit() - 1.0)),
                    )
                };
                self.through(origin, target)
            })
            .collect()
    }
}

/// The class at a seed.
pub fn generate(class: Class, seed: u64) -> Case {
    let mut g = Gen::new(class, seed);
    let (scene, frames, rays) = match class {
        Class::StaticSoup => static_soup(&mut g),
        Class::DynamicRefit => dynamic_refit(&mut g),
        Class::DynamicRebuild => dynamic_rebuild(&mut g),
        Class::Instancing => instancing(&mut g),
        Class::AlphaTested => alpha_tested(&mut g),
        Class::Overlapping => overlapping(&mut g),
        Class::Transformed => transformed(&mut g),
        Class::HitOrdering => hit_ordering(&mut g),
        Class::Precision => precision(&mut g),
        Class::MasksAndFlags => masks_and_flags(&mut g),
        Class::Single => single(&mut g),
        Class::Closed => closed(&mut g),
        Class::Far => far(&mut g),
        Class::Seam => seam(&mut g),
        Class::SurfaceRays => surface_rays(&mut g),
        Class::Scale => scale(&mut g),
        Class::SeamEdge => seam_edge(&mut g),
        Class::Edge => edge(&mut g, [2.0, 3.0, 1.0], false),
        Class::Thin => thin(&mut g),
        Class::LargeThin => large_thin(&mut g),
        Class::CullEdge => cull_edge(&mut g),
        Class::Extent => extent_class(&mut g),
        Class::Tie => tie(&mut g),
        Class::FarGraze => edge(&mut g, [262_144.0, -131_072.0, 65_536.0], true),
        Class::Flat => flat(&mut g),
        Class::Degenerate => degenerate(&mut g),
        Class::ClosedEdge => closed_edge(&mut g),
    };
    let case = Case {
        class,
        seed,
        scene,
        frames,
        rays,
    };
    debug_assert!(case.scene.validate().is_ok());
    debug_assert!(case.rays.iter().all(|r| r.validate().is_ok()));
    case
}

type Parts = (Scene, Vec<Frame>, Vec<Ray>);

fn top(instances: Vec<Instance>, bottoms: Vec<BottomLevel>, allow_update: bool) -> Scene {
    Scene {
        bottoms,
        instances,
        flags: BuildFlags {
            allow_update,
            preference: Preference::FastTrace,
        },
    }
}

fn static_soup(g: &mut Gen) -> Parts {
    let soup = soup_geometry(g.soup(160, 64, 24, 3), true);
    let ground = Mesh::grid(8, 8, [-8.0, -8.0, -9.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
    let scene = top(
        vec![instance(Transform::IDENTITY, 7, 0)],
        vec![bottom(
            vec![soup, ground.geometry(Idx::U16, None, true)],
            false,
            Preference::FastTrace,
        )],
        false,
    );
    let rays = g.rays(&world_triangles(&scene), 2000, 6);
    (scene, Vec::new(), rays)
}

fn dynamic_refit(g: &mut Gen) -> Parts {
    let ground = Mesh::grid(8, 8, [-12.0, -12.0, -6.0], [3.0, 0.0, 0.0], [0.0, 3.0, 0.0]);
    let sheet = Mesh::grid(6, 6, [-3.0, -3.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let blob = g.soup(40, 16, 12, 3);
    let scene = top(
        vec![
            instance(Transform::IDENTITY, 1, 0),
            instance(Transform::translation([-5.0, 0.0, 0.0]), 2, 1),
            instance(
                Transform::translation([5.0, 0.0, 2.0]).after(&Transform::rotation(0, 0.5)),
                3,
                1,
            ),
            instance(
                Transform::translation([0.0, 6.0, 3.0]).after(&Transform::scale([1.5, 1.5, 1.5])),
                4,
                2,
            ),
        ],
        vec![
            bottom(
                vec![ground.geometry(Idx::U32, None, true)],
                false,
                Preference::FastTrace,
            ),
            bottom(
                vec![sheet.geometry(Idx::U16, None, true)],
                true,
                Preference::FastBuild,
            ),
            bottom(
                vec![soup_geometry(blob.clone(), true)],
                true,
                Preference::FastBuild,
            ),
        ],
        true,
    );
    let mut frames = Vec::new();
    let mut blob = blob;
    for f in 1..4 {
        // The sheet ripples, the blob's vertices wander, the blob's
        // instance drifts; on the second frame a sheet's instance turns.
        let waved: Vec<P> = sheet
            .vertices
            .iter()
            .map(|v| [v[0], v[1], g.r.dyadic(-8, 8, 3)])
            .collect();
        for v in &mut blob {
            *v = add(*v, g.point(2, 3));
        }
        let mut changes = vec![
            Change::Vertices {
                blas: 1,
                geometry: 0,
                vertices: waved,
            },
            Change::Vertices {
                blas: 2,
                geometry: 0,
                vertices: blob.clone(),
            },
            Change::InstanceTransform {
                instance: 3,
                transform: Transform::translation([f as f32, 6.0, 3.0 - f as f32])
                    .after(&Transform::scale([1.5, 1.5, 1.5])),
            },
        ];
        if f == 2 {
            changes.push(Change::InstanceTransform {
                instance: 2,
                transform: Transform::translation([5.0, -1.0, 2.5])
                    .after(&Transform::rotation(0, 0.75)),
            });
        }
        frames.push(Frame { changes });
    }
    let rays = rays_over_frames(g, &scene, &frames, 1200);
    (scene, frames, rays)
}

/// Rays aimed at the triangles of every frame.
fn rays_over_frames(g: &mut Gen, scene: &Scene, frames: &[Frame], count: usize) -> Vec<Ray> {
    let mut tris = world_triangles(scene);
    let mut next = scene.clone();
    for frame in frames {
        frame.apply(&mut next).expect("a generated frame applies");
        tris.extend(world_triangles(&next));
    }
    g.rays(&tris, count, 7)
}

fn dynamic_rebuild(g: &mut Gen) -> Parts {
    let scene = top(
        vec![
            instance(Transform::IDENTITY, 1, 0),
            instance(Transform::translation([7.0, 0.0, 0.0]), 2, 1),
        ],
        vec![
            bottom(
                vec![soup_geometry(g.soup(60, 32, 16, 3), true)],
                false,
                Preference::FastTrace,
            ),
            bottom(
                vec![Mesh::cube(1, 1.0, [0.0; 3]).geometry(Idx::U16, None, true)],
                false,
                Preference::FastTrace,
            ),
        ],
        false,
    );
    let patch = Mesh::grid(3, 3, [-2.0, -2.0, 5.0], [1.5, 0.0, 0.0], [0.0, 1.5, 0.5]);
    let frames = vec![
        // More triangles.
        Frame {
            changes: vec![Change::Geometries {
                blas: 0,
                geometries: vec![soup_geometry(g.soup(90, 32, 16, 3), true)],
            }],
        },
        // Fewer, and a second geometry.
        Frame {
            changes: vec![Change::Geometries {
                blas: 0,
                geometries: vec![
                    soup_geometry(g.soup(40, 32, 16, 3), true),
                    patch.geometry(Idx::U16, None, true),
                ],
            }],
        },
        // A finer cube, and an instance more.
        Frame {
            changes: vec![
                Change::Geometries {
                    blas: 1,
                    geometries: vec![Mesh::cube(2, 1.0, [0.0; 3]).geometry(Idx::U32, None, true)],
                },
                Change::Instances(vec![
                    instance(Transform::IDENTITY, 1, 0),
                    instance(Transform::translation([7.0, 0.0, 0.0]), 2, 1),
                    instance(
                        Transform::translation([-7.0, 1.0, 0.0])
                            .after(&Transform::rotation(2, 0.375)),
                        3,
                        1,
                    ),
                ]),
            ],
        },
    ];
    let rays = rays_over_frames(g, &scene, &frames, 1200);
    (scene, frames, rays)
}

fn instancing(g: &mut Gen) -> Parts {
    let small = bottom(
        vec![
            Mesh::octahedron(1.0).geometry(Idx::U16, None, true),
            soup_geometry(g.soup(8, 8, 6, 3), true),
        ],
        false,
        Preference::FastTrace,
    );
    let cube = bottom(
        vec![Mesh::cube(1, 1.0, [0.0; 3]).geometry(Idx::U16, None, true)],
        false,
        Preference::FastTrace,
    );
    let scales = [[2.0, 0.5, 1.0], [0.25, 1.5, 3.0], [1.0, 1.0, 4.0]];
    let mirrors = [[-1.0, 1.0, 1.0], [1.0, -2.0, 0.5], [-1.5, -1.5, -1.5]];
    let mut instances = Vec::new();
    for i in 0..48u32 {
        let at = [
            (i % 4) as f32 * 6.0 - 9.0,
            (i / 4 % 4) as f32 * 6.0 - 9.0,
            (i / 16) as f32 * 6.0 - 6.0,
        ];
        let shape = match i % 4 {
            0 => g.rotation(),
            1 => g.rotation().after(&Transform::scale(*g.r.pick(&scales))),
            2 => g.rotation().after(&Transform::scale(*g.r.pick(&mirrors))),
            _ => g
                .rotation()
                .after(&g.rotation())
                .after(&Transform::scale([1.5; 3])),
        };
        // Ids are the title's: some repeat.
        let id = if i % 7 == 3 {
            0x00AB_CDEF
        } else {
            g.r.below(1 << 24)
        };
        instances.push(instance(
            Transform::translation(at).after(&shape),
            id,
            u32::from(i % 6 == 5),
        ));
    }
    let scene = top(instances, vec![small, cube], false);
    let rays = g.rays(&world_triangles(&scene), 1500, 7);
    (scene, Vec::new(), rays)
}

fn alpha_tested(g: &mut Gen) -> Parts {
    let lattice = Mesh::grid(4, 4, [-4.0, -4.0, -7.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
    let backdrop = Mesh::grid(
        1,
        1,
        [-16.0, -16.0, -12.0],
        [32.0, 0.0, 0.0],
        [0.0, 32.0, 0.0],
    );
    let blas = bottom(
        vec![
            soup_geometry(g.soup(24, 48, 48, 3), false),
            lattice.geometry(Idx::U16, None, false),
            backdrop.geometry(Idx::None, None, true),
        ],
        false,
        Preference::FastTrace,
    );
    let scene = top(
        vec![
            instance(Transform::IDENTITY, 1, 0),
            instance(
                Transform::translation([40.0, 0.0, 0.0]).after(&Transform::rotation(1, 0.25)),
                2,
                0,
            ),
        ],
        vec![blas],
        false,
    );
    let rays = g.rays(&world_triangles(&scene), 2000, 7);
    (scene, Vec::new(), rays)
}

fn overlapping(g: &mut Gen) -> Parts {
    // The same triangle two or three times in one geometry: as it is, with
    // its vertices rotated (other weights), with its winding turned.
    let base = g.soup(20, 24, 12, 2);
    let mut copies = Vec::new();
    for t in base.chunks(3) {
        copies.extend_from_slice(t);
        for _ in 0..g.r.range(1, 2) {
            match g.r.below(3) {
                0 => copies.extend_from_slice(t),
                1 => copies.extend([t[1], t[2], t[0]]),
                _ => copies.extend([t[0], t[2], t[1]]),
            }
        }
    }
    // Ten of them again in another geometry, indexed.
    let again = Mesh {
        vertices: base[..30].to_vec(),
        indices: (0..30).collect(),
    };
    // Alpha-tested pairs: the same triangle as primitives 2k and 2k + 1, of
    // which the alpha pattern takes exactly one.
    let pairs: Vec<P> = g
        .soup(12, 24, 16, 2)
        .chunks(3)
        .flat_map(|t| [t, t].concat())
        .collect();
    // Triangles in one plane that overlap in part.
    let mut plane = Vec::new();
    while plane.len() < 24 {
        let t: [P; 3] =
            std::array::from_fn(|_| [g.r.dyadic(-24, 24, 2), g.r.dyadic(-24, 24, 2), 9.0]);
        if !area_is_zero(&t) {
            plane.extend(t);
        }
    }
    let first = bottom(
        vec![
            soup_geometry(copies, true),
            again.geometry(Idx::U16, None, true),
            soup_geometry(pairs, false),
            soup_geometry(plane, true),
            soup_geometry(g.soup(16, 24, 12, 2), true),
        ],
        false,
        Preference::FastTrace,
    );
    let second = bottom(
        vec![soup_geometry(g.soup(12, 12, 12, 2), true)],
        false,
        Preference::FastTrace,
    );
    // The second bottom level twice in the same place, plain and turned.
    let here = Transform::translation([16.0, 0.0, 0.0]);
    let there = Transform::translation([-16.0, 0.0, 0.0]).after(&Transform::rotation(2, 0.25));
    let scene = top(
        vec![
            instance(Transform::IDENTITY, 10, 0),
            instance(here, 20, 1),
            instance(here, 21, 1),
            instance(there, 30, 1),
            instance(there, 31, 1),
        ],
        vec![first, second],
        false,
    );
    let rays = g.rays(&world_triangles(&scene), 1600, 6);
    (scene, Vec::new(), rays)
}

fn transformed(g: &mut Gen) -> Parts {
    let turned = Transform::translation([1.0, 2.0, -1.0])
        .after(&Transform::rotation(2, 0.375))
        .after(&Transform::scale([1.5, 0.75, 2.0]));
    // A mirror in the geometry's own transform turns its winding.
    let mirrored =
        Transform::translation([5.0, 0.0, 0.0]).after(&Transform::scale([-1.25, 1.0, 1.0]));
    let mut ball = Mesh::octahedron(1.5);
    for v in &mut ball.vertices {
        v[0] -= 5.0;
    }
    let first = bottom(
        vec![
            soup_geometry(g.soup(40, 24, 12, 3), true).with(turned),
            Mesh::cube(2, 1.0, [0.0; 3]).geometry(Idx::U16, Some(mirrored), true),
            ball.geometry(Idx::U32, None, true),
        ],
        false,
        Preference::FastTrace,
    );
    let tilted = Transform::rotation(0, 0.25).after(&Transform::scale([2.0, 2.0, 1.0]));
    let second = bottom(
        vec![
            Mesh::grid(4, 4, [-2.0, -2.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]).geometry(
                Idx::U16,
                Some(tilted),
                true,
            ),
        ],
        false,
        Preference::FastTrace,
    );
    let shear = Transform([
        1.0, 0.5, 0.0, 0.0, 0.0, 1.0, 0.25, -18.0, 0.0, 0.0, 1.0, 0.0,
    ]);
    let ccw = instance_flag::TRIANGLE_FRONT_COUNTERCLOCKWISE;
    let mut instances = vec![
        instance(Transform::IDENTITY, 1, 0),
        instance(
            Transform::translation([18.0, 0.0, 0.0]).after(&Transform::rotation(1, 0.625)),
            2,
            0,
        ),
        instance(
            Transform::translation([-18.0, 0.0, 0.0])
                .after(&Transform::scale([0.5, 2.0, 1.0]))
                .after(&Transform::rotation(2, 0.25)),
            3,
            0,
        ),
        instance(
            Transform::translation([0.0, 18.0, 0.0]).after(&Transform::scale([1.0, 1.0, -1.0])),
            4,
            0,
        ),
        instance(shear, 5, 0),
        instance(Transform::translation([0.0, 0.0, -10.0]), 6, 1),
        instance(
            Transform::translation([0.0, 0.0, 10.0]).after(&Transform::rotation(0, 1.0)),
            7,
            1,
        ),
        instance(
            Transform::translation([18.0, 18.0, 0.0])
                .after(&g.rotation())
                .after(&Transform::scale([-2.0, 0.5, 1.0])),
            8,
            0,
        ),
    ];
    instances[6].flags = ccw;
    instances[7].flags = ccw;
    let scene = top(instances, vec![first, second], false);
    let mut rays = g.rays(&world_triangles(&scene), 1600, 7);
    for ray in &mut rays {
        match g.r.below(6) {
            0 => ray.flags = ray_flag::CULL_BACK_FACING_TRIANGLES,
            1 => ray.flags = ray_flag::CULL_FRONT_FACING_TRIANGLES,
            _ => {}
        }
    }
    (scene, Vec::new(), rays)
}

impl Geometry {
    fn with(mut self, transform: Transform) -> Geometry {
        self.transform = Some(transform);
        self
    }
}

fn hit_ordering(g: &mut Gen) -> Parts {
    // Layers in the planes z = 0, 1, .., each 2 by 2 cells over [-4, 4]^2.
    let stack = |layers: u32| {
        let mut mesh = Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
        };
        for k in 0..layers {
            mesh.append(&Mesh::grid(
                2,
                2,
                [-4.0, -4.0, k as f32],
                [4.0, 0.0, 0.0],
                [0.0, 4.0, 0.0],
            ));
        }
        mesh
    };
    let backstop = Mesh::grid(1, 1, [-6.0, -6.0, 8.0], [12.0, 0.0, 0.0], [0.0, 12.0, 0.0]);
    let first = bottom(
        vec![
            stack(6).geometry(Idx::U16, None, false),
            backstop.geometry(Idx::None, None, true),
        ],
        false,
        Preference::FastTrace,
    );
    let second = bottom(
        vec![stack(3).geometry(Idx::U32, None, true)],
        false,
        Preference::FastTrace,
    );
    let mut instances = vec![
        instance(Transform::IDENTITY, 1, 0),
        instance(
            Transform::translation([16.0, 0.0, 0.0]).after(&Transform::rotation(2, 0.125)),
            2,
            0,
        ),
        instance(Transform::translation([-16.0, 0.0, 0.0]), 3, 1),
        instance(Transform::translation([0.0, 16.0, 0.0]), 4, 0),
    ];
    instances[2].flags = instance_flag::FORCE_NON_OPAQUE;
    instances[3].flags = instance_flag::FORCE_OPAQUE;
    let scene = top(instances, vec![first, second], false);
    // Rays that run through a stack: from below or above a point of it,
    // tilted a little, the range sometimes ending between layers.
    let tris = world_triangles(&scene);
    let mut rays = Vec::new();
    while rays.len() < 1600 {
        let t = *g.r.pick(&tris);
        let target = g.inside(&t);
        let rise = (6.0 + 8.0 * g.r.unit()) * if g.r.chance(1, 2) { 1.0 } else { -1.0 };
        let origin = add(
            target,
            [4.0 * g.r.unit() - 2.0, 4.0 * g.r.unit() - 2.0, rise],
        );
        // The target is at t = 1/2; the far side of the stack before 2.
        let dir = mul(sub(target, origin), 2.0);
        let (tmin, tmax) = match g.r.below(4) {
            0 => (0.0, 0.25 + g.r.unit()),
            1 => (0.25 + 0.5 * g.r.unit(), 1000.0),
            _ => (0.0, 1000.0),
        };
        rays.push(Ray::new(origin, dir, tmin, tmax));
    }
    (scene, Vec::new(), rays)
}

fn precision(g: &mut Gen) -> Parts {
    const FAR: f32 = 1_048_576.0;
    const LIFTED: f32 = 262_144.0;
    let closed = Mesh::cube(4, 2.0, [0.0; 3]);
    // Slivers: a base of 8 and a height from 2^-6 down to 2^-17, each in
    // its own plane.
    let mut slivers = Vec::new();
    for k in 0..12 {
        let (y, z) = (k as f32 - 6.0, 0.25 * k as f32);
        let height = 1.0 / (1u32 << (6 + k)) as f32;
        slivers.extend([[16.0, y, z], [24.0, y, z], [20.0, y + height, z]]);
    }
    // No area: three points on a line, two the same, all the same.
    let mut flat = Vec::new();
    for k in 0..3 {
        let a = [18.0 + 2.0 * k as f32, 0.0, 8.0];
        flat.extend([a, add(a, [1.0, 0.5, 0.25]), add(a, [2.0, 1.0, 0.5])]);
        flat.extend([a, a, add(a, [0.0, -1.0, 0.5])]);
        flat.extend([a, a, a]);
    }
    let scene = top(
        vec![
            instance(Transform::IDENTITY, 1, 0),
            instance(Transform::IDENTITY, 2, 1),
            instance(Transform::IDENTITY, 3, 2),
            instance(Transform::translation([0.0, LIFTED, 0.0]), 4, 0),
        ],
        vec![
            bottom(
                vec![closed.geometry(Idx::U16, None, true)],
                false,
                Preference::FastTrace,
            ),
            bottom(
                vec![
                    soup_geometry(slivers.clone(), true),
                    soup_geometry(flat.clone(), true),
                ],
                false,
                Preference::FastTrace,
            ),
            // The closed mesh again with 2^20 in its vertices.
            bottom(
                vec![Mesh::cube(4, 2.0, [FAR, 0.0, 0.0]).geometry(Idx::U32, None, true)],
                false,
                Preference::FastTrace,
            ),
        ],
        false,
    );

    let mut rays = Vec::new();
    // Each closed mesh with the smallest rise its coordinates can hold.
    for (center, finest) in [([0.0; 3], 8), ([FAR, 0.0, 0.0], 3), ([0.0, LIFTED, 0.0], 5)] {
        // A point of a face in cell units from the face's corner, and a ray
        // through it from `out` cells outside and (`a`, `b`) cells along:
        // every number a small multiple of a power of two, so the ray
        // passes exactly through the point, at t = 1.
        let through = |g: &mut Gen, at: [f32; 2], out: f32, along: [f32; 2]| {
            let k = g.r.below(3) as usize;
            let sign = if g.r.chance(1, 2) { 1.0 } else { -1.0 };
            let (corner, du, dv, normal) = face(k, sign, 2.0, 4, center);
            let q = add(corner, add(mul(du, at[0]), mul(dv, at[1])));
            let o = add(
                q,
                add(mul(normal, out), add(mul(du, along[0]), mul(dv, along[1]))),
            );
            Ray::new(o, sub(q, o), 0.0, 1000.0)
        };
        let cell = |g: &mut Gen| g.r.range(0, 3) as f32;
        let part = |g: &mut Gen| *g.r.pick(&[0.5, 0.25, 0.75, 0.125]);
        let slant = |g: &mut Gen| [g.r.dyadic(-16, 16, 2), g.r.dyadic(-16, 16, 2)];
        let height = |g: &mut Gen| *g.r.pick(&[3.0, 4.0, 8.0]);
        // Through the three kinds of edge: along du, along dv, the diagonal.
        for _ in 0..100 {
            let (i, j, f) = (cell(g), cell(g), part(g));
            let at = match g.r.below(3) {
                0 => [i + f, j + g.r.range(0, 1) as f32],
                1 => [i + g.r.range(0, 1) as f32, j + f],
                _ => [i + 1.0 - f, j + f],
            };
            let (out, along) = (height(g), slant(g));
            rays.push(through(g, at, out, along));
        }
        // Through vertices, the cube's edges and corners among them.
        for _ in 0..70 {
            let at = [g.r.range(0, 4) as f32, g.r.range(0, 4) as f32];
            let (out, along) = (height(g), slant(g));
            rays.push(through(g, at, out, along));
        }
        // Through the inside of a triangle, with the range exactly on the
        // hit, one step either side of it, or wide.
        for n in 0..80 {
            let (i, j) = (cell(g), cell(g));
            let at = if g.r.chance(1, 2) {
                [i + 0.25, j + 0.25]
            } else {
                [i + 0.75, j + 0.5]
            };
            let (out, along) = (height(g), slant(g));
            let mut ray = through(g, at, out, along);
            // A power of two keeps the direction exact; the hit is at 1/k.
            let k = *g.r.pick(&[1.0, 2.0, 0.5]);
            ray.dir = mul(ray.dir, k);
            let t: f32 = 1.0 / k;
            match n % 5 {
                0 => ray.tmax = t,
                1 => ray.tmin = t,
                2 => ray.tmax = t.next_up(),
                3 => ray.tmin = t.next_down(),
                _ => {}
            }
            rays.push(ray);
        }
        // Grazing: a rise of 2^-2 down to 2^-8 over a run of 3 to 6 cells;
        // and a few rays in the face's own plane.
        for n in 0..65 {
            let (i, j) = (cell(g), cell(g));
            let rise = if n < 60 {
                1.0 / (1u32 << g.r.range(2, finest)) as f32
            } else {
                0.0
            };
            let run = g.r.range(3, 6) as f32 * if g.r.chance(1, 2) { 1.0 } else { -1.0 };
            let along = if g.r.chance(1, 2) {
                [run, 0.5]
            } else {
                [0.25, run]
            };
            rays.push(through(g, [i + 0.25, j + 0.25], rise, along));
        }
    }
    // The slivers: through their inside, and exactly through their long
    // edge.
    for n in 0..150 {
        let k = g.r.below(12) as usize;
        let t = [slivers[3 * k], slivers[3 * k + 1], slivers[3 * k + 2]];
        let target = if n % 3 == 0 {
            add(t[0], [g.r.dyadic(1, 31, 2), 0.0, 0.0])
        } else {
            g.inside(&t)
        };
        let origin = add(
            target,
            [
                g.r.dyadic(-8, 8, 2),
                g.r.dyadic(-8, 8, 2),
                if g.r.chance(1, 2) { 4.0 } else { -4.0 },
            ],
        );
        rays.push(Ray::new(origin, sub(target, origin), 0.0, 1000.0));
    }
    // The triangles of no area: through a point of each line, and beside.
    for n in 0..40 {
        let k = g.r.below(9) as usize;
        let a = flat[3 * k];
        let off = if n % 2 == 0 {
            [0.0; 3]
        } else {
            [0.0, 0.25, 0.0]
        };
        let target = add(add(a, [1.0, 0.5, 0.25]), off);
        let origin = add(target, [g.r.dyadic(-8, 8, 2), g.r.dyadic(-8, 8, 2), 4.0]);
        rays.push(Ray::new(origin, sub(target, origin), 0.0, 1000.0));
    }
    // And rays with nothing special about them.
    let tris = world_triangles(&scene);
    for center in [
        [0.0; 3],
        [FAR, 0.0, 0.0],
        [0.0, LIFTED, 0.0],
        [20.0, 0.0, 4.0],
    ] {
        let near: Vec<[P; 3]> = tris
            .iter()
            .copied()
            .filter(|t| (0..3).all(|k| (t[0][k] - center[k]).abs() <= 12.0))
            .collect();
        rays.extend(g.rays(&near, 150, 7));
    }
    (scene, Vec::new(), rays)
}

fn masks_and_flags(g: &mut Gen) -> Parts {
    let cube = bottom(
        vec![Mesh::cube(1, 1.0, [0.0; 3]).geometry(Idx::U16, None, true)],
        false,
        Preference::FastTrace,
    );
    let panel = bottom(
        vec![
            Mesh::grid(2, 2, [-2.0, -2.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]).geometry(
                Idx::U16,
                None,
                false,
            ),
            soup_geometry(g.soup(6, 16, 12, 3), true),
        ],
        false,
        Preference::FastTrace,
    );
    let mixed = bottom(
        vec![
            soup_geometry(g.soup(10, 16, 16, 3), true),
            soup_geometry(g.soup(10, 16, 16, 3), false),
        ],
        false,
        Preference::FastTrace,
    );
    let masks = [0xFF, 0xFF, 0x01, 0x02, 0x04, 0x80, 0x10, 0x03, 0x00];
    let mut instances = Vec::new();
    for i in 0..24u32 {
        let at = [
            (i % 4) as f32 * 7.0 - 10.5,
            (i / 4 % 3) as f32 * 7.0 - 7.0,
            (i / 12) as f32 * 7.0 - 3.5,
        ];
        let mut flags = g.r.below(16);
        if flags & 0xC == 0xC {
            flags &= !instance_flag::FORCE_NON_OPAQUE;
        }
        instances.push(Instance {
            transform: Transform::translation(at).after(&g.rotation()),
            id: g.r.below(1 << 24),
            mask: *g.r.pick(&masks),
            flags,
            blas: i % 3,
        });
    }
    let scene = top(instances, vec![cube, panel, mixed], false);
    let mut rays = g.rays(&world_triangles(&scene), 2000, 7);
    for ray in &mut rays {
        ray.mask = match g.r.below(10) {
            0..=3 => 0xFF,
            4 | 5 => g.r.below(256),
            6 => 0x01,
            7 => 0x80,
            // Bits above the low eight are not looked at.
            8 => 0xFFFF_FF00 | g.r.below(256),
            _ => 0,
        };
        ray.flags = match g.r.below(8) {
            0 => ray_flag::FORCE_OPAQUE,
            1 => ray_flag::FORCE_NON_OPAQUE,
            2 => ray_flag::CULL_OPAQUE,
            3 => ray_flag::CULL_NON_OPAQUE,
            _ => 0,
        };
        ray.flags |= match g.r.below(4) {
            0 => ray_flag::CULL_BACK_FACING_TRIANGLES,
            1 => ray_flag::CULL_FRONT_FACING_TRIANGLES,
            _ => 0,
        };
        if g.r.chance(1, 5) {
            ray.flags |= ray_flag::ACCEPT_FIRST_HIT_AND_END_SEARCH;
        }
        if g.r.chance(1, 8) {
            ray.flags |= ray_flag::SKIP_CLOSEST_HIT_SHADER;
        }
        if g.r.chance(1, 40) {
            // Not allowed together with a facing cull.
            ray.flags &=
                !(ray_flag::CULL_BACK_FACING_TRIANGLES | ray_flag::CULL_FRONT_FACING_TRIANGLES);
            ray.flags |= ray_flag::SKIP_TRIANGLES;
        } else if g.r.chance(1, 8) {
            ray.flags |= ray_flag::SKIP_PROCEDURAL_PRIMITIVES;
        }
    }
    (scene, Vec::new(), rays)
}

// ---- the classes of the proposed rule (docs/dxr-validation.md, "Scenes") ----

/// `x` moved `k` representable numbers up, or down for a negative `k`: `k`
/// spacings.
fn stepped(mut x: f32, k: i32) -> f32 {
    for _ in 0..k.abs() {
        x = if k > 0 { x.next_up() } else { x.next_down() };
    }
    x
}

fn largest(p: P) -> f32 {
    p[0].abs().max(p[1].abs()).max(p[2].abs())
}

fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn one_instance(geometries: Vec<Geometry>, id: u32) -> Scene {
    top(
        vec![instance(Transform::IDENTITY, id, 0)],
        vec![bottom(geometries, false, Preference::FastTrace)],
        false,
    )
}

/// The offsets, in spacings, of the directed classes' rays from the edge
/// they are aimed at.
const SPACINGS: [i32; 7] = [0, 1, -1, 2, -2, 8, -8];

/// The rays of the single class: twice the 1,024 the rule's bound counts
/// in, so that the few with a sliver or grazing pair, which are taken out
/// first, do not leave fewer than the bound can be counted on.
const SINGLE_RAYS: usize = 2048;

fn single(g: &mut Gen) -> Parts {
    let scene = one_instance(vec![soup_geometry(g.soup(1, 16, 24, 3), true)], 1);
    let rays = g.rays(&world_triangles(&scene), SINGLE_RAYS, 6);
    (scene, Vec::new(), rays)
}

/// The six faces of a cube, each a geometry of its own: six surfaces around
/// one closed mesh.
fn cube_faces(n: u32, half: f32, center: P) -> Vec<Geometry> {
    let mut out = Vec::new();
    for k in 0..3 {
        for sign in [1.0, -1.0] {
            let (origin, du, dv, _) = face(k, sign, half, n, center);
            out.push(Mesh::grid(n, n, origin, du, dv).geometry(Idx::U16, None, true));
        }
    }
    out
}

/// Where the closed class's meshes are, each with its scale and bottom
/// level; ray `700 + n` of the closed class and ray `n` of the closed-edge
/// class start inside mesh `n % 4`.
const CLOSED: [(P, f32, u32); 4] = [
    ([-6.0, 0.0, 0.0], 1.0, 0),
    ([7.0, 1.0, 0.0], 2.0, 0),
    ([0.0, 9.0, 1.0], 1.0, 1),
    ([0.0, -9.0, 2.0], 0.5, 1),
];

/// The rays of the closed class that start outside its meshes; the rest
/// start inside one.
pub const CLOSED_OUTSIDE: usize = 700;

/// The rays that start inside a mesh, in the closed class after those from
/// outside and in the closed-edge class alone.
const CLOSED_INSIDE: usize = 700;

/// Two cubes of six surfaces each and two octahedra, moved and scaled.
fn closed_scene() -> Scene {
    let boxes = bottom(cube_faces(3, 2.0, [0.0; 3]), false, Preference::FastTrace);
    let ball = bottom(
        vec![Mesh::octahedron(3.0).geometry(Idx::U16, None, true)],
        false,
        Preference::FastTrace,
    );
    let instances = CLOSED
        .iter()
        .enumerate()
        .map(|(i, &(at, s, blas))| {
            instance(
                Transform::translation(at).after(&Transform::scale([s; 3])),
                1 + i as u32,
                blas,
            )
        })
        .collect();
    top(instances, vec![boxes, ball], false)
}

fn closed(g: &mut Gen) -> Parts {
    let scene = closed_scene();
    let mut rays = g.rays(&world_triangles(&scene), CLOSED_OUTSIDE, 7);
    // From inside a mesh, any way: none may leave it unseen. Random as the
    // rays from outside are: an origin within half a unit of the mesh's
    // middle (times its scale) and a direction, each coordinate of 24
    // random bits, so that no ray is aimed at an edge. The rays that are
    // are the closed-edge class's.
    for n in 0..CLOSED_INSIDE {
        let (at, s, _) = CLOSED[n % 4];
        let origin = add(at, mul(std::array::from_fn(|_| g.r.unit() - 0.5), s));
        let mut dir: P = std::array::from_fn(|_| 2.0 * g.r.unit() - 1.0);
        if dir == [0.0; 3] {
            dir = [0.0, 0.0, 1.0];
        }
        rays.push(Ray::new(origin, dir, 0.0, 1000.0));
    }
    (scene, Vec::new(), rays)
}

/// The closed class's meshes, and rays from inside them on a lattice:
/// origins on multiples of 1/8 (times the mesh's scale) within half a unit
/// of its middle, directions on multiples of 1/4 up to 4. Many go exactly
/// through an edge between two faces or through a vertex; a random class
/// has no such rays, and these were the closed class's own until its
/// count of ambiguous rays was seen to be theirs.
fn closed_edge(g: &mut Gen) -> Parts {
    let mut rays = Vec::new();
    for n in 0..CLOSED_INSIDE {
        let (at, s, _) = CLOSED[n % 4];
        let origin = add(at, mul(g.point(4, 3), s));
        let mut dir = g.point(16, 2);
        if dir == [0.0; 3] {
            dir = [0.0, 0.0, 1.0];
        }
        rays.push(Ray::new(origin, dir, 0.0, 1000.0));
    }
    (closed_scene(), Vec::new(), rays)
}

/// `count` triangles within 8 units of `center`, with edges whose largest
/// component is 2^-4 to 2^4 units; every coordinate a multiple of 2^-7,
/// which `f32` holds up to 2^17.
fn far_soup(g: &mut Gen, center: P, count: usize) -> Vec<P> {
    let mut out = Vec::new();
    while out.len() < 3 * count {
        let a = add(center, g.point(64, 3));
        let mut t = [a; 3];
        for v in t.iter_mut().skip(1) {
            let size = (1u32 << g.r.range(0, 8)) as f32 / 16.0;
            let mut e = g.point(8, 3);
            let k = g.r.below(3) as usize;
            e[k] = if g.r.chance(1, 2) { 1.0 } else { -1.0 };
            *v = add(a, mul(e, size));
        }
        if !area_is_zero(&t) {
            out.extend(t);
        }
    }
    out
}

/// The far class's triangles a cluster, and its rays a cluster.
const FAR_TRIANGLES: usize = 40;
const FAR_RAYS: usize = 300;

fn far(g: &mut Gen) -> Parts {
    let clusters: [P; 4] = [
        [4096.0, 0.0, 0.0],
        [0.0, 16_384.0, 0.0],
        [0.0, 0.0, 65_536.0],
        [65_536.0, -65_536.0, 65_536.0],
    ];
    let geometries = clusters
        .iter()
        .map(|c| soup_geometry(far_soup(g, *c, FAR_TRIANGLES), true))
        .collect();
    // And a cluster about its own origin that an instance carries far.
    let carried = soup_geometry(far_soup(g, [0.0; 3], FAR_TRIANGLES), true);
    let scene = top(
        vec![
            instance(Transform::IDENTITY, 1, 0),
            instance(Transform::translation([-16_384.0, 0.0, 0.0]), 2, 1),
        ],
        vec![
            bottom(geometries, false, Preference::FastTrace),
            bottom(vec![carried], false, Preference::FastTrace),
        ],
        false,
    );
    let mut rays = Vec::new();
    for cluster in world_triangles(&scene).chunks(FAR_TRIANGLES) {
        rays.extend(g.rays(cluster, FAR_RAYS, 7));
    }
    (scene, Vec::new(), rays)
}

/// One half of the cube of half-width 2 about the origin, two cells a face,
/// cut by the plane x = 0: the triangles on the side of `sign`, moved by
/// `shift` in their vertices. There is no wall at the cut.
fn half_cube(sign: f32, shift: P) -> Mesh {
    let cube = Mesh::cube(2, 2.0, [0.0; 3]);
    let mut mesh = Mesh {
        vertices: cube.vertices.iter().map(|v| add(*v, shift)).collect(),
        indices: Vec::new(),
    };
    for t in cube.indices.chunks(3) {
        if t.iter()
            .all(|&i| cube.vertices[i as usize][0] * sign >= 0.0)
        {
            mesh.indices.extend(t);
        }
    }
    mesh
}

/// The seam classes' scene: six cubes, each cut in two at its plane x = 0
/// and put together again in another way, and for each the transform that
/// carries the cube's own coordinates to the world. In order:
///
/// 0. two bottom levels under one transform (a translation);
/// 1. two bottom levels under one transform (a rotation and a translation);
/// 2. one bottom level twice, plain and mirrored: different transforms;
/// 3. two bottom levels, one with its vertices 16 to the side and its
///    instance 16 back: different transforms;
/// 4. one bottom level, the second half 16 to the side in its vertices and
///    back by a geometry transform;
/// 5. one bottom level, the halves two geometries with no transform: no
///    seam, the edge D3D12 promises closed, between two surfaces.
///
/// The halves of a cube have one instance id; in 0 to 3 they are one
/// surface, in 4 and 5 two (two geometries).
fn seam_scene() -> (Scene, [Transform; 6]) {
    let geo = |m: &Mesh| m.geometry(Idx::None, None, true);
    let (left, right) = (half_cube(-1.0, [0.0; 3]), half_cube(1.0, [0.0; 3]));
    let aside = half_cube(-1.0, [16.0, 0.0, 0.0]);
    let back = Transform::translation([-16.0, 0.0, 0.0]);
    let one = |geometries| bottom(geometries, false, Preference::FastTrace);
    let bottoms = vec![
        one(vec![geo(&left)]),
        one(vec![geo(&right)]),
        one(vec![geo(&aside)]),
        one(vec![geo(&right), geo(&aside).with(back)]),
        one(vec![geo(&left), geo(&right)]),
    ];
    let at = |v: usize| {
        Transform::translation([
            [-24.0, -8.0, 8.0][v % 3],
            if v < 3 { -8.0 } else { 8.0 },
            4.0,
        ])
    };
    let turned = at(1).after(&Transform::rotation(2, 0.5));
    let mirrored = at(2).after(&Transform::scale([-1.0, 1.0, 1.0]));
    let instances = vec![
        instance(at(0), 10, 0),
        instance(at(0), 10, 1),
        instance(turned, 11, 0),
        instance(turned, 11, 1),
        instance(at(2), 12, 1),
        instance(mirrored, 12, 1),
        instance(at(3), 13, 1),
        instance(at(3).after(&back), 13, 2),
        instance(at(4), 14, 3),
        instance(at(5), 15, 4),
    ];
    (
        top(instances, bottoms, false),
        [at(0), turned, at(2), at(3), at(4), at(5)],
    )
}

fn seam(g: &mut Gen) -> Parts {
    let (scene, _) = seam_scene();
    let rays = g.rays(&world_triangles(&scene), 1536, 7);
    (scene, Vec::new(), rays)
}

fn seam_edge(g: &mut Gen) -> Parts {
    let (scene, frames) = seam_scene();
    let mut rays = Vec::new();
    for frame in &frames {
        // The seam crosses the four faces that are not cut off: across y
        // and across z, each way.
        for (m, sign) in [(1usize, 1.0f32), (1, -1.0), (2, 1.0), (2, -1.0)] {
            let q = 3 - m;
            for k in SPACINGS {
                for _ in 0..6 {
                    // A point of the seam in the cube's own coordinates, one
                    // time in six the vertex in the middle of the face; and
                    // an origin off the face, slanted along the seam only.
                    let mut p = [0.0; 3];
                    p[m] = 2.0 * sign;
                    p[q] = if g.r.chance(1, 6) {
                        0.0
                    } else {
                        g.r.dyadic(-15, 15, 3)
                    };
                    let mut o = p;
                    o[m] += sign * *g.r.pick(&[3.0, 4.0, 8.0]);
                    o[q] += g.r.dyadic(-16, 16, 2);
                    let (mut target, mut origin) = (frame.apply(p), frame.apply(o));
                    // `k` spacings off the seam, the ray kept parallel.
                    let moved = stepped(target[0], k);
                    origin[0] += moved - target[0];
                    target[0] = moved;
                    rays.push(Ray::new(origin, sub(target, origin), 0.0, 1000.0));
                }
            }
        }
    }
    (scene, Vec::new(), rays)
}

/// The unit of the surface-rays class's biases, as a share of the largest
/// coordinate at the surface point: 8 spacings of `f32`. The proposed rule
/// asks for biases in tolerances. The generator cannot take the rule's
/// constant, which is measured on this class among others; the unit is
/// fixed here at what that measurement gives, and a host test fails when
/// the two part (`tests/rule.rs`).
pub const BIAS_UNIT: f32 = 8.0 / 8_388_608.0;
/// The biases, in units. Ray `n` has bias `BIASES[n % 4]`, and leaves a
/// surface when `n / 4` is even and ends on one when it is odd.
pub const BIASES: [f32; 4] = [0.0, 1.0, 16.0, 256.0];

fn surface_rays(g: &mut Gen) -> Parts {
    let ground = Mesh::grid(8, 8, [-8.0, -8.0, -9.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
    let scene = one_instance(
        vec![
            soup_geometry(g.soup(96, 64, 24, 3), true),
            ground.geometry(Idx::U16, None, true),
            Mesh::cube(2, 2.0, [3.0, -2.0, 5.0]).geometry(Idx::U16, None, true),
        ],
        7,
    );
    let tris = world_triangles(&scene);
    let (center, half) = extent(&tris);
    let mut rays = Vec::new();
    for n in 0..1600 {
        let t = *g.r.pick(&tris);
        let p = g.inside(&t);
        // The normal with its largest component made 1: the bias is along
        // it, and between 1 and 3^(1/2) times its nominal length.
        let normal = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        let normal = mul(normal, 1.0 / largest(normal));
        let m = t.iter().fold(largest(p), |m, v| m.max(largest(*v)));
        let bias = BIASES[n % 4] * BIAS_UNIT * m;
        let side = if g.r.chance(1, 2) { 1.0 } else { -1.0 };
        if n / 4 % 2 == 0 {
            // Leaves the surface: from the point lifted by the bias, on
            // the side it is lifted to.
            let origin = add(p, mul(normal, side * bias));
            let mut dir = g.point(16, 2);
            let along = dir[0] * normal[0] + dir[1] * normal[1] + dir[2] * normal[2];
            if along * side < 0.0 {
                dir = mul(dir, -1.0);
            } else if along == 0.0 {
                dir = mul(normal, 4.0 * side);
            }
            rays.push(Ray::new(origin, dir, 0.0, 1000.0));
        } else {
            // Ends on the surface: toward the point from outside, the
            // extent the bias short of it or past it.
            let origin = g.shell(center, 1.25 * half, 3.0 * half);
            let dir = sub(p, origin);
            rays.push(Ray::new(origin, dir, 0.0, 1.0 + side * bias / largest(dir)));
        }
    }
    (scene, Vec::new(), rays)
}

fn scale(g: &mut Gen) -> Parts {
    let scene = one_instance(vec![soup_geometry(g.soup(2048, 256, 16, 3), true)], 1);
    let tris = world_triangles(&scene);
    let mut rays = g.rays(&tris, 768, 6);
    // A coherent bundle: one origin, a grid of targets across the scene.
    let (center, half) = extent(&tris);
    let origin = add(center, [2.5 * half, -2.0 * half, 3.0 * half]);
    for n in 0..768u32 {
        let (i, j) = ((n % 32) as f32, (n / 32) as f32);
        let target = add(
            center,
            [
                0.0,
                half * (i / 16.0 - 0.96875),
                half * (j / 12.0 - 0.96875),
            ],
        );
        rays.push(Ray::new(origin, sub(target, origin), 0.0, 1000.0));
    }
    (scene, Vec::new(), rays)
}

/// The edge classes: about `at`, in the plane of its z, a square of two
/// triangles that share its diagonal and a fan of six about a hub; and an
/// octahedron 20 to the side, carried there by its instance. Rays through
/// points of every edge and through every vertex, exactly and 1, 2 and 8
/// spacings off; `grazing` makes them shallow.
fn edge(g: &mut Gen, at: P, grazing: bool) -> Parts {
    let flat = |x: f32, y: f32| add(at, [x, y, 0.0]);
    let square = Mesh::grid(1, 1, at, [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]);
    let hub = flat(10.0, 2.0);
    let rim = [
        flat(13.0, 2.0),
        flat(12.0, 4.0),
        flat(8.0, 4.0),
        flat(7.0, 2.0),
        flat(8.0, 0.0),
        flat(12.0, 0.0),
    ];
    let mut fan = Mesh {
        vertices: vec![hub],
        indices: Vec::new(),
    };
    fan.vertices.extend(rim);
    for i in 0..6u32 {
        fan.indices.extend([0, 1 + i, 1 + (i + 1) % 6]);
    }
    let ball_at = add(at, [20.0, 0.0, 0.0]);
    let scene = top(
        vec![
            instance(Transform::IDENTITY, 1, 0),
            instance(Transform::translation(ball_at), 2, 1),
        ],
        vec![
            bottom(
                vec![
                    square.geometry(Idx::U16, None, true),
                    fan.geometry(Idx::U16, None, true),
                ],
                false,
                Preference::FastTrace,
            ),
            bottom(
                vec![Mesh::octahedron(2.0).geometry(Idx::U16, None, true)],
                false,
                Preference::FastTrace,
            ),
        ],
        false,
    );
    // The flat shapes' edges, and their vertices as edges of no length.
    let (a, b, c, d) = (
        flat(0.0, 0.0),
        flat(4.0, 0.0),
        flat(0.0, 4.0),
        flat(4.0, 4.0),
    );
    let mut edges = vec![(b, c), (a, b), (a, c), (b, d), (c, d)];
    edges.extend([a, b, c, d, hub].map(|v| (v, v)));
    for i in 0..6 {
        edges.extend([(hub, rim[i]), (rim[i], rim[(i + 1) % 6]), (rim[i], rim[i])]);
    }
    let mut rays = Vec::new();
    for (from, to) in edges {
        // The ray is moved off the edge across it: along y for an edge
        // along x, along x otherwise.
        let across = usize::from(from[1] == to[1] && from[0] != to[0]);
        for k in SPACINGS {
            for _ in 0..3 {
                let f = if from == to { 0.0 } else { g.r.dyadic(1, 3, 2) };
                let mut target = add(from, mul(sub(to, from), f));
                let offset = if grazing {
                    let run = *g.r.pick(&[4.0, 6.0, 8.0, -4.0, -6.0, -8.0]);
                    let rise = *g.r.pick(&[0.5, 0.25, 0.125, 0.0625]);
                    if g.r.chance(1, 2) {
                        [run, 0.5, rise]
                    } else {
                        [0.5, run, -rise]
                    }
                } else {
                    let rise = *g.r.pick(&[3.0, 4.0, 8.0, -3.0, -4.0, -8.0]);
                    [g.r.dyadic(-16, 16, 2), g.r.dyadic(-16, 16, 2), rise]
                };
                let mut origin = add(target, offset);
                let moved = stepped(target[across], k);
                origin[across] += moved - target[across];
                target[across] = moved;
                rays.push(Ray::new(origin, sub(target, origin), 0.0, 1000.0));
            }
        }
    }
    // The octahedron: its twelve edges and six vertices, from outside.
    let corner = |k: usize| axis(k / 2, if k.is_multiple_of(2) { 2.0 } else { -2.0 });
    let mut features = Vec::new();
    for i in 0..6 {
        features.push((corner(i), corner(i)));
        for j in i + 1..6 {
            if i / 2 != j / 2 {
                features.push((corner(i), corner(j)));
            }
        }
    }
    for (from, to) in features {
        // The edge lies in a coordinate plane: off it along that plane's
        // normal; a vertex along x.
        let across = (0..3)
            .find(|&k| from != to && from[k] == 0.0 && to[k] == 0.0)
            .unwrap_or(0);
        for k in SPACINGS {
            for _ in 0..3 {
                let f = if from == to { 0.0 } else { g.r.dyadic(1, 3, 2) };
                let local = add(from, mul(sub(to, from), f));
                let out = if grazing {
                    add(mul(local, 0.125), mul(sub(to, from), g.r.dyadic(4, 12, 2)))
                } else {
                    add(
                        mul(local, *g.r.pick(&[1.0, 2.0])),
                        mul(sub(to, from), g.r.dyadic(-4, 4, 2)),
                    )
                };
                let mut target = add(ball_at, local);
                let mut origin = add(target, out);
                let moved = stepped(target[across], k);
                origin[across] += moved - target[across];
                target[across] = moved;
                rays.push(Ray::new(origin, sub(target, origin), 0.0, 1000.0));
            }
        }
    }
    (scene, Vec::new(), rays)
}

fn thin(g: &mut Gen) -> Parts {
    // A base of 8 and a height from 1 down to 2^-17, each in its own
    // plane: a sliver (two long edges that nearly meet) and a needle (one
    // short edge).
    let mut tris = Vec::new();
    for k in 0..18 {
        let (y, z) = (k as f32 - 9.0, 0.25 * k as f32);
        let height = 1.0 / (1u32 << k) as f32;
        tris.extend([[16.0, y, z], [24.0, y, z], [20.0, y + height, z]]);
        tris.extend([[28.0, y, z], [36.0, y, z], [36.0, y + height, z]]);
    }
    let scene = one_instance(vec![soup_geometry(tris.clone(), true)], 1);
    let mut rays = Vec::new();
    for n in 0..720 {
        let k = g.r.below(36) as usize;
        let t = [tris[3 * k], tris[3 * k + 1], tris[3 * k + 2]];
        let target = match n % 3 {
            // Exactly on the long edge, exactly through a vertex, inside.
            0 => add(t[0], [g.r.dyadic(1, 31, 2), 0.0, 0.0]),
            1 => *g.r.pick(&t),
            _ => g.inside(&t),
        };
        let origin = add(
            target,
            [
                g.r.dyadic(-8, 8, 2),
                g.r.dyadic(-8, 8, 2),
                if g.r.chance(1, 2) { 4.0 } else { -4.0 },
            ],
        );
        rays.push(Ray::new(origin, sub(target, origin), 0.0, 1000.0));
    }
    (scene, Vec::new(), rays)
}

fn large_thin(g: &mut Gen) -> Parts {
    // Each cluster's centre and the spacing of `f32` there: every
    // coordinate is a whole number of spacings.
    let clusters: [(P, f32); 3] = [
        ([65_536.0, 0.0, 0.0], 1.0 / 128.0),
        ([0.0, 1_048_576.0, 0.0], 0.125),
        ([0.0, 0.0, 16_777_216.0], 2.0),
    ];
    let mut geometries = Vec::new();
    let mut all = Vec::new();
    for (center, sp) in clusters {
        let mut tris = Vec::new();
        while tris.len() < 3 * 32 {
            let a = add(center, mul(g.point(256, 0), sp));
            let t = [
                a,
                add(a, mul(g.point(16, 0), sp)),
                add(a, mul(g.point(16, 0), sp)),
            ];
            if !area_is_zero(&t) {
                tris.extend(t);
            }
        }
        all.push((tris.clone(), sp));
        geometries.push(soup_geometry(tris, true));
    }
    let scene = one_instance(geometries, 1);
    let mut rays = Vec::new();
    for (tris, sp) in all {
        for _ in 0..240 {
            let k = g.r.below(32) as usize;
            let target = g.inside(&[tris[3 * k], tris[3 * k + 1], tris[3 * k + 2]]);
            let rise = g.r.range(64, 256) as f32 * if g.r.chance(1, 2) { 1.0 } else { -1.0 };
            let off = [g.r.dyadic(-64, 64, 0), g.r.dyadic(-64, 64, 0), rise];
            let origin = add(target, mul(off, sp));
            rays.push(Ray::new(origin, sub(target, origin), 0.0, 1000.0));
        }
    }
    (scene, Vec::new(), rays)
}

fn cull_edge(g: &mut Gen) -> Parts {
    // A square whose two triangles are wound against each other: from
    // either side one is a front face and the other a back face.
    let at = [2.0, 3.0, 1.0];
    let mut square = Mesh::grid(1, 1, at, [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]);
    square.indices.swap(4, 5);
    let scene = one_instance(vec![square.geometry(Idx::U16, None, true)], 1);
    let culls = [
        0,
        ray_flag::CULL_BACK_FACING_TRIANGLES,
        ray_flag::CULL_FRONT_FACING_TRIANGLES,
    ];
    let from = |g: &mut Gen, target: P, up: f32| {
        let rise = up * *g.r.pick(&[3.0, 4.0, 8.0]);
        add(target, [g.r.dyadic(-8, 8, 2), g.r.dyadic(-8, 8, 2), rise])
    };
    let mut rays = Vec::new();
    for flags in culls {
        for up in [1.0, -1.0] {
            for k in SPACINGS {
                for _ in 0..6 {
                    // On the diagonal the two share, and off it.
                    let f = g.r.dyadic(1, 7, 3);
                    let mut target = add(at, [4.0 - 4.0 * f, 4.0 * f, 0.0]);
                    let mut origin = from(g, target, up);
                    let moved = stepped(target[0], k);
                    origin[0] += moved - target[0];
                    target[0] = moved;
                    let mut ray = Ray::new(origin, sub(target, origin), 0.0, 1000.0);
                    ray.flags = flags;
                    rays.push(ray);
                }
            }
        }
    }
    let tris = world_triangles(&scene);
    for n in 0..300 {
        let t = *g.r.pick(&tris);
        let target = g.inside(&t);
        let origin = from(g, target, if n % 2 == 0 { 1.0 } else { -1.0 });
        let mut ray = Ray::new(origin, sub(target, origin), 0.0, 1000.0);
        ray.flags = culls[n % 3];
        rays.push(ray);
    }
    (scene, Vec::new(), rays)
}

fn extent_class(g: &mut Gen) -> Parts {
    let near = Mesh::grid(2, 2, [-4.0, -4.0, 2.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]);
    let behind = Mesh::grid(1, 1, [-6.0, -6.0, 6.0], [12.0, 0.0, 0.0], [0.0, 12.0, 0.0]);
    let scene = one_instance(
        vec![
            near.geometry(Idx::U16, None, true),
            behind.geometry(Idx::None, None, true),
        ],
        1,
    );
    let mut rays = Vec::new();
    for n in 0..600 {
        // A point well inside a triangle of the near square, met at
        // t = 1/k exactly: every number a small multiple of a power of two.
        let (fx, fy) = *g.r.pick(&[(0.25, 0.25), (0.75, 0.5), (0.5, 0.75)]);
        let cell = [g.r.range(0, 1) as f32, g.r.range(0, 1) as f32];
        let target = [
            -4.0 + 4.0 * (cell[0] + fx),
            -4.0 + 4.0 * (cell[1] + fy),
            2.0,
        ];
        // From the near square's side: the other is behind it.
        let rise = -*g.r.pick(&[3.0, 4.0, 8.0]);
        let mut origin = add(target, [g.r.dyadic(-8, 8, 2), g.r.dyadic(-8, 8, 2), rise]);
        let k = *g.r.pick(&[1.0, 2.0, 0.5]);
        let dir = mul(sub(target, origin), k);
        let t: f32 = 1.0 / k;
        let (mut tmin, mut tmax) = (0.0, 1000.0);
        match n % 10 {
            0 => tmax = t,
            1 => tmax = t.next_up(),
            2 => tmax = t.next_down(),
            3 => tmin = t,
            4 => tmin = t.next_down(),
            5 => tmin = t.next_up(),
            // From the surface itself: a hit at exactly t = 0.
            6 => origin = target,
            7 => (origin, tmin) = (target, -0.0),
            8 => tmax = f32::INFINITY,
            _ => tmax = f32::MAX,
        }
        rays.push(Ray::new(origin, dir, tmin, tmax));
    }
    (scene, Vec::new(), rays)
}

fn tie(g: &mut Gen) -> Parts {
    // The same triangles twice in one geometry (one surface), and once in
    // each of two geometries (two surfaces).
    let twice: Vec<P> = g
        .soup(8, 24, 16, 2)
        .chunks(3)
        .flat_map(|t| [t, t].concat())
        .collect();
    let shared = g.soup(8, 24, 16, 2);
    // Squares in parallel planes 1, 2, 4 and 8 spacings apart: both of one
    // geometry, and one of each of two.
    let empty = || Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let (mut both, mut lower, mut upper) = (empty(), empty(), empty());
    for (n, k) in [1, 2, 4, 8].into_iter().enumerate() {
        let x = 16.0 + 6.0 * n as f32;
        let layer = |y: f32, z: f32| Mesh::grid(1, 1, [x, y, z], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]);
        both.append(&layer(-10.0, 3.0));
        both.append(&layer(-10.0, stepped(3.0, k)));
        lower.append(&layer(2.0, 3.0));
        upper.append(&layer(2.0, stepped(3.0, k)));
    }
    let scene = one_instance(
        vec![
            soup_geometry(twice, true),
            soup_geometry(shared.clone(), true),
            soup_geometry(shared, true),
            both.geometry(Idx::U16, None, true),
            lower.geometry(Idx::U16, None, true),
            upper.geometry(Idx::U16, None, true),
        ],
        1,
    );
    let rays = g.rays(&world_triangles(&scene), 800, 7);
    (scene, Vec::new(), rays)
}

fn flat(g: &mut Gen) -> Parts {
    let center = [3.0, -5.0, 2.0];
    let ground = Mesh::grid(4, 4, [-8.0, -16.0, -4.0], [6.0, 0.0, 0.0], [0.0, 6.0, 0.0]);
    let scene = one_instance(
        vec![
            Mesh::cube(2, 2.0, center).geometry(Idx::U16, None, true),
            ground.geometry(Idx::U16, None, true),
        ],
        1,
    );
    let mut rays = Vec::new();
    // Along an axis, at a face, through a point on its quarter-cell grid:
    // on a line of the mesh one time in four either way.
    for _ in 0..500 {
        let k = g.r.below(3) as usize;
        let sign = if g.r.chance(1, 2) { 1.0 } else { -1.0 };
        let mut origin = center;
        origin[(k + 1) % 3] += g.r.dyadic(-4, 4, 1);
        origin[(k + 2) % 3] += g.r.dyadic(-4, 4, 1);
        origin[k] += sign * g.r.range(3, 9) as f32;
        rays.push(Ray::new(origin, axis(k, -sign), 0.0, 1000.0));
    }
    // In a face's own plane, from outside the cube toward it.
    for _ in 0..300 {
        let k = g.r.below(3) as usize;
        let (b, c) = ((k + 1) % 3, (k + 2) % 3);
        let side = if g.r.chance(1, 2) { 1.0 } else { -1.0 };
        let mut origin = center;
        origin[k] += if g.r.chance(1, 2) { 2.0 } else { -2.0 };
        origin[b] += side * g.r.range(3, 6) as f32;
        origin[c] += g.r.dyadic(-8, 8, 2);
        let mut dir = [0.0; 3];
        dir[b] = -side * g.r.range(1, 4) as f32;
        dir[c] = g.r.dyadic(-4, 4, 2);
        rays.push(Ray::new(origin, dir, 0.0, 1000.0));
    }
    (scene, Vec::new(), rays)
}

fn degenerate(g: &mut Gen) -> Parts {
    // No area: three points on a line, two the same, all the same.
    let mut lines = Vec::new();
    let mut none = Vec::new();
    for k in 0..4 {
        let a = [2.0 + 3.0 * k as f32, 1.0, 4.0];
        lines.push(a);
        none.extend([a, add(a, [1.0, 0.5, 0.25]), add(a, [2.0, 1.0, 0.5])]);
        none.extend([a, a, add(a, [0.0, -1.0, 0.5])]);
        none.extend([a, a, a]);
    }
    // The same under a geometry transform that is not a power of two: what
    // a build makes of them need not be on a line any more.
    let turned = Transform::translation([0.0, 9.0, 0.0]).after(&Transform::rotation(2, 0.375));
    let backdrop = Mesh::grid(
        2,
        2,
        [-24.0, -24.0, 8.0],
        [24.0, 0.0, 0.0],
        [0.0, 24.0, 0.0],
    );
    let tilted = Transform::translation([0.0, -30.0, 0.0]).after(&Transform::rotation(0, 0.25));
    let scene = top(
        vec![instance(Transform::IDENTITY, 1, 0), instance(tilted, 2, 0)],
        vec![bottom(
            vec![
                soup_geometry(none.clone(), true),
                soup_geometry(none, true).with(turned),
                backdrop.geometry(Idx::U16, None, true),
                soup_geometry(g.soup(12, 64, 24, 3), true),
            ],
            false,
            Preference::FastTrace,
        )],
        false,
    );
    let mut rays = Vec::new();
    for n in 0..520 {
        // A point of a line, or a quarter beside it; through the geometry
        // transform one time in two, the instance's one time in two.
        let a = *g.r.pick(&lines);
        let mut p = add(a, mul([1.0, 0.5, 0.25], g.r.dyadic(0, 16, 3)));
        if n % 2 == 1 {
            p[1] += 0.25;
        }
        if n / 2 % 2 == 1 {
            p = turned.apply(p);
        }
        let mut o = add(p, [g.r.dyadic(-8, 8, 2), g.r.dyadic(-8, 8, 2), -4.0]);
        if n / 4 % 2 == 1 {
            (p, o) = (tilted.apply(p), tilted.apply(o));
        }
        rays.push(Ray::new(o, sub(p, o), 0.0, 1000.0));
    }
    (scene, Vec::new(), rays)
}

/// Rays D3D12 forbids or leaves undefined, each one change away from a
/// legal ray, with what is wrong: for a screen in front of a device. No
/// class holds one and nothing here traces one: [`Ray::validate`] refuses
/// each (a test), and so must whatever hands rays to a device.
pub fn illegal_rays() -> Vec<(Ray, &'static str)> {
    let ok = Ray::new([1.0, 2.0, 3.0], [0.0, 0.5, 1.0], 0.0, 100.0);
    let with = |f: &dyn Fn(&mut Ray)| {
        let mut r = ok;
        f(&mut r);
        r
    };
    vec![
        (with(&|r| r.tmax = f32::NAN), "a NaN extent"),
        (with(&|r| r.tmin = f32::NAN), "a NaN start of the extent"),
        (with(&|r| r.origin[1] = f32::NAN), "a NaN origin"),
        (with(&|r| r.dir[2] = f32::NAN), "a NaN direction"),
        (with(&|r| r.dir = [0.0; 3]), "a direction of no length"),
        (with(&|r| r.dir = [-0.0; 3]), "a direction of no length"),
        (with(&|r| r.dir[0] = f32::INFINITY), "an infinite direction"),
        (
            with(&|r| r.origin[2] = f32::NEG_INFINITY),
            "an infinite origin",
        ),
        (with(&|r| r.tmin = -1.0), "a negative extent"),
        (
            with(&|r| (r.tmin, r.tmax) = (2.0, 1.0)),
            "an extent that ends before it starts",
        ),
        (
            with(&|r| r.tmin = f32::INFINITY),
            "an extent that starts at infinity",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Action;

    #[test]
    fn classes_have_names_that_read_back() {
        for class in Class::ALL {
            assert_eq!(Class::from_name(class.name()), Some(class));
            assert!(!class.name().contains(' ') && !class.describe().is_empty());
        }
        assert_eq!(Class::from_name("no-such-class"), None);
    }

    #[test]
    fn every_class_is_valid_and_the_same_for_the_same_seed() {
        for class in Class::ALL {
            let a = generate(class, DEFAULT_SEED);
            a.scene.validate().unwrap();
            for ray in &a.rays {
                ray.validate().unwrap();
            }
            // A random class has the rays the rule's bound is counted on,
            // and as many as the ten first classes always had; a directed
            // class has those its aim gives.
            let least = match class.kind() {
                ClassKind::Random => 1024,
                ClassKind::Directed => 500,
            };
            assert!(a.rays.len() >= least, "{}", class.name());
            assert_eq!(a.scenes().len(), a.frame_count());
            // Word for word, not just equal as floats.
            let b = generate(class, DEFAULT_SEED);
            assert_eq!(a, b);
            let words = |c: &Case| {
                c.rays
                    .iter()
                    .flat_map(|r| r.to_words())
                    .collect::<Vec<u32>>()
            };
            assert_eq!(words(&a), words(&b));
            let other = generate(class, DEFAULT_SEED + 1);
            assert_ne!(words(&a), words(&other), "{}", class.name());
        }
    }

    #[test]
    fn the_default_seed_gives_these_bytes_on_every_platform() {
        // Pinned on x86-64 Linux. A different value on another platform
        // means the generator is not deterministic there, and results from
        // it cannot be compared with results from here.
        let got: Vec<(&str, u64)> = Class::ALL
            .iter()
            .map(|c| (c.name(), generate(*c, DEFAULT_SEED).fingerprint()))
            .collect();
        let text: Vec<String> = got
            .iter()
            .map(|(name, f)| format!("(\"{name}\", {f:#018X}),"))
            .collect();
        assert_eq!(got, PINNED, "generated here:\n{}", text.join("\n"));
        // The first ten are the ten the crate had: adding classes moved
        // none of them.
        assert_eq!(Class::ALL[..10], Class::FIRST);
        // Another seed, another fingerprint; one changed bit of one ray, too.
        let mut case = generate(Class::StaticSoup, DEFAULT_SEED);
        assert_ne!(
            generate(Class::StaticSoup, DEFAULT_SEED + 1).fingerprint(),
            PINNED[0].1
        );
        case.rays[7].tmax = case.rays[7].tmax.next_up();
        assert_ne!(case.fingerprint(), PINNED[0].1);
    }

    const PINNED: [(&str, u64); 27] = [
        ("static-soup", 0xE5B061FD030C2314),
        ("dynamic-refit", 0x0D57D80B6B50C22B),
        ("dynamic-rebuild", 0xF8C3D46628C84862),
        ("instancing", 0x86D904D241E7D2AB),
        ("alpha-tested", 0x43E75F24A3205A48),
        ("overlapping", 0xAD86DE8CC1196863),
        ("transformed", 0x4D7E01E92B574DE3),
        ("hit-ordering", 0x02D9976BC7F93DD9),
        ("precision", 0x518E2FFE3E2A4D97),
        ("masks-and-flags", 0xFDE7BA5F3A0E034B),
        ("single", 0x3C3B9745FA2F418B),
        ("closed", 0x616EBAAC8F1D5E61),
        ("far", 0x9442AFE26EED39C3),
        ("seam", 0xB30CCD2BD82D003C),
        ("surface-rays", 0x85B3DAB343AAB451),
        ("scale", 0x3E1D76B2E2CD90CA),
        ("seam-edge", 0x22098AE9DB861CFF),
        ("edge", 0xA6EAE8C0FA1AC5F8),
        ("thin", 0xE1389433068063D2),
        ("large-thin", 0x22FC27BF1524C94A),
        ("cull-edge", 0x98E1E736EDB8B211),
        ("extent", 0x3EC180E71D8C0425),
        ("tie", 0xD9698B67D3DF14CE),
        ("far-graze", 0x5E47CD4A2745806D),
        ("flat", 0x10600EFF92DF1252),
        ("degenerate", 0x1B7363EE1FB512E8),
        ("closed-edge", 0x0E2EBB04EF874EB3),
    ];

    #[test]
    fn the_rule_s_classes_are_random_or_directed() {
        let random: Vec<&str> = Class::ALL
            .iter()
            .filter(|c| c.kind() == ClassKind::Random)
            .map(|c| c.name())
            .collect();
        assert_eq!(
            random,
            [
                "static-soup",
                "dynamic-refit",
                "dynamic-rebuild",
                "instancing",
                "alpha-tested",
                "transformed",
                "hit-ordering",
                "masks-and-flags",
                "single",
                "closed",
                "far",
                "seam",
                "surface-rays",
                "scale"
            ]
        );
        assert_eq!(Class::ALL.len() - random.len(), 13);
    }

    #[test]
    fn illegal_rays_are_refused_one_and_all() {
        let rays = illegal_rays();
        assert!(rays.len() >= 10);
        for (ray, what) in rays {
            assert!(ray.validate().is_err(), "{what}");
        }
    }

    #[test]
    fn spacings_are_steps_of_f32() {
        assert_eq!(stepped(8.0, 0), 8.0);
        assert_eq!(stepped(8.0, 1), 8.0f32.next_up());
        assert_eq!(stepped(8.0, -2), 8.0 - 2.0 * (8.0 - 8.0f32.next_down()));
        assert_eq!(stepped(-24.0, 8), -24.0 + 8.0 * (24.0f32.next_up() - 24.0));
        // Moving a ray by the same steps keeps it parallel, exactly.
        let (target, origin) = (stepped(8.0f32, 8), 8.0f32);
        assert_eq!(origin + (target - origin), target);
    }

    #[test]
    fn dynamic_classes_ask_for_refits_and_rebuilds() {
        let case = generate(Class::DynamicRefit, DEFAULT_SEED);
        let mut scene = case.scene.clone();
        for frame in &case.frames {
            let a = frame.apply(&mut scene).unwrap();
            assert_eq!(a.bottoms, [Action::Keep, Action::Refit, Action::Refit]);
            assert_eq!(a.top, Action::Refit);
        }
        let case = generate(Class::DynamicRebuild, DEFAULT_SEED);
        let mut scene = case.scene.clone();
        let mut seen = Vec::new();
        for frame in &case.frames {
            let a = frame.apply(&mut scene).unwrap();
            assert_eq!(a.top, Action::Rebuild);
            seen.push(a.bottoms);
        }
        assert_eq!(seen[0], [Action::Rebuild, Action::Keep]);
        assert_eq!(seen[2], [Action::Keep, Action::Rebuild]);
        assert_eq!(scene.instances.len(), 3);
    }

    #[test]
    fn meshes_are_closed_and_face_outward() {
        // Every edge of the cube is used once in each direction, by
        // position; and each triangle's normal points away from the centre.
        for mesh in [Mesh::cube(4, 2.0, [1.0, 2.0, 3.0]), {
            let mut m = Mesh::octahedron(1.5);
            m.vertices
                .iter_mut()
                .for_each(|v| *v = add(*v, [1.0, 2.0, 3.0]));
            m
        }] {
            let mut edges = std::collections::HashMap::new();
            for t in mesh.indices.chunks(3) {
                let p: Vec<[u32; 3]> = t
                    .iter()
                    .map(|&i| mesh.vertices[i as usize].map(f32::to_bits))
                    .collect();
                for k in 0..3 {
                    *edges.entry((p[k], p[(k + 1) % 3])).or_insert(0) += 1;
                }
                let v: Vec<P> = t.iter().map(|&i| mesh.vertices[i as usize]).collect();
                let (a, b) = (sub(v[1], v[0]), sub(v[2], v[0]));
                let n = [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ];
                let out = sub(v[0], [1.0, 2.0, 3.0]);
                assert!(n[0] * out[0] + n[1] * out[1] + n[2] * out[2] > 0.0);
            }
            for (&(a, b), &n) in &edges {
                assert_eq!((n, edges.get(&(b, a))), (1, Some(&1)));
            }
        }
    }
}
