//! Route-owned MODEM_LPCON shared-clock capability.
//!
//! The register block travels with the mutually exclusive Wi-Fi, Bluetooth
//! and IEEE 802.15.4 route. Public callers receive observations and semantic
//! route operations, never a register handle or a droppable clock token.

#![forbid(unsafe_code)]

use crate::{RadioPhyRegisters, generated::ModemLowPowerClockDivider};

/// One MODEM_LPCON shared clock gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedModemClockGate {
    Coexistence,
    PhyI2cMaster,
    LowPowerTimer,
}

/// Semantic route-owned observation used by protocol clock checkpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedModemClockObservation {
    pub power_state_map_configured: bool,
    pub coexistence_clock_enabled: bool,
    pub phy_i2c_master_clock_enabled: bool,
    pub low_power_timer_clock_enabled: bool,
}

/// Reviewed selector decoded inside the PAC from `COEX_LP_CLK_CONF`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexistenceLowPowerClockSource {
    Selector1,
    Selector2,
    Selector4,
    Selector8,
}

/// Semantic result of the vendor-required two-read coexistence sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexistenceLowPowerClockObservation {
    pub source: CoexistenceLowPowerClockSource,
    pub divider_minus_one: u16,
}

/// Semantic Bluetooth low-power-clock observation without register authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLowPowerClockObservation {
    pub exclusive_main_xtal_selected: bool,
    pub bluetooth_divider_configured: bool,
    pub timer_enabled: bool,
}

impl RadioPhyRegisters {
    #[doc(hidden)]
    pub fn prepare_shared_modem_clock_map(&mut self) {
        // This is the vendor's monotonic global ICG-map initialization, not
        // lease-owned state. It preserves the existing image and is therefore
        // intentionally not rolled back when an individual route is released.
        crate::generated::initialize_shared_modem_power_state_map(
            &self.peripherals.modem_lpcon_shared_clock,
        );
    }

