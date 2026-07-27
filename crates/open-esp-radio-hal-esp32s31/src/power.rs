//! Finite ESP32-S31 modem/PHY prerequisite sequence.
//!
//! The order is the merged cold-boot path immediately preceding
//! `register_chipv7_phy` in the ESP32-S31 `esp-radio` and `esp-phy` oracle.
//! Wi-Fi MAC clocks are intentionally excluded: they belong to the later MAC
//! start transition.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;
use open_esp_radio_pac_esp32s31::{
    power::{hp_sys_clkrst, modem_lpcon, modem_syscon, pmu},
    Field32, Register32,
};

const fn field_value(field: Field32, value: u32) -> u32 {
    match field.checked_value(value) {
        Some(value) => value,
        None => panic!("value does not fit recovered register field"),
    }
}

const WIFI_BB_AND_MAC_RESET: u32 = modem_syscon::modem_rst_conf::RST_WIFIBB.mask()
    | modem_syscon::modem_rst_conf::RST_WIFIMAC.mask();
const WIFI_BB_RESET: u32 = modem_syscon::modem_rst_conf::RST_WIFIBB.mask();
const HP_ACTIVE_MODEM_ICG_CODE: u32 =
    field_value(pmu::hp_active_icg_modem::HP_ACTIVE_DIG_ICG_MODEM_CODE, 2);
const PMU_UPDATE_MODEM_ICG: u32 = pmu::imm_modem_icg::UPDATE_DIG_ICG_MODEM_EN.mask();
const PMU_UPDATE_ICG_SWITCH: u32 = pmu::imm_sleep_sysclk::UPDATE_DIG_ICG_SWITCH.mask();
const MODEM_BUS_CLOCK: u32 = hp_sys_clkrst::modem_ctrl0::REG_MODEM_CLK_EN.mask();
const HP_ACTIVE_MODEM_CLOCK_MAP: u32 =
    field_value(modem_syscon::clk_conf_power_st::CLK_ZB_ST_MAP, 4)
        | field_value(modem_syscon::clk_conf_power_st::CLK_FE_ST_MAP, 6)
        | field_value(modem_syscon::clk_conf_power_st::CLK_BT_ST_MAP, 4)
        | field_value(modem_syscon::clk_conf_power_st::CLK_WIFI_ST_MAP, 6)
        | field_value(modem_syscon::clk_conf_power_st::CLK_MODEM_PERI_ST_MAP, 4)
        | field_value(modem_syscon::clk_conf_power_st::CLK_MODEM_APB_ST_MAP, 6);
const HP_MODEM_SHARED_CLOCK_MAP: u32 =
    field_value(modem_lpcon::clk_conf_power_st::CLK_WIFIPWR_ST_MAP, 6)
        | field_value(modem_lpcon::clk_conf_power_st::CLK_COEX_ST_MAP, 6)
        | field_value(modem_lpcon::clk_conf_power_st::CLK_I2C_MST_ST_MAP, 6)
        | field_value(modem_lpcon::clk_conf_power_st::CLK_LP_APB_ST_MAP, 6);
const HP_MODEM_PLL_CONFIGURATION: u32 = hp_sys_clkrst::modem_conf::MODEM_APB_CLK_EN.mask()
    | hp_sys_clkrst::modem_conf::MODEM_CLK_EN.mask()
    | hp_sys_clkrst::modem_conf::MODEM_CLK_SOURCE_SEL.mask()
    | hp_sys_clkrst::modem_conf::MODEM_PLL_CLK_EN.mask()
    | hp_sys_clkrst::modem_conf::MODEM_XTAL_CLK_EN.mask();
