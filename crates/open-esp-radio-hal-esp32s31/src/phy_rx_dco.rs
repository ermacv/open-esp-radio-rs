//! Owned ESP32-S31 RX-DCO control-field access.
//!
//! The primary source is complete pinned
//! `libphy.a[phy_rx_cal.o]::phy_xtal_duty_cal`, size `0x392`. Complete rev0
//! ROM `phy_pbus_rx_dco_cal` at `0x2f82_8f44`, size `0x228`, independently
//! uses the same field around its bounded measurement graph.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(any(test, target_arch = "riscv32"))]
use open_esp_radio_pac_esp32s31::{power::phy_rx_dco_oracle as rx_dco, Field32, Register32};

#[cfg(any(test, target_arch = "riscv32"))]
trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);
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

#[cfg(any(test, target_arch = "riscv32"))]
fn capture_and_clear_with(io: &mut impl RegisterIo, field: Field32) -> u32 {
    let saved_field = io.read(rx_dco::CONTROL) & field.mask();
    let current = io.read(rx_dco::CONTROL);
    io.write(rx_dco::CONTROL, current & !field.mask());
    saved_field
}

/// Capture and clear the two RX-DCO calibration-control bits.
///
/// Complete pinned `phy_xtal_duty_cal` saves bits 23:22 and clears them with
/// one fresh read before entering the nested RX-DCO measurement. The returned
/// value remains encoded in its register position so the caller can carry it
/// unchanged through its identity-bound transition.
#[cfg(target_arch = "riscv32")]
pub fn capture_and_clear_control(registers: &mut RadioRegisters) -> u32 {
    capture_and_clear_with(registers, rx_dco::control::CALIBRATION_CONTROL_UNKNOWN)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn restore_with(io: &mut impl RegisterIo, field: Field32, saved_field: u32) {
    let previous = io.read(rx_dco::CONTROL);
    io.write(
        rx_dco::CONTROL,
        (previous & !field.mask()) | (saved_field & field.mask()),
    );
}

/// Restore the captured RX-DCO calibration-control bits.
///
/// Complete pinned `phy_xtal_duty_cal` performs one fresh read and replaces
/// only bits 23:22 after the nested measurement. Values outside the recovered
/// field are ignored, preserving every unrelated hardware bit.
#[cfg(target_arch = "riscv32")]
pub fn restore_control(registers: &mut RadioRegisters, saved_field: u32) {
    restore_with(
        registers,
        rx_dco::control::CALIBRATION_CONTROL_UNKNOWN,
        saved_field,
    );
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Read(Register32, u32),
        Write(Register32, u32),
    }

    struct FakeRegisters {
        value: u32,
        operations: Vec<Operation>,
    }

    impl FakeRegisters {
        fn new(value: u32) -> Self {
            Self {
                value,
                operations: Vec::new(),
            }
        }
    }

    impl RegisterIo for FakeRegisters {
        fn read(&mut self, register: Register32) -> u32 {
            self.operations.push(Operation::Read(register, self.value));
            self.value
        }

        fn write(&mut self, register: Register32, value: u32) {
            self.value = value;
            self.operations.push(Operation::Write(register, value));
        }
    }

    #[test]
    fn capture_returns_only_the_field_and_preserves_every_other_bit() {
        let initial = 0x12ff_5678;
        let mut io = FakeRegisters::new(initial);

        assert_eq!(
            capture_and_clear_with(&mut io, rx_dco::control::CALIBRATION_CONTROL_UNKNOWN,),
            0x00c0_0000
        );
        assert_eq!(
            io.operations,
            [
                Operation::Read(rx_dco::CONTROL, initial),
                Operation::Read(rx_dco::CONTROL, initial),
                Operation::Write(rx_dco::CONTROL, 0x123f_5678),
            ]
        );
    }

    #[test]
    fn restore_uses_one_fresh_read_and_truncates_to_the_recovered_field() {
        let initial = 0xa53f_5678;
        let mut io = FakeRegisters::new(initial);

        restore_with(
            &mut io,
            rx_dco::control::CALIBRATION_CONTROL_UNKNOWN,
            0xff7f_ffff,
        );
        assert_eq!(
            io.operations,
            [
                Operation::Read(rx_dco::CONTROL, initial),
                Operation::Write(rx_dco::CONTROL, 0xa57f_5678),
            ]
        );
    }
}
