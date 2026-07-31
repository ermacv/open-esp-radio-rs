//! Generated-PAC ownership for the STA TSF and modem wakeup control.

use super::{RadioRegisters, device_fence};

impl RadioRegisters {
    /// Publish a station TSF value and enable the station TSF scheduler.
    ///
    /// SOURCE: complete `_oracles/libpp.a[hal_tsf.o]`: `hal_set_sta_tsf`
    /// writes the low word, writes the high word and then asserts bit four at
    /// `0x2010_d814` through a fresh-read RMW. Complete
    /// `hal_enable_sta_tsf` performs two further fresh-read RMWs at
    /// `0x2010_d858`: first it sets bits 27 and 31, then it replaces bits
    /// 22:19 with one.
    pub fn start_station_tsf(&mut self, value: u64) {
        let load = &self.peripherals.wifi_mac_sta_tsf_load;
        // SAFETY: each `u32` exactly fills the generated 32-bit VALUE field.
        unsafe {
            load.value_low()
                .write_with_zero(|w| w.value().bits(value as u32));
            load.value_high()
                .write_with_zero(|w| w.value().bits((value >> 32) as u32));
        }
        load.control().modify(|_, w| w.load_station_tsf().set_bit());

        let control = self.peripherals.wifi_mac_rtc_timer_update.sta_tsf_control();
        control.modify(|_, w| {
            w.sta_tsf_enable_low()
                .set_bit()
                .sta_tsf_enable_high()
                .set_bit()
        });
        // SAFETY: one is the instruction-exact mode selected by
        // `hal_enable_sta_tsf` and fits the generated four-bit field.
        control.modify(|_, w| unsafe { w.sta_tsf_mode().bits(1) });
        device_fence();
    }

    /// Disable modem-state wakeup protection for an always-awake STA.
    ///
    /// SOURCE: complete
    /// `_oracles/libpp.a[hal_pwr.o]::
    /// pwr_hal_set_mac_modem_state_wakeup_protect_disable`.
    ///
    /// That leaf performs one fresh-read RMW which clears bit 24 at
    /// `0x2010_d858`. The vendor pre-auth path has this bit clear after its
    /// power-management lifecycle. A standalone HIL A/B showed that copying
    /// this final bit image without that lifecycle does not recover q0 ACKs,
    /// so callers must not use it as a generic TX-enable operation.
    pub fn disable_mac_modem_state_wakeup_protect(&mut self) {
        self.peripherals
            .wifi_mac_rtc_timer_update
            .sta_tsf_control()
            .modify(|_, w| w.modem_state_wakeup_protect_enable().clear_bit());
        device_fence();
    }

    /// Return the complete shared STA TSF control image for HIL comparison.
    pub fn sta_tsf_control_image(&self) -> u32 {
        self.peripherals
            .wifi_mac_rtc_timer_update
            .sta_tsf_control()
            .read()
            .bits()
    }
}
