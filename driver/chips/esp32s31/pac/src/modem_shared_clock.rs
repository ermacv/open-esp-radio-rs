//! Route-owned MODEM_LPCON shared-clock capability.
//!
//! The register block travels with the mutually exclusive Wi-Fi, Bluetooth
//! and IEEE 802.15.4 route. Public callers receive observations and semantic
//! route operations, never a register handle or a droppable clock token.

#![forbid(unsafe_code)]

use crate::{
    RadioPhyRegisters,
    generated::{
        CoexistenceClockGateImage, LowPowerTimerClockGateImage, ModemLowPowerClockDivider,
        ModemLowPowerTimerConfigurationBits, PhyI2cMasterClockGateImage, SharedModemClockMapImage,
    },
};

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

    const fn mask(self) -> u32 {
        match self {
            Self::Coexistence => 1 << 1,
            Self::PhyI2cMaster => 1 << 2,
            Self::LowPowerTimer => 1 << 3,
        }
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

impl ModemLowPowerClockSource {
    const fn selector_bits(self) -> u32 {
        match self {
            Self::SlowOscillator => 1 << 0,
            Self::FastOscillator => 1 << 1,
            Self::Crystal => 1 << 2,
            Self::Crystal32Khz => 1 << 3,
        }
    }
}

pub(crate) struct SharedModemClockState {
    counts: [u8; REQUIREMENT_COUNT],
    baseline_enabled: u8,
    low_power_timer_baseline: u16,
}

impl SharedModemClockState {
    pub(crate) const fn new() -> Self {
        Self {
            counts: [0; REQUIREMENT_COUNT],
            baseline_enabled: 0,
            low_power_timer_baseline: 0,
        }
    }

    fn count(&self, requirement: Requirement) -> u8 {
        self.counts[requirement.index()]
    }

    fn retain(&mut self, requirement: Requirement, observed: bool) -> bool {
        let index = requirement.index();
        let first = self.counts[index] == 0;
        if first {
            let bit = 1 << index;
            if observed {
                self.baseline_enabled |= bit;
            } else {
                self.baseline_enabled &= !bit;
            }
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
            let bit = 1 << index;
            let baseline = self.baseline_enabled & bit != 0;
            self.baseline_enabled &= !bit;
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
    pub slow_oscillator_selected: bool,
    pub fast_oscillator_selected: bool,
    pub main_xtal_selected: bool,
    pub xtal32k_selected: bool,
    pub divider: u16,
    pub timer_enabled: bool,
}

impl RadioPhyRegisters {
    pub(crate) fn prepare_shared_modem_clock_map(&mut self) {
        // This is the vendor's monotonic global ICG-map initialization, not
        // lease-owned state. It preserves the existing image and is therefore
        // intentionally not rolled back when an individual route is released.
        crate::svd::masked_register_modify::prepare_shared_modem_clock_map(
            &self.peripherals.modem_lpcon_shared_clock,
            SharedModemClockMapImage::ActiveStates.bits(),
        );
    }

    pub(crate) fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let clock_configuration = registers.clk_conf().read().bits();
        let power_state_map = registers.clk_conf_power_st().read().bits();
        SharedModemClockObservation {
            power_state_map_configured: [16, 20, 24, 28]
                .into_iter()
                .all(|shift| ((power_state_map >> shift) & 0x06) == 0x06),
            coexistence_clock_enabled: clock_configuration & Requirement::Coexistence.mask() != 0,
            phy_i2c_master_clock_enabled: clock_configuration & Requirement::PhyI2cMaster.mask()
                != 0,
            low_power_timer_clock_enabled: clock_configuration & Requirement::LowPowerTimer.mask()
                != 0,
        }
    }

    pub(crate) fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        let register = self.peripherals.modem_lpcon_shared_clock.coex_lp_clk_conf();
        let selector_image = register.read().bits();
        let divider_image = register.read().bits();
        let source = match selector_image & 0x0f {
            1 => CoexistenceLowPowerClockSource::Selector1,
            2 => CoexistenceLowPowerClockSource::Selector2,
            4 => CoexistenceLowPowerClockSource::Selector4,
            8 => CoexistenceLowPowerClockSource::Selector8,
            _ => return None,
        };
        Some(CoexistenceLowPowerClockObservation {
            source,
            divider_minus_one: ((divider_image >> 4) & 0x0fff) as u16,
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
        let baseline = registers.lp_timer_conf().read().bits() & 0x0000_ffff;
        self.shared_clock.low_power_timer_baseline = baseline as u16;
        let configuration = source.selector_bits() | (divider.get() << 4);
        let configuration = ModemLowPowerTimerConfigurationBits::new(configuration)
            .expect("typed source and divider fit LP_TIMER_CONF");
        crate::svd::masked_register_modify::configure_shared_low_power_timer(
            registers,
            configuration.get(),
        );

        BluetoothLowPowerTimerLease(self.retain_requirement(requirement))
    }

    pub(crate) fn release_bluetooth_low_power_timer(&mut self, lease: BluetoothLowPowerTimerLease) {
        let BluetoothLowPowerTimerLease(lease) = lease;
        let baseline = core::mem::take(&mut self.shared_clock.low_power_timer_baseline);
        let baseline = ModemLowPowerTimerConfigurationBits::new(u32::from(baseline))
            .expect("sampled low sixteen-bit image remains bounded");
        crate::svd::masked_register_modify::configure_shared_low_power_timer(
            &self.peripherals.modem_lpcon_shared_clock,
            baseline.get(),
        );
        self.release_shared_modem_clock(lease);
    }

    pub(crate) fn bluetooth_low_power_clock_observation(
        &self,
    ) -> BluetoothLowPowerClockObservation {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        let configuration = registers.lp_timer_conf().read().bits();
        BluetoothLowPowerClockObservation {
            slow_oscillator_selected: configuration & (1 << 0) != 0,
            fast_oscillator_selected: configuration & (1 << 1) != 0,
            main_xtal_selected: configuration & (1 << 2) != 0,
            xtal32k_selected: configuration & (1 << 3) != 0,
            divider: ((configuration >> 4) & 0x0fff) as u16,
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
        self.peripherals
            .modem_lpcon_shared_clock
            .clk_conf()
            .read()
            .bits()
            & requirement.mask()
            != 0
    }

    fn set_requirement_gate(&mut self, requirement: Requirement, enabled: bool) {
        let registers = &self.peripherals.modem_lpcon_shared_clock;
        match requirement {
            Requirement::Coexistence => {
                let image = if enabled {
                    CoexistenceClockGateImage::Enabled
                } else {
                    CoexistenceClockGateImage::Disabled
                };
                crate::svd::masked_register_modify::set_shared_coexistence_clock_gate(
                    registers,
                    image.bits(),
                );
            }
            Requirement::PhyI2cMaster => {
                let image = if enabled {
                    PhyI2cMasterClockGateImage::Enabled
                } else {
                    PhyI2cMasterClockGateImage::Disabled
                };
                crate::svd::masked_register_modify::set_shared_phy_i2c_master_clock_gate(
                    registers,
                    image.bits(),
                );
            }
            Requirement::LowPowerTimer => {
                let image = if enabled {
                    LowPowerTimerClockGateImage::Enabled
                } else {
                    LowPowerTimerClockGateImage::Disabled
                };
                crate::svd::masked_register_modify::set_shared_low_power_timer_clock_gate(
                    registers,
                    image.bits(),
                );
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
