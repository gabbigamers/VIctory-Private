//! Snapshot memory kept for reuse (already touched, so no page faults): the
//! buffers `crate::shadow` copies a range of a host copy into before a
//! submission whose shaders write it. The pool needs no device, so it is
//! here and not in that module, where its bookkeeping is tested on any host.
//!
//! Two counts follow the buffers (`crate::account`): one of those handed out
//! and not yet given back, one of those the pool holds. Both are by
//! capacity, which is what each buffer asked the system for. A buffer handed
//! out carries its count with it ([`Taken`]), so the count ends wherever the
//! buffer does. The caller holds the pool's lock around every call, so the
//! pool's count moves with its buffers.

use crate::account::{Gauge, Held};

/// The buffers kept, in no order between calls.
pub(crate) struct Pool {
    buffers: Vec<Vec<u8>>,
}

/// A snapshot buffer handed out, used as the `Vec` it holds. Its capacity
/// when handed out is counted as a live snapshot for as long as this lives:
/// until [`Pool::give_back`], or until it is dropped any other way (with the
/// copy that still holds it, or where a caller unwinds).
pub(crate) struct Taken {
    bytes: Vec<u8>,
    _held: Held,
}

impl std::ops::Deref for Taken {
    type Target = Vec<u8>;

    fn deref(&self) -> &Vec<u8> {
        &self.bytes
    }
}

impl std::ops::DerefMut for Taken {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.bytes
    }
}

impl Pool {
    pub(crate) const fn new() -> Self {
        Pool { buffers: Vec::new() }
    }