const PHY_AND_CALIBRATION_CLOCKS: u32 = modem_syscon::clk_conf1::CLK_WIFIBB_22M_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_40M_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_44M_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_80M_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_40X_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_80X_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_40X1_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_80X1_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_160X1_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFI_APB_EN.mask()
    | modem_syscon::clk_conf1::CLK_FE_80M_EN.mask()
    | modem_syscon::clk_conf1::CLK_FE_160M_EN.mask()
    | modem_syscon::clk_conf1::CLK_FE_APB_EN.mask()
    | modem_syscon::clk_conf1::CLK_BT_APB_EN.mask()
    | modem_syscon::clk_conf1::CLK_BTBB_EN.mask()
    | modem_syscon::clk_conf1::CLK_FE_PWDET_ADC_EN.mask()
    | modem_syscon::clk_conf1::CLK_FE_ADC_EN.mask()
    | modem_syscon::clk_conf1::CLK_FE_DAC_EN.mask();
const I2C_MASTER_SELECT_160M: u32 = modem_syscon::clk_conf::CLK_I2C_MST_SEL_160M.mask();
const I2C_MASTER_CLOCK: u32 = modem_lpcon::clk_conf::CLK_I2C_MST_EN.mask();

/// Evidence family for one cold power operation.
///
/// The SVD supplies register layout; the sequence source independently
/// supplies order and values. This distinction prevents a neighboring-chip
/// field name from being mistaken for S31 execution evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerEvidence {
    /// S31 modem headers plus the pinned S31 `esp-hal` clock implementation.
    S31ModemHeadersAndClockOracle,
    /// S31 SoC SVD/PMU headers plus the pinned S31 `esp-hal` clock implementation.
    S31SocDescriptionAndClockOracle,
}

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

impl PowerOperation {
    /// State which recovered sources justify this register operation.
    ///
    /// MODEM_SYSCON/LPCON layout comes from the pinned S31 headers at
    /// `esp-wifi-sys` commit `2585f278`; HP_SYS_CLKRST/PMU layout comes from
    /// the S31 SVD and PMU headers. Operation ordering and values are from the
    /// pinned S31 `esp-hal` clock path at commit `6899213e`.
    pub const fn evidence(self) -> PowerEvidence {
        let register = match self {
            Self::Modify { register, .. } | Self::Write { register, .. } => register,
        };
        if register.address() == modem_syscon::BASE
            || (register.address() > modem_syscon::BASE
                && register.address() < modem_syscon::BASE + 0x34)
            || register.address() == modem_lpcon::BASE
            || (register.address() > modem_lpcon::BASE
                && register.address() < modem_lpcon::BASE + 0x5c)
        {
            PowerEvidence::S31ModemHeadersAndClockOracle
        } else {
            PowerEvidence::S31SocDescriptionAndClockOracle
        }
    }
}

