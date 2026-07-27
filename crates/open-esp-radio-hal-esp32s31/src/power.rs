//! Finite ESP32-S31 modem/PHY prerequisite sequence.
//!
//! The order is the merged cold-boot path immediately preceding
//! `register_chipv7_phy` in the ESP32-S31 `esp-radio` and `esp-phy` oracle.
//! Wi-Fi MAC clocks are intentionally excluded: they belong to the later MAC
//! start transition.

#[cfg(any(test, target_arch = "riscv32"))]
use open_esp_radio_pac_esp32s31::RadioRegisters;
use open_esp_radio_pac_esp32s31::{
    power::{hp_modem, modem_lpcon, modem_syscon, pmu},
    Register32,
};

const WIFI_BB_AND_MAC_RESET: u32 = (1 << 8) | (1 << 9);
const WIFI_BB_RESET: u32 = 1 << 8;
const HP_ACTIVE_MODEM_ICG_CODE: u32 = 2 << 30;
const PMU_UPDATE_MODEM_ICG: u32 = 1 << 31;
const PMU_UPDATE_ICG_SWITCH: u32 = 1 << 28;
const MODEM_BUS_CLOCK: u32 = 1;
const HP_ACTIVE_MODEM_CLOCK_MAP: u32 = 0x6464_6400;
const HP_MODEM_SHARED_CLOCK_MAP: u32 = 0x6666_0000;
const HP_MODEM_PLL_CONFIGURATION: u32 = 0x3d;
const PHY_AND_CALIBRATION_CLOCKS: u32 = 0x003b_e5ff;
const I2C_MASTER_SELECT_160M: u32 = 1 << 12;
const I2C_MASTER_CLOCK: u32 = 1 << 2;

/// One finite register operation in the cold modem/PHY prerequisite path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerOperation {
    /// Replace only the selected bits after reading the current register.
    Modify {
        /// PAC-described register.
        register: Register32,
        /// Bits changed by the operation.
        mask: u32,
        /// New value inside `mask`.
        value: u32,
    },
    /// Publish an exact value to a write-trigger or complete configuration
    /// register.
    Write {
        /// PAC-described register.
        register: Register32,
        /// Exact value.
        value: u32,
    },
}

const POWER_OPERATIONS: [PowerOperation; 14] = [
    PowerOperation::Modify {
        register: modem_syscon::RST_CONF,
        mask: WIFI_BB_AND_MAC_RESET,
        value: WIFI_BB_AND_MAC_RESET,
    },
    PowerOperation::Modify {
        register: modem_syscon::RST_CONF,
        mask: WIFI_BB_AND_MAC_RESET,
        value: 0,
    },
    PowerOperation::Write {
        register: pmu::HP_ACTIVE_ICG_MODEM,
        value: HP_ACTIVE_MODEM_ICG_CODE,
    },
    PowerOperation::Write {
        register: pmu::IMM_MODEM_ICG,
        value: PMU_UPDATE_MODEM_ICG,
    },
    PowerOperation::Write {
        register: pmu::IMM_SLEEP_SYSCLK,
        value: PMU_UPDATE_ICG_SWITCH,
    },
    PowerOperation::Modify {
        register: hp_modem::CTRL0,
        mask: MODEM_BUS_CLOCK,
        value: MODEM_BUS_CLOCK,
    },
    PowerOperation::Modify {
        register: modem_syscon::CLK_CONF_POWER_ST,
        mask: HP_ACTIVE_MODEM_CLOCK_MAP,
        value: HP_ACTIVE_MODEM_CLOCK_MAP,
    },
    PowerOperation::Modify {
        register: modem_lpcon::CLK_CONF_POWER_ST,
        mask: HP_MODEM_SHARED_CLOCK_MAP,
        value: HP_MODEM_SHARED_CLOCK_MAP,
    },
    PowerOperation::Write {
        register: hp_modem::CONF,
        value: HP_MODEM_PLL_CONFIGURATION,
    },
    PowerOperation::Modify {
        register: modem_syscon::RST_CONF,
        mask: WIFI_BB_RESET,
        value: WIFI_BB_RESET,
    },
    PowerOperation::Modify {
        register: modem_syscon::RST_CONF,
        mask: WIFI_BB_RESET,
        value: 0,
    },
    PowerOperation::Modify {
        register: modem_syscon::CLK_CONF1,
        mask: PHY_AND_CALIBRATION_CLOCKS,
        value: PHY_AND_CALIBRATION_CLOCKS,
    },
    PowerOperation::Modify {
        register: modem_syscon::CLK_CONF,
        mask: I2C_MASTER_SELECT_160M,
        value: I2C_MASTER_SELECT_160M,
    },
    PowerOperation::Modify {
        register: modem_lpcon::CLK_CONF,
        mask: I2C_MASTER_CLOCK,
        value: I2C_MASTER_CLOCK,
    },
];

