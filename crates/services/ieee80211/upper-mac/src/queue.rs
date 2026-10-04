//! A service's transmit queue: Ethernet frames in the order the caller sent
//! them, each with its user priority, until the service sends them one by
//! one or as an A-MPDU.

use oer_ieee80211_mac::qos::WmmUserPriority;

/// Octets of one frame a service keeps: a received MPDU, or a queued
/// Ethernet frame.
pub const PORT_FRAME_CAPACITY: usize = 2_352;

/// Frames the queue holds; also the most subframes of one A-MPDU.
pub const PORT_TX_QUEUE: usize = 8;

/// One queued Ethernet-II frame.
pub struct QueuedFrame {
    ethernet: [u8; PORT_FRAME_CAPACITY],
    len: usize,
    pub priority: WmmUserPriority,
}

impl QueuedFrame {
    pub fn ethernet(&self) -> &[u8] {
        &self.ethernet[..self.len]
    }
}

/// A first-in first-out ring of [`PORT_TX_QUEUE`] frames.
pub struct TxQueue {
    frames: [Option<QueuedFrame>; PORT_TX_QUEUE],
    head: usize,
    len: usize,
}

impl Default for TxQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TxQueue {
    pub const fn new() -> Self {
        Self {
            frames: [const { None }; PORT_TX_QUEUE],
            head: 0,
            len: 0,
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len == PORT_TX_QUEUE
    }

    /// Queue `ethernet` at `priority`; `false` when the queue is full or
    /// the frame exceeds [`PORT_FRAME_CAPACITY`].
    pub fn push(&mut self, ethernet: &[u8], priority: WmmUserPriority) -> bool {
        if self.is_full() || ethernet.len() > PORT_FRAME_CAPACITY {
            return false;
        }
        let mut frame = QueuedFrame {
            ethernet: [0; PORT_FRAME_CAPACITY],
            len: ethernet.len(),
            priority,
        };
        frame.ethernet[..ethernet.len()].copy_from_slice(ethernet);
        self.frames[(self.head + self.len) % PORT_TX_QUEUE] = Some(frame);
        self.len += 1;
        true
    }

    /// Frame `index` from the head.
    pub fn get(&self, index: usize) -> Option<&QueuedFrame> {
        if index >= self.len {
            return None;
        }
        self.frames[(self.head + index) % PORT_TX_QUEUE].as_ref()
    }

    pub fn pop(&mut self) -> Option<QueuedFrame> {
        if self.len == 0 {
            return None;
        }
        let frame = self.frames[self.head].take();
        self.head = (self.head + 1) % PORT_TX_QUEUE;
        self.len -= 1;
        frame
    }

    /// The frames from the head, at most `limit`, whose user priority is
    /// the head's: the run one A-MPDU may carry.
    pub fn head_run(&self, limit: usize) -> usize {
        let Some(head) = self.get(0) else {
            return 0;
        };
        (0..self.len.min(limit))
            .take_while(|index| {
                self.get(*index)
                    .is_some_and(|frame| frame.priority == head.priority)
            })
            .count()
    }

    pub fn clear(&mut self) {
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priority(value: u8) -> WmmUserPriority {
        WmmUserPriority::new(value).unwrap()
    }

    #[test]
    fn frames_leave_in_order_and_runs_stop_at_another_priority() {
        let mut queue = TxQueue::new();
        for (index, up) in [0, 0, 0, 6, 0].into_iter().enumerate() {
            assert!(queue.push(&[index as u8; 20], priority(up)));
        }
        assert_eq!(queue.head_run(8), 3);
        assert_eq!(queue.head_run(2), 2);
        assert_eq!(queue.pop().unwrap().ethernet(), &[0; 20]);
        assert_eq!(queue.head_run(8), 2);
        queue.pop();
        queue.pop();
        assert_eq!(queue.get(0).unwrap().priority, priority(6));
        assert_eq!(queue.head_run(8), 1);
        assert!(queue.get(1).is_some() && queue.get(2).is_none());
    }

    #[test]
    fn a_full_queue_refuses_until_a_frame_leaves() {
        let mut queue = TxQueue::new();
        for index in 0..PORT_TX_QUEUE {
            assert!(queue.push(&[index as u8; 14], priority(0)));
        }
        assert!(queue.is_full());
        assert!(!queue.push(&[0; 14], priority(0)));
        queue.pop();
        assert!(queue.push(&[9; 14], priority(0)));
        // The ring wraps: the newest frame is last.
        assert_eq!(queue.get(PORT_TX_QUEUE - 1).unwrap().ethernet(), &[9; 14]);
        queue.clear();
        assert!(queue.is_empty());
        assert!(!queue.push(&[0; PORT_FRAME_CAPACITY + 1], priority(0)));
    }
}
