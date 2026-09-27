use std::vec::Vec;

use oer_esp32s31_pac::{SharedModemClockGate, WifiPowerBaseline, WifiPowerRestoreReadback};

use crate::power::PowerEntry;

use super::{
    ClockPort, CommonRadioPower, CommonRadioPowerError, PowerEpoch, RadioClient, SharedClockLeases,
    WifiPowerRestoreCheckpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    SetGate(SharedModemClockGate, bool),
    RestorePower(WifiPowerBaseline),
    PowerSequence(PowerEntry),
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
        entry: PowerEntry,
    ) -> Result<(), crate::power::PowerError> {
        self.operations.push(Operation::PowerSequence(entry));
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
    leases.retain_phy_i2c(&mut port);
    leases.retain_phy_i2c(&mut port);
    assert_eq!(
        port.operations,
        [Operation::SetGate(SharedModemClockGate::PhyI2cMaster, true)]
    );
    leases.release_all(&mut port);
    assert_eq!(
        port.operations[1..],
        [Operation::SetGate(
            SharedModemClockGate::PhyI2cMaster,
            false
        )]
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
            Operation::PowerSequence(PowerEntry::FirstSinceBoot),
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

#[test]
fn power_up_after_every_client_left_keeps_the_wifi_resets() {
    let mut port = Port::new();
    let mut power = CommonRadioPower::default();

    assert_eq!(power.enter(&mut port, RadioClient::Ieee802154), Ok(()));
    assert_eq!(power.exit(&mut port, RadioClient::Ieee802154), Ok(()));
    assert_eq!(power.enter(&mut port, RadioClient::Ieee802154), Ok(()));
    let sequences: Vec<_> = port
        .operations
        .iter()
        .filter_map(|operation| match operation {
            Operation::PowerSequence(entry) => Some(*entry),
            _ => None,
        })
        .collect();
    assert_eq!(
        sequences,
        [PowerEntry::FirstSinceBoot, PowerEntry::Repeated],
    );
}

#[test]
fn a_failed_first_power_up_still_pulses_the_wifi_resets_on_retry() {
    let mut port = Port::new();
    port.power_sequence_failure = Some(crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ResetReleased,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(power.enter(&mut port, RadioClient::Wifi).is_err());
    port.power_sequence_failure = None;
    assert_eq!(power.enter(&mut port, RadioClient::Wifi), Ok(()));
    assert!(matches!(
        port.operations.as_slice(),
        [
            Operation::PowerSequence(PowerEntry::FirstSinceBoot),
            Operation::PowerSequence(PowerEntry::FirstSinceBoot),
            ..
        ]
    ));
}
