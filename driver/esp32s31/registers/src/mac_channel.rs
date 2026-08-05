//! Generated-PAC ownership for the MAC side of a PHY channel switch.

#![forbid(unsafe_code)]

use super::{RadioRegisters, device_fence};

impl RadioRegisters {
    /// Request the complete `WIFI_PS_NONE` MAC stop used before retuning PHY.
    ///
    /// SOURCE: `BLOB_LIBPP_MAC_CHANNEL_SWITCH`, specifically complete
    /// `hal_mac_deinit`, and the exact transcription retained in
    /// `MIGRATION_CHANNEL_SWITCH`. With the all-ones no-power-save retention
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

    /// Restart MAC and select its active REGDMA link after PHY retuning.
    ///
    /// SOURCE: complete `ic_mac_init -> hal_mac_init ->
    /// pwr_hal_select_wifimac_regdma_link` in
    /// `BLOB_LIBPP_MAC_CHANNEL_SWITCH`; `MIGRATION_CHANNEL_SWITCH` preserves
    /// the same finite `WIFI_PS_NONE` transaction. The vendor
    /// `hal_mac_set_csi_cbw` between PHY and this tail is a two-byte `ret` on
    /// ESP32-S31 and therefore has no omitted hardware effect.
    pub fn restart_mac_after_channel_switch_without_power_save(&mut self) {
        let control = self.peripherals.wifi_mac_control.control();
        control.modify(|_, w| {
            w.no_retention_stop_request()
                .clear_bit()
                .tx_block_stop_request()
                .run_all()
        });

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