const POWER_OPERATIONS: [PowerOperation; 14] = [
    PowerOperation::Modify {
        register: modem_syscon::MODEM_RST_CONF,
        mask: WIFI_BB_AND_MAC_RESET,
        value: WIFI_BB_AND_MAC_RESET,
    },
    PowerOperation::Modify {
        register: modem_syscon::MODEM_RST_CONF,
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
        register: hp_sys_clkrst::MODEM_CTRL0,
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
        register: hp_sys_clkrst::MODEM_CONF,
        value: HP_MODEM_PLL_CONFIGURATION,
    },
    PowerOperation::Modify {
        register: modem_syscon::MODEM_RST_CONF,
        mask: WIFI_BB_RESET,
        value: WIFI_BB_RESET,
    },
    PowerOperation::Modify {
        register: modem_syscon::MODEM_RST_CONF,
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
#[cfg(test)]
struct Verification {
    checkpoint: PowerCheckpoint,
    register: Register32,
    mask: u32,
    expected: u32,
}

#[cfg(test)]
const VERIFICATIONS: [Verification; 9] = [
    Verification {
        checkpoint: PowerCheckpoint::ResetReleased,
        register: modem_syscon::MODEM_RST_CONF,
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
        register: hp_sys_clkrst::MODEM_CTRL0,
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
        register: hp_sys_clkrst::MODEM_CONF,
        mask: hp_sys_clkrst::modem_conf::MODEM_APB_CLK_EN.mask()
            | hp_sys_clkrst::modem_conf::MODEM_RST_EN.mask()
            | hp_sys_clkrst::modem_conf::MODEM_CLK_EN.mask()
            | hp_sys_clkrst::modem_conf::MODEM_CLK_SOURCE_SEL.mask()
            | hp_sys_clkrst::modem_conf::MODEM_PLL_CLK_EN.mask()
            | hp_sys_clkrst::modem_conf::MODEM_XTAL_CLK_EN.mask(),
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

#[cfg(test)]
pub(crate) trait RegisterIo {
    fn read(&mut self, register: Register32) -> u32;
    fn write(&mut self, register: Register32, value: u32);
}

#[cfg(test)]
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

#[cfg(target_arch = "riscv32")]
pub(crate) fn execute_owned(registers: &mut RadioRegisters) -> Result<(), PowerError> {
    // Keep the operation order here: it is a lifecycle property recovered
    // from the pinned S31 clock oracle, not a property of the register layout.
    registers.set_wifi_baseband_and_mac_reset(true);
    registers.set_wifi_baseband_and_mac_reset(false);
    registers.select_hp_active_modem_icg();
    registers.apply_modem_icg_selection();
    registers.apply_sleep_icg_selection();
    registers.enable_modem_register_bus_clock();
    registers.configure_hp_active_modem_clock_map();
    registers.configure_shared_modem_clock_map();
    registers.configure_modem_source_clocks();
    registers.set_wifi_baseband_reset(true);
    registers.set_wifi_baseband_reset(false);
    registers.enable_phy_calibration_clocks();
    registers.select_phy_i2c_160mhz_source();
    registers.enable_phy_i2c_master_clock();

    let images = registers.power_clock_images();
    verify_image(
        PowerCheckpoint::ResetReleased,
        modem_syscon::MODEM_RST_CONF,
        WIFI_BB_AND_MAC_RESET,
        0,
        images.modem_reset,
    )?;
    verify_image(
        PowerCheckpoint::HpActiveIcg,
        pmu::HP_ACTIVE_ICG_MODEM,
        3 << 30,
        HP_ACTIVE_MODEM_ICG_CODE,
        images.hp_active_icg,
    )?;
    verify_image(
        PowerCheckpoint::ModemBusClock,
        hp_sys_clkrst::MODEM_CTRL0,
        MODEM_BUS_CLOCK,
        MODEM_BUS_CLOCK,
        images.modem_bus_clock,
    )?;
    verify_image(
        PowerCheckpoint::HpActiveClockMap,
        modem_syscon::CLK_CONF_POWER_ST,
        HP_ACTIVE_MODEM_CLOCK_MAP,
        HP_ACTIVE_MODEM_CLOCK_MAP,
        images.hp_active_clock_map,
    )?;
    verify_image(
        PowerCheckpoint::SharedClockMap,
        modem_lpcon::CLK_CONF_POWER_ST,
        HP_MODEM_SHARED_CLOCK_MAP,
        HP_MODEM_SHARED_CLOCK_MAP,
        images.shared_clock_map,
    )?;
    verify_image(
        PowerCheckpoint::ModemClockSource,
        hp_sys_clkrst::MODEM_CONF,
        hp_sys_clkrst::modem_conf::MODEM_APB_CLK_EN.mask()
            | hp_sys_clkrst::modem_conf::MODEM_RST_EN.mask()
            | hp_sys_clkrst::modem_conf::MODEM_CLK_EN.mask()
            | hp_sys_clkrst::modem_conf::MODEM_CLK_SOURCE_SEL.mask()
            | hp_sys_clkrst::modem_conf::MODEM_PLL_CLK_EN.mask()
            | hp_sys_clkrst::modem_conf::MODEM_XTAL_CLK_EN.mask(),
        HP_MODEM_PLL_CONFIGURATION,
        images.modem_clock_source,
    )?;
    verify_image(
        PowerCheckpoint::PhyClocks,
        modem_syscon::CLK_CONF1,
        PHY_AND_CALIBRATION_CLOCKS,
        PHY_AND_CALIBRATION_CLOCKS,
        images.phy_clocks,
    )?;
    verify_image(
        PowerCheckpoint::I2cSource,
        modem_syscon::CLK_CONF,
        I2C_MASTER_SELECT_160M,
        I2C_MASTER_SELECT_160M,
        images.i2c_source,
    )?;
    verify_image(
        PowerCheckpoint::I2cClock,
        modem_lpcon::CLK_CONF,
        I2C_MASTER_CLOCK,
        I2C_MASTER_CLOCK,
        images.i2c_clock,
    )
}

#[cfg(target_arch = "riscv32")]
fn verify_image(
    checkpoint: PowerCheckpoint,
    register: Register32,
    mask: u32,
    expected: u32,
    image: u32,
) -> Result<(), PowerError> {
    let observed = image & mask;
    if observed == expected {
        Ok(())
    } else {
        Err(PowerError {
            checkpoint,
            address: register.address(),
            mask,
            expected,
            observed,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use open_esp_radio_pac_esp32s31::{power::modem_syscon, Register32};

    use super::{
        execute, PowerCheckpoint, PowerError, PowerEvidence, PowerOperation, PowerSequence,
        RegisterIo, HP_ACTIVE_MODEM_CLOCK_MAP, HP_MODEM_PLL_CONFIGURATION,
        HP_MODEM_SHARED_CLOCK_MAP, PHY_AND_CALIBRATION_CLOCKS, WIFI_BB_AND_MAC_RESET,
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
                register: modem_syscon::MODEM_RST_CONF,
                mask: WIFI_BB_AND_MAC_RESET,
                value: WIFI_BB_AND_MAC_RESET,
            }
        );
        assert_eq!(
            operations[1],
            PowerOperation::Modify {
                register: modem_syscon::MODEM_RST_CONF,
                mask: WIFI_BB_AND_MAC_RESET,
                value: 0,
            }
        );
    }

    #[test]
    fn named_fields_reconstruct_every_qualified_clock_image() {
        assert_eq!(HP_ACTIVE_MODEM_CLOCK_MAP, 0x6464_6400);
        assert_eq!(HP_MODEM_SHARED_CLOCK_MAP, 0x6666_0000);
        assert_eq!(HP_MODEM_PLL_CONFIGURATION, 0x3d);
        assert_eq!(PHY_AND_CALIBRATION_CLOCKS, 0x003b_e5ff);
    }

    #[test]
    fn every_operation_exposes_layout_and_sequence_evidence() {
        let evidence: Vec<_> = PowerSequence::new().map(PowerOperation::evidence).collect();
        assert_eq!(
            evidence
                .iter()
                .filter(|item| **item == PowerEvidence::S31ModemHeadersAndClockOracle)
                .count(),
            9
        );
        assert_eq!(
            evidence
                .iter()
                .filter(|item| **item == PowerEvidence::S31SocDescriptionAndClockOracle)
                .count(),
            5
        );
    }

    #[test]
    fn complete_sequence_preserves_unowned_bits_and_passes_all_checkpoints() {
        let mut registers = FakeRegisters::default();
        registers
            .values
            .push((modem_syscon::MODEM_RST_CONF, 0x55aa_00f0));

        assert_eq!(execute(&mut registers), Ok(()));
        assert_eq!(
            registers.value(modem_syscon::MODEM_RST_CONF) & !WIFI_BB_AND_MAC_RESET,
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
            corrupt_read: Some((modem_syscon::MODEM_RST_CONF, WIFI_BB_AND_MAC_RESET)),
            ..FakeRegisters::default()
        };

        assert_eq!(
            execute(&mut registers),
            Err(PowerError {
                checkpoint: PowerCheckpoint::ResetReleased,
                address: modem_syscon::MODEM_RST_CONF.address(),
                mask: WIFI_BB_AND_MAC_RESET,
                expected: 0,
                observed: WIFI_BB_AND_MAC_RESET,
            })
        );
    }
}
