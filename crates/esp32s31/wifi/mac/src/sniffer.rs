//! Ownership boundary for the open promiscuous receive frontier.

use open_esp_radio_esp32s31_pac::ColdRadioRegisters;

pub trait MacSnifferHardware {
    fn configure_open_promiscuous_receive(&mut self);
}

impl MacSnifferHardware for ColdRadioRegisters {
    fn configure_open_promiscuous_receive(&mut self) {
        self.configure_open_mac_promiscuous_receive();
    }
}
