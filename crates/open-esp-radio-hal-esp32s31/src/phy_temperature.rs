//! Owned ESP32-S31 temperature-sensor MMIO leaves.
//!
//! The primary sources are pinned `libphy.a[phy_tsens.o]::phy_tsens_read_init`,
//! size `0x36`, and the complete rev0 ROM `phy_set_tsens_power_` body at
//! `0x2f82_5dc8`, size `0x1c`. Temperature-code identity is independently
//! proven by complete ROM `phy_tsens_code_read` at `0x2f82_5ee0`, size
//! `0x0c`, and `phy_tsens_temp_read_local` at `0x2f82_5f1e`, size `0x5e`.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(any(test, target_arch = "riscv32"))]
use open_esp_radio_pac_esp32s31::{
    power::{phy_temperature_sensor_oracle as sensor, phy_temperature_system_oracle as system},
    Field32, Register32,
};

#[cfg(any(test, target_arch = "riscv32"))]
trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);

    fn set(&mut self, register: Register32, field: Field32) {
        let previous = self.read(register);
        self.write(register, previous | field.mask());
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

/// Configure the temperature-sensor read path.
///
/// Complete pinned `phy_tsens_read_init` performs five independent
/// read/modify/write transactions in this order: sensor-control bit 0,
/// system-control bit 30, sensor-control bit 23, sensor-control bit 9, then
/// sensor power bit 22. The final transaction is the inlined semantics of
/// complete ROM `phy_set_tsens_power_(1)`. Both archive ABI arguments are
/// ignored and therefore do not cross this safe boundary.
#[cfg(target_arch = "riscv32")]
pub fn initialize(registers: &mut RadioRegisters) {
    initialize_with(registers);
}

#[cfg(any(test, target_arch = "riscv32"))]
fn initialize_with(io: &mut impl RegisterIo) {
    io.set(
        sensor::SENSOR_CONTROL,
        sensor::sensor_control::READ_PATH_ENABLE_UNKNOWN,
    );
    io.set(
        system::SYSTEM_CONTROL,
        system::system_control::SENSOR_CLOCK_ENABLE_UNKNOWN,
    );
    io.set(
        sensor::SENSOR_CONTROL,
        sensor::sensor_control::READOUT_ENABLE_UNKNOWN,
    );
    io.set(
        sensor::SENSOR_CONTROL,
        sensor::sensor_control::CONVERSION_ENABLE_UNKNOWN,
    );
    io.set(
        sensor::SENSOR_CODE_POWER,
        sensor::sensor_code_power::POWER_ENABLE,
    );
}

/// Sample the unsigned temperature code exactly once.
///
/// Complete ROM `phy_tsens_code_read` and `phy_tsens_temp_read_local` both
/// read `SENSOR_CODE_POWER` once and zero-extend its low-byte `CODE` field.
/// Readiness, conversion arithmetic and range selection remain in the
/// caller-driven PHY transition.
#[cfg(target_arch = "riscv32")]
pub fn read_code(registers: &mut RadioRegisters) -> u8 {
    read_code_with(registers)
}

#[cfg(any(test, target_arch = "riscv32"))]
fn read_code_with(io: &mut impl RegisterIo) -> u8 {
    sensor::sensor_code_power::CODE.extract(io.read(sensor::SENSOR_CODE_POWER)) as u8
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

    #[derive(Default)]
    struct FakeRegisters {
        values: Vec<(Register32, u32)>,
        operations: Vec<Operation>,
    }

    impl FakeRegisters {
        fn with(mut self, register: Register32, value: u32) -> Self {
            self.values.push((register, value));
            self
        }

        fn value(&self, register: Register32) -> u32 {
            self.values
                .iter()
                .find_map(|(candidate, value)| (*candidate == register).then_some(*value))
                .unwrap_or(0)
        }
    }

    impl RegisterIo for FakeRegisters {
        fn read(&mut self, register: Register32) -> u32 {
            let value = self.value(register);
            self.operations.push(Operation::Read(register, value));
            value
        }

        fn write(&mut self, register: Register32, value: u32) {
            if let Some((_, current)) = self
                .values
                .iter_mut()
                .find(|(candidate, _)| *candidate == register)
            {
                *current = value;
            } else {
                self.values.push((register, value));
            }
            self.operations.push(Operation::Write(register, value));
        }
    }

    #[test]
    fn initialization_preserves_all_five_fresh_reads_and_their_order() {
        let sensor_initial = 0x1000_0400;
        let system_initial = 0x0100_0003;
        let code_power_initial = 0x0080_00a5;
        let mut io = FakeRegisters::default()
            .with(sensor::SENSOR_CONTROL, sensor_initial)
            .with(system::SYSTEM_CONTROL, system_initial)
            .with(sensor::SENSOR_CODE_POWER, code_power_initial);

        initialize_with(&mut io);

        let sensor_first = sensor_initial | sensor::sensor_control::READ_PATH_ENABLE_UNKNOWN.mask();
        let system_next =
            system_initial | system::system_control::SENSOR_CLOCK_ENABLE_UNKNOWN.mask();
        let sensor_second = sensor_first | sensor::sensor_control::READOUT_ENABLE_UNKNOWN.mask();
        let sensor_third = sensor_second | sensor::sensor_control::CONVERSION_ENABLE_UNKNOWN.mask();
        let code_power_next = code_power_initial | sensor::sensor_code_power::POWER_ENABLE.mask();
        assert_eq!(
            io.operations,
            [
                Operation::Read(sensor::SENSOR_CONTROL, sensor_initial),
                Operation::Write(sensor::SENSOR_CONTROL, sensor_first),
                Operation::Read(system::SYSTEM_CONTROL, system_initial),
                Operation::Write(system::SYSTEM_CONTROL, system_next),
                Operation::Read(sensor::SENSOR_CONTROL, sensor_first),
                Operation::Write(sensor::SENSOR_CONTROL, sensor_second),
                Operation::Read(sensor::SENSOR_CONTROL, sensor_second),
                Operation::Write(sensor::SENSOR_CONTROL, sensor_third),
                Operation::Read(sensor::SENSOR_CODE_POWER, code_power_initial),
                Operation::Write(sensor::SENSOR_CODE_POWER, code_power_next),
            ]
        );
    }

    #[test]
    fn code_sample_reads_one_shared_word_and_extracts_only_the_low_byte() {
        let register_image = 0xa5c0_12fe;
        let mut io = FakeRegisters::default().with(sensor::SENSOR_CODE_POWER, register_image);

        assert_eq!(read_code_with(&mut io), 0xfe);
        assert_eq!(
            io.operations,
            [Operation::Read(sensor::SENSOR_CODE_POWER, register_image)]
        );
    }
}
