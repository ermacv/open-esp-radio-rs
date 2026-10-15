//! Concrete context-free esp-hal handlers of the pinned IRQ installation.
//!
//! The adapter has one address slot; DriverScope owns the registers and all
//! publication storage. Diagnostics borrow that installation's IrqDiagnostics
//! on the task side. No ISR storage or callback registry lives in composition.

use oer_esp32s31_ieee80211_esp_hal::mac_interrupt_epoch::{
    service_mac_interrupt, service_power_interrupt,
};

/// The interrupt-table handler of MODEM_WIFI_MAC.
#[allow(
    unsafe_code,
    reason = "the hard handler must execute from internal SRAM"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub fn mac_interrupt() {
    // The scope has already transferred every semantic work fact to its
    // finite batch and every diagnostic count to its installation's storage.
    service_mac_interrupt();
}

/// The interrupt-table handler of MODEM_WIFI_PWR.
#[allow(
    unsafe_code,
    reason = "the hard handler must execute from internal SRAM"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub fn power_interrupt() {
    service_power_interrupt();
}
