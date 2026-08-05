// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_mac_rx_read_rxdscrnext
use open_esp_radio_esp32s31_pac as pac;

#[inline(always)]
pub fn generated_hal_mac_rx_read_rxdscrnext(wifi_mac_rx_dma_registers: &pac::WifiMacRxDma) -> u32 {
    let read0 = wifi_mac_rx_dma_registers.rx_next_descriptor().read().bits() as u32;
    (read0 & 0xffffffff_u32) | 0x00000000_u32
}
