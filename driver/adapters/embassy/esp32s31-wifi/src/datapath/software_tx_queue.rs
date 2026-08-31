//! Role-neutral indexed software TX queues.
//!
//! Payload owners live in one bounded arena. Queueing policy stores only
//! indices and a small flow key, so adding a peer/TID queue never duplicates
//! packet storage or reserves a private DMA-sized ring. Roles remain
//! responsible for constructing and validating their keys.

#[derive(Clone, Copy)]
struct IndexedFlow<K> {
    key: K,
    head: u8,
    tail: u8,
    len: u8,
}

/// Single owner of retained packet leases referenced by indexed queues.
pub(crate) struct IndexedLeaseArena<B, const FRAME_CAPACITY: usize> {
    frames: [Option<B>; FRAME_CAPACITY],
    free: [u8; FRAME_CAPACITY],
    free_len: u8,
    initialized: bool,
}

impl<B, const FRAME_CAPACITY: usize> IndexedLeaseArena<B, FRAME_CAPACITY> {
    pub(crate) const fn new() -> Self {
        assert!(FRAME_CAPACITY > 0, "an indexed lease arena cannot be empty");
        assert!(
            FRAME_CAPACITY <= u8::MAX as usize,
            "an indexed lease arena uses byte-sized indices"
        );
        Self {
            frames: [const { None }; FRAME_CAPACITY],
            free: [0; FRAME_CAPACITY],
            free_len: 0,
            initialized: false,
        }
    }

    fn initialize_free_list(&mut self) {
        if self.initialized {
            return;
        }
        for (index, free) in self.free.iter_mut().enumerate() {
            *free = u8::try_from(index).expect("validated lease index fits u8");
        }
        self.free_len =
            u8::try_from(FRAME_CAPACITY).expect("validated lease arena capacity fits u8");
        self.initialized = true;
    }

    pub(crate) fn insert(&mut self, frame: B) -> Result<u8, B> {
        self.initialize_free_list();
        if self.free_len == 0 {
            return Err(frame);
        }
        self.free_len -= 1;
        let index = self.free[usize::from(self.free_len)];
        self.frames[usize::from(index)] = Some(frame);
        Ok(index)
    }

    pub(crate) fn take(&mut self, index: u8) -> B {
        let frame = self.frames[usize::from(index)]
            .take()
            .expect("queue index names one retained lease");
        self.free[usize::from(self.free_len)] = index;
        self.free_len += 1;
        frame
    }
}

/// Intrusive per-flow FIFOs with round-robin selection over active flows.
///
/// `FLOW_CAPACITY` bounds metadata, while `FRAME_CAPACITY` bounds the shared
/// owner arena. No payload capacity is reserved for an individual flow.
pub(crate) struct RoundRobinTxQueues<K, const FLOW_CAPACITY: usize, const FRAME_CAPACITY: usize> {
    next: [Option<u8>; FRAME_CAPACITY],
    flows: [Option<IndexedFlow<K>>; FLOW_CAPACITY],
    scheduler_cursor: u8,
    len: u8,
}

impl<K, const FLOW_CAPACITY: usize, const FRAME_CAPACITY: usize>
    RoundRobinTxQueues<K, FLOW_CAPACITY, FRAME_CAPACITY>
