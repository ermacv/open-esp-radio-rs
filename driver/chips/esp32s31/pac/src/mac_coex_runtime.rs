//! Closed runtime Wi-Fi COEX/PTI register leaves.

#![forbid(unsafe_code)]

use super::{MacItwtClearIndex, MacPti, WifiRadioRegisters};

impl WifiRadioRegisters {
    /// Publish receive-beacon and shared receive PTI in vendor instruction order.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_coex.o]::hal_set_rx_beacon_pti`.
    pub fn set_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti) {
        let runtime = &self.peripherals.coexistence.wifi_mac_coex_runtime;
        super::generated::publish_mac_rx_beacon_pti(
            runtime,
            super::generated::MacRxBeaconPtiMaskedInput::new(beacon.get() << 12),
        );
        super::generated::publish_mac_shared_rx_pti(
            runtime,
            super::generated::MacSharedRxPtiMaskedInput::new(shared.get()),
        );
    }

    /// Set the reviewed receive-beacon clear request without exposing the
    /// remaining unknown request bits.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_coex.o]::hal_clear_rx_beacon_pti`.
    pub fn clear_rx_beacon_pti(&mut self) {
        super::generated::request_mac_rx_beacon_clear(
            &self.peripherals.coexistence.wifi_mac_coex_runtime,
            super::generated::MacRxBeaconClearRequest::Beacon,
        );
    }

    /// Publish the two complete `hal_set_itwt_pti` register edges.
    ///
    /// The first vendor argument is reduced to whether it is zero. The second
    /// contributes only its low PTI nibble. The control edge replaces its low
    /// byte, while the shared-PTI edge preserves bits 31:4 and replaces only
    /// bits 3:0.
    pub fn set_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti) {
        let runtime = &self.peripherals.coexistence.wifi_mac_coex_runtime;
        super::generated::publish_mac_itwt_control(
            runtime,
            super::generated::MacItwtControlMaskedInput::new(u32::from(argument_is_zero)),
        );
        super::generated::publish_mac_shared_rx_pti(
            runtime,
            super::generated::MacSharedRxPtiMaskedInput::new(shared.get()),
        );
    }

    /// Set one machine-bounded request bit like complete `hal_clr_itwt_pti`.
    ///
    /// The individual bit meanings remain unknown. The public index type only
    /// exposes the five-bit shift domain implemented by the instruction.
    pub fn clear_itwt_pti(&mut self, index: MacItwtClearIndex) {
        super::generated::request_mac_itwt_clear(
            &self.peripherals.coexistence.wifi_mac_coex_runtime,
            index,
        );
    }
}
