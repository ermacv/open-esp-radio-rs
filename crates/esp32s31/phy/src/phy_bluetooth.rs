//! Shared PHY operations used by the ESP32-S31 Bluetooth calibration graph.
//!
//! Bluetooth-specific state and table publication remain explicitly named;
//! they are not folded into the Wi-Fi channel layer. The first recovered
//! leaves are the complete rev0 ROM conversions used by
//! `phy_bt_tx_gain_init`.

/// Convert the three Bluetooth gain indices to their baseband encodings.
///
/// Basis: complete rev0 ROM `phy_bt_index_to_bb` at `0x2f82_6b1a`, size
/// `0x1c`. All values outside the two nonzero indices map to zero.
#[inline]
pub const fn bluetooth_gain_index_to_baseband(index: u32) -> u32 {
    match index {
        1 => 0x80,
        2 => 0x100,
        _ => 0,
    }
}

/// Convert a Bluetooth baseband gain encoding back to its table index.
///
/// Basis: complete rev0 ROM `phy_bt_bb_to_index` at `0x2f82_6b36`, size
/// `0x1c`. Noncanonical encodings map to index zero exactly as in ROM.
#[inline]
pub const fn bluetooth_baseband_to_gain_index(baseband: u32) -> u32 {
    match baseband {
        0x80 => 1,
        0x100 => 2,
        _ => 0,
    }
}

const BLUETOOTH_TX_GAIN_TABLE_LOW: [u16; 18] = [
    0x003f, 0x002f, 0x001f, 0x0016, 0x000f, 0x000e, 0x000d, 0x000d, 0x000c, 0x000b, 0x0005, 0x0004,
    0x0003, 0x0002, 0x0001, 0x0001, 0x0000, 0x0000,
];
const BLUETOOTH_TX_GAIN_TABLE_MID: [u16; 18] = [
    0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0080, 0x0080, 0x0080, 0x0080, 0x0080,
    0x0080, 0x0080, 0x0080, 0x0000, 0x0080, 0x0000,
];
const BLUETOOTH_TX_GAIN_TABLE_HIGH: [u16; 18] = [
    0x002f, 0x0027, 0x001a, 0x000c, 0x0005, 0x0000, 0xfffb, 0xfff3, 0xffed, 0xffe5, 0xffda, 0xffd4,
    0xffcc, 0xffc2, 0xffb3, 0xffab, 0xff9b, 0xff93,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainParameters {
    pub seed: [u32; 6],
    pub config: u16,
    pub calibration_curve: [u8; 3],
    pub correction: i8,
    pub base: u8,
    pub attenuation: u8,
}

/// Canonical 16-entry Bluetooth gain table before its PAC-backed publication.
///
/// The three arrays correspond to semantic gain components, not to a C stack
/// layout. Keeping them separate lets the Rust implementation reuse the
/// shared hardware encoder without exposing the vendor's temporary buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainImage {
    pub seed: [u32; 6],
    pub output_32: [u8; 16],
    pub output_64: [u16; 16],
    pub output_72: [u16; 16],
    pub config: u16,
}

const fn select_bluetooth_tx_gain_index(mut index: u8, target: i16) -> u8 {
    let mut iteration = 0;
    while iteration != BLUETOOTH_TX_GAIN_TABLE_HIGH.len() {
        let current = BLUETOOTH_TX_GAIN_TABLE_HIGH[index as usize] as i16;
        if target < current {
            if index as usize == BLUETOOTH_TX_GAIN_TABLE_HIGH.len() - 1 {
                break;
            }
            index += 1;
            if target >= BLUETOOTH_TX_GAIN_TABLE_HIGH[index as usize] as i16 {
                break;
            }
        } else {
            if index == 0 {
                break;
            }
            if target < BLUETOOTH_TX_GAIN_TABLE_HIGH[index as usize - 1] as i16 {
                break;
            }
            index -= 1;
        }
        iteration += 1;
    }
    index
}

const fn clamp_i16(value: i16, low: i16, high: i16) -> i16 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

