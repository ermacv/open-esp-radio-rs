#![no_std]
#![forbid(unsafe_code)]

//! Durable Embassy wake handoff for ESP32-S31 Bluetooth controller work.
//!
//! The controller core owns the pending scheduler state. This adapter adds
//! only executor notification and an executor-neutral recheck rendezvous: an
//! interrupt publisher wakes the sole receiver on a fresh pending epoch, and
//! the receiver always registers its waker before rechecking durable state.

#[cfg(test)]
extern crate std;

use core::{
    future::{Future, poll_fn},
    task::Poll,
};

use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::RawMutex, waitqueue::GenericAtomicWaker};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothSchedulerWakeBatch, BluetoothSchedulerWakeCell, BluetoothSchedulerWakePublication,
    BluetoothSchedulerWorkerWakeClass,
};

/// Statically allocated scheduler wake state for one Bluetooth runtime epoch.
///
/// [`split`](Self::split) creates exactly one interrupt publisher and one async
/// receiver at a time. The mutable borrow prevents a second endpoint pair
/// while either endpoint from the current epoch is alive. A live interrupt to
/// task route must use a [`RawMutex`] implementation that synchronizes those
/// contexts; `NoopRawMutex` is suitable only for single-executor use and tests.
pub struct EmbassyBluetoothWakeResources<M: RawMutex> {
    scheduler: BluetoothSchedulerWakeCell,
    waker: GenericAtomicWaker<M>,
}

impl<M: RawMutex> EmbassyBluetoothWakeResources<M> {
    /// Construct an empty wake handoff.
    pub const fn new() -> Self {
        Self {
            scheduler: BluetoothSchedulerWakeCell::new(),
            waker: GenericAtomicWaker::new(M::INIT),
        }
    }

    /// Bind the sole interrupt publisher to the sole async receiver.
    ///
    /// The resource borrow makes a second live endpoint pair impossible:
    ///
    /// ```compile_fail
    /// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    /// use open_esp_radio_esp32s31_bluetooth_embassy::EmbassyBluetoothWakeResources;
    ///
    /// let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
    /// let first = resources.split();
    /// let second = resources.split();
    /// let _ = (first, second);
    /// ```
    pub fn split(
        &mut self,
    ) -> (
        EmbassyBluetoothIrqPublisher<'_, M>,
        EmbassyBluetoothWakeReceiver<'_, M>,
    ) {
        let resources = &*self;
        (
            EmbassyBluetoothIrqPublisher { resources },
            EmbassyBluetoothWakeReceiver { resources },
        )
    }
}

impl<M: RawMutex> Default for EmbassyBluetoothWakeResources<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Interrupt-side endpoint for publishing classified scheduler work.
pub struct EmbassyBluetoothIrqPublisher<'resources, M: RawMutex> {
    resources: &'resources EmbassyBluetoothWakeResources<M>,
}

impl<M: RawMutex> EmbassyBluetoothIrqPublisher<'_, M> {
    /// Publish one classified scheduler wake and notify the worker on a fresh
    /// pending epoch.
    ///
    /// Coalesced publications do not create redundant executor wakeups. The
    /// underlying pending/marker state remains the source of truth.
    pub fn publish_scheduler(
        &self,
        class: BluetoothSchedulerWorkerWakeClass,
    ) -> BluetoothSchedulerWakePublication {
        let publication = self.resources.scheduler.publish_from_interrupt(class);
        if publication == BluetoothSchedulerWakePublication::WakeWorker {
            self.resources.waker.wake();
        }
        publication
    }
}

/// Reason for resuming the sole Bluetooth controller worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum EmbassyBluetoothWake {
    /// One or more classified scheduler publications are pending.
    Scheduler(BluetoothSchedulerWakeBatch),
    /// The caller-supplied bounded recheck future completed.
    Recheck,
}

/// Async endpoint for the sole Bluetooth controller worker.
pub struct EmbassyBluetoothWakeReceiver<'resources, M: RawMutex> {
    resources: &'resources EmbassyBluetoothWakeResources<M>,
}

