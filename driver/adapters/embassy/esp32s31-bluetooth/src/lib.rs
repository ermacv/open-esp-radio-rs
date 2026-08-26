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
    BluetoothModemLpTimerEventCell, BluetoothModemLpTimerEventPublication,
    BluetoothModemLpTimerExpiration, BluetoothModemLpTimerExpirationPending,
    BluetoothModemLpTimerSoftwareWork, BluetoothPrimaryControllerFault,
    BluetoothPrimaryInterruptStep, BluetoothPrimaryNoSchedulerWork,
    BluetoothPrimaryReferenceRecoveryRequired, BluetoothPrimarySchedulerEvent,
    BluetoothSchedulerLockModifyEvent, BluetoothSchedulerLockModifyEventCell,
    BluetoothSchedulerLockModifyEventPublication, BluetoothSchedulerLockModifyInterruptObservation,
    BluetoothSchedulerLockModifyWorker, BluetoothSchedulerLockModifyWorkerStep,
    BluetoothSchedulerWakeBatch, BluetoothSchedulerWakeCell, BluetoothSchedulerWakePublication,
    BluetoothSchedulerWorkerWakeClass, step_primary_interrupt,
};
use open_esp_radio_esp32s31_hal::{BluetoothControllerHal, BluetoothInterruptRegistersOwner};

/// Statically allocated scheduler wake state for one Bluetooth runtime epoch.
///
/// [`split`](Self::split) creates exactly one interrupt publisher and one async
/// receiver at a time. The mutable borrow prevents a second endpoint pair
/// while either endpoint from the current epoch is alive. A live interrupt to
/// task route must use a [`RawMutex`] implementation that synchronizes those
/// contexts; `NoopRawMutex` is suitable only for single-executor use and tests.
pub struct EmbassyBluetoothWakeResources<M: RawMutex> {
    scheduler: BluetoothSchedulerWakeCell,
    scheduler_waker: GenericAtomicWaker<M>,
    lock_modify: BluetoothSchedulerLockModifyEventCell,
    lock_modify_waker: GenericAtomicWaker<M>,
    timer_event: BluetoothModemLpTimerEventCell,
    timer_event_waker: GenericAtomicWaker<M>,
}

