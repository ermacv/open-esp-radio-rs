// @generated production candidate from the qualified register slice of
// hal_mac_txq_enable. The enclosing semantic adapter owns the excluded vendor
// queue-context, HE trigger-based, and instrumentation suffix.
use open_esp_radio_esp32s31_pac as pac;

#[inline(always)]
pub fn generated_hal_mac_txq_enable_register_slice(
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
            // SAFETY: the adapter-qualified register slice requires the
            // complete evidenced CONTROL3 image with ENABLE|VALID asserted.
            unsafe {
                wifi_mac_tx_queue_control_registers
                    .control(3)
                    .write_with_zero(|writer| {
                        writer.bits(((read0 & 0x3fff_ffff_u32) | 0xc000_0000_u32) as u32)
                    });
            }
        }
        1 => {
            // SAFETY: same closed register slice for CONTROL2.
            unsafe {
                wifi_mac_tx_queue_control_registers
                    .control(2)
                    .write_with_zero(|writer| {
                        writer.bits(((read0 & 0x3fff_ffff_u32) | 0xc000_0000_u32) as u32)
                    });
            }
        }
        2 => {
            // SAFETY: same closed register slice for CONTROL1.
            unsafe {
                wifi_mac_tx_queue_control_registers
                    .control(1)
                    .write_with_zero(|writer| {
                        writer.bits(((read0 & 0x3fff_ffff_u32) | 0xc000_0000_u32) as u32)
                    });
            }
        }
        3 => {
            // SAFETY: same closed register slice for CONTROL0.
            unsafe {
                wifi_mac_tx_queue_control_registers
                    .control(0)
                    .write_with_zero(|writer| {
                        writer.bits(((read0 & 0x3fff_ffff_u32) | 0xc000_0000_u32) as u32)
                    });
            }
        }
        _ => panic!("indexed PAC selector is outside the evidenced register bank"),
    }
    0
}
