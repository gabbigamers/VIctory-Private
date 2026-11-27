//! Memory that builds need only while they run: the scratch a build works
//! in, and the instance descriptions a top-level build reads. Handed out
//! from chunks that are reused once the GPU is done with them, under a
//! bound.
//!
//! The pool holds no D3D12 type: a chunk is whatever buffer the caller
//! makes for it, and time is the fence values the caller passes, so the
//! bookkeeping is tested on any host.
//!
//! The rules it keeps:
//!
//! * every piece handed out for one open list is its own range: builds in
//!   one list may run side by side, so no two share scratch;
//! * a chunk used by a list is not handed out again until that list's
//!   submission is complete: the build may still be working in it (and a
//!   top-level build still reading its instances);
//! * the pool never holds more than [`Limits::live`] bytes. A request that
//!   would pass it is refused: the caller submits what it has, waits and
//!   asks again, or does without;
//! * idle chunks beyond [`Limits::keep`] bytes are let go ([`Pool::trim`]),
//!   so one large build does not hold its scratch for the rest of the run.

/// The unit committed buffers are sized in.
pub const GRANULE: u64 = 64 << 10;

/// A pool's sizes, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The alignment of every piece (a power of two).
    pub align: u64,
    /// The size of a chunk made for requests that fit one.
    pub chunk: u64,
    /// Idle memory kept for reuse.
    pub keep: u64,
    /// The most the pool holds at once, in use or idle.
    pub live: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    Idle,
    /// Pieces of it are handed out for the list being recorded.
    Open,
    /// The submission with this fence value uses it.
    Busy(u64),
}

struct Chunk<B> {
    buffer: B,
    capacity: u64,
    used: u64,
    in_use: Use,
}

/// Why a request got no memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// `live` bytes of the pool are in use; `asked` more would pass
    /// `limit`.
    OverBudget { live: u64, asked: u64, limit: u64 },
    /// No buffer of this many bytes could be made.
    NoBuffer(u64),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Refused::OverBudget { live, asked, limit } => {
                write!(f, "{asked} bytes asked with {live} in use, over the pool's {limit}")
            }
            Refused::NoBuffer(bytes) => write!(f, "no buffer of {bytes} bytes"),
        }
    }
}

pub struct Pool<B> {
    chunks: Vec<Chunk<B>>,
    limits: Limits,
}

impl<B> Pool<B> {
    pub fn new(limits: Limits) -> Self {
        debug_assert!(limits.align.is_power_of_two());
        Pool { chunks: Vec::new(), limits }
    }

    /// `bytes` of memory for the list being recorded, with every submission
    /// up to fence value `completed` known to have run: the chunk's buffer
    /// and the offset in it. `make` makes a buffer of the bytes it is given
    /// when no chunk has room.
    pub fn take(
        &mut self,
        bytes: u64,
        completed: u64,
        make: impl FnOnce(u64) -> Option<B>,
    ) -> Result<(&B, u64), Refused> {
        let limit = self.limits.live;
        let over = |live, asked| Refused::OverBudget { live, asked, limit };
        let held = self.bytes();
        let size = bytes.max(1).checked_next_multiple_of(self.limits.align).ok_or(over(held, bytes))?;
        for chunk in &mut self.chunks {
            if matches!(chunk.in_use, Use::Busy(fence) if fence <= completed) {
                chunk.in_use = Use::Idle;
            }
        }
        // Room left in a chunk this list already uses.
        let open = self.chunks.iter().position(|c| c.in_use == Use::Open && c.capacity - c.used >= size);
        // Else the smallest idle chunk that holds it.
        let found = open.or_else(|| {
            (0..self.chunks.len())
                .filter(|&i| self.chunks[i].in_use == Use::Idle && self.chunks[i].capacity >= size)
                .min_by_key(|&i| self.chunks[i].capacity)
        });
        let at = match found {
            Some(at) => at,
            None => {
                let capacity =
                    size.max(self.limits.chunk).checked_next_multiple_of(GRANULE).ok_or(over(held, size))?;
                let in_use: u64 = self.chunks.iter().filter(|c| c.in_use != Use::Idle).map(|c| c.capacity).sum();
                if in_use + capacity > limit {
                    return Err(over(in_use, capacity));
                }
                // Idle chunks are all too small for this: they go if the
                // new one would not fit beside them.
                if held + capacity > limit {
                    self.chunks.retain(|c| c.in_use != Use::Idle);
                }
                let buffer = make(capacity).ok_or(Refused::NoBuffer(capacity))?;
                self.chunks.push(Chunk { buffer, capacity, used: 0, in_use: Use::Idle });
                self.chunks.len() - 1
            }
        };
        let chunk = &mut self.chunks[at];
        if chunk.in_use == Use::Idle {
            (chunk.in_use, chunk.used) = (Use::Open, 0);
        }
        let offset = chunk.used;
        chunk.used += size;
        Ok((&chunk.buffer, offset))
    }

