//! ESP32-S31 register operations behind the role-neutral platform owner.
//!
//! The Bluetooth lease follows the pinned ESP-IDF controller lifecycle in
//! `components/bt/controller/esp32s31/bt.c`, `btdm_lp.c`, and the S31 modem
//! clock implementation. These paths define semantics; the PAC stays private.

use esp_hal::peripherals::{
    HP_SYS_CLKRST, I2C_ANA_MST, LP_AON_CLK_RST, LP_PERI, LP_TSENS, MODEM_LPCON, MODEM_SYSCON, PMU,
};
use open_esp_radio_esp32s31_bluetooth::{BluetoothClockControl, BluetoothClockState};
use open_esp_radio_esp32s31_hal::{
    analog_i2c::PhyPmuControl,
    phy_i2c::{PhyI2cHost, PhyI2cMasterControl},
    wifi_bb::PhyWifiBbControl,
};

use crate::coordinator::{
    BluetoothPlatformBusy, BluetoothPlatformLease, ClockCoordinator, ClockDevice, ClockIo,
    LowPowerClockState,
};

const ICG_NOGATING_ACTIVE: u8 = 4;
const ICG_NOGATING_ACTIVE_MODEM: u8 = 6;

/// Sole safe owner of the ESP32-S31 shared radio-platform singletons.
///
/// Construction consumes the official ESP-HAL singleton tokens. The tokens
/// stay private for the coordinator's whole lifetime; safe code can only issue
/// semantic leases. Existing Wi-Fi code cannot be composed simultaneously
/// until it too consumes a lease from this coordinator, because its current
/// separate adapter requires these same non-duplicable tokens.
pub struct EspHalRadioPlatform {
    _modem_syscon: MODEM_SYSCON<'static>,
    _modem_lpcon: MODEM_LPCON<'static>,
    _hp_sys_clkrst: HP_SYS_CLKRST<'static>,
    _pmu: PMU<'static>,
    _lp_aon_clkrst: LP_AON_CLK_RST<'static>,
    _lp_peri: LP_PERI<'static>,
    _lp_tsens: LP_TSENS<'static>,
    _i2c_ana_mst: I2C_ANA_MST<'static>,
    coordinator: ClockCoordinator<EspHalClockIo>,
}

impl EspHalRadioPlatform {
    /// Establish the neutral radio-platform owner after `esp_hal::init`.
    pub const fn new(
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
            _modem_syscon: modem_syscon,
            _modem_lpcon: modem_lpcon,
            _hp_sys_clkrst: hp_sys_clkrst,
            _pmu: pmu,
            _lp_aon_clkrst: lp_aon_clkrst,
            _lp_peri: lp_peri,
            _lp_tsens: lp_tsens,
            _i2c_ana_mst: i2c_ana_mst,
            coordinator: ClockCoordinator::new(EspHalClockIo),
        }
    }

    /// Reserve the only standalone Bluetooth clock lifecycle slot.
    ///
    /// Dependency clocks remain globally reference-counted inside this owner,
    /// so a future Wi-Fi lease can share ETM, coexistence and PLL sources
    /// without either role gating clocks still owned by the other.
    pub fn try_bluetooth(&self) -> Result<EspHalBluetoothPlatform<'_>, BluetoothPlatformBusy> {
        self.coordinator
            .try_bluetooth()
            .map(|inner| EspHalBluetoothPlatform { inner })
    }
}

/// Semantic ESP-HAL platform lease consumed by Bluetooth clock typestate.
///
/// This type deliberately exposes neither peripheral singleton tokens nor PAC
/// register blocks. Dropping an incompletely unwound lease restores every
/// clock acquired through it and releases the Bluetooth reservation.
pub struct EspHalBluetoothPlatform<'a> {
    inner: BluetoothPlatformLease<'a, EspHalClockIo>,
}

