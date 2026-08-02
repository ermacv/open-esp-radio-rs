// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_mac_interrupt_clr_event
use open_esp_radio_esp32s31_svd as pac;

#[inline(always)]
pub fn generated_hal_mac_interrupt_clr_event(
    wifi_mac_interrupt_registers: &pac::WifiMacInterrupt,
    arg0: u32,
) -> u32 {
    // SAFETY: the Effect Contract requires the complete evidenced register image.
    unsafe {
        wifi_mac_interrupt_registers
            .clear()
            .write_with_zero(|writer| writer.bits(arg0 as u32));
    }
    arg0
}
