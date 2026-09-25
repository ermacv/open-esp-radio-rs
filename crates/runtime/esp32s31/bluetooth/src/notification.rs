//! Borrowed event notifications; durable pending state remains with the controller.

use crate::controller::ModemTimerWakers;

#[cfg(any(target_arch = "riscv32", test))]
use core::future::Future;

use core::{future::poll_fn, task::Poll};

#[cfg(any(target_arch = "riscv32", test))]
use embassy_futures::select::{Either, select};

use embassy_sync::{blocking_mutex::raw::RawMutex, waitqueue::GenericAtomicWaker};
#[cfg(test)]
use oer_esp32s31_bluetooth::interrupt::SchedulerWorkerWakeClass;

use oer_esp32s31_bluetooth::{
    interrupt::{SchedulerWakeCell, SchedulerWakePublication},
    le::dtm::{
        DtmPostUnlinkMailboxPublication, DtmPostUnlinkWakeCell, PrimaryOrdinaryPublication,
        PrimarySerializedServiceStep,
    },
    modem_lp_timer_queue::ModemLpTimerPublishedInterruptStep,
    scheduler::SchedulerLockModifyEventPublication,
};

pub(crate) fn poll_borrowed_ready<M: RawMutex>(
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
pub(crate) enum PostUnlinkSignal {
    Mailbox,
    Recheck,
}

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) async fn select_post_unlink_first<M, R>(mailbox: M, recheck: R) -> PostUnlinkSignal
where
    M: Future<Output = ()>,
    R: Future<Output = ()>,
{
    match select(mailbox, recheck).await {
        Either::First(()) => PostUnlinkSignal::Mailbox,
        Either::Second(()) => PostUnlinkSignal::Recheck,
    }
}

/// Embassy wakers for one borrowed Bluetooth Controller runtime epoch.
///
/// This adapter owns no pending state or Controller worker. Its notification
/// and wait methods borrow the durable core cells that are bound to the
/// hardware epoch. A live interrupt-to-task route must use a [`RawMutex`]
/// implementation that synchronizes those contexts; `NoopRawMutex` is
/// suitable only for single-executor use and tests.
pub struct RuntimeNotifications<M: RawMutex> {
    scheduler_waker: GenericAtomicWaker<M>,
    lock_modify_waker: GenericAtomicWaker<M>,
    post_unlink_waker: GenericAtomicWaker<M>,
    modem_timer: ModemTimerWakers<M>,
}

impl<M: RawMutex> RuntimeNotifications<M> {
    /// Construct executor notification state without a duplicate event cell.
    pub const fn new() -> Self {
        Self {
            scheduler_waker: GenericAtomicWaker::new(M::INIT),
            lock_modify_waker: GenericAtomicWaker::new(M::INIT),
            post_unlink_waker: GenericAtomicWaker::new(M::INIT),
            modem_timer: ModemTimerWakers::new(),
        }
    }

    /// Notification and finite borrowed driver boundary for source 127.
    pub const fn modem_timer(&self) -> &ModemTimerWakers<M> {
        &self.modem_timer
    }

    /// Route one exact source-127 service result to its borrowed task wait.
    pub fn notify_modem_timer_service(
        &self,
        step: ModemLpTimerPublishedInterruptStep,
    ) -> ModemLpTimerPublishedInterruptStep {
        self.modem_timer.notify_modem_timer_service(step)
    }

    /// Wait until the borrowed scheduler cell contains durable work.
    ///
    /// This future owns neither the task runtime nor the scheduler batch. It can therefore
    /// be selected beside HCI capacity while an affine DTM session remains in
    /// the caller. Successful completion is only a readiness hint; the core
    /// session transition remains responsible for consuming the exact batch.
    pub async fn wait_scheduler_ready(&self, wake: &SchedulerWakeCell) {
        poll_fn(|context| poll_borrowed_ready(&self.scheduler_waker, context, || wake.is_pending()))
            .await
    }

    /// Whether the borrowed scheduler cell contains durable work.
    pub fn scheduler_pending(&self, wake: &SchedulerWakeCell) -> bool {
        wake.is_pending()
    }

    fn notify_post_unlink(
        &self,
        publication: DtmPostUnlinkMailboxPublication,
    ) -> DtmPostUnlinkMailboxPublication {
        if publication == DtmPostUnlinkMailboxPublication::WakeConsumer {
            self.post_unlink_waker.wake();
        }
        publication
    }

    fn notify_ordinary(
        &self,
        publication: PrimaryOrdinaryPublication,
    ) -> PrimaryOrdinaryPublication {
        if let PrimaryOrdinaryPublication::Scheduler {
            scheduler,
            lock_modify,
        } = publication
        {
            if scheduler == SchedulerWakePublication::WakeWorker {
                self.scheduler_waker.wake();
            }
            if lock_modify == SchedulerLockModifyEventPublication::WakeWorker {
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
        step: &PrimarySerializedServiceStep,
    ) -> Option<DtmPostUnlinkMailboxPublication> {
        let (ordinary, mailbox) = match step {
            PrimarySerializedServiceStep::General { ordinary, .. } => (*ordinary, None),
            PrimarySerializedServiceStep::DtmStored {
                mailbox, ordinary, ..
            }
            | PrimarySerializedServiceStep::MailboxFull {
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
    pub async fn wait_post_unlink_ready(&self, wake: &DtmPostUnlinkWakeCell) {
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
    pub(crate) async fn wait_post_unlink_or_recheck<R>(
        &self,
        wake: &DtmPostUnlinkWakeCell,
        recheck: R,
    ) -> PostUnlinkSignal
    where
        R: Future<Output = ()>,
    {
        select_post_unlink_first(self.wait_post_unlink_ready(wake), recheck).await
    }

    /// Whether the post-unlink consumer has durable ready work.
    pub fn post_unlink_pending(&self, wake: &DtmPostUnlinkWakeCell) -> bool {
        wake.is_pending()
    }
}

impl<M: RawMutex> Default for RuntimeNotifications<M> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