impl BluetoothClockControl for EspHalBluetoothPlatform<'_> {
    fn enable_bluetooth_controller_clocks(&mut self) {
        self.inner.enable_bluetooth_controller_clocks();
    }

    fn enable_bluetooth_apb_clocks(&mut self) {
        self.inner.enable_bluetooth_apb_clocks();
    }

    fn reset_bluetooth_controller_domains(&mut self) {
        self.inner.reset_bluetooth_controller_domains();
    }

    fn select_main_xtal_low_power_clock(&mut self, divider: u16) {
        self.inner.select_main_xtal_low_power_clock(divider);
    }

    fn bluetooth_clock_state(&mut self) -> BluetoothClockState {
        self.inner.bluetooth_clock_state()
    }

    fn deselect_low_power_clock(&mut self) {
        self.inner.deselect_low_power_clock();
    }

    fn disable_bluetooth_apb_clocks(&mut self) {
        self.inner.disable_bluetooth_apb_clocks();
    }

    fn disable_bluetooth_controller_clocks(&mut self) {
        self.inner.disable_bluetooth_controller_clocks();
    }
}

impl PhyWifiBbControl for EspHalBluetoothPlatform<'_> {
    fn clear_cold_start_wifi_control(&mut self) {
        // The common PHY transition temporarily owns this physical shared
        // baseband control even when Bluetooth is the requesting protocol.
        MODEM_SYSCON::regs().wifi_bb_cfg().modify(|_, w| {
            w.cold_start_clear_unknown()
                .clear_bit()
                .wifi_enable()
                .clear_bit()
        });
    }

    fn wifi_baseband_is_enabled(&self) -> bool {
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .read()
            .wifi_enable()
            .bit_is_set()
    }

    fn set_wifi_baseband_enabled(&mut self, enabled: bool) {
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .modify(|_, w| w.wifi_enable().bit(enabled));
    }

    fn set_bss_cbw_40_digital(&mut self, enabled: bool) {
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .modify(|_, w| w.bss_cbw_40_digital_unknown().set(u8::from(enabled)));
    }

    fn set_bb_agc_update_encoding(&mut self, encoding: u8) {
        debug_assert!(encoding <= 7, "PHY AGC update encoding exceeds its field");
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .modify(|_, w| w.bb_agc_update_enable_unknown().set(encoding));
    }

    fn set_mac_baseband_enabled(&mut self, enabled: bool) {
        MODEM_SYSCON::regs()
            .wifi_bb_cfg()
            .modify(|_, w| w.mac_baseband_enable_unknown().bit(enabled));
    }
}

impl PhyPmuControl for EspHalBluetoothPlatform<'_> {
    fn set_rf_circuit_power(&mut self, enabled: bool) {
        PMU::regs()
            .rf_pwc()
            .modify(|_, w| w.xpd_rf_circuit().set(if enabled { u16::MAX } else { 0 }));
    }

    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
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
        PMU::regs().hp_active_hp_ck_power().modify(|_, w| {
            w.rom_open_fe_bb_unknown_low()
                .set(0x0f)
                .hp_active_xpd_bb_i2c()
                .set_bit()
        });
    }
}

impl PhyI2cMasterControl for EspHalBluetoothPlatform<'_> {
    fn configure_phy_i2c_host_map(&mut self) {
        I2C_ANA_MST::regs().ana_conf2().modify(|r, w| {
            w.ana_conf2().set(
                open_esp_radio_esp32s31_hal::phy_i2c::configured_host_map_image(
                    r.ana_conf2().bits(),
                ),
            )
        });
    }

    fn pulse_phy_i2c_master_reset(&mut self, host: PhyI2cHost) {
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
        match host {
            PhyI2cHost::Host0 => I2C_ANA_MST::regs().i2c0_ctrl().read().busy().bit_is_set(),
            PhyI2cHost::Host1 => I2C_ANA_MST::regs().i2c1_ctrl().read().busy().bit_is_set(),
        }
    }

    fn publish_phy_i2c_read_mask(&mut self, read_mask: u16) {
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
        match host {
            PhyI2cHost::Host0 => I2C_ANA_MST::regs().i2c0_ctrl().read().data().bits(),
            PhyI2cHost::Host1 => I2C_ANA_MST::regs().i2c1_ctrl().read().data().bits(),
        }
    }

    fn set_phy_i2c_clock_selection_high(&mut self, index: usize, value: u8) -> bool {
        if value > 0x1f {
            return false;
        }
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
        I2C_ANA_MST::regs()
            .ana_conf0()
            .modify(|_, w| w.phy_register_mode().set(mode));
        true
    }

    fn enable_phy_i2c_register_mode(&mut self) {
        I2C_ANA_MST::regs()
            .ana_conf0()
            .modify(|_, w| w.phy_register_enable().set_bit());
    }

    fn set_phy_i2c_bbpll_calibration(&mut self, enabled: bool) {
        I2C_ANA_MST::regs()
            .ana_conf0()
            .modify(|_, w| w.bbpll_cal_mode_unknown().set(if enabled { 2 } else { 1 }));
    }
}

