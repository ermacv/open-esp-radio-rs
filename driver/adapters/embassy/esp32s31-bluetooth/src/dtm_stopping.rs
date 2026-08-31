//! Cancellation-safe borrowed waits for DTM quiescence and Test End response.
//!
//! The affine stopping runner and terminal response transaction always remain
//! in their caller. Futures created here observe only durable readiness and can
//! be dropped without consuming scheduler, mailbox, Controller-time or HCI
//! state.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    LeControllerCommandEndpoint, LeControllerEndpointMismatch,
    LeControllerResponsePending as PortableControllerResponsePending,
};

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmStoppingRunner, BluetoothDtmStoppingWait, BluetoothDtmTestEndResponsePending,
    BluetoothSchedulerRunInterruptStorage,
};

#[cfg(target_arch = "riscv32")]
use crate::EmbassyBluetoothRuntimeWakers;

/// Exact readiness source for one parked Test End quiescence runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmStoppingSignal {
    /// The durable scheduler wake cell became non-empty.
    Scheduler,
    /// The exact post-unlink mailbox wake became durable.
    PostUnlink,
    /// The caller-provided absolute Controller-time recheck completed.
    ControllerTime,
}

/// Terminal Test End response capacity became observable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmTestEndResponseSignal {
    /// The matching Controller-to-Host queue reported a non-reserving hint.
    Capacity,
}

/// Failure to wait through a foreign HCI endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmTestEndResponseWaitError {
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
) -> Option<EmbassyBluetoothDtmStoppingSignal>
where
    Source: StoppingWaitSource + ?Sized,
    Backend: StoppingWaitBackend<Source::SchedulerWake, Source::PostUnlinkWake> + ?Sized,
    Recheck: Future<Output = ()>,
{
    match source.stopping_wait()? {
        StoppingWaitRef::Scheduler(wake) => {
            backend.wait_scheduler(wake).await;
            Some(EmbassyBluetoothDtmStoppingSignal::Scheduler)
        }
        StoppingWaitRef::PostUnlink(wake) => Some(
            match crate::select_post_unlink_first(
                backend.wait_post_unlink(wake),
                controller_time_recheck,
            )
            .await
            {
                crate::EmbassyBluetoothPostUnlinkSignal::Mailbox => {
                    EmbassyBluetoothDtmStoppingSignal::PostUnlink
                }
                crate::EmbassyBluetoothPostUnlinkSignal::Recheck => {
                    EmbassyBluetoothDtmStoppingSignal::ControllerTime
                }
            },
        ),
        StoppingWaitRef::ControllerTime => {
            controller_time_recheck.await;
            Some(EmbassyBluetoothDtmStoppingSignal::ControllerTime)
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
) -> Result<EmbassyBluetoothDtmTestEndResponseSignal, EmbassyBluetoothDtmTestEndResponseWaitError>
where
    Source: TestEndResponseWaitSource + ?Sized,
{
    source
        .wait_response_capacity(controller)
        .await
        .map(|()| EmbassyBluetoothDtmTestEndResponseSignal::Capacity)
        .map_err(|_| EmbassyBluetoothDtmTestEndResponseWaitError::EndpointMismatch)
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> StoppingWaitSource for BluetoothDtmStoppingRunner<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    type SchedulerWake = open_esp_radio_esp32s31_bluetooth::BluetoothSchedulerWakeCell;
    type PostUnlinkWake = open_esp_radio_esp32s31_bluetooth::BluetoothDtmPostUnlinkWakeCell;

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
        open_esp_radio_esp32s31_bluetooth::BluetoothSchedulerWakeCell,
        open_esp_radio_esp32s31_bluetooth::BluetoothDtmPostUnlinkWakeCell,
    > for EmbassyBluetoothRuntimeWakers<M>
{
    fn wait_scheduler<'wait>(
        &'wait self,
        wake: &'wait open_esp_radio_esp32s31_bluetooth::BluetoothSchedulerWakeCell,
    ) -> impl Future<Output = ()> + 'wait {
        self.wait_scheduler_ready(wake)
    }

    fn wait_post_unlink<'wait>(
        &'wait self,
        wake: &'wait open_esp_radio_esp32s31_bluetooth::BluetoothDtmPostUnlinkWakeCell,
    ) -> impl Future<Output = ()> + 'wait {
        self.wait_post_unlink_ready(wake)
    }
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> TestEndResponseWaitSource
    for BluetoothDtmTestEndResponsePending<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
        BluetoothDtmTestEndResponsePending::wait_response_capacity(self, controller)
    }
}

