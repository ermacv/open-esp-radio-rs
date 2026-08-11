//! Generated-PAC ownership for the complete MAC enable transaction.

#![forbid(unsafe_code)]

use super::{ColdRadioRegisters, MacInterruptMask};

impl ColdRadioRegisters {
    /// Enable the shared MAC gates and publish the enabled-event bitmap.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_mac.o]::hal_enable_mac`, size `0x18`.
    /// It clears all four disable gates in one fresh-read RMW, then stores its
    /// complete argument into the interrupt-enable register.
    pub fn enable_mac_with_interrupt_mask(&mut self, event_mask: MacInterruptMask) {
        self.registers
            .peripherals
            .wifi_mac_core_enable
            .control()
            .modify(|_, w| w.mac_disable_gates_unknown().enabled());
        super::generated::mac_interrupt_enable(&self.interrupts.wifi_mac_interrupt, event_mask);
    }
}
