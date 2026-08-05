//! Ownership boundary for the complete cold MAC antenna transaction.

use open_esp_radio_esp32s31_registers::{ColdRadioRegisters, RadioRegisters};

pub trait MacColdAntennaHardware {
    fn initialize_mac_antenna(&mut self);
}

impl MacColdAntennaHardware for ColdRadioRegisters {
    fn initialize_mac_antenna(&mut self) {
        RadioRegisters::initialize_mac_antenna(self);
    }
}