#[cfg(target_arch = "riscv32")]
/// Borrowed Embassy wait view over one parked stopping runner.
///
/// The view contains references only. [`Self::from_waiting`] rejects a runner
/// which can make immediate progress instead of inventing a wait for it.
#[must_use = "borrow this view only while waiting, then step the separately owned runner"]
pub struct EmbassyBluetoothDtmStoppingWait<'borrow, 'runtime, S, const CAPACITY: usize, M>
where
    S: BluetoothSchedulerRunInterruptStorage,
    M: RawMutex,
{
    runner: &'borrow BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>,
    wakers: &'borrow EmbassyBluetoothRuntimeWakers<M>,
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize, M>
    EmbassyBluetoothDtmStoppingWait<'borrow, 'runtime, S, CAPACITY, M>
where
    S: BluetoothSchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Borrow a stopping runner only while its core typestate exposes a wait.
    pub fn from_waiting(
        runner: &'borrow BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>,
        wakers: &'borrow EmbassyBluetoothRuntimeWakers<M>,
    ) -> Option<Self> {
        runner.wait().map(|_| Self { runner, wakers })
    }

    /// Wait for the runner's exact current readiness source.
    ///
    /// `controller_time_recheck` must remain anchored to a caller-owned
    /// absolute deadline across cancelled or repeated waits.
    pub async fn wait_next<R>(
        &self,
        controller_time_recheck: R,
    ) -> EmbassyBluetoothDtmStoppingSignal
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
pub struct EmbassyBluetoothDtmTestEndResponseWait<'borrow, 'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending: &'borrow BluetoothDtmTestEndResponsePending<'runtime, S, CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothDtmTestEndResponseWait<'borrow, 'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Borrow one exact terminal response without moving its affine recovery owner.
    pub const fn new(
        pending: &'borrow BluetoothDtmTestEndResponsePending<'runtime, S, CAPACITY>,
    ) -> Self {
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
    ) -> Result<EmbassyBluetoothDtmTestEndResponseSignal, EmbassyBluetoothDtmTestEndResponseWaitError>
    {
        wait_test_end_response_capacity(self.pending, controller).await
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, pending},
        pin::Pin,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        task::{Context, Poll},
    };

    use bt_hci::{cmd::le::LeTestEnd, transport::Transport};
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerCommandIntake,
        LeControllerCommandReady, LeControllerCommandReadyClaim, LeControllerHciEndpoints,
        LeControllerHciResources, LeControllerIdleClassifiedCommandRoute,
        LeControllerResponsePending, LeControllerResponsePublication,
    };
    use std::{boxed::Box, task::Waker};

    use super::{
        EmbassyBluetoothDtmStoppingSignal, EmbassyBluetoothDtmTestEndResponseSignal,
        EmbassyBluetoothDtmTestEndResponseWaitError, StoppingWaitBackend, StoppingWaitRef,
        StoppingWaitSource, wait_stopping_source, wait_test_end_response_capacity,
    };

    #[derive(Clone, Copy)]
    enum FakeStoppingPhase {
        Scheduler,
        PostUnlink,
        ControllerTime,
        Immediate,
    }

    struct FakeStoppingSource {
        phase: FakeStoppingPhase,
        scheduler_ready: AtomicBool,
        post_unlink_ready: AtomicBool,
    }

    impl FakeStoppingSource {
        const fn new(phase: FakeStoppingPhase) -> Self {
            Self {
                phase,
                scheduler_ready: AtomicBool::new(false),
                post_unlink_ready: AtomicBool::new(false),
            }
        }
    }

    impl StoppingWaitSource for FakeStoppingSource {
        type SchedulerWake = AtomicBool;
        type PostUnlinkWake = AtomicBool;

        fn stopping_wait(
            &self,
        ) -> Option<StoppingWaitRef<'_, Self::SchedulerWake, Self::PostUnlinkWake>> {
            match self.phase {
                FakeStoppingPhase::Scheduler => {
                    Some(StoppingWaitRef::Scheduler(&self.scheduler_ready))
                }
                FakeStoppingPhase::PostUnlink => {
                    Some(StoppingWaitRef::PostUnlink(&self.post_unlink_ready))
                }
                FakeStoppingPhase::ControllerTime => Some(StoppingWaitRef::ControllerTime),
                FakeStoppingPhase::Immediate => None,
            }
        }
    }

    struct AtomicWaitBackend {
        scheduler_polls: AtomicUsize,
        post_unlink_polls: AtomicUsize,
    }

    impl AtomicWaitBackend {
        const fn new() -> Self {
            Self {
                scheduler_polls: AtomicUsize::new(0),
                post_unlink_polls: AtomicUsize::new(0),
            }
        }
    }

    struct AtomicReadiness<'wait> {
        ready: &'wait AtomicBool,
        polls: &'wait AtomicUsize,
    }

    impl Future for AtomicReadiness<'_> {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            self.polls.fetch_add(1, Ordering::Relaxed);
            if self.ready.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    impl StoppingWaitBackend<AtomicBool, AtomicBool> for AtomicWaitBackend {
        fn wait_scheduler<'wait>(
            &'wait self,
            wake: &'wait AtomicBool,
        ) -> impl Future<Output = ()> + 'wait {
            AtomicReadiness {
                ready: wake,
                polls: &self.scheduler_polls,
            }
        }

        fn wait_post_unlink<'wait>(
            &'wait self,
            wake: &'wait AtomicBool,
        ) -> impl Future<Output = ()> + 'wait {
            AtomicReadiness {
                ready: wake,
                polls: &self.post_unlink_polls,
            }
        }
    }

    type ControllerResources = LeControllerHciResources<NoopRawMutex, 1, 1, 16>;

    fn controller_resources() -> ControllerResources {
        LeControllerHciResources::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
                12,
                1,
            )
            .expect("the test HCI profile is nonzero"),
        )
        .expect("the test profile fits its source-owned storage")
    }

    fn test_end_pending_with_ready<'epoch>(
        endpoints: &mut LeControllerHciEndpoints<'epoch, NoopRawMutex, 1, 1, 16>,
        ready: LeControllerCommandReady<'epoch, ()>,
    ) -> LeControllerResponsePending<'epoch, ()> {
        block_on(endpoints.host.write(&LeTestEnd::new()))
            .expect("Test End enters the real Host queue");
        let mut command_buffer = [0; 16];
        let LeControllerCommandIntake::Command {
            command: classified,
            ..
        } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(ready, &mut command_buffer)
        else {
            panic!("Test End is classified with its affine command authority");
        };
        let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) = endpoints
            .controller
            .route_idle_classified_command(classified)
        else {
            panic!("idle Test End becomes one ordered response");
        };
        pending
    }

    fn initial_test_end_pending<'epoch>(
        endpoints: &mut LeControllerHciEndpoints<'epoch, NoopRawMutex, 1, 1, 16>,
    ) -> LeControllerResponsePending<'epoch, ()> {
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("the test epoch exposes its sole initial command authority");
        };
        test_end_pending_with_ready(endpoints, ready)
    }

    #[test]
    fn production_mapping_routes_every_wait_kind() {
        let backend = AtomicWaitBackend::new();

        let scheduler = FakeStoppingSource::new(FakeStoppingPhase::Scheduler);
        scheduler.scheduler_ready.store(true, Ordering::Release);
        assert_eq!(
            block_on(wait_stopping_source(&scheduler, &backend, pending())),
            Some(EmbassyBluetoothDtmStoppingSignal::Scheduler)
        );

        let post_unlink = FakeStoppingSource::new(FakeStoppingPhase::PostUnlink);
        post_unlink.post_unlink_ready.store(true, Ordering::Release);
        assert_eq!(
            block_on(wait_stopping_source(&post_unlink, &backend, pending())),
            Some(EmbassyBluetoothDtmStoppingSignal::PostUnlink)
        );

        let controller_time = FakeStoppingSource::new(FakeStoppingPhase::ControllerTime);
        assert_eq!(
            block_on(wait_stopping_source(
                &controller_time,
                &backend,
                core::future::ready(()),
            )),
            Some(EmbassyBluetoothDtmStoppingSignal::ControllerTime)
        );

        let immediate = FakeStoppingSource::new(FakeStoppingPhase::Immediate);
        assert_eq!(
            block_on(wait_stopping_source(&immediate, &backend, pending())),
            None
        );
    }

    #[test]
    fn cancelling_production_wait_mapping_preserves_durable_source() {
        let source = FakeStoppingSource::new(FakeStoppingPhase::Scheduler);
        let backend = AtomicWaitBackend::new();
        let mut first = Box::pin(wait_stopping_source(&source, &backend, pending()));
        let mut context = Context::from_waker(Waker::noop());
        assert!(first.as_mut().poll(&mut context).is_pending());
        drop(first);

        source.scheduler_ready.store(true, Ordering::Release);
        assert_eq!(
            block_on(wait_stopping_source(&source, &backend, pending())),
            Some(EmbassyBluetoothDtmStoppingSignal::Scheduler)
        );
        assert_eq!(backend.scheduler_polls.load(Ordering::Relaxed), 2);
        assert_eq!(backend.post_unlink_polls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn real_hci_affinity_rejects_foreign_endpoint_and_reports_capacity() {
        let mut first_resources = controller_resources();
        let mut first = first_resources.split();
        let mut second_resources = controller_resources();
        let second = second_resources.split();
        let pending = initial_test_end_pending(&mut first);

        assert_eq!(
            block_on(wait_test_end_response_capacity(
                &pending,
                &second.controller,
            )),
            Err(EmbassyBluetoothDtmTestEndResponseWaitError::EndpointMismatch)
        );
        assert_eq!(
            block_on(wait_test_end_response_capacity(&pending, &first.controller,)),
            Ok(EmbassyBluetoothDtmTestEndResponseSignal::Capacity)
        );
    }

    #[test]
    fn cancelling_real_hci_capacity_wait_reserves_and_consumes_nothing() {
        let mut resources = controller_resources();
        let mut endpoints = resources.split();
        let first = initial_test_end_pending(&mut endpoints);
        let LeControllerResponsePublication::Published(ready) =
            first.try_publish(&endpoints.controller)
        else {
            panic!("the empty real HCI queue must accept the first response");
        };
        let pending = test_end_pending_with_ready(&mut endpoints, ready);

        let mut wait = Box::pin(wait_test_end_response_capacity(
            &pending,
            &endpoints.controller,
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(wait.as_mut().poll(&mut context).is_pending());
        drop(wait);

        let LeControllerResponsePublication::Pending(_retained) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("cancelling the capacity wait must leave the older packet queued");
        };
    }
}
