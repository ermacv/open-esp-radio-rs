//! Route-owned MODEM_SYSCON radio clock, reset and baseband control.
//!
//! Register geometry remains in the generated PAC. This module publishes only
//! semantic operations through the affine radio route and preserves every
//! vendor RMW edge and protocol-specific order.

#![forbid(unsafe_code)]

use crate::{
    RadioPhyRegisters,
    generated::{
        ModemSysconClockGateState, ModemSysconEnableState, ModemSysconResetState,
        WifiBaseband40MhzState, WifiBasebandAgcUpdateMode,
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModemSysconPowerObservation {
    pub wifi_reset_released: bool,
    pub active_clock_map_configured: bool,
    pub phy_calibration_clocks_enabled: bool,
    pub phy_i2c_160mhz_selected: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModemSysconIeee802154ClockObservation {
    pub active_clock_map_configured: bool,
    pub wifi_bb_80x1_clock_enabled: bool,
    pub etm_clock_enabled: bool,
    pub bt_apb_clock_enabled: bool,
    pub modem_security_apb_clock_enabled: bool,
    pub common_baseband_clock_enabled: bool,
    pub ieee802154_apb_clock_enabled: bool,
    pub ieee802154_mac_clock_enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModemSysconIeee802154ResetObservation {
    pub mac_reset_released: bool,
    pub apb_reset_released: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModemSysconBluetoothClock {
    WifiBaseband80x1,
    Etm,
    BluetoothMac,
    BluetoothPeripheral,
    BluetoothApb,
    BluetoothBaseband,
}

const BLUETOOTH_CLOCK_COUNT: usize = 6;
const BLUETOOTH_PHYSICAL_CLOCK_COUNT: usize = 11;
const BLUETOOTH_CONTROLLER_CLOCKS: [ModemSysconBluetoothClock; 6] = [
    ModemSysconBluetoothClock::WifiBaseband80x1,
    ModemSysconBluetoothClock::Etm,
    ModemSysconBluetoothClock::BluetoothMac,
    ModemSysconBluetoothClock::BluetoothPeripheral,
    ModemSysconBluetoothClock::BluetoothApb,
    ModemSysconBluetoothClock::BluetoothBaseband,
];
const BLUETOOTH_APB_CLOCKS: [ModemSysconBluetoothClock; 3] = [
    ModemSysconBluetoothClock::Etm,
    ModemSysconBluetoothClock::BluetoothMac,
    ModemSysconBluetoothClock::BluetoothApb,
];

const fn bluetooth_clock_gate_state(enabled: bool) -> ModemSysconClockGateState {
    if enabled {
        ModemSysconClockGateState::Enabled
    } else {
        ModemSysconClockGateState::Disabled
    }
}

const fn modem_syscon_enable_state(enabled: bool) -> ModemSysconEnableState {
    if enabled {
        ModemSysconEnableState::Enabled
    } else {
        ModemSysconEnableState::Disabled
    }
}

const fn modem_syscon_reset_state(asserted: bool) -> ModemSysconResetState {
    if asserted {
        ModemSysconResetState::Asserted
    } else {
        ModemSysconResetState::Released
    }
}

pub(crate) struct BluetoothModemSysconClockState {
    counts: [u8; BLUETOOTH_CLOCK_COUNT],
    baseline: BluetoothClockBaseline,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModemSysconBluetoothObservation {
    pub controller_clocks_enabled: bool,
    pub apb_clocks_enabled: bool,
    pub controller_resets_released: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiBasebandAgcUpdate {
    Initialization,
    RegisterUpdatesEnabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothClockTransition {
    Enable,
    Restore(BluetoothClockBaseline),
    NoChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothPhysicalClock {
    WifiBaseband80x1,
    Etm,
    BluetoothMac,
    ModemSecurity,
    ModemSecurityEcb,
    ModemSecurityCcm,
    ModemSecurityBah,
    BleTimer,
    BluetoothApb,
    ModemSecurityApb,
    BluetoothBaseband,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothClockBaseline([bool; BLUETOOTH_PHYSICAL_CLOCK_COUNT]);

impl Default for BluetoothClockBaseline {
    fn default() -> Self {
        Self([false; BLUETOOTH_PHYSICAL_CLOCK_COUNT])
    }
}

impl BluetoothClockBaseline {
    const fn contains(self, clock: BluetoothPhysicalClock) -> bool {
        self.0[clock as usize]
    }

    fn record(&mut self, clock: BluetoothPhysicalClock, enabled: bool) {
        self.0[clock as usize] = enabled;
    }

    const fn all_enabled(self, clock: ModemSysconBluetoothClock) -> bool {
        let clocks = clock.physical_clocks();
        let mut index = 0;
        while index < clocks.len() {
            if !self.contains(clocks[index]) {
                return false;
            }
            index += 1;
        }
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UnbalancedBluetoothClockRelease;

impl BluetoothModemSysconClockState {
    pub(crate) const fn new() -> Self {
        Self {
            counts: [0; BLUETOOTH_CLOCK_COUNT],
            baseline: BluetoothClockBaseline([false; BLUETOOTH_PHYSICAL_CLOCK_COUNT]),
        }
    }

    fn retain(
        &mut self,
        clock: ModemSysconBluetoothClock,
        observed_baseline: Option<BluetoothClockBaseline>,
    ) -> BluetoothClockTransition {
        let index = clock.index();
        if self.counts[index] == 0 {
            let baseline = observed_baseline.expect("first retain requires one MMIO observation");
            for &physical in clock.physical_clocks() {
                self.baseline.record(physical, baseline.contains(physical));
            }
            self.counts[index] = 1;
            if baseline.all_enabled(clock) {
                BluetoothClockTransition::NoChange
            } else {
                BluetoothClockTransition::Enable
            }
        } else {
            assert!(observed_baseline.is_none());
            self.counts[index] = self.counts[index]
                .checked_add(1)
                .expect("Bluetooth route MODEM_SYSCON reference count cannot overflow");
            BluetoothClockTransition::NoChange
        }
    }

    fn release(
        &mut self,
        clock: ModemSysconBluetoothClock,
    ) -> Result<BluetoothClockTransition, UnbalancedBluetoothClockRelease> {
        let index = clock.index();
        if self.counts[index] == 0 {
            return Err(UnbalancedBluetoothClockRelease);
        }
        self.counts[index] -= 1;
        if self.counts[index] != 0 {
            return Ok(BluetoothClockTransition::NoChange);
        }
        let mut baseline = BluetoothClockBaseline::default();
        for &physical in clock.physical_clocks() {
            baseline.record(physical, self.baseline.contains(physical));
            self.baseline.record(physical, false);
        }
        let transition = if baseline.all_enabled(clock) {
            BluetoothClockTransition::NoChange
        } else {
            BluetoothClockTransition::Restore(baseline)
        };
        Ok(transition)
    }
}

impl ModemSysconBluetoothClock {
    const fn index(self) -> usize {
        match self {
            Self::WifiBaseband80x1 => 0,
            Self::Etm => 1,
            Self::BluetoothMac => 2,
            Self::BluetoothPeripheral => 3,
            Self::BluetoothApb => 4,
            Self::BluetoothBaseband => 5,
        }
    }

    const fn physical_clocks(self) -> &'static [BluetoothPhysicalClock] {
        use BluetoothPhysicalClock as Physical;
        match self {
            Self::WifiBaseband80x1 => &[Physical::WifiBaseband80x1],
            Self::Etm => &[Physical::Etm],
            Self::BluetoothMac => &[Physical::BluetoothMac],
            Self::BluetoothPeripheral => &[
                Physical::ModemSecurity,
                Physical::ModemSecurityEcb,
                Physical::ModemSecurityCcm,
                Physical::ModemSecurityBah,
                Physical::BleTimer,
            ],
            Self::BluetoothApb => &[Physical::BluetoothApb, Physical::ModemSecurityApb],
            Self::BluetoothBaseband => &[Physical::BluetoothBaseband],
        }
    }
}

impl RadioPhyRegisters {
    pub(crate) fn retain_bluetooth_controller_clocks(
        &mut self,
        state: &mut BluetoothModemSysconClockState,
    ) {
        self.retain_bluetooth_clock_set(state, &BLUETOOTH_CONTROLLER_CLOCKS);
    }

    pub(crate) fn retain_bluetooth_apb_clocks(
        &mut self,
        state: &mut BluetoothModemSysconClockState,
    ) {
        self.retain_bluetooth_clock_set(state, &BLUETOOTH_APB_CLOCKS);
    }

    pub(crate) fn release_bluetooth_apb_clocks(
        &mut self,
        state: &mut BluetoothModemSysconClockState,
    ) {
        self.release_bluetooth_clock_set(state, &BLUETOOTH_APB_CLOCKS);
    }

    pub(crate) fn release_bluetooth_controller_clocks(
        &mut self,
        state: &mut BluetoothModemSysconClockState,
    ) {
        self.release_bluetooth_clock_set(state, &BLUETOOTH_CONTROLLER_CLOCKS);
    }

    fn retain_bluetooth_clock_set(
        &mut self,
        state: &mut BluetoothModemSysconClockState,
        clocks: &[ModemSysconBluetoothClock],
    ) {
        self.prepare_modem_syscon_clock_map();
        let baselines = clocks
            .iter()
            .any(|clock| state.counts[clock.index()] == 0)
            .then(|| self.bluetooth_clock_baselines());
        for &clock in clocks {
            let index = clock.index();
            let baseline = (state.counts[index] == 0)
                .then(|| baselines.expect("first group retain sampled the clock registers")[index]);
            if state.retain(clock, baseline) == BluetoothClockTransition::Enable {
                self.set_bluetooth_clock_enabled(clock, true);
            }
        }
    }

    fn release_bluetooth_clock_set(
        &mut self,
        state: &mut BluetoothModemSysconClockState,
        clocks: &[ModemSysconBluetoothClock],
    ) {
        for &clock in clocks {
            let transition = state
                .release(clock)
                .expect("unbalanced Bluetooth MODEM_SYSCON release");
            if let BluetoothClockTransition::Restore(baseline) = transition {
                self.restore_bluetooth_clock(clock, baseline);
            }
        }
    }
    pub(crate) fn prepare_modem_syscon_clock_map(&mut self) {
        crate::generated::prepare_modem_syscon_clock_map(&self.peripherals.modem_syscon_radio);
    }

    pub(crate) fn configure_wifi_power_clock_map(&mut self) {
        self.prepare_modem_syscon_clock_map();
    }

    fn modem_syscon_clock_map_configured(&self) -> bool {
        let (
            zb_map_bit_two,
            frontend_map_bit_one,
            frontend_map_bit_two,
            bluetooth_map_bit_two,
            wifi_map_bit_one,
            wifi_map_bit_two,
            modem_peripheral_map_bit_two,
            modem_apb_map_bit_one,
            modem_apb_map_bit_two,
        ) = crate::svd::field_snapshot_read::observe_modem_syscon_clock_map(
            &self.peripherals.modem_syscon_radio,
        );
        zb_map_bit_two
            && frontend_map_bit_one
            && frontend_map_bit_two
            && bluetooth_map_bit_two
            && wifi_map_bit_one
            && wifi_map_bit_two
            && modem_peripheral_map_bit_two
            && modem_apb_map_bit_one
            && modem_apb_map_bit_two
    }

    pub(crate) fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        crate::generated::set_wifi_baseband_and_mac_reset(
            &self.peripherals.modem_syscon_radio,
            modem_syscon_reset_state(asserted),
        );
    }

    pub(crate) fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        crate::generated::set_wifi_baseband_reset(
            &self.peripherals.modem_syscon_radio,
            modem_syscon_reset_state(asserted),
        );
    }

    pub(crate) fn enable_phy_calibration_clocks(&mut self) {
        crate::generated::enable_phy_calibration_clocks(&self.peripherals.modem_syscon_radio);
    }

    pub(crate) fn select_phy_i2c_160mhz_source(&mut self) {
        crate::generated::select_phy_i2c_160mhz_source(&self.peripherals.modem_syscon_radio);
    }

    pub(crate) fn modem_syscon_power_observation(&self) -> ModemSysconPowerObservation {
        let (wifi_baseband_reset, wifi_mac_reset) =
            crate::svd::field_snapshot_read::observe_wifi_modem_resets(
                &self.peripherals.modem_syscon_radio,
            );
        let (
            wifi_bb_22m,
            wifi_bb_40m,
            wifi_bb_44m,
            wifi_bb_80m,
            wifi_bb_40x,
            wifi_bb_80x,
            wifi_bb_40x1,
            wifi_bb_80x1,
            wifi_bb_160x1,
            wifi_apb,
            frontend_80m,
            frontend_160m,
            frontend_apb,
            bluetooth_apb,
            bluetooth_baseband,
            frontend_power_detector_adc,
            frontend_adc,
            frontend_dac,
        ) = crate::svd::field_snapshot_read::observe_phy_calibration_clocks(
            &self.peripherals.modem_syscon_radio,
        );
        ModemSysconPowerObservation {
            wifi_reset_released: !wifi_baseband_reset && !wifi_mac_reset,
            active_clock_map_configured: self.modem_syscon_clock_map_configured(),
            phy_calibration_clocks_enabled: wifi_bb_22m
                && wifi_bb_40m
                && wifi_bb_44m
                && wifi_bb_80m
                && wifi_bb_40x
                && wifi_bb_80x
                && wifi_bb_40x1
                && wifi_bb_80x1
                && wifi_bb_160x1
                && wifi_apb
                && frontend_80m
                && frontend_160m
                && frontend_apb
                && bluetooth_apb
                && bluetooth_baseband
                && frontend_power_detector_adc
                && frontend_adc
                && frontend_dac,
            phy_i2c_160mhz_selected: crate::svd::field_read::observe_phy_i2c_160mhz_source(
                &self.peripherals.modem_syscon_radio,
            ),
        }
    }

    pub(crate) fn enable_wifi_mac_clocks(&mut self) {
        crate::generated::enable_wifi_mac_clocks(&self.peripherals.modem_syscon_radio);
    }

    pub(crate) fn set_wifi_mac_reset(&mut self, asserted: bool) {
        crate::generated::set_wifi_mac_reset(
            &self.peripherals.modem_syscon_radio,
            modem_syscon_reset_state(asserted),
        );
    }

    pub fn clear_cold_start_wifi_control(&mut self) {
        crate::generated::clear_cold_start_wifi_control(&self.peripherals.modem_syscon_radio);
    }

    pub fn wifi_baseband_is_enabled(&self) -> bool {
        crate::svd::field_read::observe_wifi_baseband_enable(&self.peripherals.modem_syscon_radio)
    }

    pub fn set_wifi_baseband_enabled(&mut self, enabled: bool) {
        crate::generated::set_wifi_baseband_enable(
            &self.peripherals.modem_syscon_radio,
            modem_syscon_enable_state(enabled),
        );
    }

    pub fn set_bss_cbw_40_digital(&mut self, enabled: bool) {
        let state = if enabled {
            WifiBaseband40MhzState::Enabled
        } else {
            WifiBaseband40MhzState::Disabled
        };
        crate::generated::set_wifi_baseband_40mhz_state(
            &self.peripherals.modem_syscon_radio,
            state,
        );
    }

    pub fn set_bb_agc_update_mode(&mut self, mode: WifiBasebandAgcUpdate) {
        let mode = match mode {
            WifiBasebandAgcUpdate::Initialization => WifiBasebandAgcUpdateMode::Initialization,
            WifiBasebandAgcUpdate::RegisterUpdatesEnabled => {
                WifiBasebandAgcUpdateMode::RegisterUpdatesEnabled
            }
        };
        crate::generated::set_wifi_baseband_agc_update_mode(
            &self.peripherals.modem_syscon_radio,
            mode,
        );
    }

    pub fn set_mac_baseband_enabled(&mut self, enabled: bool) {
        crate::generated::set_mac_baseband_enable(
            &self.peripherals.modem_syscon_radio,
            modem_syscon_enable_state(enabled),
        );
    }

    pub fn enable_mac_baseband(&mut self) {
        self.set_mac_baseband_enabled(true);
        self.set_wifi_baseband_enabled(false);
        self.set_wifi_baseband_enabled(true);
    }

    pub(crate) fn configure_ieee802154_modem_clock_maps(&mut self) {
        let registers = &self.peripherals.modem_syscon_radio;
        crate::generated::prepare_ieee802154_modem_apb_clock_map(registers);
        crate::generated::prepare_ieee802154_modem_peripheral_clock_map(registers);
        crate::generated::prepare_ieee802154_wifi_clock_map(registers);
        crate::generated::prepare_ieee802154_bluetooth_clock_map(registers);
        crate::generated::prepare_ieee802154_frontend_clock_map(registers);
        crate::generated::prepare_ieee802154_bluetooth_clock_map(registers);
        crate::generated::prepare_ieee802154_clock_map(registers);
    }

    pub(crate) fn enable_ieee802154_wifi_bb_clock(&mut self) {
        crate::generated::enable_ieee802154_wifi_baseband_clock(
            &self.peripherals.modem_syscon_radio,
        );
    }
    pub(crate) fn enable_ieee802154_etm_clock(&mut self) {
        crate::generated::enable_ieee802154_etm_clock(&self.peripherals.modem_syscon_radio);
    }
    pub(crate) fn enable_ieee802154_bt_apb_clocks(&mut self) {
        crate::generated::enable_ieee802154_bluetooth_apb_clock(
            &self.peripherals.modem_syscon_radio,
        );
        crate::generated::enable_ieee802154_modem_security_apb_clock(
            &self.peripherals.modem_syscon_radio,
        );
    }
    pub(crate) fn enable_ieee802154_common_baseband_clock(&mut self) {
        crate::generated::enable_ieee802154_common_baseband_clock(
            &self.peripherals.modem_syscon_radio,
        );
    }
    pub(crate) fn enable_ieee802154_mac_clocks(&mut self) {
        crate::generated::enable_ieee802154_apb_clock(&self.peripherals.modem_syscon_radio);
        crate::generated::enable_ieee802154_mac_clock(&self.peripherals.modem_syscon_radio);
    }

    pub(crate) fn ieee802154_clock_observation(&self) -> ModemSysconIeee802154ClockObservation {
        let (
            etm_clock_enabled,
            modem_security_apb_clock_enabled,
            ieee802154_apb_clock_enabled,
            ieee802154_mac_clock_enabled,
        ) = crate::svd::field_snapshot_read::observe_ieee802154_modem_clock_conf(
            &self.peripherals.modem_syscon_radio,
        );
        let (wifi_bb_80x1_clock_enabled, bt_apb_clock_enabled, common_baseband_clock_enabled) =
            crate::svd::field_snapshot_read::observe_ieee802154_modem_clock_conf1(
                &self.peripherals.modem_syscon_radio,
            );
        ModemSysconIeee802154ClockObservation {
            active_clock_map_configured: self.modem_syscon_clock_map_configured(),
            wifi_bb_80x1_clock_enabled,
            etm_clock_enabled,
            bt_apb_clock_enabled,
            modem_security_apb_clock_enabled,
            common_baseband_clock_enabled,
            ieee802154_apb_clock_enabled,
            ieee802154_mac_clock_enabled,
        }
    }

    pub(crate) fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        crate::generated::set_ieee802154_mac_reset(
            &self.peripherals.modem_syscon_radio,
            modem_syscon_reset_state(asserted),
        );
    }

    pub(crate) fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        crate::generated::set_ieee802154_apb_reset(
            &self.peripherals.modem_syscon_radio,
            modem_syscon_reset_state(asserted),
        );
    }

    pub(crate) fn ieee802154_reset_observation(&self) -> ModemSysconIeee802154ResetObservation {
        let (mac_reset, apb_reset) =
            crate::svd::field_snapshot_read::observe_ieee802154_modem_resets(
                &self.peripherals.modem_syscon_radio,
            );
        ModemSysconIeee802154ResetObservation {
            mac_reset_released: !mac_reset,
            apb_reset_released: !apb_reset,
        }
    }

    fn bluetooth_clock_baselines(&self) -> [BluetoothClockBaseline; BLUETOOTH_CLOCK_COUNT] {
        let (
            etm_enabled,
            modem_security_enabled,
            modem_security_ecb_enabled,
            modem_security_ccm_enabled,
            modem_security_bah_enabled,
            ble_timer_enabled,
            modem_security_apb_enabled,
        ) = crate::svd::field_snapshot_read::observe_bluetooth_modem_clock_conf(
            &self.peripherals.modem_syscon_radio,
        );
        let (
            wifi_baseband_80x1_enabled,
            bluetooth_mac_enabled,
            bluetooth_apb_enabled,
            bluetooth_baseband_enabled,
        ) = crate::svd::field_snapshot_read::observe_bluetooth_modem_clock_conf1(
            &self.peripherals.modem_syscon_radio,
        );
        let mut baseline = BluetoothClockBaseline::default();
        baseline.record(
            BluetoothPhysicalClock::WifiBaseband80x1,
            wifi_baseband_80x1_enabled,
        );
        baseline.record(BluetoothPhysicalClock::Etm, etm_enabled);
        baseline.record(BluetoothPhysicalClock::BluetoothMac, bluetooth_mac_enabled);
        baseline.record(
            BluetoothPhysicalClock::ModemSecurity,
            modem_security_enabled,
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityEcb,
            modem_security_ecb_enabled,
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityCcm,
            modem_security_ccm_enabled,
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityBah,
            modem_security_bah_enabled,
        );
        baseline.record(BluetoothPhysicalClock::BleTimer, ble_timer_enabled);
        baseline.record(BluetoothPhysicalClock::BluetoothApb, bluetooth_apb_enabled);
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityApb,
            modem_security_apb_enabled,
        );
        baseline.record(
            BluetoothPhysicalClock::BluetoothBaseband,
            bluetooth_baseband_enabled,
        );
        let mut baselines = [BluetoothClockBaseline::default(); BLUETOOTH_CLOCK_COUNT];
        for logical in BLUETOOTH_CONTROLLER_CLOCKS {
            for &physical in logical.physical_clocks() {
                baselines[logical.index()].record(physical, baseline.contains(physical));
            }
        }
        baselines
    }

    fn set_bluetooth_clock_enabled(&mut self, device: ModemSysconBluetoothClock, enabled: bool) {
        let state = bluetooth_clock_gate_state(enabled);
        let registers = &self.peripherals.modem_syscon_radio;
        match device {
            ModemSysconBluetoothClock::WifiBaseband80x1 => {
                crate::generated::set_bluetooth_wifi_baseband_80x1_clock(registers, state);
            }
            ModemSysconBluetoothClock::Etm => {
                crate::generated::set_bluetooth_etm_clock(registers, state);
            }
            ModemSysconBluetoothClock::BluetoothMac => {
                crate::generated::set_bluetooth_mac_clock(registers, state);
            }
            ModemSysconBluetoothClock::BluetoothPeripheral => {
                crate::generated::set_bluetooth_peripheral_clocks(registers, state);
            }
            ModemSysconBluetoothClock::BluetoothApb => {
                crate::generated::set_bluetooth_apb_clock(registers, state);
                crate::generated::set_bluetooth_modem_security_apb_clock(registers, state);
            }
            ModemSysconBluetoothClock::BluetoothBaseband => {
                crate::generated::set_bluetooth_baseband_clock(registers, state);
            }
        }
    }

    fn restore_bluetooth_clock(
        &mut self,
        device: ModemSysconBluetoothClock,
        baseline: BluetoothClockBaseline,
    ) {
        match device {
            ModemSysconBluetoothClock::BluetoothPeripheral => {
                crate::generated::restore_bluetooth_peripheral_clocks(
                    &self.peripherals.modem_syscon_radio,
                    baseline.contains(BluetoothPhysicalClock::ModemSecurity),
                    baseline.contains(BluetoothPhysicalClock::ModemSecurityEcb),
                    baseline.contains(BluetoothPhysicalClock::ModemSecurityCcm),
                    baseline.contains(BluetoothPhysicalClock::ModemSecurityBah),
                    baseline.contains(BluetoothPhysicalClock::BleTimer),
                );
            }
            ModemSysconBluetoothClock::BluetoothApb => {
                crate::generated::set_bluetooth_apb_clock(
                    &self.peripherals.modem_syscon_radio,
                    bluetooth_clock_gate_state(
                        baseline.contains(BluetoothPhysicalClock::BluetoothApb),
                    ),
                );
                crate::generated::set_bluetooth_modem_security_apb_clock(
                    &self.peripherals.modem_syscon_radio,
                    bluetooth_clock_gate_state(
                        baseline.contains(BluetoothPhysicalClock::ModemSecurityApb),
                    ),
                );
            }
            _ => self.set_bluetooth_clock_enabled(device, baseline.all_enabled(device)),
        }
    }

    pub(crate) fn bluetooth_clock_observation(&self) -> ModemSysconBluetoothObservation {
        let clocks = self.bluetooth_clock_baselines();
        ModemSysconBluetoothObservation {
            controller_clocks_enabled: BLUETOOTH_CONTROLLER_CLOCKS
                .into_iter()
                .all(|clock| clocks[clock.index()].all_enabled(clock)),
            apb_clocks_enabled: BLUETOOTH_APB_CLOCKS
                .into_iter()
                .all(|clock| clocks[clock.index()].all_enabled(clock)),
            controller_resets_released: self.bluetooth_controller_resets_released(),
        }
    }

    pub(crate) fn reset_bluetooth_controller_domains(&mut self) {
        let registers = &self.peripherals.modem_syscon_radio;
        crate::generated::set_bluetooth_mac_reset(registers, ModemSysconResetState::Asserted);
        crate::generated::set_bluetooth_mac_reset(registers, ModemSysconResetState::Released);
        crate::generated::set_bluetooth_mac_apb_reset(registers, ModemSysconResetState::Asserted);
        crate::generated::set_bluetooth_mac_apb_reset(registers, ModemSysconResetState::Released);
        crate::generated::set_bluetooth_timer_reset(registers, ModemSysconResetState::Asserted);
        crate::generated::set_bluetooth_timer_reset(registers, ModemSysconResetState::Released);
        crate::generated::set_bluetooth_modem_ecb_reset(registers, ModemSysconResetState::Asserted);
        crate::generated::set_bluetooth_modem_ccm_reset(registers, ModemSysconResetState::Asserted);
        crate::generated::set_bluetooth_modem_bah_reset(registers, ModemSysconResetState::Asserted);
        crate::generated::set_bluetooth_modem_security_reset(
            registers,
            ModemSysconResetState::Asserted,
        );
        crate::generated::set_bluetooth_modem_ecb_reset(registers, ModemSysconResetState::Released);
        crate::generated::set_bluetooth_modem_ccm_reset(registers, ModemSysconResetState::Released);
        crate::generated::set_bluetooth_modem_bah_reset(registers, ModemSysconResetState::Released);
        crate::generated::set_bluetooth_modem_security_reset(
            registers,
            ModemSysconResetState::Released,
        );
    }

    pub(crate) fn bluetooth_controller_resets_released(&self) -> bool {
        let (
            bluetooth_mac_reset,
            bluetooth_mac_apb_reset,
            bluetooth_timer_reset,
            modem_ecb_reset,
            modem_ccm_reset,
            modem_bah_reset,
            modem_security_reset,
        ) = crate::svd::field_snapshot_read::observe_bluetooth_controller_resets(
            &self.peripherals.modem_syscon_radio,
        );
        !bluetooth_mac_reset
            && !bluetooth_mac_apb_reset
            && !bluetooth_timer_reset
            && !modem_ecb_reset
            && !modem_ccm_reset
            && !modem_bah_reset
            && !modem_security_reset
    }
}

#[cfg(test)]
mod tests;
