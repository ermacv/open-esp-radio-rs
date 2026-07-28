//! Ownership boundary for bounded complete `hal_he_init` transactions.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub trait MacColdHeHardware {
    fn initialize_he_prefix(&mut self);
}

impl MacColdHeHardware for RadioRegisters {
    fn initialize_he_prefix(&mut self) {
        self.initialize_mac_he_prefix();
    }
}
