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
use open_esp_radio_esp32s31_phy::PhyTxTargetPowerProfile;
use open_esp_radio_esp32s31_wifi::mac_start::Esp32s31WifiMacPlatform;
use open_esp_radio_esp32s31_wifi_mac::init::{
    MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacDelayEntropy, MacSlowClockCalibration,
    MacSlowClockCalibrationSource, MacTxPowerPair, MacTxPowerSource,
};

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
