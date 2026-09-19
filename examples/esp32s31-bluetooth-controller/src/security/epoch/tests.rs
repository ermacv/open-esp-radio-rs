use super::*;
use crate::security::bonds::RamBondStore;
use bt_hci::controller::ExternalController;
use core::{
    future::pending,
    pin::pin,
    task::{Context, Poll, Waker},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_bluetooth_hci::*;
use trouble_host::prelude::*;

fn poll<T>(future: impl Future<Output = T>) -> T {
    match pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("expected finite in-memory operation"),
    }
}

#[test]
fn stop_before_host_initialization_retains_ram_bond_and_waits_for_real_reset_response() {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
        251,
        4,
    )
    .unwrap();
    let mut transport = LeControllerHciResources::<NoopRawMutex, 4, 4, 258>::new(config).unwrap();
    let endpoints = transport.split();
    let mut peer = endpoints.controller;
    let LeControllerCommandReadyClaim::Ready(ready) = peer.claim_initial_command_ready(()) else {
        panic!("initial command ownership");
    };
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 1>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let mut bonds = RamBondStore::<1>::new();
    let bond = BondInformation::new(
        Identity::from(Address::random([1, 2, 3, 4, 5, 0xc6])),
        LongTermKey::new(42),
        SecurityLevel::EncryptedAuthenticated,
        true,
    );
    poll(bonds.insert(bond.clone())).unwrap();
    let comparison = NumericComparison::new();
    let exit = {
        let mut epoch = pin!(run(stack, &mut bonds, &comparison, async {}, |_| {}));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(epoch.as_mut().poll(&mut cx).is_pending());
        let mut buffer = [0; 258];
        let LeControllerCommandIntake::Command { command, .. } =
            peer.try_receive_classified_command_with_buffer(ready, &mut buffer)
        else {
            panic!("shutdown submitted Reset before initialization");
        };
        let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
            peer.route_idle_classified_command(command)
        else {
            panic!("expected Reset");
        };
        assert!(epoch.as_mut().poll(&mut cx).is_pending());
        let LeControllerResetCompletion::ResponsePending(response) =
            peer.complete_reset_after_quiescence(barrier)
        else {
            panic!("matching reset barrier");
        };
        assert!(epoch.as_mut().poll(&mut cx).is_pending());
        let LeControllerResponsePublication::Published(_ready) = response.try_publish(&peer) else {
            panic!("response capacity");
        };
        // Controller::read dispatches completion; the next poll observes it.
        let mut exit = None;
        for _ in 0..4 {
            if let Poll::Ready(value) = epoch.as_mut().poll(&mut cx) {
                exit = Some(value);
                break;
            }
        }
        exit.expect("reset response releases the old Host epoch")
    };
    assert!(matches!(exit.cause, Cause::Requested));
    assert!(exit.reset.is_ok());
    assert_eq!(poll(bonds.load(0)).unwrap(), Some(bond.clone()));
    assert!(comparison.pending().is_none());

    // Reuse Host storage only after consuming the old Host. This tests software
    // reconstruction; physical cold release is independently required by HIL.
    let stack = trouble_host::new(exit.controller, &mut resources).build();
    assert!(stack.with_bond_information(|keys| keys.is_empty()));
    {
        let mut app = pin!(gatt::run(&stack, &mut bonds, &comparison, |_| {}));
        assert!(
            app.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert!(stack.with_bond_information(|keys| keys == [bond.clone()]));
    }
    assert_eq!(poll(bonds.load(0)).unwrap(), Some(bond));
}

#[test]
fn reset_receive_failure_is_not_a_successful_shutdown() {
    #[derive(Debug)]
    struct Fault;
    impl core::fmt::Display for Fault {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("closed")
        }
    }
    impl core::error::Error for Fault {}
    impl embedded_io_async::Error for Fault {
        fn kind(&self) -> embedded_io_async::ErrorKind {
            embedded_io_async::ErrorKind::BrokenPipe
        }
    }
    impl From<bt_hci::ReadHciError<Infallible>> for Fault {
        fn from(_: bt_hci::ReadHciError<Infallible>) -> Self {
            Self
        }
    }
    struct Failed;
    impl embedded_io_async::ErrorType for Failed {
        type Error = Fault;
    }
    impl bt_hci::transport::Transport for Failed {
        async fn read<'a, P: bt_hci::transport::PacketToHost<'a>>(
            &self,
            _: &'a mut [u8],
        ) -> Result<P, Self::Error> {
            Err(Fault)
        }
        async fn write<P: bt_hci::transport::PacketToController>(
            &self,
            _: &P,
        ) -> Result<(), Self::Error> {
            pending().await
        }
    }
    let controller = ExternalController::<_, 1>::new(Failed);
    assert!(matches!(
        poll(reset(&controller)),
        Err(ResetError::Receive(Fault))
    ));
}
