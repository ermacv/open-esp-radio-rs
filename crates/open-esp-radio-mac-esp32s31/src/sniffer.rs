//! Ownership boundary for promiscuous-sniffer enable.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub trait MacSnifferHardware {
    fn enable_promiscuous_sniffer(&mut self);
}

impl MacSnifferHardware for RadioRegisters {
    fn enable_promiscuous_sniffer(&mut self) {
        self.enable_mac_promiscuous_sniffer();
    }
}
