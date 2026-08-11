//! Ownership boundary for the complete cold MAC enable edge.

use open_esp_radio_esp32s31_pac::{ColdRadioRegisters, MacInterruptMask};

pub trait MacColdEnableHardware {
    fn enable_mac_interrupts(&mut self, event_mask: MacInterruptMask);
}

impl MacColdEnableHardware for ColdRadioRegisters {
    fn enable_mac_interrupts(&mut self, event_mask: MacInterruptMask) {
        self.enable_mac_with_interrupt_mask(event_mask);
    }
}
