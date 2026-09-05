//! Cancellation-safe Embassy notification for the disjoint source-127 task.
//!
//! One outer Controller runner owns the chip task. Its source-127 handler passes
//! `interrupt.service_modem_lp_timer_interrupt()` to
//! `runtime_wakers.notify_modem_timer_service()`. The runner selects
//! `runtime_wakers.modem_timer().driver().wait_ready(&task)` beside its other
//! borrowed waits, then executes exactly one `drive_once` on that winner. A
//! published expiration is dispatched explicitly through `take_expiration`;
//! the successful take wakes a publication blocked on event capacity. No
//! separate executor actor or relative-delay loop is required.

#[cfg(any(test, target_arch = "riscv32"))]
use core::{
    future::poll_fn,
    task::{Context, Poll},
};

use embassy_sync::{blocking_mutex::raw::RawMutex, waitqueue::GenericAtomicWaker};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothModemLpTimerPublishedInterruptStep, BluetoothModemLpTimerWorkerWakePublication,
};

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothControllerModemTimerBegin, BluetoothControllerModemTimerReadinessClass,
    BluetoothControllerModemTimerRearm, BluetoothControllerModemTimerStep,
    BluetoothControllerModemTimerTask, BluetoothModemLpTimerExpiration,
    BluetoothModemLpTimerSoftwareOwnerStorage,
};

#[cfg(any(test, target_arch = "riscv32"))]
fn poll_registered_ready<M: RawMutex>(
    waker: &GenericAtomicWaker<M>,
    context: &mut Context<'_>,
    is_ready: impl FnOnce() -> bool,
) -> Poll<()> {
    waker.register(context.waker());
    if is_ready() {
        Poll::Ready(())
    } else {
        Poll::Pending
    }
}

/// Embassy notification cells for one disjoint source-127 task.
///
/// The cells contain no timer event, queue entry, positional epoch or HAL
/// owner. Those remain in the chip task and its durable core cells. This value
/// only bridges changes in the borrowed readiness predicates to the executor.
pub struct EmbassyBluetoothModemTimerWakers<M: RawMutex> {
    worker_waker: GenericAtomicWaker<M>,
    #[cfg(any(test, target_arch = "riscv32"))]
    event_capacity_waker: GenericAtomicWaker<M>,
}

impl<M: RawMutex> EmbassyBluetoothModemTimerWakers<M> {
    /// Construct notification state for one final Controller runtime epoch.
    pub const fn new() -> Self {
        Self {
            worker_waker: GenericAtomicWaker::new(M::INIT),
            #[cfg(any(test, target_arch = "riscv32"))]
            event_capacity_waker: GenericAtomicWaker::new(M::INIT),
        }
    }

    /// Notify the task only when source 127 opened a fresh durable work epoch.
    ///
    /// Spurious/rearmed entries have no task work. Coalesced publications are
    /// already represented by the core wake cell and therefore need no second
    /// executor wake.
    pub fn notify_modem_timer_service(
        &self,
        step: BluetoothModemLpTimerPublishedInterruptStep,
    ) -> BluetoothModemLpTimerPublishedInterruptStep {
        let publication = match step {
            BluetoothModemLpTimerPublishedInterruptStep::AwaitingSoftware(publication)
            | BluetoothModemLpTimerPublishedInterruptStep::SoftwarePending(publication) => {
                Some(publication)
            }
            BluetoothModemLpTimerPublishedInterruptStep::Spurious
            | BluetoothModemLpTimerPublishedInterruptStep::Rearmed => None,
        };
        if publication == Some(BluetoothModemLpTimerWorkerWakePublication::WakeWorker) {
            self.worker_waker.wake();
        }
        step
    }

    /// Borrow a finite task driver for this notification epoch.
    ///
    /// The returned value borrows only these wakers. It never stores the
    /// affine chip task; every operation receives that task as a bounded
    /// borrow from the sole outer Controller runner.
    #[cfg(target_arch = "riscv32")]
    pub const fn driver(&self) -> EmbassyBluetoothModemTimerDriver<'_, M> {
        EmbassyBluetoothModemTimerDriver { wakers: self }
    }

    #[cfg(any(test, target_arch = "riscv32"))]
    async fn wait_worker_ready(&self, is_ready: impl Fn() -> bool) {
        poll_fn(|context| poll_registered_ready(&self.worker_waker, context, &is_ready)).await
    }

    #[cfg(any(test, target_arch = "riscv32"))]
    async fn wait_event_capacity(&self, has_capacity: impl Fn() -> bool) {
        poll_fn(|context| poll_registered_ready(&self.event_capacity_waker, context, &has_capacity))
            .await
    }

    #[cfg(any(test, target_arch = "riscv32"))]
    fn notify_event_capacity_opened(&self) {
        self.event_capacity_waker.wake();
    }
}

