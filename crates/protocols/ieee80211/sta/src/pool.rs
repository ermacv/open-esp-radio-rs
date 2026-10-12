//! A bounded pool of tokens, whose capacity a const check proves.

/// The tokens of one kind a machine holds at most.
pub(crate) const POOL_CAPACITY: usize = 8;

/// A bounded pool of tokens of one kind.
pub(crate) struct Pool<X> {
    items: [Option<X>; POOL_CAPACITY],
    /// Tokens given back to a full pool: dropped, counted.
    lost: u32,
}

impl<X> Pool<X> {
    /// A pool of `items`, at most [`POOL_CAPACITY`]: checked at compile
    /// time, so no granted token is ever lost.
    pub(crate) fn new<const N: usize>(items: [X; N]) -> Self {
        const {
            assert!(
                N <= POOL_CAPACITY,
                "a station holds at most 8 tokens of a kind"
            )
        };
        let mut pool = Self {
            items: [const { None }; POOL_CAPACITY],
            lost: 0,
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

    /// Keep `item`, a token of its kind the machine was granted and gave
    /// out: the tokens are unique and their number checked at the pool's
    /// creation, so a free entry waits for it. A token beyond them (not
    /// one of the machine's) is dropped and counted.
    pub(crate) fn put(&mut self, item: X) {
        match self.items.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(item),
            None => self.lost = self.lost.saturating_add(1),
        }
    }

    /// The tokens dropped because the pool was full.
    pub(crate) const fn lost(&self) -> u32 {
        self.lost
    }
}

/// A first-in, first-out queue of at most `N` items. A machine sizes it by
/// a const parameter checked at build against the tokens its items hold;
/// an item beyond `N` comes back to the caller.
pub(crate) struct Fifo<X, const N: usize> {
    items: [Option<X>; N],
    head: usize,
    len: usize,
}

impl<X, const N: usize> Fifo<X, N> {
    pub(crate) const fn new() -> Self {
        Self {
            items: [const { None }; N],
            head: 0,
            len: 0,
        }
    }

    /// Queue `item` at the back.
    ///
    /// # Errors
    ///
    /// The item, when the queue holds `N`.
    pub(crate) fn push(&mut self, item: X) -> Result<(), X> {
        if self.len >= N {
            return Err(item);
        }
        let at = (self.head + self.len) % N;
        self.items[at] = Some(item);
        self.len += 1;
        Ok(())
    }

    /// Queue `item` at the front.
    ///
    /// # Errors
    ///
    /// The item, when the queue holds `N`.
    pub(crate) fn push_front(&mut self, item: X) -> Result<(), X> {
        if self.len >= N {
            return Err(item);
        }
        self.head = (self.head + N - 1) % N;
        self.items[self.head] = Some(item);
        self.len += 1;
        Ok(())
    }

    /// The front item, in place.
    pub(crate) fn front(&self) -> Option<&X> {
        if self.len == 0 {
            return None;
        }
        self.items[self.head].as_ref()
    }

    /// The front item.
    pub(crate) fn pop(&mut self) -> Option<X> {
        if self.len == 0 {
            return None;
        }
        let item = self.items[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        item
    }
}
