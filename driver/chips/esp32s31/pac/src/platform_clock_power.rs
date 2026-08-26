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
        self.peripherals
            .pmu_radio
            .hp_active_icg_modem()
            .write(|w| w.hp_active_dig_icg_modem_code().active());
    }

    pub(crate) fn apply_modem_icg_selection(&mut self) {
        self.peripherals
            .pmu_radio
            .imm_modem_icg()
            .write(|w| w.update_dig_icg_modem_en().set_bit());
    }

    pub(crate) fn apply_sleep_icg_selection(&mut self) {
        self.peripherals
            .pmu_radio
            .imm_sleep_sysclk()
            .write(|w| w.update_dig_icg_switch().set_bit());
    }

    pub(crate) fn enable_modem_register_bus_clock(&mut self) {
        self.peripherals
            .hp_sys_clkrst_radio
            .modem_ctrl0()
            .modify(|_, w| w.modem_clk_en().set_bit());
    }

    pub(crate) fn configure_modem_source_clocks(&mut self) {
        self.peripherals
            .hp_sys_clkrst_radio
            .ref_160m_ctrl0()
            .modify(|_, w| w.ref_160m_clk_en().set_bit());
        self.peripherals
            .hp_sys_clkrst_radio
            .modem_conf()
            .modify(|_, w| {
                w.modem_apb_clk_en()
                    .set_bit()
                    .modem_rst_en()
                    .clear_bit()
                    .modem_clk_en()
                    .set_bit()
                    .modem_clk_source_sel()
                    .set_bit()
                    .modem_pll_clk_en()
                    .set_bit()
                    .modem_xtal_clk_en()
                    .set_bit()
            });
    }

    pub(crate) fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        let hp_active_icg = self.peripherals.pmu_radio.hp_active_icg_modem().read();
        let modem_bus = self.peripherals.hp_sys_clkrst_radio.modem_ctrl0().read();
        let ref_160m = self.peripherals.hp_sys_clkrst_radio.ref_160m_ctrl0().read();
        let modem_source = self.peripherals.hp_sys_clkrst_radio.modem_conf().read();
        PlatformClockPowerObservation {
            hp_active_icg_selected: hp_active_icg.hp_active_dig_icg_modem_code().is_active(),
            modem_register_bus_clock_enabled: modem_bus.modem_clk_en().bit_is_set(),
            ref_160m_clock_enabled: ref_160m.ref_160m_clk_en().bit_is_set(),
            modem_source_clocks_configured: modem_source.modem_apb_clk_en().bit_is_set()
                && modem_source.modem_rst_en().bit_is_clear()
                && modem_source.modem_clk_en().bit_is_set()
                && modem_source.modem_clk_source_sel().bit_is_set()
                && modem_source.modem_pll_clk_en().bit_is_set()
                && modem_source.modem_xtal_clk_en().bit_is_set(),
        }
    }

    fn platform_pll_source_baseline(&self) -> PlatformPllSourceBaseline {
        let ref_160m = self.peripherals.hp_sys_clkrst_radio.ref_160m_ctrl0().read();
        let modem = self.peripherals.hp_sys_clkrst_radio.modem_conf().read();
        PlatformPllSourceBaseline {
            ref_160m_clock_enabled: ref_160m.ref_160m_clk_en().bit_is_set(),
            modem_apb_clock_enabled: modem.modem_apb_clk_en().bit_is_set(),
            modem_reset_asserted: modem.modem_rst_en().bit_is_set(),
            modem_source_clock_enabled: modem.modem_clk_en().bit_is_set(),
            modem_pll_selected: modem.modem_clk_source_sel().bit_is_set(),
            modem_pll_clock_enabled: modem.modem_pll_clk_en().bit_is_set(),
            modem_xtal_clock_enabled: modem.modem_xtal_clk_en().bit_is_set(),
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
        self.peripherals
            .hp_sys_clkrst_radio
            .ref_160m_ctrl0()
            .modify(|_, w| w.ref_160m_clk_en().bit(baseline.ref_160m_clock_enabled));
        self.peripherals
            .hp_sys_clkrst_radio
            .modem_conf()
            .modify(|_, w| {
                w.modem_apb_clk_en()
                    .bit(baseline.modem_apb_clock_enabled)
                    .modem_rst_en()
                    .bit(baseline.modem_reset_asserted)
                    .modem_clk_en()
                    .bit(baseline.modem_source_clock_enabled)
                    .modem_clk_source_sel()
                    .bit(baseline.modem_pll_selected)
                    .modem_pll_clk_en()
                    .bit(baseline.modem_pll_clock_enabled)
                    .modem_xtal_clk_en()
                    .bit(baseline.modem_xtal_clock_enabled)
            });
    }

    #[doc(hidden)]
    pub fn set_rf_circuit_power(&mut self, enabled: bool) {
        self.peripherals.pmu_radio.rf_pwc().modify(|_, w| {
            if enabled {
                w.xpd_rf_circuit().powered_on()
            } else {
                w.xpd_rf_circuit().powered_off()
            }
        });
    }

    #[doc(hidden)]
    pub fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        self.peripherals
            .pmu_radio
            .imm_hp_ck_power_0()
            .modify(|_, w| w.tie_high_xpd_bb_i2c().bit(enabled));
    }

    #[doc(hidden)]
    pub fn analog_i2c_is_powered(&self) -> bool {
        self.peripherals
            .pmu_radio
            .ana_peri_pwr_ctrl()
            .read()
            .xpd_perif_i2c()
            .bit_is_set()
    }

    #[doc(hidden)]
    pub fn set_analog_i2c_power(&mut self, enabled: bool) {
        self.peripherals
            .pmu_radio
            .ana_peri_pwr_ctrl()
            .modify(|_, w| w.xpd_perif_i2c().bit(enabled));
    }

    #[doc(hidden)]
    pub fn analog_i2c_reset_is_released(&self) -> bool {
        self.peripherals
            .pmu_radio
            .ana_peri_pwr_ctrl()
            .read()
            .rstb_perif_i2c()
            .bit_is_set()
    }

    #[doc(hidden)]
    pub fn set_analog_i2c_reset_released(&mut self, released: bool) {
        self.peripherals
            .pmu_radio
            .ana_peri_pwr_ctrl()
            .modify(|_, w| w.rstb_perif_i2c().bit(released));
    }

    #[doc(hidden)]
    pub fn enable_frontend_baseband_power(&mut self) {
        self.peripherals
            .pmu_radio
            .hp_active_hp_ck_power()
            .modify(|_, w| {
                w.rom_open_fe_bb_unknown_low()
                    .rom_required()
                    .hp_active_xpd_bb_i2c()
                    .set_bit()
            });
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
