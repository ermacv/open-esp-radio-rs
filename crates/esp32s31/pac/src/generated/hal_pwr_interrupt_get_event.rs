// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_pwr_interrupt_get_event
use open_esp_radio_esp32s31_svd as pac;

#[inline(always)]
pub fn generated_hal_pwr_interrupt_get_event(
    wifi_mac_power_interrupt_registers: &pac::WifiMacPowerInterrupt,
) -> u32 {
    let read0 = wifi_mac_power_interrupt_registers.status().read().bits() as u32;
    (read0 & 0xffffffff_u32) | 0x00000000_u32
}
