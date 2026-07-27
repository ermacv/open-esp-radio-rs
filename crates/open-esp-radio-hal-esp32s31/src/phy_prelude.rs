//! Owned ESP32-S31 cold-PHY prelude register leaves.
//!
//! This module contains only semantic operations used before and around the
//! RF initializer. Numeric MMIO identities stay in the generated PAC.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(test)]
use open_esp_radio_pac_esp32s31::{
    power::{modem_lpcon, phy_cold_deadline_oracle},
    Register32,
};

#[cfg(test)]
trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);
}

#[cfg(test)]
fn configure_fixed_xtal_40mhz_with(io: &mut impl RegisterIo) {
    let field = modem_lpcon::tick_conf::MODEM_PWR_TICK_TARGET;
    let previous = io.read(modem_lpcon::TICK_CONF);
    io.write(
        modem_lpcon::TICK_CONF,
        field.checked_insert(previous, 39).unwrap_or(previous),
    );
}

/// Configure the fixed ESP32-S31 40 MHz crystal-derived tick value.
///
/// Complete pinned `libphy.a[phy_init.o]::phy_get_xtal_freq`, size `0x40`,
/// replaces the six-bit target with `frequency_mhz - 1`. ESP32-S31's public
/// chip contract fixes the crystal at 40 MHz, so this finite method performs
/// one fresh PAC read and writes the exact value 39 without hidden RTC state.
#[cfg(target_arch = "riscv32")]
pub fn configure_fixed_xtal_40mhz(registers: &mut RadioRegisters) {
    registers.configure_fixed_xtal_40mhz_tick();
}

#[cfg(test)]
fn sample_sdm_deadline_counter_with(io: &mut impl RegisterIo) -> u32 {
    io.read(phy_cold_deadline_oracle::DEADLINE_COUNTER_UNKNOWN)
}

/// Sample the full-width counter used by the SDM-stability deadline.
///
/// Complete rev0 ROM `phy_wait_i2c_sdm_stable` at `0x2f823e76`, size
/// `0x4a`, samples this word before and after each PHY-I2C read and compares
/// their wrapping unsigned difference with 9,999. This method performs one
/// read; deadline arithmetic and retry ownership stay in the transition.
#[cfg(target_arch = "riscv32")]
pub fn sample_sdm_deadline_counter(registers: &mut RadioRegisters) -> u32 {
    registers.sample_sdm_deadline_counter()
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
    fn fixed_xtal_replaces_only_the_public_tick_target() {
        let initial = 0xa5a5_5aff;
        let mut io = FakeRegisters::new(initial);

        configure_fixed_xtal_40mhz_with(&mut io);

        assert_eq!(
            io.operations,
            [
                Operation::Read(modem_lpcon::TICK_CONF, initial),
                Operation::Write(modem_lpcon::TICK_CONF, 0xa5a5_5ae7),
            ]
        );
    }

    #[test]
    fn sdm_deadline_sample_is_one_full_pac_read() {
        let mut io = FakeRegisters::new(0xffff_fffe);

        assert_eq!(sample_sdm_deadline_counter_with(&mut io), 0xffff_fffe);
        assert_eq!(
            io.operations,
            [Operation::Read(
                phy_cold_deadline_oracle::DEADLINE_COUNTER_UNKNOWN,
                0xffff_fffe,
            )]
        );
    }
}
