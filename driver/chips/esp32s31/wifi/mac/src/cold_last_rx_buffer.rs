//! Ownership boundary for the complete last-RX-buffer table init.

use open_esp_radio_esp32s31_pac::ColdRadioRegisters;

pub trait MacColdLastRxBufferHardware {
    fn initialize_last_rx_buffer_table(&mut self);
}

impl MacColdLastRxBufferHardware for ColdRadioRegisters {
    fn initialize_last_rx_buffer_table(&mut self) {
        self.initialize_mac_last_rx_buffer();
    }
}
