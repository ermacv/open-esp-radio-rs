//! Cancellation-safe borrowed waits for DTM quiescence and Test End response.
//!
//! The affine stopping runner and terminal response transaction always remain
//! in their caller. Futures created here observe only durable readiness and can
//! be dropped without consuming scheduler, mailbox, Controller-time or HCI
//! state.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    LeControllerCommandEndpoint, LeControllerEndpointMismatch,
    LeControllerResponsePending as PortableControllerResponsePending,
};

#[cfg(target_arch = "riscv32")]
use crate::notification::RuntimeNotifications;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::dtm::{BluetoothDtmStoppingWait, DtmStoppingRunner, DtmTestEndResponsePending},
};

/// Exact readiness source for one parked Test End quiescence runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmStoppingSignal {
    /// The durable scheduler wake cell became non-empty.
    Scheduler,
    /// The exact post-unlink mailbox wake became durable.
    PostUnlink,
    /// The caller-provided absolute Controller-time recheck completed.
    ControllerTime,
}

/// Terminal Test End response capacity became observable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmTestEndResponseSignal {
    /// The matching Controller-to-Host queue reported a non-reserving hint.
    Capacity,
}

/// Failure to wait through a foreign HCI endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmTestEndResponseWaitError {
    /// The supplied endpoint belongs to another live HCI epoch.
    EndpointMismatch,
}

enum StoppingWaitRef<'wait, SchedulerWake: ?Sized, PostUnlinkWake: ?Sized> {
    Scheduler(&'wait SchedulerWake),
    PostUnlink(&'wait PostUnlinkWake),
    ControllerTime,
}

trait StoppingWaitSource {
    type SchedulerWake: ?Sized;
    type PostUnlinkWake: ?Sized;

    fn stopping_wait(
        &self,
    ) -> Option<StoppingWaitRef<'_, Self::SchedulerWake, Self::PostUnlinkWake>>;
}

trait StoppingWaitBackend<SchedulerWake: ?Sized, PostUnlinkWake: ?Sized> {
    fn wait_scheduler<'wait>(
        &'wait self,
        wake: &'wait SchedulerWake,
    ) -> impl Future<Output = ()> + 'wait;

    fn wait_post_unlink<'wait>(
        &'wait self,
        wake: &'wait PostUnlinkWake,
    ) -> impl Future<Output = ()> + 'wait;
}

async fn wait_stopping_source<Source, Backend, Recheck>(
    source: &Source,
    backend: &Backend,
    controller_time_recheck: Recheck,
) -> Option<DtmStoppingSignal>
where
    Source: StoppingWaitSource + ?Sized,
    Backend: StoppingWaitBackend<Source::SchedulerWake, Source::PostUnlinkWake> + ?Sized,
    Recheck: Future<Output = ()>,
{
    match source.stopping_wait()? {
        StoppingWaitRef::Scheduler(wake) => {
            backend.wait_scheduler(wake).await;
            Some(DtmStoppingSignal::Scheduler)
        }
        StoppingWaitRef::PostUnlink(wake) => Some(
            match crate::notification::select_post_unlink_first(
                backend.wait_post_unlink(wake),
                controller_time_recheck,
            )
            .await
            {
                crate::notification::PostUnlinkSignal::Mailbox => DtmStoppingSignal::PostUnlink,
                crate::notification::PostUnlinkSignal::Recheck => DtmStoppingSignal::ControllerTime,
            },
        ),
        StoppingWaitRef::ControllerTime => {
            controller_time_recheck.await;
            Some(DtmStoppingSignal::ControllerTime)
        }
    }
}

trait TestEndResponseWaitSource {
    fn wait_response_capacity<
        'wait,
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &'wait self,
        controller: &'wait LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> impl Future<Output = Result<(), LeControllerEndpointMismatch>> + 'wait;
}

impl<Radio> TestEndResponseWaitSource for PortableControllerResponsePending<'_, Radio> {
    fn wait_response_capacity<
        'wait,
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &'wait self,
        controller: &'wait LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> impl Future<Output = Result<(), LeControllerEndpointMismatch>> + 'wait {
        controller.wait_response_capacity(self)
    }
}

