//! A bounded pool of tokens, whose capacity a const check proves.

/// The tokens of one kind a machine holds at most.
pub(crate) const POOL_CAPACITY: usize = 8;

/// A bounded pool of tokens of one kind.
pub(crate) struct Pool<X> {
    items: [Option<X>; POOL_CAPACITY],
}

impl<X> Pool<X> {
    /// A pool of `items`, at most [`POOL_CAPACITY`]: checked at compile
    /// time, so no granted token is ever lost.
    pub(crate) fn new<const N: usize>(items: [X; N]) -> Self {
        const { assert!(N <= POOL_CAPACITY, "a station holds at most 8 tokens of a kind") };
        let mut pool = Self {
            items: [const { None }; POOL_CAPACITY],
        };
        for (slot, item) in pool.items.iter_mut().zip(items) {
            *slot = Some(item);
        }
        pool
    }

    pub(crate) fn take(&mut self) -> Option<X> {
        self.items.iter_mut().find_map(Option::take)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.iter().all(Option::is_none)
    }

    /// Keep `item`, a token of its kind the station was granted and gave
    /// out: the tokens are unique and their number checked at the pool's
    /// creation, so a free entry always waits for it.
    pub(crate) fn put(&mut self, item: X) {
        let slot = self.items.iter_mut().find(|slot| slot.is_none());
        debug_assert!(
            slot.is_some(),
            "a station holds at most POOL_CAPACITY tokens of a kind"
        );
        if let Some(slot) = slot {
            *slot = Some(item);
        }
    }
}

/// A first-in, first-out queue of at most [`POOL_CAPACITY`] items: it holds
/// what a machine owes for tokens it took from a [`Pool`] of that size, so
/// it never fills beyond them.
pub(crate) struct Fifo<X> {
    items: [Option<X>; POOL_CAPACITY],
    head: usize,
    len: usize,
}

impl<X> Fifo<X> {
    pub(crate) const fn new() -> Self {
        Self {
            items: [const { None }; POOL_CAPACITY],
            head: 0,
            len: 0,
        }
    }

    /// Queue `item` at the back.
    pub(crate) fn push(&mut self, item: X) {
        assert!(self.len < POOL_CAPACITY, "a queue holds at most its pool's tokens");
        let at = (self.head + self.len) % POOL_CAPACITY;
        self.items[at] = Some(item);
        self.len += 1;
    }

    /// The front item.
    pub(crate) fn pop(&mut self) -> Option<X> {
        if self.len == 0 {
            return None;
        }
        let item = self.items[self.head].take();
        self.head = (self.head + 1) % POOL_CAPACITY;
        self.len -= 1;
        item
    }
}
