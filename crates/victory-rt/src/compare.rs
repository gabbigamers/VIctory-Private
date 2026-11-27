//! The comparison and its policy, fixed before any hardware measurement
//! (docs/dxr-validation-reference.md states it in prose).
//!
//! A result is judged against the reference's answer for its ray
//! ([`crate::reference::Diag`]):
//!
//! * **Must match exactly**: hit or miss; instance index, instance id,
//!   geometry index and primitive index; front or back face; and on a miss
//!   every word of the record (all zero).
//! * **Within a tolerance**: `t`, `u` and `v`, against the exact values,
//!   with the tolerance the error model gives that ray and that triangle.
//!   Where that tolerance is too wide to judge by
//!   ([`crate::reference::Permitted::wide`]) the numbers are not compared
//!   at all: the result is right by its ids and face and counted as **ids
//!   only**, so that it does not read as agreement in `t`, `u` and `v`.
//! * **The band.** The tolerance bounds what `f32` arithmetic can do in
//!   the worst case, with a factor for formulations other than the modelled
//!   one; on instanced geometry it is far wider than the errors the
//!   software path shows. A result within tolerance but with an error past
//!   [`BAND`] of it is counted (**beyond the band**) and reported with the
//!   verdict. It is not a defect: the model permits it. It is a size of
//!   error the software path has not shown on the host, to be looked at.
//! * A **clear** ray has one right result. An **ambiguous** ray has a list
//!   of permitted results (and perhaps a permitted miss); a result must be
//!   one of them, under the same exact and tolerance rules. An **excluded**
//!   ray is counted and not judged.
//!
//! Anything else is a defect of the implementation that produced it, by
//! kind: a hit missed, a hit that is not there, the wrong triangle, the
//! wrong face, numbers out of tolerance, a malformed record.
//!
//! Two records are neither. A **refused** record (the kind's bit 31 with
//! the reasons below it, seven zero words: docs/dxr-shader-emission.md) is
//! a shader's statement that it could not decide the ray. It is its own
//! outcome, counted by reason, never a miss and never malformed; its ray
//! is traced another way and judged there. A **procedural** hit (kind 3)
//! names a triangle like any hit and is judged like one, but for the face,
//! which the record does not carry.
//!
//! This comparison judges by the reference's own error model. The proposed
//! rule for functionally equivalent rendering, with its measured tolerance,
//! its kinds of ambiguous ray and its levels 2 and 3, is [`crate::rule`];
//! docs/dxr-validation-reference.md says how the two stand to each other.
//!
//! Two lists are compared at once, a baseline (the software path) and a
//! candidate (the native path). Where both are right but name different
//! results, the ray is **divergent**: D3D12 permits either and the title
//! would see a difference. The verdict for a class:
//!
//! * [`Verdict::Fail`]: the candidate has a defect.
//! * [`Verdict::BaselineFail`]: the baseline has one; the comparison says
//!   nothing about the candidate until that is explained.
//! * [`Verdict::Useless`]: too few rays, too few clear ones, too many
//!   excluded ones or too many whose numbers are not compared to mean
//!   anything ([`MIN_RAYS`], [`MIN_CLEAR_PERCENT`],
//!   [`MAX_EXCLUDED_PERCENT`], [`MAX_WIDE_PERCENT`]).
//! * [`Verdict::Fallback`]: no defect, but divergent rays outside
//!   first-hit queries. A traversal region whose result the title reads
//!   beyond what both paths agree on keeps the software path.
//! * [`Verdict::Pass`].
//!
//! Divergence on a first-hit ray (any accepted hit ends the search) is
//! counted apart and does not change the verdict: such a query asks
//! whether anything is hit. A region that reads more from it than that
//! is a case for the software path, which the region's translator has to
//! decide, not this comparison.

use crate::record::{Hit, hit_kind, refusal};
use crate::reference::{Diag, Status, flag};

/// A class with fewer rays than this proves nothing.
pub const MIN_RAYS: usize = 200;
/// At least this share of a class's rays must be clear.
pub const MIN_CLEAR_PERCENT: usize = 20;
/// At most this share of a class's rays may be excluded.
pub const MAX_EXCLUDED_PERCENT: usize = 10;
/// At most this share of a class's rays may have a permitted result whose
/// numbers are not compared ([`flag::WIDE`]).
pub const MAX_WIDE_PERCENT: usize = 10;
/// The share of its tolerance past which a result's error is counted as
/// beyond the band. Measured, not derived: the software model's worst
/// error is under half of it at the documented seed and inside it at three
/// more (tests/agreement.rs asserts both), and the worst of a sweep of
/// 1000 seeds by hand is 0.096 of the tolerance
/// (docs/dxr-validation-reference.md). The rounding of an exact result to
/// `f32` is under a twelfth of any tolerance, so a record cannot be beyond
/// the band by that alone.
pub const BAND: f64 = 1.0 / 8.0;

