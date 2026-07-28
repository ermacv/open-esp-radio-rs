//! Owned ESP32-S31 PHY power-detector register leaves.
//!
//! These methods own only finite MMIO. Delays, readiness repetition, PBus
//! transactions, calibration policy, and cleanup sequencing remain explicit
//! in the PHY state machines.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;

/// Apply the complete power-detector register initialization leaf.
///
/// Complete rev0 ROM `phy_pwdet_reg_init` at `0x2f82_634a`, size `0x5c`,
/// performs six finite stores through the PAC's table-zero, table-one,
/// control, reference, and auxiliary-mode identities.
#[cfg(target_arch = "riscv32")]
pub fn initialize_registers(
    platform: &mut impl crate::power_detector_platform::PhyPowerDetectorPlatformControl,
    registers: &mut RadioRegisters,
) {
    registers.initialize_power_detector_registers();
    crate::power_detector_platform::select_initialization_mode(platform);
}

/// Configure and enable the background TX power-control path.
///
/// Complete rev0 ROM `phy_tx_pwctrl_bg_init` at `0x2f82_67f6`, size `0x1e`,
/// calls complete `phy_en_pwdet` (`0x2f82_63da`, size `0x26`) and
/// `phy_pwdet_sar2_init` (`0x2f82_63a6`, size `0x34`), then sets bit 16 at
/// `POWER_DETECTOR_CONTROL`. All eight fresh-read/full-write operations are
/// preserved.
#[cfg(target_arch = "riscv32")]
pub fn configure_background(
    platform: &mut impl crate::power_detector_platform::PhyPowerDetectorPlatformControl,
    registers: &mut RadioRegisters,
) {
    configure_enabled(platform, registers);
    registers.enable_power_detector_background_control();
}

/// Configure the power-detector/SAR path without enabling background control.
///
/// Complete rev0 ROM `phy_en_pwdet` at `0x2f82_63da`, size `0x26`, and its
/// complete `phy_pwdet_sar2_init` callee at `0x2f82_63a6`, size `0x34`,
/// supply the eight ordered operations.
#[cfg(target_arch = "riscv32")]
pub fn configure_enabled(
    platform: &mut impl crate::power_detector_platform::PhyPowerDetectorPlatformControl,
    registers: &mut RadioRegisters,
) {
    registers.configure_power_detector_enabled();
    crate::power_detector_platform::select_enabled_mode(platform);
}

/// Select the auxiliary power-detector calibration mode.
///
/// Complete rev0 ROM `phy_txcal_debuge_mode_` at `0x2f82_44fe`, size `0x56`,
/// replaces the PAC's three-bit auxiliary-mode field with mode two after
/// enabling PWDET.
#[cfg(target_arch = "riscv32")]
pub fn configure_calibration_mode(
    platform: &mut impl crate::power_detector_platform::PhyPowerDetectorPlatformControl,
) {
    crate::power_detector_platform::select_calibration_mode(platform);
}

/// Capture and replace the two TX-DC power-detector fields.
///
/// Complete pinned `libphy.a[phy_tx_cal.o]::phy_txdc_cal_pwdet_init`, size
/// `0x208`, first reads `POWER_DETECTOR_TABLE_1` and
/// `POWER_DETECTOR_CONTROL`, then replaces the low table byte with `0xf0` and
/// control bits 11:4 with `0x78`. The returned tuple contains the original
/// low byte and its original already-shifted mask.
#[cfg(target_arch = "riscv32")]
pub fn capture_txdc_fields(registers: &mut RadioRegisters) -> (u8, u32) {
    registers.capture_txdc_power_detector_fields()
}

/// Select the TX-DC PWDET SAR mode after the initial PBus setup.
///
/// Complete pinned `libphy.a[phy_tx_cal.o]::phy_txdc_cal_pwdet_init`, size
/// `0x208`, replaces the two-bit PAC SAR-mode field with one at this point.
#[cfg(target_arch = "riscv32")]
pub fn configure_txdc_sar(registers: &mut RadioRegisters) {
    registers.configure_txdc_power_detector_sar();
}

/// Restore captured TX-DC fields and select the final SAR mode.
///
/// The unconditional cleanup tail of complete pinned
/// `libphy.a[phy_tx_cal.o]::phy_txdc_cal_pwdet_init`, size `0x208`, restores
/// the table-one low byte and control calibration field, then sets the
/// two-bit SAR-mode field.
#[cfg(target_arch = "riscv32")]
pub fn restore_txdc_fields(
    registers: &mut RadioRegisters,
    power_table_low: u8,
    shifted_power_control_field: u32,
) {
    registers.restore_txdc_power_detector_fields(power_table_low, shifted_power_control_field);
}

/// Publish one power-detector reference word.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// and its complete callers write the evidenced `0`, `0x5555`, or `0xaaaa`
/// values to `POWER_DETECTOR_REFERENCE`; PWDET SAR initialization additionally
/// uses `0x016a`. This finite leaf performs the full 32-bit zero-extended
/// store.
#[cfg(target_arch = "riscv32")]
pub fn write_reference(registers: &mut RadioRegisters, value: u16) {
    registers.write_power_detector_reference(value);
}

/// Pulse the power-detector SAR trigger.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// clears then sets the PAC `SAR_TRIGGER` bit through two fresh reads, with the
/// one-microsecond delay remaining in the caller.
#[cfg(target_arch = "riscv32")]
pub fn trigger_sar(registers: &mut RadioRegisters) {
    registers.trigger_power_detector_sar();
}

/// Read one power-detector readiness register image.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// samples `POWER_DETECTOR_SAR_CONTROL_STATUS`; repetition and deadline
/// ownership remain in the Rust async state machine.
#[cfg(target_arch = "riscv32")]
pub fn sample_ready(registers: &mut RadioRegisters) -> u32 {
    registers.power_detector_ready_image()
}

/// Read one power-detector SAR result register image.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// consumes the upper thirteen-bit sample from `POWER_DETECTOR_SAR_RESULT`.
/// Extraction is retained by the caller to preserve its existing transition
/// observation.
#[cfg(target_arch = "riscv32")]
pub fn sample_sar(registers: &mut RadioRegisters) -> u32 {
    registers.power_detector_sar_image()
}
