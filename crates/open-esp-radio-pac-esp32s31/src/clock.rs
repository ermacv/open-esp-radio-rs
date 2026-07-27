//! Safe, ownership-bound access to the recovered modem clock/reset registers.
//!
//! Register layout and field positions come from `svd/esp32s31-radio.svd`.
//! Operation values are independently evidenced by the pinned ESP32-S31
//! `esp-hal` clock implementation at commit `6899213e`. The complete cold-boot
//! ordering intentionally remains in the HAL crate.

use super::RadioRegisters;

impl RadioRegisters {
    /// Enable or disable the recovered PHY calibration clock.
    ///
    /// SOURCE[BLOB_LIBPHY_PHY_BB_INIT]; the complete parent sets this bit
    /// before executing the baseband calibration sequence.
    pub fn set_phy_calibration_clock(&mut self, enabled: bool) {
        self.peripherals
            .phy_clock_oracle
            .fe_bb_clock_control_opaque()
            .modify(|_, w| w.phy_calibration_clock_unknown().bit(enabled));
    }

    /// Open the recovered front-end and baseband clock gates.
    ///
    /// SOURCE[ROM_REV0_PHY_OPEN_FE_BB_CLK]; the four operations retain the
    /// complete ROM body's order. Opaque full-register values stay inside the
    /// PAC, while callers need only the unique mutable radio capability.
    pub fn open_frontend_baseband_clocks(&mut self) {
        // SAFETY: 0x1e7 is the instruction-exact full register value written
        // by the cited ROM leaf; the SVD deliberately marks the register
        // write-only because no field semantics have been inferred.
        unsafe {
            self.peripherals
                .phy_clock_oracle
                .fe_clock_gate_opaque()
                .write_with_zero(|w| w.bits(0x1e7));
        }

        // SAFETY: value three fits the recovered two-bit field and reproduces
        // the ROM OR mask without assigning meanings to its constituent bits.
        self.peripherals
            .phy_clock_oracle
            .fe_bb_clock_control_opaque()
            .modify(|_, w| unsafe { w.rom_fe_bb_enable_unknown().bits(3) });

        // SAFETY: all ones is the instruction-exact full register value
        // written by the cited ROM leaf to this write-only opaque gate.
        unsafe {
            self.peripherals
                .phy_clock_oracle
                .bb_clock_gate_opaque()
                .write_with_zero(|w| w.bits(u32::MAX));
        }

        // SAFETY: 0x0f fits the recovered four-bit field. The adjacent named
        // bit is independently present in the S31 PMU description.
        self.peripherals
            .pmu
            .hp_active_hp_ck_power()
            .modify(|_, w| unsafe {
                w.rom_open_fe_bb_unknown_low()
                    .bits(0x0f)
                    .hp_active_xpd_bb_i2c()
                    .set_bit()
            });
    }

    /// Close the recovered front-end and baseband clock gates.
    ///
    /// SOURCE[BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK]; this is the exact inverse
    /// three-operation blob leaf. It intentionally leaves PMU power policy to
    /// the surrounding lifecycle owner, matching the source.
    pub fn close_frontend_baseband_clocks(&mut self) {
        // SAFETY: zero is the complete blob value for this write-only opaque
        // gate register.
        unsafe {
            self.peripherals
                .phy_clock_oracle
                .fe_clock_gate_opaque()
                .write_with_zero(|w| w.bits(0));
        }

        // SAFETY: zero fits the recovered two-bit field.
        self.peripherals
            .phy_clock_oracle
            .fe_bb_clock_control_opaque()
            .modify(|_, w| unsafe { w.rom_fe_bb_enable_unknown().bits(0) });

        // SAFETY: zero is the complete blob value for this write-only opaque
        // gate register.
        unsafe {
            self.peripherals
                .phy_clock_oracle
                .bb_clock_gate_opaque()
                .write_with_zero(|w| w.bits(0));
        }
    }