/// How one result stands against the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The ray is not judged.
    Excluded,
    /// The one right result of a clear ray.
    Match,
    /// On an ambiguous ray, the result the reference's own rules pick.
    PermittedPrimary,
    /// On an ambiguous ray, another permitted result.
    PermittedOther,
    /// A miss where a hit is required.
    Missed,
    /// A hit where only a miss is right.
    Phantom,
    /// A hit on a triangle that is not a permitted result.
    WrongTriangle,
    /// The right triangle with the other face.
    WrongFace,
    /// The right triangle and face; `t`, `u` or `v` out of tolerance.
    Numeric,
    /// Not a hit record: an unknown kind, a miss or a refusal with other
    /// words set, a refusal without a known reason, a hit with a number
    /// that is not finite.
    Malformed,
    /// The shader refused the ray: not judged here, counted by reason.
    Refused,
}

impl Outcome {
    pub fn is_defect(self) -> bool {
        !matches!(
            self,
            Outcome::Excluded
                | Outcome::Match
                | Outcome::PermittedPrimary
                | Outcome::PermittedOther
                | Outcome::Refused
        )
    }
}

/// A judged result, with its errors where it named a permitted triangle
/// whose numbers are compared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Judged {
    pub outcome: Outcome,
    /// The result names a permitted triangle and face whose tolerance is
    /// too wide: right by its ids, its numbers not compared (the errors
    /// below are 0).
    pub ids_only: bool,
    /// `|t - exact|` as a share of the distance it is measured against
    /// ([`crate::reference::Permitted::t_scale`]), and as a share of the
    /// tolerance.
    pub t_error: f64,
    pub t_ratio: f64,
    /// The larger of the `u` and `v` errors, and that over the tolerance.
    pub uv_error: f64,
    pub uv_ratio: f64,
    /// The [`refusal`] bits of a refused record; 0 otherwise.
    pub reasons: u32,
    /// A right result of kind 3: its face was not compared.
    pub procedural: bool,
}

impl Judged {
    /// Right, with numbers that were compared, and an error past [`BAND`]
    /// of the tolerance.
    pub fn beyond_band(&self) -> bool {
        !self.outcome.is_defect() && !self.ids_only && (self.t_ratio > BAND || self.uv_ratio > BAND)
    }
}

/// One result against the reference's answer for its ray.
pub fn judge(diag: &Diag, got: &Hit) -> Judged {
    let plain = |outcome| Judged {
        outcome,
        ids_only: false,
        t_error: 0.0,
        t_ratio: 0.0,
        uv_error: 0.0,
        uv_ratio: 0.0,
        reasons: 0,
        procedural: false,
    };
    // A refusal says nothing of the ray, whatever the ray is.
    if got.is_refused() {
        return match got.refusal_reasons() {
            Some(reasons) => Judged {
                reasons,
                ..plain(Outcome::Refused)
            },
            None => plain(Outcome::Malformed),
        };
    }
    if diag.status == Status::Excluded {
        return plain(Outcome::Excluded);
    }
    let ambiguous = diag.status == Status::Ambiguous;
    if got.is_miss() {
        return plain(if *got != Hit::MISS {
            Outcome::Malformed
        } else if !diag.miss_permitted {
            Outcome::Missed
        } else if !ambiguous {
            Outcome::Match
        } else if diag.primary.is_miss() {
            Outcome::PermittedPrimary
        } else {
            Outcome::PermittedOther
        });
    }
    let numbers = [got.t, got.u, got.v];
    let procedural = got.kind == hit_kind::PROCEDURAL;
    if !matches!(
        got.kind,
        hit_kind::FRONT | hit_kind::BACK | hit_kind::PROCEDURAL
    ) || numbers.iter().any(|x| !x.is_finite())
    {
        return plain(Outcome::Malformed);
    }
    let Some(at) = diag.permitted.iter().position(|p| p.hit.ids() == got.ids()) else {
        return plain(if diag.permitted.is_empty() {
            Outcome::Phantom
        } else {
            Outcome::WrongTriangle
        });
    };
    let p = &diag.permitted[at];
    // A procedural hit carries no face: there is none to compare.
    if !procedural && p.hit.kind != got.kind {
        return plain(Outcome::WrongFace);
    }
    let right = if !ambiguous {
        Outcome::Match
    } else if at == 0 && !diag.primary.is_miss() {
        Outcome::PermittedPrimary
    } else {
        Outcome::PermittedOther
    };
    if p.wide {
        return Judged {
            ids_only: true,
            procedural,
            ..plain(right)
        };
    }
    let t_error = (f64::from(got.t) - p.t).abs();
    let uv_error = (f64::from(got.u) - p.u)
        .abs()
        .max((f64::from(got.v) - p.v).abs());
    let (t_ratio, uv_ratio) = (t_error / p.tol_t, uv_error / p.tol_uv);
    Judged {
        outcome: if t_error > p.tol_t || uv_error > p.tol_uv {
            Outcome::Numeric
        } else {
            right
        },
        ids_only: false,
        t_error: t_error / p.t_scale,
        t_ratio,
        uv_error,
        uv_ratio,
        reasons: 0,
        procedural,
    }
}

