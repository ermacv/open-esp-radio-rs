#![no_std]
#![forbid(unsafe_code)]

//! Durable Embassy wake handoff for ESP32-S31 Bluetooth controller work.
//!
//! The controller core owns the pending scheduler state. This adapter adds
//! only executor notification and an executor-neutral recheck rendezvous: an
//! interrupt service wakes task-side work on a fresh pending epoch, and each
//! wait registers its waker before rechecking durable state.

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
#[cfg(target_arch = "riscv32")]
mod legacy_advertising_active;
#[cfg(target_arch = "riscv32")]
mod legacy_advertising_first;
mod modem_timer_task;
#[cfg(target_arch = "riscv32")]
mod passive_scanning_active;
#[cfg(target_arch = "riscv32")]
mod passive_scanning_first;

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
#[cfg(target_arch = "riscv32")]
pub use legacy_advertising_active::{
    EmbassyBluetoothLegacyAdvertisingActiveDrive, EmbassyBluetoothLegacyAdvertisingDelaySource,
    EmbassyBluetoothLegacyAdvertisingRecurringDrive, drive_legacy_advertising_active_ready,
    drive_legacy_advertising_recurring_ready,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_advertising_first::{
    EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait,
    EmbassyBluetoothLegacyAdvertisingFirstDrive, EmbassyBluetoothLegacyAdvertisingFirstResume,
    drive_legacy_advertising_first_ready,
};
#[cfg(target_arch = "riscv32")]
pub use passive_scanning_active::{
    EmbassyBluetoothPassiveScanActiveDrive, EmbassyBluetoothPassiveScanRecurringDrive,
    drive_passive_scan_active_ready, drive_passive_scan_recurring_ready,
};
#[cfg(target_arch = "riscv32")]
pub use passive_scanning_first::{
    EmbassyBluetoothPassiveScanFirstControllerTimeWait, EmbassyBluetoothPassiveScanFirstDrive,
    EmbassyBluetoothPassiveScanFirstResume, drive_passive_scan_first_ready,
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

#[cfg(any(target_arch = "riscv32", test))]
use core::future::Future;
use core::{future::poll_fn, task::Poll};

#[cfg(any(target_arch = "riscv32", test))]
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::RawMutex, waitqueue::GenericAtomicWaker};
#[cfg(test)]
use open_esp_radio_esp32s31_bluetooth::BluetoothSchedulerWorkerWakeClass;
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmPostUnlinkMailboxPublication, BluetoothDtmPostUnlinkWakeCell,
    BluetoothModemLpTimerPublishedInterruptStep, BluetoothPrimaryOrdinaryPublication,
    BluetoothPrimarySerializedServiceStep, BluetoothSchedulerLockModifyEventPublication,
    BluetoothSchedulerWakeCell, BluetoothSchedulerWakePublication,
};

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
/// This adapter owns no pending state or Controller worker. Its notification
/// and wait methods borrow the durable core cells that are bound to the
/// hardware epoch. A live interrupt-to-task route must use a [`RawMutex`]
/// implementation that synchronizes those contexts; `NoopRawMutex` is
/// suitable only for single-executor use and tests.
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
    /// This future owns neither the task runtime nor the scheduler batch. It can therefore
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
}

impl<M: RawMutex> Default for EmbassyBluetoothRuntimeWakers<M> {
    fn default() -> Self {
        Self::new()
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
