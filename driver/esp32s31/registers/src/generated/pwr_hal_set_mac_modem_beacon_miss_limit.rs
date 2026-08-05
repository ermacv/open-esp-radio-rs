// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: pwr_hal_set_mac_modem_beacon_miss_limit
use open_esp_radio_esp32s31_pac as pac;

#[inline(always)]
pub fn generated_pwr_hal_set_mac_modem_beacon_miss_limit(
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
                writer.bits(
                    ((arg0 >> 0) & 1_u32
                        | (((arg0 >> 1) & 1_u32) << 1)
                        | (((arg0 >> 2) & 1_u32) << 2)
                        | (((arg0 >> 3) & 1_u32) << 3)
                        | (((read0 >> 4) & 1_u32) << 4)
                        | (((read0 >> 5) & 1_u32) << 5)
                        | (((read0 >> 6) & 1_u32) << 6)
                        | (((read0 >> 7) & 1_u32) << 7)
                        | (((read0 >> 8) & 1_u32) << 8)
                        | (((read0 >> 9) & 1_u32) << 9)
                        | (((read0 >> 10) & 1_u32) << 10)
                        | (((read0 >> 11) & 1_u32) << 11)
                        | (((read0 >> 12) & 1_u32) << 12)
                        | (((read0 >> 13) & 1_u32) << 13)
                        | (((read0 >> 14) & 1_u32) << 14)
                        | (((read0 >> 15) & 1_u32) << 15)
                        | (((read0 >> 16) & 1_u32) << 16)
                        | (((read0 >> 17) & 1_u32) << 17)
                        | (((read0 >> 18) & 1_u32) << 18)
                        | (((read0 >> 19) & 1_u32) << 19)
                        | (((read0 >> 20) & 1_u32) << 20)
                        | (((read0 >> 21) & 1_u32) << 21)
                        | (((read0 >> 22) & 1_u32) << 22)
                        | (((read0 >> 23) & 1_u32) << 23)
                        | (((read0 >> 24) & 1_u32) << 24)
                        | (((read0 >> 25) & 1_u32) << 25)
                        | (((read0 >> 26) & 1_u32) << 26)
                        | (((read0 >> 27) & 1_u32) << 27)
                        | (((read0 >> 28) & 1_u32) << 28)
                        | (((read0 >> 29) & 1_u32) << 29)
                        | (((read0 >> 30) & 1_u32) << 30)
                        | (((read0 >> 31) & 1_u32) << 31)) as u32,
                )
            });
    }
    (arg0 >> 0) & 1_u32
        | (((arg0 >> 1) & 1_u32) << 1)
        | (((arg0 >> 2) & 1_u32) << 2)
        | (((arg0 >> 3) & 1_u32) << 3)
}