    /// An empty buffer with room for `n` bytes, from the pool if it has one.
    /// It is counted in `live` for as long as it lives outside the pool, and
    /// no longer in `kept` if it came from the pool.
    pub(crate) fn take(&mut self, n: usize, live: &'static Gauge, kept: &Gauge) -> Taken {
        let fit = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, v)| v.capacity() >= n)
            .min_by_key(|(_, v)| v.capacity())
            .map(|(i, _)| i);
        let mut v = match fit {
            Some(i) => {
                let v = self.buffers.swap_remove(i);
                kept.sub(v.capacity() as u64);
                v
            }
            None => Vec::with_capacity(n),
        };
        v.clear();
        let held = Held::on(live, v.capacity() as u64);
        Taken { bytes: v, _held: held }
    }

    /// Returns a buffer [`Pool::take`] gave to the pool, which keeps the
    /// largest up to `limit` bytes in all and frees the rest. The buffer's
    /// count as a live snapshot ends, and `kept` is what the pool holds
    /// after.
    pub(crate) fn give_back(&mut self, v: Taken, limit: usize, kept: &Gauge) {
        let Taken { bytes: v, _held: live } = v;
        drop(live);
        self.buffers.push(v);
        self.buffers.sort_by_key(|v| std::cmp::Reverse(v.capacity()));
        // The buffers kept are the first ones: `held` is their total.
        let (mut total, mut held) = (0, 0);
        self.buffers.retain(|v| {
            total += v.capacity();
            if total <= limit {
                held = total;
            }
            total <= limit
        });
        kept.set(held as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(pool: &Pool) -> u64 {
        pool.buffers.iter().map(|v| v.capacity() as u64).sum()
    }

    /// A buffer is in exactly one of the two counts from the moment it is
    /// made: handed out, or kept. A kept one handed out again is not counted
    /// as a new allocation's bytes on top of the pool's.
    #[test]
    fn a_buffer_is_counted_as_handed_out_or_as_kept_never_both() {
        static LIVE: Gauge = Gauge::new();
        static KEPT: Gauge = Gauge::new();
        let mut pool = Pool::new();
        let a = pool.take(100, &LIVE, &KEPT);
        let size = a.capacity() as u64;
        assert!(size >= 100);
        assert_eq!((LIVE.now(), KEPT.now()), (size, 0));
        pool.give_back(a, 1 << 20, &KEPT);
        assert_eq!((LIVE.now(), KEPT.now()), (0, size));
        // A smaller request takes the kept buffer: the same bytes move back.
        let b = pool.take(60, &LIVE, &KEPT);
        assert_eq!(b.capacity() as u64, size, "reused, not made");
        assert_eq!((LIVE.now(), KEPT.now()), (size, 0));
        // Written to within its capacity, as a snapshot is: the same count
        // comes off when it goes back.
        let mut b = b;
        b.extend_from_slice(&[7; 60]);
        pool.give_back(b, 1 << 20, &KEPT);
        assert_eq!((LIVE.now(), KEPT.now()), (0, size));
        assert_eq!((LIVE.peak(), LIVE.underflows(), KEPT.underflows()), (size, 0, 0));
    }

    /// The pool's count is what it holds after every call, never above its
    /// limit; what it frees leaves both counts.
    #[test]
    fn the_kept_count_is_what_the_pool_holds_and_stays_under_its_limit() {
        static LIVE: Gauge = Gauge::new();
        static KEPT: Gauge = Gauge::new();
        const LIMIT: usize = 1000;
        let mut pool = Pool::new();
        // Three snapshots alive at once, as one submission takes them.
        let out: Vec<Taken> = [600, 300, 200].map(|n| pool.take(n, &LIVE, &KEPT)).into();
        let sizes: Vec<u64> = out.iter().map(|v| v.capacity() as u64).collect();
        assert_eq!((LIVE.now(), KEPT.now()), (sizes.iter().sum(), 0));
        let mut alive: u64 = sizes.iter().sum();
        for v in out.into_iter().rev() {
            alive -= v.capacity() as u64;
            pool.give_back(v, LIMIT, &KEPT);
            assert_eq!((LIVE.now(), KEPT.now()), (alive, held(&pool)));
            assert!(KEPT.now() <= LIMIT as u64);
        }
        // All three do not fit: the smallest was freed and is in no count.
        assert_eq!(pool.buffers.len(), 2);
        assert_eq!((LIVE.now(), KEPT.now()), (0, sizes[0] + sizes[1]));
        // The smallest that fits is taken, and leaves the kept count.
        let v = pool.take(250, &LIVE, &KEPT);
        assert_eq!(v.capacity() as u64, sizes[1]);
        assert_eq!((LIVE.now(), KEPT.now()), (sizes[1], sizes[0]));
        // A request nothing kept fits is a new buffer: the pool's count
        // stays.
        let big = pool.take(700, &LIVE, &KEPT);
        assert_eq!((LIVE.now(), KEPT.now()), (sizes[1] + big.capacity() as u64, sizes[0]));
        pool.give_back(big, LIMIT, &KEPT);
        pool.give_back(v, LIMIT, &KEPT);
        assert_eq!((LIVE.now(), KEPT.now()), (0, held(&pool)));
        assert!(KEPT.peak() <= LIMIT as u64, "{}", KEPT.peak());
        assert_eq!((LIVE.underflows(), KEPT.underflows()), (0, 0));
    }

    /// A buffer over the limit is not kept, whatever else that does to the
    /// pool: the count still says what is held.
    #[test]
    fn a_buffer_over_the_limit_leaves_the_count_at_what_is_held() {
        static LIVE: Gauge = Gauge::new();
        static KEPT: Gauge = Gauge::new();
        const LIMIT: usize = 1000;
        let mut pool = Pool::new();
        let (small, large) = (pool.take(100, &LIVE, &KEPT), pool.take(LIMIT + 1, &LIVE, &KEPT));
        pool.give_back(small, LIMIT, &KEPT);
        assert_eq!(KEPT.now(), held(&pool));
        assert!(KEPT.now() > 0);
        pool.give_back(large, LIMIT, &KEPT);
        assert!(pool.buffers.iter().all(|v| v.capacity() <= LIMIT));
        assert_eq!((LIVE.now(), KEPT.now()), (0, held(&pool)));
        assert_eq!((LIVE.underflows(), KEPT.underflows()), (0, 0));
    }

    /// A snapshot that never goes back (its copy is dropped between a
    /// submission and the copy-back, or the caller unwinds while filling
    /// it) is freed, not pooled: it leaves the live count and the pool's
    /// count is not touched.
    #[test]
    fn a_buffer_dropped_without_going_back_leaves_the_live_count() {
        static LIVE: Gauge = Gauge::new();
        static KEPT: Gauge = Gauge::new();
        let mut pool = Pool::new();
        let kept = pool.take(300, &LIVE, &KEPT);
        let kept_size = kept.capacity() as u64;
        pool.give_back(kept, 1 << 20, &KEPT);
        let (a, mut b) = (pool.take(100, &LIVE, &KEPT), pool.take(500, &LIVE, &KEPT));
        assert_eq!(a.capacity() as u64, kept_size, "the kept one, handed out");
        let fresh = b.capacity() as u64;
        b.extend_from_slice(&[1; 500]);
        assert_eq!((LIVE.now(), KEPT.now()), (kept_size + fresh, 0));
        // Held in a copy's segment list, as `crate::shadow` holds them, and
        // dropped with it.
        let segments = vec![(0usize, a), (4096, b)];
        drop(segments);
        assert_eq!((LIVE.now(), KEPT.now()), (0, 0), "freed: in neither count");
        assert!(pool.buffers.is_empty());
        // The next one is counted afresh, once.
        let c = pool.take(100, &LIVE, &KEPT);
        assert_eq!(LIVE.now(), c.capacity() as u64);
        pool.give_back(c, 1 << 20, &KEPT);
        assert_eq!((LIVE.now(), LIVE.underflows(), KEPT.underflows()), (0, 0, 0));
    }
}
