//! Level 2 of the proposed rule on synthetic scenes: what shading consumes.
//! A fixed function of a hit record, the ray and the scene's vertex data
//! stands for the code after a traversal region, and what it produces is
//! compared: integers and booleans equal on unambiguous rays and those of
//! an admissible answer on the others, floats within the tolerance carried
//! through its arithmetic (docs/dxr-compatibility.md, "Level 2";
//! docs/dxr-validation-reference.md defines the function).
//!
//! What is consumed of a hit ([`consumed`]):
//!
//! * the **surface**: the instance's 24-bit id with the geometry index's
//!   low 8 bits above it. Two triangles belong to one surface, and to one
//!   material, when they agree in both;
//! * the **face**: the record's kind (front, back, or none for a
//!   procedural hit, which carries no face);
//! * the **hit point**, `origin + direction * t` in `f32`, each component a
//!   product and a sum;
//! * an **attribute**, interpolated by the weights between values the three
//!   vertices carry: `a0 + u (a1 - a0) + v (a2 - a0)` in `f32` in that
//!   order. A vertex's value is a function of its position's bits as the
//!   geometry gives them ([`vertex_attribute`]), a multiple of 1/256 in
//!   [0, 1): triangles that share a vertex's bits share its value, so the
//!   attribute is continuous across a shared edge and across a closed seam,
//!   and not across a seam whose sides hold other numbers.
//!
//! A miss consumes nothing (eight zero words); a refused record is not
//! consumed at all.

use crate::record::{Hit, Ray, hit_kind};
use crate::rule::{RayRule, SPACING};
use crate::scene::Scene;

/// The surface a hit belongs to.
pub fn surface(hit: &Hit) -> u32 {
    (hit.instance_id & 0x00FF_FFFF) | (hit.geometry_index & 0xFF) << 24
}

/// 32 bits to 32 well-mixed bits (the finaliser of MurmurHash3, which is
/// public domain): integer operations only.
pub fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ x >> 16
}

/// The attribute a vertex carries: a multiple of 1/256 in [0, 1) from the
/// bits of its position as the geometry holds it (before any transform). A
/// negative zero counts as zero.
pub fn vertex_attribute(v: [f32; 3]) -> f32 {
    let bits = v.map(|x| if x == 0.0 { 0 } else { x.to_bits() });
    let h = mix(bits[0] ^ mix(bits[1] ^ mix(bits[2])));
    (h >> 24) as f32 / 256.0
}

/// What the stand-in for shading takes from one record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Consumed {
    pub hit: bool,
    pub surface: u32,
    /// The record's kind: 1 front, 2 back, 3 none (a procedural hit).
    pub face: u32,
    pub point: [f32; 3],
    pub attribute: f32,
}

impl Consumed {
    pub const MISS: Consumed = Consumed {
        hit: false,
        surface: 0,
        face: 0,
        point: [0.0; 3],
        attribute: 0.0,
    };

    /// Eight words, as a program would store them: hit, surface, face, the
    /// point, the attribute, zero.
    pub fn to_words(&self) -> [u32; 8] {
        [
            u32::from(self.hit),
            self.surface,
            self.face,
            self.point[0].to_bits(),
            self.point[1].to_bits(),
            self.point[2].to_bits(),
            self.attribute.to_bits(),
            0,
        ]
    }

    pub fn from_words(w: &[u32; 8]) -> Consumed {
        Consumed {
            hit: w[0] != 0,
            surface: w[1],
            face: w[2],
            point: [w[3], w[4], w[5]].map(f32::from_bits),
            attribute: f32::from_bits(w[6]),
        }
    }
}

/// The attributes of the three vertices of the triangle `hit` names, or
/// `None` when the scene has no such triangle.
fn attributes(scene: &Scene, hit: &Hit) -> Option<[f32; 3]> {
    let inst = scene.instances.get(hit.instance_index as usize)?;
    let geo = scene.bottoms[inst.blas as usize]
        .geometries
        .get(hit.geometry_index as usize)?;
    if hit.primitive_index as usize >= geo.primitive_count() {
        return None;
    }
    Some(
        geo.triangle(hit.primitive_index as usize)
            .map(vertex_attribute),
    )
}

/// What shading consumes of `hit`, the record of `ray` in `scene`. `None`
/// for a refused record, a kind that is none, and a hit that names a
/// triangle the scene does not have.
pub fn consumed(scene: &Scene, ray: &Ray, hit: &Hit) -> Option<Consumed> {
    if hit.is_miss() {
        return Some(Consumed::MISS);
    }
    if !matches!(
        hit.kind,
        hit_kind::FRONT | hit_kind::BACK | hit_kind::PROCEDURAL
    ) {
        return None;
    }
    let a = attributes(scene, hit)?;
    Some(Consumed {
        hit: true,
        surface: surface(hit),
        face: hit.kind,
        point: std::array::from_fn(|k| ray.origin[k] + ray.dir[k] * hit.t),
        attribute: (a[0] + hit.u * (a[1] - a[0])) + hit.v * (a[2] - a[0]),
    })
}

