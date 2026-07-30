//! Ownership boundary for the cold hardware-crypto bypass state.

use open_esp_radio_pac_esp32s31::ColdRadioRegisters;

pub trait MacColdCryptoHardware {
    fn initialize_crypto_bypass(&mut self);
}

impl MacColdCryptoHardware for ColdRadioRegisters {
    fn initialize_crypto_bypass(&mut self) {
        self.initialize_mac_crypto_bypass();
    }
}