impl<M: RawMutex> Default for EmbassyBluetoothModemTimerWakers<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// One finite synchronous source-127 task transition.
///
/// This enum preserves the chip result without interpreting it in the
/// executor adapter. In particular, an expiration remains in the durable event
/// cell until the runner explicitly consumes it.
#[cfg(target_arch = "riscv32")]
#[must_use = "the outer Controller runner must handle the exact source-127 transition"]
pub enum EmbassyBluetoothModemTimerDriveStep<TakeError, RestoreError> {
    /// The idle task attempted to acquire stable software-pending ownership.
    Begin(BluetoothControllerModemTimerBegin<TakeError>),
    /// One queue, publication or compare transition ran.
    Step(BluetoothControllerModemTimerStep),
    /// One fully rearmed owner attempted to return to ISR storage.
    Rearm(BluetoothControllerModemTimerRearm<RestoreError>),
}

/// Borrow-only Embassy driver for one disjoint source-127 chip task.
///
/// No future owns or moves [`BluetoothControllerModemTimerTask`]. The sole
/// outer runner retains that task, awaits a borrowed readiness predicate, and
/// then calls one synchronous finite transition.
#[cfg(target_arch = "riscv32")]
pub struct EmbassyBluetoothModemTimerDriver<'wakers, M: RawMutex> {
    wakers: &'wakers EmbassyBluetoothModemTimerWakers<M>,
}

#[cfg(target_arch = "riscv32")]
impl<M: RawMutex> EmbassyBluetoothModemTimerDriver<'_, M> {
    /// Wait for the current borrowed readiness predicate.
    ///
    /// Waker registration always precedes the durable chip-state recheck.
    /// Cancelling the future changes neither the endpoint phase nor the core
    /// wake/event cells, so a replacement wait observes the same work.
    pub async fn wait_ready<S, const CAPACITY: usize>(
        &self,
        task: &BluetoothControllerModemTimerTask<'_, S, CAPACITY>,
    ) -> BluetoothControllerModemTimerReadinessClass
    where
        S: BluetoothModemLpTimerSoftwareOwnerStorage,
    {
        let readiness = task.readiness();
        let class = readiness.class();
        match class {
            BluetoothControllerModemTimerReadinessClass::Interrupt => {
                self.wakers.wait_worker_ready(|| readiness.is_ready()).await;
            }
            BluetoothControllerModemTimerReadinessClass::EventCapacity => {
                self.wakers
                    .wait_event_capacity(|| readiness.is_ready())
                    .await;
            }
            BluetoothControllerModemTimerReadinessClass::Step
            | BluetoothControllerModemTimerReadinessClass::Rearm => {}
        }
        class
    }

    /// Execute exactly one transition selected by the task's current phase.
    ///
    /// This method does not loop and does not await. All affine HAL state stays
    /// inside `task`, including storage-rejected acquisition and rearm states.
    pub fn drive_once<S, const CAPACITY: usize>(
        &self,
        task: &mut BluetoothControllerModemTimerTask<'_, S, CAPACITY>,
    ) -> EmbassyBluetoothModemTimerDriveStep<S::TakeError, S::RestoreError>
    where
        S: BluetoothModemLpTimerSoftwareOwnerStorage,
    {
        match task.readiness().class() {
            BluetoothControllerModemTimerReadinessClass::Interrupt => {
                EmbassyBluetoothModemTimerDriveStep::Begin(task.begin())
            }
            BluetoothControllerModemTimerReadinessClass::Step
            | BluetoothControllerModemTimerReadinessClass::EventCapacity => {
                EmbassyBluetoothModemTimerDriveStep::Step(task.step())
            }
            BluetoothControllerModemTimerReadinessClass::Rearm => {
                EmbassyBluetoothModemTimerDriveStep::Rearm(task.rearm())
            }
        }
    }

    /// Consume one published expiration and notify a backpressured producer.
    ///
    /// `None` emits no capacity notification. A successful take is the only
    /// adapter operation that opens the one-event cell, so it wakes the exact
    /// readiness class which may be waiting to retry publication.
    pub fn take_expiration<S, const CAPACITY: usize>(
        &self,
        task: &mut BluetoothControllerModemTimerTask<'_, S, CAPACITY>,
    ) -> Option<BluetoothModemLpTimerExpiration>
    where
        S: BluetoothModemLpTimerSoftwareOwnerStorage,
    {
        let expiration = task.take_expiration();
        if expiration.is_some() {
            self.wakers.notify_event_capacity_opened();
        }
        expiration
    }
}

#[cfg(test)]
mod tests;
