//! Ownership boundary for the complete cold MAC antenna transaction.

use open_esp_radio_esp32s31_hal::wifi_mac::WifiMacColdHal;

pub trait MacColdAntennaHardware {
    fn initialize_mac_antenna(&mut self);
}

impl MacColdAntennaHardware for WifiMacColdHal<'_> {
    fn initialize_mac_antenna(&mut self) {
        self.initialize_antenna();
    }
}