async fn wait_test_end_response_capacity<
    Source,
    M: RawMutex,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>(
    source: &Source,
    controller: &LeControllerCommandEndpoint<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
) -> Result<DtmTestEndResponseSignal, DtmTestEndResponseWaitError>
where
    Source: TestEndResponseWaitSource + ?Sized,
{
    source
        .wait_response_capacity(controller)
        .await
        .map(|()| DtmTestEndResponseSignal::Capacity)
        .map_err(|_| DtmTestEndResponseWaitError::EndpointMismatch)
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> StoppingWaitSource for DtmStoppingRunner<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    type SchedulerWake = oer_esp32s31_bluetooth::interrupt::SchedulerWakeCell;
    type PostUnlinkWake = oer_esp32s31_bluetooth::le::dtm::DtmPostUnlinkWakeCell;

    fn stopping_wait(
        &self,
    ) -> Option<StoppingWaitRef<'_, Self::SchedulerWake, Self::PostUnlinkWake>> {
        self.wait().map(|wait| match wait {
            BluetoothDtmStoppingWait::Scheduler(wake) => StoppingWaitRef::Scheduler(wake),
            BluetoothDtmStoppingWait::PostUnlink(wake) => StoppingWaitRef::PostUnlink(wake),
            BluetoothDtmStoppingWait::ControllerTime => StoppingWaitRef::ControllerTime,
        })
    }
}

#[cfg(target_arch = "riscv32")]
impl<M: RawMutex>
    StoppingWaitBackend<
        oer_esp32s31_bluetooth::interrupt::SchedulerWakeCell,
        oer_esp32s31_bluetooth::le::dtm::DtmPostUnlinkWakeCell,
    > for RuntimeNotifications<M>
{
    fn wait_scheduler<'wait>(
        &'wait self,
        wake: &'wait oer_esp32s31_bluetooth::interrupt::SchedulerWakeCell,
    ) -> impl Future<Output = ()> + 'wait {
        self.wait_scheduler_ready(wake)
    }

    fn wait_post_unlink<'wait>(
        &'wait self,
        wake: &'wait oer_esp32s31_bluetooth::le::dtm::DtmPostUnlinkWakeCell,
    ) -> impl Future<Output = ()> + 'wait {
        self.wait_post_unlink_ready(wake)
    }
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> TestEndResponseWaitSource
    for DtmTestEndResponsePending<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn wait_response_capacity<
        'wait,
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &'wait self,
        controller: &'wait LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> impl Future<Output = Result<(), LeControllerEndpointMismatch>> + 'wait {
        DtmTestEndResponsePending::wait_response_capacity(self, controller)
    }
}

#[cfg(target_arch = "riscv32")]
/// Borrowed Embassy wait view over one parked stopping runner.
///
/// The view contains references only. [`Self::from_waiting`] rejects a runner
/// which can make immediate progress instead of inventing a wait for it.
#[must_use = "borrow this view only while waiting, then step the separately owned runner"]
pub struct DtmShutdownWait<'borrow, 'runtime, S, const CAPACITY: usize, M>
where
    S: SchedulerRunInterruptStorage,
    M: RawMutex,
{
    runner: &'borrow DtmStoppingRunner<'runtime, S, CAPACITY>,
    wakers: &'borrow RuntimeNotifications<M>,
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize, M>
    DtmShutdownWait<'borrow, 'runtime, S, CAPACITY, M>
where
    S: SchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Borrow a stopping runner only while its core typestate exposes a wait.
    pub fn from_waiting(
        runner: &'borrow DtmStoppingRunner<'runtime, S, CAPACITY>,
        wakers: &'borrow RuntimeNotifications<M>,
    ) -> Option<Self> {
        runner.wait().map(|_| Self { runner, wakers })
    }

    /// Wait for the runner's exact current readiness source.
    ///
    /// `controller_time_recheck` must remain anchored to a caller-owned
    /// absolute deadline across cancelled or repeated waits.
    pub async fn wait_next<R>(&self, controller_time_recheck: R) -> DtmStoppingSignal
    where
        R: Future<Output = ()>,
    {
        wait_stopping_source(self.runner, self.wakers, controller_time_recheck)
            .await
            .expect("the borrowed wait view retains an unchanged parked runner")
    }
}

#[cfg(target_arch = "riscv32")]
/// Borrowed capacity wait over one terminal Test End response transaction.
///
/// The pending response, task service and reclaimed graph stay in the caller;
/// this view can observe capacity but cannot publish or restore anything.
#[must_use = "wait for a hint, then publish through the separately owned transaction"]
pub struct DtmTestEndResponseWait<'borrow, 'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pending: &'borrow DtmTestEndResponsePending<'runtime, S, CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize>
    DtmTestEndResponseWait<'borrow, 'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Borrow one exact terminal response without moving its affine recovery owner.
    pub const fn new(pending: &'borrow DtmTestEndResponsePending<'runtime, S, CAPACITY>) -> Self {
        Self { pending }
    }

    /// Verify endpoint affinity, then await a non-reserving C2H capacity hint.
    pub async fn wait_next<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<DtmTestEndResponseSignal, DtmTestEndResponseWaitError> {
        wait_test_end_response_capacity(self.pending, controller).await
    }
}

#[cfg(test)]
mod tests;
