//! Failure/cancellation through the actual Host and affine Controller endpoint.

use super::*;
use crate::security::bonds::StoreError;
use core::cell::Cell;

type Transport = LeControllerHciResources<NoopRawMutex, 4, 4, 258>;
type Peer<'a> = LeControllerCommandEndpoint<'a, NoopRawMutex, 4, 4, 258>;

pub(super) fn transport() -> Transport {
    Transport::new(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
            251,
            4,
        )
        .unwrap(),
    )
    .unwrap()
}

pub(super) fn reset_barrier<'a>(
    peer: &mut Peer<'a>,
    ready: LeControllerCommandReady<'a, ()>,
) -> LeControllerResetBarrier<'a, ()> {
    let LeControllerCommandIntake::Command { command, .. } =
        peer.try_receive_classified_command_with_buffer(ready, &mut [0; 258])
    else {
        panic!("Host must submit its shutdown command");
    };
    let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
        peer.route_idle_classified_command(command)
    else {
        panic!("expected Reset, not advertising or enrollment");
    };
    barrier
}

pub(super) fn complete_reset<'a>(
    peer: &mut Peer<'a>,
    barrier: LeControllerResetBarrier<'a, ()>,
) -> LeControllerCommandReady<'a, ()> {
    let LeControllerResetCompletion::ResponsePending(response) =
        peer.complete_reset_after_quiescence(barrier)
    else {
        panic!("matching Reset owner");
    };
    let LeControllerResponsePublication::Published(ready) = response.try_publish(peer) else {
        panic!("available response capacity");
    };
    ready
}

struct Store<'a> {
    wait: bool,
    reads: &'a Cell<u32>,
    cancelled: &'a Cell<u32>,
}

impl BondStore for Store<'_> {
    type Error = u8;

    fn capacity(&self) -> usize {
        1
    }

    async fn load(&mut self, _: usize) -> Result<Option<BondInformation>, StoreError<u8>> {
        self.reads.set(self.reads.get() + 1);
        if !self.wait {
            return Err(StoreError::Backend(42));
        }
        struct Cancelled<'a>(&'a Cell<u32>);
        impl Drop for Cancelled<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let _cancelled = Cancelled(self.cancelled);
        pending().await
    }

    async fn insert(&mut self, _: BondInformation) -> Result<(), StoreError<u8>> {
        panic!("failed or cancelled restoration must not enroll")
    }

    async fn remove(&mut self, _: Identity) -> Result<bool, StoreError<u8>> {
        panic!("shutdown must not erase application bonds")
    }
}

#[test]
fn successful_reset_preserves_application_store_failure() {
    check_store_exit(false);
}

#[test]
fn immediate_stop_cancels_pending_store_read_before_reset_completes() {
    check_store_exit(true);
}

fn check_store_exit(stop: bool) {
    let mut transport = transport();
    let endpoints = transport.split();
    let mut peer = endpoints.controller;
    let LeControllerCommandReadyClaim::Ready(ready) = peer.claim_initial_command_ready(()) else {
        panic!("fresh Controller authority");
    };
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 1>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let reads = Cell::new(0);
    let cancelled = Cell::new(0);
    let mut store = Store {
        wait: stop,
        reads: &reads,
        cancelled: &cancelled,
    };
    let comparison = NumericComparison::new();
    let exit = {
        let mut epoch = pin!(run(
            stack,
            &mut store,
            &comparison,
            async {
                if !stop {
                    pending::<()>().await;
                }
            },
            |_| panic!("no successful startup observation")
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(epoch.as_mut().poll(&mut cx).is_pending());
        assert_eq!(reads.get(), 1);
        assert_eq!(cancelled.get(), u32::from(stop));
        let barrier = reset_barrier(&mut peer, ready);
        // Ending the application does not synthesize a Reset response.
        assert!(epoch.as_mut().poll(&mut cx).is_pending());
        let _ready = complete_reset(&mut peer, barrier);
        let mut exit = None;
        for _ in 0..4 {
            if let Poll::Ready(value) = epoch.as_mut().poll(&mut cx) {
                exit = Some(value);
                break;
            }
        }
        exit.expect("real Reset completion")
    };
    assert!(exit.reset.is_ok());
    if stop {
        assert!(matches!(exit.cause, Cause::Requested));
        assert_eq!(exit.action(), ShutdownAction::Restart);
    } else {
        assert_eq!(exit.action(), ShutdownAction::Close);
        assert!(matches!(
            exit.cause,
            Cause::Application(gatt::RunError::Store(StoreError::Backend(42)))
        ));
    }
    assert!(comparison.pending().is_none());
    let stack = trouble_host::new(exit.controller, &mut resources).build();
    assert!(stack.with_bond_information(|keys| keys.is_empty()));
}

#[test]
fn cancelled_host_epoch_cannot_retire_an_unconsumed_reset_response() {
    let mut transport = transport();
    let endpoints = transport.split();
    let mut peer = endpoints.controller;
    let LeControllerCommandReadyClaim::Ready(ready) = peer.claim_initial_command_ready(()) else {
        panic!("fresh Controller authority");
    };
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 1>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let mut bonds = RamBondStore::<1>::new();
    let comparison = NumericComparison::new();
    let barrier = {
        let mut epoch = pin!(run(stack, &mut bonds, &comparison, async {}, |_| {}));
        assert!(
            epoch
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        reset_barrier(&mut peer, ready)
        // Drop the consuming Host future while the peer still owns the barrier.
    };
    let ready = complete_reset(&mut peer, barrier);
    let Err((error, retained)) = peer.try_retire_transport(ready) else {
        panic!("dropped Host must not manufacture drained HCI ownership");
    };
    assert_eq!(
        error,
        LeControllerHciRetirementError::ControllerPacketsPending
    );
    // Rejection returns the same command owner; it cannot be retried into PASS
    // until the independent response-consumption obligation has been discharged.
    let Err((again, _retained)) = peer.try_retire_transport(retained) else {
        panic!("unconsumed completion remains outstanding");
    };
    assert_eq!(again, error);
    assert!(comparison.pending().is_none());
}