/// The outcomes of one list over a class.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tally {
    pub matched: u32,
    pub permitted_primary: u32,
    pub permitted_other: u32,
    pub missed: u32,
    pub phantom: u32,
    pub wrong_triangle: u32,
    pub wrong_face: u32,
    pub numeric: u32,
    pub malformed: u32,
    /// Refused records, and how many of them give each reason
    /// ([`refusal::NAMES`]: geometry, range; a record can give both).
    pub refused: u32,
    pub refused_by_reason: [u32; refusal::NAMES.len()],
    /// Of the right results (the first three counts), those of kind 3,
    /// whose face was not compared.
    pub procedural: u32,
    /// Of the right results (the first three counts), those whose numbers
    /// were not compared: the tolerance was too wide.
    pub ids_only: u32,
    /// Of the right results whose numbers were compared, those with an
    /// error past [`BAND`] of the tolerance.
    pub beyond_band: u32,
    /// The worst errors over the results whose numbers were compared, the
    /// numeric defects among them: `u` and `v` absolute, `t` as a share of
    /// its scale ([`Judged::t_error`]), and each as a share of the
    /// tolerance.
    pub worst_t: f64,
    pub worst_t_ratio: f64,
    pub worst_uv: f64,
    pub worst_uv_ratio: f64,
}

impl Tally {
    pub fn defects(&self) -> u32 {
        self.missed
            + self.phantom
            + self.wrong_triangle
            + self.wrong_face
            + self.numeric
            + self.malformed
    }

    fn add(&mut self, j: &Judged) {
        let count = match j.outcome {
            Outcome::Excluded => return,
            Outcome::Match => &mut self.matched,
            Outcome::PermittedPrimary => &mut self.permitted_primary,
            Outcome::PermittedOther => &mut self.permitted_other,
            Outcome::Missed => &mut self.missed,
            Outcome::Phantom => &mut self.phantom,
            Outcome::WrongTriangle => &mut self.wrong_triangle,
            Outcome::WrongFace => &mut self.wrong_face,
            Outcome::Numeric => &mut self.numeric,
            Outcome::Malformed => &mut self.malformed,
            Outcome::Refused => &mut self.refused,
        };
        *count += 1;
        for (k, (bit, _)) in refusal::NAMES.iter().enumerate() {
            self.refused_by_reason[k] += u32::from(j.reasons & bit != 0);
        }
        self.procedural += u32::from(j.procedural && !j.outcome.is_defect());
        self.ids_only += u32::from(j.ids_only);
        self.beyond_band += u32::from(j.beyond_band());
        self.worst_t = self.worst_t.max(j.t_error);
        self.worst_t_ratio = self.worst_t_ratio.max(j.t_ratio);
        self.worst_uv = self.worst_uv.max(j.uv_error);
        self.worst_uv_ratio = self.worst_uv_ratio.max(j.uv_ratio);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fallback,
    Fail,
    BaselineFail,
    Useless,
}

/// What size of error passes in a class: the tolerances of the reference's
/// own hits, over the rays that are compared and whose hit is not wide.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spread {
    /// The hits the figures are taken over.
    pub hits: u32,
    /// The barycentric tolerance: median and largest.
    pub uv_median: f64,
    pub uv_max: f64,
    /// The `t` tolerance as a share of its scale: median and largest.
    pub t_median: f64,
    pub t_max: f64,
}

impl Spread {
    fn of(diags: &[Diag]) -> Spread {
        let (mut uv, mut t) = (Vec::new(), Vec::new());
        for d in diags.iter().filter(|d| d.status != Status::Excluded) {
            // The primary is listed first when it is a hit.
            let Some(p) = d.permitted.first() else {
                continue;
            };
            if d.primary.is_miss() || p.hit != d.primary || p.wide {
                continue;
            }
            uv.push(p.tol_uv);
            t.push(p.tol_t / p.t_scale);
        }
        let stats = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            (
                v.get(v.len() / 2).copied().unwrap_or(0.0),
                v.last().copied().unwrap_or(0.0),
            )
        };
        let ((uv_median, uv_max), (t_median, t_max)) = (stats(&mut uv), stats(&mut t));
        Spread {
            hits: uv.len() as u32,
            uv_median,
            uv_max,
            t_median,
            t_max,
        }
    }
}

/// Two lists against the reference over one class.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub class: String,
    pub rays: u32,
    pub clear: u32,
    pub ambiguous: u32,
    pub excluded: u32,
    /// Compared rays with a permitted result whose numbers are not
    /// ([`flag::WIDE`]); they are among the clear and the ambiguous.
    pub wide: u32,
    pub tolerance: Spread,
    /// Rays that are not clear, by reason: one count per [`flag::NAMES`]
    /// entry (a ray can have several reasons).
    pub reasons: [u32; flag::NAMES.len()],
    pub baseline: Tally,
    pub candidate: Tally,
    /// Rays both lists answer rightly and identically, word for word.
    pub identical: u32,
    /// Rays both answer rightly with different triangles, faces or hit and
    /// miss; first-hit rays apart.
    pub divergent: u32,
    pub divergent_first_hit: u32,
    /// The divergent rays (first-hit ones included) by reason.
    pub divergent_reasons: [u32; flag::NAMES.len()],
    pub verdict: Verdict,
}

