//! Ownership-bound access to the shared PHY analog-I²C master.

#![forbid(unsafe_code)]

use super::generated::PhyI2cBbpllCalibrationState;
use super::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError, RadioPhyRegisters,
};

pub use super::generated::PhyI2cField;

/// One of the two reviewed analog-register command hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

/// Validated identity of one internal analog PHY-I²C block.
///
/// This is exposed only so ABI bridges can validate a vendor block argument
/// without inventing a register address outside the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cBlock {
    code: u8,
}

impl PhyI2cBlock {
    /// Convert the raw block byte accepted by the vendor ABI.
    pub const fn from_vendor_abi(code: u8) -> Option<Self> {
        if code >= 0x61 && code <= 0x6d {
            Some(Self { code })
        } else {
            None
        }
    }

    const fn recovered(code: u8) -> Self {
        assert!(code >= 0x61 && code <= 0x6d);
        Self { code }
    }

    const fn host(self) -> PhyI2cHost {
        match self.code {
            0x61 | 0x62 | 0x63 | 0x67 | 0x6a | 0x6b => PhyI2cHost::Host1,
            _ => PhyI2cHost::Host0,
        }
    }

    const fn read_mask_complement_low(self) -> u32 {
        match self.code {
            0x61 => 0x00ff_feff,
            0x62 => 0x00ff_ffdf,
            0x63 => 0x00ff_ffef,
            0x66 => 0x00ff_ff7f,
            0x67 => 0x00ff_fffb,
            0x69 => 0x00ff_f7ff,
            0x6a => 0x00ff_ffbf,
            0x6b => 0x00ff_fff7,
            0x6d => 0x00ff_7fff,
            _ => 0x00ff_ffff,
        }
    }
}

/// Validated address of one internal analog PHY-I²C byte register.
///
/// The PAC owns the block-to-host mapping and read-mask encoding. Higher
/// layers may retain this opaque identity while driving an asynchronous
/// transaction, but cannot decompose it into command-register fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cAddress {
    block: PhyI2cBlock,
    register: u8,
}

impl PhyI2cField {
    /// Return the opaque byte-register identity needed for the bus transfer.
    #[inline]
    pub const fn address(self) -> PhyI2cAddress {
        PhyI2cAddress::recovered(self.bank(), self.register())
    }
}

/// One finite analog-register command is still owned by its hardware host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cAccessError {
    Busy,
}

impl PhyI2cAddress {
    const fn recovered(block: u8, register: u8) -> Self {
        Self {
            block: PhyI2cBlock::recovered(block),
            register,
        }
    }

    const fn host(self) -> PhyI2cHost {
        self.block.host()
    }
}

/// Reviewed analog-register identities and fields.
///
/// These are not CPU addresses. Their command-bus geometry is private to the
/// PAC; the names expose only semantics established by the recovered code.
pub mod analog_registers {
    use super::PhyI2cAddress;

    pub use crate::generated::phy_i2c_fields::*;

    pub const XTAL_DUTY_SEED: PhyI2cAddress = PhyI2cAddress::recovered(0x61, 0x09);
    pub const XTAL_DUTY_CANDIDATE: PhyI2cAddress = PhyI2cAddress::recovered(0x61, 0x0a);
    pub const RFPLL_CAPACITOR_LOW: PhyI2cAddress = PhyI2cAddress::recovered(0x62, 0x01);
    pub const RFPLL_CALIBRATED_CAPACITOR_LOW: PhyI2cAddress = PhyI2cAddress::recovered(0x62, 0x05);
    pub const RFPLL_SDM_MOST_SIGNIFICANT_BYTE: PhyI2cAddress = PhyI2cAddress::recovered(0x63, 0x03);
    pub const RFPLL_SDM_UPPER_MIDDLE_BYTE: PhyI2cAddress = PhyI2cAddress::recovered(0x63, 0x04);
    pub const RFPLL_SDM_LOWER_MIDDLE_BYTE: PhyI2cAddress = PhyI2cAddress::recovered(0x63, 0x05);
    pub const TX_CAPACITOR_BANKS: PhyI2cAddress = PhyI2cAddress::recovered(0x6b, 0x02);
}

const PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT: usize = 45;

// Complete command order recovered from
// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`. Values which depend on
// the explicit PHY parameter snapshot are replaced by
// `PhyI2cCommandMemoryInputs::dynamic_values` below.
const PHY_I2C_COMMAND_MEMORY_TEMPLATE: [(u8, u8, u8); PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT] = [
    (0x67, 0x02, 0x07),
    (0x6b, 0x01, 0x01),
    (0x6b, 0x02, 0x73),
    (0x6b, 0x03, 0xba),
    (0x6b, 0x04, 0x88),
    (0x6b, 0x05, 0x01),
    (0x6b, 0x06, 0x11),
    (0x6b, 0x07, 0xfd),
    (0x6b, 0x08, 0xbb),
    (0x6b, 0x09, 0x02),
    (0x6b, 0x0a, 0x08),
    (0x6b, 0x0b, 0x04),
    (0x6b, 0x0c, 0xa7),
    (0x6b, 0x0d, 0x7a),
    (0x6b, 0x0e, 0xf4),
    (0x6b, 0x0f, 0x81),
    (0x62, 0x00, 0x68),
    (0x62, 0x04, 0xa8),
    (0x62, 0x0b, 0x44),
    (0x62, 0x0d, 0x0a),
    (0x62, 0x0f, 0x00),
    (0x62, 0x15, 0x08),
    (0x66, 0x02, 0x70),
    (0x67, 0x02, 0x27),
    (0x67, 0x04, 0x00),
    (0x67, 0x05, 0x00),
    (0x67, 0x06, 0x00),
    (0x67, 0x07, 0x00),
    (0x67, 0x0c, 0x00),
    (0x67, 0x0d, 0x00),
    (0x67, 0x0e, 0x00),
    (0x67, 0x0f, 0x00),
    (0x67, 0x14, 0x00),
    (0x67, 0x15, 0x00),
    (0x67, 0x16, 0x00),
    (0x67, 0x17, 0x00),
    (0x67, 0x18, 0x00),
    (0x67, 0x19, 0x00),
    (0x67, 0x1c, 0x00),
    (0x67, 0x1d, 0x00),
    (0x67, 0x1e, 0x00),
    (0x67, 0x1f, 0x00),
    (0x63, 0x06, 0x00),
    (0x6a, 0x00, 0xaf),
    (0x6a, 0x01, 0x7f),
];

const PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES: [usize; 19] = [
    20, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
];

/// Parameter facts needed to build the complete PAC-owned PHY-I²C command
/// memory. Offset-based names remain explicit because their electrical
/// meaning is not yet established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cCommandMemoryInputs {
    parameter_18e: u8,
    parameter_e9: u8,
    parameter_ea: u8,
    parameter_ed: u8,
    parameter_ee: u8,
    parameter_f0: u8,
}

impl PhyI2cCommandMemoryInputs {
    pub const fn new(
        parameter_18e: u8,
        parameter_e9: u8,
        parameter_ea: u8,
        parameter_ed: u8,
        parameter_ee: u8,
        parameter_f0: u8,
    ) -> Self {
        Self {
            parameter_18e,
            parameter_e9,
            parameter_ea,
            parameter_ed,
            parameter_ee,
            parameter_f0,
        }
    }

