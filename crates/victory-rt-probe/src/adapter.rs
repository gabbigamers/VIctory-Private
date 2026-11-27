//! The probe's only use of `victory-rt`: its generated cases, its CPU
//! reference, its CPU model of the software traversal, its comparison and
//! its result-file format, behind types of the probe's own. Nothing else
//! in this crate names that crate, so that a change to its comparison is
//! met here and nowhere else.
//!
//! **The verdicts are the crate's, not the rule's.** This module judges
//! by `victory-rt`'s older comparison (`victory_rt::compare`), with that
//! comparison's own tolerance and its own verdicts
//! (docs/dxr-validation-reference.md, "The error model and the older
//! comparison"). The rule for functionally equivalent rendering
//! (docs/dxr-compatibility.md) is a proposal; the crate has since gained
//! it as code beside the comparison (`victory_rt::rule`), and nothing
//! here calls that: moving the probe to the rule is a change of its own.
//! Every verdict this module returns is therefore "under the crate's
//! policy, not yet the rule", and the result file's header says so
//! ([`POLICY`]).

use victory_rt::compare::{self, Outcome, Verdict};
use victory_rt::console::{BuildOptions, SwScene};
use victory_rt::generate::{self, Class};
use victory_rt::record::{Hit, Ray, hit_kind};
use victory_rt::reference::{Diag, Reference, flag};
use victory_rt::results::{ResultFile, Section};
use victory_rt::scene as rt;
use victory_rt::traverse;

use crate::records::Answer;
use crate::scene::{
    Action, Actions, Bottom, Geometry, HitWords, Indices, Instance, Prefer, RayWords, Scene,
};

/// What every verdict in a result file is judged by; a header line.
pub const POLICY: &str = "victory-rt's comparison as the crate has it (docs/dxr-validation-reference.md): \
     under the crate's policy, not yet the rule of docs/dxr-compatibility.md";

/// The defects listed for one class, frame and path; the rest are counted.
pub const DEFECTS_LISTED: usize = 64;

/// The kind a procedural shader writes for a hit: it reports no face.
const PROCEDURAL: u32 = hit_kind::PROCEDURAL;

/// A kind that is none of the crate's. Two kinds the crate's comparison
/// did not know when the probe was written are handed to it as this one,
/// so that it says of them what it said then (no record: a defect, but on
/// a ray the reference excludes, where nothing is judged):
///
/// * kind 3 from a path that says it reports faces. The comparison now
///   takes kind 3 from anyone as a hit whose face is not compared;
/// * a kind with bit 31 that is no refusal (an unwritten record's poison
///   among them; refusals never reach the comparison). The comparison now
///   judges a kind with bit 31 before it looks at the ray, so that such a
///   record would be a defect on an excluded ray too.
const NO_KIND: u32 = 4;

/// The scene classes the probe runs: the ten the crate had when the probe
/// was written (`Class::FIRST`), in the generator's order. The seventeen
/// the crate added for the rule (`Class::ALL` has 27) are not the probe's:
/// they were made to be judged by the rule, which the probe does not yet
/// judge by, and nothing here has been held against them. A name of one
/// of them is no class of the probe's, as before.
const PROBED: [Class; 10] = Class::FIRST;

fn probed(class: &str) -> Option<Class> {
    PROBED.into_iter().find(|c| c.name() == class)
}

/// The names of the scene classes, in the generator's order.
pub fn classes() -> Vec<&'static str> {
    PROBED.iter().map(|c| c.name()).collect()
}

/// What a class holds, in a line.
pub fn describe(class: &str) -> Option<&'static str> {
    probed(class).map(Class::describe)
}

/// The seed the reference half's documented tables were taken at.
pub fn default_seed() -> u64 {
    generate::DEFAULT_SEED
}

/// One frame of a case: the scene, and what the frame asks of each
/// structure (everything built, for the first).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub scene: Scene,
    pub actions: Actions,
}

