//! Ownership boundary for RX buffer geometry before ring publication.

use open_esp_radio_esp32s31_registers::ColdRadioRegisters;

pub trait MacColdRxBufferHardware {
    fn initialize_rx_buffer_prefix(&mut self);
}

impl MacColdRxBufferHardware for ColdRadioRegisters {
    fn initialize_rx_buffer_prefix(&mut self) {
        self.initialize_mac_rx_buffer_prefix();
    }
}
