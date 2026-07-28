//! Generated-PAC ownership for the open promiscuous receive boundary.

use super::{device_fence, RadioRegisters};

impl RadioRegisters {
    /// Configure the complete working open promiscuous receive frontier.
    ///
    /// SOURCE: `OPEN_DRIVER_PROMISCUOUS_RX_FRONTIER`, complete
    /// `BLOB_LIBPP_HAL_SNIFFER_ENABLE`, and the register identity independently
    /// confirmed by `BLOB_LIBPP_HAL_SNIFFER_MISC`.
    pub fn configure_open_mac_promiscuous_receive(&mut self) {
        // SAFETY: zero is the complete HIL-qualified cold register image.
        unsafe {
            self.peripherals
                .wifi_mac_control
                .control()
                .write_with_zero(|w| w.bits(0));
        }

        let filter = &self.peripherals.wifi_mac_rx_filter;
        let control = filter.policy(3);
        // Every line below is a separate fresh-read RMW in the complete
        // hal_sniffer_enable leaf.
        control.modify(|_, w| w.sniffer_enable().set_bit());
        control.modify(|_, w| w.low_unknown().clear_bit());
        control.modify(|_, w| w.ubssid_check_low_unknown().clear_bit());
        control.modify(|_, w| w.policy_bit_2_unknown().clear_bit());
        control.modify(|_, w| w.policy_bit_3_unknown().clear_bit());
        control.modify(|_, w| w.ubssid_check_high_unknown().clear_bit());
        control.modify(|_, w| {
            w.policy_bit_7_unknown()
                .clear_bit()
                .policy_bit_9_unknown()
                .clear_bit()
        });

        // The open frontier enables all eight HIL-qualified miscellaneous
        // packet classes while preserving the other mode-dependent fields.
        filter
            .misc_packet_policy()
            .modify(|_, w| unsafe { w.open_misc_packet_classes().bits(0xff) });
        device_fence();
    }
}
