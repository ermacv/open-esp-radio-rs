use std::vec::Vec;

use oer_esp32s31_pac::{
    BLUETOOTH_APB_CLOCKS, BLUETOOTH_CLOCK_COUNT, BLUETOOTH_CONTROLLER_CLOCKS,
    BluetoothClockBaseline, BluetoothLowPowerTimerConfiguration, ModemLowPowerClockDivider,
    ModemLowPowerClockSource, ModemSysconBluetoothClock, PlatformPllSourceBaseline,
    SharedModemClockGate, WifiPowerBaseline, WifiPowerRestoreReadback,
};

use super::{
    BluetoothClocks, BluetoothSysconClocks, ClockPort, PowerEpoch, SharedClockLeases,
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
}

struct Port {
    gates: [bool; 3],
    groups: [BluetoothClockBaseline; BLUETOOTH_CLOCK_COUNT],
    power: WifiPowerBaseline,
    power_readback: Result<(), WifiPowerRestoreReadback>,
    operations: Vec<Operation>,
}

impl Port {
    fn new() -> Self {
        Self {
            gates: [false; 3],
            groups: [BluetoothClockBaseline::default(); BLUETOOTH_CLOCK_COUNT],
            power: WifiPowerBaseline::for_validation(false),
            power_readback: Ok(()),
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