    /// The list being recorded was submitted with fence value `fence`: what
    /// it was handed stays in use until that value is complete.
    pub fn submitted(&mut self, fence: u64) {
        for chunk in &mut self.chunks {
            if chunk.in_use == Use::Open {
                chunk.in_use = Use::Busy(fence);
            }
        }
    }

    /// The list being recorded was dropped without being submitted: nothing
    /// uses what it was handed.
    pub fn abandoned(&mut self) {
        for chunk in &mut self.chunks {
            if chunk.in_use == Use::Open {
                chunk.in_use = Use::Idle;
            }
        }
    }

    /// Lets idle chunks go, largest first, until no more than
    /// [`Limits::keep`] bytes are idle; how many went. With every
    /// submission up to fence value `completed` known to have run.
    pub fn trim(&mut self, completed: u64) -> usize {
        let before = self.chunks.len();
        loop {
            let idle = |c: &Chunk<B>| match c.in_use {
                Use::Idle => true,
                Use::Busy(fence) => fence <= completed,
                Use::Open => false,
            };
            let kept: u64 = self.chunks.iter().filter(|c| idle(c)).map(|c| c.capacity).sum();
            if kept <= self.limits.keep {
                break;
            }
            let Some(largest) =
                (0..self.chunks.len()).filter(|&i| idle(&self.chunks[i])).max_by_key(|&i| self.chunks[i].capacity)
            else {
                break;
            };
            self.chunks.swap_remove(largest);
        }
        before - self.chunks.len()
    }

    /// The bytes of every chunk the pool holds.
    pub fn bytes(&self) -> u64 {
        self.chunks.iter().map(|c| c.capacity).sum()
    }

    pub fn chunks(&self) -> usize {
        self.chunks.len()
    }

    /// The highest fence value a chunk waits for, if any does.
    pub fn busy_until(&self) -> Option<u64> {
        self.chunks
            .iter()
            .filter_map(|c| match c.in_use {
                Use::Busy(fence) => Some(fence),
                _ => None,
            })
            .max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    const KB: u64 = 1 << 10;
    const LIMITS: Limits = Limits { align: 256, chunk: 256 * KB, keep: 512 * KB, live: 1024 * KB };

    /// A stand-in buffer: its number, and a count of those alive.
    struct Buffer(u32, Rc<Cell<u32>>);

    impl Drop for Buffer {
        fn drop(&mut self) {
            self.1.set(self.1.get() - 1);
        }
    }

    struct Maker {
        made: Cell<u32>,
        alive: Rc<Cell<u32>>,
    }

    impl Maker {
        fn new() -> Self {
            Maker { made: Cell::new(0), alive: Rc::new(Cell::new(0)) }
        }

        fn make(&self) -> impl FnOnce(u64) -> Option<Buffer> + '_ {
            |_| {
                self.made.set(self.made.get() + 1);
                self.alive.set(self.alive.get() + 1);
                Some(Buffer(self.made.get(), self.alive.clone()))
            }
        }
    }

