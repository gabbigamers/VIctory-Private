//! One host ray-tracing structure's life, as a state machine with no
//! device in it: what may be done with the structure now, and from which
//! submission on its memory may go. `super::cache` keeps one of these for
//! each structure, and `super::device` asks it before every D3D12 call.
//!
//! Time is counted in submissions. Each list handed to the queue gets the
//! next value of the queue's fence, and that value is complete when the
//! list has run. A list being recorded has no value yet: it is "open".
//!
//! The ordering this machine keeps (the contract in full is in
//! docs/dxr-host-backend.md):
//!
//! * a build is recorded into an open list, and a barrier after it; only
//!   then may anything later in that list, or in a later submission, read
//!   the structure;
//! * a structure whose source changed, or that was built over something
//!   that changed, is stale and is not read until it is built again;
//! * every reader holds the structure from when it is recorded (a pin)
//!   until its submission is complete; memory goes only when no pin is
//!   left and the latest submission that may read it is complete.

/// Where a structure is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Known, with nothing built.
    Empty,
    /// A build is recorded and not known to have run.
    Building,
    /// Built, and the build has run.
    Ready,
    /// What it holds is no longer what its source holds. It may still be
    /// read by submissions recorded before that; not by new ones.
    Stale,
    /// Given up. Its memory goes once nothing can read it.
    Retired,
}

/// Why a structure may not be built into or read now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Never built.
    Empty,
    /// Its source changed and it has not been built again.
    Stale,
    /// Built, but no barrier follows the build yet.
    NoBarrier,
    /// Given up.
    Retired,
    /// No such structure (a handle that outlived it).
    Unknown,
    /// A top level over a bottom level that cannot be read now.
    Bottom,
    /// Its build has not run yet, and the request needs what the build
    /// reports (a compacting copy needs the size).
    NotRun,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refusal::Empty => "the structure was never built",
            Refusal::Stale => "the structure's source changed since it was built",
            Refusal::NoBarrier => "no barrier follows the structure's build",
            Refusal::Retired => "the structure was given up",
            Refusal::Unknown => "no such structure",
            Refusal::Bottom => "a bottom level under the top level cannot be read",
            Refusal::NotRun => "the structure's build has not run yet",
        })
    }
}

/// The latest build's place in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// No build is outstanding.
    None,
    /// In a list still being recorded.
    Open,
    /// In the submission with this fence value, not known complete.
    Submitted(u64),
}

/// A structure's state, the readers holding it, and the latest submission
/// that may read it.
#[derive(Clone, Debug)]
pub struct Life {
    state: State,
    /// The content generation it holds, or is being built to hold.
    generation: u64,
    pending: Pending,
    /// A barrier was recorded after the latest build.
    barrier: bool,
    /// Whether a build of it has ever been submitted.
    built: bool,
    /// It could be read when it was given up: the state is then "retired"
    /// and no longer says so.
    read_when_retired: bool,
    /// Reads of it recorded in a list not yet submitted.
    pins: u32,
    /// The highest fence value of a submission that reads or writes it.
    last_use: u64,
}

impl Default for Life {
    fn default() -> Self {
        Self::new()
    }
}

