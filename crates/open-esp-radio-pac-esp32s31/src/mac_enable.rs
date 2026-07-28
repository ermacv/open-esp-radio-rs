//! Generated-PAC ownership for the complete MAC enable transaction.

use super::RadioRegisters;

impl RadioRegisters {
    /// Enable the shared MAC gates and publish the enabled-event bitmap.
    ///
    /// SOURCE: complete pinned
    /// `_oracles/libpp.a[hal_mac.o]::hal_enable_mac`, size `0x18`.
    /// It clears all four disable gates in one fresh-read RMW, then stores its
    /// complete argument into the interrupt-enable register.
    pub fn enable_mac_with_interrupt_mask(&mut self, event_mask: u32) {
        self.peripherals
            .wifi_mac_core_enable
            .control()
            .modify(|_, w| unsafe { w.mac_disable_gates_unknown().bits(0) });
        // SAFETY: the complete leaf publishes all 32 argument bits as the
        // enabled-event image; this is not a write-to-clear register.
        unsafe {
            self.peripherals
                .wifi_mac_interrupt
                .enable()
                .write_with_zero(|w| w.event_mask().bits(event_mask))
        };
    }
}