    const fn dynamic_values(self) -> [u8; 19] {
        let high_filter = saturate_phy_i2c_value(self.parameter_ed as i32 + 6, 0x3c, 2);
        let low_filter = saturate_phy_i2c_value(self.parameter_ed as i32 - 2, 0x3c, 2);
        let auxiliary = self.parameter_ee.wrapping_add(2);
        [
            self.parameter_18e,
            self.parameter_e9,
            self.parameter_e9,
            self.parameter_ea,
            self.parameter_ea,
            self.parameter_e9,
            self.parameter_e9,
            self.parameter_ea,
            self.parameter_ea,
            high_filter,
            high_filter,
            low_filter,
            self.parameter_ed,
            auxiliary,
            auxiliary,
            self.parameter_f0,
            self.parameter_f0,
            analog_registers::FILTER_DCAP_HIGH_ENABLE_UNKNOWN.replace(self.parameter_f0, 1),
            self.parameter_f0,
        ]
    }
}

const fn saturate_phy_i2c_value(value: i32, upper: u8, lower: u8) -> u8 {
    if value < lower as i32 {
        lower
    } else if value > upper as i32 {
        upper
    } else {
        value as u8
    }
}

const PHY_FILTER_DCAP_COMMAND_COUNT: u8 = 18;
const PHY_I2C_INITIALIZATION_STAGE_ONE_COMMAND_COUNT: u8 = 26;
const PHY_BIAS_REGISTER_COMMAND_COUNT: u8 = 2;
const PHY_BBPLL_CALIBRATION_COMMAND_COUNT: u8 = 2;
const PHY_RC_CALIBRATION_SETTINGS_COMMAND_COUNT: u8 = 3;
const PHY_SAR2_INITIALIZATION_COMMAND_COUNT: u8 = 2;

/// One of the two complete-ROM ADC-rate selections.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyAdcRate {
    Low,
    High,
}

impl PhyAdcRate {
    /// Decode the low selector bit accepted by the complete vendor leaf.
    pub const fn from_vendor_rate(rate: u32) -> Self {
        if rate & 1 == 0 { Self::Low } else { Self::High }
    }

    const fn analog_configuration_field_value(self) -> u8 {
        match self {
            Self::Low => 2,
            Self::High => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyI2cConfigurationCommand {
    Read { block: u8, register: u8 },
    Write { block: u8, register: u8, value: u8 },
    Modify { field: PhyI2cField, value: u8 },
}

impl PhyI2cConfigurationCommand {
    const fn address(self) -> (u8, u8) {
        match self {
            Self::Read { block, register }
            | Self::Write {
                block, register, ..
            } => (block, register),
            Self::Modify { field, .. } => {
                let address = field.address();
                (address.block.code, address.register)
            }
        }
    }
}

const fn bias_register_command(index: u8) -> Option<(u8, u8, u8)> {
    match index {
        0 => Some((0x6a, 0x00, 0xaf)),
        1 => Some((0x6a, 0x01, 0x7f)),
        _ => None,
    }
}

/// Parameter facts consumed by the complete PAC-owned filter-DCAP operation.
///
/// Offset-based names remain explicit because the electrical meaning of these
/// retained vendor parameters is not yet established. Analog-register
/// identities and value encodings remain private to the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFilterDcapInputs {
    parameter_e9: u8,
    parameter_ea: u8,
    parameter_ed: u8,
    parameter_ee: u8,
    parameter_f0: u8,
}

impl PhyFilterDcapInputs {
    pub const fn new(
        parameter_e9: u8,
        parameter_ea: u8,
        parameter_ed: u8,
        parameter_ee: u8,
        parameter_f0: u8,
    ) -> Self {
        Self {
            parameter_e9,
            parameter_ea,
            parameter_ed,
            parameter_ee,
            parameter_f0,
        }
    }

    const fn command(self, index: u8) -> Option<(u8, u8, u8)> {
        let high_filter = saturate_phy_i2c_value(self.parameter_ed as i32 + 6, 0x3c, 2);
        let low_filter = saturate_phy_i2c_value(self.parameter_ed as i32 - 2, 0x3c, 2);
        match index {
            0 => Some((0x67, 0x14, high_filter)),
            1 => Some((0x67, 0x15, high_filter)),
            2 => Some((0x67, 0x16, low_filter)),
            3 => Some((0x67, 0x17, self.parameter_ed)),
            4 => Some((0x67, 0x18, self.parameter_ee)),
            5 => Some((0x67, 0x19, self.parameter_ee)),
            6 => Some((0x67, 0x1c, self.parameter_f0)),
            7 => Some((0x67, 0x1d, self.parameter_f0)),
            8 => Some((
                0x67,
                0x1e,
                analog_registers::FILTER_DCAP_HIGH_ENABLE_UNKNOWN.replace(self.parameter_f0, 1),
            )),
            9 => Some((0x67, 0x1f, self.parameter_f0)),
            10 => Some((0x67, 0x04, self.parameter_e9)),
            11 => Some((0x67, 0x05, self.parameter_e9)),
            12 => Some((0x67, 0x06, self.parameter_ea)),
            13 => Some((0x67, 0x07, self.parameter_ea)),
            14 => Some((0x67, 0x0c, self.parameter_e9)),
            15 => Some((0x67, 0x0d, self.parameter_e9)),
            16 => Some((0x67, 0x0e, self.parameter_ea)),
            17 => Some((0x67, 0x0f, self.parameter_ea)),
            _ => None,
        }
    }
}

/// Dynamic facts consumed by the first complete PHY-I²C initialization stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cInitializationStageOneInputs {
    parameter_18e: u8,
    parameter_ee: u8,
}

impl PhyI2cInitializationStageOneInputs {
    pub const fn new(parameter_18e: u8, parameter_ee: u8) -> Self {
        Self {
            parameter_18e,
            parameter_ee,
        }
    }

