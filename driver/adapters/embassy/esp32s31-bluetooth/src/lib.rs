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

mod controller_command_task;
#[cfg(any(test, target_arch = "riscv32"))]
mod controller_time_recheck;
#[cfg(any(target_arch = "riscv32", test))]
mod dtm_active;
#[cfg(target_arch = "riscv32")]
mod dtm_first;
mod dtm_session_task;
#[cfg(any(target_arch = "riscv32", test))]
mod dtm_stopping;
mod modem_timer_task;

#[cfg(target_arch = "riscv32")]
pub use dtm_active::EmbassyBluetoothDtmActiveWait;
#[cfg(any(target_arch = "riscv32", test))]
pub use dtm_active::{
    EmbassyBluetoothDtmActiveCommandSignal, EmbassyBluetoothDtmActivePendingSignal,
    EmbassyBluetoothDtmActiveRadioSignal, EmbassyBluetoothDtmActiveWaitError,
};

#[cfg(target_arch = "riscv32")]
pub use dtm_first::{
    EmbassyBluetoothDtmFirstControllerTimeWait, EmbassyBluetoothDtmFirstDrive,
    EmbassyBluetoothDtmFirstResume, drive_dtm_first_ready,
};

pub use dtm_session_task::{
    EmbassyBluetoothDtmControllerTimeRecheck, EmbassyBluetoothDtmControllerTimeRecheckStatus,
    EmbassyBluetoothDtmSessionRetry,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_session_task::{EmbassyBluetoothDtmSessionBoundary, EmbassyBluetoothDtmSessionTask};

#[cfg(target_arch = "riscv32")]
pub use controller_command_task::{
    EmbassyBluetoothControllerCommandBoundary, EmbassyBluetoothControllerCommandTask,
};
pub use controller_command_task::{
    EmbassyBluetoothControllerCommandPhase, EmbassyBluetoothControllerIdleCompletion,
    EmbassyBluetoothControllerRetry,
};
#[cfg(target_arch = "riscv32")]
pub use controller_time_recheck::{
    EmbassyBluetoothDtmAbsoluteRecheck, EmbassyBluetoothDtmAbsoluteRecheckWait,
    EmbassyBluetoothDtmRecheckDeadline, EmbassyBluetoothDtmRecheckPeriod,
    EmbassyBluetoothDtmRecheckPeriodError, EmbassyBluetoothDtmRecheckScheduleState,
    EmbassyBluetoothDtmRecheckStartError,
};

#[cfg(any(target_arch = "riscv32", test))]
pub use dtm_stopping::{
    EmbassyBluetoothDtmStoppingSignal, EmbassyBluetoothDtmTestEndResponseSignal,
    EmbassyBluetoothDtmTestEndResponseWaitError,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_stopping::{EmbassyBluetoothDtmStoppingWait, EmbassyBluetoothDtmTestEndResponseWait};

pub use modem_timer_task::EmbassyBluetoothModemTimerWakers;
#[cfg(target_arch = "riscv32")]
pub use modem_timer_task::{EmbassyBluetoothModemTimerDriveStep, EmbassyBluetoothModemTimerDriver};

use core::{
    future::{Future, poll_fn},
    task::Poll,
};

use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::RawMutex, waitqueue::GenericAtomicWaker};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothControllerInterruptRuntime, BluetoothControllerModemTimerRuntime,
    BluetoothControllerTaskRuntime, BluetoothDtmPostUnlinkMailboxPublication,
    BluetoothDtmPostUnlinkWakeCell, BluetoothModemLpTimerPublishedInterruptStep,
    BluetoothNrtDefaultInterruptEpoch, BluetoothPrimaryControllerFault,
    BluetoothPrimaryInterruptStep, BluetoothPrimaryNoSchedulerWork,
    BluetoothPrimaryOrdinaryPublication, BluetoothPrimarySchedulerEvent,
    BluetoothPrimarySerializedServiceStep, BluetoothSchedulerLockModifyEvent,
    BluetoothSchedulerLockModifyEventPublication, BluetoothSchedulerLockModifyInterruptObservation,
    BluetoothSchedulerLockModifyWorkerStep, BluetoothSchedulerWakeBatch,
    BluetoothSchedulerWakeCell, BluetoothSchedulerWakePublication,
    BluetoothSchedulerWorkerWakeClass, step_nrt_default_interrupt, step_primary_interrupt,
};
use open_esp_radio_esp32s31_hal::{BluetoothControllerHal, BluetoothInterruptRegistersOwner};

