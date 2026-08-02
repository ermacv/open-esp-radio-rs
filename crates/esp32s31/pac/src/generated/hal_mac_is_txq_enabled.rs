// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_mac_is_txq_enabled
use open_esp_radio_esp32s31_svd as pac;

#[inline(always)]
pub fn generated_hal_mac_is_txq_enabled(
    wifi_mac_tx_queue_control_registers: &pac::WifiMacTxQueueControl,
    arg0: u32,
) -> u32 {
    let read0 = match arg0 {
        0 => wifi_mac_tx_queue_control_registers.control(3).read().bits() as u32,
        1 => wifi_mac_tx_queue_control_registers.control(2).read().bits() as u32,
        2 => wifi_mac_tx_queue_control_registers.control(1).read().bits() as u32,
        3 => wifi_mac_tx_queue_control_registers.control(0).read().bits() as u32,
        _ => panic!("indexed PAC selector is outside the evidenced register bank"),
    };
    (read0 >> 31) & 1_u32
}
