//! Remaining platform-owned prerequisites for ESP32-S31 IEEE 802.15.4.
//!
//! MODEM_SYSCON belongs to the custom affine radio route. This adapter retains
//! only the upstream HP_SYS_CLKRST source selection and observation.

use esp_hal::peripherals::HP_SYS_CLKRST;
use open_esp_radio_esp32s31_hal::{
    PowerClockControl,
    ieee802154_lifecycle::{Ieee802154PlatformClockImages, Ieee802154PlatformControl},
};

use crate::EspHalRadioPeripheral;

impl Ieee802154PlatformControl for EspHalRadioPeripheral {
    fn configure_modem_source_clock(&mut self) {
        PowerClockControl::configure_modem_source_clocks(self);
    }

    fn ieee802154_platform_clock_images(&self) -> Ieee802154PlatformClockImages {
        let pll_160m = HP_SYS_CLKRST::regs().ref_160m_ctrl0().read();
        let power = PowerClockControl::platform_power_clock_images(self);
        Ieee802154PlatformClockImages {
            pll_160m_clock_enabled: pll_160m.ref_160m_clk_en().bit_is_set(),
            modem_source_clock_configured: power.modem_source_clocks_configured,
        }
    }
}

const _: fn() = {
    fn assert_backend<Backend: Ieee802154PlatformControl>() {}
    assert_backend::<EspHalRadioPeripheral>
};
