//! Ownership boundary for the open promiscuous receive frontier.

use open_esp_radio_esp32s31_hal::{
    RadioRuntimeOwner,
    wifi_mac::{WifiMacColdHal, WifiMacHal},
};

pub trait MacSnifferHardware {
    fn configure_open_promiscuous_receive(&mut self);
}

impl MacSnifferHardware for WifiMacColdHal<'_> {
    fn configure_open_promiscuous_receive(&mut self) {
        WifiMacColdHal::configure_open_promiscuous_receive(self);
    }
}

impl MacSnifferHardware for WifiMacHal<'_> {
    fn configure_open_promiscuous_receive(&mut self) {
        WifiMacHal::configure_open_promiscuous_receive(self);
    }
}

impl MacSnifferHardware for RadioRuntimeOwner {
    fn configure_open_promiscuous_receive(&mut self) {
        self.wifi_mac_hal().configure_open_promiscuous_receive();
    }
}
