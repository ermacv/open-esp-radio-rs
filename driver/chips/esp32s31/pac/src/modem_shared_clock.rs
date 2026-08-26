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
    baseline_enabled: [bool; REQUIREMENT_COUNT],
    low_power_timer_baseline: BluetoothLowPowerTimerConfiguration,
}

impl SharedModemClockState {
    pub(crate) const fn new() -> Self {
        Self {
            counts: [0; REQUIREMENT_COUNT],
            baseline_enabled: [false; REQUIREMENT_COUNT],
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
            self.baseline_enabled[index] = observed;
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
            let baseline = core::mem::take(&mut self.baseline_enabled[index]);
            Some(baseline)
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
        self.peripherals
            .modem_lpcon_shared_clock
            .clk_conf_power_st()
            .modify(|_, w| {
                w.clk_wifipwr_st_map_bit_one()
                    .set_bit()
                    .clk_wifipwr_st_map_bit_two()
                    .set_bit()
                    .clk_coex_st_map_bit_one()
                    .set_bit()
                    .clk_coex_st_map_bit_two()
                    .set_bit()
                    .clk_i2c_mst_st_map_bit_one()
                    .set_bit()
                    .clk_i2c_mst_st_map_bit_two()
                    .set_bit()
                    .clk_lp_apb_st_map_bit_one()
                    .set_bit()
                    .clk_lp_apb_st_map_bit_two()
                    .set_bit()
            });
    }

    pub(crate) fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let clock_configuration = registers.clk_conf().read();
        let power_state_map = registers.clk_conf_power_st().read();
        SharedModemClockObservation {
            power_state_map_configured: power_state_map.clk_wifipwr_st_map_bit_one().bit_is_set()
                && power_state_map.clk_wifipwr_st_map_bit_two().bit_is_set()
                && power_state_map.clk_coex_st_map_bit_one().bit_is_set()
                && power_state_map.clk_coex_st_map_bit_two().bit_is_set()
                && power_state_map.clk_i2c_mst_st_map_bit_one().bit_is_set()
                && power_state_map.clk_i2c_mst_st_map_bit_two().bit_is_set()
                && power_state_map.clk_lp_apb_st_map_bit_one().bit_is_set()
                && power_state_map.clk_lp_apb_st_map_bit_two().bit_is_set(),
            coexistence_clock_enabled: clock_configuration.clk_coex_en().bit_is_set(),
            phy_i2c_master_clock_enabled: clock_configuration.clk_i2c_mst_en().bit_is_set(),
            low_power_timer_clock_enabled: clock_configuration.clk_lp_timer_en().bit_is_set(),
        }
    }

