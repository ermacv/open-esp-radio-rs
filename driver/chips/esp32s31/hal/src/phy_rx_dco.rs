//! Owned ESP32-S31 RX-DCO control-field access.
//!
//! The primary source is complete pinned
//! `libphy.a[phy_rx_cal.o]::phy_xtal_duty_cal`, size `0x392`. Complete rev0
//! ROM `phy_pbus_rx_dco_cal` at `0x2f82_8f44`, size `0x228`, independently
//! uses the same field around its bounded measurement graph.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_pac::RadioRegisters;

/// Capture and clear the two RX-DCO calibration-control bits.
///
/// Complete pinned `phy_xtal_duty_cal` saves bits 23:22 and clears them with
/// one fresh read before entering the nested RX-DCO measurement. The returned
/// value is the generated two-bit field value, with no physical register
/// identity exposed to the caller.
#[cfg(target_arch = "riscv32")]
pub fn capture_and_clear_control(registers: &mut RadioRegisters) -> u8 {
    registers.capture_and_clear_rx_dco_control()
}

/// Restore the captured RX-DCO calibration-control bits.
///
/// Complete pinned `phy_xtal_duty_cal` performs one fresh read and replaces
/// only the generated field after the nested measurement, preserving every
/// unrelated hardware bit.
#[cfg(target_arch = "riscv32")]
pub fn restore_control(registers: &mut RadioRegisters, saved_field: u8) {
    registers.restore_rx_dco_control(saved_field);
}