    const fn command(self, index: u8) -> Option<(u8, u8, u8)> {
        let parameter_ee_plus_two = self.parameter_ee.wrapping_add(2);
        match index {
            0 => Some((0x6b, 0x01, 0x01)),
            1 => Some((0x6b, 0x02, 0x73)),
            2 => Some((0x6b, 0x03, 0xba)),
            3 => Some((0x6b, 0x04, 0x88)),
            4 => Some((0x6b, 0x0e, 0xf4)),
            5 => Some((0x6b, 0x09, 0x02)),
            6 => Some((0x6b, 0x07, 0xfd)),
            7 => Some((0x6b, 0x08, 0xbb)),
            8 => Some((0x6b, 0x05, 0x01)),
            9 => Some((0x6b, 0x06, 0x11)),
            10 => Some((0x6b, 0x0c, 0xa7)),
            11 => Some((0x6b, 0x0d, 0x7a)),
            12 => Some((0x6b, 0x0a, 0x08)),
            13 => Some((0x6b, 0x0b, 0x04)),
            14 => Some((0x6b, 0x0f, 0x81)),
            15 => Some((0x62, 0x00, 0x68)),
            16 => Some((0x62, 0x04, 0xa8)),
            17 => Some((0x62, 0x0f, self.parameter_18e)),
            18 => Some((0x62, 0x0b, 0x44)),
            19 => Some((0x62, 0x15, 0x08)),
            20 => Some((0x63, 0x06, 0x00)),
            21 => Some((0x62, 0x0d, 0x0a)),
            22 => Some((0x67, 0x02, 0x27)),
            23 => Some((0x66, 0x02, 0x70)),
            24 => Some((0x67, 0x18, parameter_ee_plus_two)),
            25 => Some((0x67, 0x19, parameter_ee_plus_two)),
            _ => None,
        }
    }
}

/// One complete PAC-owned PHY-I²C configuration operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationOperation {
    ConfigureAdcRate(PhyAdcRate),
    BiasRegisters,
    /// Clear the reviewed BBPLL calibration field and consume the vendor's
    /// final readback without exposing its register image.
    EnableBbpllCalibration,
    FilterDcap(PhyFilterDcapInputs),
    InitializationStageOne(PhyI2cInitializationStageOneInputs),
    RcCalibrationSettings,
    Sar2Initialization,
}

impl PhyI2cConfigurationOperation {
    const fn command(self, index: u8) -> Option<PhyI2cConfigurationCommand> {
        let write = match self {
            Self::ConfigureAdcRate(rate) => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify {
                        field: analog_registers::ADC_RATE_CONFIGURATION,
                        value: rate.analog_configuration_field_value(),
                    }),
                    _ => None,
                };
            }
            Self::BiasRegisters => bias_register_command(index),
            Self::EnableBbpllCalibration => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify {
                        field: analog_registers::ADC_RATE_CONFIGURATION,
                        value: 0,
                    }),
                    1 => Some(PhyI2cConfigurationCommand::Read {
                        block: 0x66,
                        register: 0x04,
                    }),
                    _ => None,
                };
            }
            Self::FilterDcap(inputs) => inputs.command(index),
            Self::InitializationStageOne(inputs) => inputs.command(index),
            Self::RcCalibrationSettings => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify {
                        field: analog_registers::RC_CONFIGURATION_0,
                        value: 3,
                    }),
                    1 => Some(PhyI2cConfigurationCommand::Modify {
                        field: analog_registers::RC_CONFIGURATION_1,
                        value: 1,
                    }),
                    2 => Some(PhyI2cConfigurationCommand::Modify {
                        field: analog_registers::RC_CONFIGURATION_2,
                        value: 9,
                    }),
                    _ => None,
                };
            }
            Self::Sar2Initialization => {
                return match index {
                    0 => Some(PhyI2cConfigurationCommand::Modify {
                        field: analog_registers::TEMPERATURE_SENSOR_SAR2_STATUS,
                        value: 0x05,
                    }),
                    1 => Some(PhyI2cConfigurationCommand::Write {
                        block: 0x69,
                        register: 0x03,
                        value: 0x78,
                    }),
                    _ => None,
                };
            }
        };
        match write {
            Some((block, register, value)) => Some(PhyI2cConfigurationCommand::Write {
                block,
                register,
                value,
            }),
            None => None,
        }
    }

    const fn command_count(self) -> u8 {
        match self {
            Self::ConfigureAdcRate(_) => 1,
            Self::BiasRegisters => PHY_BIAS_REGISTER_COMMAND_COUNT,
            Self::EnableBbpllCalibration => PHY_BBPLL_CALIBRATION_COMMAND_COUNT,
            Self::FilterDcap(_) => PHY_FILTER_DCAP_COMMAND_COUNT,
            Self::InitializationStageOne(_) => PHY_I2C_INITIALIZATION_STAGE_ONE_COMMAND_COUNT,
            Self::RcCalibrationSettings => PHY_RC_CALIBRATION_SETTINGS_COMMAND_COUNT,
            Self::Sar2Initialization => PHY_SAR2_INITIALIZATION_COMMAND_COUNT,
        }
    }
}

/// Current externally driven edge of a PAC-owned PHY-I²C configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationAction {
    StartCommand,
    AwaitCompletionEdge,
    Complete,
}

/// One independently delivered configuration-completion edge was consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationObservation {
    StillPending,
    EdgeConsumed,
}

/// A PAC-owned PHY-I²C configuration transaction could not advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cConfigurationError {
    BusyAtStart,
    WrongAction,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyI2cConfigurationPhase {
    Start,
    AwaitRead,
    AwaitWrite,
    Complete,
}

trait PhyI2cConfigurationAccess {
    fn start_read(&mut self, block: u8, register: u8) -> Result<(), ()>;
    fn start_write(&mut self, block: u8, register: u8, value: u8) -> Result<(), ()>;
    fn observe_read(&self) -> Result<u8, ()>;
    fn observe_write(&self) -> Result<(), ()>;
}

/// Non-cloneable owner of one complete recovered PHY-I²C write plan.
///
/// Analog blocks, register identities, derived values and command counts are
/// private PAC implementation details. Callers can only select and drive a
/// finite semantic operation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyI2cConfigurationTransaction {
    operation: PhyI2cConfigurationOperation,
    phase: PhyI2cConfigurationPhase,
    command_index: u8,
    pending_write: Option<u8>,
}

impl PhyI2cConfigurationTransaction {
    pub const fn new(operation: PhyI2cConfigurationOperation) -> Self {
        Self {
            operation,
            phase: PhyI2cConfigurationPhase::Start,
            command_index: 0,
            pending_write: None,
        }
    }

    pub const fn action(&self) -> PhyI2cConfigurationAction {
        match self.phase {
            PhyI2cConfigurationPhase::Start => PhyI2cConfigurationAction::StartCommand,
            PhyI2cConfigurationPhase::AwaitRead | PhyI2cConfigurationPhase::AwaitWrite => {
                PhyI2cConfigurationAction::AwaitCompletionEdge
            }
            PhyI2cConfigurationPhase::Complete => PhyI2cConfigurationAction::Complete,
        }
    }

    pub fn start(
        &mut self,
        registers: &mut RadioPhyRegisters,
    ) -> Result<(), PhyI2cConfigurationError> {
        self.start_with(registers)
    }

    pub fn observe_completion_edge(
        &mut self,
        registers: &mut RadioPhyRegisters,
    ) -> Result<PhyI2cConfigurationObservation, PhyI2cConfigurationError> {
        self.observe_with(registers)
    }

