// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_mac_get_txq_in_trig_flow_state
use open_esp_radio_esp32s31_pac as pac;

#[inline(always)]
pub fn generated_hal_mac_get_txq_in_trig_flow_state(
    wifi_mac_tx_common_registers: &pac::WifiMacTxCommon,
) -> u32 {
    let read0 = wifi_mac_tx_common_registers.queue_state().read().bits() as u32;
    (read0 >> 24) & 1_u32
        | (((read0 >> 25) & 1_u32) << 1)
        | (((read0 >> 26) & 1_u32) << 2)
        | (((read0 >> 27) & 1_u32) << 3)
        | (((read0 >> 28) & 1_u32) << 4)
        | (((read0 >> 29) & 1_u32) << 5)
        | (((read0 >> 30) & 1_u32) << 6)
        | (((read0 >> 31) & 1_u32) << 7)
}
