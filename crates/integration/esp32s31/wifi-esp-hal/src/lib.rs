#![no_std]
#![cfg(feature = "esp32s31")]

//! ESP-HAL ownership adapter for the open ESP32-S31 radio driver.
//!
//! The open driver owns the recovered cold-start sequence. This adapter owns
//! the documented chip-level peripheral singletons and realizes each semantic
//! operation through the official `esp32s31` svd2rust PAC used by `esp-hal`.

use esp_hal::{
    interrupt::{self, InterruptHandler},
    peripherals::{
        HP_SYS_CLKRST, I2C_ANA_MST, Interrupt, LP_AON_CLK_RST, LP_PERI, LP_TSENS, MODEM_LPCON,
        MODEM_SYSCON, PMU, WIFI,
    },
    rng::Rng,
};
use open_esp_radio_esp32s31_hal::{
    PowerClockControl, PowerClockImages,
    analog_i2c::PhyPmuControl,
    phy_i2c::{PhyI2cHost, PhyI2cMasterControl},
    phy_prelude::PhyPreludePlatformControl,
    phy_temperature::PhyTemperatureSystemControl,
    power_detector_platform::PhyPowerDetectorPlatformControl,
    wifi_bb::PhyWifiBbControl,
};
use open_esp_radio_esp32s31_phy::PhyTxTargetPowerProfile;
use open_esp_radio_esp32s31_wifi_mac::init::{
    MacClockControl, MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacDelayEntropy,
    MacSlowClockCalibrationSource, MacTxPowerPair, MacTxPowerSource,
};

/// Complete platform capability needed by the open radio power transition.
///
/// Keeping these singleton tokens together prevents the application from
/// independently constructing another safe owner while `Radio<Self>` is live.
/// `esp-hal` currently exposes register access as associated methods, so the
/// fields themselves are retained as ownership proofs rather than dereferenced.
pub struct EspHalRadioPeripheral {
    _wifi: WIFI<'static>,
    _modem_syscon: MODEM_SYSCON<'static>,
    _modem_lpcon: MODEM_LPCON<'static>,
    _hp_sys_clkrst: HP_SYS_CLKRST<'static>,
    _pmu: PMU<'static>,
    _lp_aon_clkrst: LP_AON_CLK_RST<'static>,
    _lp_peri: LP_PERI<'static>,
    _lp_tsens: LP_TSENS<'static>,
    _i2c_ana_mst: I2C_ANA_MST<'static>,
    phy_tx_power: Option<PhyTxTargetPowerProfile>,
}

impl EspHalRadioPeripheral {
    pub fn new(
        wifi: WIFI<'static>,
        modem_syscon: MODEM_SYSCON<'static>,
        modem_lpcon: MODEM_LPCON<'static>,
        hp_sys_clkrst: HP_SYS_CLKRST<'static>,
        pmu: PMU<'static>,
        lp_aon_clkrst: LP_AON_CLK_RST<'static>,
        lp_peri: LP_PERI<'static>,
        lp_tsens: LP_TSENS<'static>,
        i2c_ana_mst: I2C_ANA_MST<'static>,
    ) -> Self {
        Self {
            _wifi: wifi,
            _modem_syscon: modem_syscon,
            _modem_lpcon: modem_lpcon,
            _hp_sys_clkrst: hp_sys_clkrst,
            _pmu: pmu,
            _lp_aon_clkrst: lp_aon_clkrst,
            _lp_peri: lp_peri,
            _lp_tsens: lp_tsens,
            _i2c_ana_mst: i2c_ana_mst,
            phy_tx_power: None,
        }
    }

    /// Transfer the calibrated Rust-owned PHY target-power snapshot into the
    /// platform capability consumed by cold MAC initialization.
    pub fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        self.phy_tx_power = Some(profile);
    }

    /// Bind both ESP32-S31 Wi-Fi interrupt lines while this value proves
    /// ownership of the virtual `WIFI` singleton.
    pub fn bind_interrupts(&self, mac: InterruptHandler, power: InterruptHandler) {
        interrupt::bind_handler(Interrupt::WIFI_MAC, mac);
        interrupt::bind_handler(Interrupt::WIFI_PWR, power);
    }
}

