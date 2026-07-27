//! Owned ESP32-S31 analog PHY-I2C power and reset leaves.
//!
//! Register identities and field names come from the pinned S31 PMU SVD and
//! headers. Operation order comes independently from the complete
//! `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new` body. Keeping those two kinds
//! of evidence separate prevents a register-layout name from silently
//! becoming an unevidenced initialization sequence.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
#[cfg(test)]
use open_esp_radio_pac_esp32s31::{power::pmu, Register32};

#[cfg(test)]
const RF_CIRCUIT_POWER: u32 = pmu::rf_pwc::XPD_RF_CIRCUIT.mask();
#[cfg(test)]
const TIE_HIGH_BB_I2C_POWER: u32 = pmu::imm_hp_ck_power_0::TIE_HIGH_XPD_BB_I2C.mask();
#[cfg(test)]
const ANALOG_I2C_POWER: u32 = pmu::ana_peri_pwr_ctrl::XPD_PERIF_I2C.mask();
#[cfg(test)]
const ANALOG_I2C_RESET_RELEASE: u32 = pmu::ana_peri_pwr_ctrl::RSTB_PERIF_I2C.mask();

#[cfg(test)]
trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);

    fn modify(&mut self, register: Register32, clear_mask: u32, set_bits: u32) {
        let previous = self.read(register);
        self.write(register, (previous & !clear_mask) | (set_bits & clear_mask));
    }
}

/// Apply the two register updates before the 100 us analog-I2C power delay.
///
/// Basis: complete pinned
/// `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`, offsets `0x2e..0x4e`.
/// It clears `PMU.RF_PWC.XPD_RF_CIRCUIT`, then clears
/// `PMU.IMM_HP_CK_POWER_0.TIE_HIGH_XPD_BB_I2C`. The S31 header marks the
/// latter field WT, but this exact blob body performs a read/modify/write;
/// the PAC records that access-model conflict explicitly.
///
/// The mutable register capability is borrowed from `Radio<P, Powered>`, so
/// no additional `unsafe` block or raw address escapes into the PHY state
/// machine.
#[cfg(target_arch = "riscv32")]
pub fn prepare_open_i2c_pre_delay(registers: &mut RadioRegisters) {
    registers.set_rf_circuit_power(false);
    registers.set_bb_i2c_power_tie(false);
}

#[cfg(test)]
fn prepare_open_i2c_pre_delay_with(io: &mut impl RegisterIo) {
    io.modify(pmu::RF_PWC, RF_CIRCUIT_POWER, 0);
    io.modify(pmu::IMM_HP_CK_POWER_0, TIE_HIGH_BB_I2C_POWER, 0);
}

/// Power the RF/analog-I2C circuits and release the peripheral-I2C reset.
///
/// Basis: complete pinned
/// `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`, common suffix after its
/// optional 100 us delay. It powers all 16 recovered RF circuit bits, ties
/// BB-I2C power high, and then preserves the blob's conditional
/// `XPD_PERIF_I2C` / `RSTB_PERIF_I2C` edge sequence. In particular, when
/// analog-I2C was powered down, reset is explicitly asserted before release;
/// this is not collapsed into one final OR.
///
/// PMU field identities come from the pinned ESP32-S31 SVD and official
/// `pmu_reg.h`; operation ordering comes only from the blob body.
#[cfg(target_arch = "riscv32")]
pub fn complete_open_i2c_power_and_reset(registers: &mut RadioRegisters) {
    registers.set_rf_circuit_power(true);
    registers.set_bb_i2c_power_tie(true);

    if !registers.analog_i2c_is_powered() {
        registers.set_analog_i2c_power(true);
        registers.set_analog_i2c_reset_released(false);
        registers.set_analog_i2c_reset_released(true);
    }
    if !registers.analog_i2c_reset_is_released() {
        registers.set_analog_i2c_reset_released(true);
    }
}

