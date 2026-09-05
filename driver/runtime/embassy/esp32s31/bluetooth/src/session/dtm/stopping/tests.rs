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
    LeControllerHciResources, LeControllerIdleClassifiedCommandRoute, LeControllerResponsePending,
    LeControllerResponsePublication,
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
            FakeStoppingPhase::Scheduler => Some(StoppingWaitRef::Scheduler(&self.scheduler_ready)),
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

type ControllerResources = LeControllerHciResources<NoopRawMutex, 1, 1, 45>;

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
    endpoints: &mut LeControllerHciEndpoints<'epoch, NoopRawMutex, 1, 1, 45>,
    ready: LeControllerCommandReady<'epoch, ()>,
) -> LeControllerResponsePending<'epoch, ()> {
    block_on(endpoints.host.write(&LeTestEnd::new())).expect("Test End enters the real Host queue");
    let mut command_buffer = [0; 45];
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
    endpoints: &mut LeControllerHciEndpoints<'epoch, NoopRawMutex, 1, 1, 45>,
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
