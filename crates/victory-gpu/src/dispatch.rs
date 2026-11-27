//! The console's dispatch flags, and dispatches counted in threads.
//!
//! `DispatchX(x, y, z, flags)` (`crate::list`) is the console's dispatch with
//! flags; the ray-tracing structure builder dispatches its stages this way.
//! The host has no such argument. What is known about it, and how well:
//!
//! * AMD's public register headers for this GPU generation (the Linux
//!   kernel's `gc_10_3_0_sh_mask.h`) give the dispatch initiator register,
//!   which every dispatch packet carries, a bit 5 (0x20)
//!   `USE_THREAD_DIMENSIONS`, beside bit 1 `PARTIAL_TG_EN`; and the thread
//!   count register has a "partial" field beside the full group's. Mesa's
//!   driver for the same hardware sets bit 5 where it passes the three
//!   dimensions in threads, not groups. So the hardware can count a dispatch
//!   in threads, start only those, and that is spelled 0x20. (A public
//!   header and an open driver: independent of every fork.)
//! * That `DispatchX`'s flags argument is those register bits is an
//!   inference. No public header for the console's method was found, and no
//!   disassembly of it is in the repositories read. The forks that follow
//!   flag 0x20 (one line of descent: one introduced it, the others took it
//!   from there) report from their own runs that a builder stage ran up to
//!   64 times its threads and wrote over the structures after its own
//!   while the counts were taken for groups, and name the flag among the
//!   causes of every ray missing. Their word, not shown here.
//!
//! Not enough to change what every run does without a run of ours: if the
//! flag meant something else, counts taken for threads would start a
//! sixty-fourth of the threads a stage needs. So the default is as before
//! (the flags are ignored: counts are groups), and
//! `VICTORY_DISPATCH_THREADS` follows the flag ([`Mode`]). In every run,
//! each distinct flags value is counted and logged with what it was passed
//! ([`Tally`]), so that one run tells what the title passes; see
//! [`Tally::note`] for what those lines can clear or make likely. What
//! confirms the reading is a run with the switch on.
//!
//! A host dispatch is whole thread groups. Counted in threads, a count that
//! is no multiple of the group's size along its dimension needs one more
//! group, of which only the first threads run: the program is told how
//! many threads run along each dimension and stops the others itself
//! (`victory_shader::emit`, "Thread limits"; such programs exist only with
//! the switch on). [`plan`] is that arithmetic, and what happens where the
//! program cannot stop them.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// The `DispatchX` flag taken to say that the three counts are threads.
pub(crate) const THREAD_COUNTS: u32 = 0x20;

/// The value of a thread limit that stops no thread.
#[cfg(any(windows, test))]
const NO_LIMIT: u32 = victory_shader::emit::THREAD_LIMIT_NONE;

/// The most thread groups a host dispatch takes along one dimension
/// (`D3D12_CS_DISPATCH_MAX_THREAD_GROUPS_PER_DIMENSION`).
#[cfg(any(windows, test))]
const MAX_GROUPS: u32 = 65_535;

/// What `VICTORY_DISPATCH_THREADS` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Unset or `0`: the flags are not followed; every count is groups.
    Groups,
    /// Set (`1`): with flag 0x20 the counts are threads. A dispatch that
    /// cannot run exactly those threads is not run.
    Threads,
    /// `round`: as [`Mode::Threads`], but a partial group the program
    /// cannot cut short runs whole, its extra threads counted and logged.
    /// For telling whether such a program is what stands in the way.
    /// (What the host cannot run at all is refused here too: [`plan`].)
    Rounded,
}

/// The switch's value as a [`Mode`]. Like the other switches, any value
/// but `0` is on.
fn parse_mode(value: Option<&str>) -> Mode {
    match value.map(str::trim) {
        None | Some("0") => Mode::Groups,
        Some(v) if v.eq_ignore_ascii_case("round") => Mode::Rounded,
        Some(_) => Mode::Threads,
    }
}

/// The run's [`Mode`], read once and said once when it is not the default.
pub(crate) fn mode() -> Mode {
    static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        let mode = parse_mode(std::env::var("VICTORY_DISPATCH_THREADS").ok().as_deref());
        if mode != Mode::Groups {
            tracing::warn!(
                ?mode,
                "VICTORY_DISPATCH_THREADS: DispatchX counts with flag 0x20 are taken for threads \
                 (experimental); compute shaders are translated with thread limits"
            );
        }
        mode
    })
}

/// Whether compute programs are translated with thread limits: whenever
/// the flag is followed. Part of what a translation depends on.
#[cfg(any(windows, test))]
pub(crate) fn thread_limits(mode: Mode) -> bool {
    mode != Mode::Groups
}

/// A dispatch as the host runs it.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    /// Thread groups along x, y and z.
    pub(crate) groups: [u32; 3],
    /// With `Some`, the program must be told, for each dimension it has a
    /// limit for, that this many threads run along it ([`NO_LIMIT`] where
    /// it has none or needs none). `None`: every thread of every group is
    /// one the title asked for, and the limits stay as they are bound.
    pub(crate) limits: Option<[u32; 3]>,
    /// Threads that run beyond those asked for: 0 except in
    /// [`Mode::Rounded`], for a program that cannot stop them.
    pub(crate) extra: u64,
}

/// What [`plan`] decides.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    Run(Run),
    /// The dispatch cannot be run as asked, and why: it is dropped and
    /// counted, never run with other threads than the title's.
    Refused(&'static str),
}

