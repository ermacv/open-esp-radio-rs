//! Route-owned PMU and upstream radio clock transactions.

use super::RadioPhyRegisters;

/// Semantic readback of the system-clock prerequisites shared by all radios.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlatformClockPowerObservation {
    pub hp_active_icg_selected: bool,
    pub modem_register_bus_clock_enabled: bool,
    pub ref_160m_clock_enabled: bool,
    pub modem_source_clocks_configured: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlatformPllSourceBaseline {
    ref_160m_clock_enabled: bool,
    modem_apb_clock_enabled: bool,
    modem_reset_asserted: bool,
    modem_source_clock_enabled: bool,
    modem_pll_selected: bool,
    modem_pll_clock_enabled: bool,
    modem_xtal_clock_enabled: bool,
}

/// Linear proof of one retained upstream PLL-source dependency.
pub(crate) struct PlatformPllSourceLease {
    _private: (),
}

pub(crate) struct PlatformClockPowerState {
    pll_source_retain_count: u16,
    pll_source_baseline: Option<PlatformPllSourceBaseline>,
}

impl PlatformClockPowerState {
    pub(crate) const fn new() -> Self {
        Self {
            pll_source_retain_count: 0,
            pll_source_baseline: None,
        }
    }

    fn retain(&mut self, baseline: PlatformPllSourceBaseline) -> bool {
        let first = self.pll_source_retain_count == 0;
        if first {
            self.pll_source_baseline = Some(baseline);
        }
        self.pll_source_retain_count = self
            .pll_source_retain_count
            .checked_add(1)
            .expect("platform PLL source retain count cannot overflow");
        first
    }

    fn release(&mut self) -> Option<PlatformPllSourceBaseline> {
        assert!(
            self.pll_source_retain_count != 0,
            "unbalanced platform PLL source release"
        );
        self.pll_source_retain_count -= 1;
        if self.pll_source_retain_count == 0 {
            self.pll_source_baseline.take()
        } else {
            None
        }
    }
}

impl RadioPhyRegisters {
    pub(crate) fn select_hp_active_modem_icg(&mut self) {
        crate::svd::fixed_register_image::select_hp_active_modem_icg(&self.peripherals.pmu_radio);
    }

    pub(crate) fn apply_modem_icg_selection(&mut self) {
        crate::svd::fixed_register_image::apply_modem_icg_selection(&self.peripherals.pmu_radio);
    }

    pub(crate) fn apply_sleep_icg_selection(&mut self) {
        crate::svd::fixed_register_image::apply_sleep_icg_selection(&self.peripherals.pmu_radio);
    }

    pub(crate) fn enable_modem_register_bus_clock(&mut self) {
        crate::generated::enable_modem_register_bus_clock(&self.peripherals.hp_sys_clkrst_radio);
    }

    pub(crate) fn configure_modem_source_clocks(&mut self) {
        crate::generated::enable_modem_reference_160m_clock(&self.peripherals.hp_sys_clkrst_radio);
        crate::generated::configure_modem_source_clocks(&self.peripherals.hp_sys_clkrst_radio);
    }