impl<M: RawMutex> EmbassyBluetoothWakeResources<M> {
    /// Construct an empty wake handoff.
    pub const fn new() -> Self {
        Self {
            scheduler: BluetoothSchedulerWakeCell::new(),
            scheduler_waker: GenericAtomicWaker::new(M::INIT),
            lock_modify: BluetoothSchedulerLockModifyEventCell::new(),
            lock_modify_waker: GenericAtomicWaker::new(M::INIT),
            timer_event: BluetoothModemLpTimerEventCell::new(),
            timer_event_waker: GenericAtomicWaker::new(M::INIT),
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
    /// Publish one due source-127 timer event through its proof gate.
    ///
    /// Backpressure returns the complete pending owner unchanged. Only a
    /// successful durable publication recovers the software work required to
    /// continue toward final hardware rearm.
    pub fn publish_timer_expiration<'queue, const CAPACITY: usize>(
        &self,
        pending: BluetoothModemLpTimerExpirationPending<'queue, CAPACITY>,
    ) -> Result<
        BluetoothModemLpTimerSoftwareWork<'queue, CAPACITY>,
        BluetoothModemLpTimerExpirationPending<'queue, CAPACITY>,
    > {
        match pending.publish(&self.resources.timer_event) {
            Ok((work, BluetoothModemLpTimerEventPublication::WakeWorker)) => {
                self.resources.timer_event_waker.wake();
                Ok(work)
            }
            Err(pending) => Err(pending),
        }
    }

    /// Execute one complete bounded primary source-124 step and publish both
    /// scheduler handoffs when the classifier reaches ordinary work.
    ///
    /// The scheduler wake and lock/modify BUSY value come from the same later
    /// scheduler-state observation. Fault and selector-6 recovery paths do not
    /// publish ordinary work.
    pub fn capture_and_publish_primary(
        &self,
        interrupts: &mut BluetoothInterruptRegistersOwner,
    ) -> EmbassyBluetoothPrimaryInterruptStep {
        match step_primary_interrupt(interrupts) {
            BluetoothPrimaryInterruptStep::Fault(fault) => {
                EmbassyBluetoothPrimaryInterruptStep::Fault(fault)
            }
            BluetoothPrimaryInterruptStep::NoSchedulerWork(epoch) => {
                EmbassyBluetoothPrimaryInterruptStep::NoSchedulerWork(epoch)
            }
            BluetoothPrimaryInterruptStep::ReferenceRecoveryRequired(required) => {
                EmbassyBluetoothPrimaryInterruptStep::ReferenceRecoveryRequired(required)
            }
            BluetoothPrimaryInterruptStep::Scheduler(event) => {
                let scheduler = self.publish_scheduler(event.wake().class());
                let lock_modify = self.publish_lock_modify(event.lock_modify_observation());
                EmbassyBluetoothPrimaryInterruptStep::Scheduler {
                    event,
                    scheduler,
                    lock_modify,
                }
            }
        }
    }

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
            self.resources.scheduler_waker.wake();
        }
        publication
    }

    /// Publish one scheduler lock/modify BUSY observation and wake its worker
    /// only when this opens a fresh pending epoch.
    pub fn publish_lock_modify(
        &self,
        observation: BluetoothSchedulerLockModifyInterruptObservation,
    ) -> BluetoothSchedulerLockModifyEventPublication {
        let publication = self
            .resources
            .lock_modify
            .publish_from_interrupt(observation);
        if publication == BluetoothSchedulerLockModifyEventPublication::WakeWorker {
            self.resources.lock_modify_waker.wake();
        }
        publication
    }

    /// Capture BUSY through the unique interrupt-register owner and publish it
    /// to the lock/modify worker in one bounded ISR-side call.
    pub fn capture_and_publish_lock_modify(
        &self,
        interrupts: &mut BluetoothInterruptRegistersOwner,
    ) -> BluetoothSchedulerLockModifyEventPublication {
        self.publish_lock_modify(interrupts.capture_scheduler_lock_modify_interrupt())
    }
}

/// Published disposition of one bounded primary source-124 IRQ step.
#[must_use = "fault, recovery or scheduler publication must reach the controller owner"]
pub enum EmbassyBluetoothPrimaryInterruptStep {
    /// A baseline fault was retained without publishing ordinary work.
    Fault(BluetoothPrimaryControllerFault),
    /// No reviewed dynamic scheduler source was present.
    NoSchedulerWork(BluetoothPrimaryNoSchedulerWork),
    /// The missing selector-6 invariant prevents further IRQ progression.
    ReferenceRecoveryRequired(BluetoothPrimaryReferenceRecoveryRequired),
    /// Both durable task handoffs were updated from one scheduler event.
    Scheduler {
        /// Lossless primary classification and scheduler-state projection.
        event: BluetoothPrimarySchedulerEvent,
        /// Coalescing result for the general scheduler worker.
        scheduler: BluetoothSchedulerWakePublication,
        /// Coalescing result for the lock/modify worker.
        lock_modify: BluetoothSchedulerLockModifyEventPublication,
    },
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
    /// Wait for one durably published source-127 timer expiration.
    ///
    /// Register-before-recheck closes the publication race. Cancelling this
    /// future leaves the event in its atomic cell, which also keeps the source
    /// software path backpressured until a replacement receiver consumes it.
    pub async fn wait_timer_expiration(&mut self) -> BluetoothModemLpTimerExpiration {
        poll_fn(|context| {
            self.resources.timer_event_waker.register(context.waker());
            match self.resources.timer_event.take() {
                Some(event) => Poll::Ready(event),
                None => Poll::Pending,
            }
        })
        .await
    }

    /// Whether one source-127 timer expiration is awaiting the receiver.
    pub fn timer_expiration_pending(&self) -> bool {
        self.resources.timer_event.is_pending()
    }