fn poll_borrowed_ready<M: RawMutex>(
    waker: &GenericAtomicWaker<M>,
    context: &mut core::task::Context<'_>,
    is_pending: impl FnOnce() -> bool,
) -> Poll<()> {
    waker.register(context.waker());
    if is_pending() {
        Poll::Ready(())
    } else {
        Poll::Pending
    }
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EmbassyBluetoothPostUnlinkSignal {
    Mailbox,
    Recheck,
}

#[cfg(any(target_arch = "riscv32", test))]
async fn select_post_unlink_first<M, R>(mailbox: M, recheck: R) -> EmbassyBluetoothPostUnlinkSignal
where
    M: Future<Output = ()>,
    R: Future<Output = ()>,
{
    match select(mailbox, recheck).await {
        Either::First(()) => EmbassyBluetoothPostUnlinkSignal::Mailbox,
        Either::Second(()) => EmbassyBluetoothPostUnlinkSignal::Recheck,
    }
}

/// Embassy wakers for one borrowed Bluetooth Controller runtime epoch.
///
/// This adapter owns no pending state or Controller worker. [`split`](Self::split)
/// must borrow the core [`BluetoothControllerRuntimeResources`] that is bound
/// to the hardware epoch. A live interrupt-to-task route must use a
/// [`RawMutex`] implementation that synchronizes those contexts;
/// `NoopRawMutex` is suitable only for single-executor use and tests.
pub struct EmbassyBluetoothRuntimeWakers<M: RawMutex> {
    scheduler_waker: GenericAtomicWaker<M>,
    lock_modify_waker: GenericAtomicWaker<M>,
    post_unlink_waker: GenericAtomicWaker<M>,
    modem_timer: EmbassyBluetoothModemTimerWakers<M>,
}

impl<M: RawMutex> EmbassyBluetoothRuntimeWakers<M> {
    /// Construct executor notification state without a duplicate event cell.
    pub const fn new() -> Self {
        Self {
            scheduler_waker: GenericAtomicWaker::new(M::INIT),
            lock_modify_waker: GenericAtomicWaker::new(M::INIT),
            post_unlink_waker: GenericAtomicWaker::new(M::INIT),
            modem_timer: EmbassyBluetoothModemTimerWakers::new(),
        }
    }

    /// Notification and finite borrowed driver boundary for source 127.
    pub const fn modem_timer(&self) -> &EmbassyBluetoothModemTimerWakers<M> {
        &self.modem_timer
    }

    /// Route one exact source-127 service result to its borrowed task wait.
    pub fn notify_modem_timer_service(
        &self,
        step: BluetoothModemLpTimerPublishedInterruptStep,
    ) -> BluetoothModemLpTimerPublishedInterruptStep {
        self.modem_timer.notify_modem_timer_service(step)
    }

    /// Wait until the borrowed scheduler cell contains durable work.
    ///
    /// Unlike [`EmbassyBluetoothWakeReceiver::wait_scheduler`], this future
    /// owns neither the task runtime nor the scheduler batch. It can therefore
    /// be selected beside HCI capacity while an affine DTM session remains in
    /// the caller. Successful completion is only a readiness hint; the core
    /// session transition remains responsible for consuming the exact batch.
    pub async fn wait_scheduler_ready(&self, wake: &BluetoothSchedulerWakeCell) {
        poll_fn(|context| poll_borrowed_ready(&self.scheduler_waker, context, || wake.is_pending()))
            .await
    }

    /// Whether the borrowed scheduler cell contains durable work.
    pub fn scheduler_pending(&self, wake: &BluetoothSchedulerWakeCell) -> bool {
        wake.is_pending()
    }

    fn notify_post_unlink(
        &self,
        publication: BluetoothDtmPostUnlinkMailboxPublication,
    ) -> BluetoothDtmPostUnlinkMailboxPublication {
        if publication == BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer {
            self.post_unlink_waker.wake();
        }
        publication
    }

    fn notify_ordinary(
        &self,
        publication: BluetoothPrimaryOrdinaryPublication,
    ) -> BluetoothPrimaryOrdinaryPublication {
        if let BluetoothPrimaryOrdinaryPublication::Scheduler {
            scheduler,
            lock_modify,
        } = publication
        {
            if scheduler == BluetoothSchedulerWakePublication::WakeWorker {
                self.scheduler_waker.wake();
            }
            if lock_modify == BluetoothSchedulerLockModifyEventPublication::WakeWorker {
                self.lock_modify_waker.wake();
            }
        }
        publication
    }

    /// Deliver every executor notification carried by one exact serialized
    /// primary-service result.
    ///
    /// Ordinary scheduler and lock/modify publications are notified for all
    /// variants. Both the first stored post-unlink event and a full mailbox are
    /// additionally routed through the same coalescing mailbox notifier.
    pub fn notify_primary_service(
        &self,
        step: &BluetoothPrimarySerializedServiceStep,
    ) -> Option<BluetoothDtmPostUnlinkMailboxPublication> {
        let (ordinary, mailbox) = match step {
            BluetoothPrimarySerializedServiceStep::General { ordinary, .. } => (*ordinary, None),
            BluetoothPrimarySerializedServiceStep::DtmStored {
                mailbox, ordinary, ..
            }
            | BluetoothPrimarySerializedServiceStep::MailboxFull {
                mailbox, ordinary, ..
            } => (*ordinary, Some(*mailbox)),
        };
        self.notify_ordinary(ordinary);
        mailbox.map(|publication| self.notify_post_unlink(publication))
    }

    /// Wait until the Controller-owned post-unlink mailbox becomes ready.
    ///
    /// The executor waker is registered before the durable lower-cell recheck.
    /// This future cannot close the wake epoch, so cancellation cannot discard
    /// readiness; only successful mailbox consumption performs that transition.
    pub async fn wait_post_unlink_ready(&self, wake: &BluetoothDtmPostUnlinkWakeCell) {
        poll_fn(|context| {
            poll_borrowed_ready(&self.post_unlink_waker, context, || wake.is_pending())
        })
        .await
    }

    /// Wait for either a durable post-unlink publication or the caller's
    /// already-anchored absolute recheck deadline.
    ///
    /// Mailbox readiness is the first select operand and therefore wins a
    /// simultaneous-ready tie. Cancelling this borrowed wait consumes neither
    /// source.
    #[cfg(target_arch = "riscv32")]
    async fn wait_post_unlink_or_recheck<R>(
        &self,
        wake: &BluetoothDtmPostUnlinkWakeCell,
        recheck: R,
    ) -> EmbassyBluetoothPostUnlinkSignal
    where
        R: Future<Output = ()>,
    {
        select_post_unlink_first(self.wait_post_unlink_ready(wake), recheck).await
    }

    /// Whether the post-unlink consumer has durable ready work.
    pub fn post_unlink_pending(&self, wake: &BluetoothDtmPostUnlinkWakeCell) -> bool {
        wake.is_pending()
    }

    /// Bind Embassy notification to the sole core interrupt/task endpoint pair.
    ///
    /// The resource borrow makes a second live endpoint pair impossible:
    ///
    /// ```compile_fail
    /// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    /// use open_esp_radio_esp32s31_bluetooth::BluetoothControllerRuntimeResources;
    /// use open_esp_radio_esp32s31_bluetooth_embassy::EmbassyBluetoothRuntimeWakers;
    ///
    /// let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
    /// let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
    /// let first = wakers.split(runtime.split());
    /// let second = wakers.split(runtime.split());
    /// let _ = (first, second);
    /// ```
    pub fn split<'resources, const MODEM_TIMER_CAPACITY: usize>(
        &'resources mut self,
        runtime: (
            BluetoothControllerInterruptRuntime<'resources>,
            BluetoothControllerTaskRuntime<'resources>,
            BluetoothControllerModemTimerRuntime<'resources, MODEM_TIMER_CAPACITY>,
        ),
    ) -> (
        EmbassyBluetoothIrqPublisher<'resources, M>,
        EmbassyBluetoothWakeReceiver<'resources, M>,
        BluetoothControllerModemTimerRuntime<'resources, MODEM_TIMER_CAPACITY>,
    ) {
        let wakers = &*self;
        let (runtime, task, modem_timer) = runtime;
        (
            EmbassyBluetoothIrqPublisher { runtime, wakers },
            EmbassyBluetoothWakeReceiver {
                runtime: task,
                wakers,
            },
            modem_timer,
        )
    }
}

