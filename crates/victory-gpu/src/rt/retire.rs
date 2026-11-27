//! Things that are given up but may still be in use by the GPU: each waits
//! for the fence value of the last submission that can touch it, and is
//! handed back for release once that value is complete.
//!
//! The queue knows nothing of D3D12: its items are whatever the caller
//! holds (buffers, in `super::device`), and time is the fence values the
//! caller passes. An item used by a list that is not submitted yet has no
//! value to wait for and does not belong here: its owner keeps it until the
//! list is submitted (`super::life`, `super::cache`).

/// Items waiting for a fence value each, with their sizes.
pub struct RetireQueue<T> {
    /// In the order they were retired: the value to wait for, the item, its
    /// bytes.
    waiting: Vec<(u64, T, u64)>,
    bytes: u64,
}

impl<T> Default for RetireQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> RetireQueue<T> {
    pub const fn new() -> Self {
        RetireQueue { waiting: Vec::new(), bytes: 0 }
    }

    /// `item`, of `bytes` bytes, is given up: no submission after the one
    /// with fence value `after` uses it.
    pub fn retire(&mut self, item: T, after: u64, bytes: u64) {
        self.waiting.push((after, item, bytes));
        self.bytes += bytes;
    }

    /// Every submission up to fence value `completed` has run: the items
    /// that waited for no later one, in the order they were retired. The
    /// caller drops them.
    pub fn release(&mut self, completed: u64) -> Vec<T> {
        if self.waiting.iter().all(|(after, ..)| *after > completed) {
            return Vec::new();
        }
        let mut released = Vec::new();
        for (after, item, bytes) in std::mem::take(&mut self.waiting) {
            if after <= completed {
                self.bytes -= bytes;
                released.push(item);
            } else {
                self.waiting.push((after, item, bytes));
            }
        }
        released
    }

    /// How many items wait, and their bytes: memory that is given up and
    /// not yet free.
    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The highest fence value anything waits for: waiting for it releases
    /// everything.
    pub fn latest(&self) -> Option<u64> {
        self.waiting.iter().map(|(after, ..)| *after).max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_goes_before_its_submission_has_run() {
        let mut queue = RetireQueue::new();
        queue.retire("a", 5, 100);
        queue.retire("b", 3, 10);
        queue.retire("c", 7, 1);
        assert_eq!((queue.len(), queue.bytes(), queue.latest()), (3, 111, Some(7)));
        assert!(queue.release(2).is_empty());
        assert_eq!(queue.release(3), ["b"]);
        assert_eq!((queue.len(), queue.bytes()), (2, 101));
        assert!(queue.release(4).is_empty());
        assert_eq!(queue.release(6), ["a"]);
        assert_eq!(queue.release(100), ["c"]);
        assert!(queue.is_empty());
        assert_eq!((queue.bytes(), queue.latest()), (0, None));
    }

    #[test]
    fn what_is_released_together_comes_in_the_order_it_was_retired() {
        let mut queue = RetireQueue::new();
        for (i, after) in [4u64, 1, 9, 2, 4, 3].into_iter().enumerate() {
            queue.retire(i, after, 1);
        }
        assert_eq!(queue.release(4), [0, 1, 3, 4, 5]);
        assert_eq!(queue.release(4), [0usize; 0], "once");
        assert_eq!(queue.release(9), [2]);
    }

    /// Something no submission used waits for value 0, which is complete
    /// from the start: it goes at the next release.
    #[test]
    fn something_never_used_goes_at_once() {
        let mut queue = RetireQueue::new();
        queue.retire((), 0, 64);
        assert_eq!(queue.release(0).len(), 1);
    }

    /// Dropping what is handed back is the release: the queue keeps no
    /// second holder.
    #[test]
    fn the_queue_holds_each_item_once() {
        let item = std::rc::Rc::new(());
        let mut queue = RetireQueue::new();
        queue.retire(item.clone(), 2, 0);
        assert_eq!(std::rc::Rc::strong_count(&item), 2);
        assert!(queue.release(1).is_empty());
        assert_eq!(std::rc::Rc::strong_count(&item), 2, "kept while its submission may run");
        drop(queue.release(2));
        assert_eq!(std::rc::Rc::strong_count(&item), 1);
    }
}
