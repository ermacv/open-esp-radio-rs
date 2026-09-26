use std::vec::Vec;

use oer_esp32s31_pac::{
    BLUETOOTH_APB_CLOCKS, BLUETOOTH_CLOCK_COUNT, BLUETOOTH_CONTROLLER_CLOCKS,
    BluetoothClockBaseline, BluetoothLowPowerTimerConfiguration, ModemLowPowerClockDivider,
    ModemLowPowerClockSource, ModemSysconBluetoothClock, PlatformPllSourceBaseline,
    SharedModemClockGate, WifiPowerBaseline, WifiPowerRestoreReadback,
};

use super::{
    BluetoothClocks, BluetoothSysconClocks, ClockPort, CommonPhyPowerError, CommonRadioPower,
    CommonRadioPowerError, PowerEpoch, RadioClient, SharedClockLeases, WifiClocks,
    WifiPowerRestoreCheckpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    SetGate(SharedModemClockGate, bool),
    ConfigureTimer,
    RestoreTimer,
    ConfigurePll,
    RestorePll,
    RestorePower(WifiPowerBaseline),
    PrepareMap,
    EnableGroup(ModemSysconBluetoothClock),
    RestoreGroup(ModemSysconBluetoothClock),
    PowerSequence,
}

struct Port {
    gates: [bool; 3],
    groups: [BluetoothClockBaseline; BLUETOOTH_CLOCK_COUNT],
    power: WifiPowerBaseline,
    power_readback: Result<(), WifiPowerRestoreReadback>,
    power_sequence_failure: Option<crate::power::PowerError>,
    operations: Vec<Operation>,
}

