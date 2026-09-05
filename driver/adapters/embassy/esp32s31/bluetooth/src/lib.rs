#![no_std]
#![forbid(unsafe_code)]

//! Embassy execution and ownership adapters for the ESP32-S31 Bluetooth controller.
//!
//! The controller actor retains idle and active radio owners across borrowed
//! waits, publishes lossless response boundaries, and orchestrates finite session
//! actors. Session adapters provide DTM, advertising, scanning, and peripheral
//! waits. Durable wake notifications register before rechecking chip-owned work;
//! command interpretation and hardware transitions remain in the controller core.

#[cfg(test)]
extern crate std;

mod controller;
mod session;

#[cfg(target_arch = "riscv32")]
pub use session::dtm::active::EmbassyBluetoothDtmActiveWait;
#[cfg(any(target_arch = "riscv32", test))]
pub use session::dtm::active::{
    EmbassyBluetoothDtmActiveCommandSignal, EmbassyBluetoothDtmActivePendingSignal,
    EmbassyBluetoothDtmActiveRadioSignal, EmbassyBluetoothDtmActiveWaitError,
};

#[cfg(target_arch = "riscv32")]
pub use session::advertising::active::{
    EmbassyBluetoothLegacyAdvertisingActiveDrive, EmbassyBluetoothLegacyAdvertisingDelaySource,
    EmbassyBluetoothLegacyAdvertisingRecurringDrive, drive_legacy_advertising_active_ready,
    drive_legacy_advertising_recurring_ready,
};
#[cfg(target_arch = "riscv32")]
pub use session::advertising::connectable::active::{
    EmbassyBluetoothLegacyConnectableAdvertisingReadyContinuations,
    drive_legacy_connectable_advertising_active_ready,
    drive_legacy_connectable_advertising_initial_pending_ready_with,
    drive_legacy_connectable_advertising_pending_ready_with,
    drive_legacy_connectable_advertising_stopping_ready,
};
#[cfg(target_arch = "riscv32")]
pub use session::advertising::connectable::first::{
    EmbassyBluetoothLegacyConnectableAdvertisingFirstControllerTimeWait,
    EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive,
    EmbassyBluetoothLegacyConnectableAdvertisingFirstResume,
    drive_legacy_connectable_advertising_first_ready,
};
#[cfg(target_arch = "riscv32")]
pub use session::advertising::connectable::recurring::{
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationReady,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeReady,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler,
    begin_legacy_connectable_advertising_recurring_command_ready_with,
    begin_legacy_connectable_advertising_recurring_response_pending_with,
    cancel_legacy_connectable_advertising_recurring_candidate_with,
    cancel_legacy_connectable_advertising_recurring_graph_prepared_with,
    cancel_legacy_connectable_advertising_recurring_merged_with,
    cancel_legacy_connectable_advertising_recurring_prepared_with,
    cancel_legacy_connectable_advertising_recurring_scheduled_with,
    cancel_legacy_connectable_advertising_recurring_sequence_pending_with,
    cancel_legacy_connectable_advertising_recurring_sequence_ready_with,
    drive_legacy_connectable_advertising_recurring_candidate_with,
    drive_legacy_connectable_advertising_recurring_graph_prepared_with,
    drive_legacy_connectable_advertising_recurring_merged_with,
    drive_legacy_connectable_advertising_recurring_prepared_with,
    finish_legacy_connectable_advertising_no_connection_stopping_with,
    retain_legacy_connectable_advertising_recurring_controller_time,
    retain_legacy_connectable_advertising_recurring_retry_for_hci,
};
#[cfg(target_arch = "riscv32")]
pub use session::advertising::first::{
    EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait,
    EmbassyBluetoothLegacyAdvertisingFirstDrive, EmbassyBluetoothLegacyAdvertisingFirstResume,
    drive_legacy_advertising_first_ready,
};
#[cfg(target_arch = "riscv32")]
pub use session::dtm::first::{
    EmbassyBluetoothDtmFirstControllerTimeWait, EmbassyBluetoothDtmFirstDrive,
    EmbassyBluetoothDtmFirstResume, drive_dtm_first_ready,
};
#[cfg(target_arch = "riscv32")]
pub use session::peripheral::first::{
    EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeReady,
    EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeWait,
    EmbassyBluetoothLegacyConnectablePeripheralFirstDrive,
    EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep,
    EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication,
    EmbassyBluetoothLegacyConnectablePeripheralFirstRetry,
    EmbassyBluetoothLegacyConnectablePeripheralFirstStoppingStep,
    begin_legacy_connectable_peripheral_first_command_ready,
    begin_legacy_connectable_peripheral_first_response_pending,
    begin_legacy_connectable_peripheral_first_stopping,
};
#[cfg(target_arch = "riscv32")]
pub use session::scan::active::{
    EmbassyBluetoothPassiveScanActiveDrive, EmbassyBluetoothPassiveScanRecurringDrive,
    drive_passive_scan_active_ready, drive_passive_scan_recurring_ready,
};
#[cfg(target_arch = "riscv32")]
pub use session::scan::first::{
    EmbassyBluetoothPassiveScanFirstControllerTimeWait, EmbassyBluetoothPassiveScanFirstDrive,
    EmbassyBluetoothPassiveScanFirstResume, drive_passive_scan_first_ready,
};

pub use session::dtm::task::{
    EmbassyBluetoothDtmControllerTimeRecheck, EmbassyBluetoothDtmControllerTimeRecheckStatus,
    EmbassyBluetoothDtmSessionRetry,
};
#[cfg(target_arch = "riscv32")]
pub use session::dtm::task::{EmbassyBluetoothDtmSessionBoundary, EmbassyBluetoothDtmSessionTask};

#[cfg(target_arch = "riscv32")]
pub use controller::time_recheck::{
    EmbassyBluetoothDtmAbsoluteRecheck, EmbassyBluetoothDtmAbsoluteRecheckWait,
    EmbassyBluetoothDtmRecheckDeadline, EmbassyBluetoothDtmRecheckPeriod,
    EmbassyBluetoothDtmRecheckPeriodError, EmbassyBluetoothDtmRecheckScheduleState,
    EmbassyBluetoothDtmRecheckStartError,
};
#[cfg(target_arch = "riscv32")]
pub use controller::{
    EmbassyBluetoothControllerCommandBoundary, EmbassyBluetoothControllerCommandTask,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringFailStop,
};
pub use controller::{
    EmbassyBluetoothControllerCommandPhase, EmbassyBluetoothControllerIdleCompletion,
    EmbassyBluetoothControllerRetry,
};

#[cfg(any(target_arch = "riscv32", test))]
pub use session::dtm::stopping::{
    EmbassyBluetoothDtmStoppingSignal, EmbassyBluetoothDtmTestEndResponseSignal,
    EmbassyBluetoothDtmTestEndResponseWaitError,
};
#[cfg(target_arch = "riscv32")]
pub use session::dtm::stopping::{
    EmbassyBluetoothDtmStoppingWait, EmbassyBluetoothDtmTestEndResponseWait,
};

pub use controller::modem_timer::EmbassyBluetoothModemTimerWakers;
#[cfg(target_arch = "riscv32")]
pub use controller::modem_timer::{
    EmbassyBluetoothModemTimerDriveStep, EmbassyBluetoothModemTimerDriver,
};

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
mod tests;