    /// Assert or release the Wi-Fi baseband and MAC reset lines together.
    pub fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.peripherals
            .modem_syscon
            .modem_rst_conf()
            .modify(|_, w| w.rst_wifibb().bit(asserted).rst_wifimac().bit(asserted));
    }

    /// Assert or release only the Wi-Fi baseband reset line.
    pub fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.peripherals
            .modem_syscon
            .modem_rst_conf()
            .modify(|_, w| w.rst_wifibb().bit(asserted));
    }

    /// Program the HP-active modem ICG code used by the S31 clock oracle.
    pub fn select_hp_active_modem_icg(&mut self) {
        // SAFETY: code 2 fits the recovered two-bit field and is the exact
        // value used by the pinned S31 `esp-hal` clock implementation.
        unsafe {
            self.peripherals
                .pmu
                .hp_active_icg_modem()
                .write_with_zero(|w| w.hp_active_dig_icg_modem_code().bits(2));
        }
    }

    /// Trigger application of the immediate modem ICG selection.
    pub fn apply_modem_icg_selection(&mut self) {
        // SAFETY: this is a write-only one-bit trigger from the exact S31 PMU
        // layout. `write_with_zero` cannot publish unrelated trigger bits.
        unsafe {
            self.peripherals
                .pmu
                .imm_modem_icg()
                .write_with_zero(|w| w.update_dig_icg_modem_en().set_bit());
        }
    }

    /// Trigger application of the immediate sleep/system ICG switch.
    pub fn apply_sleep_icg_selection(&mut self) {
        // SAFETY: this is a write-only one-bit trigger from the exact S31 PMU
        // layout. `write_with_zero` cannot publish unrelated trigger bits.
        unsafe {
            self.peripherals
                .pmu
                .imm_sleep_sysclk()
                .write_with_zero(|w| w.update_dig_icg_switch().set_bit());
        }
    }

    /// Enable the high-performance modem register-bus clock.
    pub fn enable_modem_register_bus_clock(&mut self) {
        self.peripherals
            .hp_sys_clkrst
            .modem_ctrl0()
            .modify(|_, w| w.reg_modem_clk_en().set_bit());
    }

    /// Install the HP-active state map used for modem-domain clocks.
    pub fn configure_hp_active_modem_clock_map(&mut self) {
        // SAFETY: every value fits its four-bit field and reproduces the
        // pinned S31 `esp-hal` clock map (4, 6, 4, 6, 4, 6).
        unsafe {
            self.peripherals
                .modem_syscon
                .clk_conf_power_st()
                .modify(|_, w| {
                    w.clk_zb_st_map()
                        .bits(4)
                        .clk_fe_st_map()
                        .bits(6)
                        .clk_bt_st_map()
                        .bits(4)
                        .clk_wifi_st_map()
                        .bits(6)
                        .clk_modem_peri_st_map()
                        .bits(4)
                        .clk_modem_apb_st_map()
                        .bits(6)
                });
        }
    }

    /// Install the HP-active state map used for shared low-power modem clocks.
    pub fn configure_shared_modem_clock_map(&mut self) {
        // SAFETY: value 6 fits every four-bit field and reproduces the pinned
        // S31 `esp-hal` clock map.
        unsafe {
            self.peripherals
                .modem_lpcon
                .clk_conf_power_st()
                .modify(|_, w| {
                    w.clk_wifipwr_st_map()
                        .bits(6)
                        .clk_coex_st_map()
                        .bits(6)
                        .clk_i2c_mst_st_map()
                        .bits(6)
                        .clk_lp_apb_st_map()
                        .bits(6)
                });
        }
    }

    /// Select and enable the modem APB, PLL and XTAL source clocks.
    pub fn configure_modem_source_clocks(&mut self) {
        self.peripherals.hp_sys_clkrst.modem_conf().write(|w| {
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

    /// Enable the PHY/baseband/frontend clocks required before calibration.
    pub fn enable_phy_calibration_clocks(&mut self) {
        self.peripherals.modem_syscon.clk_conf1().modify(|_, w| {
            w.clk_wifibb_22m_en()
                .set_bit()
                .clk_wifibb_40m_en()
                .set_bit()
                .clk_wifibb_44m_en()
                .set_bit()
                .clk_wifibb_80m_en()
                .set_bit()
                .clk_wifibb_40x_en()
                .set_bit()
                .clk_wifibb_80x_en()
                .set_bit()
                .clk_wifibb_40x1_en()
                .set_bit()
                .clk_wifibb_80x1_en()
                .set_bit()
                .clk_wifibb_160x1_en()
                .set_bit()
                .clk_wifi_apb_en()
                .set_bit()
                .clk_fe_80m_en()
                .set_bit()
                .clk_fe_160m_en()
                .set_bit()
                .clk_fe_apb_en()
                .set_bit()
                .clk_bt_apb_en()
                .set_bit()
                .clk_btbb_en()
                .set_bit()
                .clk_fe_pwdet_adc_en()
                .set_bit()
                .clk_fe_adc_en()
                .set_bit()
                .clk_fe_dac_en()
                .set_bit()
        });
    }

    /// Select the 160 MHz source for the PHY-I²C master.
    pub fn select_phy_i2c_160mhz_source(&mut self) {
        self.peripherals
            .modem_syscon
            .clk_conf()
            .modify(|_, w| w.clk_i2c_mst_sel_160m().set_bit());
    }

    /// Enable the shared PHY-I²C master clock.
    pub fn enable_phy_i2c_master_clock(&mut self) {
        self.peripherals
            .modem_lpcon
            .clk_conf()
            .modify(|_, w| w.clk_i2c_mst_en().set_bit());
    }

    /// Raw images used only by the HAL's bounded post-sequence verification.
    pub fn power_clock_images(&mut self) -> PowerClockImages {
        PowerClockImages {
            modem_reset: self.peripherals.modem_syscon.modem_rst_conf().read().bits(),
            hp_active_icg: self.peripherals.pmu.hp_active_icg_modem().read().bits(),
            modem_bus_clock: self.peripherals.hp_sys_clkrst.modem_ctrl0().read().bits(),
            hp_active_clock_map: self
                .peripherals
                .modem_syscon
                .clk_conf_power_st()
                .read()
                .bits(),
            shared_clock_map: self
                .peripherals
                .modem_lpcon
                .clk_conf_power_st()
                .read()
                .bits(),
            modem_clock_source: self.peripherals.hp_sys_clkrst.modem_conf().read().bits(),
            phy_clocks: self.peripherals.modem_syscon.clk_conf1().read().bits(),
            i2c_source: self.peripherals.modem_syscon.clk_conf().read().bits(),
            i2c_clock: self.peripherals.modem_lpcon.clk_conf().read().bits(),
        }
    }
}

/// Register images captured after the cold clock/reset sequence.
///
/// These values are deliberately not register handles: observing a checkpoint
/// must not let the HAL retain or duplicate access to the generated PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerClockImages {
    pub modem_reset: u32,
    pub hp_active_icg: u32,
    pub modem_bus_clock: u32,
    pub hp_active_clock_map: u32,
    pub shared_clock_map: u32,
    pub modem_clock_source: u32,
    pub phy_clocks: u32,
    pub i2c_source: u32,
    pub i2c_clock: u32,
}
