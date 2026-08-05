//! Semantic MMIO contract for the ESP32-S31 RX descriptor walker.
//!
//! The contract exposes finite RX-DMA operations instead of register
//! identities. Descriptor-ring ownership is modeled separately by
//! [`crate::rx`]; production and host models can therefore share the same
//! state machine without exposing the generated PAC above the register leaf.

use open_esp_radio_esp32s31_registers::{ColdRadioRegisters, RadioRegisters};

/// Semantic ownership boundary for the S31 RX descriptor walker.
///
/// Production uses the generated PAC implementation below. Host tests model
/// these finite operations without receiving arbitrary register identities.
pub trait RxDma {
    /// Optional monotonic hardware starvation counter for boundary telemetry.
    ///
    /// This observation never participates in descriptor ownership. Host
    /// models and platforms without such a counter keep the default `None`.
    fn buffer_full_count(&mut self) -> Option<u16> {
        None
    }

    fn last_descriptor_low(&mut self) -> u32;
    fn next_descriptor_low(&mut self) -> u32;
    fn walker_enabled(&mut self) -> bool;
    fn reload_pending(&mut self) -> bool;
    fn set_descriptor_high_window(&mut self, address_high: u16);
    fn write_descriptor_base(&mut self, address: u32);
    fn publish_walker_enable(&mut self);
    fn request_reload(&mut self);
    fn try_enable_walker(&mut self) -> bool;
    fn try_disable_walker(&mut self) -> bool;
    fn fence(&mut self);
}

impl RxDma for RadioRegisters {
    fn buffer_full_count(&mut self) -> Option<u16> {
        Some(self.mac_rx_buffer_full_count())
    }

    fn last_descriptor_low(&mut self) -> u32 {
        self.mac_rx_last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.mac_rx_next_descriptor_low()
    }

    fn walker_enabled(&mut self) -> bool {
        self.mac_rx_walker_enabled()
    }

    fn reload_pending(&mut self) -> bool {
        self.mac_rx_reload_pending()
    }

    fn set_descriptor_high_window(&mut self, address_high: u16) {
        self.set_mac_rx_descriptor_high_window(address_high);
    }

    fn write_descriptor_base(&mut self, address: u32) {
        self.write_mac_rx_descriptor_base(address);
    }

    fn publish_walker_enable(&mut self) {
        self.publish_mac_rx_walker_enable();
    }

    fn request_reload(&mut self) {
        self.request_mac_rx_descriptor_reload();
    }

    fn try_enable_walker(&mut self) -> bool {
        self.try_enable_mac_rx_walker()
    }

    fn try_disable_walker(&mut self) -> bool {
        self.try_disable_mac_rx_walker()
    }

    fn fence(&mut self) {
        self.order_device_accesses();
    }
}

impl RxDma for ColdRadioRegisters {
    fn buffer_full_count(&mut self) -> Option<u16> {
        RxDma::buffer_full_count(&mut **self)
    }

    fn last_descriptor_low(&mut self) -> u32 {
        RxDma::last_descriptor_low(&mut **self)
    }

    fn next_descriptor_low(&mut self) -> u32 {
        RxDma::next_descriptor_low(&mut **self)
    }

    fn walker_enabled(&mut self) -> bool {
        RxDma::walker_enabled(&mut **self)
    }

    fn reload_pending(&mut self) -> bool {
        RxDma::reload_pending(&mut **self)
    }

    fn set_descriptor_high_window(&mut self, address_high: u16) {
        RxDma::set_descriptor_high_window(&mut **self, address_high);
    }

    fn write_descriptor_base(&mut self, address: u32) {
        RxDma::write_descriptor_base(&mut **self, address);
    }

    fn publish_walker_enable(&mut self) {
        RxDma::publish_walker_enable(&mut **self);
    }

    fn request_reload(&mut self) {
        RxDma::request_reload(&mut **self);
    }

    fn try_enable_walker(&mut self) -> bool {
        RxDma::try_enable_walker(&mut **self)
    }

    fn try_disable_walker(&mut self) -> bool {
        RxDma::try_disable_walker(&mut **self)
    }

    fn fence(&mut self) {
        RxDma::fence(&mut **self);
    }
}
