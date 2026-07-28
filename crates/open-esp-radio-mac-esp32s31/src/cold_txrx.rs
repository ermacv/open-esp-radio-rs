//! Ownership boundary for the direct cold `mac_txrx_init` prefix.

use open_esp_radio_pac_esp32s31::RadioRegisters;

pub trait MacColdTxRxHardware {
    fn initialize_txrx_prefix(&mut self);
    fn initialize_txrx_suffix(&mut self);
}

impl MacColdTxRxHardware for RadioRegisters {
    fn initialize_txrx_prefix(&mut self) {
        self.initialize_mac_txrx_prefix();
    }

    fn initialize_txrx_suffix(&mut self) {
        self.initialize_mac_txrx_suffix();
    }
}