impl Port {
    fn new() -> Self {
        Self {
            gates: [false; 3],
            groups: [BluetoothClockBaseline::default(); BLUETOOTH_CLOCK_COUNT],
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
    fn low_power_timer_configuration(&self) -> BluetoothLowPowerTimerConfiguration {
        BluetoothLowPowerTimerConfiguration::default()
    }
    fn configure_low_power_timer(
        &mut self,
        source: ModemLowPowerClockSource,
        _divider: ModemLowPowerClockDivider,
    ) {
        assert_eq!(source, ModemLowPowerClockSource::Crystal);
        self.operations.push(Operation::ConfigureTimer);
    }
    fn restore_low_power_timer(&mut self, _configuration: BluetoothLowPowerTimerConfiguration) {
        self.operations.push(Operation::RestoreTimer);
    }
    fn pll_source_baseline(&self) -> PlatformPllSourceBaseline {
        PlatformPllSourceBaseline::default()
    }
    fn configure_modem_source_clocks(&mut self) {
        self.operations.push(Operation::ConfigurePll);
    }
    fn restore_pll_source(&mut self, _baseline: PlatformPllSourceBaseline) {
        self.operations.push(Operation::RestorePll);
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
    fn prepare_syscon_clock_map(&mut self) {
        self.operations.push(Operation::PrepareMap);
    }
    fn bluetooth_clock_baselines(&self) -> [BluetoothClockBaseline; BLUETOOTH_CLOCK_COUNT] {
        self.groups
    }
    fn enable_bluetooth_clock(&mut self, clock: ModemSysconBluetoothClock) {
        self.operations.push(Operation::EnableGroup(clock));
    }
    fn restore_bluetooth_clock(
        &mut self,
        clock: ModemSysconBluetoothClock,
        _baseline: BluetoothClockBaseline,
    ) {
        self.operations.push(Operation::RestoreGroup(clock));
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
    assert_eq!(epoch.restore(&mut port, false), Ok(()));
    assert_eq!(port.operations, [Operation::RestorePower(original)]);

    epoch.prepare(&port);
    assert_eq!(epoch.restore(&mut port, false), Ok(()));
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

    assert_eq!(
        epoch.restore(&mut port, true),
        Err(WifiPowerRestoreCheckpoint::PlatformPllLease)
    );
    assert!(port.operations.is_empty());

    port.power_readback = Err(WifiPowerRestoreReadback::ModemSourceClocks);
    assert_eq!(
        epoch.restore(&mut port, false),
        Err(WifiPowerRestoreCheckpoint::ModemSourceClocks)
    );
    port.power_readback = Ok(());
    assert_eq!(epoch.restore(&mut port, false), Ok(()));
    assert_eq!(port.operations.len(), 2);
}

#[test]
fn overlapping_apb_release_keeps_controller_dependencies_retained() {
    let mut port = Port::new();
    let mut clocks = BluetoothSysconClocks::default();
    clocks.retain_set(&mut port, &BLUETOOTH_CONTROLLER_CLOCKS);
    clocks.retain_set(&mut port, &BLUETOOTH_APB_CLOCKS);
    let enabled = port
        .operations
        .iter()
        .filter(|operation| matches!(operation, Operation::EnableGroup(_)))
        .count();
    assert_eq!(enabled, BLUETOOTH_CONTROLLER_CLOCKS.len());

    port.operations.clear();
    clocks.release_set(&mut port, &BLUETOOTH_APB_CLOCKS);
    assert!(port.operations.is_empty());
    clocks.release_set(&mut port, &BLUETOOTH_CONTROLLER_CLOCKS);
    assert_eq!(
        port.operations,
        BLUETOOTH_CONTROLLER_CLOCKS.map(Operation::RestoreGroup)
    );
}

#[test]
fn preexisting_clock_group_is_neither_enabled_nor_restored() {
    let mut port = Port::new();
    port.groups = [BluetoothClockBaseline::all_enabled_for_validation(); BLUETOOTH_CLOCK_COUNT];
    let mut clocks = BluetoothSysconClocks::default();
    clocks.retain_set(&mut port, &BLUETOOTH_APB_CLOCKS);
    clocks.release_set(&mut port, &BLUETOOTH_APB_CLOCKS);
    assert_eq!(port.operations, [Operation::PrepareMap]);
}

#[test]
#[should_panic(expected = "unbalanced Bluetooth MODEM_SYSCON release")]
fn unbalanced_clock_group_release_is_rejected() {
    let mut port = Port::new();
    BluetoothSysconClocks::default().release_set(&mut port, &BLUETOOTH_APB_CLOCKS);
}

#[test]
fn bluetooth_route_releases_every_lease_in_teardown_order() {
    let mut port = Port::new();
    let mut clocks = BluetoothClocks::default();
    clocks.retain_platform_pll_source(&mut port);
    clocks.retain_syscon_controller_clocks(&mut port);
    clocks.retain_syscon_apb_clocks(&mut port);
    clocks.retain_coexistence(&mut port);
    clocks.retain_main_xtal_low_power_timer(&mut port);
    clocks.shared_mut().retain_phy_i2c(&mut port);
    port.operations.clear();

    clocks.release_all(&mut port);
    assert_eq!(
        port.operations,
        [
            Operation::SetGate(SharedModemClockGate::PhyI2cMaster, false),
            Operation::RestoreTimer,
            Operation::SetGate(SharedModemClockGate::LowPowerTimer, false),
            Operation::SetGate(SharedModemClockGate::Coexistence, false),
        ]
        .into_iter()
        .chain(BLUETOOTH_CONTROLLER_CLOCKS.map(Operation::RestoreGroup))
        .chain([Operation::RestorePll])
        .collect::<Vec<_>>()
    );
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
fn wifi_handoff_keeps_common_power_for_bluetooth_to_restore_once() {
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

    // Bluetooth inherits the power; its own later capture must not replace
    // the first route's cold baseline.
    port.operations.clear();
    port.power = WifiPowerBaseline::for_validation(true);
    let mut bluetooth = BluetoothClocks::from_common(common);
    assert!(bluetooth.common_inherited());
    bluetooth.prepare_power_epoch(&port);
    bluetooth.retain_platform_pll_source(&mut port);
    bluetooth.retain_coexistence(&mut port);
    bluetooth.release_all(&mut port);
    assert_eq!(bluetooth.restore_power_epoch(&mut port), Ok(()));
    assert_eq!(
        port.operations,
        [
            Operation::ConfigurePll,
            Operation::SetGate(SharedModemClockGate::Coexistence, true),
            Operation::SetGate(SharedModemClockGate::PhyI2cMaster, false),
            Operation::SetGate(SharedModemClockGate::Coexistence, false),
            Operation::RestorePll,
            Operation::RestorePower(cold),
        ]
    );
}

#[test]
fn bluetooth_handoff_releases_its_own_leases_and_supersedes_the_pll_source() {
    let cold = WifiPowerBaseline::for_validation(false);
    let mut port = Port::new();
    port.power = cold;
    let mut clocks = BluetoothClocks::default();
    clocks.prepare_power_epoch(&port);
    clocks.retain_platform_pll_source(&mut port);
    clocks.retain_syscon_controller_clocks(&mut port);
    clocks.retain_syscon_apb_clocks(&mut port);
    clocks.retain_coexistence(&mut port);
    clocks.retain_main_xtal_low_power_timer(&mut port);
    clocks.shared_mut().retain_phy_i2c(&mut port);
    port.operations.clear();

    let common = match clocks.into_common(&mut port) {
        Ok(common) => common,
        Err(_) => panic!("a powered Bluetooth route hands over its common PHY power"),
    };
    assert_eq!(
        port.operations,
        [
            Operation::RestoreTimer,
            Operation::SetGate(SharedModemClockGate::LowPowerTimer, false),
        ]
        .into_iter()
        .chain(BLUETOOTH_CONTROLLER_CLOCKS.map(Operation::RestoreGroup))
        .chain([Operation::SetGate(SharedModemClockGate::Coexistence, false)])
        .collect::<Vec<_>>()
    );

    // Wi-Fi inherits the power and restores the PHY-I2C gate and the cold
    // baseline once when it finally returns the cold root.
    port.operations.clear();
    let mut wifi = WifiClocks::from_common(common);
    wifi.shared.release_all(&mut port);
    assert_eq!(wifi.power.restore(&mut port, false), Ok(()));
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
    let Err((_clocks, error)) = BluetoothClocks::default().into_common(&mut port) else {
        panic!("an unpowered Bluetooth route must keep its clocks");
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
