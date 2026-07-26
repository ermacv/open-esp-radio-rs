use core::ffi::c_void;

use crate::{
    diagnostics::{BlockingCall, BlockingCallProbe},
    event::PpEvent,
    queue::RadioQueue,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawQueueError {
    NullItem,
    Full(PpEvent),
}

/// Bridge used by the OSI queue callbacks servicing `pp_post`.
///
/// All send variants are immediate `try_push` operations, irrespective of the
/// timeout requested by the synchronous vendor ABI. Queue receive is forbidden
/// because the Rust `RadioFuture` owns the consumer side.
pub struct OsiPpQueue<'a, const N: usize> {
    queue: &'a RadioQueue<N>,
    blocking_probe: &'a BlockingCallProbe,
}

impl<'a, const N: usize> OsiPpQueue<'a, N> {
    pub const fn new(queue: &'a RadioQueue<N>, blocking_probe: &'a BlockingCallProbe) -> Self {
        Self {
            queue,
            blocking_probe,
        }
    }

    /// Copy an eight-byte PP queue item into the Rust radio queue.
    ///
    /// # Safety
    /// `item` must point to a readable ESP32-S31 `PpEvent` supplied by the
    /// vendor library. The pointed-to memory only needs to remain valid for the
    /// duration of this call.
    pub unsafe fn try_send_raw(&self, item: *const c_void) -> Result<(), RawQueueError> {
        let Some(item) = item.cast::<PpEvent>().as_ref() else {
            return Err(RawQueueError::NullItem);
        };
        let event = *item;
        self.queue
            .try_push(event)
            .map_err(|error| RawQueueError::Full(error.0))
    }

    /// Return convention expected by `_queue_send`, `_queue_send_to_back`,
    /// `_queue_send_to_front`, and `_queue_send_from_isr`.
    ///
    /// # Safety
    /// Same requirements as [`Self::try_send_raw`].
    pub unsafe fn send_osi(&self, item: *const c_void) -> i32 {
        i32::from(self.try_send_raw(item).is_ok())
    }

    pub fn messages_waiting(&self) -> u32 {
        self.queue.len().min(u32::MAX as usize) as u32
    }

    /// Diagnostic implementation for `_queue_recv`. It never blocks and
    /// records the violation for the on-device reachability audit.
    pub fn reject_receive(&self, timeout_ticks: u32, current_event: u32) -> i32 {
        self.blocking_probe.record(
            BlockingCall::QueueReceive,
            current_event,
            timeout_ticks as usize,
        );
        0
    }
}

#[cfg(test)]
mod tests {
    use core::ffi::c_void;

    use super::OsiPpQueue;
    use crate::{diagnostics::BlockingCallProbe, event::PpEvent, queue::RadioQueue};

    #[test]
    fn raw_send_copies_stack_item() {
        let queue = RadioQueue::<1>::new();
        let probe = BlockingCallProbe::new();
        let bridge = OsiPpQueue::new(&queue, &probe);
        let event = PpEvent {
            kind: 8,
            argument: core::ptr::null_mut(),
        };

        assert_eq!(
            unsafe { bridge.send_osi((&event as *const PpEvent).cast::<c_void>()) },
            1
        );
        assert_eq!(bridge.messages_waiting(), 1);
        assert_eq!(queue.try_pop(), Some(event));
    }
}
