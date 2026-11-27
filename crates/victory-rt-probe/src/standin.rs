//! Test code: a device on the CPU, so that the run's own logic is tested
//! without one. It keeps the bytes a build is handed and answers a
//! dispatch by brute force over them, in double precision for triangles
//! and with `rayquery::triangle_test` for the hybrid's triangle data,
//! reading both as a device and the shaders would. It is no path and no
//! reference: it exists to show that what the probe hands a device
//! describes the scene the CPU reference traces, and that the run counts
//! what a device does and fails to do.
//!
//! Its times and memory figures are made up, and made up so that no two
//! quantities share a value: each call reports figures of its own, which
//! vary from call to call, and everything reported is kept in the order of
//! the calls. A test can therefore hold every measurement column of a row
//! against what the device reported for exactly that quantity, and a
//! figure that reached another column, or came from another dispatch,
//! shows.

use std::time::Duration;

use victory_gpu::rt::desc::IndexFormat;
use victory_shader::emit::rayquery::{
    Form, HIT_BACK, HIT_FRONT, HIT_PROCEDURAL, HIT_REFUSED, REFUSED_GEOMETRY, REFUSED_RANGE, RayQueryShader,
    alpha_accepts, triangle_test,
};

use crate::device::{Built, Device, Level, Memory, POISON, Path, Structures, Trace, Traced};
use crate::inputs::{BottomInputs, TopInstance};
use crate::scene::{Action, HitWords, RayWords};

