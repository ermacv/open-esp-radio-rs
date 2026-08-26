//! ESP32-S31 register operations behind the role-neutral platform owner.
//!
//! The Bluetooth lease follows the pinned ESP-IDF controller lifecycle in
//! `components/bt/controller/esp32s31/bt.c`, `btdm_lp.c`, and the S31 modem
//! clock implementation. These paths define semantics; the PAC stays private.

use esp_hal::peripherals::{
    HP_SYS_CLKRST, I2C_ANA_MST, LP_AON_CLK_RST, LP_PERI, LP_TSENS, MODEM_LPCON, MODEM_SYSCON, PMU,
};
use open_esp_radio_esp32s31_bluetooth::{BluetoothClockControl, BluetoothPlatformClockState};
use open_esp_radio_esp32s31_hal::analog_i2c::PhyPmuControl;

use crate::coordinator::{
    BluetoothPlatformBusy, BluetoothPlatformLease, ClockCoordinator, ClockDevice, ClockIo,
};

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
    /// The remaining upstream PLL source is reference-counted here. Shared
    /// MODEM clock dependencies are retained by the affine custom-PAC route.
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
    fn enable_bluetooth_controller_pll_source(&mut self) {
        self.inner.enable_bluetooth_controller_pll_source();
    }

    fn bluetooth_platform_clock_state(&mut self) -> BluetoothPlatformClockState {
        self.inner.bluetooth_platform_clock_state()
    }

    fn disable_bluetooth_controller_pll_source(&mut self) {
        self.inner.disable_bluetooth_controller_pll_source();
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
    fn clock_is_enabled(&self, device: ClockDevice) -> bool {
        match device {
            ClockDevice::Pll160mSource => Self::pll_160m_source_is_enabled(),
        }
    }

    fn set_clock_enabled(&mut self, device: ClockDevice, enabled: bool) {
        match device {
            ClockDevice::Pll160mSource => Self::set_pll_160m_source(enabled),
        }
    }
}