struct EspHalClockIo;

impl EspHalClockIo {
    fn set_pll_160m_source(enabled: bool) {
        if enabled {
            HP_SYS_CLKRST::regs()
                .ref_160m_ctrl0()
                .modify(|_, w| w.ref_160m_clk_en().set_bit());
        }

        HP_SYS_CLKRST::regs().modem_conf().write(|w| {
            w.modem_apb_clk_en()
                .set_bit()
                .modem_rst_en()
                .clear_bit()
                .modem_clk_en()
                .set_bit()
                .modem_clk_source_sel()
                .bit(enabled)
                .modem_pll_clk_en()
                .bit(enabled)
                .modem_xtal_clk_en()
                .set_bit()
        });

        if !enabled {
            HP_SYS_CLKRST::regs()
                .ref_160m_ctrl0()
                .modify(|_, w| w.ref_160m_clk_en().clear_bit());
        }
    }

    fn pll_160m_source_is_enabled() -> bool {
        let source = HP_SYS_CLKRST::regs().modem_conf().read();
        HP_SYS_CLKRST::regs()
            .ref_160m_ctrl0()
            .read()
            .ref_160m_clk_en()
            .bit_is_set()
            && source.modem_apb_clk_en().bit_is_set()
            && source.modem_rst_en().bit_is_clear()
            && source.modem_clk_en().bit_is_set()
            && source.modem_clk_source_sel().bit_is_set()
            && source.modem_pll_clk_en().bit_is_set()
            && source.modem_xtal_clk_en().bit_is_set()
    }
}

impl ClockIo for EspHalClockIo {
    fn prepare_icg_maps(&mut self) {
        MODEM_SYSCON::regs().clk_conf_power_st().modify(|r, w| {
            w.clk_modem_apb_st_map()
                .set(r.clk_modem_apb_st_map().bits() | ICG_NOGATING_ACTIVE_MODEM)
                .clk_modem_peri_st_map()
                .set(r.clk_modem_peri_st_map().bits() | ICG_NOGATING_ACTIVE)
                .clk_wifi_st_map()
                .set(r.clk_wifi_st_map().bits() | ICG_NOGATING_ACTIVE_MODEM)
                .clk_bt_st_map()
                .set(r.clk_bt_st_map().bits() | ICG_NOGATING_ACTIVE)
                .clk_fe_st_map()
                .set(r.clk_fe_st_map().bits() | ICG_NOGATING_ACTIVE_MODEM)
                .clk_zb_st_map()
                .set(r.clk_zb_st_map().bits() | ICG_NOGATING_ACTIVE)
        });
        MODEM_LPCON::regs().clk_conf_power_st().modify(|r, w| {
            w.clk_lp_apb_st_map()
                .set(r.clk_lp_apb_st_map().bits() | ICG_NOGATING_ACTIVE_MODEM)
                .clk_i2c_mst_st_map()
                .set(r.clk_i2c_mst_st_map().bits() | ICG_NOGATING_ACTIVE_MODEM)
                .clk_coex_st_map()
                .set(r.clk_coex_st_map().bits() | ICG_NOGATING_ACTIVE_MODEM)
                .clk_wifipwr_st_map()
                .set(r.clk_wifipwr_st_map().bits() | ICG_NOGATING_ACTIVE_MODEM)
        });
    }