/// A class at a seed.
pub struct Case {
    pub class: String,
    pub seed: u64,
    /// What two machines must agree on to have traced the same rays
    /// against the same triangles (`victory_rt::generate::Case::fingerprint`).
    pub fingerprint: u64,
    /// The rays, traced against every frame.
    pub rays: Vec<RayWords>,
    pub frames: Vec<Frame>,
    inner: generate::Case,
}

fn prefer(preference: rt::Preference) -> Prefer {
    match preference {
        rt::Preference::None => Prefer::Neither,
        rt::Preference::FastTrace => Prefer::FastTrace,
        rt::Preference::FastBuild => Prefer::FastBuild,
    }
}

fn scene_of(scene: &rt::Scene) -> Scene {
    let geometry = |g: &rt::Geometry| Geometry {
        vertices: g.vertices.clone(),
        indices: match &g.indices {
            rt::Indices::None => Indices::None,
            rt::Indices::U16(i) => Indices::U16(i.clone()),
            rt::Indices::U32(i) => Indices::U32(i.clone()),
        },
        transform: g.transform.map(|t| t.0),
        opaque: g.opaque,
    };
    Scene {
        bottoms: scene
            .bottoms
            .iter()
            .map(|b| Bottom {
                geometries: b.geometries.iter().map(geometry).collect(),
                allow_update: b.flags.allow_update,
                prefer: prefer(b.flags.preference),
            })
            .collect(),
        instances: scene
            .instances
            .iter()
            .map(|i| Instance {
                transform: i.transform.0,
                id: i.id,
                mask: i.mask,
                flags: i.flags,
                bottom: i.blas,
            })
            .collect(),
        allow_update: scene.flags.allow_update,
        prefer: prefer(scene.flags.preference),
    }
}

fn action(action: rt::Action) -> Action {
    match action {
        rt::Action::Keep => Action::Keep,
        rt::Action::Refit => Action::Refit,
        rt::Action::Rebuild => Action::Rebuild,
    }
}

/// The scene of each frame of `case` with the actions that lead to it, in
/// `victory-rt`'s own types (`None` for the first).
fn frames_of(case: &generate::Case) -> Result<Vec<(rt::Scene, Option<rt::Actions>)>, String> {
    let mut scene = case.scene.clone();
    let mut out = vec![(scene.clone(), None)];
    for frame in &case.frames {
        let actions = frame.apply(&mut scene)?;
        out.push((scene.clone(), Some(actions)));
    }
    Ok(out)
}

/// The case of `class` at `seed`, generated here.
pub fn case(class: &str, seed: u64) -> Result<Case, String> {
    let known = probed(class)
        .ok_or_else(|| format!("no scene class {class:?}; there are: {}", classes().join(", ")))?;
    let inner = generate::generate(known, seed);
    let frames = frames_of(&inner)?
        .into_iter()
        .map(|(scene, actions)| Frame {
            actions: match actions {
                None => Actions::build_all(scene.bottoms.len()),
                Some(actions) => Actions {
                    bottoms: actions.bottoms.iter().map(|a| action(*a)).collect(),
                    top: action(actions.top),
                },
            },
            scene: scene_of(&scene),
        })
        .collect();
    Ok(Case {
        class: class.to_string(),
        seed,
        fingerprint: inner.fingerprint(),
        rays: inner.rays.iter().map(Ray::to_words).collect(),
        frames,
        inner,
    })
}

/// What the CPU says of one frame: the reference's answer for each ray,
/// and the hits of the CPU model of the software traversal, or why the
/// model refused the frame.
struct FrameTruth {
    diags: Vec<Diag>,
    model: Result<Vec<Hit>, String>,
}

/// The CPU's answers for every frame of a case.
pub struct Truth {
    frames: Vec<FrameTruth>,
}