where
    K: Copy + Eq,
{
    pub(crate) const fn new() -> Self {
        assert!(FLOW_CAPACITY > 0, "a TXQ set must contain a flow slot");
        assert!(
            FLOW_CAPACITY <= u8::MAX as usize,
            "indexed TX queues use byte-sized flow indices"
        );
        assert!(
            FRAME_CAPACITY <= u8::MAX as usize,
            "indexed TX queues use byte-sized frame indices"
        );
        Self {
            next: [None; FRAME_CAPACITY],
            flows: [const { None }; FLOW_CAPACITY],
            scheduler_cursor: 0,
            len: 0,
        }
    }

    fn flow_index(&self, key: K) -> Option<usize> {
        self.flows
            .iter()
            .position(|flow| flow.is_some_and(|flow| flow.key == key))
    }

    pub(crate) fn push_back(&mut self, key: K, frame_index: u8) -> Result<(), u8> {
        let flow_index = if let Some(index) = self.flow_index(key) {
            index
        } else if let Some(index) = self.flows.iter().position(Option::is_none) {
            index
        } else {
            return Err(frame_index);
        };
        debug_assert!(usize::from(frame_index) < FRAME_CAPACITY);
        debug_assert!(self.next[usize::from(frame_index)].is_none());
        if let Some(flow) = self.flows[flow_index].as_mut() {
            self.next[usize::from(flow.tail)] = Some(frame_index);
            flow.tail = frame_index;
            flow.len = flow
                .len
                .checked_add(1)
                .expect("one flow cannot exceed the bounded frame arena");
        } else {
            self.flows[flow_index] = Some(IndexedFlow {
                key,
                head: frame_index,
                tail: frame_index,
                len: 1,
            });
        }
        self.len = self
            .len
            .checked_add(1)
            .expect("TXQ length is bounded by its frame arena");
        Ok(())
    }

    /// Restore an older frame ahead of every still-queued frame for `key`.
    ///
    /// A caller uses this only when a frame was removed speculatively and a
    /// lower-layer admission step failed. Appending that frame would let
    /// younger packets overtake it and corrupt the per-flow FIFO contract.
    pub(crate) fn push_front(&mut self, key: K, frame_index: u8) -> Result<(), u8> {
        let flow_index = if let Some(index) = self.flow_index(key) {
            index
        } else if let Some(index) = self.flows.iter().position(Option::is_none) {
            index
        } else {
            return Err(frame_index);
        };
        debug_assert!(usize::from(frame_index) < FRAME_CAPACITY);
        debug_assert!(self.next[usize::from(frame_index)].is_none());
        if let Some(flow) = self.flows[flow_index].as_mut() {
            self.next[usize::from(frame_index)] = Some(flow.head);
            flow.head = frame_index;
            flow.len = flow
                .len
                .checked_add(1)
                .expect("one flow cannot exceed the bounded frame arena");
        } else {
            self.flows[flow_index] = Some(IndexedFlow {
                key,
                head: frame_index,
                tail: frame_index,
                len: 1,
            });
        }
        self.len = self
            .len
            .checked_add(1)
            .expect("TXQ length is bounded by its frame arena");
        Ok(())
    }

    fn pop_flow_index(&mut self, flow_index: usize) -> Option<u8> {
        let mut flow = self.flows.get(flow_index).copied().flatten()?;
        let index = flow.head;
        let next = self.next[usize::from(index)].take();
        flow.len -= 1;
        if flow.len == 0 {
            self.flows[flow_index] = None;
        } else {
            flow.head = next.expect("a nonterminal TXQ frame has a successor");
            self.flows[flow_index] = Some(flow);
        }
        self.len -= 1;
        Some(index)
    }

    pub(crate) fn pop_key(&mut self, key: K) -> Option<u8> {
        let flow = self.flow_index(key)?;
        self.pop_flow_index(flow)
    }

    fn next_active_flow(&self) -> Option<usize> {
        for offset in 0..FLOW_CAPACITY {
            let index = (usize::from(self.scheduler_cursor) + offset) % FLOW_CAPACITY;
            if self.flows[index].is_some() {
                return Some(index);
            }
        }
        None
    }

    pub(crate) fn pop_scheduled(&mut self) -> Option<(K, u8)> {
        let index = self.next_active_flow()?;
        let key = self.flows[index]
            .expect("the scheduler scan names an occupied flow")
            .key;
        self.scheduler_cursor =
            u8::try_from((index + 1) % FLOW_CAPACITY).expect("flow index fits u8");
        self.pop_flow_index(index).map(|frame| (key, frame))
    }

    pub(crate) fn len_for(&self, key: K) -> usize {
        self.flow_index(key)
            .and_then(|index| self.flows[index])
            .map_or(0, |flow| usize::from(flow.len))
    }

    pub(crate) fn scheduled_len(&self) -> usize {
        self.next_active_flow()
            .and_then(|index| self.flows[index])
            .map_or(0, |flow| usize::from(flow.len))
    }

    pub(crate) const fn len(&self) -> usize {
        self.len as usize
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexedLeaseArena, RoundRobinTxQueues};

    const FLOW_A: u8 = 1;
    const FLOW_B: u8 = 2;
    const FLOW_CAPACITY: usize = 4;
    const FRAME_CAPACITY: usize = 8;

    struct TestQueues<B> {
        leases: IndexedLeaseArena<B, FRAME_CAPACITY>,
        queues: RoundRobinTxQueues<u8, FLOW_CAPACITY, FRAME_CAPACITY>,
    }

    impl<B> TestQueues<B> {
        const fn new() -> Self {
            Self {
                leases: IndexedLeaseArena::new(),
                queues: RoundRobinTxQueues::new(),
            }
        }

        fn push(&mut self, key: u8, frame: B) -> Result<(), B> {
            let index = self.leases.insert(frame)?;
            if self.queues.push_back(key, index).is_err() {
                return Err(self.leases.take(index));
            }
            Ok(())
        }

        fn push_front(&mut self, key: u8, frame: B) -> Result<(), B> {
            let index = self.leases.insert(frame)?;
            if self.queues.push_front(key, index).is_err() {
                return Err(self.leases.take(index));
            }
            Ok(())
        }

        fn pop_key(&mut self, key: u8) -> Option<B> {
            self.queues
                .pop_key(key)
                .map(|index| self.leases.take(index))
        }

        fn pop_scheduled(&mut self) -> Option<(u8, B)> {
            self.queues
                .pop_scheduled()
                .map(|(key, index)| (key, self.leases.take(index)))
        }
    }

    #[test]
    fn interleaved_frames_keep_per_flow_order() {
        let mut queues = TestQueues::new();
        for sequence in 0..4_u8 {
            queues.push(FLOW_A, sequence * 2).unwrap();
            queues.push(FLOW_B, sequence * 2 + 1).unwrap();
        }

        assert_eq!(queues.queues.len_for(FLOW_A), 4);
        assert_eq!(queues.queues.len_for(FLOW_B), 4);
        for sequence in 0..4_u8 {
            assert_eq!(queues.pop_key(FLOW_A), Some(sequence * 2));
        }
        for sequence in 0..4_u8 {
            assert_eq!(queues.pop_key(FLOW_B), Some(sequence * 2 + 1));
        }
        assert_eq!(queues.queues.len(), 0);
    }

    #[test]
    fn scheduler_round_robins_nonempty_flows() {
        let mut queues = TestQueues::new();
        queues.push(FLOW_A, 10).unwrap();
        queues.push(FLOW_A, 11).unwrap();
        queues.push(FLOW_B, 20).unwrap();
        queues.push(FLOW_B, 21).unwrap();

        assert_eq!(queues.pop_scheduled(), Some((FLOW_A, 10)));
        assert_eq!(queues.pop_scheduled(), Some((FLOW_B, 20)));
        assert_eq!(queues.pop_scheduled(), Some((FLOW_A, 11)));
        assert_eq!(queues.pop_scheduled(), Some((FLOW_B, 21)));
        assert_eq!(queues.queues.scheduled_len(), 0);
    }

    #[test]
    fn arena_reuses_every_bounded_frame_slot() {
        let mut queues = TestQueues::new();
        for value in 0..FRAME_CAPACITY {
            queues.push(FLOW_A, value).unwrap();
        }
        assert_eq!(queues.push(FLOW_A, usize::MAX), Err(usize::MAX));
        for value in 0..FRAME_CAPACITY {
            assert_eq!(queues.pop_key(FLOW_A), Some(value));
        }
        queues.push(FLOW_B, 42).unwrap();
        assert_eq!(queues.pop_key(FLOW_B), Some(42));
    }

    #[test]
    fn failed_admission_restores_the_original_flow_prefix() {
        let mut queues = TestQueues::new();
        queues.push(FLOW_A, 12).unwrap();
        queues.push(FLOW_A, 13).unwrap();

        // Frames 10 and 11 were speculatively removed before 12 and 13.
        // Restore in reverse insertion order so their original prefix is
        // visible to the next scheduler turn.
        queues.push_front(FLOW_A, 11).unwrap();
        queues.push_front(FLOW_A, 10).unwrap();

        for expected in 10..14 {
            assert_eq!(queues.pop_key(FLOW_A), Some(expected));
        }
    }

    #[test]
    fn full_flow_table_returns_the_unqueued_index() {
        let mut queues = TestQueues::new();
        for flow in 0..FLOW_CAPACITY as u8 {
            queues.push(flow, flow).unwrap();
        }
        assert_eq!(queues.push(99, 99), Err(99));
    }
}