impl Life {
    pub const fn new() -> Self {
        Life {
            state: State::Empty,
            generation: 0,
            pending: Pending::None,
            barrier: false,
            built: false,
            read_when_retired: false,
            pins: 0,
            last_use: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn pins(&self) -> u32 {
        self.pins
    }

    /// Whether a build of it is recorded in a list not yet submitted.
    pub fn build_is_open(&self) -> bool {
        self.pending == Pending::Open
    }

    /// A build (or refit, or compacting copy) into the structure is being
    /// recorded into the open list, to hold `generation`. Allowed in every
    /// state but retired; after an earlier build in the same open list only
    /// once that one's barrier is recorded, since the second reads or
    /// overwrites what the first writes.
    pub fn build(&mut self, generation: u64) -> Result<(), Refusal> {
        self.can_build()?;
        self.state = State::Building;
        self.generation = generation;
        self.pending = Pending::Open;
        self.barrier = false;
        Ok(())
    }

    /// Whether [`Life::build`] would be allowed now.
    pub fn can_build(&self) -> Result<(), Refusal> {
        match self.state {
            State::Retired => Err(Refusal::Retired),
            _ if self.pending == Pending::Open && !self.barrier => Err(Refusal::NoBarrier),
            _ => Ok(()),
        }
    }

    /// The barrier after the latest build is recorded.
    pub fn barrier(&mut self) {
        if matches!(self.state, State::Building | State::Stale) && self.pending != Pending::None {
            self.barrier = true;
        }
    }

    /// Whether something recorded now may read the structure.
    pub fn readable(&self) -> Result<(), Refusal> {
        match self.state {
            State::Empty => Err(Refusal::Empty),
            State::Stale => Err(Refusal::Stale),
            State::Retired => Err(Refusal::Retired),
            State::Building | State::Ready if !self.barrier => Err(Refusal::NoBarrier),
            State::Building | State::Ready => Ok(()),
        }
    }

    /// Whether a top level recorded now may be read through to this bottom
    /// level: as [`Life::readable`], and also once it is given up if it
    /// could be read then, since a top level keeps what it was built from.
    /// One that was stale when it was given up stays what it was: giving a
    /// structure up does not make its contents current.
    pub fn readable_under_a_top(&self) -> Result<(), Refusal> {
        match self.state {
            State::Retired if self.read_when_retired && self.barrier && self.built => Ok(()),
            _ => self.readable(),
        }
    }

    /// An open list reads the structure: it is held until that list is
    /// submitted and complete, or abandoned. The caller has asked
    /// [`Life::readable`], and knows the read is in the list any open build
    /// of the structure is in (this machine cannot tell lists apart:
    /// `super::cache` has one).
    pub fn pin(&mut self) {
        self.pins += 1;
    }

    /// A list that pinned the structure was submitted with fence value
    /// `fence`.
    pub fn sealed(&mut self, fence: u64) {
        self.pins = self.pins.saturating_sub(1);
        self.last_use = self.last_use.max(fence);
    }

    /// A list that pinned the structure was dropped without being
    /// submitted.
    pub fn unpinned(&mut self) {
        self.pins = self.pins.saturating_sub(1);
    }

    /// The open list holding the structure's build was submitted with fence
    /// value `fence`.
    pub fn build_submitted(&mut self, fence: u64) {
        if self.pending == Pending::Open {
            self.pending = Pending::Submitted(fence);
            self.built = true;
            self.last_use = self.last_use.max(fence);
        }
    }

    /// The open list holding the structure's build was dropped without
    /// being submitted: the build never runs. What the structure holds is
    /// then not what was recorded for it, and it is stale (or empty, never
    /// having been built) until built again, whole.
    pub fn build_abandoned(&mut self) {
        if self.pending == Pending::Open {
            self.pending = Pending::None;
            self.barrier = false;
            if self.state != State::Retired {
                self.state = if self.built { State::Stale } else { State::Empty };
            }
        }
    }

    /// Every submission up to fence value `completed` has run.
    pub fn completed(&mut self, completed: u64) {
        if let Pending::Submitted(fence) = self.pending
            && fence <= completed
        {
            self.pending = Pending::None;
            if self.state == State::Building {
                self.state = State::Ready;
            }
        }
    }

    /// The structure's source, or something it was built over, changed.
    /// Whether that made it stale (it was built or building).
    pub fn invalidate(&mut self) -> bool {
        let was = matches!(self.state, State::Building | State::Ready);
        if was {
            self.state = State::Stale;
        }
        was
    }

    /// The structure is given up. Nothing new reads it, but a top level
    /// built over it may still read through to it if it could be read now
    /// ([`Life::readable_under_a_top`]); see [`Life::releasable`] for its
    /// memory.
    pub fn retire(&mut self) {
        if self.state != State::Retired {
            self.read_when_retired = self.readable().is_ok();
            self.state = State::Retired;
        }
    }

    /// The fence value after which memory the structure used so far may go:
    /// the latest submission that may read or write it. `None` while an
    /// open list still does, since that list's value is not known yet.
    pub fn releasable(&self) -> Option<u64> {
        match self.pending {
            _ if self.pins > 0 => None,
            Pending::Open => None,
            Pending::Submitted(fence) => Some(self.last_use.max(fence)),
            Pending::None => Some(self.last_use),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built by submission 1, which has run.
    fn ready() -> Life {
        let mut life = Life::new();
        life.build(1).unwrap();
        life.barrier();
        life.build_submitted(1);
        life.completed(1);
        life
    }

    #[test]
    fn a_structure_goes_from_empty_through_building_to_ready() {
        let mut life = Life::new();
        assert_eq!((life.state(), life.readable()), (State::Empty, Err(Refusal::Empty)));
        life.build(7).unwrap();
        assert_eq!((life.state(), life.generation()), (State::Building, 7));
        assert!(life.build_is_open());
        life.barrier();
        life.build_submitted(4);
        assert_eq!(life.state(), State::Building, "submitted is not run");
        life.completed(3);
        assert_eq!(life.state(), State::Building, "an earlier submission says nothing of ours");
        life.completed(4);
        assert_eq!((life.state(), life.readable()), (State::Ready, Ok(())));
    }

    /// The barrier is what orders a read after the build inside one list:
    /// without it nothing reads, in the list or after it.
    #[test]
    fn nothing_reads_a_structure_before_the_barrier_after_its_build() {
        let mut life = Life::new();
        life.build(1).unwrap();
        assert_eq!(life.readable(), Err(Refusal::NoBarrier));
        life.barrier();
        assert_eq!(life.readable(), Ok(()), "later in the same list");
        let mut unbarred = Life::new();
        unbarred.build(1).unwrap();
        unbarred.build_submitted(1);
        unbarred.completed(1);
        assert_eq!(unbarred.state(), State::Ready);
        assert_eq!(unbarred.readable(), Err(Refusal::NoBarrier), "a build that ran is still not ordered before a read");
    }

    #[test]
    fn a_second_build_in_one_list_waits_for_the_firsts_barrier() {
        let mut life = Life::new();
        life.build(1).unwrap();
        assert_eq!(life.build(2), Err(Refusal::NoBarrier));
        life.barrier();
        assert_eq!(life.build(2), Ok(()));
        assert_eq!((life.generation(), life.readable()), (2, Err(Refusal::NoBarrier)), "the new build has none yet");
        // A build in a later list, with the first still in flight: the
        // queue runs them in order.
        life.barrier();
        life.build_submitted(1);
        assert_eq!(life.build(3), Ok(()));
    }

    #[test]
    fn a_stale_structure_is_not_read_until_it_is_built_again() {
        let mut life = ready();
        assert!(life.invalidate());
        assert_eq!((life.state(), life.readable()), (State::Stale, Err(Refusal::Stale)));
        assert!(!life.invalidate(), "already stale");
        life.build(2).unwrap();
        life.barrier();
        assert_eq!(life.readable(), Ok(()));
        let mut empty = Life::new();
        assert!(!empty.invalidate());
        assert_eq!(empty.state(), State::Empty, "nothing built is nothing stale");
    }

    /// A source that changes while its build is in flight: the build still
    /// runs, and its result is stale when it has.
    #[test]
    fn a_structure_made_stale_while_building_stays_stale_when_the_build_has_run() {
        let mut life = Life::new();
        life.build(1).unwrap();
        life.barrier();
        life.build_submitted(5);
        assert!(life.invalidate());
        assert_eq!(life.releasable(), Some(5), "the build in flight still writes it");
        life.completed(5);
        assert_eq!(life.state(), State::Stale);
    }

    #[test]
    fn memory_goes_only_after_the_latest_submission_that_may_use_it() {
        let mut life = ready();
        assert_eq!(life.releasable(), Some(1));
        life.pin();
        life.pin();
        assert_eq!(life.releasable(), None, "open lists have no fence value yet");
        life.sealed(9);
        assert_eq!(life.releasable(), None, "one list is still open");
        life.sealed(4);
        assert_eq!(life.releasable(), Some(9), "the later of the two, whichever was sealed last");
        life.retire();
        assert_eq!((life.state(), life.releasable()), (State::Retired, Some(9)));
        assert_eq!(life.readable(), Err(Refusal::Retired));
    }

    #[test]
    fn a_dropped_list_lets_go_without_a_use() {
        let mut life = ready();
        life.pin();
        life.unpinned();
        assert_eq!((life.pins(), life.releasable()), (0, Some(1)));
        life.unpinned();
        assert_eq!(life.pins(), 0, "never below zero");
    }

    #[test]
    fn an_open_build_holds_the_memory_until_it_is_submitted() {
        let mut life = ready();
        life.build(2).unwrap();
        assert_eq!(life.releasable(), None);
        life.barrier();
        life.build_submitted(6);
        assert_eq!(life.releasable(), Some(6));
    }

    #[test]
    fn an_abandoned_build_leaves_nothing_readable() {
        let mut first = Life::new();
        first.build(1).unwrap();
        first.barrier();
        first.build_abandoned();
        assert_eq!((first.state(), first.readable()), (State::Empty, Err(Refusal::Empty)));
        assert_eq!(first.releasable(), Some(0));

        let mut refit = ready();
        refit.build(2).unwrap();
        refit.barrier();
        refit.build_abandoned();
        assert_eq!((refit.state(), refit.readable()), (State::Stale, Err(Refusal::Stale)));
        // A build that was submitted is not abandoned by a later call.
        let mut sent = ready();
        sent.build(2).unwrap();
        sent.barrier();
        sent.build_submitted(3);
        sent.build_abandoned();
        assert_eq!(sent.state(), State::Building);
    }

    #[test]
    fn a_retired_structure_takes_no_build_and_keeps_serving_a_top_level() {
        let mut life = ready();
        life.retire();
        assert_eq!(life.build(2), Err(Refusal::Retired));
        assert_eq!(life.readable(), Err(Refusal::Retired));
        assert_eq!(life.readable_under_a_top(), Ok(()), "a top level keeps what it was built from");
        let mut never = Life::new();
        never.retire();
        assert_eq!(never.readable_under_a_top(), Err(Refusal::Retired));
        let mut stale = ready();
        stale.invalidate();
        assert_eq!(stale.readable_under_a_top(), Err(Refusal::Stale));
    }

    /// Giving a structure up says nothing new of its contents: one that was
    /// stale then (its source changed, or its last build never ran) is not
    /// read through a top level afterwards either.
    #[test]
    fn a_structure_given_up_while_stale_is_not_read_through_a_top_level() {
        let mut stale = ready();
        stale.invalidate();
        stale.retire();
        assert_eq!(stale.readable_under_a_top(), Err(Refusal::Retired));
        stale.retire();
        assert_eq!(stale.readable_under_a_top(), Err(Refusal::Retired), "given up again changes nothing");
        // Made stale while its build was in flight, given up, and then the
        // build runs.
        let mut building = Life::new();
        building.build(1).unwrap();
        building.barrier();
        building.build_submitted(3);
        building.invalidate();
        building.retire();
        building.completed(3);
        assert_eq!(building.readable_under_a_top(), Err(Refusal::Retired));
        // A refit recorded and dropped leaves it stale.
        let mut dropped = ready();
        dropped.build(2).unwrap();
        dropped.barrier();
        dropped.build_abandoned();
        dropped.retire();
        assert_eq!(dropped.readable_under_a_top(), Err(Refusal::Retired));
        // Given up twice while it could be read: still read through.
        let mut read = ready();
        read.retire();
        read.retire();
        assert_eq!(read.readable_under_a_top(), Ok(()));
        // Given up before the barrier after its build.
        let mut unbarred = Life::new();
        unbarred.build(1).unwrap();
        unbarred.build_submitted(1);
        unbarred.retire();
        unbarred.barrier();
        unbarred.completed(1);
        assert_eq!(unbarred.readable_under_a_top(), Err(Refusal::Retired));
    }

    /// Any order of events: a read is allowed only of a structure that is
    /// built, barred and not stale or retired, and the fence value given
    /// for its memory is never below a submission that used it.
    #[test]
    fn no_sequence_of_events_reads_too_early_or_frees_too_early() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let (mut through, mut refused_given_up) = (0u32, 0u32);
        for _ in 0..2_000 {
            let mut life = Life::new();
            let (mut fence, mut completed, mut open_pins, mut highest_use) = (0u64, 0u64, 0u32, 0u64);
            // The latest build: recorded in the open list, or submitted with
            // this fence value and not known complete; and whether a barrier
            // follows it.
            let (mut build_open, mut build_fence, mut barred) = (false, None::<u64>, false);
            // Whether it could be read when it was given up, once it is.
            let mut read_when_given_up = None::<bool>;
            for _ in 0..40 {
                match next(9) {
                    0 => {
                        if life.build(next(5)).is_ok() {
                            (build_open, build_fence, barred) = (true, None, false);
                        }
                    }
                    1 => {
                        life.barrier();
                        if (build_open || build_fence.is_some()) && life.state() != State::Retired {
                            barred = true;
                        }
                    }
                    2 => {
                        if life.readable().is_ok() {
                            assert!(matches!(life.state(), State::Building | State::Ready));
                            assert!(barred, "read with no barrier after the build");
                            life.pin();
                            open_pins += 1;
                        }
                    }
                    3 => {
                        // The open list is submitted: its build and pins.
                        fence += 1;
                        if build_open {
                            life.build_submitted(fence);
                            (build_open, build_fence) = (false, Some(fence));
                            highest_use = fence;
                        }
                        if open_pins > 0 {
                            highest_use = fence;
                        }
                        for _ in 0..std::mem::take(&mut open_pins) {
                            life.sealed(fence);
                        }
                    }
                    4 => {
                        completed = fence.min(completed + next(3));
                        life.completed(completed);
                        if build_fence.is_some_and(|f| f <= completed) {
                            build_fence = None;
                        }
                    }
                    5 => {
                        life.invalidate();
                    }
                    6 => {
                        // The open list is dropped.
                        if build_open {
                            (build_open, build_fence, barred) = (false, None, false);
                        }
                        life.build_abandoned();
                        for _ in 0..std::mem::take(&mut open_pins) {
                            life.unpinned();
                        }
                    }
                    7 => {
                        read_when_given_up.get_or_insert(life.readable().is_ok());
                        life.retire();
                    }
                    _ => {}
                }
                // Through a top level: what may be read anyway, and a
                // structure given up only if it could be read then.
                if life.readable_under_a_top().is_ok() {
                    through += 1;
                    assert!(barred, "read through a top level with no barrier after the build");
                    match life.state() {
                        State::Retired => assert_eq!(read_when_given_up, Some(true), "given up while not readable"),
                        _ => assert_eq!(life.readable(), Ok(())),
                    }
                } else if life.state() == State::Retired {
                    refused_given_up += 1;
                }
                match life.releasable() {
                    None => assert!(open_pins > 0 || build_open, "held with nothing open"),
                    Some(after) => {
                        assert!(open_pins == 0 && !build_open, "free while an open list uses it");
                        assert!(after >= highest_use, "free after {after}, used by {highest_use}");
                    }
                }
                if life.state() == State::Ready {
                    assert!(!build_open, "ready with a build not yet submitted");
                }
            }
        }
        assert!(through > 1_000 && refused_given_up > 1_000, "{through} read through, {refused_given_up} refused");
    }
}