/// How the host runs a dispatch of `counts` with the console's `flags`
/// (`None`: the plain method, which has none).
///
/// Without [`THREAD_COUNTS`] in the flags, or in [`Mode::Groups`], the
/// counts are groups and go through unchanged, whatever else is known.
/// Otherwise they are threads of a program whose groups are `size` threads:
/// each dimension takes the groups that hold its threads, rounded up.
/// Where a count does not fill its last group, the threads past it must
/// not run: `limited` says along which dimensions the program can stop
/// them.
/// * All counts fill whole groups, or one count is 0 (nothing runs): no
///   limit is needed.
/// * Every partial dimension is limited: the limits are the counts.
/// * A partial dimension is not limited: refused in [`Mode::Threads`]; in
///   [`Mode::Rounded`] that dimension's last group runs whole and
///   [`Run::extra`] says how many threads that adds.
///
///
/// A dispatch counted in threads that the host cannot run is refused in
/// both modes: more than [`MAX_GROUPS`] groups along a dimension, or
/// threads numbered past what the program's 32-bit test at entry holds
/// (group times size plus thread would wrap, and wrapped numbers would
/// pass a limit they are beyond).
///
/// So a planned dispatch counted in threads runs exactly the threads asked
/// for unless [`Run::extra`] says otherwise, and none is ever left out.
/// (Counts taken for groups go to the host as they are, as before,
/// whatever their size.)
#[cfg(any(windows, test))]
pub(crate) fn plan(
    counts: [u32; 3],
    flags: Option<u32>,
    size: Option<[u32; 3]>,
    limited: [bool; 3],
    mode: Mode,
) -> Plan {
    let threads = flags.is_some_and(|f| f & THREAD_COUNTS != 0);
    if !threads || mode == Mode::Groups {
        return Plan::Run(Run { groups: counts, limits: None, extra: 0 });
    }
    let Some(size) = size else {
        return Plan::Refused("counted in threads, and the program's thread-group size is not known");
    };
    if size.contains(&0) {
        return Plan::Refused("counted in threads, by a program whose thread group has no threads");
    }
    let groups: [u32; 3] = std::array::from_fn(|i| counts[i].div_ceil(size[i]));
    if groups.iter().any(|&n| n > MAX_GROUPS) {
        return Plan::Refused("counted in threads, in more thread groups along a dimension than the host runs (65535)");
    }
    // The last thread's number along each dimension is below
    // groups x size: that must fit 32 bits, and stay below `NO_LIMIT`.
    if (0..3).any(|i| u64::from(groups[i]) * u64::from(size[i]) > u64::from(NO_LIMIT)) {
        return Plan::Refused("counted in threads, with thread numbers that do not fit 32 bits");
    }
    let partial: [bool; 3] = std::array::from_fn(|i| !counts[i].is_multiple_of(size[i]));
    if counts.contains(&0) || partial == [false; 3] {
        return Plan::Run(Run { groups, limits: None, extra: 0 });
    }
    let uncut = (0..3).any(|i| partial[i] && !limited[i]);
    if uncut && mode == Mode::Threads {
        return Plan::Refused("counted in threads, a partial thread group, and the program has no thread limit");
    }
    let limits = std::array::from_fn(|i| if limited[i] { counts[i] } else { NO_LIMIT });
    // What runs along each dimension: the count, or whole groups where
    // nothing stops the last one's other threads.
    let product = |f: &dyn Fn(usize) -> u128| (0..3).map(f).product::<u128>();
    let ran = product(&|i| {
        if partial[i] && !limited[i] {
            u128::from(groups[i]) * u128::from(size[i])
        } else {
            u128::from(counts[i])
        }
    });
    let asked = product(&|i| u128::from(counts[i]));
    let extra = u64::try_from(ran - asked).unwrap_or(u64::MAX);
    Plan::Run(Run { groups, limits: Some(limits), extra })
}

/// Whether thread `thread` of group `group` runs under `limits`, in a
/// program whose groups are `size` threads: the test such a program makes
/// at entry (`victory_shader::emit`, "Thread limits"; its own tests check
/// the translated code against the same rule).
#[cfg(test)]
fn runs(group: [u32; 3], thread: [u32; 3], size: [u32; 3], limits: [u32; 3]) -> bool {
    (0..3).all(|i| size[i] <= 1 || group[i] * size[i] + thread[i] < limits[i])
}

/// The dispatch methods that take console flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// (The mesh method is recorded only where there is a host.)
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum Method {
    DispatchX,
    DispatchMeshX,
}

impl Method {
    /// The flag bits of the method whose meaning is taken as known.
    fn known(self) -> u64 {
        match self {
            Method::DispatchX => THREAD_COUNTS.into(),
            Method::DispatchMeshX => 0,
        }
    }
}

/// One call of a method with flags, as far as the tally looks at it.
pub(crate) struct Call<'a> {
    pub(crate) method: Method,
    pub(crate) flags: u64,
    pub(crate) counts: [u32; 3],
    /// The thread-group size of the compute program it runs, if it has a
    /// translated one.
    pub(crate) size: Option<[u32; 3]>,
    /// What runs: the host pipeline's name, or why there is none.
    pub(crate) program: &'a str,
    /// Whether a value is one of the call's own 32-bit root constants.
    pub(crate) is_constant: &'a dyn Fn(u32) -> bool,
    /// Those constants, for the line of a first sight.
    pub(crate) constants: &'a dyn Fn() -> Vec<u32>,
    /// Whether the counts are taken for threads in this run.
    pub(crate) followed: bool,
}

/// A line for the log; `warn` for what nothing here accounts for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) warn: bool,
    pub(crate) text: String,
}

/// Distinct flags values kept apart (per method); further ones are counted
/// together.
const VALUES: usize = 16;
/// Programs listed per flags value; further ones are counted, not listed.
/// (More than a structure builder has stages, with room for the title's
/// other users of the method; the whole tally stays under 20 KiB.)
const PROGRAMS: usize = 128;

#[derive(Clone, Copy)]
struct Seen {
    method: Method,
    flags: u64,
    calls: u64,
    /// Calls of a program whose thread group is wider than one thread along
    /// a dimension with a count above 1 (the others say nothing either way).
    wide: u64,
    /// Of those: a count that is no multiple of the group's size there.
    partial: u64,
    /// Of those: such a count is also one of the call's root constants.
    constant: u64,
    /// The programs seen with it, by a hash of their names.
    programs: [u64; PROGRAMS],
    /// Which of them had a call shown whose count is no multiple of the
    /// group's size (a bit per place in `programs`).
    partial_shown: [u64; PROGRAMS.div_ceil(64)],
    listed: usize,
}

/// The flags values seen and what was passed with them: fixed in size, so
/// it holds no memory of its own however long the run.
pub(crate) struct Tally {
    seen: [Option<Seen>; VALUES],
    /// Calls with a flags value that found no place in `seen`.
    others: u64,
}

/// Whether `counts` say anything about threads against groups for a program
/// of `size`: (some dimension is wider than a thread with a count above 1,
/// such a count is no multiple of the size, such a count is a constant).
fn telling(counts: [u32; 3], size: [u32; 3], is_constant: &dyn Fn(u32) -> bool) -> (bool, bool, bool) {
    let wide = |i: &usize| size[*i] > 1 && counts[*i] > 1;
    let dims = || (0..3).filter(wide);
    (
        dims().next().is_some(),
        dims().any(|i| !counts[i].is_multiple_of(size[i])),
        dims().any(|i| is_constant(counts[i])),
    )
}

/// FNV-1a of `bytes`; never 0, which stands for an empty place.
fn hash(bytes: impl Iterator<Item = u8>) -> u64 {
    let h = bytes.fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    h.max(1)
}

fn by(v: [u32; 3]) -> String {
    format!("{}x{}x{}", v[0], v[1], v[2])
}

