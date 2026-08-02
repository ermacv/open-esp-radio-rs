// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: pwr_hal_set_mac_modem_tbtt_auto_period_interval
use open_esp_radio_esp32s31_svd as pac;

#[inline(always)]
pub fn generated_pwr_hal_set_mac_modem_tbtt_auto_period_interval(
    wifi_mac_regdma_control_registers: &pac::WifiMacRegdmaControl,
    arg0: u32,
) -> u32 {
    let read0 = wifi_mac_regdma_control_registers.control().read().bits() as u32;
    // SAFETY: the Effect Contract requires the complete evidenced register image.
    unsafe {
        wifi_mac_regdma_control_registers
            .control()
            .write_with_zero(|writer| {
                writer.bits(
                    ((read0 >> 0) & 1_u32
                        | (((read0 >> 1) & 1_u32) << 1)
                        | (((read0 >> 2) & 1_u32) << 2)
                        | (((read0 >> 3) & 1_u32) << 3)
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
                        | (((arg0 >> 0) & 1_u32) << 21)
                        | (((arg0 >> 1) & 1_u32) << 22)
                        | (((arg0 >> 2) & 1_u32) << 23)
                        | (((arg0 >> 3) & 1_u32) << 24)
                        | (((arg0 >> 4) & 1_u32) << 25)
                        | (((arg0 >> 5) & 1_u32) << 26)
                        | (((arg0 >> 6) & 1_u32) << 27)
                        | (((arg0 >> 7) & 1_u32) << 28)
                        | (((arg0 >> 8) & 1_u32) << 29)
                        | (((arg0 >> 9) & 1_u32) << 30)
                        | (((read0 >> 31) & 1_u32) << 31)) as u32,
                )
            });
    }
    (read0 >> 0) & 1_u32
        | (((read0 >> 1) & 1_u32) << 1)
        | (((read0 >> 2) & 1_u32) << 2)
        | (((read0 >> 3) & 1_u32) << 3)
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
        | (((arg0 >> 0) & 1_u32) << 21)
        | (((arg0 >> 1) & 1_u32) << 22)
        | (((arg0 >> 2) & 1_u32) << 23)
        | (((arg0 >> 3) & 1_u32) << 24)
        | (((arg0 >> 4) & 1_u32) << 25)
        | (((arg0 >> 5) & 1_u32) << 26)
        | (((arg0 >> 6) & 1_u32) << 27)
        | (((arg0 >> 7) & 1_u32) << 28)
        | (((arg0 >> 8) & 1_u32) << 29)
        | (((arg0 >> 9) & 1_u32) << 30)
        | (((read0 >> 31) & 1_u32) << 31)
}