impl<M: RawMutex> EmbassyBluetoothWakeReceiver<'_, M> {
    /// Wait for the next durable scheduler batch.
    ///
    /// Waker registration deliberately precedes the state recheck. A
    /// concurrent publication is therefore either observed by `take` or
    /// delivers a wake to the registered task. Cancelling while pending does
    /// not remove scheduler state; a replacement waiter can consume it.
    pub async fn wait_scheduler(&mut self) -> BluetoothSchedulerWakeBatch {
        poll_fn(|context| {
            self.resources.waker.register(context.waker());
            match self.resources.scheduler.take() {
                Some(batch) => Poll::Ready(batch),
                None => Poll::Pending,
            }
        })
        .await
    }

    /// Whether classified scheduler work is currently pending.
    pub fn scheduler_pending(&self) -> bool {
        self.resources.scheduler.is_pending()
    }

    /// Wait for scheduler work or a caller-owned bounded recheck.
    ///
    /// Scheduler work wins when both inputs are ready in the same poll. A
    /// production caller should retain an absolute recheck deadline outside
    /// this future and rebuild the timer for that same deadline after a
    /// scheduler wake; repeatedly starting a relative delay can starve the
    /// controller-time recheck under sustained interrupt traffic.
    pub async fn wait_with_recheck<R>(&mut self, recheck: R) -> EmbassyBluetoothWake
    where
        R: Future<Output = ()>,
    {
        match select(self.wait_scheduler(), recheck).await {
            Either::First(batch) => EmbassyBluetoothWake::Scheduler(batch),
            Either::Second(()) => EmbassyBluetoothWake::Recheck,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::Pin,
        sync::atomic::{AtomicUsize, Ordering},
        task::{Context, Poll, Waker},
    };
    use std::{boxed::Box, sync::Arc, task::Wake};

    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::*;

    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn counting_waker() -> (Arc<WakeCounter>, Waker) {
        let counter = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&counter));
        (counter, waker)
    }

    fn poll_once<F: Future>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(waker))
    }

    #[test]
    fn publication_before_wait_is_rechecked_without_a_lost_wake() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();

        assert_eq!(
            publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        assert!(receiver.scheduler_pending());

        let batch = block_on(receiver.wait_scheduler());
        assert!(!batch.is_marked());
        assert!(!receiver.scheduler_pending());
    }

    #[test]
    fn pending_wait_is_woken_only_by_first_coalesced_epoch() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();
        let (counter, waker) = counting_waker();
        let mut wait = Box::pin(receiver.wait_scheduler());

        assert!(poll_once(wait.as_mut(), &waker).is_pending());
        assert_eq!(
            publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::Coalesced
        );
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);

        let Poll::Ready(batch) = poll_once(wait.as_mut(), &waker) else {
            panic!("published scheduler batch must be ready");
        };
        assert!(!batch.is_marked());
    }

    #[test]
    fn cancelled_wait_leaves_batch_for_replacement_waiter() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();
        let (_counter, waker) = counting_waker();
        let mut cancelled = Box::pin(receiver.wait_scheduler());

        assert!(poll_once(cancelled.as_mut(), &waker).is_pending());
        drop(cancelled);
        assert_eq!(
            publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::WakeWorker
        );

        let batch = block_on(receiver.wait_scheduler());
        assert!(!batch.is_marked());
        assert!(!receiver.scheduler_pending());
    }

    #[test]
    fn marked_duplicate_is_returned_in_the_same_batch() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();

        assert_eq!(
            publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        assert_eq!(
            publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Marked),
            BluetoothSchedulerWakePublication::Coalesced
        );

        assert!(block_on(receiver.wait_scheduler()).is_marked());
    }

    #[test]
    fn scheduler_wins_a_ready_tie_with_external_recheck() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();

        publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Ordinary);

        assert!(matches!(
            block_on(receiver.wait_with_recheck(ready(()))),
            EmbassyBluetoothWake::Scheduler(batch) if !batch.is_marked()
        ));
    }

    #[test]
    fn external_recheck_does_not_consume_a_later_scheduler_batch() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();

        assert_eq!(
            block_on(receiver.wait_with_recheck(ready(()))),
            EmbassyBluetoothWake::Recheck
        );
        assert_eq!(
            publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Marked),
            BluetoothSchedulerWakePublication::WakeWorker
        );

        assert!(block_on(receiver.wait_scheduler()).is_marked());
    }
}
