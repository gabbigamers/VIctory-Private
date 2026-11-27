//! Level 3 of the proposed rule on synthetic scenes: pictures. A scene is
//! drawn from hit records alone by a fixed function, one ray a pixel, and
//! two drawings are compared pixel by pixel (docs/dxr-compatibility.md,
//! "Level 3"; docs/dxr-validation-reference.md defines the function).
//!
//! * **The rays** ([`Camera::rays`]): from one eye, one through the centre
//!   of each pixel of a square image plane around the scene, `tmin` 0 and
//!   `tmax` 1, the direction scaled so that the plane is at `t = 1/2`.
//!   Sums, differences and products of `f32` only.
//! * **A pixel** ([`pixel`]): of what shading consumes
//!   ([`crate::shade::consumed`]) and the record's `t`, the surface's
//!   colour (three channels of 96 to 223 from its number), halved on a back
//!   face, times a ramp of the distance (`1 - t/2`, `t` held to 0..1),
//!   times `1/2 + attribute/2`; each channel rounded to 8 bits. A miss is
//!   the background, a refused record a colour no hit has.
//! * **Comparing** ([`compare`]): a pixel differs when a channel differs by
//!   more than 2 of 255. A pass: at most one pixel in 1,024 differs, each
//!   from a ray the reference flagged, and no group of touching differing
//!   pixels (8 neighbours) is larger than 2. Every differing pixel is
//!   listed with its ray and whether that ray was flagged.

use crate::generate::{Case, Class};
use crate::record::{Hit, Ray, hit_kind};
use crate::rule::Constants;
use crate::scene::Scene;
use crate::shade::{self, Consumed};

/// The side of the rule's pictures, in pixels.
pub const SIDE: usize = 1024;
/// A pixel no ray hit.
pub const BACKGROUND: [u8; 3] = [16, 16, 24];
/// A pixel whose record was refused or is not a record.
pub const NOT_DRAWN: [u8; 3] = [255, 0, 255];

/// Where a picture is taken from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: [f32; 3],
    /// The middle of the image plane, and its half-extents to the right
    /// and upward.
    pub center: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
}

impl Camera {
    /// A fixed oblique view of the cube of half-width `half` about
    /// `center`: the eye at `center + half * (1.5, -3, 2.25)`, the image
    /// plane through `center` and a little wider than the cube.
    pub fn around(center: [f32; 3], half: f32) -> Camera {
        let add = |v: [f32; 3], k: f32| -> [f32; 3] { std::array::from_fn(|a| v[a] * (k * half)) };
        let eye = add([1.5, -3.0, 2.25], 1.0);
        Camera {
            eye: std::array::from_fn(|a| center[a] + eye[a]),
            center,
            // Across the view direction (-1, 2, -1.5), and of one length.
            right: add([2.0, 1.0, 0.0], 0.8125),
            up: add([-1.5, 3.0, 5.0], 0.30078125),
        }
    }

    /// One ray a pixel, row by row from the top left, `side` a row.
    pub fn rays(&self, side: usize) -> Vec<Ray> {
        let n = side as f32;
        let mut rays = Vec::with_capacity(side * side);
        for j in 0..side {
            for i in 0..side {
                let x = ((2 * i + 1) as f32 - n) / n;
                let y = (n - (2 * j + 1) as f32) / n;
                let dir: [f32; 3] = std::array::from_fn(|a| {
                    let target = (self.center[a] + self.right[a] * x) + self.up[a] * y;
                    (target - self.eye[a]) * 2.0
                });
                rays.push(Ray::new(self.eye, dir, 0.0, 1.0));
            }
        }
        rays
    }
}