impl Tally {
    pub(crate) const fn new() -> Self {
        Self { seen: [None; VALUES], others: 0 }
    }

    /// Counts a call and gives the lines it is the occasion for:
    /// * the first call with a flags value whose bits are not all known:
    ///   a warning naming the bits (they are ignored);
    /// * the first call of each program with a flags value: the counts,
    ///   the program's thread-group size, how many threads that is taken
    ///   either way, and the call's root constants;
    /// * the same once more for the program's first call with a count
    ///   above 1 that is no multiple of its group's size, if its first
    ///   call was not one (a first call that happens to fill whole groups
    ///   tells nothing);
    /// * the 2nd, 4th, 8th... call with a flags value: the totals so far,
    ///   beside the plain method's ([`plain`]).
    ///
    /// The first [`PROGRAMS`] programs of a flags value are listed; calls
    /// of further ones are in the totals only, which a line says once.
    ///
    /// What a run with the default settings can tell about threads against
    /// groups, and what it cannot. It clears the flag if 0x20 is never
    /// passed, or never to the stages in question. It makes the thread
    /// reading likely, no more, by two signs: a stage that works on n
    /// elements with groups of 64 is dispatched with n counted in threads
    /// and about n/64 counted in groups, and usually gets n as a constant
    /// too, so counts passed with flag 0x20 that are again and again among
    /// the call's own root constants (the totals' last figure), while the
    /// plain method's almost never are, point to threads; so do counts
    /// that are no multiple of the group and thread totals that are only
    /// sensible one way. Neither is proof (a title may pass a group count
    /// as a constant). What settles it is a run with the switch on.
    pub(crate) fn note(&mut self, call: &Call) -> Vec<Line> {
        let mut lines = Vec::new();
        let key = |s: &Seen| s.method == call.method && s.flags == call.flags;
        let at = match self.seen.iter().position(|s| s.as_ref().is_some_and(key)) {
            Some(at) => at,
            None => match self.seen.iter().position(Option::is_none) {
                Some(at) => {
                    self.seen[at] = Some(Seen {
                        method: call.method,
                        flags: call.flags,
                        calls: 0,
                        wide: 0,
                        partial: 0,
                        constant: 0,
                        programs: [0; PROGRAMS],
                        partial_shown: [0; PROGRAMS.div_ceil(64)],
                        listed: 0,
                    });
                    at
                }
                None => {
                    self.others += 1;
                    if self.others.is_power_of_two() {
                        lines.push(Line {
                            warn: true,
                            text: format!(
                                "{:?} flags {:#x}: more than {VALUES} distinct flags values; {} calls with further ones counted together",
                                call.method, call.flags, self.others
                            ),
                        });
                    }
                    return lines;
                }
            },
        };
        let s = self.seen[at].as_mut().expect("just found or made");
        s.calls += 1;
        let unknown = call.flags & !call.method.known();
        if s.calls == 1 && unknown != 0 {
            // For whoever reads the line: the register's other bits, should
            // the flags turn out to be the register's.
            let hint = match call.method {
                Method::DispatchX => {
                    "; in AMD's dispatch initiator register 0x2 is PARTIAL_TG_EN, 0x4 FORCE_START_AT_000, \
                     0x8 and 0x10 ordered append, 0x40 ORDER_MODE, 0x8000 CS_W32_EN"
                }
                Method::DispatchMeshX => "",
            };
            lines.push(Line {
                warn: true,
                text: format!(
                    "{:?} flags {:#x}: bits {unknown:#x} are not known and are ignored (logged once per flags value){hint}",
                    call.method, call.flags
                ),
            });
        }
        let told = call.size.map(|size| telling(call.counts, size, call.is_constant));
        if let Some((wide, partial, constant)) = told {
            s.wide += u64::from(wide);
            s.partial += u64::from(partial);
            s.constant += u64::from(constant);
        }
        let partial = told.is_some_and(|(_, partial, _)| partial);
        // The call in full, as `what` of its program.
        let shown = |what: &str| {
            let threads = |per_count: [u32; 3]| {
                (0..3).map(|i| u128::from(call.counts[i]) * u128::from(per_count[i])).product::<u128>()
            };
            let size = match call.size {
                Some(size) => format!(
                    "thread group {} ({} threads if the counts are groups, {} if they are threads)",
                    by(size),
                    threads(size),
                    threads([1; 3])
                ),
                None => "no translated compute program".to_owned(),
            };
            let taken = match (call.method, call.flags & u64::from(THREAD_COUNTS) != 0, call.followed) {
                (Method::DispatchX, true, true) => "taken for threads (VICTORY_DISPATCH_THREADS)",
                (Method::DispatchX, true, false) => "taken for groups (VICTORY_DISPATCH_THREADS is off)",
                _ => "taken for groups",
            };
            Line {
                warn: false,
                text: format!(
                    "{:?} flags {:#x}, {what}: counts {} {taken}; {size}; root constants {:x?}; {}",
                    call.method,
                    call.flags,
                    by(call.counts),
                    (call.constants)(),
                    call.program
                ),
            }
        };
        let program = hash(call.program.bytes());
        match s.programs[..s.listed.min(PROGRAMS)].iter().position(|&h| h == program) {
            Some(at) => {
                let (word, bit) = (at / 64, 1u64 << (at % 64));
                if partial && s.partial_shown[word] & bit == 0 {
                    s.partial_shown[word] |= bit;
                    lines.push(shown(
                        "first call of this program with a count that is no multiple of its thread group",
                    ));
                }
            }
            None if s.listed < PROGRAMS => {
                let at = s.listed;
                s.programs[at] = program;
                s.listed += 1;
                if partial {
                    s.partial_shown[at / 64] |= 1 << (at % 64);
                }
                lines.push(shown("first call of this program"));
            }
            None if s.listed == PROGRAMS => {
                // Said once; further programs are in the totals only.
                s.listed += 1;
                lines.push(Line {
                    warn: false,
                    text: format!(
                        "{:?} flags {:#x}: more than {PROGRAMS} programs; further ones are counted, not listed",
                        call.method, call.flags
                    ),
                });
            }
            None => {}
        }
        if s.calls > 1 && s.calls.is_power_of_two() {
            let (plain_wide, plain_constant) = plain();
            lines.push(Line {
                warn: false,
                text: format!(
                    "{:?} flags {:#x}: {} calls so far; {} with a count above 1 where the thread group is wider than a thread, \
                     of which {} with a count that is no multiple of the group and {} with such a count among the call's root constants \
                     (plain Dispatch: {plain_constant} of {plain_wide})",
                    s.method, s.flags, s.calls, s.wide, s.partial, s.constant
                ),
            });
        }
        lines
    }

