//! Owned ESP32-S31 PHY power-detector register leaves.
//!
//! These methods own only finite MMIO. Delays, readiness repetition, PBus
//! transactions, calibration policy, and cleanup sequencing remain explicit
//! in the PHY state machines.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(any(test, target_arch = "riscv32"))]
use open_esp_radio_pac_esp32s31::{power::phy_baseband_config_oracle as pd, Field32, Register32};

#[cfg(any(test, target_arch = "riscv32"))]
const fn field_value(field: Field32, value: u32) -> u32 {
    match field.checked_value(value) {
        Some(value) => value,
        None => panic!("value does not fit recovered register field"),
    }
}

#[cfg(any(test, target_arch = "riscv32"))]
trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);

    fn modify(&mut self, register: Register32, clear_mask: u32, set_bits: u32) {
        let previous = self.read(register);
        self.write(register, (previous & !clear_mask) | (set_bits & clear_mask));
    }

    fn replace(&mut self, register: Register32, field: Field32, value: u32) {
        self.modify(register, field.mask(), field_value(field, value));
    }

    fn set(&mut self, register: Register32, field: Field32) {
        self.modify(register, field.mask(), field.mask());
    }

    fn clear(&mut self, register: Register32, field: Field32) {
        self.modify(register, field.mask(), 0);
    }
}

#[cfg(target_arch = "riscv32")]
impl RegisterIo for RadioRegisters {
    fn read(&mut self, register: Register32) -> u32 {
        self.read32(register)
    }

    fn write(&mut self, register: Register32, value: u32) {
        self.write32(register, value);
    }
}

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
    initialize_registers_with(registers);
    crate::power_detector_platform::select_initialization_mode(platform);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn initialize_registers_with(io: &mut impl RegisterIo) {
    io.write(pd::POWER_DETECTOR_TABLE_0_OPAQUE, 0x0f0f_0fff);
    io.write(pd::POWER_DETECTOR_TABLE_1, 0x00ff_0f64);
    io.replace(
        pd::POWER_DETECTOR_CONTROL,
        pd::power_detector_control::CALIBRATION_FIELD_UNKNOWN,
        0x50,
    );
    io.write(pd::POWER_DETECTOR_REFERENCE, 0x0000_aaaa);
    io.replace(
        pd::POWER_DETECTOR_CONTROL,
        pd::power_detector_control::INITIALIZATION_MODE_UNKNOWN,
        2,
    );
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
    registers.set(
        pd::POWER_DETECTOR_CONTROL,
        pd::power_detector_control::BACKGROUND_CONTROL_ENABLE_UNKNOWN,
    );
}