/// The cameras a class's scene is drawn from: one around everything it
/// holds, or, where that would leave the triangles smaller than a pixel
/// (the far and large-thin classes' clusters), one around each instance's
/// first geometries.
pub fn cameras(case: &Case) -> Vec<Camera> {
    let scene = &case.scene;
    let bounds = |geometries: &mut dyn Iterator<Item = [f32; 3]>| {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in geometries {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        let center: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
        let half = (0..3).fold(0.0f32, |m, k| m.max((hi[k] - lo[k]) * 0.5));
        Camera::around(center, half)
    };
    let placed = |i: usize, g: usize| {
        let inst = scene.instances[i];
        let geo = &scene.bottoms[inst.blas as usize].geometries[g];
        geo.vertices.iter().map(move |v| {
            inst.transform
                .apply(geo.transform.map_or(*v, |t| t.apply(*v)))
        })
    };
    let each = |i: usize| {
        (0..scene.bottoms[scene.instances[i].blas as usize]
            .geometries
            .len())
            .map(move |g| (i, g))
    };
    if matches!(case.class, Class::Far | Class::LargeThin) {
        (0..scene.instances.len())
            .flat_map(each)
            .map(|(i, g)| bounds(&mut placed(i, g)))
            .collect()
    } else {
        let mut all = (0..scene.instances.len())
            .flat_map(each)
            .flat_map(|(i, g)| placed(i, g));
        vec![bounds(&mut all)]
    }
}

/// The surface's colour: three channels of 96 to 223.
pub fn surface_colour(surface: u32) -> [f32; 3] {
    let h = shade::mix(surface);
    [h & 0x7F, h >> 8 & 0x7F, h >> 16 & 0x7F].map(|c| (96 + c) as f32)
}

/// The pixel of one record: of what shading consumed of it (`None` where
/// nothing was: a refused record) and of its distance `t`. Each channel is
/// `((colour * face) * ramp) * (1/2 + attribute/2)` in `f32`, plus a half,
/// rounded down: `face` 1/2 on a back face and 1 otherwise, `ramp` `1 -
/// t/2` with `t` held to 0..1. With `faces` off no back face is darkened:
/// the drawing of a path whose records carry no face, and of what it is
/// compared with.
pub fn pixel(consumed: Option<&Consumed>, t: f32, faces: bool) -> [u8; 3] {
    let Some(c) = consumed else {
        return NOT_DRAWN;
    };
    if !c.hit {
        return BACKGROUND;
    }
    let face = if faces && c.face == hit_kind::BACK {
        0.5
    } else {
        1.0
    };
    let ramp = 1.0 - 0.5 * t.clamp(0.0, 1.0);
    surface_colour(c.surface).map(|base| {
        let x = ((base * face) * ramp) * (0.5 + 0.5 * c.attribute);
        (x + 0.5).floor().clamp(0.0, 255.0) as u8
    })
}

/// A drawing: `side` by `side` pixels of three channels, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub side: usize,
    pub rgb: Vec<u8>,
}

/// Draws `hits`, the records of `rays` (a camera's, `side` a row), by the
/// fixed function [`pixel`].
pub fn draw(scene: &Scene, rays: &[Ray], hits: &[Hit], side: usize, faces: bool) -> Picture {
    assert!(rays.len() == side * side && hits.len() == rays.len());
    let mut rgb = Vec::with_capacity(3 * rays.len());
    for (ray, hit) in rays.iter().zip(hits) {
        let c = shade::consumed(scene, ray, hit);
        rgb.extend(pixel(c.as_ref(), hit.t, faces));
    }
    Picture { side, rgb }
}

/// One differing pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Differing {
    /// The pixel, which is also its ray's place in the camera's list.
    pub pixel: u32,
    /// The reference flagged that ray (ambiguous, by a kind or by a sliver
    /// or grazing pair).
    pub flagged: bool,
    /// The largest difference of a channel.
    pub by: u8,
}

/// Two drawings compared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureReport {
    pub pixels: u32,
    /// Pixels of the reference's drawing that show a surface (are not the
    /// background): never 0.
    pub shown: u32,
    /// Pixels that differ, and of those the ones whose ray the reference
    /// did not flag.
    pub differ: u32,
    pub unflagged: u32,
    /// Groups of touching differing pixels, and the size of the largest.
    pub groups: u32,
    pub group_max: u32,
    /// Every differing pixel, in order.
    pub differing: Vec<Differing>,
    pub pass: bool,
}