    pub(crate) fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        let hp_active_icg_code =
            crate::svd::field_read::observe_hp_active_modem_icg_code(&self.peripherals.pmu_radio);
        let modem_register_bus_clock_enabled =
            crate::svd::field_read::observe_modem_register_bus_clock(
                &self.peripherals.hp_sys_clkrst_radio,
            );
        let ref_160m_clock_enabled = crate::svd::field_read::observe_modem_reference_160m_clock(
            &self.peripherals.hp_sys_clkrst_radio,
        );
        let (
            modem_apb_clock_enabled,
            modem_reset_asserted,
            modem_source_clock_enabled,
            modem_pll_selected,
            modem_pll_clock_enabled,
            modem_xtal_clock_enabled,
        ) = crate::svd::field_snapshot_read::observe_modem_source_clocks(
            &self.peripherals.hp_sys_clkrst_radio,
        );
        PlatformClockPowerObservation {
            hp_active_icg_selected: hp_active_icg_code
                == u8::from(crate::svd::pmu_radio::hp_active_icg_modem::ActiveModemIcgCode::Active),
            modem_register_bus_clock_enabled,
            ref_160m_clock_enabled,
            modem_source_clocks_configured: modem_apb_clock_enabled
                && !modem_reset_asserted
                && modem_source_clock_enabled
                && modem_pll_selected
                && modem_pll_clock_enabled
                && modem_xtal_clock_enabled,
        }
    }

    fn platform_pll_source_baseline(&self) -> PlatformPllSourceBaseline {
        let ref_160m_clock_enabled = crate::svd::field_read::observe_modem_reference_160m_clock(
            &self.peripherals.hp_sys_clkrst_radio,
        );
        let (
            modem_apb_clock_enabled,
            modem_reset_asserted,
            modem_source_clock_enabled,
            modem_pll_selected,
            modem_pll_clock_enabled,
            modem_xtal_clock_enabled,
        ) = crate::svd::field_snapshot_read::observe_modem_source_clocks(
            &self.peripherals.hp_sys_clkrst_radio,
        );
        PlatformPllSourceBaseline {
            ref_160m_clock_enabled,
            modem_apb_clock_enabled,
            modem_reset_asserted,
            modem_source_clock_enabled,
            modem_pll_selected,
            modem_pll_clock_enabled,
            modem_xtal_clock_enabled,
        }
    }

    pub(crate) fn retain_platform_pll_source(&mut self) -> PlatformPllSourceLease {
        let baseline = self.platform_pll_source_baseline();
        if self.platform_clock_power.retain(baseline) {
            self.configure_modem_source_clocks();
        }
        PlatformPllSourceLease { _private: () }
    }

    pub(crate) fn release_platform_pll_source(&mut self, _lease: PlatformPllSourceLease) {
        let Some(baseline) = self.platform_clock_power.release() else {
            return;
        };
        if baseline.ref_160m_clock_enabled {
            crate::generated::enable_modem_reference_160m_clock(
                &self.peripherals.hp_sys_clkrst_radio,
            );
        } else {
            crate::generated::disable_modem_reference_160m_clock(
                &self.peripherals.hp_sys_clkrst_radio,
            );
        }
        crate::generated::restore_modem_source_clocks(
            &self.peripherals.hp_sys_clkrst_radio,
            baseline.modem_apb_clock_enabled,
            baseline.modem_reset_asserted,
            baseline.modem_source_clock_enabled,
            baseline.modem_pll_selected,
            baseline.modem_pll_clock_enabled,
            baseline.modem_xtal_clock_enabled,
        );
    }

    #[doc(hidden)]
    pub fn set_rf_circuit_power(&mut self, enabled: bool) {
        if enabled {
            crate::generated::power_on_rf_circuits(&self.peripherals.pmu_radio);
        } else {
            crate::generated::power_off_rf_circuits(&self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        if enabled {
            crate::generated::enable_baseband_i2c_power_tie(&self.peripherals.pmu_radio);
        } else {
            crate::generated::disable_baseband_i2c_power_tie(&self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn analog_i2c_is_powered(&self) -> bool {
        crate::svd::field_read::observe_analog_i2c_power(&self.peripherals.pmu_radio)
    }

    #[doc(hidden)]
    pub fn set_analog_i2c_power(&mut self, enabled: bool) {
        if enabled {
            crate::generated::power_on_analog_i2c(&self.peripherals.pmu_radio);
        } else {
            crate::generated::power_off_analog_i2c(&self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn analog_i2c_reset_is_released(&self) -> bool {
        crate::svd::field_read::observe_analog_i2c_reset_release(&self.peripherals.pmu_radio)
    }

    #[doc(hidden)]
    pub fn set_analog_i2c_reset_released(&mut self, released: bool) {
        if released {
            crate::generated::release_analog_i2c_reset(&self.peripherals.pmu_radio);
        } else {
            crate::generated::assert_analog_i2c_reset(&self.peripherals.pmu_radio);
        }
    }

    #[doc(hidden)]
    pub fn enable_frontend_baseband_power(&mut self) {
        crate::generated::enable_frontend_baseband_power(&self.peripherals.pmu_radio);
    }
}

#[cfg(test)]
mod tests {
    use super::{PlatformClockPowerState, PlatformPllSourceBaseline};

    const BASELINE: PlatformPllSourceBaseline = PlatformPllSourceBaseline {
        ref_160m_clock_enabled: true,
        modem_apb_clock_enabled: false,
        modem_reset_asserted: true,
        modem_source_clock_enabled: true,
        modem_pll_selected: false,
        modem_pll_clock_enabled: false,
        modem_xtal_clock_enabled: true,
    };

    #[test]
    fn nested_retain_restores_only_after_last_release() {
        let mut state = PlatformClockPowerState::new();
        assert!(state.retain(BASELINE));
        assert!(!state.retain(BASELINE));
        assert_eq!(state.release(), None);
        assert_eq!(state.release(), Some(BASELINE));
    }
}