/// Runs the CPU reference and the CPU model of the software traversal
/// over every frame of `case`. The model's image is brought forward as
/// each frame asks (refits where the structures allow, a build where they
/// do not); a frame it refuses has no model hits, and neither has any
/// frame after it.
pub fn truth(case: &Case) -> Result<Truth, String> {
    let rays = &case.inner.rays;
    let mut image: Option<SwScene> = None;
    // Why the image could not be brought to a frame: it then stands for
    // no later frame either.
    let mut lost: Option<String> = None;
    let mut frames = Vec::new();
    for (f, (scene, actions)) in frames_of(&case.inner)?.iter().enumerate() {
        if lost.is_none() {
            let step = match (image.as_mut(), actions) {
                (Some(image), Some(actions)) => image.update(scene, actions),
                _ => SwScene::build(scene, BuildOptions::for_rays(rays)).map(|built| {
                    image = Some(built);
                }),
            };
            lost = step.err().map(|e| format!("frame {f}: {e}"));
        }
        let model = match (&lost, &image) {
            (None, Some(image)) => traverse::trace_all(image, rays)
                .map_err(|refusal| format!("frame {f}: the traversal refused a ray: {refusal:?}")),
            (Some(why), _) => Err(why.clone()),
            (None, None) => Err(format!("frame {f}: no image")),
        };
        frames.push(FrameTruth { diags: Reference::new(scene).trace_all(rays), model });
    }
    Ok(Truth { frames })
}

/// The model's standing against the same reference, where it ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Model {
    /// Its own defects against the reference.
    pub defects: u32,
    /// Rays the path and the model answer rightly and word for word alike.
    pub identical: u32,
    /// Rays both answer rightly with different triangles, faces or hit
    /// and miss; first-hit rays apart.
    pub divergent: u32,
    pub divergent_first_hit: u32,
}

/// One defect: the ray, and what was wrong with its record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Defect {
    pub ray: u32,
    pub outcome: &'static str,
    pub record: HitWords,
}

/// A path's records for one frame against the reference and the model,
/// over the rays the path did not refuse. Plain numbers; the names are
/// `victory-rt`'s (`compare::Report`, `compare::Tally`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Judgement {
    /// The crate's verdict, in lower case: `pass`, `fallback`, `fail`,
    /// `baseline-fail` or `useless`. Under the crate's policy, not yet
    /// the rule.
    pub verdict: &'static str,
    /// The rays judged: those the path has a record for that is no refusal.
    pub judged: u32,
    pub clear: u32,
    pub ambiguous: u32,
    pub excluded: u32,
    pub wide: u32,
    /// The rays that are not clear, by the reference's reasons.
    pub not_clear: String,
    pub matched: u32,
    pub permitted_primary: u32,
    pub permitted_other: u32,
    pub ids_only: u32,
    pub beyond_band: u32,
    pub missed: u32,
    pub phantom: u32,
    pub wrong_triangle: u32,
    pub wrong_face: u32,
    pub numeric: u32,
    pub malformed: u32,
    pub worst_t: f64,
    pub worst_t_ratio: f64,
    pub worst_uv: f64,
    pub worst_uv_ratio: f64,
    /// `None` where the model refused the frame, with the reason beside.
    pub model: Option<Model>,
    pub model_refusal: String,
    /// Hits of a path that reports no face, judged with the face of the
    /// permitted result they name: the face was not compared.
    pub faces_not_reported: u32,
    /// The first [`DEFECTS_LISTED`] defects.
    pub defects: Vec<Defect>,
}

impl Judgement {
    pub fn defect_count(&self) -> u32 {
        self.missed
            + self.phantom
            + self.wrong_triangle
            + self.wrong_face
            + self.numeric
            + self.malformed
    }
}

fn verdict(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "pass",
        Verdict::Fallback => "fallback",
        Verdict::Fail => "fail",
        Verdict::BaselineFail => "baseline-fail",
        Verdict::Useless => "useless",
    }
}

fn outcome(outcome: Outcome) -> Option<&'static str> {
    Some(match outcome {
        Outcome::Missed => "missed",
        Outcome::Phantom => "phantom",
        Outcome::WrongTriangle => "wrong-triangle",
        Outcome::WrongFace => "wrong-face",
        Outcome::Numeric => "numeric",
        Outcome::Malformed => "malformed",
        // `Refused` does not come back from here: a refusal is taken out
        // before the comparison (`Answer::answered`).
        Outcome::Excluded
        | Outcome::Match
        | Outcome::PermittedPrimary
        | Outcome::PermittedOther
        | Outcome::Refused => return None,
    })
}

