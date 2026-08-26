//! Official-PAC platform backend for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! This module covers only system clock and reset operations. The HAL keeps
//! ownership of the IEEE 802.15.4 MAC register lease and owns the transition
//! order. Completion here is not evidence of PHY or RF readiness.

use esp_hal::peripherals::{HP_SYS_CLKRST, MODEM_SYSCON};
use open_esp_radio_esp32s31_hal::{
    PowerClockControl,
    ieee802154_lifecycle::{
        Ieee802154PlatformClockImages, Ieee802154PlatformControl, Ieee802154ResetImages,
    },
};

use crate::EspHalRadioPeripheral;

// Pinned ESP-IDF `modem_clock_domain_icg_config`: these are bitmaps, not
// enumerated states. Existing bits must be retained when a client is added.
const ICG_CODE_4: u8 = 4;
const ICG_CODE_6: u8 = 6;

const fn contains_icg_code(observed: u8, required: u8) -> bool {
    observed & required == required
}

impl Ieee802154PlatformControl for EspHalRadioPeripheral {
    fn configure_modem_clock_maps(&mut self) {
        // `modem_clock_module_icg_map_init_all` walks domains in this order and
        // ORs each reviewed bitmap into the existing field. The IEEE 802.15.4
        // domain setter deliberately updates BT and ZB because those domains
        // share baseband resources on S31.
        MODEM_SYSCON::regs().clk_conf_power_st().modify(|r, w| {
            w.clk_modem_apb_st_map()
                .set(r.clk_modem_apb_st_map().bits() | ICG_CODE_6)
        });
        MODEM_SYSCON::regs().clk_conf_power_st().modify(|r, w| {
            w.clk_modem_peri_st_map()
                .set(r.clk_modem_peri_st_map().bits() | ICG_CODE_4)
        });
        MODEM_SYSCON::regs().clk_conf_power_st().modify(|r, w| {
            w.clk_wifi_st_map()
                .set(r.clk_wifi_st_map().bits() | ICG_CODE_6)
        });
        MODEM_SYSCON::regs()
            .clk_conf_power_st()
            .modify(|r, w| w.clk_bt_st_map().set(r.clk_bt_st_map().bits() | ICG_CODE_4));
        MODEM_SYSCON::regs()
            .clk_conf_power_st()
            .modify(|r, w| w.clk_fe_st_map().set(r.clk_fe_st_map().bits() | ICG_CODE_6));
        MODEM_SYSCON::regs()
            .clk_conf_power_st()
            .modify(|r, w| w.clk_bt_st_map().set(r.clk_bt_st_map().bits() | ICG_CODE_4));
        MODEM_SYSCON::regs()
            .clk_conf_power_st()
            .modify(|r, w| w.clk_zb_st_map().set(r.clk_zb_st_map().bits() | ICG_CODE_4));
    }

    fn configure_modem_source_clock(&mut self) {
        // ESP-HAL's global clock tree keeps the common upstream PLL_F160M gate
        // enabled. Publish the exact 0x3d modem-source image required by the
        // pinned vendor dependency without inventing a second clock owner.
        // `ieee802154_clock_images` proves the upstream gate separately.
        PowerClockControl::configure_modem_source_clocks(self);
    }

    fn enable_wifi_bb_80x1_clock(&mut self) {
        MODEM_SYSCON::regs()
            .clk_conf1()
            .modify(|_, w| w.clk_wifibb_80x1_en().set_bit());
    }

