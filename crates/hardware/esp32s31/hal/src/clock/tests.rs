use std::vec::Vec;

use oer_esp32s31_pac::{SharedModemClockGate, WifiPowerBaseline, WifiPowerRestoreReadback};

use super::{
    ClockPort, CommonPhyPowerError, CommonRadioPower, CommonRadioPowerError, PowerEpoch,
    RadioClient, SharedClockLeases, WifiClocks, WifiPowerRestoreCheckpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    SetGate(SharedModemClockGate, bool),
    RestorePower(WifiPowerBaseline),
    PowerSequence,
}

struct Port {
    gates: [bool; 3],
    power: WifiPowerBaseline,
    power_readback: Result<(), WifiPowerRestoreReadback>,
    power_sequence_failure: Option<crate::power::PowerError>,
    operations: Vec<Operation>,
}

impl Port {
    fn new() -> Self {
        Self {
            gates: [false; 3],
            power: WifiPowerBaseline::for_validation(false),
            power_readback: Ok(()),
            power_sequence_failure: None,
            operations: Vec::new(),
        }
    }

    const fn gate_index(gate: SharedModemClockGate) -> usize {
        match gate {
            SharedModemClockGate::Coexistence => 0,
            SharedModemClockGate::PhyI2cMaster => 1,
            SharedModemClockGate::LowPowerTimer => 2,
        }
    }
}

impl ClockPort for Port {
    fn gate_enabled(&self, gate: SharedModemClockGate) -> bool {
        self.gates[Self::gate_index(gate)]
    }
    fn set_gate(&mut self, gate: SharedModemClockGate, enabled: bool) {
        self.operations.push(Operation::SetGate(gate, enabled));
        self.gates[Self::gate_index(gate)] = enabled;
    }
    fn capture_power_baseline(&self) -> WifiPowerBaseline {
        self.power
    }
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback> {
        self.operations.push(Operation::RestorePower(baseline));
        self.power_readback
    }
    fn run_common_power_sequence(
        &mut self,
        leases: &mut SharedClockLeases,
    ) -> Result<(), crate::power::PowerError> {
        self.operations.push(Operation::PowerSequence);
        if let Some(error) = self.power_sequence_failure {
            return Err(error);
        }
        leases.retain_phy_i2c(self);
        Ok(())
    }
}

#[test]
fn shared_gate_restores_exactly_the_state_observed_before_retain() {
    let mut port = Port::new();
    let mut leases = SharedClockLeases::default();
    leases.retain_coexistence(&mut port);
    leases.retain_coexistence(&mut port);
    assert_eq!(
        port.operations,
        [Operation::SetGate(SharedModemClockGate::Coexistence, true)]
    );
    leases.release_all(&mut port);
    assert_eq!(
        port.operations[1..],
        [Operation::SetGate(SharedModemClockGate::Coexistence, false)]
    );

    let mut port = Port::new();
    port.gates = [true; 3];
    leases.retain_phy_i2c(&mut port);
    leases.release_all(&mut port);
    assert!(port.operations.is_empty());
    assert!(port.gates.iter().all(|&enabled| enabled));
}

#[test]
fn power_retry_preserves_the_original_cold_baseline_until_commit() {
    let original = WifiPowerBaseline::for_validation(false);
    let retry_observation = WifiPowerBaseline::for_validation(true);
    let mut port = Port::new();
    port.power = original;
    let mut epoch = PowerEpoch::default();

    epoch.prepare(&port);
    port.power = retry_observation;
    epoch.prepare(&port);
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(port.operations, [Operation::RestorePower(original)]);

    epoch.prepare(&port);
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(
        port.operations[1..],
        [Operation::RestorePower(retry_observation)]
    );
}

#[test]
fn power_restore_failure_retains_the_baseline_for_retry() {
    let mut port = Port::new();
    let mut epoch = PowerEpoch::default();
    epoch.prepare(&port);

    port.power_readback = Err(WifiPowerRestoreReadback::ModemSourceClocks);
    assert_eq!(
        epoch.restore(&mut port),
        Err(WifiPowerRestoreCheckpoint::ModemSourceClocks)
    );
    port.power_readback = Ok(());
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(port.operations.len(), 2);
}

/// A Wi-Fi route after its power sequence and MAC coexistence retention.
fn powered_wifi(port: &mut Port) -> WifiClocks {
    let mut clocks = WifiClocks::default();
    clocks.power.prepare(port);
    clocks.shared.retain_phy_i2c(port);
    clocks.shared.retain_coexistence(port);
    clocks
}