    fn clock_is_enabled(&self, device: ClockDevice) -> bool {
        match device {
            ClockDevice::Pll160mSource => Self::pll_160m_source_is_enabled(),
            ClockDevice::Coexistence => MODEM_LPCON::regs()
                .clk_conf()
                .read()
                .clk_coex_en()
                .bit_is_set(),
            ClockDevice::WifiBaseband80x1 => MODEM_SYSCON::regs()
                .clk_conf1()
                .read()
                .clk_wifibb_80x1_en()
                .bit_is_set(),
            ClockDevice::Etm => MODEM_SYSCON::regs()
                .clk_conf()
                .read()
                .clk_etm_en()
                .bit_is_set(),
            ClockDevice::BluetoothMac => MODEM_SYSCON::regs()
                .clk_conf1()
                .read()
                .clk_btmac_en()
                .bit_is_set(),
            ClockDevice::BluetoothPeripheral => {
                let clocks = MODEM_SYSCON::regs().clk_conf().read();
                clocks.clk_modem_sec_en().bit_is_set()
                    && clocks.clk_modem_sec_ecb_en().bit_is_set()
                    && clocks.clk_modem_sec_ccm_en().bit_is_set()
                    && clocks.clk_modem_sec_bah_en().bit_is_set()
                    && clocks.clk_ble_timer_en().bit_is_set()
            }
            ClockDevice::BluetoothApb => {
                MODEM_SYSCON::regs()
                    .clk_conf1()
                    .read()
                    .clk_bt_apb_en()
                    .bit_is_set()
                    && MODEM_SYSCON::regs()
                        .clk_conf()
                        .read()
                        .clk_modem_sec_apb_en()
                        .bit_is_set()
            }
            ClockDevice::BluetoothBaseband => MODEM_SYSCON::regs()
                .clk_conf1()
                .read()
                .clk_btbb_en()
                .bit_is_set(),
            ClockDevice::Count => false,
        }
    }

    fn set_clock_enabled(&mut self, device: ClockDevice, enabled: bool) {
        match device {
            ClockDevice::Pll160mSource => Self::set_pll_160m_source(enabled),
            ClockDevice::Coexistence => {
                MODEM_LPCON::regs()
                    .clk_conf()
                    .modify(|_, w| w.clk_coex_en().bit(enabled));
            }
            ClockDevice::WifiBaseband80x1 => {
                MODEM_SYSCON::regs()
                    .clk_conf1()
                    .modify(|_, w| w.clk_wifibb_80x1_en().bit(enabled));
            }
            ClockDevice::Etm => {
                MODEM_SYSCON::regs()
                    .clk_conf()
                    .modify(|_, w| w.clk_etm_en().bit(enabled));
            }
            ClockDevice::BluetoothMac => {
                MODEM_SYSCON::regs()
                    .clk_conf1()
                    .modify(|_, w| w.clk_btmac_en().bit(enabled));
            }
            ClockDevice::BluetoothPeripheral => {
                MODEM_SYSCON::regs().clk_conf().modify(|_, w| {
                    w.clk_modem_sec_en()
                        .bit(enabled)
                        .clk_modem_sec_ecb_en()
                        .bit(enabled)
                        .clk_modem_sec_ccm_en()
                        .bit(enabled)
                        .clk_modem_sec_bah_en()
                        .bit(enabled)
                        .clk_ble_timer_en()
                        .bit(enabled)
                });
            }
            ClockDevice::BluetoothApb => {
                MODEM_SYSCON::regs()
                    .clk_conf1()
                    .modify(|_, w| w.clk_bt_apb_en().bit(enabled));
                MODEM_SYSCON::regs()
                    .clk_conf()
                    .modify(|_, w| w.clk_modem_sec_apb_en().bit(enabled));
            }
            ClockDevice::BluetoothBaseband => {
                MODEM_SYSCON::regs()
                    .clk_conf1()
                    .modify(|_, w| w.clk_btbb_en().bit(enabled));
            }
            ClockDevice::Count => {}
        }
    }

