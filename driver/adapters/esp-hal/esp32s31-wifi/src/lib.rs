#![no_std]
#![cfg(feature = "esp32s31")]
#![forbid(unsafe_code)]

//! ESP-HAL ownership adapter for the open ESP32-S31 radio driver.
//!
//! The open driver owns the recovered cold-start sequence. This adapter retains
//! the documented chip-level singleton tokens. Route-owned radio words are
//! accessed only through the affine custom PAC; the remaining platform words
//! use the official `esp32s31` PAC until their own reviewed carveouts exist.

use esp_hal::{
    interrupt::{self, InterruptHandler},
    peripherals::{
        HP_SYS_CLKRST, I2C_ANA_MST, Interrupt, LP_AON_CLK_RST, LP_PERI, LP_TSENS, MODEM_LPCON,
        MODEM_SYSCON, PMU, WIFI,
    },
    rng::Rng,
    system::Cpu,
};
use open_esp_radio_esp32s31_hal::{
    PlatformPowerClockImages, PowerClockControl,
    analog_i2c::PhyPmuControl,
    phy_i2c::{PhyI2cHost, PhyI2cMasterControl},
};
use open_esp_radio_esp32s31_phy::PhyTxTargetPowerProfile;
use open_esp_radio_esp32s31_wifi::mac_start::Esp32s31WifiMacPlatform;
use open_esp_radio_esp32s31_wifi_mac::init::{
    MacClockControl, MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacDelayEntropy,
    MacSlowClockCalibration, MacSlowClockCalibrationSource, MacTxPowerPair, MacTxPowerSource,
};

pub mod ieee802154;
pub mod mac_interrupt_epoch;

/// Complete platform capability needed by the open radio power transition.
///
/// Keeping these singleton tokens together prevents the application from
/// independently constructing another safe owner while `Radio<Self>` is live.
/// `esp-hal` currently exposes register access as associated methods, so the
/// fields themselves are retained as ownership proofs. In particular,
/// `_modem_syscon` is never dereferenced outside the custom PAC route.
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

    /// Disable both Wi-Fi CPU interrupt routes on their binding core.
    ///
    /// This closes only the platform routing edge. The caller must then mask
    /// and acknowledge the peripheral banks before moving their PAC owners
    /// back into task-side setup. Binding and teardown are intentionally kept
    /// on the same core by the station lifecycle owner.
    pub fn disable_interrupts(&self) {
        let cpu = Cpu::current();
        interrupt::disable(cpu, Interrupt::WIFI_MAC);
        interrupt::disable(cpu, Interrupt::WIFI_PWR);
    }
}

impl Esp32s31WifiMacPlatform for EspHalRadioPeripheral {
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        EspHalRadioPeripheral::install_phy_tx_power_profile(self, profile);
    }
}