#[cfg(test)]
fn complete_open_i2c_power_and_reset_with(io: &mut impl RegisterIo) {
    io.modify(pmu::RF_PWC, RF_CIRCUIT_POWER, RF_CIRCUIT_POWER);
    io.modify(
        pmu::IMM_HP_CK_POWER_0,
        TIE_HIGH_BB_I2C_POWER,
        TIE_HIGH_BB_I2C_POWER,
    );

    if io.read(pmu::ANA_PERI_PWR_CTRL) & ANALOG_I2C_POWER == 0 {
        io.modify(pmu::ANA_PERI_PWR_CTRL, ANALOG_I2C_POWER, ANALOG_I2C_POWER);
        io.modify(pmu::ANA_PERI_PWR_CTRL, ANALOG_I2C_RESET_RELEASE, 0);
        io.modify(
            pmu::ANA_PERI_PWR_CTRL,
            ANALOG_I2C_RESET_RELEASE,
            ANALOG_I2C_RESET_RELEASE,
        );
    }
    if io.read(pmu::ANA_PERI_PWR_CTRL) & ANALOG_I2C_RESET_RELEASE == 0 {
        io.modify(
            pmu::ANA_PERI_PWR_CTRL,
            ANALOG_I2C_RESET_RELEASE,
            ANALOG_I2C_RESET_RELEASE,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{vec, vec::Vec};

    use super::{
        complete_open_i2c_power_and_reset_with, prepare_open_i2c_pre_delay_with, RegisterIo,
        ANALOG_I2C_POWER, ANALOG_I2C_RESET_RELEASE, RF_CIRCUIT_POWER, TIE_HIGH_BB_I2C_POWER,
    };
    use open_esp_radio_pac_esp32s31::{power::pmu, Register32};

    #[derive(Default)]
    struct FakeRegisters {
        values: Vec<(Register32, u32)>,
        writes: Vec<(Register32, u32)>,
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
            self.value(register)
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
    fn pre_delay_clears_only_the_two_evidenced_power_fields() {
        let mut io = FakeRegisters::default()
            .with(pmu::RF_PWC, u32::MAX)
            .with(pmu::IMM_HP_CK_POWER_0, u32::MAX);

        prepare_open_i2c_pre_delay_with(&mut io);

        assert_eq!(io.value(pmu::RF_PWC), u32::MAX & !RF_CIRCUIT_POWER);
        assert_eq!(
            io.value(pmu::IMM_HP_CK_POWER_0),
            u32::MAX & !TIE_HIGH_BB_I2C_POWER
        );
        assert_eq!(io.writes.len(), 2);
    }

    #[test]
    fn powered_down_i2c_gets_the_complete_assert_release_edge() {
        let mut io = FakeRegisters::default();

        complete_open_i2c_power_and_reset_with(&mut io);

        assert_eq!(io.value(pmu::RF_PWC), RF_CIRCUIT_POWER);
        assert_eq!(io.value(pmu::IMM_HP_CK_POWER_0), TIE_HIGH_BB_I2C_POWER);
        assert_eq!(
            io.value(pmu::ANA_PERI_PWR_CTRL) & (ANALOG_I2C_POWER | ANALOG_I2C_RESET_RELEASE),
            ANALOG_I2C_POWER | ANALOG_I2C_RESET_RELEASE
        );
        assert_eq!(
            io.writes
                .iter()
                .filter(|(register, _)| *register == pmu::ANA_PERI_PWR_CTRL)
                .map(|(_, value)| { value & (ANALOG_I2C_POWER | ANALOG_I2C_RESET_RELEASE) })
                .collect::<Vec<_>>(),
            vec![
                ANALOG_I2C_POWER,
                ANALOG_I2C_POWER,
                ANALOG_I2C_POWER | ANALOG_I2C_RESET_RELEASE
            ]
        );
    }

    #[test]
    fn already_powered_i2c_only_releases_reset_when_needed() {
        let mut io =
            FakeRegisters::default().with(pmu::ANA_PERI_PWR_CTRL, ANALOG_I2C_POWER | 0x1234);

        complete_open_i2c_power_and_reset_with(&mut io);

        let analog_writes = io
            .writes
            .iter()
            .filter(|(register, _)| *register == pmu::ANA_PERI_PWR_CTRL)
            .count();
        assert_eq!(analog_writes, 1);
        assert_ne!(
            io.value(pmu::ANA_PERI_PWR_CTRL) & ANALOG_I2C_RESET_RELEASE,
            0
        );
    }
}