/// Judges one frame's answers of one path. `reports_face` is whether the
/// path's shader writes front or back: where it does not (the procedural
/// shader writes kind 3), a hit takes the face of the permitted result
/// with its ids, so that the comparison judges its triangle and numbers,
/// and the hits so treated are counted. Rays the path refused are not
/// judged here at all: they are the caller's to count
/// (`crate::records::refusals`). A record that is neither answer nor
/// refusal (an unwritten one, say) is judged, and is a defect.
pub fn judge(
    case: &Case,
    truth: &Truth,
    frame: usize,
    answers: &[Answer],
    reports_face: bool,
) -> Result<Judgement, String> {
    let truth = truth
        .frames
        .get(frame)
        .ok_or_else(|| format!("{}: no frame {frame}", case.class))?;
    if answers.len() != truth.diags.len() {
        return Err(format!(
            "{} frame {frame}: {} answers for {} rays",
            case.class,
            answers.len(),
            truth.diags.len()
        ));
    }
    let mut judged = Judgement::default();
    let (mut rays, mut diags, mut candidate, mut baseline) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (ray, answer) in answers.iter().enumerate() {
        let Some(record) = answer.answered() else { continue };
        let diag = &truth.diags[ray];
        let mut hit = Hit::from_words(record);
        if !reports_face && hit.kind == PROCEDURAL {
            judged.faces_not_reported += 1;
            let named = diag.permitted.iter().find(|p| p.hit.ids() == hit.ids());
            hit.kind = named.map_or(hit_kind::FRONT, |p| p.hit.kind);
        } else if hit.kind == PROCEDURAL || hit.is_refused() {
            // Bit 31 here is no refusal (those were taken out above): the
            // poison of an unwritten record, or words that are no record.
            hit.kind = NO_KIND;
        }
        rays.push(ray as u32);
        diags.push(diag.clone());
        candidate.push(hit);
        // Without the model the reference's own answers stand in, so that
        // the comparison runs; what it says of the baseline is then left
        // out below.
        baseline.push(match &truth.model {
            Ok(model) => model[ray],
            Err(_) => diag.primary,
        });
    }
    let report = compare::compare(&case.class, &diags, &baseline, &candidate)?;
    for ((diag, hit), ray) in diags.iter().zip(&candidate).zip(&rays) {
        if judged.defects.len() == DEFECTS_LISTED {
            break;
        }
        if let Some(outcome) = outcome(compare::judge(diag, hit).outcome) {
            let record = *answers[*ray as usize].answered().expect("an answered ray");
            judged.defects.push(Defect { ray: *ray, outcome, record });
        }
    }
    let not_clear: Vec<String> = flag::NAMES
        .iter()
        .zip(&report.reasons)
        .filter(|(_, n)| **n > 0)
        .map(|((_, name), n)| format!("{name} {n}"))
        .collect();
    let tally = &report.candidate;
    Ok(Judgement {
        verdict: verdict(report.verdict),
        judged: report.rays,
        clear: report.clear,
        ambiguous: report.ambiguous,
        excluded: report.excluded,
        wide: report.wide,
        not_clear: not_clear.join("; "),
        matched: tally.matched,
        permitted_primary: tally.permitted_primary,
        permitted_other: tally.permitted_other,
        ids_only: tally.ids_only,
        beyond_band: tally.beyond_band,
        missed: tally.missed,
        phantom: tally.phantom,
        wrong_triangle: tally.wrong_triangle,
        wrong_face: tally.wrong_face,
        numeric: tally.numeric,
        malformed: tally.malformed,
        worst_t: tally.worst_t,
        worst_t_ratio: tally.worst_t_ratio,
        worst_uv: tally.worst_uv,
        worst_uv_ratio: tally.worst_uv_ratio,
        model: truth.model.as_ref().ok().map(|_| Model {
            defects: report.baseline.defects(),
            identical: report.identical,
            divergent: report.divergent,
            divergent_first_hit: report.divergent_first_hit,
        }),
        model_refusal: truth.model.as_ref().err().cloned().unwrap_or_default(),
        ..judged
    })
}

