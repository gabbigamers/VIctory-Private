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
}

impl Class {
    pub const ALL: [Class; 10] = [
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
            assert!(a.rays.len() >= 1000, "{}", class.name());
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
        let pinned: [(&str, u64); 10] = PINNED;
        assert_eq!(got, pinned);
        // Another seed, another fingerprint; one changed bit of one ray, too.
        let mut case = generate(Class::StaticSoup, DEFAULT_SEED);
        assert_ne!(
            generate(Class::StaticSoup, DEFAULT_SEED + 1).fingerprint(),
            PINNED[0].1
        );
        case.rays[7].tmax = case.rays[7].tmax.next_up();
        assert_ne!(case.fingerprint(), PINNED[0].1);
    }

    const PINNED: [(&str, u64); 10] = [
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
    ];

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
