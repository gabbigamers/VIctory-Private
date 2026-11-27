//! The workload description: what one recognised traversal region means,
//! naming no path that could run it, and the reasons a loop is given none.
//!
//! A description is made of values of the program's SSA form (the ray's
//! components, what a miss leaves in each kept slot), which belong to one
//! program, and of forms (which compares, which order, what is kept), which
//! do not. The **class** is a hash of the forms alone: a synthetic program
//! and a title loop with the same forms are one class.

use std::cmp::Ordering;

use crate::ir::{Region, Value};
use crate::resource::Site;

/// A value or a constant: an operand as the loop reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arg {
    Value(Value),
    Const(u32),
}

/// How the candidate's distance `t` is compared with a bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Compare {
    /// `<` or `>`; else `<=` or `>=`.
    pub strict: bool,
    /// Written as the negation of the opposite compare (`!(t >= e)` for
    /// `t < e`), which a NaN passes; else an ordered compare, which a NaN
    /// fails.
    pub unordered: bool,
}

impl Compare {
    /// `<`, which a NaN fails.
    pub const LESS: Compare = Compare {
        strict: true,
        unordered: false,
    };

    /// The form of `v_cmp_<op>` (or `v_cmpx_<op>`) read as "the lesser side
    /// is less than the greater": whether the lesser side is the second
    /// operand, and the form. `None` for any other compare.
    pub(crate) fn of(op: &str) -> Option<(bool, Compare)> {
        let form = |strict, unordered| Compare { strict, unordered };
        Some(match op {
            "lt_f32" => (false, form(true, false)),
            "gt_f32" => (true, form(true, false)),
            "le_f32" => (false, form(false, false)),
            "ge_f32" => (true, form(false, false)),
            // "Not greater or equal" is "less, or one is a NaN".
            "nge_f32" => (false, form(true, true)),
            "nle_f32" => (true, form(true, true)),
            "ngt_f32" => (false, form(false, true)),
            "nlt_f32" => (true, form(false, true)),
            _ => return None,
        })
    }

    /// Whether `low` is below `high` in this form.
    pub fn holds(self, low: f32, high: f32) -> bool {
        match (self.strict, self.unordered) {
            (true, false) => low < high,
            (false, false) => low <= high,
            // Negations: a NaN on either side passes.
            (true, true) => low.partial_cmp(&high) != Some(Ordering::Greater) && low != high,
            (false, true) => low.partial_cmp(&high) != Some(Ordering::Greater),
        }
    }

    fn text(self) -> &'static str {
        match (self.strict, self.unordered) {
            (true, false) => "strict",
            (false, false) => "inclusive",
            (true, true) => "strict-unordered",
            (false, true) => "inclusive-unordered",
        }
    }
}

/// How the inverse direction the node test is given is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Inverse {
    /// Each component is `v_rcp_f32` of the direction's, which the
    /// translation computes as an exact division.
    Reciprocal,
}

/// When the accept passes a candidate that is no hit. The node test
/// answers a triangle the ray misses with the miss word (+inf, 1, 0, 0),
/// so the loop's candidate is then `t` = +inf with weights of 0 and a
/// positive denominator, and an accept that passes it replaces what the
/// loop keeps as for a hit: a distance of +inf, weights of 0, the front
/// face's constant, the flag, the missed triangle's pointer; a first-hit
/// walk ends there. A ray that hits nothing then leaves that, or what
/// [`Kept::miss`] says, by whether it came to test a triangle at all:
/// which is the guest's box culling, and nothing a host structure
/// reproduces. A minimum of +inf or a NaN can keep the miss word out;
/// that is data and not counted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MissWord {
    /// While the extent is +inf: an inclusive compare (`+inf <= +inf`), or
    /// a tie by key the kept pointer's first value does not rule out
    /// (`+inf == +inf`).
    pub at_infinite: bool,
    /// While the extent is a NaN: a compare written as a negation.
    pub at_nan: bool,
}

impl MissWord {
    /// The accept passes a hit only.
    pub const NEVER: MissWord = MissWord {
        at_infinite: false,
        at_nan: false,
    };

    fn text(self) -> &'static str {
        match (self.at_infinite, self.at_nan) {
            (false, false) => "never",
            (true, false) => "at-infinite-extent",
            (false, true) => "at-nan-extent",
            (true, true) => "at-infinite-or-nan-extent",
        }
    }
}

/// The extent: what `t` must be below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Extent {
    pub compare: Compare,
    /// The extent before any hit.
    pub start: Arg,
    /// It becomes the distance of each accepted candidate (the running
    /// minimum), and the node test is given it as it stands; else it is
    /// `start` throughout.
    pub shrinks: bool,
    /// Whether, and at which extents, the accept passes the node test's
    /// answer for a triangle the ray misses.
    pub miss_word: MissWord,
}