impl PictureReport {
    /// The share of differing pixels.
    pub fn share(&self) -> f64 {
        f64::from(self.differ) / f64::from(self.pixels.max(1))
    }
}

/// Compares `candidate` with `reference`, two drawings of the same rays;
/// `flagged[k]` says whether the reference flagged ray `k`
/// ([`crate::rule::RayRule::flagged`]).
///
/// An error, and no verdict, for drawings of different sizes, for a
/// drawing of no pixels, and for a reference drawing that shows no surface
/// (every pixel the background): a picture of nothing compares nothing,
/// and would pass whatever the path did elsewhere.
pub fn compare(
    reference: &Picture,
    candidate: &Picture,
    flagged: &[bool],
    c: &Constants,
) -> Result<PictureReport, String> {
    let side = reference.side;
    let pixels = side * side;
    if candidate.side != side
        || reference.rgb.len() != 3 * pixels
        || candidate.rgb.len() != 3 * pixels
        || flagged.len() != pixels
    {
        return Err("pictures of different sizes, or flags of another count".into());
    }
    if pixels == 0 {
        return Err("a picture of no pixels".into());
    }
    let shown = (0..pixels)
        .filter(|p| reference.rgb[3 * p..3 * p + 3] != BACKGROUND)
        .count() as u32;
    if shown == 0 {
        return Err("the reference's picture shows no surface: nothing to compare".into());
    }
    let mut differing = Vec::new();
    let mut mark = vec![false; pixels];
    for p in 0..pixels {
        let by = (0..3)
            .map(|k| reference.rgb[3 * p + k].abs_diff(candidate.rgb[3 * p + k]))
            .max()
            .unwrap_or(0);
        if u32::from(by) > c.pixel_step {
            mark[p] = true;
            differing.push(Differing {
                pixel: p as u32,
                flagged: flagged[p],
                by,
            });
        }
    }
    // Groups of touching pixels, eight neighbours.
    let (mut groups, mut group_max) = (0u32, 0u32);
    let mut seen = vec![false; pixels];
    let mut stack = Vec::new();
    for d in &differing {
        let start = d.pixel as usize;
        if seen[start] {
            continue;
        }
        groups += 1;
        let mut size = 0u32;
        seen[start] = true;
        stack.push(start);
        while let Some(p) = stack.pop() {
            size += 1;
            let (x, y) = ((p % side) as i64, (p / side) as i64);
            for (dx, dy) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= side as i64 || ny >= side as i64 {
                    continue;
                }
                let q = ny as usize * side + nx as usize;
                if mark[q] && !seen[q] {
                    seen[q] = true;
                    stack.push(q);
                }
            }
        }
        group_max = group_max.max(size);
    }
    let differ = differing.len() as u32;
    let unflagged = differing.iter().filter(|d| !d.flagged).count() as u32;
    Ok(PictureReport {
        pixels: pixels as u32,
        shown,
        differ,
        unflagged,
        groups,
        group_max,
        pass: u64::from(differ) * u64::from(c.pixel_one_in) <= pixels as u64
            && unflagged == 0
            && group_max <= c.pixel_group,
        differing,
    })
}