/// Whether a class with these counts can mean anything.
pub fn is_useless(rays: usize, clear: usize, excluded: usize, wide: usize) -> bool {
    rays < MIN_RAYS
        || clear * 100 < rays * MIN_CLEAR_PERCENT
        || excluded * 100 > rays * MAX_EXCLUDED_PERCENT
        || wide * 100 > rays * MAX_WIDE_PERCENT
}

/// Judges `baseline` and `candidate`, one hit per ray, against `diags`.
pub fn compare(
    class: &str,
    diags: &[Diag],
    baseline: &[Hit],
    candidate: &[Hit],
) -> Result<Report, String> {
    if baseline.len() != diags.len() || candidate.len() != diags.len() {
        return Err(format!(
            "{class}: {} rays, {} baseline hits, {} candidate hits",
            diags.len(),
            baseline.len(),
            candidate.len()
        ));
    }
    let mut r = Report {
        class: class.to_string(),
        rays: diags.len() as u32,
        clear: 0,
        ambiguous: 0,
        excluded: 0,
        wide: 0,
        tolerance: Spread::of(diags),
        reasons: [0; flag::NAMES.len()],
        baseline: Tally::default(),
        candidate: Tally::default(),
        identical: 0,
        divergent: 0,
        divergent_first_hit: 0,
        divergent_reasons: [0; flag::NAMES.len()],
        verdict: Verdict::Pass,
    };
    let by_reason = |counts: &mut [u32; flag::NAMES.len()], flags: u32| {
        for (k, (bit, _)) in flag::NAMES.iter().enumerate() {
            counts[k] += u32::from(flags & bit != 0);
        }
    };
    for (k, diag) in diags.iter().enumerate() {
        match diag.status {
            Status::Clear => r.clear += 1,
            Status::Ambiguous => r.ambiguous += 1,
            Status::Excluded => r.excluded += 1,
        }
        if diag.status != Status::Clear {
            by_reason(&mut r.reasons, diag.flags);
        }
        if diag.status != Status::Excluded && diag.flags & flag::WIDE != 0 {
            r.wide += 1;
        }
        let (a, b) = (judge(diag, &baseline[k]), judge(diag, &candidate[k]));
        r.baseline.add(&a);
        r.candidate.add(&b);
        if diag.status == Status::Excluded
            || a.outcome.is_defect()
            || b.outcome.is_defect()
            || a.outcome == Outcome::Refused
            || b.outcome == Outcome::Refused
        {
            continue;
        }
        let (x, y) = (&baseline[k], &candidate[k]);
        if x == y {
            r.identical += 1;
        }
        // A hit with no face differs from one with a face by its triangle
        // alone.
        let faces = x.kind != hit_kind::PROCEDURAL && y.kind != hit_kind::PROCEDURAL;
        if (faces && x.kind != y.kind) || x.is_miss() != y.is_miss() || x.ids() != y.ids() {
            if diag.flags & flag::FIRST_HIT != 0 {
                r.divergent_first_hit += 1;
            } else {
                r.divergent += 1;
            }
            by_reason(&mut r.divergent_reasons, diag.flags);
        }
    }
    r.verdict = if r.candidate.defects() > 0 {
        Verdict::Fail
    } else if r.baseline.defects() > 0 {
        Verdict::BaselineFail
    } else if is_useless(
        diags.len(),
        r.clear as usize,
        // A refused ray is not judged either.
        (r.excluded + r.candidate.refused.max(r.baseline.refused)) as usize,
        r.wide as usize,
    ) {
        Verdict::Useless
    } else if r.divergent > 0 {
        Verdict::Fallback
    } else {
        Verdict::Pass
    };
    Ok(r)
}

fn reasons_text(counts: &[u32; flag::NAMES.len()]) -> String {
    let parts: Vec<String> = flag::NAMES
        .iter()
        .zip(counts)
        .filter(|(_, n)| **n > 0)
        .map(|((_, name), n)| format!("{name} {n}"))
        .collect();
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(", ")
    }
}

