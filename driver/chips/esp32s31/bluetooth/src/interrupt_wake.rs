//! Lock-free handoff from the bounded Bluetooth hard handler to one worker.
//!
//! The recovered deferred-work path owns one static scheduler event. Repeated
//! queue insertion is coalesced, while its one-bit marker remains sticky until
//! the worker dequeues the event. This cell preserves exactly that contract
//! without an RTOS queue, allocation, or a callback-list ABI.
//!
//! Waker storage is deliberately outside this type. A platform integration
//! must register its worker waker before rechecking
//! [`BluetoothSchedulerWakeCell::take`] and must deliver every
//! [`BluetoothSchedulerWakePublication::WakeWorker`] result.

#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU8, Ordering};

use crate::BluetoothSchedulerWorkerWakeClass;

const PENDING: u8 = 1 << 0;
const MARKED: u8 = 1 << 1;

/// Result of publishing one classified scheduler wake from interrupt context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerWakePublication {
    /// This publication created a fresh pending epoch; wake the sole worker.
    WakeWorker,
    /// A pending epoch already exists and now covers this publication.
    Coalesced,
}

/// One coalesced scheduler-work batch consumed by the sole worker.
///
/// The batch is affine because one dequeued hardware-work notification may
/// authorize at most one fresh finished-list transfer:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth::BluetoothSchedulerWakeBatch;
///
/// fn replay(batch: BluetoothSchedulerWakeBatch) {
///     consume(batch);
///     consume(batch);
/// }
///
/// fn consume(_batch: BluetoothSchedulerWakeBatch) {}
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerWakeBatch {
    marked: bool,
}

impl BluetoothSchedulerWakeBatch {
    /// Whether any publication in this coalesced batch carried the marker.
    pub const fn is_marked(&self) -> bool {
        self.marked
    }
}

/// Atomic pending/marker state shared by the hard handler and async worker.
///
/// There is intentionally no count. The reference worker drains scheduler
/// state rather than executing once per interrupt, and the public OSAL drops
/// duplicate insertion of its same static event. The marker is accumulated by
/// OR, so an ordinary publication can never clear an earlier marked one.
pub struct BluetoothSchedulerWakeCell {
    state: AtomicU8,
}

impl BluetoothSchedulerWakeCell {
    /// Construct an empty handoff cell.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
        }
    }

    /// Publish one scheduler-work classification from the hard handler.
    ///
    /// This method is finite, lock-free, allocation-free, and performs no
    /// MMIO. The caller remains responsible for waking the worker when the
    /// return value is [`BluetoothSchedulerWakePublication::WakeWorker`].
    pub fn publish_from_interrupt(
        &self,
        class: BluetoothSchedulerWorkerWakeClass,
    ) -> BluetoothSchedulerWakePublication {
        let publication = PENDING
            | match class {
                BluetoothSchedulerWorkerWakeClass::Ordinary => 0,
                BluetoothSchedulerWorkerWakeClass::Marked => MARKED,
            };
        let previous = self.state.fetch_or(publication, Ordering::AcqRel);

        if previous & PENDING == 0 {
            BluetoothSchedulerWakePublication::WakeWorker
        } else {
            BluetoothSchedulerWakePublication::Coalesced
        }
    }

    /// Atomically dequeue the current coalesced batch.
    ///
    /// A publication racing after the swap creates a distinct pending epoch
    /// and returns `WakeWorker`, so it cannot be consumed by this batch without
    /// an accompanying wake edge.
    pub fn take(&self) -> Option<BluetoothSchedulerWakeBatch> {
        let state = self.state.swap(0, Ordering::AcqRel);
        if state & PENDING == 0 {
            None
        } else {
            Some(BluetoothSchedulerWakeBatch {
                marked: state & MARKED != 0,
            })
        }
    }

    /// Whether a worker batch is currently pending.
    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) & PENDING != 0
    }
}

impl Default for BluetoothSchedulerWakeCell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothSchedulerWakeCell, BluetoothSchedulerWakePublication,
        BluetoothSchedulerWorkerWakeClass,
    };

    #[test]
    fn first_publication_wakes_and_ordinary_duplicates_coalesce() {
        let cell = BluetoothSchedulerWakeCell::new();

        assert_eq!(
            cell.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        assert_eq!(
            cell.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::Coalesced
        );
        assert!(cell.is_pending());
        assert!(!cell.take().expect("one batch must be pending").is_marked());
        assert!(!cell.is_pending());
    }

    #[test]
    fn marker_is_sticky_for_both_publication_orders() {
        for classes in [
            [
                BluetoothSchedulerWorkerWakeClass::Ordinary,
                BluetoothSchedulerWorkerWakeClass::Marked,
            ],
            [
                BluetoothSchedulerWorkerWakeClass::Marked,
                BluetoothSchedulerWorkerWakeClass::Ordinary,
            ],
        ] {
            let cell = BluetoothSchedulerWakeCell::new();
            assert_eq!(
                cell.publish_from_interrupt(classes[0]),
                BluetoothSchedulerWakePublication::WakeWorker
            );
            assert_eq!(
                cell.publish_from_interrupt(classes[1]),
                BluetoothSchedulerWakePublication::Coalesced
            );
            assert!(cell.take().expect("one batch must be pending").is_marked());
        }
    }

    #[test]
    fn dequeue_closes_the_epoch_and_the_next_publication_wakes_again() {
        let cell = BluetoothSchedulerWakeCell::new();
        assert_eq!(
            cell.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Marked),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        assert!(
            cell.take()
                .expect("first batch must be pending")
                .is_marked()
        );
        assert_eq!(cell.take(), None);

        assert_eq!(
            cell.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        assert!(
            !cell
                .take()
                .expect("second batch must be pending")
                .is_marked()
        );
    }
}