/// Hit records as a `victory-rt` result file, written by that crate's own
/// writer: each of `sections` is a case, a frame of it, and one record per
/// ray. A section that is not of its case as generated here (a frame it
/// does not have, a record short) is refused, as is a class and frame
/// given twice.
pub fn hits_text(producer: &str, sections: &[(&Case, u32, &[HitWords])]) -> Result<String, String> {
    let sections = sections
        .iter()
        .map(|(case, frame, records)| {
            let section =
                Section::of(&case.inner, *frame, records.iter().map(Hit::from_words).collect());
            section.check(&case.inner).map(|()| section)
        })
        .collect::<Result<Vec<_>, _>>()?;
    ResultFile { producer: producer.to_string(), sections }.to_text()
}

/// One section of a `victory-rt` result file, read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitsSection {
    pub class: String,
    pub seed: u64,
    pub frame: u32,
    pub records: Vec<HitWords>,
}

/// Reads a `victory-rt` result file with that crate's own reader, and
/// checks each section against its case as generated here: the class, the
/// seed, a frame the case has, one record per ray, and the fingerprint, so
/// that a generator that differs between the writing and the reading
/// machine shows as that.
pub fn read_hits(text: &str) -> Result<Vec<HitsSection>, String> {
    let file = ResultFile::parse(text)?;
    let mut cases: Vec<Case> = Vec::new();
    let mut out = Vec::new();
    for section in file.sections {
        if !cases.iter().any(|c| c.class == section.class && c.seed == section.seed) {
            cases.push(case(&section.class, section.seed)?);
        }
        let of = cases
            .iter()
            .find(|c| c.class == section.class && c.seed == section.seed)
            .expect("generated above");
        section.check(&of.inner)?;
        out.push(HitsSection {
            class: section.class,
            seed: section.seed,
            frame: section.frame,
            records: section.hits.iter().map(Hit::to_words).collect(),
        });
    }
    Ok(out)
}