    #[doc(hidden)]
    pub fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let (
            coexistence_clock_enabled,
            phy_i2c_master_clock_enabled,
            low_power_timer_clock_enabled,
        ) = crate::svd::field_snapshot_read::observe_shared_modem_clock_gates(registers);
        let (
            wifi_power_map_bit_one,
            wifi_power_map_bit_two,
            coexistence_map_bit_one,
            coexistence_map_bit_two,
            phy_i2c_map_bit_one,
            phy_i2c_map_bit_two,
            low_power_apb_map_bit_one,
            low_power_apb_map_bit_two,
        ) = crate::svd::field_snapshot_read::observe_shared_modem_power_state_map(registers);
        SharedModemClockObservation {
            power_state_map_configured: wifi_power_map_bit_one
                && wifi_power_map_bit_two
                && coexistence_map_bit_one
                && coexistence_map_bit_two
                && phy_i2c_map_bit_one
                && phy_i2c_map_bit_two
                && low_power_apb_map_bit_one
                && low_power_apb_map_bit_two,
            coexistence_clock_enabled,
            phy_i2c_master_clock_enabled,
            low_power_timer_clock_enabled,
        }
    }

    /// Select the coexistence timer clock and its divider, as complete
    /// `coex_hw_timer_freq_set` does: the one-hot source first, then the
    /// divider, each through its own fresh-read RMW.
    #[doc(hidden)]
    pub fn configure_coexistence_timer_clock(
        &mut self,
        source: crate::CoexTimerClockSource,
        divider_minus_one: crate::CoexTimerClockDividerMinusOne,
    ) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        crate::generated::select_coex_timer_clock_source(registers, source);
        crate::generated::set_coex_timer_clock_divider(registers, divider_minus_one);
    }

    #[doc(hidden)]
    pub fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let source =
            match crate::svd::field_snapshot_read::observe_coexistence_low_power_clock_source(
                registers,
            ) {
                (true, false, false, false) => CoexistenceLowPowerClockSource::Selector1,
                (false, true, false, false) => CoexistenceLowPowerClockSource::Selector2,
                (false, false, true, false) => CoexistenceLowPowerClockSource::Selector4,
                (false, false, false, true) => CoexistenceLowPowerClockSource::Selector8,
                _ => return None,
            };
        Some(CoexistenceLowPowerClockObservation {
            source,
            divider_minus_one: crate::svd::field_read::observe_coexistence_low_power_clock_divider(
                registers,
            ),
        })
    }

    /// Read whether one shared clock gate is enabled.
    #[doc(hidden)]
    pub fn shared_modem_clock_gate_enabled(&self, gate: SharedModemClockGate) -> bool {
        let (coexistence, phy_i2c_master, low_power_timer) =
            crate::svd::field_snapshot_read::observe_shared_modem_clock_gates(
                &self.peripherals.modem_lpcon_shared_clock,
            );
        match gate {
            SharedModemClockGate::Coexistence => coexistence,
            SharedModemClockGate::PhyI2cMaster => phy_i2c_master,
            SharedModemClockGate::LowPowerTimer => low_power_timer,
        }
    }

    /// Enable or disable one shared clock gate.
    #[doc(hidden)]
    pub fn set_shared_modem_clock_gate(&mut self, gate: SharedModemClockGate, enabled: bool) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        match (gate, enabled) {
            (SharedModemClockGate::Coexistence, true) => {
                crate::generated::enable_shared_modem_coexistence_clock(registers);
            }
            (SharedModemClockGate::Coexistence, false) => {
                crate::generated::disable_shared_modem_coexistence_clock(registers);
            }
            (SharedModemClockGate::PhyI2cMaster, true) => {
                crate::generated::enable_shared_modem_phy_i2c_master_clock(registers);
            }
            (SharedModemClockGate::PhyI2cMaster, false) => {
                crate::generated::disable_shared_modem_phy_i2c_master_clock(registers);
            }
            (SharedModemClockGate::LowPowerTimer, true) => {
                crate::generated::enable_shared_modem_low_power_timer_clock(registers);
            }
            (SharedModemClockGate::LowPowerTimer, false) => {
                crate::generated::disable_shared_modem_low_power_timer_clock(registers);
            }
        }
    }

    /// Deselect every Bluetooth low-power timer source.
    ///
    /// This is `modem_clock_hal_deselect_all_ble_rtc_timer_lpclk_source`: one
    /// write per selector, in the vendor's slow-oscillator, fast-oscillator,
    /// 32-kHz crystal, main-crystal order. The divider is untouched.
    #[doc(hidden)]
    pub fn deselect_bluetooth_low_power_timer_sources(&mut self) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        crate::generated::deselect_bluetooth_low_power_timer_slow_oscillator(registers);
        crate::generated::deselect_bluetooth_low_power_timer_fast_oscillator(registers);
        crate::generated::deselect_bluetooth_low_power_timer_crystal_32khz(registers);
        crate::generated::deselect_bluetooth_low_power_timer_crystal(registers);
    }

    /// Select the main crystal as the only Bluetooth low-power timer source
    /// and program its divider.
    ///
    /// This is the source and divider half of
    /// `modem_clock_select_lp_clock_source(PERIPH_BT_MODULE, MAIN_XTAL, divider)`:
    /// every selector is cleared, the main crystal is selected and the
    /// divider written, each as its own field write.
    #[doc(hidden)]
    pub fn select_bluetooth_low_power_timer_main_crystal(
        &mut self,
        divider: ModemLowPowerClockDivider,
    ) {
        self.deselect_bluetooth_low_power_timer_sources();
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        crate::generated::select_bluetooth_low_power_timer_crystal(registers);
        crate::generated::set_bluetooth_low_power_timer_divider(registers, divider);
    }

    #[doc(hidden)]
    pub fn bluetooth_low_power_clock_observation(&self) -> BluetoothLowPowerClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let (
            slow_oscillator_selected,
            fast_oscillator_selected,
            crystal_selected,
            crystal_32khz_selected,
            divider_minus_one,
        ) = crate::svd::field_snapshot_read::observe_bluetooth_low_power_timer_configuration(
            registers,
        );
        BluetoothLowPowerClockObservation {
            exclusive_main_xtal_selected: !slow_oscillator_selected
                && !fast_oscillator_selected
                && crystal_selected
                && !crystal_32khz_selected,
            bluetooth_divider_configured: u32::from(divider_minus_one)
                == crate::BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER.get(),
            timer_enabled: self
                .shared_modem_clock_gate_enabled(SharedModemClockGate::LowPowerTimer),
        }
    }
}
