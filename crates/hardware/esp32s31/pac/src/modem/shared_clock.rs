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

/// Reviewed Bluetooth low-power timer sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemLowPowerClockSource {
    SlowOscillator,
    FastOscillator,
    Crystal,
    Crystal32Khz,
}

/// Complete Bluetooth low-power timer source and divider configuration.
///
/// The value is an opaque readback used to restore the exact configuration
/// captured before a route changed it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothLowPowerTimerConfiguration {
    slow_oscillator_selected: bool,
    fast_oscillator_selected: bool,
    crystal_selected: bool,
    crystal_32khz_selected: bool,
    divider_minus_one: u16,
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

    /// Read the complete Bluetooth low-power timer configuration.
    #[doc(hidden)]
    pub fn bluetooth_low_power_timer_configuration(&self) -> BluetoothLowPowerTimerConfiguration {
        let (
            slow_oscillator_selected,
            fast_oscillator_selected,
            crystal_selected,
            crystal_32khz_selected,
            divider_minus_one,
        ) = crate::svd::field_snapshot_read::observe_bluetooth_low_power_timer_configuration(
            &self.peripherals.modem_lpcon_shared_clock,
        );
        BluetoothLowPowerTimerConfiguration {
            slow_oscillator_selected,
            fast_oscillator_selected,
            crystal_selected,
            crystal_32khz_selected,
            divider_minus_one,
        }
    }

    /// Select one Bluetooth low-power timer source and divider.
    #[doc(hidden)]
    pub fn configure_bluetooth_low_power_timer(
        &mut self,
        source: ModemLowPowerClockSource,
        divider: ModemLowPowerClockDivider,
    ) {
        crate::generated::configure_shared_modem_low_power_timer(
            &self.peripherals.modem_lpcon_shared_clock,
            source == ModemLowPowerClockSource::SlowOscillator,
            source == ModemLowPowerClockSource::FastOscillator,
            source == ModemLowPowerClockSource::Crystal,
            source == ModemLowPowerClockSource::Crystal32Khz,
            divider,
        );
    }

    /// Restore one configuration captured by
    /// [`Self::bluetooth_low_power_timer_configuration`].
    #[doc(hidden)]
    pub fn restore_bluetooth_low_power_timer_configuration(
        &mut self,
        configuration: BluetoothLowPowerTimerConfiguration,
    ) {
        let divider = ModemLowPowerClockDivider::new(u32::from(configuration.divider_minus_one))
            .expect("generated twelve-bit LP timer readback must fit its write domain");
        crate::generated::configure_shared_modem_low_power_timer(
            &self.peripherals.modem_lpcon_shared_clock,
            configuration.slow_oscillator_selected,
            configuration.fast_oscillator_selected,
            configuration.crystal_selected,
            configuration.crystal_32khz_selected,
            divider,
        );
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