/// Bounded iterator over the exact cold prerequisite operations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PowerSequence {
    next: usize,
}

impl PowerSequence {
    /// Construct a sequence positioned before the modem reset pulse.
    pub const fn new() -> Self {
        Self { next: 0 }
    }

    /// Number of operations in the complete sequence.
    pub const fn len() -> usize {
        POWER_OPERATIONS.len()
    }

    /// The sequence is never empty.
    pub const fn is_empty() -> bool {
        false
    }
}

impl Iterator for PowerSequence {
    type Item = PowerOperation;

    fn next(&mut self) -> Option<Self::Item> {
        let operation = POWER_OPERATIONS.get(self.next).copied();
        if operation.is_some() {
            self.next += 1;
        }
        operation
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = POWER_OPERATIONS.len().saturating_sub(self.next);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for PowerSequence {}

/// Read-back checkpoint following the finite prerequisite sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerCheckpoint {
    /// Both Wi-Fi reset lines were released.
    ResetReleased,
    /// PMU selects the HP-active modem ICG code.
    HpActiveIcg,
    /// The modem register bus clock is enabled.
    ModemBusClock,
    /// HP-active modem domain clocks are ungated.
    HpActiveClockMap,
    /// Shared low-power modem clocks are ungated.
    SharedClockMap,
    /// The modem PLL/XTAL source configuration is active.
    ModemClockSource,
    /// All PHY frontend and calibration clocks are enabled.
    PhyClocks,
    /// PHY-I²C uses the 160 MHz source.
    I2cSource,
    /// The PHY-I²C master clock is enabled.
    I2cClock,
}

#[derive(Clone, Copy)]
#[cfg(any(test, target_arch = "riscv32"))]
struct Verification {
    checkpoint: PowerCheckpoint,
    register: Register32,
    mask: u32,
    expected: u32,
}

#[cfg(any(test, target_arch = "riscv32"))]
const VERIFICATIONS: [Verification; 9] = [
    Verification {
        checkpoint: PowerCheckpoint::ResetReleased,
        register: modem_syscon::RST_CONF,
        mask: WIFI_BB_AND_MAC_RESET,
        expected: 0,
    },
    Verification {
        checkpoint: PowerCheckpoint::HpActiveIcg,
        register: pmu::HP_ACTIVE_ICG_MODEM,
        mask: 3 << 30,
        expected: HP_ACTIVE_MODEM_ICG_CODE,
    },
    Verification {
        checkpoint: PowerCheckpoint::ModemBusClock,
        register: hp_modem::CTRL0,
        mask: MODEM_BUS_CLOCK,
        expected: MODEM_BUS_CLOCK,
    },
    Verification {
        checkpoint: PowerCheckpoint::HpActiveClockMap,
        register: modem_syscon::CLK_CONF_POWER_ST,
        mask: HP_ACTIVE_MODEM_CLOCK_MAP,
        expected: HP_ACTIVE_MODEM_CLOCK_MAP,
    },
    Verification {
        checkpoint: PowerCheckpoint::SharedClockMap,
        register: modem_lpcon::CLK_CONF_POWER_ST,
        mask: HP_MODEM_SHARED_CLOCK_MAP,
        expected: HP_MODEM_SHARED_CLOCK_MAP,
    },
    Verification {
        checkpoint: PowerCheckpoint::ModemClockSource,
        register: hp_modem::CONF,
        mask: 0x3f,
        expected: HP_MODEM_PLL_CONFIGURATION,
    },
    Verification {
        checkpoint: PowerCheckpoint::PhyClocks,
        register: modem_syscon::CLK_CONF1,
        mask: PHY_AND_CALIBRATION_CLOCKS,
        expected: PHY_AND_CALIBRATION_CLOCKS,
    },
    Verification {
        checkpoint: PowerCheckpoint::I2cSource,
        register: modem_syscon::CLK_CONF,
        mask: I2C_MASTER_SELECT_160M,
        expected: I2C_MASTER_SELECT_160M,
    },
    Verification {
        checkpoint: PowerCheckpoint::I2cClock,
        register: modem_lpcon::CLK_CONF,
        mask: I2C_MASTER_CLOCK,
        expected: I2C_MASTER_CLOCK,
    },
];

/// A prerequisite register failed its bounded read-back checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerError {
    /// Semantic checkpoint that failed.
    pub checkpoint: PowerCheckpoint,
    /// Register observed by the checkpoint.
    pub address: usize,
    /// Bits relevant to the checkpoint.
    pub mask: u32,
    /// Expected value after applying `mask`.
    pub expected: u32,
    /// Observed value after applying `mask`.
    pub observed: u32,
}

