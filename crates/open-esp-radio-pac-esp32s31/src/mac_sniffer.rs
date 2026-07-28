//! Generated-PAC ownership for promiscuous-sniffer enable.

use super::RadioRegisters;

impl RadioRegisters {
    /// Enable the MAC promiscuous sniffer using the complete vendor leaf.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_sniffer.o]::hal_sniffer_enable`.
    pub fn enable_mac_promiscuous_sniffer(&mut self) {
        let control = self.peripherals.wifi_mac_rx_filter.policy(3);

        // Every line below is a separate fresh-read RMW in the complete leaf.
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
    }
}