impl<M: RawMutex> Default for EmbassyBluetoothRuntimeWakers<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Interrupt-side endpoint for publishing classified scheduler work.
pub struct EmbassyBluetoothIrqPublisher<'resources, M: RawMutex> {
    runtime: BluetoothControllerInterruptRuntime<'resources>,
    wakers: &'resources EmbassyBluetoothRuntimeWakers<M>,
}

impl<M: RawMutex> EmbassyBluetoothIrqPublisher<'_, M> {
    /// Execute one complete bounded primary source-124 step and publish both
    /// scheduler handoffs when the classifier reaches ordinary work.
    ///
    /// The scheduler wake and lock/modify BUSY value come from the same later
    /// scheduler-state observation. Fault paths do not publish ordinary work.
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

    /// Capture and acknowledge one default-profile NRT source-133 epoch.
    ///
    /// The pinned default profile has no NRT software consumer, so this path
    /// intentionally wakes neither scheduler nor async worker. The returned
    /// opaque epoch remains available for diagnostics and future explicit
    /// feature policy.
    pub fn capture_nrt_default(
        &self,
        interrupts: &mut BluetoothInterruptRegistersOwner,
    ) -> BluetoothNrtDefaultInterruptEpoch {
        step_nrt_default_interrupt(interrupts)
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
        let publication = self.runtime.scheduler_wake().publish_from_interrupt(class);
        if publication == BluetoothSchedulerWakePublication::WakeWorker {
            self.wakers.scheduler_waker.wake();
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
            .runtime
            .scheduler_lock_modify_events()
            .publish_from_interrupt(observation);
        if publication == BluetoothSchedulerLockModifyEventPublication::WakeWorker {
            self.wakers.lock_modify_waker.wake();
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
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub enum EmbassyBluetoothWake {
    /// One or more classified scheduler publications are pending.
    Scheduler(BluetoothSchedulerWakeBatch),
    /// The caller-supplied bounded recheck future completed.
    Recheck,
}

/// Async endpoint for the sole Bluetooth controller worker.
pub struct EmbassyBluetoothWakeReceiver<'resources, M: RawMutex> {
    runtime: BluetoothControllerTaskRuntime<'resources>,
    wakers: &'resources EmbassyBluetoothRuntimeWakers<M>,
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
            self.wakers.scheduler_waker.register(context.waker());
            match self.runtime.scheduler_wake().take() {
                Some(batch) => Poll::Ready(batch),
                None => Poll::Pending,
            }
        })
        .await
    }

    /// Whether classified scheduler work is currently pending.
    pub fn scheduler_pending(&self) -> bool {
        self.runtime.scheduler_wake().is_pending()
    }

    /// Wait for one durable scheduler lock/modify event.
    ///
    /// Registration precedes the atomic recheck, and cancellation never
    /// removes the event cell. A replacement waiter therefore observes either
    /// the original pending value or a newer coalesced BUSY value.
    pub async fn wait_lock_modify(&mut self) -> BluetoothSchedulerLockModifyEvent {
        poll_fn(|context| {
            self.wakers.lock_modify_waker.register(context.waker());
            match self.runtime.scheduler_lock_modify_events().take() {
                Some(event) => Poll::Ready(event),
                None => Poll::Pending,
            }
        })
        .await
    }

    /// Whether a lock/modify event is currently pending.
    pub fn lock_modify_pending(&self) -> bool {
        self.runtime.scheduler_lock_modify_events().is_pending()
    }

    /// Await one lock/modify interrupt event and perform exactly one durable
    /// controller-worker step.
    ///
    /// Cancellation while waiting leaves the event cell and worker unchanged.
    /// Once the event is returned, the worker step is synchronous and bounded:
    /// it performs one task observation and at most one finite publication.
    pub async fn wait_and_step_lock_modify(
        &mut self,
        controller: &mut BluetoothControllerHal<'_>,
    ) -> BluetoothSchedulerLockModifyWorkerStep {
        let event = self.wait_lock_modify().await;
        self.runtime
            .scheduler_lock_modify_worker()
            .step(event, controller)
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
        future::{Future, pending, ready},
        pin::Pin,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        task::{Context, Poll, Waker},
    };
    use std::{boxed::Box, sync::Arc, task::Wake};

    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_esp32s31_bluetooth::BluetoothControllerRuntimeResources;

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
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());

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
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());
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
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());
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
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());

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
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());
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
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());
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
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());

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
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());

        publisher.publish_scheduler(BluetoothSchedulerWorkerWakeClass::Ordinary);

        assert!(matches!(
            block_on(receiver.wait_with_recheck(ready(()))),
            EmbassyBluetoothWake::Scheduler(batch) if !batch.is_marked()
        ));
    }

    #[test]
    fn external_recheck_does_not_consume_a_later_scheduler_batch() {
        let mut runtime = BluetoothControllerRuntimeResources::<4>::new();
        let mut wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (publisher, mut receiver, _modem_timer) = wakers.split(runtime.split());

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

    #[test]
    fn serialized_primary_notification_wakes_both_fresh_ordinary_consumers() {
        let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (scheduler_counter, scheduler_waker) = counting_waker();
        let (lock_modify_counter, lock_modify_waker) = counting_waker();
        wakers.scheduler_waker.register(&scheduler_waker);
        wakers.lock_modify_waker.register(&lock_modify_waker);

        assert_eq!(
            wakers.notify_ordinary(BluetoothPrimaryOrdinaryPublication::Scheduler {
                scheduler: BluetoothSchedulerWakePublication::WakeWorker,
                lock_modify: BluetoothSchedulerLockModifyEventPublication::WakeWorker,
            }),
            BluetoothPrimaryOrdinaryPublication::Scheduler {
                scheduler: BluetoothSchedulerWakePublication::WakeWorker,
                lock_modify: BluetoothSchedulerLockModifyEventPublication::WakeWorker,
            }
        );
        assert_eq!(scheduler_counter.0.load(Ordering::Relaxed), 1);
        assert_eq!(lock_modify_counter.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn serialized_primary_notification_does_not_wake_coalesced_consumers() {
        let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let (scheduler_counter, scheduler_waker) = counting_waker();
        let (lock_modify_counter, lock_modify_waker) = counting_waker();
        wakers.scheduler_waker.register(&scheduler_waker);
        wakers.lock_modify_waker.register(&lock_modify_waker);

        assert_eq!(
            wakers.notify_ordinary(BluetoothPrimaryOrdinaryPublication::Scheduler {
                scheduler: BluetoothSchedulerWakePublication::Coalesced,
                lock_modify: BluetoothSchedulerLockModifyEventPublication::Coalesced,
            }),
            BluetoothPrimaryOrdinaryPublication::Scheduler {
                scheduler: BluetoothSchedulerWakePublication::Coalesced,
                lock_modify: BluetoothSchedulerLockModifyEventPublication::Coalesced,
            }
        );
        assert_eq!(scheduler_counter.0.load(Ordering::Relaxed), 0);
        assert_eq!(lock_modify_counter.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn borrowed_scheduler_wait_observes_without_consuming() {
        let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let wake = BluetoothSchedulerWakeCell::new();

        assert_eq!(
            wake.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Ordinary),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        block_on(wakers.wait_scheduler_ready(&wake));
        assert!(wakers.scheduler_pending(&wake));
        assert!(wake.take().is_some());
    }

    #[test]
    fn cancelled_borrowed_scheduler_wait_preserves_replacement_readiness() {
        let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let wake = BluetoothSchedulerWakeCell::new();
        let (_counter, task_waker) = counting_waker();
        let mut cancelled = Box::pin(wakers.wait_scheduler_ready(&wake));

        assert!(poll_once(cancelled.as_mut(), &task_waker).is_pending());
        drop(cancelled);
        assert_eq!(
            wake.publish_from_interrupt(BluetoothSchedulerWorkerWakeClass::Marked),
            BluetoothSchedulerWakePublication::WakeWorker
        );
        wakers.scheduler_waker.wake();

        block_on(wakers.wait_scheduler_ready(&wake));
        assert!(
            wake.take()
                .expect("published batch remains durable")
                .is_marked()
        );
    }

    #[test]
    fn post_unlink_publication_before_wait_is_rechecked_without_loss() {
        let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let ready = AtomicBool::new(true);

        assert_eq!(
            wakers.notify_post_unlink(BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer),
            BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer
        );
        block_on(poll_fn(|context| {
            poll_borrowed_ready(&wakers.post_unlink_waker, context, || {
                ready.load(Ordering::Acquire)
            })
        }));
    }

    #[test]
    fn post_unlink_mailbox_wins_a_simultaneous_recheck_tie() {
        assert_eq!(
            block_on(select_post_unlink_first(ready(()), ready(()))),
            EmbassyBluetoothPostUnlinkSignal::Mailbox
        );
    }

    #[test]
    fn absolute_recheck_advances_post_unlink_without_an_interrupt_edge() {
        assert_eq!(
            block_on(select_post_unlink_first(pending::<()>(), ready(()))),
            EmbassyBluetoothPostUnlinkSignal::Recheck
        );
    }

    #[test]
    fn post_unlink_wait_wakes_once_while_ready_publications_coalesce() {
        let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let ready = AtomicBool::new(false);
        let (counter, waker) = counting_waker();
        let mut wait = Box::pin(poll_fn(|context| {
            poll_borrowed_ready(&wakers.post_unlink_waker, context, || {
                ready.load(Ordering::Acquire)
            })
        }));

        assert!(poll_once(wait.as_mut(), &waker).is_pending());
        ready.store(true, Ordering::Release);
        wakers.notify_post_unlink(BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer);
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            wakers.notify_post_unlink(BluetoothDtmPostUnlinkMailboxPublication::AlreadyReady),
            BluetoothDtmPostUnlinkMailboxPublication::AlreadyReady
        );
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert!(poll_once(wait.as_mut(), &waker).is_ready());
    }

    #[test]
    fn cancelled_post_unlink_wait_leaves_ready_epoch_for_replacement() {
        let wakers = EmbassyBluetoothRuntimeWakers::<NoopRawMutex>::new();
        let ready = AtomicBool::new(false);
        let (_counter, waker) = counting_waker();
        let mut cancelled = Box::pin(poll_fn(|context| {
            poll_borrowed_ready(&wakers.post_unlink_waker, context, || {
                ready.load(Ordering::Acquire)
            })
        }));

        assert!(poll_once(cancelled.as_mut(), &waker).is_pending());
        drop(cancelled);
        ready.store(true, Ordering::Release);
        wakers.notify_post_unlink(BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer);

        block_on(poll_fn(|context| {
            poll_borrowed_ready(&wakers.post_unlink_waker, context, || {
                ready.load(Ordering::Acquire)
            })
        }));
    }
}