/// What a path's structures hold: the bytes of its last builds.
#[derive(Default)]
pub(crate) struct Held {
    bottoms: Vec<BottomInputs>,
    instances: Vec<TopInstance>,
    triangle_data: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct StandIn {
    // The fields a test does not set are open to it all the same: a test
    // fills the rest with `..StandIn::default()`.
    pub held: [Option<Held>; 2],
    pub rays: Vec<RayWords>,
    pub hits: Vec<HitWords>,
    /// Every call, in a line.
    pub log: Vec<String>,
    pub dispatches: u32,
    pub builds: u32,
    /// What each call reported, in the order of the calls.
    pub traced: Vec<(Trace, Traced)>,
    pub built: Vec<(Path, Built)>,
    pub uploaded: Vec<(u64, Duration)>,
    pub read: Vec<(u32, Duration)>,
    pub memories: Vec<Memory>,
    /// Tracing dispatches since the last build.
    pub since_build: u32,
    /// What its validation layer has said and nobody has asked for yet.
    pub pending: Vec<String>,
    /// Faults a test asks for. A pipeline that is not made:
    pub no_pipeline: Option<Form>,
    /// The build call (from 0, over the run) that fails:
    pub fail_build: Option<u32>,
    /// The native shader refuses every so-manyth listed ray (geometry),
    /// the hybrid's every so-manyth (range):
    pub native_refuses_every: Option<u32>,
    pub hybrid_refuses_every: Option<u32>,
    /// A refit is granted and not done: the structure keeps its old bytes.
    pub stale_refits: bool,
    /// The tracing shaders write nothing:
    pub writes_nothing: bool,
    /// The tracing shaders leave every so-manyth listed record unwritten:
    pub leaves_unwritten_every: Option<u32>,
    /// A dispatch of any shader writes the first so-many records it lists
    /// and no more:
    pub writes_at_most: Option<u32>,
    /// Every tracing dispatch but the first after a build writes another
    /// record 1:
    pub flickers: bool,
    /// The dispatch (from 0, over the run) that fails:
    pub fail_dispatch: Option<u32>,
    /// Every so-manyth dispatch has no time:
    pub untimed_every: Option<u32>,
    /// A validation layer is on, and has something to say of every
    /// pipeline and every build:
    pub complains: bool,
}

/// The made-up processor time of a pipeline, by its shader.
pub(crate) fn pipeline_time(form: Form) -> Duration {
    Duration::from_micros(match form {
        Form::Null => 1500,
        Form::Triangles => 2500,
        Form::Procedural => 3500,
    })
}

fn place(path: Path) -> usize {
    Path::ALL.iter().position(|p| *p == path).expect("a path")
}

fn f(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn d(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A point (`w` 1) or a direction (`w` 0) through three rows of four.
fn through(m: &[f64; 12], p: V3, w: f64) -> V3 {
    std::array::from_fn(|r| m[4 * r] * p[0] + m[4 * r + 1] * p[1] + m[4 * r + 2] * p[2] + m[4 * r + 3] * w)
}

/// The inverse of three rows of four, as three rows of four.
fn inverse(m: &[f32; 12]) -> [f64; 12] {
    let a = |r: usize, c: usize| f64::from(m[4 * r + c]);
    let rows: [V3; 3] = std::array::from_fn(|r| [a(r, 0), a(r, 1), a(r, 2)]);
    let det = dot(rows[0], cross(rows[1], rows[2]));
    let co = [cross(rows[1], rows[2]), cross(rows[2], rows[0]), cross(rows[0], rows[1])];
    // The inverse's row r is column r of the cofactors over the determinant.
    let inv: [V3; 3] = std::array::from_fn(|r| [co[0][r] / det, co[1][r] / det, co[2][r] / det]);
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    let mut out = [0.0; 12];
    for r in 0..3 {
        out[4 * r..4 * r + 3].copy_from_slice(&inv[r]);
        out[4 * r + 3] = -dot(inv[r], t);
    }
    out
}

/// A ray as its words give it.
struct Ray {
    origin: V3,
    dir: V3,
    tmin: f32,
    tmax: f32,
    mask: u32,
    flags: u32,
}

impl Ray {
    fn of(words: &RayWords) -> Ray {
        let v = |k: usize| f64::from(f32::from_bits(words[k]));
        Ray {
            origin: [v(0), v(1), v(2)],
            dir: [v(4), v(5), v(6)],
            tmin: f32::from_bits(words[3]),
            tmax: f32::from_bits(words[7]),
            mask: words[8] & 0xFF,
            flags: words[9],
        }
    }
}

const FORCE_OPAQUE: u32 = 0x01;
const FORCE_NON_OPAQUE: u32 = 0x02;
const CULL_BACK: u32 = 0x10;
const CULL_FRONT: u32 = 0x20;
const CULL_OPAQUE: u32 = 0x40;
const CULL_NON_OPAQUE: u32 = 0x80;
const SKIP_TRIANGLES: u32 = 0x100;
const INSTANCE_CULL_DISABLE: u32 = 1;
const INSTANCE_COUNTERCLOCKWISE: u32 = 2;
const INSTANCE_FORCE_OPAQUE: u32 = 4;
const INSTANCE_FORCE_NON_OPAQUE: u32 = 8;

/// Whether a candidate is opaque: the geometry's flag, then the
/// instance's, then the ray's. `None` where the ray culls it.
fn opacity(geometry: bool, instance: u32, ray: u32) -> Option<bool> {
    let mut opaque = geometry;
    if instance & INSTANCE_FORCE_OPAQUE != 0 {
        opaque = true;
    }
    if instance & INSTANCE_FORCE_NON_OPAQUE != 0 {
        opaque = false;
    }
    if ray & FORCE_OPAQUE != 0 {
        opaque = true;
    }
    if ray & FORCE_NON_OPAQUE != 0 {
        opaque = false;
    }
    let culled = ray & if opaque { CULL_OPAQUE } else { CULL_NON_OPAQUE } != 0;
    (!culled).then_some(opaque)
}

/// Whether the ray's flags cull a face, under the instance's flags.
fn face_culled(front: bool, instance: u32, ray: u32) -> bool {
    instance & INSTANCE_CULL_DISABLE == 0 && ray & if front { CULL_FRONT } else { CULL_BACK } != 0
}

fn refusal(reason: u32) -> HitWords {
    [0, 0, 0, HIT_REFUSED | reason, 0, 0, 0, 0]
}

/// The nearest accepted hit of host triangles, as a device with the
/// triangles shader would report it.
fn trace_triangles(held: &Held, words: &RayWords) -> HitWords {
    let ray = Ray::of(words);
    let mut best: HitWords = [0; 8];
    let mut committed = ray.tmax;
    if ray.flags & SKIP_TRIANGLES != 0 {
        return best;
    }
    for (index, instance) in held.instances.iter().enumerate() {
        if u32::from(instance.mask) & ray.mask == 0 {
            continue;
        }
        let BottomInputs::Triangles(level) = &held.bottoms[instance.bottom as usize] else {
            return refusal(REFUSED_GEOMETRY);
        };
        let to_object = inverse(&instance.transform);
        let (origin, dir) = (through(&to_object, ray.origin, 1.0), through(&to_object, ray.dir, 0.0));
        let flags = u32::from(instance.flags);
        for (g, geometry) in level.geometries.iter().enumerate() {
            let Some(opaque) = opacity(geometry.flags.opaque, flags, ray.flags) else { continue };
            let transform: Option<[f64; 12]> = geometry
                .transform
                .map(|at| std::array::from_fn(|k| f64::from(f(&level.fixed, at as usize + 4 * k))));
            let corners = geometry.indices.map_or(geometry.vertex_count, |(_, _, count)| count);
            for primitive in 0..corners / 3 {
                let vertex = |k: u32| -> V3 {
                    let corner = (3 * primitive + k) as usize;
                    let index = match geometry.indices {
                        None => corner,
                        Some((at, IndexFormat::U16, _)) => {
                            let at = at as usize + 2 * corner;
                            usize::from(u16::from_le_bytes([level.fixed[at], level.fixed[at + 1]]))
                        }
                        Some((at, IndexFormat::U32, _)) => d(&level.fixed, at as usize + 4 * corner) as usize,
                    };
                    let at = geometry.vertex_offset as usize + 12 * index;
                    let p: V3 = std::array::from_fn(|x| f64::from(f(&level.positions, at + 4 * x)));
                    transform.as_ref().map_or(p, |m| through(m, p, 1.0))
                };
                let (a, b, c) = (vertex(0), vertex(1), vertex(2));
                let (e1, e2) = (sub(b, a), sub(c, a));
                let p = cross(dir, e2);
                let det = dot(e1, p);
                if det == 0.0 {
                    continue;
                }
                let s = sub(origin, a);
                let u = dot(s, p) / det;
                let q = cross(s, e1);
                let v = dot(dir, q) / det;
                let t = (dot(e2, q) / det) as f32;
                if u < 0.0 || v < 0.0 || u + v > 1.0 || !(t > ray.tmin && t < committed) {
                    continue;
                }
                // Clockwise from the origin, left-handed: the direction
                // against the normal.
                let front = (det > 0.0) != (flags & INSTANCE_COUNTERCLOCKWISE != 0);
                if face_culled(front, flags, ray.flags) {
                    continue;
                }
                let (u, v) = (u as f32, v as f32);
                if !opaque && !alpha_accepts(u, v, primitive) {
                    continue;
                }
                committed = t;
                let kind = if front { HIT_FRONT } else { HIT_BACK };
                best = [t.to_bits(), u.to_bits(), v.to_bits(), kind, index as u32, instance.id, g as u32, primitive];
            }
        }
    }
    best
}

/// Whether the ray's line can meet the box of six floats at `at` of
/// `boxes`, as a device that errs on the side of offering a candidate
/// would decide it: the box grown on every side by 2^-12 of the largest
/// coordinate in play. Wide enough that no rounding of this stand-in's
/// own loses a candidate; narrow enough that a box left where its triangle
/// was before the triangle moved is missed.
fn box_offered(boxes: &[u8], at: usize, origin: [f32; 3], dir: [f32; 3]) -> bool {
    let corner = |k: usize| f64::from(f(boxes, at + 4 * k));
    let largest = (0..6).map(|k| corner(k).abs()).chain(origin.iter().map(|x| f64::from(x.abs()))).fold(1.0, f64::max);
    let grow = largest / 4096.0;
    let (mut near, mut far) = (f64::NEG_INFINITY, f64::INFINITY);
    for axis in 0..3 {
        let (least, greatest) = (corner(axis) - grow, corner(axis + 3) + grow);
        let (o, d) = (f64::from(origin[axis]), f64::from(dir[axis]));
        if d == 0.0 {
            if o < least || o > greatest {
                return false;
            }
            continue;
        }
        let (a, b) = ((least - o) / d, (greatest - o) / d);
        near = near.max(a.min(b));
        far = far.min(a.max(b));
    }
    near <= far
}

/// The same of host boxes, each box the ray can meet offered as a
/// candidate ([`box_offered`]) and tested as the procedural shader tests
/// it: the triangle from the triangle data, the instance's flags from
/// there too.
fn trace_boxes(held: &Held, words: &RayWords) -> HitWords {
    let ray = Ray::of(words);
    let data = &held.triangle_data;
    let mut best: HitWords = [0; 8];
    let mut committed = ray.tmax;
    for (index, instance) in held.instances.iter().enumerate() {
        if u32::from(instance.mask) & ray.mask == 0 {
            continue;
        }
        let BottomInputs::Boxes(level) = &held.bottoms[instance.bottom as usize] else {
            return refusal(REFUSED_GEOMETRY);
        };
        let to_object = inverse(&instance.transform);
        let origin = through(&to_object, ray.origin, 1.0).map(|x| x as f32);
        let dir = through(&to_object, ray.dir, 0.0).map(|x| x as f32);
        let (table, flags) = (d(data, 8 * index) as usize, d(data, 8 * index + 4));
        for (g, geometry) in level.geometries.iter().enumerate() {
            let Some(opaque) = opacity(geometry.flags.opaque, u32::from(instance.flags), ray.flags) else {
                continue;
            };
            let first = d(data, table + 4 * g) as usize;
            for primitive in 0..geometry.count as u32 {
                if ray.flags & SKIP_TRIANGLES != 0 {
                    continue;
                }
                if !box_offered(&level.boxes, geometry.offset as usize + 24 * primitive as usize, origin, dir) {
                    continue;
                }
                let at = first + 36 * primitive as usize;
                let triangle: [[f32; 3]; 3] =
                    std::array::from_fn(|k| std::array::from_fn(|x| f(data, at + 12 * k + 4 * x)));
                let hit = match triangle_test(origin, dir, triangle) {
                    Err(_) => return refusal(REFUSED_RANGE),
                    Ok(None) => continue,
                    Ok(Some(hit)) => hit,
                };
                if !(hit.t > ray.tmin && hit.t < committed) {
                    continue;
                }
                let front = hit.clockwise != (flags & INSTANCE_COUNTERCLOCKWISE != 0);
                if face_culled(front, flags, ray.flags) || (!opaque && !alpha_accepts(hit.u, hit.v, primitive)) {
                    continue;
                }
                committed = hit.t;
                best = [
                    hit.t.to_bits(),
                    hit.u.to_bits(),
                    hit.v.to_bits(),
                    HIT_PROCEDURAL,
                    index as u32,
                    instance.id,
                    g as u32,
                    primitive,
                ];
            }
        }
    }
    best
}

impl Device for StandIn {
    fn identity(&self) -> Vec<(String, String)> {
        vec![("adapter".into(), "a stand-in on the CPU, in a test".into())]
    }

    fn pipeline(&mut self, shader: &RayQueryShader) -> Result<Duration, String> {
        self.log.push(format!("pipeline {:?}", shader.form));
        if self.complains {
            self.pending.push(format!("the {:?} pipeline: a message", shader.form));
        }
        if self.no_pipeline == Some(shader.form) {
            return Err("the stand-in was told to refuse it".into());
        }
        Ok(pipeline_time(shader.form))
    }

    fn rays(&mut self, list: &[u8]) -> Result<Duration, String> {
        self.rays = list.as_chunks::<48>().0.iter().map(|record| std::array::from_fn(|k| d(record, 4 * k))).collect();
        self.hits = vec![[0; 8]; self.rays.len()];
        self.log.push(format!("rays {}", self.rays.len()));
        let took = Duration::from_micros(200 + 17 * self.uploaded.len() as u64);
        self.uploaded.push((list.len() as u64, took));
        Ok(took)
    }

    fn build(&mut self, s: &Structures) -> Result<Built, String> {
        let actions: Vec<Action> = s.bottoms.iter().map(|(_, action)| *action).collect();
        self.log.push(format!(
            "build {} bottoms {actions:?} top {:?} data {}",
            s.path.name(),
            s.top,
            s.triangle_data.is_some()
        ));
        let call = self.builds;
        self.builds += 1;
        self.since_build = 0;
        if self.complains {
            self.pending.push(format!("build {call}: a message, with a comma"));
            self.pending.push(format!("build {call}: another"));
        }
        if self.fail_build == Some(call) {
            return Err("the stand-in was told to fail this build".into());
        }
        let held = self.held[place(s.path)].get_or_insert_with(Held::default);
        // Each figure its own, and other figures at every call.
        let n = u64::from(call);
        let step = 0.001 * f64::from(call);
        let mut built = Built {
            upload_cpu: Duration::from_micros(300 + 7 * n),
            record_cpu: Duration::from_micros(100 + 3 * n),
            wait: Duration::from_micros(700 + 11 * n),
            ..Built::default()
        };
        for (at, (level, action)) in s.bottoms.iter().enumerate() {
            let bytes = match level {
                Level::Triangles(t) => BottomInputs::Triangles((*t).clone()),
                Level::Boxes(b) => BottomInputs::Boxes((*b).clone()),
            };
            match action {
                Action::Keep if at < held.bottoms.len() => continue,
                Action::Keep => return Err(format!("bottom level {at} kept before it was built")),
                Action::Refit if at >= held.bottoms.len() => {
                    return Err(format!("bottom level {at} refitted before it was built"));
                }
                Action::Refit => {
                    built.refits += 1;
                    built.bottom_refit.add(Some(0.125 + step));
                    // A refit uploads what moves: positions, or boxes.
                    built.input_bytes += match level {
                        Level::Triangles(t) => t.positions.len() as u64,
                        Level::Boxes(b) => b.boxes.len() as u64,
                    };
                    if !self.stale_refits {
                        held.bottoms[at] = bytes;
                    }
                }
                Action::Rebuild => {
                    built.builds += 1;
                    built.bottom_build.add(Some(0.5 + step));
                    built.input_bytes += bytes.bytes();
                    if at < held.bottoms.len() {
                        held.bottoms[at] = bytes;
                    } else {
                        held.bottoms.push(bytes);
                    }
                }
            }
        }
        held.bottoms.truncate(s.bottoms.len());
        match s.top {
            Action::Keep => {}
            Action::Refit => {
                built.refits += 1;
                built.top_refit.add(Some(0.0625 + step));
                held.instances = s.instances.to_vec();
            }
            Action::Rebuild => {
                built.builds += 1;
                built.top_build.add(Some(0.25 + step));
                held.instances = s.instances.to_vec();
            }
        }
        if let Some(data) = s.triangle_data {
            held.triangle_data = data.to_vec();
            built.input_bytes += data.len() as u64;
        }
        self.built.push((s.path, built.clone()));
        Ok(built)
    }

    fn trace(&mut self, trace: &Trace) -> Result<Traced, String> {
        let rays = trace.rays as usize;
        if rays > self.rays.len() {
            return Err(format!("{rays} rays listed of {} uploaded", self.rays.len()));
        }
        let n = self.dispatches;
        self.dispatches += 1;
        if self.fail_dispatch == Some(n) {
            return Err("the stand-in was told to fail this dispatch".into());
        }
        self.log.push(format!("trace {:?} {rays}{}", trace.form, if trace.poison { " poisoned" } else { "" }));
        if trace.poison {
            self.hits[..rays].fill([POISON; 8]);
        }
        let every = |n: Option<u32>, k: usize| n.is_some_and(|n| (k as u32).is_multiple_of(n));
        let written = self.writes_at_most.map_or(rays, |most| rays.min(most as usize));
        for k in 0..written {
            let record = match (trace.form, trace.path) {
                (Form::Null, _) => [0; 8],
                _ if self.writes_nothing || every(self.leaves_unwritten_every, k) => continue,
                (form, Some(path)) => {
                    let held = self.held[place(path)].as_ref().ok_or("a dispatch over a path that is not built")?;
                    match form {
                        Form::Triangles if every(self.native_refuses_every, k) => refusal(REFUSED_GEOMETRY),
                        Form::Triangles => trace_triangles(held, &self.rays[k]),
                        _ if every(self.hybrid_refuses_every, k) => refusal(REFUSED_RANGE),
                        _ => trace_boxes(held, &self.rays[k]),
                    }
                }
                (_, None) => return Err("a tracing shader with no path".into()),
            };
            self.hits[k] = record;
        }
        if trace.form != Form::Null {
            if self.flickers && self.since_build > 0 && rays > 1 {
                self.hits[1][0] ^= 1;
            }
            self.since_build += 1;
        }
        let untimed = self.untimed_every.is_some_and(|every| self.dispatches.is_multiple_of(every));
        // Another time and another wait at every dispatch, in no order.
        let traced = Traced {
            gpu_ms: (!untimed).then(|| 1.0 + f64::from((n * 37) % 101) * 0.01),
            record_cpu: Duration::from_micros(50),
            wait: Duration::from_micros(900 + u64::from((n * 53) % 97)),
        };
        self.traced.push((*trace, traced));
        Ok(traced)
    }

    fn hits(&mut self, records: u32) -> Result<(Vec<u8>, Duration), String> {
        let wait = Duration::from_micros(400 + 13 * self.read.len() as u64);
        self.read.push((records, wait));
        let records = self.hits.get(..records as usize).ok_or("more records asked than the buffer holds")?;
        let bytes = records.iter().flatten().flat_map(|w| w.to_le_bytes()).collect();
        Ok((bytes, wait))
    }

    fn release(&mut self, path: Path) -> Result<(), String> {
        self.log.push(format!("release {}", path.name()));
        self.held[place(path)] = None;
        Ok(())
    }

    fn memory(&mut self) -> Memory {
        let structures: u64 =
            self.held.iter().flatten().flat_map(|h| &h.bottoms).map(BottomInputs::bytes).sum();
        // The adapter's and the process's figures: each its own, and
        // others at every call.
        let call = self.memories.len() as u64;
        let memory = Memory {
            structures,
            scratch: 32 * self.hits.len() as u64,
            buffers: 48 * self.rays.len() as u64,
            video_local_mb: Some(1000.0 + call as f64),
            video_budget_mb: Some(8000.5 + 0.25 * call as f64),
            video_shared_mb: Some(200.0 + 0.5 * call as f64),
            commit: Some(500_000_000 + 1_000_000 * call),
            working_set: Some(300_000_000 + 2_000_000 * call),
        };
        self.memories.push(memory);
        memory
    }

    fn counters(&self) -> String {
        format!("{} builds, {} dispatches", self.builds, self.dispatches)
    }

    fn messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending)
    }
}