    fn enable_etm_clock(&mut self) {
        MODEM_SYSCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_etm_en().set_bit());
    }

    fn enable_bt_apb_clocks(&mut self) {
        // `modem_clock_bt_apb_configure` performs two fresh writes in this
        // order. Keep them separate even though both fields are one logical
        // dependency.
        MODEM_SYSCON::regs()
            .clk_conf1()
            .modify(|_, w| w.clk_bt_apb_en().set_bit());
        MODEM_SYSCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_modem_sec_apb_en().set_bit());
    }

    fn enable_bt_ieee802154_common_baseband_clock(&mut self) {
        MODEM_SYSCON::regs()
            .clk_conf1()
            .modify(|_, w| w.clk_btbb_en().set_bit());
    }

    fn enable_ieee802154_mac_clocks(&mut self) {
        // The official LL opens the APB clock before the functional MAC clock.
        MODEM_SYSCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_zb_apb_en().set_bit());
        MODEM_SYSCON::regs()
            .clk_conf()
            .modify(|_, w| w.clk_zbmac_en().set_bit());
    }

    fn ieee802154_platform_clock_images(&self) -> Ieee802154PlatformClockImages {
        // Sample each additional register once so related fields are decoded
        // from one coherent hardware image.
        let pll_160m = HP_SYS_CLKRST::regs().ref_160m_ctrl0().read();
        let modem_source = HP_SYS_CLKRST::regs().modem_conf().read();
        let hp_active_map = MODEM_SYSCON::regs().clk_conf_power_st().read();
        let modem_clock = MODEM_SYSCON::regs().clk_conf().read();
        let modem_clock1 = MODEM_SYSCON::regs().clk_conf1().read();

        Ieee802154PlatformClockImages {
            hp_active_clock_maps_configured: contains_icg_code(
                hp_active_map.clk_zb_st_map().bits(),
                ICG_CODE_4,
            ) && contains_icg_code(
                hp_active_map.clk_fe_st_map().bits(),
                ICG_CODE_6,
            ) && contains_icg_code(
                hp_active_map.clk_bt_st_map().bits(),
                ICG_CODE_4,
            ) && contains_icg_code(
                hp_active_map.clk_wifi_st_map().bits(),
                ICG_CODE_6,
            ) && contains_icg_code(
                hp_active_map.clk_modem_peri_st_map().bits(),
                ICG_CODE_4,
            ) && contains_icg_code(
                hp_active_map.clk_modem_apb_st_map().bits(),
                ICG_CODE_6,
            ),
            pll_160m_clock_enabled: pll_160m.ref_160m_clk_en().bit_is_set(),
            // This matches the pinned vendor check, which compares the whole
            // S31 MODEM_CONF image rather than a subset of named fields.
            modem_source_clock_configured: modem_source.bits() == 0x3d,
            wifi_bb_80x1_clock_enabled: modem_clock1.clk_wifibb_80x1_en().bit_is_set(),
            etm_clock_enabled: modem_clock.clk_etm_en().bit_is_set(),
            bt_apb_clock_enabled: modem_clock1.clk_bt_apb_en().bit_is_set(),
            modem_security_apb_clock_enabled: modem_clock.clk_modem_sec_apb_en().bit_is_set(),
            bt_ieee802154_common_baseband_clock_enabled: modem_clock1.clk_btbb_en().bit_is_set(),
            ieee802154_apb_clock_enabled: modem_clock.clk_zb_apb_en().bit_is_set(),
            ieee802154_mac_clock_enabled: modem_clock.clk_zbmac_en().bit_is_set(),
        }
    }

    fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        MODEM_SYSCON::regs()
            .modem_rst_conf()
            .modify(|_, w| w.rst_zbmac().bit(asserted));
    }

    fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        MODEM_SYSCON::regs()
            .modem_rst_conf()
            .modify(|_, w| w.rst_zbmac_apb().bit(asserted));
    }

    fn ieee802154_reset_images(&self) -> Ieee802154ResetImages {
        let reset = MODEM_SYSCON::regs().modem_rst_conf().read();
        Ieee802154ResetImages {
            mac_reset_released: reset.rst_zbmac().bit_is_clear(),
            apb_reset_released: reset.rst_zbmac_apb().bit_is_clear(),
        }
    }
}

// Compile-time proof that the unique platform token exposes the complete
// clock/reset contract; it needs no hardware or test harness.
const _: fn() = {
    fn assert_backend<Backend: Ieee802154PlatformControl>() {}
    assert_backend::<EspHalRadioPeripheral>
};
