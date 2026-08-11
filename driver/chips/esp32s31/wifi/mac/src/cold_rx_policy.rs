//! Ownership boundary for the complete cold receive-policy transaction.

use open_esp_radio_esp32s31_pac::{ColdRadioRegisters, RadioRegisters};

pub trait MacColdRxPolicyHardware {
    fn initialize_cold_receive_policy(&mut self);
}

impl MacColdRxPolicyHardware for ColdRadioRegisters {
    fn initialize_cold_receive_policy(&mut self) {
        RadioRegisters::initialize_cold_receive_policy(self);
    }
}