/// Pure Rust translation of archive `phy_bt_get_tx_tab_new` and complete ROM
/// children `phy_bt_get_tx_gain`, `phy_get_data_sat` and
/// `phy_get_tx_gain_value`.
///
/// The debug-print argument used by the vendor caller is zero and is omitted.
/// Arithmetic retains the vendor's signed 8/16-bit domains explicitly.
pub const fn calculate_bluetooth_tx_gain(
    parameters: PhyBluetoothTxGainParameters,
) -> PhyBluetoothTxGainImage {
    let mut image = PhyBluetoothTxGainImage {
        seed: parameters.seed,
        output_32: [0; 16],
        output_64: [0; 16],
        output_72: [0; 16],
        config: parameters.config,
    };
    let base_delta = parameters.base.wrapping_sub(parameters.attenuation) as i8 as i16;
    let interpolation = parameters.calibration_curve[1] as i8 as i16;
    let mut driver = -96_i16;
    let mut table_index = 0_u8;
    let mut output_index = 0;
    while output_index != 16 {
        let target = base_delta
            .wrapping_sub(parameters.correction as i16)
            .wrapping_add(clamp_i16(driver, -72, 80));
        table_index = select_bluetooth_tx_gain_index(table_index, target);
        let table_index = table_index as usize;
        image.output_72[output_index] = BLUETOOTH_TX_GAIN_TABLE_LOW[table_index];
        image.output_64[output_index] = BLUETOOTH_TX_GAIN_TABLE_MID[table_index];
        let residual = target
            .wrapping_sub(BLUETOOTH_TX_GAIN_TABLE_HIGH[table_index] as i16)
            .wrapping_sub(interpolation);
        image.output_32[output_index] = clamp_i16(residual, -60, 24) as u8;
        driver = driver.wrapping_add(12);
        output_index += 1;
    }
    image
}

/// Direct PAC-backed publication edge for one canonical Bluetooth gain image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainPublication {
    image: PhyBluetoothTxGainImage,
}

impl PhyBluetoothTxGainPublication {
    pub const fn new(image: PhyBluetoothTxGainImage) -> Self {
        Self { image }
    }

    pub const fn image(self) -> PhyBluetoothTxGainImage {
        self.image
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(self, registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters) {
        crate::radio_hal::publish_bluetooth_tx_gain_memory(registers, self.image);
    }
}

/// Rust-owned implementation of archive `phy_bt_txdc_cal_new`.
///
/// The common comparator search remains in `phy_txdc`; this wrapper gives the
/// Bluetooth graph an explicit type instead of disguising it as Wi-Fi state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxDcTransition {
    inner: crate::phy_txdc::PhyTxDcTransition,
}

/// Bluetooth-owned use of the shared power-detector TX-DC calibration.
///
/// This keeps archive `phy_txdc_cal_pwdet_init(1, 0, 1)` distinct from the
/// Wi-Fi invocation while reusing its protocol-independent search engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxDcPwdetTransition {
    inner: crate::phy_txdc_pwdet::PhyTxDcPwdetTransition,
}

