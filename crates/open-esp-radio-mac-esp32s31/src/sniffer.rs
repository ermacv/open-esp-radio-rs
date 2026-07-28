//! Ownership boundary for the open promiscuous receive frontier.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub trait MacSnifferHardware {
    fn configure_open_promiscuous_receive(&mut self);
}

impl MacSnifferHardware for RadioRegisters {
    fn configure_open_promiscuous_receive(&mut self) {
        self.configure_open_mac_promiscuous_receive();
    }
}