/// [`compare`] with the constants as given, and the names of the constants
/// whose halving or doubling changes the verdict (`sensitive` when there
/// are any). Only the pixel constants enter; which rays are flagged is
/// level 1's, and its sensitivity is [`crate::rule::assess`]'s.
pub fn assess(
    reference: &Picture,
    candidate: &Picture,
    flagged: &[bool],
    c: &Constants,
) -> Result<(PictureReport, Vec<&'static str>), String> {
    let base = compare(reference, candidate, flagged, c)?;
    let mut sensitive = Vec::new();
    for (name, v) in c.variants() {
        if (v.pixel_step, v.pixel_one_in, v.pixel_group)
            == (c.pixel_step, c.pixel_one_in, c.pixel_group)
        {
            continue;
        }
        if compare(reference, candidate, flagged, &v)?.pass != base.pass {
            sensitive.push(name);
        }
    }
    Ok((base, sensitive))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(side: usize, value: u8) -> Picture {
        Picture {
            side,
            rgb: vec![value; 3 * side * side],
        }
    }

    fn with(mut p: Picture, pixels: &[(usize, usize)], value: u8) -> Picture {
        for &(x, y) in pixels {
            let at = 3 * (y * p.side + x);
            p.rgb[at + 1] = value;
        }
        p
    }

    #[test]
    fn the_pixel_function_is_fixed() {
        assert_eq!(pixel(None, 0.0, true), NOT_DRAWN);
        assert_eq!(pixel(Some(&Consumed::MISS), 0.0, true), BACKGROUND);
        let c = Consumed {
            hit: true,
            surface: 7,
            face: hit_kind::FRONT,
            point: [0.0; 3],
            attribute: 0.5,
        };
        let base = surface_colour(7);
        assert!(base.iter().all(|x| (96.0..=223.0).contains(x)));
        let front = pixel(Some(&c), 0.0, true);
        assert_eq!(front, base.map(|x| (x * 0.75 + 0.5).floor() as u8));
        // The ramp: three quarters at t = 1/2, a half from t = 1 on.
        assert_eq!(
            pixel(Some(&c), 0.5, true),
            base.map(|x| (x * 0.75 * 0.75 + 0.5).floor() as u8)
        );
        assert_eq!(pixel(Some(&c), 7.0, true), pixel(Some(&c), 1.0, true));
        let back = Consumed {
            face: hit_kind::BACK,
            ..c
        };
        assert_eq!(
            pixel(Some(&back), 0.0, true),
            base.map(|x| (x * 0.375 + 0.5).floor() as u8)
        );
        // Without faces a back face and a hit with none draw as a front.
        assert_eq!(pixel(Some(&back), 0.0, false), front);
        let none = Consumed {
            face: hit_kind::PROCEDURAL,
            ..c
        };
        assert_eq!(pixel(Some(&none), 0.0, true), front);
        assert_ne!(surface_colour(8), base);
    }

    #[test]
    fn a_camera_s_rays_cover_its_plane_and_reach_it_at_a_half() {
        let cam = Camera::around([8.0, -4.0, 2.0], 4.0);
        let rays = cam.rays(4);
        assert_eq!(rays.len(), 16);
        for ray in &rays {
            ray.validate().unwrap();
            assert_eq!((ray.origin, ray.tmin, ray.tmax), (cam.eye, 0.0, 1.0));
        }
        // The first is the top left: centre - 3/4 right + 3/4 up, at t = 1/2.
        let at: [f32; 3] = std::array::from_fn(|a| rays[0].origin[a] + rays[0].dir[a] * 0.5);
        let want: [f32; 3] =
            std::array::from_fn(|a| cam.center[a] - 0.75 * cam.right[a] + 0.75 * cam.up[a]);
        assert_eq!(at, want);
        // The plane is across the view, and its two half-extents are of
        // one length to within a hundredth.
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let view: [f32; 3] = std::array::from_fn(|a| cam.center[a] - cam.eye[a]);
        assert_eq!((dot(view, cam.right), dot(view, cam.up)), (0.0, 0.0));
        assert_eq!(dot(cam.right, cam.up), 0.0);
        let ratio = dot(cam.right, cam.right) / dot(cam.up, cam.up);
        assert!((ratio - 1.0).abs() < 0.02, "{ratio}");
    }

    #[test]
    fn pictures_are_compared_by_step_share_and_group() {
        let c = Constants::PROPOSED;
        let a = flat(64, 100);
        let flags = vec![true; 64 * 64];
        // A step of 2 is no difference; 3 is.
        let r = compare(&a, &with(a.clone(), &[(5, 5)], 102), &flags, &c).unwrap();
        assert_eq!((r.differ, r.pass), (0, true));
        let r = compare(&a, &with(a.clone(), &[(5, 5)], 103), &flags, &c).unwrap();
        assert_eq!((r.differ, r.groups, r.group_max), (1, 1, 1));
        assert_eq!(
            r.differing,
            [Differing {
                pixel: 5 * 64 + 5,
                flagged: true,
                by: 3
            }]
        );
        // 4,096 pixels: four may differ, five may not.
        let four = [(1, 1), (10, 1), (20, 1), (30, 1)];
        let r = compare(&a, &with(a.clone(), &four, 0), &flags, &c).unwrap();
        assert_eq!((r.differ, r.pass, r.share()), (4, true, 4.0 / 4096.0));
        let mut five = four.to_vec();
        five.push((40, 1));
        assert!(
            !compare(&a, &with(a.clone(), &five, 0), &flags, &c)
                .unwrap()
                .pass
        );
        // Two touching pixels pass, diagonally too; three in a row are a
        // seam and fail.
        let r = compare(&a, &with(a.clone(), &[(7, 7), (8, 8)], 0), &flags, &c).unwrap();
        assert_eq!((r.groups, r.group_max, r.pass), (1, 2, true));
        let r = compare(
            &a,
            &with(a.clone(), &[(7, 7), (8, 8), (9, 8)], 0),
            &flags,
            &c,
        )
        .unwrap();
        assert_eq!((r.groups, r.group_max, r.pass), (1, 3, false));
        // A pixel that wraps to the next row does not touch.
        let r = compare(&a, &with(a.clone(), &[(63, 3), (0, 4)], 0), &flags, &c).unwrap();
        assert_eq!((r.groups, r.group_max), (2, 1));
        // A differing pixel of a ray the reference did not flag fails.
        let mut some = flags.clone();
        some[5 * 64 + 5] = false;
        let r = compare(&a, &with(a.clone(), &[(5, 5)], 0), &some, &c).unwrap();
        assert_eq!((r.differ, r.unflagged, r.pass), (1, 1, false));
        assert!(compare(&a, &flat(32, 100), &flags, &c).is_err());
        assert!(compare(&a, &a, &flags[1..], &c).is_err());
        // A picture of no pixels, and one that shows nothing, are refused:
        // neither is a pass.
        let none = Picture {
            side: 0,
            rgb: Vec::new(),
        };
        assert_eq!(
            compare(&none, &none, &[], &c).unwrap_err(),
            "a picture of no pixels"
        );
        let empty = Picture {
            side: 64,
            rgb: BACKGROUND.repeat(64 * 64),
        };
        assert!(
            compare(&empty, &a, &flags, &c)
                .unwrap_err()
                .contains("shows no surface")
        );
        assert!(assess(&empty, &empty, &flags, &c).is_err());
        // One pixel of a surface is a picture.
        let one = with(empty.clone(), &[(3, 3)], 200);
        let r = compare(&one, &one, &flags, &c).unwrap();
        assert_eq!((r.shown, r.pass), (1, true));
        assert_eq!(compare(&a, &a, &flags, &c).unwrap().shown, 4096);
    }

    #[test]
    fn a_picture_verdict_names_what_it_is_sensitive_to() {
        let c = Constants::PROPOSED;
        let a = flat(64, 100);
        let flags = vec![true; 64 * 64];
        // Nothing differs: nothing changes it.
        let (r, s) = assess(&a, &a, &flags, &c).unwrap();
        assert!(r.pass && s.is_empty());
        // Three apart pixels: a pass that halving the share turns.
        let b = with(a.clone(), &[(1, 1), (10, 1), (20, 1)], 0);
        let (r, s) = assess(&a, &b, &flags, &c).unwrap();
        assert!(r.pass);
        assert_eq!(s, ["pixel share halved"]);
        // A pair: halving the group turns it. A step of 3: doubling the
        // step hides it.
        let b = with(a.clone(), &[(7, 7), (8, 7)], 0);
        assert_eq!(
            assess(&a, &b, &flags, &c).unwrap().1,
            ["pixel group halved"]
        );
        let b = with(a.clone(), &[(7, 7), (8, 7), (9, 7)], 103);
        let (r, s) = assess(&a, &b, &flags, &c).unwrap();
        assert!(!r.pass);
        assert_eq!(s, ["pixel step doubled", "pixel group doubled"]);
    }
}
