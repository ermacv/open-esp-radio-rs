//! Register-local PAC operations for the cold MAC handshake.

#![forbid(unsafe_code)]

use super::{MacInterruptMask, WifiColdRegisters};

impl WifiColdRegisters {
    /// Set the cold-start request bit.
    pub fn request_mac_cold_start(&mut self) {
        self.registers
            .peripherals
            .wifi_mac
            .wifi_mac_cold_handshake
            .control()
            .modify(|_, w| w.request().set_bit());
    }

    /// Sample the cold-start handshake register once.
    pub fn sample_mac_cold_start(&self) -> u32 {
        self.registers
            .peripherals
            .wifi_mac
            .wifi_mac_cold_handshake
            .control()
            .read()
            .bits()
    }

    /// Mask every MAC interrupt source.
    pub fn mask_all_mac_interrupts(&mut self) {
        super::generated::mac_interrupt_enable(
            &self.interrupts.wifi_mac_interrupt,
            MacInterruptMask::NONE,
        );
    }

    /// Acknowledge every pending MAC interrupt source.
    pub fn clear_all_mac_interrupts(&mut self) {
        super::generated::mac_interrupt_clear(
            &self.interrupts.wifi_mac_interrupt,
            super::generated::MacInterruptClearImage::new(u32::MAX),
        );
    }
}