    /// Wait for the next durable scheduler batch.
    ///
    /// Waker registration deliberately precedes the state recheck. A
    /// concurrent publication is therefore either observed by `take` or
    /// delivers a wake to the registered task. Cancelling while pending does
    /// not remove scheduler state; a replacement waiter can consume it.
    pub async fn wait_scheduler(&mut self) -> BluetoothSchedulerWakeBatch {
        poll_fn(|context| {
            self.resources.scheduler_waker.register(context.waker());
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

    /// Wait for one durable scheduler lock/modify event.
    ///
    /// Registration precedes the atomic recheck, and cancellation never
    /// removes the event cell. A replacement waiter therefore observes either
    /// the original pending value or a newer coalesced BUSY value.
    pub async fn wait_lock_modify(&mut self) -> BluetoothSchedulerLockModifyEvent {
        poll_fn(|context| {
            self.resources.lock_modify_waker.register(context.waker());
            match self.resources.lock_modify.take() {
                Some(event) => Poll::Ready(event),
                None => Poll::Pending,
            }
        })
        .await
    }

    /// Whether a lock/modify event is currently pending.
    pub fn lock_modify_pending(&self) -> bool {
        self.resources.lock_modify.is_pending()
    }

    /// Await one lock/modify interrupt event and perform exactly one durable
    /// controller-worker step.
    ///
    /// Cancellation while waiting leaves the event cell and worker unchanged.
    /// Once the event is returned, the worker step is synchronous and bounded:
    /// it performs one task observation and at most one finite publication.
    pub async fn wait_and_step_lock_modify(
        &mut self,
        worker: &mut BluetoothSchedulerLockModifyWorker,
        controller: &mut BluetoothControllerHal<'_>,
    ) -> BluetoothSchedulerLockModifyWorkerStep {
        let event = self.wait_lock_modify().await;
        worker.step(event, controller)
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
    fn lock_modify_publication_before_wait_is_rechecked_without_loss() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();

        assert_eq!(
            publisher.publish_lock_modify(
                BluetoothSchedulerLockModifyInterruptObservation::from_busy(true)
            ),
            BluetoothSchedulerLockModifyEventPublication::WakeWorker
        );
        assert!(receiver.lock_modify_pending());
        assert!(block_on(receiver.wait_lock_modify()).is_busy());
        assert!(!receiver.lock_modify_pending());
    }

    #[test]
    fn lock_modify_wait_wakes_once_and_consumes_the_latest_value() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();
        let (counter, waker) = counting_waker();
        let mut wait = Box::pin(receiver.wait_lock_modify());

        assert!(poll_once(wait.as_mut(), &waker).is_pending());
        assert_eq!(
            publisher.publish_lock_modify(
                BluetoothSchedulerLockModifyInterruptObservation::from_busy(true)
            ),
            BluetoothSchedulerLockModifyEventPublication::WakeWorker
        );
        assert_eq!(
            publisher.publish_lock_modify(
                BluetoothSchedulerLockModifyInterruptObservation::from_busy(false)
            ),
            BluetoothSchedulerLockModifyEventPublication::Coalesced
        );
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);

        let Poll::Ready(event) = poll_once(wait.as_mut(), &waker) else {
            panic!("latest lock/modify event must be ready");
        };
        assert!(!event.is_busy());
    }

    #[test]
    fn cancelled_lock_modify_wait_retains_the_event_for_replacement() {
        let mut resources = EmbassyBluetoothWakeResources::<NoopRawMutex>::new();
        let (publisher, mut receiver) = resources.split();
        let (_counter, waker) = counting_waker();
        let mut cancelled = Box::pin(receiver.wait_lock_modify());

        assert!(poll_once(cancelled.as_mut(), &waker).is_pending());
        drop(cancelled);
        publisher.publish_lock_modify(BluetoothSchedulerLockModifyInterruptObservation::from_busy(
            false,
        ));

        assert!(!block_on(receiver.wait_lock_modify()).is_busy());
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
