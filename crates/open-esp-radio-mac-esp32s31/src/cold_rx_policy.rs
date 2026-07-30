//! Ownership boundary for the complete cold receive-policy transaction.

use open_esp_radio_pac_esp32s31::{ColdRadioRegisters, RadioRegisters};

pub trait MacColdRxPolicyHardware {
    fn initialize_cold_receive_policy(&mut self);
}

impl MacColdRxPolicyHardware for ColdRadioRegisters {
    fn initialize_cold_receive_policy(&mut self) {
        RadioRegisters::initialize_cold_receive_policy(&mut **self);
    }
}