    pub(crate) fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        let register = self.peripherals.modem_lpcon_shared_clock.coex_lp_clk_conf();
        let selector = register.read();
        let source = match (
            selector.clk_coex_lp_sel_osc_slow().bit_is_set(),
            selector.clk_coex_lp_sel_osc_fast().bit_is_set(),
            selector.clk_coex_lp_sel_xtal().bit_is_set(),
            selector.clk_coex_lp_sel_xtal32k().bit_is_set(),
        ) {
            (true, false, false, false) => CoexistenceLowPowerClockSource::Selector1,
            (false, true, false, false) => CoexistenceLowPowerClockSource::Selector2,
            (false, false, true, false) => CoexistenceLowPowerClockSource::Selector4,
            (false, false, false, true) => CoexistenceLowPowerClockSource::Selector8,
            _ => return None,
        };
        Some(CoexistenceLowPowerClockObservation {
            source,
            divider_minus_one: register.read().clk_coex_lp_div_num().bits(),
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
        let baseline = registers.lp_timer_conf().read();
        self.shared_clock.low_power_timer_baseline = BluetoothLowPowerTimerConfiguration {
            slow_oscillator_selected: baseline.clk_lp_timer_sel_osc_slow().bit_is_set(),
            fast_oscillator_selected: baseline.clk_lp_timer_sel_osc_fast().bit_is_set(),
            crystal_selected: baseline.clk_lp_timer_sel_xtal().bit_is_set(),
            crystal_32khz_selected: baseline.clk_lp_timer_sel_xtal32k().bit_is_set(),
            divider_minus_one: baseline.clk_lp_timer_div_num().bits(),
        };
        registers.lp_timer_conf().modify(|_, w| {
            w.clk_lp_timer_sel_osc_slow()
                .bit(source == ModemLowPowerClockSource::SlowOscillator)
                .clk_lp_timer_sel_osc_fast()
                .bit(source == ModemLowPowerClockSource::FastOscillator)
                .clk_lp_timer_sel_xtal()
                .bit(source == ModemLowPowerClockSource::Crystal)
                .clk_lp_timer_sel_xtal32k()
                .bit(source == ModemLowPowerClockSource::Crystal32Khz)
                .clk_lp_timer_div_num()
                .set(divider.get() as u16)
        });

        BluetoothLowPowerTimerLease(self.retain_requirement(requirement))
    }

    pub(crate) fn release_bluetooth_low_power_timer(&mut self, lease: BluetoothLowPowerTimerLease) {
        let BluetoothLowPowerTimerLease(lease) = lease;
        let baseline = core::mem::take(&mut self.shared_clock.low_power_timer_baseline);
        self.peripherals
            .modem_lpcon_shared_clock
            .lp_timer_conf()
            .modify(|_, w| {
                w.clk_lp_timer_sel_osc_slow()
                    .bit(baseline.slow_oscillator_selected)
                    .clk_lp_timer_sel_osc_fast()
                    .bit(baseline.fast_oscillator_selected)
                    .clk_lp_timer_sel_xtal()
                    .bit(baseline.crystal_selected)
                    .clk_lp_timer_sel_xtal32k()
                    .bit(baseline.crystal_32khz_selected)
                    .clk_lp_timer_div_num()
                    .set(baseline.divider_minus_one)
            });
        self.release_shared_modem_clock(lease);
    }

    pub(crate) fn bluetooth_low_power_clock_observation(
        &self,
    ) -> BluetoothLowPowerClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let configuration = registers.lp_timer_conf().read();
        BluetoothLowPowerClockObservation {
            exclusive_main_xtal_selected: configuration.clk_lp_timer_sel_osc_slow().bit_is_clear()
                && configuration.clk_lp_timer_sel_osc_fast().bit_is_clear()
                && configuration.clk_lp_timer_sel_xtal().bit_is_set()
                && configuration.clk_lp_timer_sel_xtal32k().bit_is_clear(),
            bluetooth_divider_configured: u32::from(configuration.clk_lp_timer_div_num().bits())
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
        let configuration = self.peripherals.modem_lpcon_shared_clock.clk_conf().read();
        match requirement {
            Requirement::Coexistence => configuration.clk_coex_en().bit_is_set(),
            Requirement::PhyI2cMaster => configuration.clk_i2c_mst_en().bit_is_set(),
            Requirement::LowPowerTimer => configuration.clk_lp_timer_en().bit_is_set(),
        }
    }

    fn set_requirement_gate(&mut self, requirement: Requirement, enabled: bool) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        match requirement {
            Requirement::Coexistence => {
                registers
                    .clk_conf()
                    .modify(|_, w| w.clk_coex_en().bit(enabled));
            }
            Requirement::PhyI2cMaster => {
                registers
                    .clk_conf()
                    .modify(|_, w| w.clk_i2c_mst_en().bit(enabled));
            }
            Requirement::LowPowerTimer => {
                registers
                    .clk_conf()
                    .modify(|_, w| w.clk_lp_timer_en().bit(enabled));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_restores_only_the_final_retained_baseline() {
        let mut state = SharedModemClockState::new();
        assert!(state.retain(Requirement::Coexistence, false));
        assert!(!state.retain(Requirement::Coexistence, true));
        assert_eq!(state.release(Requirement::Coexistence), None);
        assert_eq!(state.release(Requirement::Coexistence), Some(false));

        assert!(!state.retain(Requirement::Coexistence, true));
        assert_eq!(state.release(Requirement::Coexistence), Some(true));
    }
}
