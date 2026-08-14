//! Ownership boundary for the complete cold receive-policy transaction.

use open_esp_radio_esp32s31_hal::wifi_mac::WifiMacColdHal;

pub trait MacColdRxPolicyHardware {
    fn initialize_cold_receive_policy(&mut self);
}

impl MacColdRxPolicyHardware for WifiMacColdHal<'_> {
    fn initialize_cold_receive_policy(&mut self) {
        self.initialize_receive_policy();
    }
}