/// Owned inputs of archive `phy_bt_tx_pwctrl_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxPowerParameters {
    pub calibration: crate::phy_tx_power::PhyTxPowerParameters,
    pub pbus_power_path_value: u8,
    pub pbus_tx_path_value: u8,
    pub dco: [u16; 4],
    pub tone_selector: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxPowerOutcome {
    pub calibration: crate::phy_tx_power::PhyTxPowerOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerFailure {
    PbusTimedOut(crate::phy_pbus::PhyPbusForceTest),
    Calibration(crate::phy_tx_power::PhyTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerAction {
    I2c(crate::phy_cold::PhyColdI2cRequest),
    Prepare(crate::phy_tx_cal::PhyTxCalibrationEnvironmentAction),
    ForcePbus(crate::phy_pbus::PhyPbusForceTest),
    ReadPbus { selector: u8, path: u8 },
    Calibration(crate::phy_tx_power::PhyTxPowerAction),
    Cleanup(crate::phy_tx_cal::PhyTxCalibrationEnvironmentAction),
    Complete(PhyBluetoothTxPowerOutcome),
    Failed(PhyBluetoothTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerCompletion {
    I2c(crate::phy_cold::PhyColdI2cOutcome),
    Prepare(crate::phy_tx_cal::PhyTxCalibrationEnvironmentCompletion),
    PbusCompleted(crate::phy_pbus::PhyPbusForceTest),
    PbusTimedOut(crate::phy_pbus::PhyPbusForceTest),
    PbusRead { selector: u8, path: u8, value: u16 },
    Calibration(crate::phy_tx_power::PhyTxPowerCompletion),
    Cleanup(crate::phy_tx_cal::PhyTxCalibrationEnvironmentCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyBluetoothTxPowerTerminal {
    Complete(crate::phy_tx_power::PhyTxPowerOutcome),
    Failed(PhyBluetoothTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyBluetoothTxPowerStep {
    ReadSavedByte,
    ReadSavedField,
    ConfigureI2c(u8),
    Prepare(crate::phy_tx_cal::PhyTxCalibrationEnvironmentTransition),
    ForcePowerPath,
    ForceRxPath,
    ReadForcedPath,
    RestoreForcedPath {
        value: u16,
    },
    ForceTxPath,
    ProgramDco(u8),
    Calibration(crate::phy_tx_power::PhyTxPowerTransition),
    RestoreI2c {
        index: u8,
        terminal: PhyBluetoothTxPowerTerminal,
    },
    Cleanup {
        terminal: PhyBluetoothTxPowerTerminal,
        transition: crate::phy_tx_cal::PhyTxCalibrationEnvironmentTransition,
    },
    Complete(PhyBluetoothTxPowerOutcome),
    Failed(PhyBluetoothTxPowerFailure),
}

const fn bluetooth_dco_transaction(dco: [u16; 4], index: u8) -> crate::phy_pbus::PhyPbusForceTest {
    match index {
        0 => crate::phy_pbus::PhyPbusForceTest::new(2, 1, dco[0]),
        1 => crate::phy_pbus::PhyPbusForceTest::new(3, 1, dco[1]),
        2 => crate::phy_pbus::PhyPbusForceTest::new(2, 2, dco[2]),
        _ => crate::phy_pbus::PhyPbusForceTest::new(3, 2, dco[3]),
    }
}

/// Complete Rust-owned state machine for archive `phy_bt_tx_pwctrl_init`.
///
/// The vendor's blocking PHY-I2C and PBus calls are represented as externally
/// completed actions. Temporary analog configuration is restored on both the
/// success path and every finite Rust failure path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxPowerTransition {
    parameters: PhyBluetoothTxPowerParameters,
    step: PhyBluetoothTxPowerStep,
    saved_byte: u8,
    saved_field: u8,
}

impl PhyBluetoothTxPowerTransition {
    pub const fn new(parameters: PhyBluetoothTxPowerParameters) -> Self {
        let step = if parameters.calibration.already_calibrated {
            PhyBluetoothTxPowerStep::Complete(PhyBluetoothTxPowerOutcome {
                calibration: crate::phy_tx_power::PhyTxPowerOutcome {
                    reference_codes: parameters.calibration.reference_codes,
                    power_curve: [0; 3],
                    point_corrections: [0; 3],
                    power_adjustment: 0,
                    final_attenuation: parameters.calibration.initial_attenuation,
                    current_channel: 0,
                    calibration_performed: false,
                },
            })
        } else {
            PhyBluetoothTxPowerStep::ReadSavedByte
        };
        Self {
            parameters,
            step,
            saved_byte: 0,
            saved_field: 0,
        }
    }

    const fn i2c_request(&self, index: u8, restore: bool) -> crate::phy_cold::PhyColdI2cRequest {
        use crate::phy_i2c::analog_registers as registers;
        let byte = if restore { self.saved_byte } else { 2 };
        let field = if restore { self.saved_field } else { 2 };
        match index {
            0 => crate::phy_cold::PhyColdI2cRequest::write_byte(
                registers::BT_TX_POWER_CONTROL_LOW_0,
                byte,
            ),
            1 => crate::phy_cold::PhyColdI2cRequest::write_byte(
                registers::BT_TX_POWER_CONTROL_LOW_1,
                byte,
            ),
            2 => crate::phy_cold::PhyColdI2cRequest::write_masked(
                registers::BT_TX_POWER_CONTROL_HIGH_0.address,
                registers::BT_TX_POWER_CONTROL_HIGH_0.high_bit,
                registers::BT_TX_POWER_CONTROL_HIGH_0.low_bit,
                field,
            )
            .expect("recovered six-bit field is valid"),
            _ => crate::phy_cold::PhyColdI2cRequest::write_masked(
                registers::BT_TX_POWER_CONTROL_HIGH_1.address,
                registers::BT_TX_POWER_CONTROL_HIGH_1.high_bit,
                registers::BT_TX_POWER_CONTROL_HIGH_1.low_bit,
                field,
            )
            .expect("recovered six-bit field is valid"),
        }
    }

    fn cleanup(&mut self, terminal: PhyBluetoothTxPowerTerminal) {
        self.step = PhyBluetoothTxPowerStep::RestoreI2c { index: 0, terminal };
    }

    pub const fn action(self) -> PhyBluetoothTxPowerAction {
        use crate::phy_i2c::analog_registers as registers;
        match self.step {
            PhyBluetoothTxPowerStep::ReadSavedByte => PhyBluetoothTxPowerAction::I2c(
                crate::phy_cold::PhyColdI2cRequest::read_byte(registers::BT_TX_POWER_CONTROL_LOW_0),
            ),
            PhyBluetoothTxPowerStep::ReadSavedField => PhyBluetoothTxPowerAction::I2c(
                crate::phy_cold::PhyColdI2cRequest::read_masked(
                    registers::BT_TX_POWER_CONTROL_HIGH_0.address,
                    registers::BT_TX_POWER_CONTROL_HIGH_0.high_bit,
                    registers::BT_TX_POWER_CONTROL_HIGH_0.low_bit,
                )
                .expect("recovered six-bit field is valid"),
            ),
            PhyBluetoothTxPowerStep::ConfigureI2c(index) => {
                PhyBluetoothTxPowerAction::I2c(self.i2c_request(index, false))
            }
            PhyBluetoothTxPowerStep::Prepare(transition) => {
                PhyBluetoothTxPowerAction::Prepare(transition.action())
            }
            PhyBluetoothTxPowerStep::ForcePowerPath => {
                PhyBluetoothTxPowerAction::ForcePbus(crate::phy_pbus::PhyPbusForceTest::new(
                    5,
                    1,
                    self.parameters.pbus_power_path_value as u16 + 0x1c0,
                ))
            }
            PhyBluetoothTxPowerStep::ForceRxPath => PhyBluetoothTxPowerAction::ForcePbus(
                crate::phy_pbus::PhyPbusForceTest::new(1, 2, 0),
            ),
            PhyBluetoothTxPowerStep::ReadForcedPath => PhyBluetoothTxPowerAction::ReadPbus {
                selector: 1,
                path: 1,
            },
            PhyBluetoothTxPowerStep::RestoreForcedPath { value } => {
                PhyBluetoothTxPowerAction::ForcePbus(crate::phy_pbus::PhyPbusForceTest::new(
                    1,
                    1,
                    value | 2,
                ))
            }
            PhyBluetoothTxPowerStep::ForceTxPath => {
                PhyBluetoothTxPowerAction::ForcePbus(crate::phy_pbus::PhyPbusForceTest::new(
                    4,
                    2,
                    (self.parameters.pbus_tx_path_value as u16) << 3,
                ))
            }
            PhyBluetoothTxPowerStep::ProgramDco(index) => PhyBluetoothTxPowerAction::ForcePbus(
                bluetooth_dco_transaction(self.parameters.dco, index),
            ),
            PhyBluetoothTxPowerStep::Calibration(transition) => {
                PhyBluetoothTxPowerAction::Calibration(transition.action())
            }
            PhyBluetoothTxPowerStep::RestoreI2c { index, .. } => {
                PhyBluetoothTxPowerAction::I2c(self.i2c_request(index, true))
            }
            PhyBluetoothTxPowerStep::Cleanup { transition, .. } => {
                PhyBluetoothTxPowerAction::Cleanup(transition.action())
            }
            PhyBluetoothTxPowerStep::Complete(outcome) => {
                PhyBluetoothTxPowerAction::Complete(outcome)
            }
            PhyBluetoothTxPowerStep::Failed(failure) => PhyBluetoothTxPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyBluetoothTxPowerCompletion,
    ) -> Result<(), PhyBluetoothTxPowerTransitionError> {
        use crate::phy_i2c::analog_registers as registers;
        self.step = match (self.step, completion) {
            (
                PhyBluetoothTxPowerStep::ReadSavedByte,
                PhyBluetoothTxPowerCompletion::I2c(crate::phy_cold::PhyColdI2cOutcome::Read {
                    address,
                    value,
                }),
            ) if address == registers::BT_TX_POWER_CONTROL_LOW_0 => {
                self.saved_byte = value;
                PhyBluetoothTxPowerStep::ReadSavedField
            }
            (
                PhyBluetoothTxPowerStep::ReadSavedField,
                PhyBluetoothTxPowerCompletion::I2c(crate::phy_cold::PhyColdI2cOutcome::Read {
                    address,
                    value,
                }),
            ) if address == registers::BT_TX_POWER_CONTROL_HIGH_0.address => {
                self.saved_field = value;
                PhyBluetoothTxPowerStep::ConfigureI2c(0)
            }
            (
                PhyBluetoothTxPowerStep::ConfigureI2c(index),
                PhyBluetoothTxPowerCompletion::I2c(crate::phy_cold::PhyColdI2cOutcome::Written {
                    address,
                }),
            ) if address == self.i2c_request(index, false).address() => {
                if index == 3 {
                    PhyBluetoothTxPowerStep::Prepare(
                        crate::phy_tx_cal::PhyTxCalibrationEnvironmentTransition::enter(
                            self.parameters.calibration.environment,
                        ),
                    )
                } else {
                    PhyBluetoothTxPowerStep::ConfigureI2c(index + 1)
                }
            }
            (
                PhyBluetoothTxPowerStep::Prepare(mut transition),
                PhyBluetoothTxPowerCompletion::Prepare(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    crate::phy_tx_cal::PhyTxCalibrationEnvironmentAction::Complete(
                        crate::phy_tx_cal::PhyTxCalibrationEnvironment::Debug,
                    ) => PhyBluetoothTxPowerStep::ForcePowerPath,
                    crate::phy_tx_cal::PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        PhyBluetoothTxPowerStep::Failed(PhyBluetoothTxPowerFailure::Calibration(
                            crate::phy_tx_power::PhyTxPowerFailure::Environment(failure),
                        ))
                    }
                    _ => PhyBluetoothTxPowerStep::Prepare(transition),
                }
            }
            (PhyBluetoothTxPowerStep::ForcePowerPath, completion) => self.force_next(
                completion,
                crate::phy_pbus::PhyPbusForceTest::new(
                    5,
                    1,
                    self.parameters.pbus_power_path_value as u16 + 0x1c0,
                ),
                PhyBluetoothTxPowerStep::ForceRxPath,
            )?,
            (PhyBluetoothTxPowerStep::ForceRxPath, completion) => self.force_next(
                completion,
                crate::phy_pbus::PhyPbusForceTest::new(1, 2, 0),
                PhyBluetoothTxPowerStep::ReadForcedPath,
            )?,
            (
                PhyBluetoothTxPowerStep::ReadForcedPath,
                PhyBluetoothTxPowerCompletion::PbusRead {
                    selector: 1,
                    path: 1,
                    value,
                },
            ) => PhyBluetoothTxPowerStep::RestoreForcedPath { value },
            (PhyBluetoothTxPowerStep::RestoreForcedPath { value }, completion) => self.force_next(
                completion,
                crate::phy_pbus::PhyPbusForceTest::new(1, 1, value | 2),
                PhyBluetoothTxPowerStep::ForceTxPath,
            )?,
            (PhyBluetoothTxPowerStep::ForceTxPath, completion) => self.force_next(
                completion,
                crate::phy_pbus::PhyPbusForceTest::new(
                    4,
                    2,
                    (self.parameters.pbus_tx_path_value as u16) << 3,
                ),
                PhyBluetoothTxPowerStep::ProgramDco(0),
            )?,
            (PhyBluetoothTxPowerStep::ProgramDco(index), completion) => self.force_next(
                completion,
                bluetooth_dco_transaction(self.parameters.dco, index),
                if index == 3 {
                    PhyBluetoothTxPowerStep::Calibration(
                        crate::phy_tx_power::PhyTxPowerTransition::new_bluetooth(
                            self.parameters.calibration,
                            self.parameters.tone_selector,
                        ),
                    )
                } else {
                    PhyBluetoothTxPowerStep::ProgramDco(index + 1)
                },
            )?,
            (
                PhyBluetoothTxPowerStep::Calibration(mut transition),
                PhyBluetoothTxPowerCompletion::Calibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    crate::phy_tx_power::PhyTxPowerAction::Complete(outcome) => {
                        self.cleanup(PhyBluetoothTxPowerTerminal::Complete(outcome));
                        return Ok(());
                    }
                    crate::phy_tx_power::PhyTxPowerAction::Failed(failure) => {
                        self.cleanup(PhyBluetoothTxPowerTerminal::Failed(
                            PhyBluetoothTxPowerFailure::Calibration(failure),
                        ));
                        return Ok(());
                    }
                    _ => PhyBluetoothTxPowerStep::Calibration(transition),
                }
            }
            (
                PhyBluetoothTxPowerStep::RestoreI2c { index, terminal },
                PhyBluetoothTxPowerCompletion::I2c(crate::phy_cold::PhyColdI2cOutcome::Written {
                    address,
                }),
            ) if address == self.i2c_request(index, true).address() => {
                if index == 3 {
                    PhyBluetoothTxPowerStep::Cleanup {
                        terminal,
                        transition: crate::phy_tx_cal::PhyTxCalibrationEnvironmentTransition::exit(
                            self.parameters.calibration.environment,
                        ),
                    }
                } else {
                    PhyBluetoothTxPowerStep::RestoreI2c {
                        index: index + 1,
                        terminal,
                    }
                }
            }
            (
                PhyBluetoothTxPowerStep::Cleanup {
                    terminal,
                    mut transition,
                },
                PhyBluetoothTxPowerCompletion::Cleanup(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    crate::phy_tx_cal::PhyTxCalibrationEnvironmentAction::Complete(
                        crate::phy_tx_cal::PhyTxCalibrationEnvironment::Work,
                    ) => match terminal {
                        PhyBluetoothTxPowerTerminal::Complete(calibration) => {
                            PhyBluetoothTxPowerStep::Complete(PhyBluetoothTxPowerOutcome {
                                calibration,
                            })
                        }
                        PhyBluetoothTxPowerTerminal::Failed(failure) => {
                            PhyBluetoothTxPowerStep::Failed(failure)
                        }
                    },
                    crate::phy_tx_cal::PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        PhyBluetoothTxPowerStep::Failed(PhyBluetoothTxPowerFailure::Calibration(
                            crate::phy_tx_power::PhyTxPowerFailure::Environment(failure),
                        ))
                    }
                    _ => PhyBluetoothTxPowerStep::Cleanup {
                        terminal,
                        transition,
                    },
                }
            }
            (PhyBluetoothTxPowerStep::Complete(_) | PhyBluetoothTxPowerStep::Failed(_), _) => {
                return Err(PhyBluetoothTxPowerTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyBluetoothTxPowerTransitionError::WrongCompletion),
        };
        Ok(())
    }

    fn force_next(
        &mut self,
        completion: PhyBluetoothTxPowerCompletion,
        expected: crate::phy_pbus::PhyPbusForceTest,
        next: PhyBluetoothTxPowerStep,
    ) -> Result<PhyBluetoothTxPowerStep, PhyBluetoothTxPowerTransitionError> {
        match completion {
            PhyBluetoothTxPowerCompletion::PbusCompleted(transaction)
                if transaction == expected =>
            {
                Ok(next)
            }
            PhyBluetoothTxPowerCompletion::PbusTimedOut(transaction) if transaction == expected => {
                self.cleanup(PhyBluetoothTxPowerTerminal::Failed(
                    PhyBluetoothTxPowerFailure::PbusTimedOut(transaction),
                ));
                Ok(self.step)
            }
            _ => Err(PhyBluetoothTxPowerTransitionError::WrongCompletion),
        }
    }
}

impl PhyBluetoothTxDcTransition {
    pub const fn new(parameters: crate::phy_txdc::PhyTxDcParameters, tx_path_value: u8) -> Self {
        Self {
            inner: crate::phy_txdc::PhyTxDcTransition::new_bluetooth(parameters, tx_path_value),
        }
    }

    pub const fn action(self) -> crate::phy_txdc::PhyTxDcAction {
        self.inner.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_txdc::PhyTxDcCompletion,
    ) -> Result<(), crate::phy_txdc::PhyTxDcTransitionError> {
        self.inner.advance(completion)
    }
}

impl PhyBluetoothTxDcPwdetTransition {
    pub const fn new(
        parameters: crate::phy_txdc_pwdet::PhyTxDcPwdetParameters,
        tx_path_value: u8,
    ) -> Self {
        Self {
            inner: crate::phy_txdc_pwdet::PhyTxDcPwdetTransition::new_bluetooth(
                parameters,
                tx_path_value,
            ),
        }
    }

    pub const fn action(&self) -> crate::phy_txdc_pwdet::PhyTxDcPwdetAction {
        self.inner.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::phy_txdc_pwdet::PhyTxDcPwdetCompletion,
    ) -> Result<(), crate::phy_txdc_pwdet::PhyTxDcPwdetTransitionError> {
        self.inner.advance(completion)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_index_conversion_preserves_only_the_two_rom_encodings() {
        assert_eq!(bluetooth_gain_index_to_baseband(0), 0);
        assert_eq!(bluetooth_gain_index_to_baseband(1), 0x80);
        assert_eq!(bluetooth_gain_index_to_baseband(2), 0x100);
        assert_eq!(bluetooth_gain_index_to_baseband(3), 0);
        assert_eq!(bluetooth_gain_index_to_baseband(u32::MAX), 0);
    }

    #[test]
    fn baseband_conversion_rejects_noncanonical_values() {
        assert_eq!(bluetooth_baseband_to_gain_index(0), 0);
        assert_eq!(bluetooth_baseband_to_gain_index(0x80), 1);
        assert_eq!(bluetooth_baseband_to_gain_index(0x100), 2);
        assert_eq!(bluetooth_baseband_to_gain_index(0x180), 0);
        assert_eq!(bluetooth_baseband_to_gain_index(u32::MAX), 0);
    }

    #[test]
    fn bluetooth_gain_image_matches_the_linked_vendor_cold_state() {
        let state = crate::phy_cold::PhyColdState::new();
        let image = state.bluetooth_tx_gain_image();
        assert_eq!(
            image.output_72,
            [1, 1, 1, 2, 3, 5, 11, 13, 14, 22, 22, 31, 63, 63, 63, 63]
        );
        assert_eq!(
            image.output_64,
            [
                0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x100, 0x100, 0x100, 0x100, 0x100,
                0x100, 0x100, 0x100,
            ]
        );
        assert_eq!(
            image.output_32,
            [5, 5, 5, 2, 4, 2, 3, 1, 0, 0, 12, 10, 1, 13, 24, 24]
        );
    }

    #[test]
    fn bluetooth_txdc_outcome_updates_only_three_bt_rows() {
        let mut state = crate::phy_cold::PhyColdState::new();
        let wifi_before = state.parameter_image()[0x0a8..0x0d0].to_vec();
        let outcome = crate::phy_txdc::PhyTxDcOutcome {
            dco: [
                [0x101, 0x102, 0x103, 0x104],
                [0x201, 0x202, 0x203, 0x204],
                [0x301, 0x302, 0x303, 0x304],
                [0x401, 0x402, 0x403, 0x404],
                [0x501, 0x502, 0x503, 0x504],
            ],
        };

        state.apply_bluetooth_tx_dc_outcome(outcome);

        assert!(state.bluetooth_tx_dc_calibrated());
        assert_eq!(&state.parameter_image()[0x0a8..0x0d0], wifi_before);
        for row in 0..3 {
            for column in 0..4 {
                let offset = 0x104 + row * 8 + column * 2;
                assert_eq!(
                    u16::from_le_bytes([
                        state.parameter_image()[offset],
                        state.parameter_image()[offset + 1],
                    ]),
                    outcome.dco[row][column]
                );
            }
        }
    }

    fn power_parameters() -> PhyBluetoothTxPowerParameters {
        PhyBluetoothTxPowerParameters {
            calibration: crate::phy_tx_power::PhyTxPowerParameters {
                already_calibrated: false,
                crystal_selector: 0,
                environment: crate::phy_tx_cal::PhyTxCalibrationParameters {
                    pbus_tx_path_value: 0,
                    pbus_rx_path_value: 0,
                    dco: [0; 4],
                },
                capacitance: [1, 2, 3, 4, 5, 6],
                target_adjustment: 0,
                power_offset: 0,
                initial_attenuation: 8,
                clear_tone_after_ready: false,
                reference_codes: [80, 120],
            },
            pbus_power_path_value: 7,
            pbus_tx_path_value: 9,
            dco: [0x101, 0x102, 0x103, 0x104],
            tone_selector: 0x55,
        }
    }

    fn complete_i2c_request(
        action: PhyBluetoothTxPowerAction,
        read_value: u8,
    ) -> PhyBluetoothTxPowerCompletion {
        let PhyBluetoothTxPowerAction::I2c(request) = action else {
            panic!("expected I2C action, got {action:?}");
        };
        match request {
            crate::phy_cold::PhyColdI2cRequest::ReadByte { address }
            | crate::phy_cold::PhyColdI2cRequest::ReadMasked { address, .. } => {
                PhyBluetoothTxPowerCompletion::I2c(crate::phy_cold::PhyColdI2cOutcome::Read {
                    address,
                    value: read_value,
                })
            }
            crate::phy_cold::PhyColdI2cRequest::WriteByte { address, .. }
            | crate::phy_cold::PhyColdI2cRequest::WriteMasked { address, .. } => {
                PhyBluetoothTxPowerCompletion::I2c(crate::phy_cold::PhyColdI2cOutcome::Written {
                    address,
                })
            }
        }
    }

    #[test]
    fn bluetooth_power_root_preserves_vendor_prefix_and_bt_child_mode() {
        let mut transition = PhyBluetoothTxPowerTransition::new(power_parameters());
        for read in [0x2a, 0x15] {
            let completion = complete_i2c_request(transition.action(), read);
            transition.advance(completion).unwrap();
        }
        for _ in 0..4 {
            let completion = complete_i2c_request(transition.action(), 0);
            transition.advance(completion).unwrap();
        }
        loop {
            let PhyBluetoothTxPowerAction::Prepare(action) = transition.action() else {
                break;
            };
            use crate::phy_tx_cal::{
                PhyTxCalibrationEnvironmentAction as Action,
                PhyTxCalibrationEnvironmentCompletion as Completion,
            };
            let completion = match action {
                Action::ConfigurePbusDebugMode => Completion::PbusDebugModeConfigured,
                Action::ForcePbus(transaction) => Completion::PbusCompleted(transaction),
                Action::ConfigureTxClock { enabled } => Completion::TxClockConfigured { enabled },
                Action::ConfigurePowerDetector => Completion::PowerDetectorConfigured,
                Action::ConfigureCalibrationMode => Completion::CalibrationModeConfigured,
                terminal => panic!("unexpected prepare action {terminal:?}"),
            };
            transition
                .advance(PhyBluetoothTxPowerCompletion::Prepare(completion))
                .unwrap();
        }

        for expected in [
            crate::phy_pbus::PhyPbusForceTest::new(5, 1, 0x1c7),
            crate::phy_pbus::PhyPbusForceTest::new(1, 2, 0),
        ] {
            assert_eq!(
                transition.action(),
                PhyBluetoothTxPowerAction::ForcePbus(expected)
            );
            transition
                .advance(PhyBluetoothTxPowerCompletion::PbusCompleted(expected))
                .unwrap();
        }
        assert_eq!(
            transition.action(),
            PhyBluetoothTxPowerAction::ReadPbus {
                selector: 1,
                path: 1
            }
        );
        transition
            .advance(PhyBluetoothTxPowerCompletion::PbusRead {
                selector: 1,
                path: 1,
                value: 0x41,
            })
            .unwrap();
        for expected in [
            crate::phy_pbus::PhyPbusForceTest::new(1, 1, 0x43),
            crate::phy_pbus::PhyPbusForceTest::new(4, 2, 0x48),
            crate::phy_pbus::PhyPbusForceTest::new(2, 1, 0x101),
            crate::phy_pbus::PhyPbusForceTest::new(3, 1, 0x102),
            crate::phy_pbus::PhyPbusForceTest::new(2, 2, 0x103),
            crate::phy_pbus::PhyPbusForceTest::new(3, 2, 0x104),
        ] {
            assert_eq!(
                transition.action(),
                PhyBluetoothTxPowerAction::ForcePbus(expected)
            );
            transition
                .advance(PhyBluetoothTxPowerCompletion::PbusCompleted(expected))
                .unwrap();
        }
        assert!(matches!(
            transition.action(),
            PhyBluetoothTxPowerAction::Calibration(
                crate::phy_tx_power::PhyTxPowerAction::WriteI2c { value: 0xc3, .. }
            )
        ));
    }

    #[test]
    fn bluetooth_power_outcome_publishes_bt_fields_only() {
        let mut state = crate::phy_cold::PhyColdState::new();
        let wifi_before = state.parameter_image()[0x0f1..0x0f8].to_vec();
        state.apply_bluetooth_tx_power_outcome(PhyBluetoothTxPowerOutcome {
            calibration: crate::phy_tx_power::PhyTxPowerOutcome {
                reference_codes: [80, 120],
                power_curve: [-3, 4, 5],
                point_corrections: [6, -7, 8],
                power_adjustment: -9,
                final_attenuation: 13,
                current_channel: 11,
                calibration_performed: true,
            },
        });
        assert!(state.bluetooth_tx_power_calibrated());
        assert_eq!(&state.parameter_image()[0x0f1..0x0f8], wifi_before);
        assert_eq!(&state.parameter_image()[0x0f8..0x0fb], &[6, 249, 8]);
        assert_eq!(&state.parameter_image()[0x0fb..0x0fe], &[253, 4, 5]);
        assert_eq!(state.parameter_image()[0x0fe], 247);
        assert_eq!(state.parameter_image()[0x018], 13);
        assert_eq!(&state.parameter_image()[0x100..0x102], &[0, 0]);
    }
}
