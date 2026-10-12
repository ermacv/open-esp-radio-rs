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