#[cfg(test)]
fn configure_background_with(io: &mut impl RegisterIo) {
    configure_enabled_with(io);
    io.set(
        pd::POWER_DETECTOR_CONTROL,
        pd::power_detector_control::BACKGROUND_CONTROL_ENABLE_UNKNOWN,
    );
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
    configure_enabled_with(registers);
    crate::power_detector_platform::select_enabled_mode(platform);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_enabled_with(io: &mut impl RegisterIo) {
    // The ROM clears these adjacent bits through three independent reads.
    let clear = pd::power_detector_control::ENABLE_CLEAR_UNKNOWN;
    io.modify(pd::POWER_DETECTOR_CONTROL, field_value(clear, 2), 0);
    io.modify(pd::POWER_DETECTOR_CONTROL, field_value(clear, 1), 0);
    io.modify(pd::POWER_DETECTOR_CONTROL, field_value(clear, 4), 0);
    io.replace(
        pd::POWER_DETECTOR_SAR_CONTROL_STATUS,
        pd::power_detector_sar_control_status::SAR_MODE_UNKNOWN,
        3,
    );
    io.clear(
        pd::POWER_DETECTOR_SAR_CONTROL_STATUS,
        pd::power_detector_sar_control_status::SAR_CONFIG_CLEAR_UNKNOWN,
    );
    io.write(pd::POWER_DETECTOR_REFERENCE, 0x0000_016a);
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
    capture_txdc_fields_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn capture_txdc_fields_with(io: &mut impl RegisterIo) -> (u8, u32) {
    let table = io.read(pd::POWER_DETECTOR_TABLE_1);
    let control = io.read(pd::POWER_DETECTOR_CONTROL);
    let table_field = pd::power_detector_table_1::TX_DC_TEMPORARY_LOW_UNKNOWN;
    let control_field = pd::power_detector_control::CALIBRATION_FIELD_UNKNOWN;
    io.write(
        pd::POWER_DETECTOR_TABLE_1,
        (table & !table_field.mask()) | field_value(table_field, 0xf0),
    );
    io.write(
        pd::POWER_DETECTOR_CONTROL,
        (control & !control_field.mask()) | field_value(control_field, 0x78),
    );
    (
        (table & table_field.mask()) as u8,
        control & control_field.mask(),
    )
}

/// Select the TX-DC PWDET SAR mode after the initial PBus setup.
///
/// Complete pinned `libphy.a[phy_tx_cal.o]::phy_txdc_cal_pwdet_init`, size
/// `0x208`, replaces the two-bit PAC SAR-mode field with one at this point.
#[cfg(target_arch = "riscv32")]
pub fn configure_txdc_sar(registers: &mut RadioRegisters) {
    configure_txdc_sar_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn configure_txdc_sar_with(io: &mut impl RegisterIo) {
    io.replace(
        pd::POWER_DETECTOR_SAR_CONTROL_STATUS,
        pd::power_detector_sar_control_status::SAR_MODE_UNKNOWN,
        1,
    );
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
    restore_txdc_fields_with(registers, power_table_low, shifted_power_control_field);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn restore_txdc_fields_with(
    io: &mut impl RegisterIo,
    power_table_low: u8,
    shifted_power_control_field: u32,
) {
    io.replace(
        pd::POWER_DETECTOR_TABLE_1,
        pd::power_detector_table_1::TX_DC_TEMPORARY_LOW_UNKNOWN,
        u32::from(power_table_low),
    );
    let field = pd::power_detector_control::CALIBRATION_FIELD_UNKNOWN;
    io.modify(
        pd::POWER_DETECTOR_CONTROL,
        field.mask(),
        shifted_power_control_field,
    );
    io.set(
        pd::POWER_DETECTOR_SAR_CONTROL_STATUS,
        pd::power_detector_sar_control_status::SAR_MODE_UNKNOWN,
    );
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
    registers.write32(pd::POWER_DETECTOR_REFERENCE, u32::from(value));
}

/// Pulse the power-detector SAR trigger.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// clears then sets the PAC `SAR_TRIGGER` bit through two fresh reads, with the
/// one-microsecond delay remaining in the caller.
#[cfg(target_arch = "riscv32")]
pub fn trigger_sar(registers: &mut RadioRegisters) {
    trigger_sar_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn trigger_sar_with(io: &mut impl RegisterIo) {
    let trigger = pd::power_detector_control::SAR_TRIGGER;
    io.clear(pd::POWER_DETECTOR_CONTROL, trigger);
    io.set(pd::POWER_DETECTOR_CONTROL, trigger);
}

/// Read one power-detector readiness register image.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// samples `POWER_DETECTOR_SAR_CONTROL_STATUS`; repetition and deadline
/// ownership remain in the Rust async state machine.
#[cfg(target_arch = "riscv32")]
pub fn sample_ready(registers: &mut RadioRegisters) -> u32 {
    registers.read32(pd::POWER_DETECTOR_SAR_CONTROL_STATUS)
}

/// Read one power-detector SAR result register image.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// consumes the upper thirteen-bit sample from `POWER_DETECTOR_SAR_RESULT`.
/// Extraction is retained by the caller to preserve its existing transition
/// observation.
#[cfg(target_arch = "riscv32")]
pub fn sample_sar(registers: &mut RadioRegisters) -> u32 {
    registers.read32(pd::POWER_DETECTOR_SAR_RESULT)
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    #[derive(Default)]
    struct FakeRegisters {
        values: Vec<(Register32, u32)>,
        reads: Vec<Register32>,
        writes: Vec<(Register32, u32)>,
    }

    impl FakeRegisters {
        fn with(mut self, register: Register32, value: u32) -> Self {
            self.values.push((register, value));
            self
        }
    }

    impl RegisterIo for FakeRegisters {
        fn read(&mut self, register: Register32) -> u32 {
            self.reads.push(register);
            self.values
                .iter()
                .find_map(|(candidate, value)| (*candidate == register).then_some(*value))
                .unwrap_or(0)
        }

        fn write(&mut self, register: Register32, value: u32) {
            if let Some(entry) = self
                .values
                .iter_mut()
                .find(|(candidate, _)| *candidate == register)
            {
                entry.1 = value;
            } else {
                self.values.push((register, value));
            }
            self.writes.push((register, value));
        }
    }

    #[test]
    fn register_initialization_is_five_internal_oracle_stores() {
        let mut io = FakeRegisters::default();
        initialize_registers_with(&mut io);

        assert_eq!(io.writes.len(), 5);
        assert_eq!(
            io.writes,
            [
                (pd::POWER_DETECTOR_TABLE_0_OPAQUE, 0x0f0f_0fff),
                (pd::POWER_DETECTOR_TABLE_1, 0x00ff_0f64),
                (pd::POWER_DETECTOR_CONTROL, 0x0000_0500),
                (pd::POWER_DETECTOR_REFERENCE, 0x0000_aaaa),
                (pd::POWER_DETECTOR_CONTROL, 0x0020_0500),
            ]
        );
    }

    #[test]
    fn enabled_sequence_keeps_three_independent_clears() {
        let mut io = FakeRegisters::default()
            .with(pd::POWER_DETECTOR_CONTROL, 0x0000_000e)
            .with(pd::POWER_DETECTOR_SAR_CONTROL_STATUS, 0xffff_ffff);
        configure_enabled_with(&mut io);

        assert_eq!(io.writes.len(), 6);
        assert_eq!(io.writes[0].1 & 0x0e, 0x0a);
        assert_eq!(io.writes[1].1 & 0x0e, 0x08);
        assert_eq!(io.writes[2].1 & 0x0e, 0);
        assert_eq!(
            io.writes.last(),
            Some(&(pd::POWER_DETECTOR_REFERENCE, 0x0000_016a))
        );
    }

    #[test]
    fn background_sequence_adds_only_the_final_enable_edge() {
        let mut io = FakeRegisters::default();
        configure_background_with(&mut io);

        assert_eq!(io.writes.len(), 7);
        assert_eq!(
            io.writes.last(),
            Some(&(pd::POWER_DETECTOR_CONTROL, 0x0001_0000))
        );
    }

    #[test]
    fn txdc_capture_and_restore_round_trip_only_owned_fields() {
        let mut io = FakeRegisters::default()
            .with(pd::POWER_DETECTOR_TABLE_1, 0xa5a5_5a34)
            .with(pd::POWER_DETECTOR_CONTROL, 0x5a5a_0ab5);
        let saved = capture_txdc_fields_with(&mut io);

        assert_eq!(saved, (0x34, 0x0000_0ab0));
        assert_eq!(
            io.reads,
            [pd::POWER_DETECTOR_TABLE_1, pd::POWER_DETECTOR_CONTROL]
        );
        assert_eq!(io.writes[0].1, 0xa5a5_5af0);
        assert_eq!(io.writes[1].1, 0x5a5a_0785);

        configure_txdc_sar_with(&mut io);
        restore_txdc_fields_with(&mut io, saved.0, saved.1);
        assert_eq!(io.writes[3].1, 0xa5a5_5a34);
        assert_eq!(io.writes[4].1, 0x5a5a_0ab5);
        assert_eq!(io.writes[5].1 & 0x3000, 0x3000);
    }

    #[test]
    fn sar_trigger_is_clear_then_set() {
        let mut io = FakeRegisters::default().with(pd::POWER_DETECTOR_CONTROL, 0x11);
        trigger_sar_with(&mut io);

        assert_eq!(
            io.writes,
            [
                (pd::POWER_DETECTOR_CONTROL, 0x10),
                (pd::POWER_DETECTOR_CONTROL, 0x11),
            ]
        );
    }
}