    fn reset_bluetooth_controller_domains(&mut self) {
        let reset = MODEM_SYSCON::regs().modem_rst_conf();

        reset.modify(|_, w| w.rst_btmac().set_bit());
        reset.modify(|_, w| w.rst_btmac().clear_bit());
        reset.modify(|_, w| w.rst_btmac_apb().set_bit());
        reset.modify(|_, w| w.rst_btmac_apb().clear_bit());
        reset.modify(|_, w| w.rst_ble_timer().set_bit());
        reset.modify(|_, w| w.rst_ble_timer().clear_bit());

        reset.modify(|_, w| w.rst_modem_ecb().set_bit());
        reset.modify(|_, w| w.rst_modem_ccm().set_bit());
        reset.modify(|_, w| w.rst_modem_bah().set_bit());
        reset.modify(|_, w| w.rst_modem_sec().set_bit());
        reset.modify(|_, w| w.rst_modem_ecb().clear_bit());
        reset.modify(|_, w| w.rst_modem_ccm().clear_bit());
        reset.modify(|_, w| w.rst_modem_bah().clear_bit());
        reset.modify(|_, w| w.rst_modem_sec().clear_bit());
    }

    fn controller_resets_released(&self) -> bool {
        let reset = MODEM_SYSCON::regs().modem_rst_conf().read();
        reset.rst_btmac().bit_is_clear()
            && reset.rst_btmac_apb().bit_is_clear()
            && reset.rst_ble_timer().bit_is_clear()
            && reset.rst_modem_ecb().bit_is_clear()
            && reset.rst_modem_ccm().bit_is_clear()
            && reset.rst_modem_bah().bit_is_clear()
            && reset.rst_modem_sec().bit_is_clear()
    }

    #[allow(
        unsafe_code,
        reason = "the official S31 PAC lacks a checked writer for this 12-bit field; the range assertion is the complete safety precondition"
    )]
    fn select_main_xtal_low_power_clock(&mut self, divider: u16) {
        let configuration = MODEM_LPCON::regs().lp_timer_conf();
        configuration.modify(|_, w| w.clk_lp_timer_sel_osc_slow().clear_bit());
        configuration.modify(|_, w| w.clk_lp_timer_sel_osc_fast().clear_bit());
        configuration.modify(|_, w| w.clk_lp_timer_sel_xtal32k().clear_bit());
        configuration.modify(|_, w| w.clk_lp_timer_sel_xtal().clear_bit());
        configuration.modify(|_, w| w.clk_lp_timer_sel_xtal().set_bit());
        assert!(
            divider <= 0x0fff,
            "BLE low-power divider exceeds its 12-bit field"
        );
        configuration.modify(|_, w| {
            // SAFETY: the preceding assertion proves that the semantic
            // divider fits the official 12-bit S31 LP_TIMER_CONF field.
            unsafe { w.clk_lp_timer_div_num().bits(divider) }
        });
        MODEM_LPCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_lp_timer_en().set_bit());
    }

    fn deselect_low_power_clock(&mut self) {
        let configuration = MODEM_LPCON::regs().lp_timer_conf();
        configuration.modify(|_, w| w.clk_lp_timer_sel_osc_slow().clear_bit());
        configuration.modify(|_, w| w.clk_lp_timer_sel_osc_fast().clear_bit());
        configuration.modify(|_, w| w.clk_lp_timer_sel_xtal32k().clear_bit());
        configuration.modify(|_, w| w.clk_lp_timer_sel_xtal().clear_bit());
        MODEM_LPCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_lp_timer_en().clear_bit());
    }

    fn low_power_clock_state(&self) -> LowPowerClockState {
        let configuration = MODEM_LPCON::regs().lp_timer_conf().read();
        LowPowerClockState {
            slow_oscillator_selected: configuration.clk_lp_timer_sel_osc_slow().bit_is_set(),
            fast_oscillator_selected: configuration.clk_lp_timer_sel_osc_fast().bit_is_set(),
            main_xtal_selected: configuration.clk_lp_timer_sel_xtal().bit_is_set(),
            xtal32k_selected: configuration.clk_lp_timer_sel_xtal32k().bit_is_set(),
            divider: configuration.clk_lp_timer_div_num().bits(),
            timer_enabled: MODEM_LPCON::regs()
                .clk_conf()
                .read()
                .clk_lp_timer_en()
                .bit_is_set(),
        }
    }
}
