//! The target wrapper around real Host/HCI code and the Controller core, with actual queued responses.
use super::{snapshot, state};
use bt_hci::{
    cmd::{Cmd, controller_baseband::Reset},
    controller::ExternalController,
};
use core::{
    pin::pin,
    task::{Context, Poll, Waker},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use gatt_application::security::{
    bonds::RamBondStore, comparison::NumericComparison, epoch, gatt::Observation,
};
use oer_bluetooth_controller::LeController;
use oer_bluetooth_hci::*;
use oer_hil_protocol::{BluetoothGattResetReadGate as Phase, Command, Event};
use oer_hil_target_core::bluetooth_gatt::secure::reset_gate::GatedController;
use trouble_host::prelude::*;

#[test]
fn gate_requests_are_epoch_bound_one_shot_and_cannot_release_early() {
    let state = state::State::new();
    let arm = Command::BluetoothGattResetReadGate {
        epoch: 1,
        release: false,
    };
    let release = Command::BluetoothGattResetReadGate {
        epoch: 1,
        release: true,
    };
    assert!(matches!(state.command(arm.clone()), Event::Rejected(_)));
    state.observe(Observation::Advertising);
    assert!(matches!(
        state.command(Command::BluetoothGattResetReadGate {
            epoch: 0,
            release: false
        }),
        Event::Rejected(_)
    ));
    assert!(matches!(state.command(release.clone()), Event::Rejected(_)));
    assert!(matches!(
        state.command(arm.clone()),
        Event::BluetoothSecureGatt(_)
    ));
    assert_eq!(snapshot(&state).reset_read_gate, Phase::Armed);
    assert!(matches!(state.command(arm), Event::Rejected(_)));
    assert!(matches!(state.command(release.clone()), Event::Rejected(_)));
    state.command(Command::RestartBluetoothGatt { epoch: 1 });
    assert!(matches!(
        state.command(Command::FailBluetoothGattResetRead { epoch: 1 }),
        Event::Rejected(_)
    ));
    assert!(matches!(state.command(release), Event::Rejected(_)));
    assert!(snapshot(&state).shutdown.is_none());
    assert_eq!(snapshot(&state).cold_releases, 0);
}

#[test]
fn genuine_reset_response_stays_queued_until_the_same_reader_is_released() {
    exercise(false, false);
}

#[test]
fn cancellation_does_not_release_the_gate_or_erase_the_queued_response() {
    exercise(true, false);
}

#[test]
fn read_error_preserves_requested_cause_and_unconsumed_response() {
    exercise(false, true);
}

fn exercise(cancel: bool, fail: bool) {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
        251,
        4,
    )
    .unwrap();
    let mut transport = LeControllerHciResources::<NoopRawMutex, 4, 4, 258>::new(config).unwrap();
    let endpoints = transport.split();
    let peer = endpoints.controller;
    let mut core = LeController::<'_, 4>::new(config, None);
    let state = state::State::new();
    state.observe(Observation::Advertising);
    assert!(matches!(
        state.command(Command::BluetoothGattResetReadGate {
            epoch: 1,
            release: false
        }),
        Event::BluetoothSecureGatt(_)
    ));
    state.command(Command::RestartBluetoothGatt { epoch: 1 });
    let gate = &state.reset_gate;
    let controller = GatedController {
        inner: ExternalController::<_, 1>::new(endpoints.host),
        gate,
    };
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(controller, &mut resources).build();
    let mut bonds = RamBondStore::<1>::new();
    let comparison = NumericComparison::new();
    struct WakeCount(std::sync::atomic::AtomicUsize);
    impl std::task::Wake for WakeCount {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn wake_by_ref(self: &std::sync::Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let count = std::sync::Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
    let waker = Waker::from(count.clone());
    let mut cx = Context::from_waker(&waker);
    {
        let mut lifecycle = pin!(epoch::run(stack, &mut bonds, &comparison, async {}, |_| {}));
        assert!(lifecycle.as_mut().poll(&mut cx).is_pending());
        assert_eq!(gate.phase(), Phase::ReaderHeld);
        let mut buffer = [0; 258];
        let Ok(HostToControllerFrame::Command(command)) = peer.try_receive(&mut buffer) else {
            panic!("actual Reset queued");
        };
        assert_eq!(command.opcode(), Reset::OPCODE);
        core.command(command).expect("one command");
        let response = core
            .front()
            .expect("an idle Controller completes Reset at once");
        peer.try_publish(response.kind(), response.as_bytes())
            .expect("response capacity");
        core.pop();
        for _ in 0..4 {
            assert!(lifecycle.as_mut().poll(&mut cx).is_pending());
        }
        assert_eq!(
            peer.try_retire().err(),
            Some(HciRetirementError::ControllerPacketsPending),
            "reader has not consumed response"
        );
        if !cancel {
            assert!(matches!(
                state.command(Command::BluetoothGattResetReadGate {
                    epoch: 2,
                    release: true
                }),
                Event::Rejected(_)
            ));
            let wakes = count.0.load(std::sync::atomic::Ordering::SeqCst);
            let operation = if fail {
                Command::FailBluetoothGattResetRead { epoch: 1 }
            } else {
                Command::BluetoothGattResetReadGate {
                    epoch: 1,
                    release: true,
                }
            };
            assert!(matches!(
                state.command(operation.clone()),
                Event::BluetoothSecureGatt(_)
            ));
            assert!(matches!(state.command(operation), Event::Rejected(_)));
            assert!(count.0.load(std::sync::atomic::Ordering::SeqCst) > wakes);
            assert!(matches!(
                state.command(Command::BluetoothGattResetReadGate {
                    epoch: 1,
                    release: true
                }),
                Event::Rejected(_)
            ));
            let mut exit = None;
            for _ in 0..4 {
                if let Poll::Ready(value) = lifecycle.as_mut().poll(&mut cx) {
                    exit = Some(value);
                    break;
                }
            }
            let exit = exit.expect("response or exact read failure must resolve shutdown");
            assert!(matches!(exit.cause, epoch::Cause::Requested));
            if fail {
                assert_eq!(exit.action(), epoch::ShutdownAction::Retain);
                assert!(matches!(
                    exit.reset,
                    Err(epoch::ResetError::Receive(
                        oer_hil_target_core::bluetooth_gatt::secure::reset_gate::ReadError::Injected
                    ))
                ));
            } else {
                assert_eq!(exit.action(), epoch::ShutdownAction::Restart);
                assert!(exit.reset.is_ok());
            }
        }
    }
    if cancel || fail {
        assert_eq!(
            gate.phase(),
            if fail {
                Phase::ReadFailed
            } else {
                Phase::ReaderHeld
            }
        );
        assert_eq!(
            peer.try_retire().err(),
            Some(HciRetirementError::ControllerPacketsPending)
        );
    } else {
        assert_eq!(gate.phase(), Phase::Released);
        assert!(peer.try_retire().is_ok());
    }
}