    /// The chunk's number and the offset.
    fn take(pool: &mut Pool<Buffer>, maker: &Maker, bytes: u64, completed: u64) -> (u32, u64) {
        let (buffer, offset) = pool.take(bytes, completed, maker.make()).unwrap();
        (buffer.0, offset)
    }

    #[test]
    fn pieces_for_one_list_are_ranges_of_their_own() {
        let (mut pool, maker) = (Pool::new(LIMITS), Maker::new());
        assert_eq!(take(&mut pool, &maker, 1000, 0), (1, 0));
        assert_eq!(take(&mut pool, &maker, 1, 0), (1, 1024), "after the first, rounded up to the alignment");
        assert_eq!(take(&mut pool, &maker, 256, 0), (1, 1280));
        assert_eq!(take(&mut pool, &maker, 0, 0), (1, 1536), "nothing asked is still a range");
        assert_eq!((pool.chunks(), pool.bytes()), (1, 256 * KB));
        // What does not fit the rest of the chunk gets another.
        assert_eq!(take(&mut pool, &maker, 255 * KB, 0), (2, 0));
        assert_eq!(take(&mut pool, &maker, 2 * KB, 0), (1, 1792), "the first still has room");
    }

    #[test]
    fn a_chunk_is_not_handed_out_again_while_its_submission_may_run() {
        let (mut pool, maker) = (Pool::new(LIMITS), Maker::new());
        assert_eq!(take(&mut pool, &maker, 100 * KB, 0), (1, 0));
        pool.submitted(1);
        assert_eq!(pool.busy_until(), Some(1));
        assert_eq!(take(&mut pool, &maker, 100 * KB, 0), (2, 0), "submission 1 has not run");
        pool.submitted(2);
        assert_eq!(take(&mut pool, &maker, 100 * KB, 1), (1, 0), "submission 1 has run: its chunk from the start");
        assert_eq!(take(&mut pool, &maker, 100 * KB, 1), (1, 100 * KB));
        assert_eq!(take(&mut pool, &maker, 100 * KB, 1), (3, 0), "submission 2 has not");
        assert_eq!(maker.made.get(), 3);
    }

    #[test]
    fn a_dropped_list_frees_what_it_was_handed() {
        let (mut pool, maker) = (Pool::new(LIMITS), Maker::new());
        take(&mut pool, &maker, 10 * KB, 0);
        pool.abandoned();
        assert_eq!(take(&mut pool, &maker, 10 * KB, 0), (1, 0));
        assert_eq!(maker.made.get(), 1);
    }

    #[test]
    fn a_large_request_gets_a_buffer_of_its_own_size_in_whole_granules() {
        let (mut pool, maker) = (Pool::new(LIMITS), Maker::new());
        let mut asked = 0;
        let (_, offset) = pool
            .take(300 * KB + 1, 0, |bytes| {
                asked = bytes;
                maker.make()(bytes)
            })
            .unwrap();
        assert_eq!((offset, asked), (0, 320 * KB));
        assert_eq!(pool.bytes(), 320 * KB);
    }

    #[test]
    fn the_smallest_idle_chunk_that_fits_is_reused() {
        let (mut pool, maker) = (Pool::new(LIMITS), Maker::new());
        assert_eq!(take(&mut pool, &maker, 400 * KB, 0), (1, 0));
        assert_eq!(take(&mut pool, &maker, 300 * KB, 0), (2, 0));
        assert_eq!(pool.bytes(), (448 + 320) * KB);
        pool.submitted(1);
        assert_eq!(take(&mut pool, &maker, 290 * KB, 1), (2, 0), "not the larger one");
        assert_eq!(take(&mut pool, &maker, 310 * KB, 1), (1, 0), "which the next request needs");
        assert_eq!(maker.made.get(), 2);
    }