impl Report {
    /// The report as a few lines of text.
    pub fn to_text(&self) -> String {
        let tally = |name: &str, t: &Tally| {
            format!(
                "  {name}: match {} permitted {}+{} (ids only {}, procedural {}) | missed {} phantom {} wrong-triangle {} wrong-face {} numeric {} malformed {} | refused {} (geometry {}, range {}) | beyond the band {} | worst t {:.3e} of its scale ({:.3} of tolerance) uv {:.3e} ({:.3})\n",
                t.matched,
                t.permitted_primary,
                t.permitted_other,
                t.ids_only,
                t.procedural,
                t.missed,
                t.phantom,
                t.wrong_triangle,
                t.wrong_face,
                t.numeric,
                t.malformed,
                t.refused,
                t.refused_by_reason[0],
                t.refused_by_reason[1],
                t.beyond_band,
                t.worst_t,
                t.worst_t_ratio,
                t.worst_uv,
                t.worst_uv_ratio,
            )
        };
        // A verdict without a defect is still read with what was not shown.
        let mut notes = String::new();
        if self.candidate.defects() == 0 && self.candidate.beyond_band > 0 {
            notes += &format!(
                ", {} candidate results beyond the band",
                self.candidate.beyond_band
            );
        }
        if self.candidate.refused > 0 {
            notes += &format!(", {} candidate rays refused", self.candidate.refused);
        }
        if self.candidate.defects() == 0 && self.candidate.ids_only > 0 {
            notes += &format!(
                ", {} candidate results compared by ids only",
                self.candidate.ids_only
            );
        }
        let s = &self.tolerance;
        format!(
            "{}: {:?}{notes}\n  rays {} clear {} ambiguous {} excluded {} wide {} (not clear by reason: {})\n  tolerance over {} hits: uv median {:.3e} largest {:.3e}; t, of its scale, median {:.3e} largest {:.3e}\n{}{}  identical {} divergent {} first-hit divergent {} (by reason: {})\n",
            self.class,
            self.verdict,
            self.rays,
            self.clear,
            self.ambiguous,
            self.excluded,
            self.wide,
            reasons_text(&self.reasons),
            s.hits,
            s.uv_median,
            s.uv_max,
            s.t_median,
            s.t_max,
            tally("baseline ", &self.baseline),
            tally("candidate", &self.candidate),
            self.identical,
            self.divergent,
            self.divergent_first_hit,
            reasons_text(&self.divergent_reasons),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::Permitted;

    fn hit(t: f32, primitive: u32) -> Hit {
        Hit {
            t,
            u: 0.25,
            v: 0.5,
            kind: hit_kind::FRONT,
            instance_index: 1,
            instance_id: 77,
            geometry_index: 0,
            primitive_index: primitive,
        }
    }

    fn permitted(h: Hit) -> Permitted {
        Permitted {
            hit: h,
            t: f64::from(h.t),
            u: f64::from(h.u),
            v: f64::from(h.v),
            tol_t: 1e-4,
            tol_uv: 1e-5,
            t_scale: f64::from(h.t),
            wide: false,
        }
    }

    fn diag(permitted_hits: &[Hit], miss_permitted: bool, flags: u32) -> Diag {
        let outcomes = permitted_hits.len() + usize::from(miss_permitted);
        Diag {
            primary: permitted_hits.first().copied().unwrap_or(Hit::MISS),
            status: if flags & flag::UNSTABLE != 0 {
                Status::Excluded
            } else if outcomes == 1 {
                Status::Clear
            } else {
                Status::Ambiguous
            },
            flags,
            edge_margin: 0.25,
            next_gap: f64::INFINITY,
            next_differs: 0,
            tmin_margin: 1.0,
            tmax_margin: 1.0,
            alpha_margin: f64::INFINITY,
            permitted: permitted_hits.iter().map(|h| permitted(*h)).collect(),
            miss_permitted,
        }
    }

    #[test]
    fn a_clear_hit_must_match_exactly_in_ids_and_within_tolerance_in_numbers() {
        let d = diag(&[hit(2.0, 5)], false, 0);
        assert_eq!(judge(&d, &hit(2.0, 5)).outcome, Outcome::Match);
        let near = judge(&d, &hit(2.00005, 5));
        assert_eq!(near.outcome, Outcome::Match);
        assert!(near.t_ratio > 0.4 && near.t_ratio < 0.6, "{}", near.t_ratio);
        assert_eq!(judge(&d, &hit(2.0002, 5)).outcome, Outcome::Numeric);
        let mut off = hit(2.0, 5);
        off.v = 0.50002;
        assert_eq!(judge(&d, &off).outcome, Outcome::Numeric);
        assert_eq!(judge(&d, &hit(2.0, 6)).outcome, Outcome::WrongTriangle);
        let mut other = hit(2.0, 5);
        other.instance_id = 78;
        assert_eq!(judge(&d, &other).outcome, Outcome::WrongTriangle);
        other = hit(2.0, 5);
        other.kind = hit_kind::BACK;
        assert_eq!(judge(&d, &other).outcome, Outcome::WrongFace);
        assert_eq!(judge(&d, &Hit::MISS).outcome, Outcome::Missed);
    }

    #[test]
    fn a_wide_tolerance_judges_ids_and_face_and_not_numbers() {
        let mut d = diag(&[hit(2.0, 5)], false, flag::WIDE);
        (d.permitted[0].tol_uv, d.permitted[0].wide) = (3.0, true);
        // Numbers the tolerance would pass, and numbers nothing would.
        for got in [hit(2.0, 5), hit(2.00005, 5), hit(900.0, 5)] {
            let j = judge(&d, &got);
            assert_eq!((j.outcome, j.ids_only), (Outcome::Match, true));
            assert_eq!((j.t_error, j.t_ratio, j.uv_ratio), (0.0, 0.0, 0.0));
            assert!(!j.beyond_band());
        }
        // The ids, the face and the record are judged as ever.
        assert_eq!(judge(&d, &hit(2.0, 6)).outcome, Outcome::WrongTriangle);
        let mut back = hit(2.0, 5);
        back.kind = hit_kind::BACK;
        assert_eq!(judge(&d, &back).outcome, Outcome::WrongFace);
        assert_eq!(judge(&d, &Hit::MISS).outcome, Outcome::Missed);
        assert_eq!(judge(&d, &hit(f32::NAN, 5)).outcome, Outcome::Malformed);
        // Without the mark the same tolerance would have passed anything
        // in u and v: that is what the mark is for.
        d.permitted[0].wide = false;
        let mut off = hit(2.0, 5);
        off.u = 0.9;
        assert_eq!(judge(&d, &off).outcome, Outcome::Match);
        assert!(!judge(&d, &off).ids_only);
    }

    #[test]
    fn an_error_past_the_band_is_counted_and_is_not_a_defect() {
        let d = diag(&[hit(2.0, 5)], false, 0);
        // The tolerance in t is 1e-4: a tenth of it, a half, and past it.
        let (within, beyond, out) = (hit(2.00001, 5), hit(2.00005, 5), hit(2.0002, 5));
        assert!(!judge(&d, &within).beyond_band());
        let j = judge(&d, &beyond);
        assert_eq!((j.outcome, j.beyond_band()), (Outcome::Match, true));
        // The error is reported against the scale of t, here t itself.
        assert!((j.t_error - 2.5e-5).abs() < 2e-6, "{}", j.t_error);
        let j = judge(&d, &out);
        assert_eq!((j.outcome, j.beyond_band()), (Outcome::Numeric, false));
        // In u and v alike.
        let mut uv = hit(2.0, 5);
        uv.v = 0.500002;
        assert_eq!(
            (judge(&d, &uv).outcome, judge(&d, &uv).beyond_band()),
            (Outcome::Match, true)
        );
        // Over a class: counted for the list that has it, the verdict kept
        // and the count on its line.
        let (mut diags, a, mut b) = class(300, &[]);
        b[3].t *= 1.0 + 2e-5;
        let r = compare("test", &diags, &a, &b).unwrap();
        assert_eq!(
            (r.verdict, r.baseline.beyond_band, r.candidate.beyond_band),
            (Verdict::Pass, 0, 1)
        );
        assert!(
            r.to_text()
                .starts_with("test: Pass, 1 candidate results beyond the band\n"),
            "{}",
            r.to_text()
        );
        // Ids-only results are counted the same way, and too many rays
        // with one make the class useless.
        for d in diags.iter_mut().take(31) {
            d.flags |= flag::WIDE;
            d.permitted[0].wide = true;
        }
        b[3] = a[3];
        let r = compare("test", &diags, &a, &b).unwrap();
        assert_eq!((r.verdict, r.wide, r.clear), (Verdict::Useless, 31, 300));
        assert_eq!((r.baseline.ids_only, r.candidate.ids_only), (31, 31));
        assert_eq!(r.tolerance.hits, 269);
        diags[30].flags = 0;
        diags[30].permitted[0].wide = false;
        let r = compare("test", &diags, &a, &b).unwrap();
        assert_eq!((r.verdict, r.wide), (Verdict::Pass, 30));
        assert!(
            r.to_text()
                .starts_with("test: Pass, 30 candidate results compared by ids only\n"),
            "{}",
            r.to_text()
        );
    }

    #[test]
    fn a_clear_miss_must_be_the_zero_record() {
        let d = diag(&[], true, 0);
        assert_eq!(judge(&d, &Hit::MISS).outcome, Outcome::Match);
        assert_eq!(judge(&d, &hit(2.0, 5)).outcome, Outcome::Phantom);
        let mut dirty = Hit::MISS;
        dirty.primitive_index = 3;
        assert_eq!(judge(&d, &dirty).outcome, Outcome::Malformed);
        let mut nan = hit(f32::NAN, 5);
        assert_eq!(judge(&d, &nan).outcome, Outcome::Malformed);
        nan = hit(2.0, 5);
        nan.kind = 4;
        assert_eq!(judge(&d, &nan).outcome, Outcome::Malformed);
    }

    #[test]
    fn a_refusal_is_counted_by_reason_and_is_neither_a_miss_nor_malformed() {
        let (hit_ray, miss_ray) = (diag(&[hit(2.0, 5)], false, 0), diag(&[], true, 0));
        for d in [
            &hit_ray,
            &miss_ray,
            &diag(&[hit(2.0, 5)], false, flag::UNSTABLE),
        ] {
            let j = judge(d, &Hit::refused(refusal::RANGE));
            assert_eq!((j.outcome, j.reasons), (Outcome::Refused, refusal::RANGE));
            assert!(!j.outcome.is_defect());
        }
        // A refusal that is not well formed is malformed, not a refusal.
        assert_eq!(
            judge(&hit_ray, &Hit::refused(0)).outcome,
            Outcome::Malformed
        );
        assert_eq!(
            judge(&hit_ray, &Hit::refused(8)).outcome,
            Outcome::Malformed
        );
        let mut dirty = Hit::refused(1);
        dirty.instance_id = 7;
        assert_eq!(judge(&miss_ray, &dirty).outcome, Outcome::Malformed);
        // Over a class: counted, by reason, on the verdict's line; not a
        // defect, not a divergence, not identical.
        let (diags, a, mut b) = class(300, &[]);
        b[3] = Hit::refused(refusal::GEOMETRY);
        b[4] = Hit::refused(refusal::RANGE);
        b[5] = Hit::refused(refusal::GEOMETRY | refusal::RANGE);
        let r = compare("test", &diags, &a, &b).unwrap();
        assert_eq!((r.verdict, r.candidate.defects()), (Verdict::Pass, 0));
        assert_eq!(
            (r.candidate.refused, r.candidate.refused_by_reason),
            (3, [2, 2])
        );
        assert_eq!(
            (r.identical, r.divergent, r.candidate.matched),
            (297, 0, 297)
        );
        assert!(
            r.to_text()
                .starts_with("test: Pass, 3 candidate rays refused\n"),
            "{}",
            r.to_text()
        );
        assert!(r.to_text().contains("refused 3 (geometry 2, range 2)"));
        // More refused rays than may go unjudged: the class says nothing.
        for h in b.iter_mut().take(31) {
            *h = Hit::refused(refusal::RANGE);
        }
        let r = compare("test", &diags, &a, &b).unwrap();
        assert_eq!((r.verdict, r.candidate.refused), (Verdict::Useless, 31));
    }

    #[test]
    fn a_procedural_hit_is_the_hit_of_its_triangle_without_a_face() {
        let d = diag(&[hit(2.0, 5)], false, 0);
        let mut got = hit(2.0, 5);
        got.kind = hit_kind::PROCEDURAL;
        let j = judge(&d, &got);
        assert_eq!((j.outcome, j.procedural), (Outcome::Match, true));
        // The triangle and the numbers are judged as ever.
        got.primitive_index = 6;
        assert_eq!(judge(&d, &got).outcome, Outcome::WrongTriangle);
        got = hit(2.0002, 5);
        got.kind = hit_kind::PROCEDURAL;
        assert_eq!(judge(&d, &got).outcome, Outcome::Numeric);
        // Against the same triangle with a face it is the same answer.
        let (diags, a, mut b) = class(300, &[]);
        for h in &mut b {
            h.kind = hit_kind::PROCEDURAL;
        }
        let r = compare("test", &diags, &a, &b).unwrap();
        assert_eq!((r.verdict, r.divergent, r.identical), (Verdict::Pass, 0, 0));
        assert_eq!((r.candidate.procedural, r.candidate.matched), (300, 300));
    }

    #[test]
    fn an_ambiguous_ray_takes_any_listed_result_and_nothing_else() {
        let d = diag(&[hit(2.0, 5), hit(2.0, 6)], false, flag::TIE_EXACT);
        assert_eq!(judge(&d, &hit(2.0, 5)).outcome, Outcome::PermittedPrimary);
        assert_eq!(judge(&d, &hit(2.0, 6)).outcome, Outcome::PermittedOther);
        assert_eq!(judge(&d, &hit(2.0, 7)).outcome, Outcome::WrongTriangle);
        assert_eq!(judge(&d, &Hit::MISS).outcome, Outcome::Missed);
        // A doubtful hit: the miss is permitted too.
        let d = diag(&[hit(2.0, 5)], true, flag::EDGE_NEAR);
        assert_eq!(judge(&d, &Hit::MISS).outcome, Outcome::PermittedOther);
        assert_eq!(judge(&d, &hit(2.0, 5)).outcome, Outcome::PermittedPrimary);
        // An excluded ray takes anything.
        let d = diag(&[hit(2.0, 5)], false, flag::UNSTABLE);
        assert_eq!(judge(&d, &hit(9.0, 1)).outcome, Outcome::Excluded);
    }

    fn class(n: usize, special: &[(Diag, Hit, Hit)]) -> (Vec<Diag>, Vec<Hit>, Vec<Hit>) {
        let mut out = (Vec::new(), Vec::new(), Vec::new());
        for k in 0..n {
            let h = hit(1.0 + k as f32, k as u32);
            out.0.push(diag(&[h], false, 0));
            out.1.push(h);
            out.2.push(h);
        }
        for (d, a, b) in special {
            out.0.push(d.clone());
            out.1.push(*a);
            out.2.push(*b);
        }
        out
    }

    #[test]
    fn the_verdict_follows_the_policy() {
        let verdict = |n: usize, special: &[(Diag, Hit, Hit)]| {
            let (d, a, b) = class(n, special);
            compare("test", &d, &a, &b).unwrap()
        };
        let r = verdict(300, &[]);
        assert_eq!((r.verdict, r.clear, r.identical), (Verdict::Pass, 300, 300));
        // Too few rays.
        assert_eq!(verdict(100, &[]).verdict, Verdict::Useless);
        // A tie both answer rightly and differently: fall back.
        let tie = diag(&[hit(2.0, 5), hit(2.0, 6)], false, flag::TIE_EXACT);
        let r = verdict(300, &[(tie.clone(), hit(2.0, 5), hit(2.0, 6))]);
        assert_eq!(
            (r.verdict, r.divergent, r.ambiguous),
            (Verdict::Fallback, 1, 1)
        );
        assert_eq!(r.divergent_reasons[1], 1);
        assert_eq!(
            (r.baseline.permitted_primary, r.candidate.permitted_other),
            (1, 1)
        );
        // The same answers on the tie: pass.
        assert_eq!(
            verdict(300, &[(tie.clone(), hit(2.0, 6), hit(2.0, 6))]).verdict,
            Verdict::Pass
        );
        // The same difference on a first-hit ray does not change the verdict.
        let any = diag(&[hit(2.0, 5), hit(3.0, 6)], false, flag::FIRST_HIT);
        let r = verdict(300, &[(any, hit(2.0, 5), hit(3.0, 6))]);
        assert_eq!(
            (r.verdict, r.divergent, r.divergent_first_hit),
            (Verdict::Pass, 0, 1)
        );
        // A defect of the candidate, of the baseline, of both.
        let clear = diag(&[hit(2.0, 5)], false, 0);
        let r = verdict(300, &[(clear.clone(), hit(2.0, 5), Hit::MISS)]);
        assert_eq!(
            (r.verdict, r.candidate.missed, r.candidate.defects()),
            (Verdict::Fail, 1, 1)
        );
        let r = verdict(300, &[(clear.clone(), hit(2.0, 9), hit(2.0, 5))]);
        assert_eq!(
            (r.verdict, r.baseline.wrong_triangle),
            (Verdict::BaselineFail, 1)
        );
        assert_eq!(
            verdict(300, &[(clear, hit(2.0, 9), Hit::MISS)]).verdict,
            Verdict::Fail
        );
        // Too many excluded rays, and too few clear ones.
        let out = diag(&[hit(2.0, 5)], false, flag::UNSTABLE);
        let many: Vec<_> = (0..40)
            .map(|_| (out.clone(), hit(2.0, 5), hit(2.0, 5)))
            .collect();
        let r = verdict(300, &many);
        assert_eq!(
            (r.verdict, r.excluded, r.reasons[10]),
            (Verdict::Useless, 40, 40)
        );
        let ties: Vec<_> = (0..1300)
            .map(|_| (tie.clone(), hit(2.0, 5), hit(2.0, 5)))
            .collect();
        assert_eq!(verdict(300, &ties).verdict, Verdict::Useless);
        assert!(!is_useless(200, 40, 20, 20) && is_useless(200, 39, 0, 0));
        assert!(is_useless(200, 200, 21, 0) && is_useless(200, 200, 0, 21));
    }

    #[test]
    fn worst_errors_and_lengths_are_reported() {
        let (d, a, mut b) = class(250, &[]);
        b[7].t += 0.00005;
        b[9].u += 0.000004;
        let r = compare("test", &d, &a, &b).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!((r.identical, r.divergent), (248, 0));
        // Ray 7's t is 8: the error as a share of that scale, and of the
        // tolerance of 1e-4.
        let t = &r.candidate;
        assert!(t.worst_t > 5e-6 && t.worst_t < 7e-6, "{}", t.worst_t);
        assert!(t.worst_t_ratio > 0.4 && t.worst_t_ratio < 0.6);
        assert!(t.worst_uv > 3e-6 && t.worst_uv < 5e-6, "{}", t.worst_uv);
        assert!(t.worst_uv_ratio > 0.3 && t.worst_uv_ratio < 0.5);
        // Both are past the band, and say so.
        assert_eq!((t.beyond_band, r.baseline.beyond_band), (2, 0));
        assert_eq!(r.baseline.worst_t, 0.0);
        // What passes: every hit has the helper's tolerances, 1e-5 in u and
        // v and 1e-4 in t against scales of 1 to 250.
        let s = r.tolerance;
        assert_eq!((s.hits, s.uv_median, s.uv_max), (250, 1e-5, 1e-5));
        assert!(s.t_max == 1e-4 && s.t_median > 7e-7 && s.t_median < 9e-7);
        let text = r.to_text();
        assert!(
            text.starts_with(
                "test: Pass, 2 candidate results beyond the band\n  rays 250 clear 250 ambiguous 0 excluded 0 wide 0 (not clear by reason: none)\n  tolerance over 250 hits: uv median 1.000e-5"
            ),
            "{text}"
        );
        assert!(compare("test", &d, &a[1..], &b).is_err());
    }
}
