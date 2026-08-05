// @generated production candidate; review ownership and public types before integration.
// Source vendor symbol: hal_mac_set_txq_invalid
use open_esp_radio_esp32s31_pac as pac;

#[inline(always)]
pub fn generated_hal_mac_set_txq_invalid(
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
    match arg0 {
        0 => {
            // SAFETY: the Effect Contract requires the complete evidenced register image.
            unsafe {
                wifi_mac_tx_queue_control_registers
                    .control(3)
                    .write_with_zero(|writer| {
                        writer.bits(((read0 & 0xbfffffff_u32) | 0x00000000_u32) as u32)
                    });
            }
        }
        1 => {
            // SAFETY: the Effect Contract requires the complete evidenced register image.
            unsafe {
                wifi_mac_tx_queue_control_registers
                    .control(2)
                    .write_with_zero(|writer| {
                        writer.bits(((read0 & 0xbfffffff_u32) | 0x00000000_u32) as u32)
                    });
            }
        }
        2 => {
            // SAFETY: the Effect Contract requires the complete evidenced register image.
            unsafe {
                wifi_mac_tx_queue_control_registers
                    .control(1)
                    .write_with_zero(|writer| {
                        writer.bits(((read0 & 0xbfffffff_u32) | 0x00000000_u32) as u32)
                    });
            }
        }
        3 => {
            // SAFETY: the Effect Contract requires the complete evidenced register image.
            unsafe {
                wifi_mac_tx_queue_control_registers
                    .control(0)
                    .write_with_zero(|writer| {
                        writer.bits(((read0 & 0xbfffffff_u32) | 0x00000000_u32) as u32)
                    });
            }
        }
        _ => panic!("indexed PAC selector is outside the evidenced register bank"),
    }
    ((0x020104d7_u32).wrapping_sub(arg0)).wrapping_shl((0x00000004_u32) & 31)
}
