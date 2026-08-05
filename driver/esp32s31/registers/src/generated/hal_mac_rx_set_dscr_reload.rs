// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_mac_rx_set_dscr_reload
use open_esp_radio_esp32s31_pac as pac;

#[inline(always)]
pub fn generated_hal_mac_rx_set_dscr_reload(
    wifi_mac_rx_dma_registers: &pac::WifiMacRxDma,
    arg0: u32,
) -> u32 {
    let read0 = wifi_mac_rx_dma_registers.rx_control().read().bits() as u32;
    // SAFETY: the Effect Contract requires the complete evidenced register image.
    unsafe {
        wifi_mac_rx_dma_registers
            .rx_control()
            .write_with_zero(|writer| {
                writer.bits(((read0 & 0xfffffffe_u32) | 0x00000001_u32) as u32)
            });
    }
    arg0
}