impl PowerClockControl for EspHalRadioPeripheral {
    fn select_hp_active_modem_icg(&mut self) {
        // Code 2 and the field constraint come from the qualified esp-hal S31
        // clock implementation.
        PMU::regs()
            .hp_active_icg_modem()
            .write(|w| w.hp_active_dig_icg_modem_code().set(2));
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

    fn platform_power_clock_images(&self) -> PlatformPowerClockImages {
        let hp_active_icg = PMU::regs().hp_active_icg_modem().read();
        let modem_bus_clock = HP_SYS_CLKRST::regs().modem_ctrl0().read();
        let modem_source = HP_SYS_CLKRST::regs().modem_conf().read();

        PlatformPowerClockImages {
            hp_active_icg_selected: hp_active_icg.hp_active_dig_icg_modem_code().bits() == 2,
            modem_bus_clock_enabled: modem_bus_clock.modem_clk_en().bit_is_set(),
            modem_source_clocks_configured: modem_source.modem_apb_clk_en().bit_is_set()
                && modem_source.modem_rst_en().bit_is_clear()
                && modem_source.modem_clk_en().bit_is_set()
                && modem_source.modem_clk_source_sel().bit_is_set()
                && modem_source.modem_pll_clk_en().bit_is_set()
                && modem_source.modem_xtal_clk_en().bit_is_set(),
        }
    }
}

impl PhyPmuControl for EspHalRadioPeripheral {
    fn set_rf_circuit_power(&mut self, enabled: bool) {
        // SOURCE[BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW]; the complete ESP32-S31
        // libphy.a[phy_reg.o] body writes all 16 RF-circuit power bits.
        PMU::regs()
            .rf_pwc()
            .modify(|_, w| w.xpd_rf_circuit().set(if enabled { u16::MAX } else { 0 }));
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
        PMU::regs().hp_active_hp_ck_power().modify(|_, w| {
            w.rom_open_fe_bb_unknown_low()
                .set(0x0f)
                .hp_active_xpd_bb_i2c()
                .set_bit()
        });
    }
}

impl PhyI2cMasterControl for EspHalRadioPeripheral {
    fn configure_phy_i2c_host_map(&mut self) {
        // SOURCE[BLOB_LIBPHY_PHY_I2C]. The complete S31 host callback
        // replaces ANA_CONF2 bits 17:4 with 0x3fa0.
        I2C_ANA_MST::regs().ana_conf2().modify(|r, w| {
            w.ana_conf2().set(
                open_esp_radio_esp32s31_hal::phy_i2c::configured_host_map_image(
                    r.ana_conf2().bits(),
                ),
            )
        });
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
        let image = !u32::from(read_mask);
        I2C_ANA_MST::regs().ana_conf1().write(|w| {
            w.ana_conf1()
                .set(image & 0x00ff_ffff)
                .ana_status1()
                .set((image >> 24) as u8)
        });
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
            PhyI2cHost::Host0 => I2C_ANA_MST::regs().i2c0_ctrl().write(|w| {
                w.slave_addr()
                    .set(block)
                    .slave_reg_addr()
                    .set(register)
                    .data()
                    .set(value)
                    .read_write()
                    .bit(write)
                    .start_or_reset()
                    .set_bit()
            }),
            PhyI2cHost::Host1 => I2C_ANA_MST::regs().i2c1_ctrl().write(|w| {
                w.slave_addr()
                    .set(block)
                    .slave_reg_addr()
                    .set(register)
                    .data()
                    .set(value)
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
        match index {
            0 => I2C_ANA_MST::regs()
                .i2c0_ctrl1()
                .modify(|_, w| w.i2c0_sda_side_guard().set(value)),
            1 => I2C_ANA_MST::regs()
                .i2c1_ctrl1()
                .modify(|_, w| w.i2c1_sda_side_guard().set(value)),
            2 => I2C_ANA_MST::regs()
                .hw_i2c_ctrl()
                .modify(|_, w| w.hw_i2c_sda_side_guard().set(value)),
            _ => return false,
        };
        true
    }

    fn set_phy_i2c_clock_selection_low(&mut self, index: usize, value: u8) -> bool {
        if value > 0x3f {
            return false;
        }
        // SOURCE[ROM_REV0_PHY_I2C]. Second fresh RMW for each timing word.
        match index {
            0 => I2C_ANA_MST::regs()
                .i2c0_ctrl1()
                .modify(|_, w| w.i2c0_scl_pulse_dur().set(value)),
            1 => I2C_ANA_MST::regs()
                .i2c1_ctrl1()
                .modify(|_, w| w.i2c1_scl_pulse_dur().set(value)),
            2 => I2C_ANA_MST::regs()
                .hw_i2c_ctrl()
                .modify(|_, w| w.hw_i2c_scl_pulse_dur().set(value)),
            _ => return false,
        };
        true
    }

    fn set_phy_i2c_register_mode(&mut self, mode: u8) -> bool {
        if mode > 3 {
            return false;
        }
        // SOURCE[ROM_REV0_PHY_I2C]. Complete phy_i2cmst_reg_init selects 2.
        I2C_ANA_MST::regs()
            .ana_conf0()
            .modify(|_, w| w.phy_register_mode().set(mode));
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
            .modify(|_, w| w.bbpll_cal_mode_unknown().set(mode));
    }
}

impl MacClockControl for EspHalRadioPeripheral {
    fn configure_modem_source_clocks(&mut self) {
        PowerClockControl::configure_modem_source_clocks(self);
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
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        // SOURCE: the S31 esp-hal radio adapter installs slowclk_cal_get in
        // its OSI table and currently returns an unimplemented zero placeholder.
        // Keep that absence visible here; a future real calibration belongs behind
        // this platform trait and must return `Calibrated` with provenance.
        MacSlowClockCalibration::Unavailable
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
        // SOURCE: complete `libcoexist.a[coexist_core.o]::
        // coex_pti_tab` and `libpp.a[hal_mac.o,hal_coex.o]`.
        event.cold_vendor_pti()
    }
}
