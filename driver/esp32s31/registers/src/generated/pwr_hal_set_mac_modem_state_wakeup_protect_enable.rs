// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: pwr_hal_set_mac_modem_state_wakeup_protect_enable
use open_esp_radio_esp32s31_pac as pac;

#[inline(always)]
pub fn generated_pwr_hal_set_mac_modem_state_wakeup_protect_enable(
    wifi_mac_rtc_timer_update_registers: &pac::WifiMacRtcTimerUpdate,
    arg0: u32,
) -> u32 {
    let read0 = wifi_mac_rtc_timer_update_registers
        .sta_tsf_control()
        .read()
        .bits() as u32;
    // SAFETY: the Effect Contract requires the complete evidenced register image.
    unsafe {
        wifi_mac_rtc_timer_update_registers
            .sta_tsf_control()
            .write_with_zero(|writer| {
                writer.bits(((read0 & 0xfeffffff_u32) | 0x01000000_u32) as u32)
            });
    }
    arg0
}