#[test]
fn wifi_handoff_keeps_common_power_for_the_next_route_to_restore_once() {
    let cold = WifiPowerBaseline::for_validation(false);
    let mut port = Port::new();
    port.power = cold;
    let wifi = powered_wifi(&mut port);
    port.operations.clear();

    let common = match wifi.into_common(&mut port) {
        Ok(common) => common,
        Err(_) => panic!("a powered Wi-Fi route hands over its common PHY power"),
    };
    // Only Wi-Fi's coexistence lease is released; PHY-I2C and the baseline stay.
    assert_eq!(
        port.operations,
        [Operation::SetGate(SharedModemClockGate::Coexistence, false)]
    );

    // The next route inherits the power; its own later capture must not
    // replace the first route's cold baseline.
    port.operations.clear();
    port.power = WifiPowerBaseline::for_validation(true);
    let mut next = WifiClocks::from_common(common);
    next.power.prepare(&port);
    next.shared.release_all(&mut port);
    assert_eq!(next.power.restore(&mut port), Ok(()));
    assert_eq!(
        port.operations,
        [
            Operation::SetGate(SharedModemClockGate::PhyI2cMaster, false),
            Operation::RestorePower(cold),
        ]
    );
}

#[test]
fn unpowered_routes_cannot_hand_over_common_phy_power() {
    let mut port = Port::new();
    let Err((_clocks, error)) = WifiClocks::default().into_common(&mut port) else {
        panic!("an unpowered Wi-Fi route must keep its clocks");
    };
    assert_eq!(error, CommonPhyPowerError::NotPowered);
    assert!(port.operations.is_empty());
}

#[test]
fn only_the_first_client_runs_the_power_sequence() {
    let cold = WifiPowerBaseline::for_validation(false);
    let mut port = Port::new();
    port.power = cold;
    let mut power = CommonRadioPower::default();

    assert_eq!(power.enter(&mut port, RadioClient::Wifi), Ok(()));
    // A later protocol must not pulse the Wi-Fi resets again.
    port.power = WifiPowerBaseline::for_validation(true);
    assert_eq!(power.enter(&mut port, RadioClient::Bluetooth), Ok(()));
    assert_eq!(
        port.operations,
        [
            Operation::PowerSequence,
            Operation::SetGate(SharedModemClockGate::PhyI2cMaster, true),
        ]
    );
    assert_eq!(
        power.enter(&mut port, RadioClient::Bluetooth),
        Err(CommonRadioPowerError::AlreadyEntered)
    );

    port.operations.clear();
    assert_eq!(power.exit(&mut port, RadioClient::Wifi), Ok(()));
    assert!(port.operations.is_empty());
    assert_eq!(power.exit(&mut port, RadioClient::Bluetooth), Ok(()));
    // The last client restores the baseline captured before the first edge.
    assert_eq!(
        port.operations,
        [
            Operation::SetGate(SharedModemClockGate::PhyI2cMaster, false),
            Operation::RestorePower(cold),
        ]
    );
    assert_eq!(
        power.exit(&mut port, RadioClient::Bluetooth),
        Err(CommonRadioPowerError::NotEntered)
    );
}

#[test]
fn a_failed_power_sequence_admits_no_client() {
    let mut port = Port::new();
    let error = crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::I2cClock,
        expected: true,
        observed: false,
    };
    port.power_sequence_failure = Some(error);
    let mut power = CommonRadioPower::default();
    assert_eq!(
        power.enter(&mut port, RadioClient::Ieee802154),
        Err(CommonRadioPowerError::Power(error))
    );
    assert!(!power.holds(RadioClient::Ieee802154));
    assert_eq!(
        power.exit(&mut port, RadioClient::Ieee802154),
        Err(CommonRadioPowerError::NotEntered)
    );

    port.power_sequence_failure = None;
    assert_eq!(power.enter(&mut port, RadioClient::Ieee802154), Ok(()));
}

#[test]
fn a_failed_restore_keeps_the_last_client_for_retry() {
    let mut port = Port::new();
    let mut power = CommonRadioPower::default();
    assert_eq!(power.enter(&mut port, RadioClient::Wifi), Ok(()));
    port.power_readback = Err(WifiPowerRestoreReadback::ModemSyscon);
    assert_eq!(
        power.exit(&mut port, RadioClient::Wifi),
        Err(CommonRadioPowerError::Restore(
            WifiPowerRestoreCheckpoint::ModemSyscon
        ))
    );
    assert!(power.holds(RadioClient::Wifi));
    port.power_readback = Ok(());
    assert_eq!(power.exit(&mut port, RadioClient::Wifi), Ok(()));
}
