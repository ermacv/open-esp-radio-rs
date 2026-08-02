// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_mac_rx_set_base
use open_esp_radio_esp32s31_svd as pac;

#[inline(always)]
pub fn generated_hal_mac_rx_set_base(
    wifi_mac_rx_dma_registers: &pac::WifiMacRxDma,
    arg0: u32,
) -> u32 {
    // SAFETY: the Effect Contract requires the complete evidenced register image.
    unsafe {
        wifi_mac_rx_dma_registers
            .rx_descriptor_base()
            .write_with_zero(|writer| writer.bits((arg0) as u32));
    }
    arg0
}
