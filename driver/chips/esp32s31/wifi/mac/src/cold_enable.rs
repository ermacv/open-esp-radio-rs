//! Ownership boundary for the complete cold MAC enable edge.

use open_esp_radio_esp32s31_registers::ColdRadioRegisters;

pub trait MacColdEnableHardware {
    fn enable_mac_interrupts(&mut self, event_mask: u32);
}

impl MacColdEnableHardware for ColdRadioRegisters {
    fn enable_mac_interrupts(&mut self, event_mask: u32) {
        self.enable_mac_with_interrupt_mask(event_mask);
    }
}