/// The reference's own answer for each ray of a frame, and the model's
/// hits where it ran: for tests that need records a path could have
/// written.
#[cfg(test)]
pub(crate) fn cpu_records(truth: &Truth, frame: usize) -> (Vec<HitWords>, Option<Vec<HitWords>>) {
    let truth = &truth.frames[frame];
    (
        truth.diags.iter().map(|d| d.primary.to_words()).collect(),
        truth.model.as_ref().ok().map(|hits| hits.iter().map(Hit::to_words).collect()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_shader::emit::rayquery::{HIT_REFUSED, REFUSED_GEOMETRY};

    fn answers(records: &[HitWords]) -> Vec<Answer> {
        records.iter().map(|r| Answer::of(*r)).collect()
    }

    #[test]
    fn a_case_is_the_generators_with_its_frames_and_what_each_asks() {
        assert_eq!(classes().len(), 10);
        assert!(describe("static-soup").is_some() && describe("no-such").is_none());
        assert!(case("no-such", 1).err().is_some_and(|e| e.contains("static-soup")));
        // The probe's classes are the crate's first ten, in its order; the
        // seventeen it added for the rule are no class of the probe's.
        assert_eq!(classes(), Class::ALL[..10].iter().map(|c| c.name()).collect::<Vec<_>>());
        assert_eq!(Class::ALL.len(), 27);
        for added in &Class::ALL[10..] {
            assert!(describe(added.name()).is_none(), "{}", added.name());
            assert!(case(added.name(), 1).err().is_some_and(|e| e.contains("no scene class")), "{}", added.name());
        }

        let soup = case("static-soup", default_seed()).unwrap();
        assert_eq!((soup.frames.len(), soup.rays.len()), (1, 2000));
        assert_eq!(soup.frames[0].actions, Actions::build_all(1));
        assert_eq!(soup.frames[0].scene.instances[0].id, 7);
        assert_eq!(soup.fingerprint, soup.inner.fingerprint());
        assert_eq!(soup.frames[0].scene.primitives(), soup.inner.scene.bottoms[0].primitive_count());

        // The refit class: its first bottom level is never touched, the
        // others are refitted, and so is the top level.
        let refit = case("dynamic-refit", default_seed()).unwrap();
        assert_eq!(refit.frames.len(), 4);
        for frame in &refit.frames[1..] {
            assert_eq!(frame.actions.bottoms, [Action::Keep, Action::Refit, Action::Refit]);
            assert_eq!(frame.actions.top, Action::Refit);
        }
        assert_ne!(refit.frames[0].scene, refit.frames[1].scene);
        // The rebuild class asks for builds.
        let rebuild = case("dynamic-rebuild", default_seed()).unwrap();
        assert!(rebuild.frames[1..].iter().all(|f| f.actions.asked().1 > 0));
    }

    #[test]
    fn the_cpus_own_answers_pass_under_the_crates_policy() {
        let case = case("alpha-tested", default_seed()).unwrap();
        let truth = truth(&case).unwrap();
        let (reference, model) = cpu_records(&truth, 0);
        let model = model.expect("the model runs every generated class");
        // The model as the path: no defect, and identical to itself.
        let judged = judge(&case, &truth, 0, &answers(&model), true).unwrap();
        assert_eq!(judged.defect_count(), 0, "{judged:?}");
        assert!(matches!(judged.verdict, "pass" | "fallback"), "{}", judged.verdict);
        assert_eq!(judged.judged, 2000);
        assert_eq!(judged.clear + judged.ambiguous + judged.excluded, 2000);
        let standing = judged.model.unwrap();
        assert_eq!((standing.defects, standing.divergent), (0, 0));
        assert!(standing.identical > 1900);
        assert!(judged.not_clear.contains("alpha-near"), "{}", judged.not_clear);
        assert_eq!(judged.faces_not_reported, 0);
        // The reference's own answers as the path.
        let judged = judge(&case, &truth, 0, &answers(&reference), true).unwrap();
        assert_eq!((judged.defect_count(), judged.permitted_other), (0, 0));
        assert!(judge(&case, &truth, 1, &answers(&reference), true).is_err());
        assert!(judge(&case, &truth, 0, &answers(&reference[1..]), true).is_err());
    }

    #[test]
    fn planted_defects_are_counted_and_listed_with_their_rays() {
        let case = case("static-soup", default_seed()).unwrap();
        let truth = truth(&case).unwrap();
        let (mut records, _) = cpu_records(&truth, 0);
        let hits: Vec<usize> = (0..records.len()).filter(|&k| records[k][3] != 0).collect();
        let misses: Vec<usize> = (0..records.len()).filter(|&k| records[k][3] == 0).collect();
        // A hit dropped, a hit where there is none, a face turned, a
        // record that is no record.
        records[hits[0]] = [0; 8];
        records[misses[0]] = records[hits[1]];
        records[hits[2]][3] ^= 3;
        records[hits[3]][3] = 9;
        let judged = judge(&case, &truth, 0, &answers(&records), true).unwrap();
        assert_eq!(judged.verdict, "fail");
        assert_eq!((judged.missed, judged.wrong_face, judged.malformed), (1, 1, 1));
        assert_eq!(judged.phantom + judged.wrong_triangle, 1);
        assert_eq!(judged.defect_count(), 4);
        let mut listed: Vec<u32> = judged.defects.iter().map(|d| d.ray).collect();
        let mut planted = vec![hits[0] as u32, misses[0] as u32, hits[2] as u32, hits[3] as u32];
        listed.sort_unstable();
        planted.sort_unstable();
        assert_eq!(listed, planted);
        let missed = judged.defects.iter().find(|d| d.ray == hits[0] as u32).unwrap();
        assert_eq!((missed.outcome, missed.record), ("missed", [0; 8]));

        // More defects than are listed: all counted, the first 64 listed.
        let (mut records, _) = cpu_records(&truth, 0);
        hits.iter().take(100).for_each(|&k| records[k] = [0; 8]);
        let judged = judge(&case, &truth, 0, &answers(&records), true).unwrap();
        assert!(judged.missed >= 90, "{}", judged.missed);
        assert_eq!(judged.defects.len(), DEFECTS_LISTED);
    }

    #[test]
    fn a_refused_ray_is_not_judged_and_never_read_as_a_miss() {
        let case = case("static-soup", default_seed()).unwrap();
        let truth = truth(&case).unwrap();
        let (records, _) = cpu_records(&truth, 0);
        let mut given = answers(&records);
        // Refusals on rays that hit, and one ray never dispatched.
        let hits: Vec<usize> = (0..records.len()).filter(|&k| records[k][3] != 0).collect();
        for &ray in &hits[..10] {
            given[ray] = Answer::of([0, 0, 0, HIT_REFUSED | REFUSED_GEOMETRY, 0, 0, 0, 0]);
        }
        given[hits[10]] = Answer::NotDispatched;
        let judged = judge(&case, &truth, 0, &given, true).unwrap();
        assert_eq!(judged.judged, 2000 - 11);
        assert_eq!(judged.defect_count(), 0, "a refusal is no miss: {judged:?}");
        // Read as misses they would be eleven lost hits.
        let mut as_misses = answers(&records);
        hits[..11].iter().for_each(|&ray| as_misses[ray] = Answer::of([0; 8]));
        assert_eq!(judge(&case, &truth, 0, &as_misses, true).unwrap().missed, 11);
        // Everything refused: nothing judged, and no pass.
        let none = vec![Answer::NotDispatched; records.len()];
        let judged = judge(&case, &truth, 0, &none, true).unwrap();
        assert_eq!((judged.judged, judged.verdict), (0, "useless"));
    }

    #[test]
    fn a_path_that_reports_no_face_is_judged_by_triangle_and_numbers() {
        let case = case("transformed", default_seed()).unwrap();
        let truth = truth(&case).unwrap();
        let (records, _) = cpu_records(&truth, 0);
        let hits = records.iter().filter(|r| r[3] != 0).count() as u32;
        let faceless: Vec<HitWords> = records
            .iter()
            .map(|r| if r[3] == 0 { *r } else { [r[0], r[1], r[2], 3, r[4], r[5], r[6], r[7]] })
            .collect();
        let judged = judge(&case, &truth, 0, &answers(&faceless), false).unwrap();
        assert_eq!(judged.defect_count(), 0, "{judged:?}");
        assert_eq!(judged.faces_not_reported, hits);
        // The same records from a path that says it reports faces are no
        // records at all.
        let judged = judge(&case, &truth, 0, &answers(&faceless), true).unwrap();
        assert_eq!((judged.malformed, judged.faces_not_reported), (hits, 0));
    }

    /// The crate's comparison came to know two kinds after the probe was
    /// written: kind 3 from anyone as a hit with no face to compare, and
    /// any kind with bit 31 as a refusal, judged before the ray is looked
    /// at. The probe's counts are as they were: a record of either kind
    /// that reaches the comparison is no record, and nothing is judged on
    /// a ray the reference excludes.
    #[test]
    fn a_record_of_a_kind_that_is_none_is_judged_as_before_the_crate_knew_more_kinds() {
        // `precision` has rays the reference excludes; `static-soup` none.
        for (class, rays, excluded) in [("precision", 1735, 106), ("static-soup", 2000, 0)] {
            let case = case(class, default_seed()).unwrap();
            let truth = truth(&case).unwrap();
            let (records, _) = cpu_records(&truth, 0);
            // Unwritten records (the poison has bit 31 and is no refusal),
            // a refusal with a reason no shader gives, and kind 3 on every
            // ray from a path that reports faces.
            let poison = vec![Answer::of([0xa5c3_f0f0; 8]); records.len()];
            let unknown_reason = vec![Answer::of([0, 0, 0, HIT_REFUSED | 4, 0, 0, 0, 0]); records.len()];
            let faceless: Vec<Answer> =
                records.iter().map(|r| Answer::of([r[0], r[1], r[2], 3, r[4], r[5], r[6], r[7]])).collect();
            for given in [&poison, &unknown_reason, &faceless] {
                let judged = judge(&case, &truth, 0, given, true).unwrap();
                assert_eq!((judged.judged, judged.excluded), (rays, excluded), "{class}");
                assert_eq!(judged.malformed, rays - excluded, "{class}: {judged:?}");
                assert_eq!((judged.defect_count(), judged.verdict), (rays - excluded, "fail"), "{class}");
                assert_eq!(judged.defects.len(), DEFECTS_LISTED);
            }
        }
    }

    #[test]
    fn hits_are_written_and_read_by_the_reference_crates_own_format() {
        let case = case("dynamic-refit", default_seed()).unwrap();
        let truth = truth(&case).unwrap();
        let frames: Vec<(u32, Vec<HitWords>)> =
            (0..4).map(|f| (f, cpu_records(&truth, f as usize).0)).collect();
        let other = super::case("static-soup", 3).unwrap();
        let soup = cpu_records(&super::truth(&other).unwrap(), 0).0;
        let mut sections: Vec<(&Case, u32, &[HitWords])> =
            frames.iter().map(|(f, records)| (&case, *f, records.as_slice())).collect();
        sections.push((&other, 0, &soup));
        let text = hits_text("a test", &sections).unwrap();
        // Version 2 of the format: the third line is the constant of the
        // rule's tolerance as the writing build of `victory-rt` measures
        // it. It is that crate's line; no verdict of the probe uses it.
        assert!(text.starts_with("victory-rt results 2\nproducer a test\nrule k 8\nsection dynamic-refit "), "{text}");
        let back = read_hits(&text).unwrap();
        assert_eq!(back.len(), 5);
        // A file as the probe wrote it before that line existed (version
        // 1) reads as the same sections; one whose constant is another
        // build's is refused, with the reason.
        let old = text.replacen("victory-rt results 2\nproducer a test\nrule k 8\n", "victory-rt results 1\nproducer a test\n", 1);
        assert_ne!(old, text);
        assert_eq!(read_hits(&old).unwrap(), back);
        let other_k = text.replacen("\nrule k 8\n", "\nrule k 4\n", 1);
        let refused = read_hits(&other_k).unwrap_err();
        assert!(refused.contains("k = 4") && refused.contains("differs"), "{refused}");
        assert!(read_hits(&text.replacen("\nrule k 8\n", "\n", 1)).unwrap_err().starts_with("line 3: "));
        for (section, (frame, records)) in back.iter().zip(&frames) {
            assert_eq!((section.class.as_str(), section.seed), ("dynamic-refit", default_seed()));
            assert_eq!((section.frame, &section.records), (*frame, records));
        }
        assert_eq!((back[4].class.as_str(), back[4].seed, &back[4].records), ("static-soup", 3, &soup));
        // Refusals and rays never dispatched are records like any other
        // to the format, and read back as what they were.
        let mut refused = frames[0].1.clone();
        refused[0] = [0, 0, 0, HIT_REFUSED | REFUSED_GEOMETRY, 0, 0, 0, 0];
        refused[1] = Answer::NotDispatched.words();
        let text_refused = hits_text("a test", &[(&case, 0, &refused)]).unwrap();
        let back = read_hits(&text_refused).unwrap();
        assert_eq!(Answer::of(back[0].records[0]), Answer::Refused(refused[0]));
        assert_eq!(Answer::of(back[0].records[1]), Answer::NotDispatched);
        // One record short, a frame the case does not have, a frame twice.
        assert!(hits_text("a test", &[(&case, 2, &frames[2].1[1..])]).is_err());
        assert!(hits_text("a test", &[(&case, 4, &frames[0].1)]).is_err());
        assert!(hits_text("a test", &[(&case, 1, &frames[1].1), (&case, 1, &frames[1].1)]).is_err());
        // A file written for another generator is refused on reading.
        let other = text.replacen(&format!("case {:016x}", case.fingerprint), "case 0000000000000001", 1);
        assert!(read_hits(&other).unwrap_err().contains("generator differs"));
    }
}