    /// How often a flags value of a method was passed (tests).
    #[cfg(test)]
    fn calls(&self, method: Method, flags: u64) -> u64 {
        let found = self.seen.iter().flatten().find(|s| s.method == method && s.flags == flags);
        found.map_or(0, |s| s.calls)
    }
}

static TALLY: Mutex<Tally> = Mutex::new(Tally::new());

/// Counts a call of a method with flags and logs what [`Tally::note`]
/// gives. The methods with flags are few a frame (the builder's stages).
pub(crate) fn note(call: &Call) {
    log(TALLY.lock().unwrap_or_else(|e| e.into_inner()).note(call));
}

fn log(lines: Vec<Line>) {
    for line in lines {
        if line.warn {
            tracing::warn!("{}", line.text);
        } else {
            tracing::info!("{}", line.text);
        }
    }
}

/// Plain dispatches whose counts tell something ([`telling`]), and those of
/// them with such a count among their root constants: what the totals of
/// the methods with flags are read against.
static PLAIN_WIDE: AtomicU64 = AtomicU64::new(0);
static PLAIN_CONSTANT: AtomicU64 = AtomicU64::new(0);

/// Counts a plain `Dispatch` of a program with thread groups of `size`.
#[cfg(any(windows, test))]
pub(crate) fn note_plain(counts: [u32; 3], size: [u32; 3], is_constant: &dyn Fn(u32) -> bool) {
    let (wide, _, constant) = telling(counts, size, is_constant);
    if wide {
        PLAIN_WIDE.fetch_add(1, Relaxed);
        PLAIN_CONSTANT.fetch_add(u64::from(constant), Relaxed);
    }
}

/// (Plain dispatches whose counts tell something, those with such a count
/// among their root constants) so far.
fn plain() -> (u64, u64) {
    (PLAIN_WIDE.load(Relaxed), PLAIN_CONSTANT.load(Relaxed))
}

/// Distinct (type, fields) of indirect dispatch arguments kept apart.
const INDIRECT_VALUES: usize = 16;

/// The indirect dispatch arguments seen: fixed in size.
struct Indirect {
    /// The distinct (type, fields), by hash; 0 for an empty place.
    seen: [AtomicU64; INDIRECT_VALUES],
    /// Executions with a value that found no place in `seen`.
    others: AtomicU64,
    /// Whether one of those had the bit of [`THREAD_COUNTS`] in a field
    /// (said once, whatever else is not listed).
    others_threads: AtomicBool,
}

impl Indirect {
    const fn new() -> Self {
        Self {
            seen: [const { AtomicU64::new(0) }; INDIRECT_VALUES],
            others: AtomicU64::new(0),
            others_threads: AtomicBool::new(false),
        }
    }

    /// Counts an indirect dispatch with signature argument `kind` and
    /// `fields`, and gives the lines it is the occasion for:
    /// * the first sight of each of the first [`INDIRECT_VALUES`] distinct
    ///   values: the value, as a warning if a field has the bit that
    ///   [`THREAD_COUNTS`] is in `DispatchX`'s flags;
    /// * of the values past those, which are counted together: the first
    ///   with that bit, as the same warning (once: a signature that may ask
    ///   for counts in threads is never passed over because others came
    ///   first), and the count at the 1st, 2nd, 4th... execution.
    fn note(&self, kind: u32, fields: [u32; 3]) -> Vec<Line> {
        let words = [kind, fields[0], fields[1], fields[2]];
        let key = hash(words.iter().flat_map(|w| w.to_le_bytes()));
        let threads = fields.iter().any(|f| f & THREAD_COUNTS != 0);
        let in_threads = |how_often: &str| Line {
            warn: true,
            text: format!(
                "an indirect dispatch (argument type {kind}) whose signature fields {fields:x?} have bit 0x20: \
                 if that is the flag for counts in threads, its counts are still taken for groups ({how_often})"
            ),
        };
        for seen in &self.seen {
            match seen.compare_exchange(0, key, Relaxed, Relaxed) {
                // The first sight of it.
                Ok(_) if threads => return vec![in_threads("logged once")],
                Ok(_) => {
                    return vec![Line {
                        warn: false,
                        text: format!(
                            "an indirect dispatch (argument type {kind}) with signature fields {fields:x?}: \
                             counts taken for groups (logged once)"
                        ),
                    }];
                }
                Err(k) if k == key => return Vec::new(),
                Err(_) => {}
            }
        }
        // No place left: not known whether it was seen before.
        let mut lines = Vec::new();
        if threads && !self.others_threads.swap(true, Relaxed) {
            lines.push(in_threads(
                "logged once for every such value past the distinct ones kept apart",
            ));
        }
        let n = self.others.fetch_add(1, Relaxed) + 1;
        if n.is_power_of_two() {
            lines.push(Line {
                warn: true,
                text: format!(
                    "indirect dispatches: more than {INDIRECT_VALUES} distinct signature values; \
                     {n} executions with further ones counted together, their counts taken for groups \
                     (the latest: argument type {kind}, fields {fields:x?})"
                ),
            });
        }
        lines
    }
}

static INDIRECT: Indirect = Indirect::new();

/// An indirect execution of a dispatch whose signature argument is `kind`
/// (2: a dispatch; 105: the console's single-pass dispatch) with `fields`
/// after the type. Its counts are in GPU memory, where the host takes them
/// for groups whatever the fields say: such counts cannot be turned into
/// groups here, and the dispatch runs as before. What is logged, and how
/// what is not listed is counted: [`Indirect::note`].
pub(crate) fn note_indirect(kind: u32, fields: [u32; 3]) {
    log(INDIRECT.note(kind, fields));
}

/// Dispatches counted in threads so far: run in whole groups, run with
/// thread limits, run with extra threads, and refused.
#[cfg(any(windows, test))]
static OUTCOMES: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// The threads [`Mode::Rounded`] ran beyond those asked for.
#[cfg(any(windows, test))]
static EXTRA_THREADS: AtomicU64 = AtomicU64::new(0);