    fn start_with(
        &mut self,
        access: &mut impl PhyI2cConfigurationAccess,
    ) -> Result<(), PhyI2cConfigurationError> {
        match self.phase {
            PhyI2cConfigurationPhase::Complete => {
                return Err(PhyI2cConfigurationError::AlreadyComplete);
            }
            PhyI2cConfigurationPhase::AwaitRead | PhyI2cConfigurationPhase::AwaitWrite => {
                return Err(PhyI2cConfigurationError::WrongAction);
            }
            PhyI2cConfigurationPhase::Start => {}
        }
        let command = self
            .operation
            .command(self.command_index)
            .ok_or(PhyI2cConfigurationError::WrongAction)?;
        let (block, register) = command.address();
        match (command, self.pending_write) {
            (
                PhyI2cConfigurationCommand::Read { .. } | PhyI2cConfigurationCommand::Modify { .. },
                None,
            ) => {
                access
                    .start_read(block, register)
                    .map_err(|()| PhyI2cConfigurationError::BusyAtStart)?;
                self.phase = PhyI2cConfigurationPhase::AwaitRead;
            }
            (PhyI2cConfigurationCommand::Write { value, .. }, None)
            | (PhyI2cConfigurationCommand::Modify { .. }, Some(value)) => {
                access
                    .start_write(block, register, value)
                    .map_err(|()| PhyI2cConfigurationError::BusyAtStart)?;
                self.phase = PhyI2cConfigurationPhase::AwaitWrite;
            }
            (PhyI2cConfigurationCommand::Write { .. }, Some(_)) => {
                return Err(PhyI2cConfigurationError::WrongAction);
            }
            (PhyI2cConfigurationCommand::Read { .. }, Some(_)) => {
                return Err(PhyI2cConfigurationError::WrongAction);
            }
        }
        Ok(())
    }

    fn observe_with(
        &mut self,
        access: &impl PhyI2cConfigurationAccess,
    ) -> Result<PhyI2cConfigurationObservation, PhyI2cConfigurationError> {
        match self.phase {
            PhyI2cConfigurationPhase::Complete => {
                return Err(PhyI2cConfigurationError::AlreadyComplete);
            }
            PhyI2cConfigurationPhase::Start => {
                return Err(PhyI2cConfigurationError::WrongAction);
            }
            PhyI2cConfigurationPhase::AwaitRead => {
                let current = match access.observe_read() {
                    Ok(value) => value,
                    Err(()) => return Ok(PhyI2cConfigurationObservation::StillPending),
                };
                match self.operation.command(self.command_index) {
                    Some(PhyI2cConfigurationCommand::Modify { field, value }) => {
                        self.pending_write = Some(field.replace(current, value));
                        self.phase = PhyI2cConfigurationPhase::Start;
                    }
                    Some(PhyI2cConfigurationCommand::Read { .. }) => {
                        self.command_index += 1;
                        self.phase = if self.command_index == self.operation.command_count() {
                            PhyI2cConfigurationPhase::Complete
                        } else {
                            PhyI2cConfigurationPhase::Start
                        };
                    }
                    _ => return Err(PhyI2cConfigurationError::WrongAction),
                }
                return Ok(PhyI2cConfigurationObservation::EdgeConsumed);
            }
            PhyI2cConfigurationPhase::AwaitWrite => {}
        }
        if access.observe_write().is_err() {
            return Ok(PhyI2cConfigurationObservation::StillPending);
        }
        self.pending_write = None;
        self.command_index += 1;
        self.phase = if self.command_index == self.operation.command_count() {
            PhyI2cConfigurationPhase::Complete
        } else {
            PhyI2cConfigurationPhase::Start
        };
        Ok(PhyI2cConfigurationObservation::EdgeConsumed)
    }
}

/// One complete PAC-owned Bluetooth TX-power analog-control operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlOperation {
    /// Capture the two reviewed source values into the private PAC restore slot.
    PrepareRestore,
    /// Force the four reviewed analog-control registers for calibration.
    ConfigureCalibration,
    /// Restore all four analog-control registers and release the PAC slot.
    Restore,
}

/// Current externally driven edge of a Bluetooth TX-power control transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlAction {
    StartCommand,
    AwaitCompletionEdge,
    Complete(BluetoothTxPowerControlCompletion),
}

/// Semantic result of a complete Bluetooth TX-power control transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlCompletion {
    RestorePrepared,
    CalibrationConfigured,
    Restored,
}

/// One independently delivered analog-I²C completion edge was consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlObservation {
    StillPending,
    EdgeConsumed,
}

/// A PAC-owned Bluetooth TX-power control transaction could not advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlError {
    BusyAtStart,
    RestorePending,
    RestoreNotPending,
    WrongAction,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothTxPowerControlRegister {
    Low0,
    Low1,
    High0,
    High1,
}