/// How what one record gives shading stands against the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumedOutcome {
    /// The words the reference's own answer gives, bit for bit.
    Equal,
    /// The integers of an admissible answer, the floats within the carried
    /// tolerance.
    Within,
    Differ,
    /// A refused record: nothing was consumed.
    NotJudged,
}

/// Level 2 for one record: `got` against what the rule's admissible answers
/// for `ray` give through [`consumed`]. The tolerances are the rule's,
/// carried through the function: the hit point may lie anywhere on the ray
/// over the admissible interval of `t` (for an ordinary pair `t_tol` either
/// way, which is `x_tol / c` in space where the instance does not scale),
/// plus four spacings of the largest coordinate for the product and the
/// sum; the attribute within `b_tol` times its range over the triangle,
/// plus four spacings, and not compared where the weights are not (a
/// sliver, grazing or degenerate pair). The face is not compared where the
/// record or the pair has none.
pub fn judge_consumed(scene: &Scene, ray: &Ray, rule: &RayRule, got: &Hit) -> ConsumedOutcome {
    if got.is_refused() && got.refusal_reasons().is_some() {
        return ConsumedOutcome::NotJudged;
    }
    let Some(c) = consumed(scene, ray, got) else {
        return ConsumedOutcome::Differ;
    };
    if consumed(scene, ray, &rule.primary).is_some_and(|w| w.to_words() == c.to_words()) {
        return ConsumedOutcome::Equal;
    }
    if !c.hit {
        return if rule.miss_admissible && c == Consumed::MISS {
            ConsumedOutcome::Within
        } else {
            ConsumedOutcome::Differ
        };
    }
    let (o, d) = (ray.origin.map(f64::from), ray.dir.map(f64::from));
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let p = c.point.map(f64::from);
    for e in &rule.admissible {
        let Some(a) = attributes(scene, &e.hit) else {
            continue;
        };
        if surface(&e.hit) != c.surface {
            continue;
        }
        if e.facing_known && c.face != hit_kind::PROCEDURAL && c.face != e.hit.kind {
            continue;
        }
        // The point: on the ray, inside the admissible interval.
        let t = ((0..3).map(|k| (p[k] - o[k]) * d[k]).sum::<f64>() / dd).clamp(e.t_lo, e.t_hi);
        let off = (0..3)
            .map(|k| {
                let x = p[k] - (o[k] + d[k] * t);
                x * x
            })
            .sum::<f64>()
            .sqrt();
        let coords = (0..3).fold(0.0f64, |m, k| m.max(p[k].abs()).max(o[k].abs()));
        if off > 4.0 * SPACING * coords.max(f64::MIN_POSITIVE) {
            continue;
        }
        if e.b_tol.is_finite() {
            let a = a.map(f64::from);
            let want = e.w[0] * a[0] + e.w[1] * a[1] + e.w[2] * a[2];
            let range = a[0].max(a[1]).max(a[2]) - a[0].min(a[1]).min(a[2]);
            if (f64::from(c.attribute) - want).abs() > e.b_tol * range + 4.0 * SPACING {
                continue;
            }
        }
        return ConsumedOutcome::Within;
    }
    ConsumedOutcome::Differ
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::refusal;
    use crate::rule::{Constants, Rule};
    use crate::scene::{BottomLevel, BuildFlags, Geometry, Indices, Instance, Transform};

    fn scene() -> Scene {
        Scene {
            bottoms: vec![BottomLevel {
                geometries: vec![Geometry {
                    vertices: vec![
                        [0.0, 0.0, 2.0],
                        [0.0, 1.0, 2.0],
                        [1.0, 0.0, 2.0],
                        [1.0, 1.0, 2.0],
                    ],
                    indices: Indices::U16(vec![0, 1, 2, 2, 1, 3]),
                    transform: None,
                    opaque: true,
                }],
                flags: BuildFlags::default(),
            }],
            instances: vec![Instance {
                transform: Transform::IDENTITY,
                id: 0x12_3456,
                mask: 0xFF,
                flags: 0,
                blas: 0,
            }],
            flags: BuildFlags::default(),
        }
    }

    fn toward(x: f32, y: f32) -> Ray {
        Ray::new([x, y, -4.0], [0.0, 0.0, 2.0], 0.0, 100.0)
    }

    #[test]
    fn the_function_is_fixed() {
        // Pinned: a change here changes what level 2 and the pictures
        // compare, on every machine.
        assert_eq!(mix(1), 0x514E_28B7);
        assert_eq!(vertex_attribute([0.0, 1.0, 2.0]), 0.81640625);
        assert_eq!(vertex_attribute([-0.0, 1.0, 2.0]), 0.81640625);
        let s = scene();
        let ray = toward(0.25, 0.5);
        let hit = Rule::new(&s, Constants::PROPOSED).trace(&ray).primary;
        let c = consumed(&s, &ray, &hit).unwrap();
        assert_eq!(
            (c.hit, c.surface, c.face),
            (true, 0x12_3456, hit_kind::FRONT)
        );
        assert_eq!(c.point, [0.25, 0.5, 2.0]);
        let a = s.bottoms[0].geometries[0].triangle(0).map(vertex_attribute);
        assert_eq!(
            c.attribute,
            (a[0] + 0.5 * (a[1] - a[0])) + 0.25 * (a[2] - a[0])
        );
        assert_eq!(Consumed::from_words(&c.to_words()), c);
        assert_eq!(Consumed::MISS.to_words(), [0; 8]);
        assert_eq!(consumed(&s, &ray, &Hit::MISS), Some(Consumed::MISS));
        // What is not a record of this scene consumes nothing.
        assert_eq!(consumed(&s, &ray, &Hit::refused(refusal::RANGE)), None);
        let mut elsewhere = hit;
        elsewhere.primitive_index = 2;
        assert_eq!(consumed(&s, &ray, &elsewhere), None);
        // The two triangles share the diagonal's vertices: the attribute is
        // the same on it from either side.
        let on = toward(0.5, 0.5);
        let r = Rule::new(&s, Constants::PROPOSED).trace(&on);
        let both: Vec<f32> = r
            .admissible
            .iter()
            .map(|e| consumed(&s, &on, &e.hit).unwrap().attribute)
            .collect();
        assert_eq!((both.len(), both[0]), (2, both[1]));
    }

    #[test]
    fn integers_are_equal_and_floats_within_the_carried_tolerance() {
        let s = scene();
        let ray = toward(0.25, 0.5);
        let r = Rule::new(&s, Constants::PROPOSED).trace(&ray);
        let e = r.admissible[0];
        let judge = |got: &Hit| judge_consumed(&s, &ray, &r, got);
        assert_eq!(judge(&r.primary), ConsumedOutcome::Equal);
        let mut got = r.primary;
        got.t += (0.5 * e.t_tol) as f32;
        assert_eq!(judge(&got), ConsumedOutcome::Within);
        got.t = r.primary.t + (4.0 * e.t_tol) as f32;
        assert_eq!(judge(&got), ConsumedOutcome::Differ);
        // The attribute's range over this triangle is its vertices'.
        got = r.primary;
        got.u += (0.5 * e.b_tol) as f32;
        assert_eq!(judge(&got), ConsumedOutcome::Within);
        got.u = r.primary.u + 0.25;
        assert_eq!(judge(&got), ConsumedOutcome::Differ);
        // Another face, another surface, a miss.
        got = r.primary;
        got.kind = hit_kind::BACK;
        assert_eq!(judge(&got), ConsumedOutcome::Differ);
        got.kind = hit_kind::PROCEDURAL;
        assert_eq!(judge(&got), ConsumedOutcome::Within);
        got = r.primary;
        got.instance_id = 5;
        assert_eq!(judge(&got), ConsumedOutcome::Differ);
        assert_eq!(judge(&Hit::MISS), ConsumedOutcome::Differ);
        assert_eq!(judge(&Hit::refused(1)), ConsumedOutcome::NotJudged);
        // The other triangle of the same surface, where it is admissible:
        // on the shared diagonal.
        let on = toward(0.5, 0.5);
        let r = Rule::new(&s, Constants::PROPOSED).trace(&on);
        assert_eq!(
            judge_consumed(&s, &on, &r, &r.admissible[1].hit),
            ConsumedOutcome::Equal,
            "the same surface, point and attribute"
        );
        assert_eq!(
            judge_consumed(&s, &on, &r, &Hit::MISS),
            ConsumedOutcome::Differ
        );
        // Beside the square a miss is what is consumed.
        let beside = toward(1.5, 0.5);
        let r = Rule::new(&s, Constants::PROPOSED).trace(&beside);
        assert_eq!(
            judge_consumed(&s, &beside, &r, &Hit::MISS),
            ConsumedOutcome::Equal
        );
    }
}
