//! Route-owned MODEM_LPCON shared-clock capability.
//!
//! The register block travels with the mutually exclusive Wi-Fi, Bluetooth
//! and IEEE 802.15.4 route. Public callers receive observations and semantic
//! route operations, never a register handle or a droppable clock token.

#![forbid(unsafe_code)]

use crate::{RadioPhyRegisters, generated::ModemLowPowerClockDivider};

const REQUIREMENT_COUNT: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SharedModemClock {
    Coexistence,
    PhyI2cMaster,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Requirement {
    Coexistence = 0,
    PhyI2cMaster = 1,
    LowPowerTimer = 2,
}

impl Requirement {
    const fn index(self) -> usize {
        self as usize
    }
}

impl From<SharedModemClock> for Requirement {
    fn from(value: SharedModemClock) -> Self {
        match value {
            SharedModemClock::Coexistence => Self::Coexistence,
            SharedModemClock::PhyI2cMaster => Self::PhyI2cMaster,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RequirementBaselines {
    enabled: u8,
}

impl RequirementBaselines {
    const fn bit(requirement: Requirement) -> u8 {
        match requirement {
            Requirement::Coexistence => 1,
            Requirement::PhyI2cMaster => 2,
            Requirement::LowPowerTimer => 4,
        }
    }

    fn set(&mut self, requirement: Requirement, enabled: bool) {
        let bit = Self::bit(requirement);
        if enabled {
            self.enabled |= bit;
        } else {
            self.enabled &= !bit;
        }
    }

    fn take(&mut self, requirement: Requirement) -> bool {
        let bit = Self::bit(requirement);
        let enabled = self.enabled & bit != 0;
        self.enabled &= !bit;
        enabled
    }
}

/// Reviewed Bluetooth low-power timer sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemLowPowerClockSource {
    SlowOscillator,
    FastOscillator,
    Crystal,
    Crystal32Khz,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BluetoothLowPowerTimerConfiguration {
    slow_oscillator_selected: bool,
    fast_oscillator_selected: bool,
    crystal_selected: bool,
    crystal_32khz_selected: bool,
    divider_minus_one: u16,
}

pub(crate) struct SharedModemClockState {
    counts: [u8; REQUIREMENT_COUNT],
    baselines: RequirementBaselines,
    low_power_timer_baseline: BluetoothLowPowerTimerConfiguration,
}

impl SharedModemClockState {
    pub(crate) const fn new() -> Self {
        Self {
            counts: [0; REQUIREMENT_COUNT],
            baselines: RequirementBaselines { enabled: 0 },
            low_power_timer_baseline: BluetoothLowPowerTimerConfiguration {
                slow_oscillator_selected: false,
                fast_oscillator_selected: false,
                crystal_selected: false,
                crystal_32khz_selected: false,
                divider_minus_one: 0,
            },
        }
    }

    fn count(&self, requirement: Requirement) -> u8 {
        self.counts[requirement.index()]
    }

    fn retain(&mut self, requirement: Requirement, observed: bool) -> bool {
        let index = requirement.index();
        let first = self.counts[index] == 0;
        if first {
            self.baselines.set(requirement, observed);
        }
        self.counts[index] = self.counts[index]
            .checked_add(1)
            .expect("route-owned MODEM_LPCON reference count cannot overflow");
        first && !observed
    }

    fn release(&mut self, requirement: Requirement) -> Option<bool> {
        let index = requirement.index();
        assert!(
            self.counts[index] != 0,
            "unbalanced MODEM_LPCON clock release"
        );
        self.counts[index] -= 1;
        if self.counts[index] == 0 {
            Some(self.baselines.take(requirement))
        } else {
            None
        }
    }
}

#[must_use = "the route owner must retain and release this internal clock token"]
pub(crate) struct SharedModemClockLease {
    requirement: Requirement,
}

#[must_use = "the Bluetooth route must restore the low-power timer configuration"]
pub(crate) struct BluetoothLowPowerTimerLease(SharedModemClockLease);

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
    pub(crate) fn prepare_shared_modem_clock_map(&mut self) {
        // This is the vendor's monotonic global ICG-map initialization, not
        // lease-owned state. It preserves the existing image and is therefore
        // intentionally not rolled back when an individual route is released.
        crate::generated::initialize_shared_modem_power_state_map(
            &self.peripherals.modem_lpcon_shared_clock,
        );
    }

    pub(crate) fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
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

    pub(crate) fn sample_coexistence_low_power_clock(
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

    pub(crate) fn retain_shared_modem_clock(
        &mut self,
        clock: SharedModemClock,
    ) -> SharedModemClockLease {
        let requirement = Requirement::from(clock);
        let observed = self.gate_enabled(requirement);
        if self.shared_clock.retain(requirement, observed) {
            self.set_requirement_gate(requirement, true);
        }
        SharedModemClockLease { requirement }
    }

    pub(crate) fn release_shared_modem_clock(&mut self, lease: SharedModemClockLease) {
        if let Some(baseline) = self.shared_clock.release(lease.requirement)
            && self.gate_enabled(lease.requirement) != baseline
        {
            self.set_requirement_gate(lease.requirement, baseline);
        }
    }

    pub(crate) fn retain_bluetooth_low_power_timer(
        &mut self,
        source: ModemLowPowerClockSource,
        divider: ModemLowPowerClockDivider,
    ) -> BluetoothLowPowerTimerLease {
        let requirement = Requirement::LowPowerTimer;
        assert!(
            self.shared_clock.count(requirement) == 0,
            "Bluetooth low-power timer already retained by this route"
        );

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
        self.shared_clock.low_power_timer_baseline = BluetoothLowPowerTimerConfiguration {
            slow_oscillator_selected,
            fast_oscillator_selected,
            crystal_selected,
            crystal_32khz_selected,
            divider_minus_one,
        };
        crate::generated::configure_shared_modem_low_power_timer(
            registers,
            source == ModemLowPowerClockSource::SlowOscillator,
            source == ModemLowPowerClockSource::FastOscillator,
            source == ModemLowPowerClockSource::Crystal,
            source == ModemLowPowerClockSource::Crystal32Khz,
            divider,
        );

        BluetoothLowPowerTimerLease(self.retain_requirement(requirement))
    }

    pub(crate) fn release_bluetooth_low_power_timer(&mut self, lease: BluetoothLowPowerTimerLease) {
        let BluetoothLowPowerTimerLease(lease) = lease;
        let baseline = core::mem::take(&mut self.shared_clock.low_power_timer_baseline);
        let divider = ModemLowPowerClockDivider::new(u32::from(baseline.divider_minus_one))
            .expect("generated twelve-bit LP timer readback must fit its write domain");
        crate::generated::configure_shared_modem_low_power_timer(
            &self.peripherals.modem_lpcon_shared_clock,
            baseline.slow_oscillator_selected,
            baseline.fast_oscillator_selected,
            baseline.crystal_selected,
            baseline.crystal_32khz_selected,
            divider,
        );
        self.release_shared_modem_clock(lease);
    }

    pub(crate) fn bluetooth_low_power_clock_observation(
        &self,
    ) -> BluetoothLowPowerClockObservation {
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
            timer_enabled: self.gate_enabled(Requirement::LowPowerTimer),
        }
    }

    fn retain_requirement(&mut self, requirement: Requirement) -> SharedModemClockLease {
        let observed = self.gate_enabled(requirement);
        if self.shared_clock.retain(requirement, observed) {
            self.set_requirement_gate(requirement, true);
        }
        SharedModemClockLease { requirement }
    }

    fn gate_enabled(&self, requirement: Requirement) -> bool {
        let (coexistence, phy_i2c_master, low_power_timer) =
            crate::svd::field_snapshot_read::observe_shared_modem_clock_gates(
                &self.peripherals.modem_lpcon_shared_clock,
            );
        match requirement {
            Requirement::Coexistence => coexistence,
            Requirement::PhyI2cMaster => phy_i2c_master,
            Requirement::LowPowerTimer => low_power_timer,
        }
    }

    fn set_requirement_gate(&mut self, requirement: Requirement, enabled: bool) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        match (requirement, enabled) {
            (Requirement::Coexistence, true) => {
                crate::generated::enable_shared_modem_coexistence_clock(registers);
            }
            (Requirement::Coexistence, false) => {
                crate::generated::disable_shared_modem_coexistence_clock(registers);
            }
            (Requirement::PhyI2cMaster, true) => {
                crate::generated::enable_shared_modem_phy_i2c_master_clock(registers);
            }
            (Requirement::PhyI2cMaster, false) => {
                crate::generated::disable_shared_modem_phy_i2c_master_clock(registers);
            }
            (Requirement::LowPowerTimer, true) => {
                crate::generated::enable_shared_modem_low_power_timer_clock(registers);
            }
            (Requirement::LowPowerTimer, false) => {
                crate::generated::disable_shared_modem_low_power_timer_clock(registers);
            }
        }
    }
}

#[cfg(test)]
mod tests;
