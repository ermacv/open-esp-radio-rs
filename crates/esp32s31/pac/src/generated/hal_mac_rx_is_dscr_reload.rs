// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_mac_rx_is_dscr_reload
use open_esp_radio_esp32s31_svd as pac;

#[inline(always)]
pub fn generated_hal_mac_rx_is_dscr_reload(wifi_mac_rx_dma_registers: &pac::WifiMacRxDma) -> u32 {
    let read0 = wifi_mac_rx_dma_registers.rx_control().read().bits() as u32;
    (read0 & 0x00000001_u32) | 0x00000000_u32
}