#[cfg(any(test, target_arch = "riscv32"))]
pub(crate) trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);
}

#[cfg(any(test, target_arch = "riscv32"))]
impl RegisterIo for RadioRegisters {
    fn read(&mut self, register: Register32) -> u32 {
        self.read32(register)
    }

    fn write(&mut self, register: Register32, value: u32) {
        self.write32(register, value);
    }
}

#[cfg(any(test, target_arch = "riscv32"))]
pub(crate) fn execute(io: &mut impl RegisterIo) -> Result<(), PowerError> {
    for operation in PowerSequence::new() {
        match operation {
            PowerOperation::Modify {
                register,
                mask,
                value,
            } => {
                let previous = io.read(register);
                io.write(register, (previous & !mask) | (value & mask));
            }
            PowerOperation::Write { register, value } => io.write(register, value),
        }
    }

    for verification in VERIFICATIONS {
        let observed = io.read(verification.register) & verification.mask;
        if observed != verification.expected {
            return Err(PowerError {
                checkpoint: verification.checkpoint,
                address: verification.register.address(),
                mask: verification.mask,
                expected: verification.expected,
                observed,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use open_esp_radio_pac_esp32s31::{power::modem_syscon, Register32};

    use super::{
        execute, PowerCheckpoint, PowerError, PowerOperation, PowerSequence, RegisterIo,
        HP_ACTIVE_MODEM_CLOCK_MAP, HP_MODEM_SHARED_CLOCK_MAP, PHY_AND_CALIBRATION_CLOCKS,
        WIFI_BB_AND_MAC_RESET,
    };

    #[derive(Default)]
    struct FakeRegisters {
        values: Vec<(Register32, u32)>,
        writes: Vec<(Register32, u32)>,
        corrupt_read: Option<(Register32, u32)>,
    }

    impl FakeRegisters {
        fn value(&self, register: Register32) -> u32 {
            self.values
                .iter()
                .find_map(|(candidate, value)| (*candidate == register).then_some(*value))
                .unwrap_or(0)
        }
    }

    impl RegisterIo for FakeRegisters {
        fn read(&mut self, register: Register32) -> u32 {
            if let Some((corrupt_register, value)) = self.corrupt_read {
                if corrupt_register == register && !self.writes.is_empty() {
                    return value;
                }
            }
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
    fn sequence_is_finite_and_pulses_reset_before_any_clock_write() {
        let operations: Vec<_> = PowerSequence::new().collect();
        assert_eq!(operations.len(), PowerSequence::len());
        assert_eq!(
            operations[0],
            PowerOperation::Modify {
                register: modem_syscon::RST_CONF,
                mask: WIFI_BB_AND_MAC_RESET,
                value: WIFI_BB_AND_MAC_RESET,
            }
        );
        assert_eq!(
            operations[1],
            PowerOperation::Modify {
                register: modem_syscon::RST_CONF,
                mask: WIFI_BB_AND_MAC_RESET,
                value: 0,
            }
        );
    }

    #[test]
    fn complete_sequence_preserves_unowned_bits_and_passes_all_checkpoints() {
        let mut registers = FakeRegisters::default();
        registers.values.push((modem_syscon::RST_CONF, 0x55aa_00f0));

        assert_eq!(execute(&mut registers), Ok(()));
        assert_eq!(
            registers.value(modem_syscon::RST_CONF) & !WIFI_BB_AND_MAC_RESET,
            0x55aa_00f0
        );
        assert_eq!(
            registers
                .values
                .iter()
                .find_map(|(_, value)| ((*value & HP_ACTIVE_MODEM_CLOCK_MAP)
                    == HP_ACTIVE_MODEM_CLOCK_MAP)
                    .then_some(())),
            Some(())
        );
        assert!(registers
            .values
            .iter()
            .any(|(_, value)| *value & HP_MODEM_SHARED_CLOCK_MAP == HP_MODEM_SHARED_CLOCK_MAP));
        assert!(registers
            .values
            .iter()
            .any(|(_, value)| *value & PHY_AND_CALIBRATION_CLOCKS == PHY_AND_CALIBRATION_CLOCKS));
    }

    #[test]
    fn failed_readback_names_the_exact_checkpoint() {
        let mut registers = FakeRegisters {
            corrupt_read: Some((modem_syscon::RST_CONF, WIFI_BB_AND_MAC_RESET)),
            ..FakeRegisters::default()
        };

        assert_eq!(
            execute(&mut registers),
            Err(PowerError {
                checkpoint: PowerCheckpoint::ResetReleased,
                address: modem_syscon::RST_CONF.address(),
                mask: WIFI_BB_AND_MAC_RESET,
                expected: 0,
                observed: WIFI_BB_AND_MAC_RESET,
            })
        );
    }
}