/// Counts how a dispatch counted in threads went and says so at the 1st,
/// 2nd, 4th... of each kind; a refusal or extra threads as a warning with
/// the program's name. `counts` are the title's, `plan` what [`plan`]
/// made of them.
#[cfg(any(windows, test))]
pub(crate) fn outcome(counts: [u32; 3], size: Option<[u32; 3]>, plan: &Plan, program: &str) {
    let kind = match plan {
        Plan::Run(Run { extra: 0, limits: None, .. }) => 0,
        Plan::Run(Run { extra: 0, limits: Some(_), .. }) => 1,
        Plan::Run(_) => 2,
        Plan::Refused(_) => 3,
    };
    let n = OUTCOMES[kind].fetch_add(1, Relaxed) + 1;
    let size = size.map_or_else(|| "unknown".to_owned(), by);
    match plan {
        Plan::Run(run) if run.extra > 0 => {
            let total = EXTRA_THREADS.fetch_add(run.extra, Relaxed).saturating_add(run.extra);
            if n.is_power_of_two() {
                tracing::warn!(
                    "a dispatch of {} threads (thread group {size}) ran as {} groups with {} threads the title did not ask for: \
                     the program has no thread limit (VICTORY_DISPATCH_THREADS=round; {n} such dispatches, {total} extra threads so far): {program}",
                    by(counts),
                    by(run.groups),
                    run.extra
                );
            }
        }
        Plan::Run(run) => {
            if n.is_power_of_two() {
                tracing::info!(
                    "a dispatch of {} threads (thread group {size}) ran as {} groups{} ({n} such so far): {program}",
                    by(counts),
                    by(run.groups),
                    if run.limits.is_some() { ", the last ones cut short by thread limits" } else { ", all whole" }
                );
            }
        }
        Plan::Refused(why) => {
            if n.is_power_of_two() {
                tracing::warn!(
                    "a dispatch of {} threads (thread group {size}) dropped: {why} ({n} such so far): {program}",
                    by(counts)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_off_unless_set() {
        assert_eq!(parse_mode(None), Mode::Groups);
        assert_eq!(parse_mode(Some("0")), Mode::Groups);
        assert_eq!(parse_mode(Some(" 0 ")), Mode::Groups);
        assert_eq!(parse_mode(Some("1")), Mode::Threads);
        assert_eq!(parse_mode(Some("")), Mode::Threads);
        assert_eq!(parse_mode(Some("round")), Mode::Rounded);
        assert_eq!(parse_mode(Some("ROUND")), Mode::Rounded);
        assert!(!thread_limits(Mode::Groups));
        assert!(thread_limits(Mode::Threads) && thread_limits(Mode::Rounded));
    }

    const ALL: [bool; 3] = [true; 3];
    const NONE: [bool; 3] = [false; 3];

    fn run(groups: [u32; 3], limits: Option<[u32; 3]>, extra: u64) -> Plan {
        Plan::Run(Run { groups, limits, extra })
    }

    /// Off, or without the flag, nothing about a dispatch changes, whatever
    /// the program is: the default is as before.
    #[test]
    fn counts_are_groups_unless_the_flag_is_followed() {
        let counts = [1000, 3, 2];
        for flags in [None, Some(0), Some(0x20), Some(0xffff_ffff)] {
            for size in [None, Some([64, 1, 1]), Some([0, 0, 0])] {
                assert_eq!(plan(counts, flags, size, ALL, Mode::Groups), run(counts, None, 0));
            }
        }
        for mode in [Mode::Threads, Mode::Rounded] {
            for flags in [None, Some(0), Some(0x1f), Some(!0x20)] {
                assert_eq!(plan(counts, flags, Some([64, 1, 1]), NONE, mode), run(counts, None, 0));
                assert_eq!(plan(counts, flags, None, NONE, mode), run(counts, None, 0));
            }
        }
    }

    #[test]
    fn whole_groups_need_no_limit() {
        for mode in [Mode::Threads, Mode::Rounded] {
            for limited in [ALL, NONE] {
                let p = |counts, size| plan(counts, Some(0x20), Some(size), limited, mode);
                assert_eq!(p([128, 1, 1], [64, 1, 1]), run([2, 1, 1], None, 0));
                assert_eq!(p([64, 16, 4], [8, 8, 1]), run([8, 2, 4], None, 0));
                assert_eq!(p([7, 5, 3], [1, 1, 1]), run([7, 5, 3], None, 0));
                // Nothing runs: no group along one dimension.
                assert_eq!(p([0, 5, 3], [64, 4, 4]), run([0, 2, 1], None, 0));
                assert_eq!(p([70, 0, 1], [64, 1, 1]), run([2, 0, 1], None, 0));
            }
        }
    }

    #[test]
    fn a_partial_group_is_cut_by_the_programs_limit() {
        for mode in [Mode::Threads, Mode::Rounded] {
            let p = |counts, size, limited| plan(counts, Some(0x20), Some(size), limited, mode);
            assert_eq!(p([1000, 1, 1], [64, 1, 1], [true, false, false]), run([16, 1, 1], Some([1000, NO_LIMIT, NO_LIMIT]), 0));
            assert_eq!(p([1, 1, 1], [64, 1, 1], [true, false, false]), run([1, 1, 1], Some([1, NO_LIMIT, NO_LIMIT]), 0));
            // Other flag bits beside it change nothing.
            assert_eq!(
                plan([65, 1, 1], Some(0x21), Some([64, 1, 1]), ALL, mode),
                run([2, 1, 1], Some([65, 1, 1]), 0)
            );
            // Partial along y only; x fills its groups and still gets its count.
            assert_eq!(p([16, 9, 1], [8, 8, 1], [true, true, false]), run([2, 2, 1], Some([16, 9, NO_LIMIT]), 0));
        }
    }

    #[test]
    fn a_partial_group_without_a_limit_is_refused_or_counted() {
        let p = |counts, size, limited, mode| plan(counts, Some(0x20), Some(size), limited, mode);
        assert!(matches!(p([65, 1, 1], [64, 1, 1], NONE, Mode::Threads), Plan::Refused(_)));
        // One thread asked for beyond a group: 63 more run.
        assert_eq!(p([65, 1, 1], [64, 1, 1], NONE, Mode::Rounded), run([2, 1, 1], Some([NO_LIMIT; 3]), 63));
        // Limited along x, not along y: y's last group runs whole.
        let half = [true, false, false];
        assert!(matches!(p([9, 9, 1], [8, 8, 1], half, Mode::Threads), Plan::Refused(_)));
        assert_eq!(p([9, 9, 1], [8, 8, 1], half, Mode::Rounded), run([2, 2, 1], Some([9, NO_LIMIT, NO_LIMIT]), 9 * 16 - 81));
        // The size is needed to count groups at all.
        for mode in [Mode::Threads, Mode::Rounded] {
            assert!(matches!(plan([64, 1, 1], Some(0x20), None, ALL, mode), Plan::Refused(_)));
            assert!(matches!(plan([64, 1, 1], Some(0x20), Some([64, 0, 1]), ALL, mode), Plan::Refused(_)));
        }
    }

    /// What the host cannot run is refused, not handed over: more groups
    /// along a dimension than a host dispatch takes, and thread numbers the
    /// program's 32-bit test would wrap (one of size 7 would let threads
    /// 4 to 6 of the last group of `u32::MAX` threads run as numbers 0 to 2).
    #[test]
    fn what_the_host_cannot_run_is_refused() {
        let p = |counts, size, limited, mode| plan(counts, Some(0x20), Some(size), limited, mode);
        for mode in [Mode::Threads, Mode::Rounded] {
            for limited in [ALL, NONE] {
                // The most groups there are, whole and partial.
                assert_eq!(p([MAX_GROUPS * 64, 1, 1], [64, 1, 1], limited, mode), run([MAX_GROUPS, 1, 1], None, 0));
                assert!(matches!(p([MAX_GROUPS * 64 + 1, 1, 1], [64, 1, 1], limited, mode), Plan::Refused(_)));
                assert!(matches!(p([65_536 * 64, 1, 1], [64, 1, 1], limited, mode), Plan::Refused(_)));
                assert!(matches!(p([1, 1, 65_536], [64, 1, 1], limited, mode), Plan::Refused(_)));
                assert!(matches!(p([u32::MAX, 1, 1], [7, 1, 1], limited, mode), Plan::Refused(_)));
                assert!(matches!(p([u32::MAX; 3], [64, 8, 8], limited, mode), Plan::Refused(_)));
                // Few enough groups, but their threads number past 32 bits.
                assert!(matches!(p([u32::MAX, 1, 1], [70_000, 1, 1], limited, mode), Plan::Refused(_)));
                // Even where nothing would run.
                assert!(matches!(p([0, 1, 65_536], [64, 1, 1], limited, mode), Plan::Refused(_)));
            }
            // The last partial group the host takes.
            assert_eq!(
                p([MAX_GROUPS * 64 - 1, 1, 1], [64, 1, 1], ALL, mode),
                run([MAX_GROUPS, 1, 1], Some([MAX_GROUPS * 64 - 1, 1, 1]), 0)
            );
        }
        // Thread numbers up to the last one 32 bits hold, in whole groups.
        assert_eq!(p([u32::MAX, 1, 1], [65_537, 1, 1], NONE, Mode::Threads), run([MAX_GROUPS, 1, 1], None, 0));
        // Counts taken for groups are the host's business, as before.
        let all = [u32::MAX; 3];
        assert_eq!(plan(all, Some(0x20), Some([64, 8, 8]), ALL, Mode::Groups), run(all, None, 0));
        assert_eq!(plan(all, None, Some([64, 8, 8]), ALL, Mode::Threads), run(all, None, 0));
    }

    #[test]
    fn the_largest_counts_do_not_overflow() {
        // One thread asked for of a group as large as numbers go: more
        // extra threads than fit the count.
        let Plan::Run(r) = plan([1, 1, 1], Some(0x20), Some([u32::MAX; 3]), NONE, Mode::Rounded) else {
            panic!("rounded mode runs");
        };
        assert_eq!(r.groups, [1, 1, 1]);
        assert_eq!(r.extra, u64::MAX, "more extra threads than fit: saturated");
        // The largest plan with limits: every limit is below `NO_LIMIT`.
        let size = [65_537, 65_537, 65_537];
        let most = [u32::MAX - 1; 3];
        assert_eq!(plan(most, Some(0x20), Some(size), ALL, Mode::Threads), run([MAX_GROUPS; 3], Some(most), 0));
    }

    /// Every thread the host starts for a plan, numbered in the whole
    /// dispatch, that the program lets run.
    fn started(r: &Run, size: [u32; 3]) -> Vec<[u32; 3]> {
        let limits = r.limits.unwrap_or([NO_LIMIT; 3]);
        let mut out = Vec::new();
        let each = |n: [u32; 3]| {
            (0..n[2]).flat_map(move |z| (0..n[1]).flat_map(move |y| (0..n[0]).map(move |x| [x, y, z])))
        };
        for group in each(r.groups) {
            for thread in each(size) {
                if runs(group, thread, size, limits) {
                    out.push(std::array::from_fn(|i| group[i] * size[i] + thread[i]));
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// The point of it all: for every small count and group size, a program
    /// with limits runs exactly the threads asked for, each once, none more
    /// and none fewer; without limits the same holds in the strict mode
    /// wherever a plan is made, and the rounded mode's extra threads are
    /// exactly what it reports.
    #[test]
    fn exactly_the_threads_asked_for_run() {
        let sizes = [[1, 1, 1], [4, 1, 1], [3, 2, 1], [2, 2, 2], [1, 4, 1], [5, 1, 3]];
        for size in sizes {
            for cx in 0..=9 {
                for cy in 0..=5 {
                    for cz in 0..=4 {
                        let counts = [cx, cy, cz];
                        let mut asked: Vec<[u32; 3]> = Vec::new();
                        for z in 0..cz {
                            for y in 0..cy {
                                for x in 0..cx {
                                    asked.push([x, y, z]);
                                }
                            }
                        }
                        asked.sort_unstable();
                        // The dimensions a translated program has limits for.
                        let limited = size.map(|n| n > 1);
                        for mode in [Mode::Threads, Mode::Rounded] {
                            let Plan::Run(r) = plan(counts, Some(0x20), Some(size), limited, mode) else {
                                panic!("{counts:?} of {size:?}: a program with limits is never refused");
                            };
                            assert_eq!(r.extra, 0);
                            assert_eq!(started(&r, size), asked, "{counts:?} of {size:?}");
                        }
                        match plan(counts, Some(0x20), Some(size), NONE, Mode::Threads) {
                            Plan::Run(r) => assert_eq!(started(&r, size), asked, "{counts:?} of {size:?}"),
                            Plan::Refused(_) => assert!((0..3).any(|i| !counts[i].is_multiple_of(size[i]))),
                        }
                        let Plan::Run(r) = plan(counts, Some(0x20), Some(size), NONE, Mode::Rounded) else {
                            panic!("rounded mode runs");
                        };
                        let ran = started(&r, size);
                        assert!(asked.iter().all(|t| ran.binary_search(t).is_ok()), "none is left out");
                        assert_eq!(ran.len() - asked.len(), r.extra as usize, "{counts:?} of {size:?}");
                    }
                }
            }
        }
    }

    fn call<'a>(flags: u64, counts: [u32; 3], program: &'a str, is_constant: &'a dyn Fn(u32) -> bool) -> Call<'a> {
        Call {
            method: Method::DispatchX,
            flags,
            counts,
            size: Some([64, 1, 1]),
            program,
            is_constant,
            constants: &|| vec![1000, 7],
            followed: false,
        }
    }

    #[test]
    fn each_flags_value_is_counted_and_each_program_logged_once() {
        let mut t = Tally::new();
        let thousand = |v: u32| v == 1000;
        let first = t.note(&call(0x20, [1000, 1, 1], "builder stage a", &thousand));
        assert_eq!(first.len(), 1, "{first:?}");
        let text = &first[0].text;
        assert!(!first[0].warn);
        for part in ["flags 0x20", "counts 1000x1x1", "thread group 64x1x1", "64000 threads if the counts are groups", "1000 if they are threads", "[3e8, 7]", "builder stage a", "VICTORY_DISPATCH_THREADS is off"] {
            assert!(text.contains(part), "{part}: {text}");
        }
        // The same program again: only the totals, at 2, 4, 8.
        let again: Vec<usize> = (0..7).map(|_| t.note(&call(0x20, [1000, 1, 1], "builder stage a", &thousand)).len()).collect();
        assert_eq!(again, [1, 0, 1, 0, 0, 0, 1]);
        let totals = t.note(&call(0x20, [128, 1, 1], "builder stage b", &thousand));
        assert_eq!(totals.len(), 1, "another program: its first call, no totals at 9");
        assert!(totals[0].text.contains("builder stage b"));
        assert_eq!(t.calls(Method::DispatchX, 0x20), 9);
        // Another value is another count.
        assert_eq!(t.note(&call(0, [4, 1, 1], "builder stage a", &thousand)).len(), 1);
        assert_eq!(t.calls(Method::DispatchX, 0), 1);
        assert_eq!(t.calls(Method::DispatchMeshX, 0x20), 0);
    }

    /// A program whose first call fills whole groups is shown again at its
    /// first call that does not, once: those are the counts that tell.
    #[test]
    fn the_first_count_that_fills_no_whole_group_is_shown() {
        let mut t = Tally::new();
        let never = |_: u32| false;
        let first = t.note(&call(0x20, [64, 1, 1], "stage", &never));
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(first[0].text.contains("first call of this program: counts 64x1x1"), "{first:?}");
        // Whole groups again, and one thread: nothing but the totals at 2.
        assert_eq!(t.note(&call(0x20, [128, 1, 1], "stage", &never)).len(), 1);
        assert!(t.note(&call(0x20, [1, 1, 1], "stage", &never)).is_empty());
        let mut partial = Vec::new();
        for n in 1000..1040 {
            let lines = t.note(&call(0x20, [n, 1, 1], "stage", &never));
            partial.extend(lines.into_iter().filter(|l| l.text.contains("no multiple of its thread group")));
        }
        assert_eq!(partial.len(), 1, "{partial:?}");
        let text = &partial[0].text;
        for part in ["flags 0x20", "counts 1000x1x1", "64000 threads if the counts are groups", "1000 if they are threads", "stage"] {
            assert!(text.contains(part), "{part}: {text}");
        }
        // Each program and each flags value has its own; a program that
        // begins with such a count has said it in its first line.
        let other = t.note(&call(0x20, [70, 1, 1], "another stage", &never));
        assert_eq!(other.iter().filter(|l| l.text.contains("first call of this program")).count(), 1, "{other:?}");
        assert!(other.iter().all(|l| !l.text.contains("no multiple of its thread group")), "{other:?}");
        let lines = t.note(&call(0x20, [71, 1, 1], "another stage", &never));
        assert!(lines.iter().all(|l| !l.text.contains("first call")), "{lines:?}");
        let lines = t.note(&call(0x60, [64, 1, 1], "stage", &never));
        assert!(lines.iter().any(|l| l.text.contains("first call of this program: counts 64x1x1")), "{lines:?}");
        let lines = t.note(&call(0x60, [65, 1, 1], "stage", &never));
        assert!(lines.iter().any(|l| l.text.contains("no multiple of its thread group: counts 65x1x1")), "{lines:?}");
        // A program without a known group size has no such count.
        let unknown = Call { size: None, ..call(0x20, [1000, 1, 1], "untranslated", &never) };
        assert_eq!(t.note(&unknown).len(), 1);
        assert!(t.note(&unknown).iter().all(|l| !l.text.contains("first call")));
    }

    /// As many programs as a structure builder has stages, and more, each
    /// get their line, the last places of the list included.
    #[test]
    fn every_listed_program_has_its_own_lines() {
        let mut t = Tally::new();
        let never = |_: u32| false;
        let names: Vec<String> = (0..PROGRAMS).map(|n| format!("program {n}")).collect();
        for name in &names {
            let lines = t.note(&call(0x20, [64, 1, 1], name, &never));
            assert_eq!(lines.iter().filter(|l| l.text.ends_with(name.as_str())).count(), 1, "{name}");
        }
        for name in names.iter().rev() {
            let shown = |lines: Vec<Line>| lines.iter().filter(|l| l.text.contains("no multiple of its thread group")).count();
            assert_eq!(shown(t.note(&call(0x20, [65, 1, 1], name, &never))), 1, "{name}");
            assert_eq!(shown(t.note(&call(0x20, [66, 1, 1], name, &never))), 0, "{name}");
        }
    }

    #[test]
    fn the_totals_say_what_the_counts_tell() {
        let mut t = Tally::new();
        let thousand = |v: u32| v == 1000;
        // 1000: wide, partial, a constant. 128: wide. 1: tells nothing.
        for counts in [[1000, 1, 1], [128, 1, 1], [1, 1, 1]] {
            t.note(&call(0x20, counts, "p", &thousand));
        }
        let lines = t.note(&call(0x20, [1000, 1, 1], "p", &thousand));
        assert_eq!(lines.len(), 1);
        let text = &lines[0].text;
        assert!(text.contains("4 calls so far; 3 with a count above 1"), "{text}");
        assert!(text.contains("of which 2 with a count that is no multiple of the group and 2 with such a count among"), "{text}");
    }

    #[test]
    fn unknown_bits_are_warned_about_once_per_value() {
        let mut t = Tally::new();
        let never = |_: u32| false;
        let lines = t.note(&call(0x24, [8, 1, 1], "p", &never));
        assert_eq!(lines.iter().filter(|l| l.warn).count(), 1);
        assert!(lines[0].text.contains("bits 0x4 are not known"), "{:?}", lines[0]);
        assert!(t.note(&call(0x24, [8, 1, 1], "p", &never)).iter().all(|l| !l.warn));
        // 0x20 alone is known; for the mesh method no bit is.
        assert!(t.note(&call(0x20, [8, 1, 1], "p", &never)).iter().all(|l| !l.warn));
        let mesh = Call { method: Method::DispatchMeshX, size: None, ..call(0x20, [8, 1, 1], "p", &never) };
        let lines = t.note(&mesh);
        assert!(lines[0].warn && lines[0].text.contains("bits 0x20"), "{lines:?}");
        assert!(lines[1].text.contains("no translated compute program"), "{lines:?}");
    }

    #[test]
    fn the_tally_is_bounded() {
        let mut t = Tally::new();
        let never = |_: u32| false;
        // More programs than are listed: each of the first ones once, then
        // one line saying so, then nothing but totals.
        let mut listed = 0;
        for n in 0..PROGRAMS + 10 {
            let name = format!("program {n}");
            let lines = t.note(&call(0x20, [8, 1, 1], &name, &never));
            listed += lines.iter().filter(|l| l.text.contains("first call of this program")).count();
        }
        assert_eq!(listed, PROGRAMS);
        assert_eq!(t.calls(Method::DispatchX, 0x20), (PROGRAMS + 10) as u64);
        // More values than are kept apart: counted together, said at 1, 2, 4.
        let mut t = Tally::new();
        for flags in 0..VALUES as u64 {
            t.note(&call(flags << 8, [8, 1, 1], "p", &never));
        }
        let over: Vec<usize> = (0..4).map(|n| t.note(&call(0xff00 + n, [8, 1, 1], "p", &never)).len()).collect();
        assert_eq!(over, [1, 1, 0, 1]);
        assert_eq!(t.others, 4);
    }

    fn count(lines: &[Line], part: &str) -> usize {
        lines.iter().filter(|l| l.text.contains(part)).count()
    }

    #[test]
    fn each_indirect_signature_value_is_logged_once() {
        let t = Indirect::new();
        let first = t.note(105, [0, 0, 0]);
        assert_eq!(first.len(), 1);
        assert!(!first[0].warn && first[0].text.contains("argument type 105"), "{first:?}");
        assert!(t.note(105, [0, 0, 0]).is_empty());
        // The type is part of the value, and so is each field.
        assert_eq!(t.note(2, [0, 0, 0]).len(), 1);
        assert_eq!(t.note(105, [0, 1, 0]).len(), 1);
        let flagged = t.note(105, [0x20, 0, 0]);
        assert!(flagged.len() == 1 && flagged[0].warn && flagged[0].text.contains("bit 0x20"), "{flagged:?}");
        assert!(t.note(105, [0x20, 0, 0]).is_empty());
        assert!(t.note(2, [0, 0, 0]).is_empty());
    }

    /// Past the values kept apart nothing is passed over quietly: the
    /// executions are counted and said at 1, 2, 4..., and the first value
    /// with bit 0x20 among them is warned of whatever came before it.
    #[test]
    fn indirect_values_past_the_list_are_counted_and_the_flag_still_warned_of() {
        let t = Indirect::new();
        // Values without the bit take every place (fields of a plain
        // dispatch's argument mean nothing on the host).
        for n in 0..INDIRECT_VALUES as u32 {
            assert_eq!(t.note(2, [n << 8, 0, 0]).len(), 1, "{n}");
        }
        let over = t.note(2, [0xff00, 0, 0]);
        assert_eq!((over.len(), count(&over, "1 executions with further ones")), (1, 1), "{over:?}");
        assert!(over[0].warn);
        // The single-pass dispatch with the bit, seen only now.
        let flagged = t.note(105, [0x20, 0, 0]);
        assert_eq!(count(&flagged, "argument type 105) whose signature fields [20, 0, 0] have bit 0x20"), 1, "{flagged:?}");
        assert_eq!(count(&flagged, "2 executions with further ones"), 1, "{flagged:?}");
        assert!(flagged.iter().all(|l| l.warn));
        // Again, and another with the bit: counted; the warning was given.
        let again = t.note(105, [0x20, 0, 0]);
        assert!(again.is_empty(), "the third: {again:?}");
        let fourth = t.note(105, [0x21, 0, 0]);
        assert_eq!((fourth.len(), count(&fourth, "4 executions with further ones")), (1, 1), "{fourth:?}");
        assert_eq!(t.others.load(Relaxed), 4);
        // The values kept apart are still known.
        assert!(t.note(2, [0, 0, 0]).is_empty());
        assert_eq!(t.others.load(Relaxed), 4);
    }

    /// Each way a dispatch counted in threads can go is counted, and the
    /// extra threads of the rounded mode summed.
    #[test]
    fn outcomes_are_counted_by_kind() {
        let load = || OUTCOMES.each_ref().map(|n| n.load(Relaxed));
        let (before, extra) = (load(), EXTRA_THREADS.load(Relaxed));
        let size = Some([64, 1, 1]);
        for (limited, mode) in [(ALL, Mode::Threads), (NONE, Mode::Threads), (NONE, Mode::Rounded)] {
            for counts in [[128, 1, 1], [65, 1, 1]] {
                outcome(counts, size, &plan(counts, Some(0x20), size, limited, mode), "p");
            }
        }
        // Whole groups three times; cut by a limit, refused and rounded once
        // each (at least: a test with a host device may dispatch meanwhile).
        let after = load();
        for (kind, more) in [3, 1, 1, 1].into_iter().enumerate() {
            assert!(after[kind] >= before[kind] + more, "{kind}: {before:?} {after:?}");
        }
        assert!(EXTRA_THREADS.load(Relaxed) >= extra + 63);
    }

    /// What a dispatch's counts tell: only a count above 1 along a dimension
    /// the group is wider than a thread says anything.
    #[test]
    fn only_counts_along_wide_dimensions_tell() {
        let any = |_: u32| true;
        assert_eq!(telling([1, 1, 1], [64, 1, 1], &any), (false, false, false));
        assert_eq!(telling([30, 1, 1], [1, 1, 1], &any), (false, false, false));
        assert_eq!(telling([30, 1, 1], [64, 1, 1], &|v| v == 30), (true, true, true));
        assert_eq!(telling([128, 1, 1], [64, 1, 1], &|v| v == 1), (true, false, false));
        assert_eq!(telling([16, 20, 1], [8, 8, 1], &|v| v == 16), (true, true, true));
        // The plain method's counts are kept for comparison (other tests of
        // this binary may dispatch meanwhile: at least these two).
        let before = plain();
        note_plain([1, 1, 1], [64, 1, 1], &any);
        note_plain([30, 1, 1], [64, 1, 1], &|v| v == 30);
        note_plain([30, 20, 1], [8, 8, 1], &|_| false);
        let after = plain();
        assert!(after.0 >= before.0 + 2 && after.1 > before.1, "{before:?} {after:?}");
    }
}
