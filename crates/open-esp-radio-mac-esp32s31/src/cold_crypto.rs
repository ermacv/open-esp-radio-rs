//! Ownership boundary for the cold hardware-crypto bypass state.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub trait MacColdCryptoHardware {
    fn initialize_crypto_bypass(&mut self);
}

impl MacColdCryptoHardware for RadioRegisters {
    fn initialize_crypto_bypass(&mut self) {
        self.initialize_mac_crypto_bypass();
    }
}
