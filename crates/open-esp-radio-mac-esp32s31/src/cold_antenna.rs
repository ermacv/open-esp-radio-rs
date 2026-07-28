//! Ownership boundary for the complete cold MAC antenna transaction.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub trait MacColdAntennaHardware {
    fn initialize_mac_antenna(&mut self);
}

impl MacColdAntennaHardware for RadioRegisters {
    fn initialize_mac_antenna(&mut self) {
        RadioRegisters::initialize_mac_antenna(self);
    }
}
