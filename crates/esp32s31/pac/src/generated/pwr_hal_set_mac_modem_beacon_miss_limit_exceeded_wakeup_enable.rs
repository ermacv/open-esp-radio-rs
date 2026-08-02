// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable
use open_esp_radio_esp32s31_svd as pac;

#[inline(always)]
pub fn generated_pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable(
    wifi_mac_rtc_timer_update_registers: &pac::WifiMacRtcTimerUpdate,
    arg0: u32,
) -> u32 {
    let read0 = wifi_mac_rtc_timer_update_registers
        .modem_sleep_limit_control()
        .read()
        .bits() as u32;
    // SAFETY: the Effect Contract requires the complete evidenced register image.
    unsafe {
        wifi_mac_rtc_timer_update_registers
            .modem_sleep_limit_control()
            .write_with_zero(|writer| {
                writer.bits(((read0 & 0xffffffef_u32) | 0x00000010_u32) as u32)
            });
    }
    arg0
}
