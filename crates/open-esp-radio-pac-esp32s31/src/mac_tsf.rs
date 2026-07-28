//! Generated-PAC ownership for the STA TSF and modem wakeup control.

use super::{device_fence, RadioRegisters};

impl RadioRegisters {
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
