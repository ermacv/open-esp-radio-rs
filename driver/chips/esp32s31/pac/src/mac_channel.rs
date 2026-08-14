//! Generated-PAC ownership for the MAC side of a PHY channel switch.

#![forbid(unsafe_code)]

use super::{RadioRegisters, device_fence};

impl RadioRegisters {
    /// Request the complete `WIFI_PS_NONE` MAC stop used before retuning PHY.
    ///
    /// SOURCE: `BLOB_LIBPP_MAC_CHANNEL_SWITCH`, specifically complete
    /// `hal_mac_deinit`, and the exact transcription retained in
    /// `PROMOTED_CHANNEL_SWITCH`. With the all-ones no-power-save retention
    /// mask the vendor leaf sets bit 12 and bits 23:16 in one RMW.
    pub fn request_mac_channel_stop_without_power_save(&mut self) {
        let control = self.peripherals.wifi_mac_control.control();
        control.modify(|_, w| {
            w.no_retention_stop_request()
                .set_bit()
                .tx_block_stop_request()
                .stop_all()
        });
        device_fence();
    }

    /// Sample the three activity bits polled by the vendor channel switch.
    pub fn mac_channel_active_state(&self) -> u8 {
        self.peripherals
            .wifi_mac_control
            .control()
            .read()
            .active_state()
            .bits()
    }

    /// Clear the no-power-save MAC stop request after PHY retuning.
    pub fn resume_mac_channel_without_power_save(&mut self) {
        let control = self.peripherals.wifi_mac_control.control();
        control.modify(|_, w| {
            w.no_retention_stop_request()
                .clear_bit()
                .tx_block_stop_request()
                .run_all()
        });
        device_fence();
    }

    /// Select the Wi-Fi no-power-save REGDMA link.
    pub fn select_wifi_no_power_save_regdma_link(&mut self) {
        self.peripherals
            .wifi_mac_regdma_control
            .control()
            .modify(|_, w| w.active_link().wifi_no_power_save());
        device_fence();
    }

    /// Read the active REGDMA link for diagnostics and HIL assertions.
    pub fn wifi_mac_regdma_link(&self) -> u8 {
        self.peripherals
            .wifi_mac_regdma_control
            .control()
            .read()
            .active_link()
            .bits()
    }
}