impl PowerClockControl for EspHalRadioPeripheral {
    fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        MODEM_SYSCON::regs()
            .modem_rst_conf()
            .modify(|_, w| w.rst_wifibb().bit(asserted).rst_wifimac().bit(asserted));
    }

    fn select_hp_active_modem_icg(&mut self) {
        // SAFETY: code 2 fits this two-bit official PAC field. The value and
        // operation are from esp-hal S31 clock oracle commit 6899213e.
        PMU::regs()
            .hp_active_icg_modem()
            .write(|w| unsafe { w.hp_active_dig_icg_modem_code().bits(2) });
    }

    fn apply_modem_icg_selection(&mut self) {
        PMU::regs()
            .imm_modem_icg()
            .write(|w| w.update_dig_icg_modem_en().set_bit());
    }

    fn apply_sleep_icg_selection(&mut self) {
        PMU::regs()
            .imm_sleep_sysclk()
            .write(|w| w.update_dig_icg_switch().set_bit());
    }

    fn enable_modem_register_bus_clock(&mut self) {
        HP_SYS_CLKRST::regs()
            .modem_ctrl0()
            .modify(|_, w| w.modem_clk_en().set_bit());
    }

    fn configure_hp_active_modem_clock_map(&mut self) {
        // SAFETY: values 4/6 fit the official four-bit fields and reproduce
        // esp-hal S31 clock oracle commit 6899213e.
        MODEM_SYSCON::regs()
            .clk_conf_power_st()
            .modify(|_, w| unsafe {
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

    fn configure_shared_modem_clock_map(&mut self) {
        // SAFETY: value 6 fits all official four-bit fields and reproduces
        // esp-hal S31 clock oracle commit 6899213e.
        MODEM_LPCON::regs()
            .clk_conf_power_st()
            .modify(|_, w| unsafe {
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

    fn configure_modem_source_clocks(&mut self) {
        HP_SYS_CLKRST::regs().modem_conf().write(|w| {
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

    fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        MODEM_SYSCON::regs()
            .modem_rst_conf()
            .modify(|_, w| w.rst_wifibb().bit(asserted));
    }

    fn enable_phy_calibration_clocks(&mut self) {
        MODEM_SYSCON::regs().clk_conf1().modify(|_, w| {
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

    fn select_phy_i2c_160mhz_source(&mut self) {
        MODEM_SYSCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_i2c_mst_sel_160m().set_bit());
    }

    fn enable_phy_i2c_master_clock(&mut self) {
        MODEM_LPCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_i2c_mst_en().set_bit());
    }

    fn power_clock_images(&self) -> PowerClockImages {
        let modem_reset = MODEM_SYSCON::regs().modem_rst_conf().read();
        let hp_active_icg = PMU::regs().hp_active_icg_modem().read();
        let modem_bus_clock = HP_SYS_CLKRST::regs().modem_ctrl0().read();
        let hp_active_map = MODEM_SYSCON::regs().clk_conf_power_st().read();
        let shared_map = MODEM_LPCON::regs().clk_conf_power_st().read();
        let modem_source = HP_SYS_CLKRST::regs().modem_conf().read();
        let phy_clocks = MODEM_SYSCON::regs().clk_conf1().read();
        let i2c_source = MODEM_SYSCON::regs().clk_conf().read();
        let i2c_clock = MODEM_LPCON::regs().clk_conf().read();

        PowerClockImages {
            reset_released: modem_reset.rst_wifibb().bit_is_clear()
                && modem_reset.rst_wifimac().bit_is_clear(),
            hp_active_icg_selected: hp_active_icg.hp_active_dig_icg_modem_code().bits() == 2,
            modem_bus_clock_enabled: modem_bus_clock.modem_clk_en().bit_is_set(),
            hp_active_clock_map_configured: hp_active_map.clk_zb_st_map().bits() == 4
                && hp_active_map.clk_fe_st_map().bits() == 6
                && hp_active_map.clk_bt_st_map().bits() == 4
                && hp_active_map.clk_wifi_st_map().bits() == 6
                && hp_active_map.clk_modem_peri_st_map().bits() == 4
                && hp_active_map.clk_modem_apb_st_map().bits() == 6,
            shared_clock_map_configured: shared_map.clk_wifipwr_st_map().bits() == 6
                && shared_map.clk_coex_st_map().bits() == 6
                && shared_map.clk_i2c_mst_st_map().bits() == 6
                && shared_map.clk_lp_apb_st_map().bits() == 6,
            modem_source_clocks_configured: modem_source.modem_apb_clk_en().bit_is_set()
                && modem_source.modem_rst_en().bit_is_clear()
                && modem_source.modem_clk_en().bit_is_set()
                && modem_source.modem_clk_source_sel().bit_is_set()
                && modem_source.modem_pll_clk_en().bit_is_set()
                && modem_source.modem_xtal_clk_en().bit_is_set(),
            phy_calibration_clocks_enabled: phy_clocks.clk_wifibb_22m_en().bit_is_set()
                && phy_clocks.clk_wifibb_40m_en().bit_is_set()
                && phy_clocks.clk_wifibb_44m_en().bit_is_set()
                && phy_clocks.clk_wifibb_80m_en().bit_is_set()
                && phy_clocks.clk_wifibb_40x_en().bit_is_set()
                && phy_clocks.clk_wifibb_80x_en().bit_is_set()
                && phy_clocks.clk_wifibb_40x1_en().bit_is_set()
                && phy_clocks.clk_wifibb_80x1_en().bit_is_set()
                && phy_clocks.clk_wifibb_160x1_en().bit_is_set()
                && phy_clocks.clk_wifi_apb_en().bit_is_set()
                && phy_clocks.clk_fe_80m_en().bit_is_set()
                && phy_clocks.clk_fe_160m_en().bit_is_set()
                && phy_clocks.clk_fe_apb_en().bit_is_set()
                && phy_clocks.clk_bt_apb_en().bit_is_set()
                && phy_clocks.clk_btbb_en().bit_is_set()
                && phy_clocks.clk_fe_pwdet_adc_en().bit_is_set()
                && phy_clocks.clk_fe_adc_en().bit_is_set()
                && phy_clocks.clk_fe_dac_en().bit_is_set(),
            phy_i2c_160mhz_selected: i2c_source.clk_i2c_mst_sel_160m().bit_is_set(),
            phy_i2c_master_clock_enabled: i2c_clock.clk_i2c_mst_en().bit_is_set(),
        }
    }
}

impl PhyPreludePlatformControl for EspHalRadioPeripheral {
    fn configure_fixed_xtal_40mhz_tick(&mut self) {
        // SAFETY: 39 fits the official six-bit field. Complete pinned
        // libphy.a[phy_init.o]::phy_get_xtal_freq replaces the target with
        // frequency_mhz - 1; ESP32-S31 has a fixed 40 MHz crystal contract.
        MODEM_LPCON::regs()
            .tick_conf()
            .modify(|_, w| unsafe { w.modem_pwr_tick_target().bits(39) });
    }
}

impl PhyWifiBbControl for EspHalRadioPeripheral {
    fn clear_cold_start_wifi_control(&mut self) {
        // SOURCE[BLOB_LIBPHY_REGISTER_CHIPV7_PHY]. One official-PAC RMW
        // preserves the complete blob's single low-two-bit clear edge.
        MODEM_SYSCON::regs().wifi_bb_cfg().modify(|_, w| {
            w.cold_start_clear_unknown()
                .clear_bit()
                .wifi_enable()
                .clear_bit()
        });
    }

    fn wifi_baseband_is_enabled(&self) -> bool {
        // SOURCE[ROM_REV0_PHY_PBUS]; complete `phy_pbus_force_mode(0)`
        // samples this bit to decide whether the settle pulse is required.
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .read()
            .wifi_enable()
            .bit_is_set()
    }

    fn set_wifi_baseband_enabled(&mut self, enabled: bool) {
        // SOURCE[ROM_REV0_PHY_FREQUENCY_CHANNEL]; complete
        // `phy_wifi_enable_set` is exactly one fresh RMW of this bit.
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .modify(|_, w| w.wifi_enable().bit(enabled));
    }

    fn set_bss_cbw_40_digital(&mut self, enabled: bool) {
        // SOURCE[ROM_REV0_PHY_FREQUENCY_CHANNEL]. The recovered field is two
        // bits wide but the complete digital helper writes only encodings 0/1.
        // SAFETY: both values fit the official two-bit PAC field.
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .modify(|_, w| unsafe { w.bss_cbw_40_digital_unknown().bits(u8::from(enabled)) });
    }

    fn set_bb_agc_update_encoding(&mut self, encoding: u8) {
        // SOURCE[ROM_REV0_PHY_AGC,BLOB_LIBPHY_PHY_BB_INIT]. The two complete
        // bodies write encodings 7 and 1 respectively.
        debug_assert!(encoding <= 7);
        // SAFETY: the assertion documents the recovered three-bit range; all
        // driver call sites use only the instruction-evidenced values 1/7.
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .modify(|_, w| unsafe { w.bb_agc_update_enable_unknown().bits(encoding) });
    }

    fn set_mac_baseband_enabled(&mut self, enabled: bool) {
        // SOURCE[ROM_REV0_PHY_AGC]; complete `phy_mac_enable_bb` sets this
        // field before pulsing Wi-Fi enable in two further RMW edges.
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .modify(|_, w| w.mac_baseband_enable_unknown().bit(enabled));
    }
}

impl PhyPmuControl for EspHalRadioPeripheral {
    fn set_rf_circuit_power(&mut self, enabled: bool) {
        // SOURCE[BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW]; the complete ESP32-S31
        // libphy.a[phy_reg.o] body writes all 16 RF-circuit power bits.
        // SAFETY: both possible values fit the official 16-bit PAC field.
        PMU::regs()
            .rf_pwc()
            .modify(|_, w| unsafe { w.xpd_rf_circuit().bits(if enabled { u16::MAX } else { 0 }) });
    }

    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        // SOURCE[BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW]. The esp-pacs S31 patch
        // records why this header-WT register is read-write in the PAC: the
        // complete blob body performs this same read/modify/write operation.
        PMU::regs()
            .imm_hp_ck_power_0()
            .modify(|_, w| w.tie_high_xpd_bb_i2c().bit(enabled));
    }

    fn analog_i2c_is_powered(&self) -> bool {
        PMU::regs()
            .ana_peri_pwr_ctrl()
            .read()
            .xpd_perif_i2c()
            .bit_is_set()
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        PMU::regs()
            .ana_peri_pwr_ctrl()
            .modify(|_, w| w.xpd_perif_i2c().bit(enabled));
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        PMU::regs()
            .ana_peri_pwr_ctrl()
            .read()
            .rstb_perif_i2c()
            .bit_is_set()
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        PMU::regs()
            .ana_peri_pwr_ctrl()
            .modify(|_, w| w.rstb_perif_i2c().bit(released));
    }

    fn enable_frontend_baseband_power(&mut self) {
        // SOURCE[ROM_REV0_PHY_OPEN_FE_BB_CLK]. Its complete no-call ROM body
        // sets bits 3:0 and HP_ACTIVE_XPD_BB_I2C (bit 22). The new PAC field
        // deliberately keeps the low nibble's semantics marked unknown.
        // SAFETY: 0x0f fits the four-bit evidence-only field.
        PMU::regs().hp_active_hp_ck_power().modify(|_, w| unsafe {
            w.rom_open_fe_bb_unknown_low()
                .bits(0x0f)
                .hp_active_xpd_bb_i2c()
                .set_bit()
        });
    }
}

impl PhyPowerDetectorPlatformControl for EspHalRadioPeripheral {
    fn select_power_detector_initialization_mode(&mut self) {
        // SOURCE[ROM_REV0_PHY_POWER_DETECTOR]. Complete `phy_pwdet_reg_init`
        // and `phy_pwdet_sar2_init` replace the official three-bit
        // LP_AON_CLKRST field with encoding four.
        // SAFETY: 4 fits the official three-bit PAC field.
        LP_AON_CLK_RST::regs()
            .rtc_sar2_pwdet_cct()
            .modify(|_, w| unsafe { w.rtc_sar2_pwdet_cct().bits(4) });
    }

    fn select_power_detector_calibration_mode(&mut self) {
        // SOURCE[ROM_REV0_PHY_POWER_DETECTOR]. Complete
        // `phy_txcal_debuge_mode_` replaces the same official field with
        // encoding two after enabling PWDET.
        // SAFETY: 2 fits the official three-bit PAC field.
        LP_AON_CLK_RST::regs()
            .rtc_sar2_pwdet_cct()
            .modify(|_, w| unsafe { w.rtc_sar2_pwdet_cct().bits(2) });
    }
}

impl PhyTemperatureSystemControl for EspHalRadioPeripheral {
    fn enable_temperature_sensor_register_bank(&mut self) {
        // SOURCE[BLOB_LIBPHY_PHY_TSENS_READ_INIT]. First fresh RMW.
        LP_TSENS::regs()
            .clk_conf()
            .modify(|_, w| w.clk_en().set_bit());
    }

    fn enable_temperature_sensor_clock(&mut self) {
        // SOURCE[BLOB_LIBPHY_PHY_TSENS_READ_INIT]. Complete
        // libphy.a[phy_tsens.o]::phy_tsens_read_init sets this official bit
        // between the first and second LP_TSENS read-path RMW operations.
        LP_PERI::regs()
            .tsens_ctrl()
            .modify(|_, w| w.lp_tsens_clk_en().set_bit());
    }

    fn enable_temperature_sensor_phy_readout(&mut self) {
        // SOURCE[BLOB_LIBPHY_PHY_TSENS_READ_INIT]. Third fresh RMW; the PAC
        // keeps the electrical meaning explicitly unknown.
        LP_TSENS::regs()
            .clk_conf()
            .modify(|_, w| w.phy_readout_enable_unknown().set_bit());
    }

    fn enable_temperature_sensor_phy_conversion(&mut self) {
        // SOURCE[BLOB_LIBPHY_PHY_TSENS_READ_INIT]. Fourth fresh RMW; the PAC
        // keeps the electrical meaning explicitly unknown.
        LP_TSENS::regs()
            .clk_conf()
            .modify(|_, w| w.phy_conversion_enable_unknown().set_bit());
    }

    fn enable_temperature_sensor_power(&mut self) {
        // SOURCE[ROM_REV0_PHY_TSENS]. Complete `phy_set_tsens_power_(1)`.
        LP_TSENS::regs()
            .ctrl()
            .modify(|_, w| w.power_up().set_bit());
    }

    fn read_temperature_sensor_code(&self) -> u8 {
        // SOURCE[ROM_REV0_PHY_TSENS]. Complete `phy_tsens_code_read` and
        // `phy_tsens_temp_read_local` each consume this low-byte field once.
        LP_TSENS::regs().ctrl().read().out().bits()
    }
}

impl PhyI2cMasterControl for EspHalRadioPeripheral {
    fn configure_phy_i2c_host_map(&mut self) {
        // SOURCE[BLOB_LIBPHY_PHY_I2C]. The complete S31 host callback
        // replaces ANA_CONF2 bits 17:4 with 0x3fa0.
        I2C_ANA_MST::regs()
            .ana_conf2()
            .modify(|r, w| unsafe { w.bits((r.bits() & 0xfffc_000f) | 0x0003_fa00) });
    }

    fn pulse_phy_i2c_master_reset(&mut self, host: PhyI2cHost) {
        // SOURCE[ROM_REV0_PHY_I2C]. Complete phy_i2c_master_reset writes only
        // START_OR_RESET (bit 26) to the selected host.
        match host {
            PhyI2cHost::Host0 => I2C_ANA_MST::regs()
                .i2c0_ctrl()
                .write(|w| w.start_or_reset().set_bit()),
            PhyI2cHost::Host1 => I2C_ANA_MST::regs()
                .i2c1_ctrl()
                .write(|w| w.start_or_reset().set_bit()),
        };
    }

    fn phy_i2c_master_is_busy(&self, host: PhyI2cHost) -> bool {
        // SOURCE[ROM_REV0_PHY_I2C]. Both complete host paths sample bit 25.
        match host {
            PhyI2cHost::Host0 => I2C_ANA_MST::regs().i2c0_ctrl().read().busy().bit_is_set(),
            PhyI2cHost::Host1 => I2C_ANA_MST::regs().i2c1_ctrl().read().busy().bit_is_set(),
        }
    }

    fn publish_phy_i2c_read_mask(&mut self, read_mask: u16) {
        // SOURCE[BLOB_LIBPHY_PHY_I2C]. The callback publishes the complete
        // 32-bit complement, including the ANA_STATUS1 byte.
        I2C_ANA_MST::regs()
            .ana_conf1()
            .write(|w| unsafe { w.bits(!u32::from(read_mask)) });
    }

    fn publish_phy_i2c_command(
        &mut self,
        host: PhyI2cHost,
        block: u8,
        register: u8,
        value: u8,
        write: bool,
    ) {
        // SOURCE[ROM_REV0_PHY_I2C]. Complete read/write leaves publish the
        // three bytes, direction bit and START_OR_RESET in one full word.
        match host {
            PhyI2cHost::Host0 => I2C_ANA_MST::regs().i2c0_ctrl().write(|w| unsafe {
                w.slave_addr()
                    .bits(block)
                    .slave_reg_addr()
                    .bits(register)
                    .data()
                    .bits(value)
                    .read_write()
                    .bit(write)
                    .start_or_reset()
                    .set_bit()
            }),
            PhyI2cHost::Host1 => I2C_ANA_MST::regs().i2c1_ctrl().write(|w| unsafe {
                w.slave_addr()
                    .bits(block)
                    .slave_reg_addr()
                    .bits(register)
                    .data()
                    .bits(value)
                    .read_write()
                    .bit(write)
                    .start_or_reset()
                    .set_bit()
            }),
        };
    }

    fn sample_phy_i2c_result(&self, host: PhyI2cHost) -> u8 {
        // SOURCE[ROM_REV0_PHY_I2C]. Completed reads return bits 23:16.
        match host {
            PhyI2cHost::Host0 => I2C_ANA_MST::regs().i2c0_ctrl().read().data().bits(),
            PhyI2cHost::Host1 => I2C_ANA_MST::regs().i2c1_ctrl().read().data().bits(),
        }
    }

    fn set_phy_i2c_clock_selection_high(&mut self, index: usize, value: u8) -> bool {
        if value > 0x1f {
            return false;
        }
        // SOURCE[ROM_REV0_PHY_I2C]. First fresh RMW for each of the three
        // complete phy_i2c_clk_sel timing words.
        unsafe {
            match index {
                0 => I2C_ANA_MST::regs()
                    .i2c0_ctrl1()
                    .modify(|_, w| w.i2c0_sda_side_guard().bits(value)),
                1 => I2C_ANA_MST::regs()
                    .i2c1_ctrl1()
                    .modify(|_, w| w.i2c1_sda_side_guard().bits(value)),
                2 => I2C_ANA_MST::regs()
                    .hw_i2c_ctrl()
                    .modify(|_, w| w.hw_i2c_sda_side_guard().bits(value)),
                _ => return false,
            };
        }
        true
    }

    fn set_phy_i2c_clock_selection_low(&mut self, index: usize, value: u8) -> bool {
        if value > 0x3f {
            return false;
        }
        // SOURCE[ROM_REV0_PHY_I2C]. Second fresh RMW for each timing word.
        unsafe {
            match index {
                0 => I2C_ANA_MST::regs()
                    .i2c0_ctrl1()
                    .modify(|_, w| w.i2c0_scl_pulse_dur().bits(value)),
                1 => I2C_ANA_MST::regs()
                    .i2c1_ctrl1()
                    .modify(|_, w| w.i2c1_scl_pulse_dur().bits(value)),
                2 => I2C_ANA_MST::regs()
                    .hw_i2c_ctrl()
                    .modify(|_, w| w.hw_i2c_scl_pulse_dur().bits(value)),
                _ => return false,
            };
        }
        true
    }

    fn set_phy_i2c_register_mode(&mut self, mode: u8) -> bool {
        if mode > 3 {
            return false;
        }
        // SOURCE[ROM_REV0_PHY_I2C]. Complete phy_i2cmst_reg_init selects 2.
        I2C_ANA_MST::regs()
            .ana_conf0()
            .modify(|_, w| unsafe { w.phy_register_mode().bits(mode) });
        true
    }

    fn enable_phy_i2c_register_mode(&mut self) {
        // SOURCE[ROM_REV0_PHY_I2C]. Separate fresh RMW after mode selection.
        I2C_ANA_MST::regs()
            .ana_conf0()
            .modify(|_, w| w.phy_register_enable().set_bit());
    }

    fn set_phy_i2c_bbpll_calibration(&mut self, enabled: bool) {
        let mode = if enabled { 2 } else { 1 };
        // SOURCE[ROM_REV0_PHY_I2C]. Complete phy_bbpll_cal uses only 1/2.
        I2C_ANA_MST::regs()
            .ana_conf0()
            .modify(|_, w| unsafe { w.bbpll_cal_mode_unknown().bits(mode) });
    }
}

impl MacClockControl for EspHalRadioPeripheral {
    fn enable_wifi_mac_clocks(&mut self) {
        MODEM_SYSCON::regs().clk_conf1().modify(|_, w| {
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
                .clk_wifimac_en()
                .set_bit()
                .clk_wifi_apb_en()
                .set_bit()
        });
    }

    fn enable_coexistence_clock(&mut self) {
        MODEM_LPCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_coex_en().set_bit());
    }

    fn configure_modem_source_clocks(&mut self) {
        PowerClockControl::configure_modem_source_clocks(self);
    }

    fn set_wifi_mac_reset(&mut self, asserted: bool) {
        MODEM_SYSCON::regs()
            .modem_rst_conf()
            .modify(|_, w| w.rst_wifimac().bit(asserted));
    }
}

impl MacDelayEntropy for EspHalRadioPeripheral {
    fn mac_delay_random(&mut self) -> u32 {
        // SOURCE: complete libpp hal_he_set_mac_delay on-chip branch obtains
        // `_random()` from g_wifi_osi_funcs. The esp-hal adapter implements
        // that callback with this same safe RNG facade.
        Rng::new().random()
    }
}

impl MacSlowClockCalibrationSource for EspHalRadioPeripheral {
    fn mac_slow_clock_calibration(&mut self) -> u32 {
        // SOURCE: esp-hal branch esp32s31-async-platform commit 07de554,
        // esp-radio/src/wifi/internal.rs installs slowclk_cal_get at the S31
        // OSI slot, and esp-radio/src/wifi/os_adapter/mod.rs currently returns
        // zero for S31 with an explicit TODO. Keep that oracle behavior visible
        // here; a future real calibration belongs behind this platform trait.
        0
    }
}

impl MacTxPowerSource for EspHalRadioPeripheral {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        let Some(profile) = &self.phy_tx_power else {
            // Cold MAC init is ordered after the open PHY profile transfer.
            // Keep accidental misuse fail-closed without consulting vendor
            // global state or panicking in the hardware bring-up path.
            return MacTxPowerPair::ZERO;
        };
        let pair = profile.pair(rate);
        MacTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}

impl MacCoexPtiSource for EspHalRadioPeripheral {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        // These cold values configure the MAC's own scheduler even though this
        // integration starts no Bluetooth/802.15.4 coexistence runtime. In
        // particular, complete `hal_init` publishes event three as RX_ACK PTI
        // seven. The former deterministic-zero substitution let pending BE
        // TX (PTI one) outrank an immediate response: RX-only HIL passed, but
        // concurrent TX produced thousands of WDEVRX_ABORT_FCS_PASS events.
        //
        // SOURCE: complete `_oracles/libcoexist.a[coexist_core.o]::
        // coex_pti_tab` and `_oracles/libpp.a[hal_mac.o,hal_coex.o]`.
        event.cold_vendor_pti()
    }
}