impl BluetoothTxPowerControlRegister {
    const fn address(self) -> u8 {
        match self {
            Self::Low0 => 0x1c,
            Self::Low1 => 0x1d,
            Self::High0 => 0x1e,
            Self::High1 => 0x1f,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothTxPowerControlPhase {
    Start,
    Await,
    Complete,
}

trait BluetoothTxPowerControlI2cAccess {
    fn reserve_restore(&mut self) -> Result<(), BluetoothTxPowerControlPrepareError>;
    fn capture_low(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError>;
    fn capture_high(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError>;
    fn restore_values(&self) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError>;
    fn finish_restore(&mut self) -> Result<(), BluetoothTxPowerControlRestoreError>;
    fn start_read(&mut self, register: BluetoothTxPowerControlRegister) -> Result<(), ()>;
    fn start_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), ()>;
    fn observe_read(&self) -> Result<u8, ()>;
    fn observe_write(&self) -> Result<(), ()>;
}

/// Non-cloneable transaction which keeps all four analog-register identities,
/// field geometry and saved values inside the PAC.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothTxPowerControlTransaction {
    operation: BluetoothTxPowerControlOperation,
    phase: BluetoothTxPowerControlPhase,
    step: u8,
    scratch: u8,
    restore_reserved: bool,
}

impl BluetoothTxPowerControlTransaction {
    pub const fn new(operation: BluetoothTxPowerControlOperation) -> Self {
        Self {
            operation,
            phase: BluetoothTxPowerControlPhase::Start,
            step: 0,
            scratch: 0,
            restore_reserved: false,
        }
    }

    pub const fn action(&self) -> BluetoothTxPowerControlAction {
        match self.phase {
            BluetoothTxPowerControlPhase::Start => BluetoothTxPowerControlAction::StartCommand,
            BluetoothTxPowerControlPhase::Await => {
                BluetoothTxPowerControlAction::AwaitCompletionEdge
            }
            BluetoothTxPowerControlPhase::Complete => {
                BluetoothTxPowerControlAction::Complete(match self.operation {
                    BluetoothTxPowerControlOperation::PrepareRestore => {
                        BluetoothTxPowerControlCompletion::RestorePrepared
                    }
                    BluetoothTxPowerControlOperation::ConfigureCalibration => {
                        BluetoothTxPowerControlCompletion::CalibrationConfigured
                    }
                    BluetoothTxPowerControlOperation::Restore => {
                        BluetoothTxPowerControlCompletion::Restored
                    }
                })
            }
        }
    }

    pub fn start(
        &mut self,
        registers: &mut RadioPhyRegisters,
    ) -> Result<(), BluetoothTxPowerControlError> {
        self.start_with(registers)
    }

    pub fn observe_completion_edge(
        &mut self,
        registers: &mut RadioPhyRegisters,
    ) -> Result<BluetoothTxPowerControlObservation, BluetoothTxPowerControlError> {
        self.observe_with(registers)
    }

    fn start_with(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<(), BluetoothTxPowerControlError> {
        match self.phase {
            BluetoothTxPowerControlPhase::Complete => {
                return Err(BluetoothTxPowerControlError::AlreadyComplete);
            }
            BluetoothTxPowerControlPhase::Await => {
                return Err(BluetoothTxPowerControlError::WrongAction);
            }
            BluetoothTxPowerControlPhase::Start => {}
        }
        let start_result = match self.operation {
            BluetoothTxPowerControlOperation::PrepareRestore => self.start_prepare(access)?,
            BluetoothTxPowerControlOperation::ConfigureCalibration
            | BluetoothTxPowerControlOperation::Restore => self.start_write_plan(access)?,
        };
        if start_result.is_err() {
            return Err(BluetoothTxPowerControlError::BusyAtStart);
        }
        self.phase = BluetoothTxPowerControlPhase::Await;
        Ok(())
    }

    fn observe_with(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<BluetoothTxPowerControlObservation, BluetoothTxPowerControlError> {
        match self.phase {
            BluetoothTxPowerControlPhase::Complete => {
                return Err(BluetoothTxPowerControlError::AlreadyComplete);
            }
            BluetoothTxPowerControlPhase::Start => {
                return Err(BluetoothTxPowerControlError::WrongAction);
            }
            BluetoothTxPowerControlPhase::Await => {}
        }
        let observation = match self.operation {
            BluetoothTxPowerControlOperation::PrepareRestore => self.observe_prepare(access)?,
            BluetoothTxPowerControlOperation::ConfigureCalibration
            | BluetoothTxPowerControlOperation::Restore => self.observe_write_plan(access)?,
        };
        let Some(complete) = observation else {
            return Ok(BluetoothTxPowerControlObservation::StillPending);
        };
        if complete {
            self.phase = BluetoothTxPowerControlPhase::Complete;
        } else {
            self.step += 1;
            self.phase = BluetoothTxPowerControlPhase::Start;
        }
        Ok(BluetoothTxPowerControlObservation::EdgeConsumed)
    }

    fn start_prepare(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<Result<(), ()>, BluetoothTxPowerControlError> {
        if self.step == 0 && !self.restore_reserved {
            access.reserve_restore().map_err(|error| match error {
                BluetoothTxPowerControlPrepareError::RestorePending => {
                    BluetoothTxPowerControlError::RestorePending
                }
            })?;
            self.restore_reserved = true;
        }
        Ok(access.start_read(if self.step == 0 {
            BluetoothTxPowerControlRegister::Low0
        } else {
            BluetoothTxPowerControlRegister::High0
        }))
    }

    fn start_write_plan(
        &self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<Result<(), ()>, BluetoothTxPowerControlError> {
        self.require_restore(access)?;
        Ok(match self.step {
            0 => access.start_write(
                BluetoothTxPowerControlRegister::Low0,
                self.low_value(access)?,
            ),
            1 => access.start_write(
                BluetoothTxPowerControlRegister::Low1,
                self.low_value(access)?,
            ),
            2 => access.start_read(BluetoothTxPowerControlRegister::High0),
            3 => access.start_write(
                BluetoothTxPowerControlRegister::High0,
                analog_registers::BLUETOOTH_TX_POWER_HIGH_0
                    .replace(self.scratch, self.high_value(access)?),
            ),
            4 => access.start_read(BluetoothTxPowerControlRegister::High1),
            5 => access.start_write(
                BluetoothTxPowerControlRegister::High1,
                analog_registers::BLUETOOTH_TX_POWER_HIGH_1
                    .replace(self.scratch, self.high_value(access)?),
            ),
            _ => return Err(BluetoothTxPowerControlError::WrongAction),
        })
    }

    fn observe_prepare(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<Option<bool>, BluetoothTxPowerControlError> {
        let value = match access.observe_read() {
            Ok(value) => value,
            Err(()) => return Ok(None),
        };
        if self.step == 0 {
            access.capture_low(value).map_err(map_restore_error)?;
            Ok(Some(false))
        } else {
            access
                .capture_high(analog_registers::BLUETOOTH_TX_POWER_HIGH_0.extract(value))
                .map_err(map_restore_error)?;
            Ok(Some(true))
        }
    }

    fn observe_write_plan(
        &mut self,
        access: &mut impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<Option<bool>, BluetoothTxPowerControlError> {
        match self.step {
            2 | 4 => {
                self.scratch = match access.observe_read() {
                    Ok(value) => value,
                    Err(()) => return Ok(None),
                };
            }
            _ => {
                if access.observe_write().is_err() {
                    return Ok(None);
                }
            }
        }
        if self.step != 5 {
            return Ok(Some(false));
        }
        if self.operation == BluetoothTxPowerControlOperation::Restore {
            access.finish_restore().map_err(map_restore_error)?;
        }
        Ok(Some(true))
    }

    fn require_restore(
        &self,
        access: &impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<(), BluetoothTxPowerControlError> {
        access
            .restore_values()
            .map(|_| ())
            .map_err(map_restore_error)
    }

    fn low_value(
        &self,
        access: &impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<u8, BluetoothTxPowerControlError> {
        match self.operation {
            BluetoothTxPowerControlOperation::ConfigureCalibration => {
                self.require_restore(access)?;
                Ok(2)
            }
            BluetoothTxPowerControlOperation::Restore => access
                .restore_values()
                .map(|values| values.0)
                .map_err(map_restore_error),
            BluetoothTxPowerControlOperation::PrepareRestore => {
                Err(BluetoothTxPowerControlError::WrongAction)
            }
        }
    }

    fn high_value(
        &self,
        access: &impl BluetoothTxPowerControlI2cAccess,
    ) -> Result<u8, BluetoothTxPowerControlError> {
        match self.operation {
            BluetoothTxPowerControlOperation::ConfigureCalibration => {
                self.require_restore(access)?;
                Ok(2)
            }
            BluetoothTxPowerControlOperation::Restore => access
                .restore_values()
                .map(|values| values.1)
                .map_err(map_restore_error),
            BluetoothTxPowerControlOperation::PrepareRestore => {
                Err(BluetoothTxPowerControlError::WrongAction)
            }
        }
    }
}

const fn map_restore_error(
    error: BluetoothTxPowerControlRestoreError,
) -> BluetoothTxPowerControlError {
    match error {
        BluetoothTxPowerControlRestoreError::RestoreNotPending => {
            BluetoothTxPowerControlError::RestoreNotPending
        }
    }
}

impl RadioPhyRegisters {
    /// Configure the reviewed host map and select the typed host for one
    /// opaque analog-register identity.
    pub fn configure_and_select_phy_i2c_host(&mut self, block: PhyI2cBlock) -> PhyI2cHost {
        self.configure_phy_i2c_host_map();
        block.host()
    }

    /// Start one opaque analog-register read without exposing command fields.
    pub fn try_start_phy_i2c_read(
        &mut self,
        address: PhyI2cAddress,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.configure_and_select_phy_i2c_host(address.block);
        if self.phy_i2c_master_is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_read_mask(address.block);
        self.publish_phy_i2c_command(host, address.block.code, address.register, 0, false);
        Ok(())
    }

    /// Consume one externally delivered completion edge for an opaque read.
    pub fn try_finish_phy_i2c_read(&self, address: PhyI2cAddress) -> Result<u8, PhyI2cAccessError> {
        let host = address.host();
        if self.phy_i2c_master_is_busy(host) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(self.sample_phy_i2c_result(host))
        }
    }

    /// Start one opaque analog-register write without exposing command fields.
    pub fn try_start_phy_i2c_write(
        &mut self,
        address: PhyI2cAddress,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.configure_and_select_phy_i2c_host(address.block);
        if self.phy_i2c_master_is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish_phy_i2c_command(host, address.block.code, address.register, value, true);
        Ok(())
    }

    /// Consume one externally delivered completion edge for an opaque write.
    pub fn try_finish_phy_i2c_write(
        &self,
        address: PhyI2cAddress,
    ) -> Result<(), PhyI2cAccessError> {
        if self.phy_i2c_master_is_busy(address.host()) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(())
        }
    }

    /// Install the complete reviewed PHY-I²C host map with one fresh RMW.
    fn configure_phy_i2c_host_map(&mut self) {
        crate::generated::configure_phy_i2c_host_map(&self.peripherals.i2c_ana_mst);
    }

    /// Publish the finite reset command for one analog-I²C host.
    pub fn pulse_phy_i2c_master_reset(&mut self, host: PhyI2cHost) {
        match host {
            PhyI2cHost::Host0 => super::svd::fixed_register_image::pulse_phy_i2c_host0_reset(
                &self.peripherals.i2c_ana_mst,
            ),
            PhyI2cHost::Host1 => super::svd::fixed_register_image::pulse_phy_i2c_host1_reset(
                &self.peripherals.i2c_ana_mst,
            ),
        }
    }

    /// Sample the reviewed completion predicate for one host.
    pub fn phy_i2c_master_is_busy(&self, host: PhyI2cHost) -> bool {
        match host {
            PhyI2cHost::Host0 => {
                super::svd::field_read::observe_phy_i2c_host0_busy(&self.peripherals.i2c_ana_mst)
            }
            PhyI2cHost::Host1 => {
                super::svd::field_read::observe_phy_i2c_host1_busy(&self.peripherals.i2c_ana_mst)
            }
        }
    }

    /// Publish the complete complemented read mask used by the vendor leaf.
    fn publish_phy_i2c_read_mask(&mut self, block: PhyI2cBlock) {
        super::svd::zero_based_field_write::publish_phy_i2c_read_mask(
            &self.peripherals.i2c_ana_mst,
            block.read_mask_complement_low(),
            0xff,
        );
    }

    /// Publish one complete host command in the reviewed vendor order.
    fn publish_phy_i2c_command(
        &mut self,
        host: PhyI2cHost,
        block: u8,
        register: u8,
        value: u8,
        write: bool,
    ) {
        match host {
            PhyI2cHost::Host0 => super::svd::zero_based_field_write::publish_phy_i2c_host0_command(
                &self.peripherals.i2c_ana_mst,
                block,
                register,
                value,
                write,
                true,
            ),
            PhyI2cHost::Host1 => super::svd::zero_based_field_write::publish_phy_i2c_host1_command(
                &self.peripherals.i2c_ana_mst,
                block,
                register,
                value,
                write,
                true,
            ),
        }
    }

    /// Sample the completed data byte from one host.
    fn sample_phy_i2c_result(&self, host: PhyI2cHost) -> u8 {
        match host {
            PhyI2cHost::Host0 => {
                super::svd::field_read::observe_phy_i2c_host0_data(&self.peripherals.i2c_ana_mst)
            }
            PhyI2cHost::Host1 => {
                super::svd::field_read::observe_phy_i2c_host1_data(&self.peripherals.i2c_ana_mst)
            }
        }
    }

    /// Apply all six timing RMWs in the complete vendor order.
    pub fn configure_phy_i2c_clock_selection(&mut self) {
        let registers = &self.peripherals.i2c_ana_mst;

        crate::generated::configure_phy_i2c_host0_sda_guard(registers);
        crate::generated::configure_phy_i2c_host0_scl_duration(registers);
        crate::generated::configure_phy_i2c_host1_sda_guard(registers);
        crate::generated::configure_phy_i2c_host1_scl_duration(registers);
        crate::generated::configure_phy_i2c_hardware_sda_guard(registers);
        crate::generated::configure_phy_i2c_hardware_scl_duration(registers);
    }

    /// Select register mode two, then enable it with a separate fresh RMW.
    pub fn configure_phy_i2c_master_registers(&mut self) {
        let registers = &self.peripherals.i2c_ana_mst;
        crate::generated::select_phy_i2c_register_mode(registers);
        crate::generated::enable_phy_i2c_register_access(registers);
    }

    /// Select one of the two complete-ROM BBPLL calibration encodings.
    pub fn set_phy_i2c_bbpll_calibration(&mut self, enabled: bool) {
        let state = if enabled {
            PhyI2cBbpllCalibrationState::Enabled
        } else {
            PhyI2cBbpllCalibrationState::Disabled
        };
        crate::generated::set_phy_i2c_bbpll_calibration(&self.peripherals.i2c_ana_mst, state);
    }

    /// Program the complete reviewed 45-entry PHY-I²C command memory.
    ///
    /// The PAC owns every destination block, byte-register, command index and
    /// fixed or derived register image. Callers provide only the six retained
    /// vendor-parameter facts.
    pub fn configure_phy_i2c_command_memory(&mut self, inputs: PhyI2cCommandMemoryInputs) {
        let dynamic_values = inputs.dynamic_values();
        let mut index = 0;
        let mut dynamic_cursor = 0;
        while index != PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT {
            let (block, register, fixed_value) = PHY_I2C_COMMAND_MEMORY_TEMPLATE[index];
            let value = if dynamic_cursor != PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES.len()
                && PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES[dynamic_cursor] == index
            {
                let value = dynamic_values[dynamic_cursor];
                dynamic_cursor += 1;
                value
            } else {
                fixed_value
            };
            super::svd::zero_based_field_write::phy_i2c_command_memory(
                &self.peripherals.phy_i2c_command_ram,
                index,
                block,
                register,
                value,
            );
            index += 1;
        }
    }
}

impl PhyI2cConfigurationAccess for RadioPhyRegisters {
    fn start_read(&mut self, block: u8, register: u8) -> Result<(), ()> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            return Err(());
        }
        self.publish_phy_i2c_command(PhyI2cHost::Host1, block, register, 0, false);
        Ok(())
    }

    fn start_write(&mut self, block: u8, register: u8, value: u8) -> Result<(), ()> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            return Err(());
        }
        self.publish_phy_i2c_command(PhyI2cHost::Host1, block, register, value, true);
        Ok(())
    }

    fn observe_read(&self) -> Result<u8, ()> {
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            Err(())
        } else {
            Ok(self.sample_phy_i2c_result(PhyI2cHost::Host1))
        }
    }

    fn observe_write(&self) -> Result<(), ()> {
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            Err(())
        } else {
            Ok(())
        }
    }
}

impl BluetoothTxPowerControlI2cAccess for RadioPhyRegisters {
    fn reserve_restore(&mut self) -> Result<(), BluetoothTxPowerControlPrepareError> {
        self.restore_slot.prepare_bluetooth_tx_power_control()
    }

    fn capture_low(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore_slot
            .capture_bluetooth_tx_power_control_low(value)
    }

    fn capture_high(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore_slot
            .capture_bluetooth_tx_power_control_high(value)
    }

    fn restore_values(&self) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError> {
        self.restore_slot.bluetooth_tx_power_control_values()
    }

    fn finish_restore(&mut self) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore_slot
            .finish_bluetooth_tx_power_control_restore()
    }

    fn start_read(&mut self, register: BluetoothTxPowerControlRegister) -> Result<(), ()> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            return Err(());
        }
        self.publish_phy_i2c_read_mask(PhyI2cBlock::recovered(0x67));
        self.publish_phy_i2c_command(PhyI2cHost::Host1, 0x67, register.address(), 0, false);
        Ok(())
    }

