//! Test the compiled target evidence/control module, not a host reimplementation.
#[path = "../../../targets/esp32s31/runtime/src/bluetooth_gatt/secure/state.rs"]
mod state;
use bluetooth_example::security::gatt::Observation;
use open_esp_radio_hil_protocol::*;

fn snapshot(state: &state::State) -> BluetoothSecureGattEvidence {
    let Event::BluetoothSecureGatt(e) = state.command(Command::QueryBluetoothSecureGatt) else {
        panic!("snapshot")
    };
    e
}
#[test]
fn explicit_decision_is_single_use_and_not_bond_evidence() {
    let state = state::State::new();
    let lease = state.comparison.begin(123).unwrap();
    let challenge = snapshot(&state).comparison.unwrap();
    let decision = BluetoothNumericDecision {
        challenge,
        accept: true,
    };
    assert_eq!(
        state.command(Command::ConfirmBluetoothGatt(decision)),
        Event::BluetoothGattDecisionRecorded(decision)
    );
    assert!(snapshot(&state).comparison.is_none());
    assert_eq!(snapshot(&state).bonds_stored, 0);
    assert_eq!(snapshot(&state).accepted, 0);
    assert!(matches!(
        state.command(Command::ConfirmBluetoothGatt(decision)),
        Event::Rejected(_)
    ));
    drop(lease);
    let _next = state.comparison.begin(123).unwrap();
    assert!(matches!(
        state.command(Command::ConfirmBluetoothGatt(decision)),
        Event::Rejected(_)
    ));
    state.stopped();
    let decision = BluetoothNumericDecision {
        challenge: snapshot(&state).comparison.unwrap(),
        accept: true,
    };
    assert!(matches!(
        state.command(Command::ConfirmBluetoothGatt(decision)),
        Event::Rejected(_)
    ));
}
#[test]
fn observations_preserve_epoch_counters_without_inventing_peer_delivery() {
    let state = state::State::new();
    for event in [
        Observation::Ready([1; 6]),
        Observation::Advertising,
        Observation::Connected,
        Observation::BondStored,
        Observation::Written(42),
        Observation::NotificationQueued(42),
        Observation::Disconnected(0x13),
        Observation::Advertising,
        Observation::Connected,
        Observation::BondResumed,
        Observation::Read(42),
    ] {
        state.observe(event);
    }
    let e = snapshot(&state);
    assert_eq!((e.traffic.connections, e.traffic.disconnections), (2, 1));
    assert_eq!(
        (e.bonds_stored, e.bonds_resumed, e.notifications_queued),
        (1, 1, 1)
    );
    assert_eq!(
        (e.traffic.value, e.traffic.reads, e.traffic.writes),
        (42, 1, 1)
    );
    assert!(e.comparison.is_none());
    assert!(!e.application_stopped);
    assert!(matches!(
        state.command(Command::QueryBluetoothGatt),
        Event::Rejected(_)
    ));
}

#[test]
fn cold_restart_is_epoch_bound_and_does_not_reset_bond_or_comparison_history() {
    let state = state::State::new();
    state.observe(Observation::BondStored);
    state.observe(Observation::Written(42));
    let prompt = state.comparison.begin(123).unwrap();
    let challenge = snapshot(&state).comparison.unwrap();
    let decision = BluetoothNumericDecision {
        challenge,
        accept: true,
    };
    assert!(matches!(
        state.command(Command::RestartBluetoothGatt { epoch: 0 }),
        Event::Rejected(_)
    ));
    assert!(
        matches!(state.command(Command::RestartBluetoothGatt { epoch: 1 }), Event::BluetoothSecureGatt(e) if e.restarting)
    );
    assert!(matches!(
        state.command(Command::RestartBluetoothGatt { epoch: 1 }),
        Event::Rejected(_)
    ));
    assert!(matches!(
        state.command(Command::ConfirmBluetoothGatt(decision)),
        Event::Rejected(_)
    ));
    drop(prompt);
    state.cold(true);
    state.restarted();
    let e = snapshot(&state);
    assert_eq!(
        (e.epoch, e.cold_releases, e.bonds_stored, e.traffic.value),
        (2, 1, 1, 0)
    );
    assert!(e.old_hci_closed);
    assert!(!e.restarting);
    assert!(matches!(
        state.command(Command::RestartBluetoothGatt { epoch: 1 }),
        Event::Rejected(_)
    ));
    let _new = state.comparison.begin(123).unwrap();
    assert!(matches!(
        state.command(Command::ConfirmBluetoothGatt(decision)),
        Event::Rejected(_)
    ));
    state.stopped();
    assert!(matches!(
        state.command(Command::RestartBluetoothGatt { epoch: 2 }),
        Event::Rejected(_)
    ));
    assert!(snapshot(&state).application_stopped);
}