/// Where the ray comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RaySource {
    /// The same values on every round of the loop.
    pub origin: [Value; 3],
    pub direction: [Value; 3],
    pub inverse: Inverse,
    /// `minimum < t` in the given form. (A loop that tests none is not
    /// recognised yet: [`Refusal::AcceptForm`].)
    pub minimum: (Arg, Compare),
    pub extent: Extent,
}

/// The structure the traversal starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StructureSource {
    /// The node test's descriptor is built around the address a root
    /// parameter holds: an index into the program's layout.
    RootParameter(usize),
}

/// How the traversal's first node pointer is found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Root {
    /// A constant the program pushes before the loop.
    Constant(u32),
}

/// The ray flags the loop's own code amounts to. All of them are constants
/// here by construction: a loop that reads a flag from data has a compare
/// the recogniser does not know and is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags {
    /// The walk ends at the first candidate accepted.
    pub accept_first_hit: bool,
    pub cull_front: bool,
    pub cull_back: bool,
}

/// The instance mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mask {
    /// No mask is tested: the walk is of one tree of box and triangle
    /// nodes, with no instance.
    Untested,
}

/// Code the loop runs on entering an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceCode {
    /// None: the loop enters no instance. The node test answers an
    /// instance node with four invalid words and the loop pushes nothing
    /// for it, so the walk is of one tree of box and triangle nodes.
    None,
}

/// What runs on a candidate before it is accepted, beyond the compares of
/// its distance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Filter {
    None,
    /// The blocks of the filter. No shape with one is recognised yet: the
    /// field is the design's, and a loop with code of this kind is refused.
    Region(Vec<usize>),
}

/// One thing the code after the loop is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The distance, as the numerator times `v_rcp_f32` of the
    /// denominator (a division as translated, rounded once more by the
    /// product).
    Distance,
    /// Barycentric numerator 0 or 1 of the node test (its third and fourth
    /// result word), times the same reciprocal.
    Barycentric(u8),
    /// One constant for a positive denominator (a front face), another
    /// for the rest.
    Facing { front: u32, back: u32 },
    /// A constant: that there was a hit.
    Flag(u32),
    /// The node pointer the candidate was tested with. Stored or passed
    /// on only: a pointer that is loaded through is a refusal.
    NodePointer,
    // No kind for the triangle-id bits or for instance data: the node test
    // as translated returns neither, and a loop that loads them itself is
    // refused ([`Refusal::Loads`], [`Refusal::NodeLoad`]).
}

/// A value that leaves the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Kept {
    /// The value the code after the loop reads.
    pub value: Value,
    pub kind: Kind,
    /// What it is when no candidate was accepted. That is every ray that
    /// hits nothing only where [`Extent::miss_word`] is
    /// [`MissWord::NEVER`]: otherwise a missed triangle can be accepted.
    pub miss: Arg,
}

/// How the result depends on the order nodes are visited in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Order {
    /// Not at all: yes or no.
    None,
    /// One compare of `t` against the running minimum, everything kept
    /// replaced with it: order matters only among exactly equal `t`.
    TieByVisit,
    /// Equal `t` is settled by an unsigned compare of the candidate's
    /// node pointer with the kept one.
    TieByKey,
}

/// The per-thread stack the walk keeps its node pointers on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stack {
    /// The thread's part of group-shared memory, in bytes: thread `n` of
    /// the group has the bytes from `n * stride`.
    pub stride: u32,
    /// The program does not test its depth: a tree that needs more than
    /// `stride / 4` entries runs into the next thread's part.
    pub checked: bool,
}

/// One recognised traversal region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    /// The loop. Not part of the class.
    pub region: Region,
    /// The node instruction.
    pub node: Site,
    pub ray: RaySource,
    pub structure: StructureSource,
    pub root: Root,
    pub flags: Flags,
    pub mask: Mask,
    pub instance: InstanceCode,
    pub filter: Filter,
    /// What the code after the loop reads, by value.
    pub kept: Vec<Kept>,
    pub order: Order,
    /// A cap on iterations, or a stack that drops what it cannot hold, is
    /// a refusal, so there is neither.
    pub stack: Stack,
    pub class: Class,
}

/// A workload class: the hash of a description's forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Class(pub u64);

