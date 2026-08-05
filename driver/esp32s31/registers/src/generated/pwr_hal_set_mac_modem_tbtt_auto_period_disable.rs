// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: pwr_hal_set_mac_modem_tbtt_auto_period_disable
use open_esp_radio_esp32s31_pac as pac;

#[inline(always)]
pub fn generated_pwr_hal_set_mac_modem_tbtt_auto_period_disable(
    wifi_mac_regdma_control_registers: &pac::WifiMacRegdmaControl,
    arg0: u32,
) -> u32 {
    let read0 = wifi_mac_regdma_control_registers.control().read().bits() as u32;
    // SAFETY: the Effect Contract requires the complete evidenced register image.
    unsafe {
        wifi_mac_regdma_control_registers
            .control()
            .write_with_zero(|writer| {
                writer.bits(((read0 & 0x7fffffff_u32) | 0x00000000_u32) as u32)
            });
    }
    arg0
}