    #[test]
    fn the_pool_refuses_what_would_pass_its_bound() {
        let (mut pool, maker) = (Pool::new(LIMITS), Maker::new());
        for _ in 0..4 {
            take(&mut pool, &maker, 256 * KB, 0);
        }
        assert_eq!(pool.bytes(), LIMITS.live);
        let refused = pool.take(KB, 0, maker.make()).map(|_| ()).unwrap_err();
        assert_eq!(refused, Refused::OverBudget { live: 1024 * KB, asked: 256 * KB, limit: 1024 * KB });
        assert_eq!(maker.made.get(), 4, "nothing was made for the refused request");
        // Once the submission has run there is room again.
        pool.submitted(1);
        assert!(pool.take(KB, 0, maker.make()).is_err(), "not before");
        assert_eq!(take(&mut pool, &maker, KB, 1).1, 0);
        // More than the bound is refused whatever is idle.
        pool.submitted(2);
        let refused = pool.take(2000 * KB, 2, maker.make()).map(|_| ()).unwrap_err();
        assert_eq!(refused, Refused::OverBudget { live: 0, asked: 2048 * KB, limit: 1024 * KB });
        assert_eq!(pool.chunks(), 4, "and costs the idle chunks nothing");
    }

    /// A request no idle chunk holds makes room for itself among the idle
    /// ones before it is refused.
    #[test]
    fn idle_chunks_give_way_to_a_request_they_cannot_hold() {
        let (mut pool, maker) = (Pool::new(LIMITS), Maker::new());
        for _ in 0..4 {
            take(&mut pool, &maker, 256 * KB, 0);
        }
        pool.submitted(1);
        assert_eq!(take(&mut pool, &maker, 600 * KB, 1), (5, 0));
        assert_eq!((pool.chunks(), pool.bytes(), maker.alive.get()), (1, 640 * KB, 1));
    }

    #[test]
    fn idle_memory_beyond_what_is_kept_is_let_go_largest_first() {
        let (mut pool, maker) = (Pool::new(LIMITS), Maker::new());
        take(&mut pool, &maker, 512 * KB, 0);
        take(&mut pool, &maker, 100 * KB, 0);
        take(&mut pool, &maker, 200 * KB, 0);
        assert_eq!(pool.bytes(), (512 + 256 + 256) * KB);
        pool.submitted(3);
        assert_eq!(pool.trim(2), 0, "all of it is in use");
        assert_eq!(pool.trim(3), 1);
        assert_eq!((pool.bytes(), maker.alive.get()), (512 * KB, 2), "the large one went");
        assert_eq!(pool.trim(3), 0);
        // What an open list holds is never let go.
        let (mut pool, maker) = (Pool::new(Limits { keep: 0, ..LIMITS }), Maker::new());
        take(&mut pool, &maker, KB, 0);
        assert_eq!(pool.trim(u64::MAX), 0);
        pool.abandoned();
        assert_eq!((pool.trim(0), pool.bytes(), maker.alive.get()), (1, 0, 0));
    }

    #[test]
    fn a_buffer_that_cannot_be_made_is_a_refusal_and_changes_nothing() {
        let mut pool: Pool<Buffer> = Pool::new(LIMITS);
        let refused = pool.take(KB, 0, |_| None).map(|_| ()).unwrap_err();
        assert_eq!(refused, Refused::NoBuffer(256 * KB));
        assert_eq!((pool.chunks(), pool.bytes()), (0, 0));
    }

    #[test]
    fn other_alignments_are_kept() {
        let (mut pool, maker) = (Pool::new(Limits { align: 16, ..LIMITS }), Maker::new());
        assert_eq!(take(&mut pool, &maker, 64, 0).1, 0);
        assert_eq!(take(&mut pool, &maker, 65, 0).1, 64);
        assert_eq!(take(&mut pool, &maker, 64, 0).1, 144);
    }
}