    fn start_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), ()> {
        self.configure_phy_i2c_host_map();
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            return Err(());
        }
        self.publish_phy_i2c_command(PhyI2cHost::Host1, 0x67, register.address(), value, true);
        Ok(())
    }

    fn observe_read(&self) -> Result<u8, ()> {
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            Err(())
        } else {
            Ok(self.sample_phy_i2c_result(PhyI2cHost::Host1))
        }
    }

    fn observe_write(&self) -> Result<(), ()> {
        if self.phy_i2c_master_is_busy(PhyI2cHost::Host1) {
            Err(())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, vec::Vec};

    use super::{
        BluetoothTxPowerControlAction, BluetoothTxPowerControlCompletion,
        BluetoothTxPowerControlError, BluetoothTxPowerControlI2cAccess,
        BluetoothTxPowerControlObservation, BluetoothTxPowerControlOperation,
        BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRegister,
        BluetoothTxPowerControlRestoreError, BluetoothTxPowerControlTransaction, PhyAdcRate,
        PhyFilterDcapInputs, PhyI2cConfigurationAccess, PhyI2cConfigurationAction,
        PhyI2cConfigurationError, PhyI2cConfigurationObservation, PhyI2cConfigurationOperation,
        PhyI2cConfigurationTransaction, PhyI2cInitializationStageOneInputs,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Read(BluetoothTxPowerControlRegister),
        Write(BluetoothTxPowerControlRegister, u8),
    }

    struct FakeI2c {
        restore: Option<(u8, u8)>,
        reads: RefCell<VecDeque<u8>>,
        operations: Vec<Operation>,
        busy_starts: u8,
    }

    impl FakeI2c {
        fn new(reads: impl IntoIterator<Item = u8>) -> Self {
            Self {
                restore: None,
                reads: RefCell::new(reads.into_iter().collect()),
                operations: Vec::new(),
                busy_starts: 0,
            }
        }

        fn accept_start(&mut self) -> Result<(), ()> {
            if self.busy_starts == 0 {
                Ok(())
            } else {
                self.busy_starts -= 1;
                Err(())
            }
        }
    }

    impl BluetoothTxPowerControlI2cAccess for FakeI2c {
        fn reserve_restore(&mut self) -> Result<(), BluetoothTxPowerControlPrepareError> {
            if self.restore.is_some() {
                Err(BluetoothTxPowerControlPrepareError::RestorePending)
            } else {
                self.restore = Some((0, 0));
                Ok(())
            }
        }

        fn capture_low(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
            let Some((_, high)) = self.restore else {
                return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
            };
            self.restore = Some((value, high));
            Ok(())
        }

        fn capture_high(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
            let Some((low, _)) = self.restore else {
                return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
            };
            self.restore = Some((low, value));
            Ok(())
        }

        fn restore_values(&self) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError> {
            self.restore
                .ok_or(BluetoothTxPowerControlRestoreError::RestoreNotPending)
        }

        fn finish_restore(&mut self) -> Result<(), BluetoothTxPowerControlRestoreError> {
            self.restore_values()?;
            self.restore = None;
            Ok(())
        }

        fn start_read(&mut self, register: BluetoothTxPowerControlRegister) -> Result<(), ()> {
            self.accept_start()?;
            self.operations.push(Operation::Read(register));
            Ok(())
        }

        fn start_write(
            &mut self,
            register: BluetoothTxPowerControlRegister,
            value: u8,
        ) -> Result<(), ()> {
            self.accept_start()?;
            self.operations.push(Operation::Write(register, value));
            Ok(())
        }

        fn observe_read(&self) -> Result<u8, ()> {
            self.reads.borrow_mut().pop_front().ok_or(())
        }

        fn observe_write(&self) -> Result<(), ()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeConfigurationI2c {
        accepted_commands: u8,
        busy_starts: u8,
        pending_observations: u8,
    }

    impl PhyI2cConfigurationAccess for FakeConfigurationI2c {
        fn start_read(&mut self, _block: u8, _register: u8) -> Result<(), ()> {
            if self.busy_starts != 0 {
                self.busy_starts -= 1;
                return Err(());
            }
            self.accepted_commands += 1;
            Ok(())
        }

        fn start_write(&mut self, _block: u8, _register: u8, _value: u8) -> Result<(), ()> {
            if self.busy_starts != 0 {
                self.busy_starts -= 1;
                return Err(());
            }
            self.accepted_commands += 1;
            Ok(())
        }

        fn observe_read(&self) -> Result<u8, ()> {
            if self.pending_observations == 0 {
                Ok(0xa0)
            } else {
                Err(())
            }
        }

        fn observe_write(&self) -> Result<(), ()> {
            if self.pending_observations == 0 {
                Ok(())
            } else {
                Err(())
            }
        }
    }

    fn drive_configuration(
        transaction: &mut PhyI2cConfigurationTransaction,
        access: &mut FakeConfigurationI2c,
    ) {
        for _ in 0..64 {
            match transaction.action() {
                PhyI2cConfigurationAction::StartCommand => {
                    transaction.start_with(access).unwrap();
                }
                PhyI2cConfigurationAction::AwaitCompletionEdge => {
                    assert_eq!(
                        transaction.observe_with(access).unwrap(),
                        PhyI2cConfigurationObservation::EdgeConsumed
                    );
                }
                PhyI2cConfigurationAction::Complete => return,
            }
        }
        panic!("PHY-I2C configuration exceeded its finite plan")
    }

    #[test]
    fn configuration_operations_are_finite_and_retry_a_busy_start() {
        let mut access = FakeConfigurationI2c {
            busy_starts: 1,
            ..FakeConfigurationI2c::default()
        };
        let mut bias =
            PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::BiasRegisters);

        assert_eq!(
            bias.start_with(&mut access),
            Err(PhyI2cConfigurationError::BusyAtStart)
        );
        assert_eq!(bias.action(), PhyI2cConfigurationAction::StartCommand);
        drive_configuration(&mut bias, &mut access);
        assert_eq!(bias.action(), PhyI2cConfigurationAction::Complete);
        assert_eq!(access.accepted_commands, 2);

        let mut adc_rate = PhyI2cConfigurationTransaction::new(
            PhyI2cConfigurationOperation::ConfigureAdcRate(PhyAdcRate::High),
        );
        drive_configuration(&mut adc_rate, &mut access);
        assert_eq!(adc_rate.action(), PhyI2cConfigurationAction::Complete);

        let mut bbpll = PhyI2cConfigurationTransaction::new(
            PhyI2cConfigurationOperation::EnableBbpllCalibration,
        );
        drive_configuration(&mut bbpll, &mut access);
        assert_eq!(bbpll.action(), PhyI2cConfigurationAction::Complete);

        access.accepted_commands = 0;
        let mut filter_dcap =
            PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::FilterDcap(
                PhyFilterDcapInputs::new(0x12, 0x34, 0x3a, 0x56, 0x87),
            ));

        drive_configuration(&mut filter_dcap, &mut access);
        assert_eq!(filter_dcap.action(), PhyI2cConfigurationAction::Complete);
        assert_eq!(access.accepted_commands, 18);
        assert_eq!(
            filter_dcap.observe_with(&access),
            Err(PhyI2cConfigurationError::AlreadyComplete)
        );

        access.accepted_commands = 0;
        let mut initialization = PhyI2cConfigurationTransaction::new(
            PhyI2cConfigurationOperation::InitializationStageOne(
                PhyI2cInitializationStageOneInputs::new(0x55, 0xfe),
            ),
        );
        drive_configuration(&mut initialization, &mut access);
        assert_eq!(initialization.action(), PhyI2cConfigurationAction::Complete);
        assert_eq!(access.accepted_commands, 26);

        access.accepted_commands = 0;
        let mut sar2 =
            PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::Sar2Initialization);
        access.busy_starts = 1;
        assert_eq!(
            sar2.start_with(&mut access),
            Err(PhyI2cConfigurationError::BusyAtStart)
        );
        sar2.start_with(&mut access).unwrap();
        access.pending_observations = 1;
        assert_eq!(
            sar2.observe_with(&access),
            Ok(PhyI2cConfigurationObservation::StillPending)
        );
        assert_eq!(
            sar2.action(),
            PhyI2cConfigurationAction::AwaitCompletionEdge
        );
        access.pending_observations = 0;
        drive_configuration(&mut sar2, &mut access);
        assert_eq!(sar2.action(), PhyI2cConfigurationAction::Complete);
        assert_eq!(access.accepted_commands, 3);

        access.accepted_commands = 0;
        let mut rc_calibration = PhyI2cConfigurationTransaction::new(
            PhyI2cConfigurationOperation::RcCalibrationSettings,
        );
        drive_configuration(&mut rc_calibration, &mut access);
        assert_eq!(rc_calibration.action(), PhyI2cConfigurationAction::Complete);
        assert_eq!(access.accepted_commands, 6);
    }

    fn drive(
        transaction: &mut BluetoothTxPowerControlTransaction,
        access: &mut FakeI2c,
    ) -> BluetoothTxPowerControlCompletion {
        for _ in 0..32 {
            match transaction.action() {
                BluetoothTxPowerControlAction::StartCommand => {
                    transaction.start_with(access).unwrap();
                }
                BluetoothTxPowerControlAction::AwaitCompletionEdge => {
                    assert_eq!(
                        transaction.observe_with(access).unwrap(),
                        BluetoothTxPowerControlObservation::EdgeConsumed
                    );
                }
                BluetoothTxPowerControlAction::Complete(completion) => return completion,
            }
        }
        panic!("Bluetooth TX-power control transaction exceeded its finite plan")
    }

    #[test]
    fn prepare_configure_and_restore_keep_analog_geometry_and_snapshot_in_pac() {
        let mut access = FakeI2c::new([0xa5, 0xd5]);
        let mut prepare = BluetoothTxPowerControlTransaction::new(
            BluetoothTxPowerControlOperation::PrepareRestore,
        );
        assert_eq!(
            drive(&mut prepare, &mut access),
            BluetoothTxPowerControlCompletion::RestorePrepared
        );
        assert_eq!(access.restore, Some((0xa5, 0x15)));

        access.operations.clear();
        *access.reads.borrow_mut() = [0xc0, 0x80].into_iter().collect();
        let mut configure = BluetoothTxPowerControlTransaction::new(
            BluetoothTxPowerControlOperation::ConfigureCalibration,
        );
        assert_eq!(
            drive(&mut configure, &mut access),
            BluetoothTxPowerControlCompletion::CalibrationConfigured
        );
        assert_eq!(
            access.operations,
            [
                Operation::Write(BluetoothTxPowerControlRegister::Low0, 2),
                Operation::Write(BluetoothTxPowerControlRegister::Low1, 2),
                Operation::Read(BluetoothTxPowerControlRegister::High0),
                Operation::Write(BluetoothTxPowerControlRegister::High0, 0xc2),
                Operation::Read(BluetoothTxPowerControlRegister::High1),
                Operation::Write(BluetoothTxPowerControlRegister::High1, 0x82),
            ]
        );

        access.operations.clear();
        *access.reads.borrow_mut() = [0xc0, 0x80].into_iter().collect();
        let mut restore =
            BluetoothTxPowerControlTransaction::new(BluetoothTxPowerControlOperation::Restore);
        assert_eq!(
            drive(&mut restore, &mut access),
            BluetoothTxPowerControlCompletion::Restored
        );
        assert_eq!(
            access.operations,
            [
                Operation::Write(BluetoothTxPowerControlRegister::Low0, 0xa5),
                Operation::Write(BluetoothTxPowerControlRegister::Low1, 0xa5),
                Operation::Read(BluetoothTxPowerControlRegister::High0),
                Operation::Write(BluetoothTxPowerControlRegister::High0, 0xd5),
                Operation::Read(BluetoothTxPowerControlRegister::High1),
                Operation::Write(BluetoothTxPowerControlRegister::High1, 0x95),
            ]
        );
        assert_eq!(access.restore, None);
    }

    #[test]
    fn prepare_rejects_an_owned_restore_before_starting_i2c() {
        let mut access = FakeI2c::new([]);
        access.restore = Some((0, 0));
        let mut transaction = BluetoothTxPowerControlTransaction::new(
            BluetoothTxPowerControlOperation::PrepareRestore,
        );
        assert_eq!(
            transaction.start_with(&mut access),
            Err(BluetoothTxPowerControlError::RestorePending)
        );
        assert!(access.operations.is_empty());
    }

    #[test]
    fn prepare_retries_a_busy_first_command_without_reserving_restore_twice() {
        let mut access = FakeI2c::new([0xa5, 0xd5]);
        access.busy_starts = 1;
        let mut transaction = BluetoothTxPowerControlTransaction::new(
            BluetoothTxPowerControlOperation::PrepareRestore,
        );

        assert_eq!(
            transaction.start_with(&mut access),
            Err(BluetoothTxPowerControlError::BusyAtStart)
        );
        assert_eq!(
            transaction.action(),
            BluetoothTxPowerControlAction::StartCommand
        );
        assert!(access.operations.is_empty());

        assert_eq!(
            drive(&mut transaction, &mut access),
            BluetoothTxPowerControlCompletion::RestorePrepared
        );
        assert_eq!(access.restore, Some((0xa5, 0x15)));
    }
}