impl std::fmt::Display for Class {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl Description {
    /// The forms the class is the hash of, as text: every field that
    /// decides which path may run the workload, and no value, block,
    /// register, constant of the program or name.
    pub fn forms(&self) -> String {
        let mut kept: Vec<&'static str> = self
            .kept
            .iter()
            .map(|k| match k.kind {
                Kind::Distance => "distance-by-reciprocal",
                Kind::Barycentric(0) => "barycentric-0",
                Kind::Barycentric(_) => "barycentric-1",
                Kind::Facing { .. } => "facing-by-sign",
                Kind::Flag(_) => "flag",
                Kind::NodePointer => "node-pointer",
            })
            .collect();
        kept.sort_unstable();
        kept.dedup();
        let minimum = self.ray.minimum.1.text();
        let Inverse::Reciprocal = self.ray.inverse;
        let InstanceCode::None = self.instance;
        let StructureSource::RootParameter(_) = self.structure;
        let Root::Constant(_) = self.root;
        let Mask::Untested = self.mask;
        format!(
            "lanes=per-lane; inverse=reciprocal; minimum={minimum}; extent={}{}; miss-word={}; \
             structure=root-parameter; root=constant; first-hit={}; cull={}{}; mask=untested; \
             instance-code=none; filter={}; kept={}; order={}; iteration-cap=none; stack={}",
            self.ray.extent.compare.text(),
            if self.ray.extent.shrinks {
                ",shrinks"
            } else {
                ",fixed"
            },
            self.ray.extent.miss_word.text(),
            u8::from(self.flags.accept_first_hit),
            u8::from(self.flags.cull_front),
            u8::from(self.flags.cull_back),
            match self.filter {
                Filter::None => "none",
                Filter::Region(_) => "region",
            },
            if kept.is_empty() {
                "nothing".to_owned()
            } else {
                kept.join("+")
            },
            match self.order {
                Order::None => "none",
                Order::TieByVisit => "tie-by-visit",
                Order::TieByKey => "tie-by-key",
            },
            if self.stack.checked {
                "checked"
            } else {
                "unchecked"
            },
        )
    }

    /// The class of these forms: FNV-1a, 64 bits, of [`Description::forms`].
    pub fn class_of(forms: &str) -> Class {
        let mut h = 0xCBF2_9CE4_8422_2325u64;
        for b in forms.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3);
        }
        Class(h)
    }
}

/// Why a loop is given no description. Each is a reason to leave the loop
/// to the software path, never a quiet default; the recogniser gives the
/// first one it meets, in the order of its checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refusal {
    NoNodeInstruction,
    NoPerLaneForm,
    WholeWave,
    SeveralNodeTests,
    NestedLoop,
    NodeForm,
    StructureSource,
    RayChanges,
    InverseNotProven,
    StackShape,
    Stores,
    Loads,
    IterationCap,
    ExitForm,
    TooManyCompares,
    CompareForm,
    AcceptForm,
    ExtentCompare,
    VisitOrder,
    NodeLoad,
    LiveOut,
}

impl Refusal {
    pub const ALL: [Refusal; 21] = [
        Refusal::NoNodeInstruction,
        Refusal::NoPerLaneForm,
        Refusal::WholeWave,
        Refusal::SeveralNodeTests,
        Refusal::NestedLoop,
        Refusal::NodeForm,
        Refusal::StructureSource,
        Refusal::RayChanges,
        Refusal::InverseNotProven,
        Refusal::StackShape,
        Refusal::Stores,
        Refusal::Loads,
        Refusal::IterationCap,
        Refusal::ExitForm,
        Refusal::TooManyCompares,
        Refusal::CompareForm,
        Refusal::AcceptForm,
        Refusal::ExtentCompare,
        Refusal::VisitOrder,
        Refusal::NodeLoad,
        Refusal::LiveOut,
    ];

    /// The reason, as the census prints it.
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::NoNodeInstruction => "no node instruction in the loop",
            Refusal::NoPerLaneForm => {
                "the program has no per-lane translation (its structure, resources, I/O or lowering is refused)"
            }
            Refusal::WholeWave => "a whole-wave walk: the node pointer is one for the wave",
            Refusal::SeveralNodeTests => "more than one node instruction in the loop",
            Refusal::NestedLoop => "a loop inside the loop",
            Refusal::NodeForm => "a node instruction with 64-bit pointers or 16-bit directions",
            Refusal::StructureSource => "the structure's base is not a root parameter's address",
            Refusal::RayChanges => {
                "the ray's origin or direction changes inside the loop, or its extent other than to an accepted distance"
            }
            Refusal::InverseNotProven => "the inverse direction is not proven to be 1 / direction",
            Refusal::StackShape => {
                "not the known walk: a per-thread stack popped once a round, every valid child of a box node pushed"
            }
            Refusal::Stores => "a store inside the loop other than a push",
            Refusal::Loads => "a load inside the loop other than the node test and the pop",
            Refusal::IterationCap => "a cap on iterations",
            Refusal::ExitForm => "the loop does not end in one place, on an empty stack alone",
            Refusal::TooManyCompares => {
                "more compares decide what the loop keeps, pushes and ends on than are enumerated (14)"
            }
            Refusal::CompareForm => "a compare of an unrecognised form decides a candidate",
            Refusal::AcceptForm => {
                "the compares that accept a candidate are joined in an unrecognised way"
            }
            Refusal::ExtentCompare => {
                "candidates are accepted with no compare of t against the extent the node test got"
            }
            Refusal::VisitOrder => "the result depends on the order of the visits",
            Refusal::NodeLoad => "a hit's node pointer is kept and loaded through",
            Refusal::LiveOut => "a value the loop carries or hands on has no recipe",
        }
    }
}
