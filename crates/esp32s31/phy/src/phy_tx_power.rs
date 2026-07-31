//! Rust-owned ESP32-S31 TX power-control calibration.
//!
//! This module replaces the mandatory 154-byte archive root
//! `phy_tx_pwctrl_init`, its 396-byte archive child
//! `phy_tx_pwctrl_init_cal_new`, and the complete 462-byte ROM search
//! `phy_rfcal_pwrctrl`. Diagnostic formatting branches are intentionally
//! absent. Every tone/SAR measurement, RFPLL operation, PHY-I2C write, PBus
//! command and cleanup edge remains independently completed by the async
//! executor.

use crate::{
    phy_i2c::{PhyI2cAddress, analog_registers},
    phy_param::PHY_PARAM_LEN,
    phy_rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyRequest, RfpllFrequencyTransition,
    },
    phy_tx_cal::{
        PhyToneSarAction, PhyToneSarCompletion, PhyToneSarFailure, PhyToneSarRequest,
        PhyToneSarTransition, PhyTxCalibrationEnvironment, PhyTxCalibrationEnvironmentAction,
        PhyTxCalibrationEnvironmentCompletion, PhyTxCalibrationEnvironmentFailure,
        PhyTxCalibrationEnvironmentTransition, PhyTxCalibrationParameters, phy_tx_power_db,
    },
};

/// Number of signed quarter-unit targets consumed by the rev0 rate mapping.
pub const PHY_TX_TARGET_POWER_COUNT: usize = 18;

/// Calibrated PHY gain-table indices for one MAC rate.
///
/// These are signed table indices after division of the PHY quarter-unit
/// target by four. They are not dBm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxTargetPowerPair {
    pub primary: i8,
    pub alternate: i8,
}

impl PhyTxTargetPowerPair {
    pub const ZERO: Self = Self {
        primary: 0,
        alternate: 0,
    };
}

/// Rust-owned replacement for the target-power part of `phy_get_max_pwr`.
///
/// SOURCE[`ROM_REV0_PHY_GET_MAX_PWR`]: complete rev0 ROM
/// `phy_get_max_pwr` (0x2f82_49fe), `phy_get_target_pwr` (0x2f82_4976),
/// `phy_wifi_get_target_power` (0x2f82_70fa), and `phy_rate_to_index`
/// (0x2f82_491e), recovered from `_oracles/esp32s31_rev0_rom.elf`.
///
/// The profile owns the former `phy_param` inputs. It contains no pointer to
/// the cold state and never reads the vendor `phy_param` pointer cell or
/// `s_phy_get_max_pwr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxTargetPowerProfile {
    maximum: i8,
    target: [i8; PHY_TX_TARGET_POWER_COUNT],
    regulatory_override: bool,
}

impl PhyTxTargetPowerProfile {
    pub(crate) fn from_parameter_image(parameter: &[u8; PHY_PARAM_LEN]) -> Self {
        let mut target = [0_i8; PHY_TX_TARGET_POWER_COUNT];
        for (destination, source) in target.iter_mut().zip(&parameter[0x50..0x62]) {
            *destination = *source as i8;
        }
        Self {
            maximum: parameter[0x06] as i8,
            target,
            regulatory_override: parameter[0x64] == 1,
        }
    }

    /// Apply the upper-MAC transmit-power limit expressed in quarter-dBm.
    ///
    /// This is the source-owned equivalent of the limit installed by
    /// `esp_wifi_set_max_tx_power`: the PHY init-data ceiling and the runtime
    /// ceiling are both quarter-dBm values and the smaller one bounds every
    /// rate before `phy_get_max_pwr` converts it to the integer power code.
    ///
    /// SOURCE: `_oracles/esp32s31_rev0_rom.elf::phy_get_target_pwr` clamps the
    /// signed per-rate target before `phy_get_max_pwr` arithmetic-shifts it by
    /// two; `_oracles/libpp.a[hal_mac_tx.o]::hal_set_tx_pwr` publishes the
    /// resulting per-rate byte. The pinned ESP32-S31 vendor HIL confirms that
    /// a quarter-dBm limit of 20 produces table byte 5 and
    /// `TX_Q0_POWER=0x0505_0005`.
    pub fn with_maximum_quarter_dbm(mut self, maximum_quarter_dbm: i8) -> Self {
        self.maximum = self.maximum.min(maximum_quarter_dbm);
        self
    }

    /// Produce the two calibrated indices returned by the ROM root for a rate.
    ///
    /// Codes 32..=40 map beyond the recovered 18-byte target array and are
    /// reserved by the current MAC tables. They fail closed to zero instead of
    /// indexing outside owned state as the ROM routine can.
    pub fn pair(&self, rate: u8) -> PhyTxTargetPowerPair {
        let Some(index) = rate_to_target_index(rate) else {
            return PhyTxTargetPowerPair::ZERO;
        };

        // With the cold profile's regulatory flag clear, complete
        // phy_get_chan_target_power clamps every signed target first to the
        // selected FCC byte. All four default bytes at ROM 0x2f84_834c are 84.
        //
        // The override branch selects country/FCC state not yet owned by the
        // open driver. Its strongest safe bound is the calibrated maximum.
        let target = |target_index: usize| {
            let regulatory_limit = if self.regulatory_override {
                self.maximum
            } else {
                84
            };
            self.target[target_index]
                .min(regulatory_limit)
                .min(self.maximum)
        };
        let primary = target(index);
        let alternate = if (6..=9).contains(&index) {
            target(index + 8)
        } else {
            primary
        };
        PhyTxTargetPowerPair {
            primary: primary >> 2,
            alternate: alternate >> 2,
        }
    }
}

/// Exact valid-index portion of ROM `phy_rate_to_index` at 0x2f82_491e.
const fn rate_to_target_index(rate: u8) -> Option<usize> {
    let index = match rate {
        0..=7 => ((rate >> 1) & 1) as usize,
        8 => 5,
        9 => 4,
        10 => 3,
        11 => 2,
        12 => 5,
        13 => 4,
        14 => 3,
        15 => 2,
        16..=23 => ((((rate as i8) >> 1) as u8 & 7) + 6) as usize,
        24..=31 => (rate - 14) as usize,
        41 | 42 => 0,
        _ => return None,
    };
    Some(index)
}

const TX_CAP_ADDRESS: PhyI2cAddress = analog_registers::TX_CAPACITOR_BANKS;
const CHANNEL_CODES: [u16; 3] = [1, 6, 11];
const POWER_CONTROL_BASE_ATTENUATION: i16 = 52;
const POWER_CONTROL_MAX_ITERATIONS: u8 = 10;

const fn clamp_i16(value: i16, low: i16, high: i16) -> i16 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

const fn average_measured_power(first: i16, second: i16) -> i16 {
    let average = first.wrapping_add(second).wrapping_add(4) >> 3;
    if average < 0 { 0 } else { average }
}

const fn tx_cap_value(capacitance: [u8; 6], channel_code: u16) -> u8 {
    let index = if channel_code <= 3 {
        0
    } else if channel_code <= 8 {
        2
    } else {
        4
    };
    capacitance[index] | 0xc0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerControlPointRequest {
    pub identity: u8,
    pub target: i16,
    pub tone_selector: u16,
    pub base_attenuation: i16,
    pub initial_serial_error: i16,
    pub power_offset: i16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerControlPointOutcome {
    pub identity: u8,
    pub correction: i8,
    pub attenuation: u8,
    pub iterations: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerControlPointAction {
    ConfigureTone {
        identity: u8,
        iteration: u8,
        selector: u16,
        attenuation: u8,
    },
    ToneSar(PhyToneSarAction),
    StopTone {
        identity: u8,
    },
    Complete(PhyPowerControlPointOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerControlPointCompletion {
    ToneConfigured {
        identity: u8,
        iteration: u8,
        selector: u16,
        attenuation: u8,
    },
    ToneSar(PhyToneSarCompletion),
    ToneStopped {
        identity: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerControlPointTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PointStep {
    ConfigureTone,
    Measure {
        phase: u8,
        transition: PhyToneSarTransition,
    },
    StopTone,
    Complete,
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerControlPointTransition {
    request: PhyPowerControlPointRequest,
    step: PointStep,
    iteration: u8,
    serial_error: i16,
    previous_error: i16,
    attenuation: u8,
    first_power: i16,
}

impl PhyPowerControlPointTransition {
    pub const fn new(request: PhyPowerControlPointRequest) -> Self {
        Self {
            request,
            step: PointStep::ConfigureTone,
            iteration: 0,
            serial_error: request.initial_serial_error,
            previous_error: 2,
            attenuation: 0,
            first_power: 0,
        }
    }

    const fn requested_attenuation(&self) -> u8 {
        clamp_i16(
            self.request
                .base_attenuation
                .wrapping_add(self.serial_error),
            0,
            100,
        ) as u8
    }

    const fn measurement_identity(&self, phase: u8) -> u8 {
        self.request
            .identity
            .wrapping_mul(32)
            .wrapping_add(self.iteration.wrapping_mul(2))
            .wrapping_add(phase)
    }

    fn measurement(&self, phase: u8) -> PhyToneSarTransition {
        PhyToneSarTransition::new(PhyToneSarRequest {
            measurement: self.measurement_identity(phase),
            samples: 2,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
        .expect("two samples are nonzero")
    }

    const fn outcome(&self) -> PhyPowerControlPointOutcome {
        let correction = clamp_i16(
            self.attenuation as i16 - POWER_CONTROL_BASE_ATTENUATION,
            -24,
            48,
        ) as i8;
        PhyPowerControlPointOutcome {
            identity: self.request.identity,
            correction,
            attenuation: self.attenuation,
            iterations: self.iteration + 1,
        }
    }

    pub const fn action(&self) -> PhyPowerControlPointAction {
        match self.step {
            PointStep::ConfigureTone => PhyPowerControlPointAction::ConfigureTone {
                identity: self.request.identity,
                iteration: self.iteration,
                selector: self.request.tone_selector,
                attenuation: self.requested_attenuation(),
            },
            PointStep::Measure { transition, .. } => {
                PhyPowerControlPointAction::ToneSar(transition.action())
            }
            PointStep::StopTone => PhyPowerControlPointAction::StopTone {
                identity: self.request.identity,
            },
            PointStep::Complete => PhyPowerControlPointAction::Complete(self.outcome()),
            PointStep::Failed(failure) => PhyPowerControlPointAction::Failed(failure),
        }
    }

    fn finish_measurement(&mut self, second_power: i16) {
        let measured = average_measured_power(self.first_power, second_power);
        let error = clamp_i16(measured.wrapping_sub(self.request.target), -24, 24);
        let requested = self.requested_attenuation();
        self.attenuation = requested;

        let converged =
            (error == 0 && self.iteration != 0) || (error == -1 && self.previous_error == 1);
        let hit_limit = (requested == 0 && error < 0) || (requested == 100 && error > 0);
        if converged || hit_limit || self.iteration + 1 == POWER_CONTROL_MAX_ITERATIONS {
            self.step = PointStep::StopTone;
            return;
        }

        let mut serial = self.serial_error.wrapping_add(error);
        if !(-2..=2).contains(&error) {
            serial = serial.wrapping_sub(error >> 2);
        }
        self.serial_error = serial;
        self.previous_error = error;
        self.iteration += 1;
        self.step = PointStep::ConfigureTone;
    }

    pub fn advance(
        &mut self,
        completion: PhyPowerControlPointCompletion,
    ) -> Result<(), PhyPowerControlPointTransitionError> {
        match (self.step, completion) {
            (
                PointStep::ConfigureTone,
                PhyPowerControlPointCompletion::ToneConfigured {
                    identity,
                    iteration,
                    selector,
                    attenuation,
                },
            ) if identity == self.request.identity
                && iteration == self.iteration
                && selector == self.request.tone_selector
                && attenuation == self.requested_attenuation() =>
            {
                self.attenuation = attenuation;
                self.step = PointStep::Measure {
                    phase: 0,
                    transition: self.measurement(0),
                };
            }
            (
                PointStep::Measure {
                    phase,
                    mut transition,
                },
                PhyPowerControlPointCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyPowerControlPointTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        let power = phy_tx_power_db(
                            outcome.sample,
                            self.request.reference_codes,
                            self.request.power_offset,
                        );
                        if phase == 0 {
                            self.first_power = power;
                            self.step = PointStep::Measure {
                                phase: 1,
                                transition: self.measurement(1),
                            };
                        } else {
                            self.finish_measurement(power);
                        }
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.step = PointStep::Failed(failure);
                    }
                    _ => {
                        self.step = PointStep::Measure { phase, transition };
                    }
                }
            }
            (PointStep::StopTone, PhyPowerControlPointCompletion::ToneStopped { identity })
                if identity == self.request.identity =>
            {
                self.step = PointStep::Complete;
            }
            (PointStep::Complete | PointStep::Failed(_), _) => {
                return Err(PhyPowerControlPointTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyPowerControlPointTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerParameters {
    pub already_calibrated: bool,
    pub crystal_selector: u8,
    pub environment: PhyTxCalibrationParameters,
    pub capacitance: [u8; 6],
    pub target_adjustment: u8,
    pub power_offset: i16,
    pub initial_attenuation: u8,
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerOutcome {
    pub reference_codes: [i16; 2],
    pub power_curve: [i8; 3],
    pub point_corrections: [i8; 3],
    pub power_adjustment: i8,
    pub final_attenuation: u8,
    pub current_channel: u16,
    pub calibration_performed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerFailure {
    Environment(PhyTxCalibrationEnvironmentFailure),
    Rfpll(RfpllFrequencyFailure),
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerAction {
    Environment(PhyTxCalibrationEnvironmentAction),
    Rfpll(RfpllFrequencyAction),
    WriteI2c {
        address: PhyI2cAddress,
        value: u8,
    },
    ConfigureTone {
        selector: u16,
        attenuation: u8,
        enabled: bool,
    },
    WriteReferenceControl {
        value: u16,
    },
    ToneSar(PhyToneSarAction),
    Point(PhyPowerControlPointAction),
    Complete(PhyTxPowerOutcome),
    Failed(PhyTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerCompletion {
    Environment(PhyTxCalibrationEnvironmentCompletion),
    Rfpll(RfpllFrequencyCompletion),
    I2cWritten {
        address: PhyI2cAddress,
        value: u8,
    },
    ToneConfigured {
        selector: u16,
        attenuation: u8,
        enabled: bool,
    },
    ReferenceControlWritten {
        value: u16,
    },
    ToneSar(PhyToneSarCompletion),
    Point(PhyPowerControlPointCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxPowerTerminal {
    Complete,
    Failed(PhyTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxPowerStep {
    Enter(PhyTxCalibrationEnvironmentTransition),
    Rfpll(RfpllFrequencyTransition),
    TxCap,
    ReferenceTone,
    ReferenceControl {
        phase: u8,
    },
    ReferenceSample {
        phase: u8,
        transition: PhyToneSarTransition,
    },
    Point(PhyPowerControlPointTransition),
    Exit {
        terminal: TxPowerTerminal,
        transition: PhyTxCalibrationEnvironmentTransition,
    },
    Complete,
    Failed(PhyTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxPowerTransition {
    parameters: PhyTxPowerParameters,
    step: TxPowerStep,
    channel: u8,
    reference_codes: [i16; 2],
    power_curve: [i8; 3],
    point_corrections: [i8; 3],
    power_adjustment: i8,
    attenuation: u8,
}

impl PhyTxPowerTransition {
    pub const fn new(parameters: PhyTxPowerParameters) -> Self {
        let step = if parameters.already_calibrated {
            TxPowerStep::Complete
        } else {
            TxPowerStep::Enter(PhyTxCalibrationEnvironmentTransition::enter(
                parameters.environment,
            ))
        };
        Self {
            parameters,
            step,
            channel: 0,
            reference_codes: [0; 2],
            power_curve: [0; 3],
            point_corrections: [0; 3],
            power_adjustment: 0,
            attenuation: parameters.initial_attenuation,
        }
    }

    const fn outcome(&self) -> PhyTxPowerOutcome {
        PhyTxPowerOutcome {
            reference_codes: self.reference_codes,
            power_curve: self.power_curve,
            point_corrections: self.point_corrections,
            power_adjustment: self.power_adjustment,
            final_attenuation: self.attenuation,
            current_channel: if self.parameters.already_calibrated {
                0
            } else {
                11
            },
            calibration_performed: !self.parameters.already_calibrated,
        }
    }

    fn exit(&mut self, terminal: TxPowerTerminal) {
        self.step = TxPowerStep::Exit {
            terminal,
            transition: PhyTxCalibrationEnvironmentTransition::exit(self.parameters.environment),
        };
    }

    fn fail(&mut self, failure: PhyTxPowerFailure) {
        self.exit(TxPowerTerminal::Failed(failure));
    }

    fn rfpll(&self) -> RfpllFrequencyTransition {
        RfpllFrequencyTransition::new(RfpllFrequencyRequest {
            crystal_selector: self.parameters.crystal_selector,
            frequency_code: CHANNEL_CODES[self.channel as usize],
            offset: 0,
        })
    }

    fn reference_sample(&self, phase: u8) -> PhyToneSarTransition {
        PhyToneSarTransition::new(PhyToneSarRequest {
            measurement: 0x60 + phase,
            samples: 4,
            clear_tone_after_ready: self.parameters.clear_tone_after_ready,
        })
        .expect("four samples are nonzero")
    }

    fn point(&self) -> PhyPowerControlPointTransition {
        PhyPowerControlPointTransition::new(PhyPowerControlPointRequest {
            identity: self.channel,
            target: 56_i16.wrapping_sub(self.parameters.target_adjustment as i16),
            tone_selector: 0x80,
            base_attenuation: self.attenuation as i16,
            initial_serial_error: 0,
            power_offset: self.parameters.power_offset,
            reference_codes: self.reference_codes,
            // `phy_tx_pwctrl_init_cal_new` forces former
            // `phy_param[0x1aa] = 1` around all three point searches.
            clear_tone_after_ready: true,
        })
    }

    fn finish_points(&mut self) {
        let first = self.power_curve[0] as i16;
        let adjustment = if !(0..=16).contains(&first) {
            clamp_i16(first - 12, -40, 40) as i8
        } else {
            0
        };
        self.power_adjustment = (56_i16
            .wrapping_sub(self.parameters.target_adjustment as i16)
            .wrapping_add(adjustment as i16)) as i8;
        if adjustment != 0 {
            let mut index = 0;
            while index != self.power_curve.len() {
                self.power_curve[index] = self.power_curve[index].wrapping_sub(adjustment);
                index += 1;
            }
        }
        self.exit(TxPowerTerminal::Complete);
    }

    pub const fn action(&self) -> PhyTxPowerAction {
        match self.step {
            TxPowerStep::Enter(transition) | TxPowerStep::Exit { transition, .. } => {
                PhyTxPowerAction::Environment(transition.action())
            }
            TxPowerStep::Rfpll(transition) => PhyTxPowerAction::Rfpll(transition.action()),
            TxPowerStep::TxCap => PhyTxPowerAction::WriteI2c {
                address: TX_CAP_ADDRESS,
                value: tx_cap_value(
                    self.parameters.capacitance,
                    CHANNEL_CODES[self.channel as usize],
                ),
            },
            TxPowerStep::ReferenceTone => PhyTxPowerAction::ConfigureTone {
                selector: 0x80,
                attenuation: 0x50,
                enabled: true,
            },
            TxPowerStep::ReferenceControl { phase } => PhyTxPowerAction::WriteReferenceControl {
                value: match phase {
                    0 => 0,
                    1 => 0x5555,
                    _ => 0xaaaa,
                },
            },
            TxPowerStep::ReferenceSample { transition, .. } => {
                PhyTxPowerAction::ToneSar(transition.action())
            }
            TxPowerStep::Point(transition) => PhyTxPowerAction::Point(transition.action()),
            TxPowerStep::Complete => PhyTxPowerAction::Complete(self.outcome()),
            TxPowerStep::Failed(failure) => PhyTxPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxPowerCompletion,
    ) -> Result<(), PhyTxPowerTransitionError> {
        match (self.step, completion) {
            (TxPowerStep::Enter(mut transition), PhyTxPowerCompletion::Environment(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Debug,
                    ) => self.step = TxPowerStep::Rfpll(self.rfpll()),
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.fail(PhyTxPowerFailure::Environment(failure));
                    }
                    _ => self.step = TxPowerStep::Enter(transition),
                }
            }
            (TxPowerStep::Rfpll(mut transition), PhyTxPowerCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => self.step = TxPowerStep::TxCap,
                    RfpllFrequencyAction::Failed(failure) => {
                        self.fail(PhyTxPowerFailure::Rfpll(failure));
                    }
                    _ => self.step = TxPowerStep::Rfpll(transition),
                }
            }
            (TxPowerStep::TxCap, PhyTxPowerCompletion::I2cWritten { address, value })
                if address == TX_CAP_ADDRESS
                    && value
                        == tx_cap_value(
                            self.parameters.capacitance,
                            CHANNEL_CODES[self.channel as usize],
                        ) =>
            {
                self.step = if self.channel == 0 {
                    TxPowerStep::ReferenceTone
                } else {
                    TxPowerStep::Point(self.point())
                };
            }
            (
                TxPowerStep::ReferenceTone,
                PhyTxPowerCompletion::ToneConfigured {
                    selector: 0x80,
                    attenuation: 0x50,
                    enabled: true,
                },
            ) => self.step = TxPowerStep::ReferenceControl { phase: 0 },
            (
                TxPowerStep::ReferenceControl { phase },
                PhyTxPowerCompletion::ReferenceControlWritten { value },
            ) if value
                == match phase {
                    0 => 0,
                    1 => 0x5555,
                    _ => 0xaaaa,
                } =>
            {
                if phase == 2 {
                    self.step = TxPowerStep::Point(self.point());
                } else {
                    self.step = TxPowerStep::ReferenceSample {
                        phase,
                        transition: self.reference_sample(phase),
                    };
                }
            }
            (
                TxPowerStep::ReferenceSample {
                    phase,
                    mut transition,
                },
                PhyTxPowerCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        self.reference_codes[phase as usize] = outcome.sample as i16;
                        self.step = TxPowerStep::ReferenceControl { phase: phase + 1 };
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.fail(PhyTxPowerFailure::ToneSar(failure));
                    }
                    _ => {
                        self.step = TxPowerStep::ReferenceSample { phase, transition };
                    }
                }
            }
            (TxPowerStep::Point(mut transition), PhyTxPowerCompletion::Point(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyPowerControlPointAction::Complete(outcome) => {
                        let index = self.channel as usize;
                        self.point_corrections[index] = outcome.correction;
                        self.power_curve[index] = if self.channel == 1 {
                            outcome.correction.wrapping_add(2)
                        } else {
                            outcome.correction
                        };
                        self.attenuation = self.power_curve[index].wrapping_add(52_i8) as u8;
                        self.channel += 1;
                        if self.channel == 3 {
                            self.finish_points();
                        } else {
                            self.step = TxPowerStep::Rfpll(self.rfpll());
                        }
                    }
                    PhyPowerControlPointAction::Failed(failure) => {
                        self.fail(PhyTxPowerFailure::ToneSar(failure));
                    }
                    _ => self.step = TxPowerStep::Point(transition),
                }
            }
            (
                TxPowerStep::Exit {
                    terminal,
                    mut transition,
                },
                PhyTxPowerCompletion::Environment(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Work,
                    ) => {
                        self.step = match terminal {
                            TxPowerTerminal::Complete => TxPowerStep::Complete,
                            TxPowerTerminal::Failed(failure) => TxPowerStep::Failed(failure),
                        };
                    }
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.step = TxPowerStep::Failed(PhyTxPowerFailure::Environment(failure));
                    }
                    _ => {
                        self.step = TxPowerStep::Exit {
                            terminal,
                            transition,
                        };
                    }
                }
            }
            (TxPowerStep::Complete | TxPowerStep::Failed(_), _) => {
                return Err(PhyTxPowerTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxPowerTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerBindingError {
    NotDirectMmio,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyPowerControlPointMmioBinding {
    action: PhyPowerControlPointAction,
}

impl PhyPowerControlPointMmioBinding {
    pub fn new(action: PhyPowerControlPointAction) -> Result<Self, PhyTxPowerBindingError> {
        match action {
            PhyPowerControlPointAction::ConfigureTone { .. }
            | PhyPowerControlPointAction::StopTone { .. } => Ok(Self { action }),
            _ => Err(PhyTxPowerBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> PhyPowerControlPointCompletion {
        match self.action {
            PhyPowerControlPointAction::ConfigureTone {
                identity,
                iteration,
                selector,
                attenuation,
            } => {
                crate::radio_hal::configure_phy_power_control_tone(
                    registers,
                    selector,
                    attenuation,
                );
                PhyPowerControlPointCompletion::ToneConfigured {
                    identity,
                    iteration,
                    selector,
                    attenuation,
                }
            }
            PhyPowerControlPointAction::StopTone { identity } => {
                crate::radio_hal::stop_phy_power_detector_tone(registers);
                PhyPowerControlPointCompletion::ToneStopped { identity }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxPowerMmioBinding {
    action: PhyTxPowerAction,
}

impl PhyTxPowerMmioBinding {
    pub fn new(action: PhyTxPowerAction) -> Result<Self, PhyTxPowerBindingError> {
        match action {
            PhyTxPowerAction::ConfigureTone { enabled: true, .. }
            | PhyTxPowerAction::WriteReferenceControl { .. } => Ok(Self { action }),
            _ => Err(PhyTxPowerBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> PhyTxPowerCompletion {
        match self.action {
            PhyTxPowerAction::ConfigureTone {
                selector,
                attenuation,
                enabled: true,
            } => {
                crate::radio_hal::configure_phy_power_control_tone(
                    registers,
                    selector,
                    attenuation,
                );
                PhyTxPowerCompletion::ToneConfigured {
                    selector,
                    attenuation,
                    enabled: true,
                }
            }
            PhyTxPowerAction::WriteReferenceControl { value } => {
                open_esp_radio_esp32s31_hal::phy_power_detector::write_reference(registers, value);
                PhyTxPowerCompletion::ReferenceControlWritten { value }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxPowerExternalBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyPowerControlPointExternalBinding {
    Mmio(PhyPowerControlPointMmioBinding),
    ToneSar(crate::phy_tx_cal::PhyToneSarExternalBinding),
}

impl PhyPowerControlPointExternalBinding {
    pub fn lower(
        action: PhyPowerControlPointAction,
    ) -> Result<Self, PhyTxPowerExternalBindingError> {
        match action {
            PhyPowerControlPointAction::ConfigureTone { .. }
            | PhyPowerControlPointAction::StopTone { .. } => {
                PhyPowerControlPointMmioBinding::new(action)
                    .map(Self::Mmio)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyPowerControlPointAction::ToneSar(action) => {
                crate::phy_tx_cal::PhyToneSarExternalBinding::lower(action)
                    .map(Self::ToneSar)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyPowerControlPointAction::Complete(_) | PhyPowerControlPointAction::Failed(_) => {
                Err(PhyTxPowerExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxPowerI2cBinding {
    address: PhyI2cAddress,
    value: u8,
    transaction: crate::phy_cold::PhyColdI2cTransaction,
}

impl PhyTxPowerI2cBinding {
    pub fn new(action: PhyTxPowerAction) -> Result<Self, PhyTxPowerExternalBindingError> {
        let PhyTxPowerAction::WriteI2c { address, value } = action else {
            return Err(PhyTxPowerExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
            address,
            value,
            transaction: crate::phy_cold::PhyColdI2cTransaction::new(
                crate::phy_cold::PhyColdI2cRequest::write_byte(address, value),
            ),
        })
    }

    pub const fn action(&self) -> crate::phy_cold::PhyColdI2cAction {
        self.transaction.action()
    }

    pub fn write_started(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), crate::phy_i2c::PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl>(
        &mut self,
        platform: &mut P,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(self) -> Result<PhyTxPowerCompletion, PhyTxPowerExternalBindingError> {
        match self.transaction.action() {
            crate::phy_cold::PhyColdI2cAction::Complete(
                crate::phy_cold::PhyColdI2cOutcome::Written { address },
            ) if address == self.address => Ok(PhyTxPowerCompletion::I2cWritten {
                address: self.address,
                value: self.value,
            }),
            crate::phy_cold::PhyColdI2cAction::Complete(_) => {
                Err(PhyTxPowerExternalBindingError::UnexpectedOutcome)
            }
            _ => Err(PhyTxPowerExternalBindingError::IncompleteTransaction),
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_tx_pwctrl_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxPowerExternalBinding {
    Environment(crate::phy_tx_cal::PhyTxCalibrationEnvironmentExternalBinding),
    Rfpll(crate::phy_rfpll::RfpllFrequencyExternalBinding),
    I2c(PhyTxPowerI2cBinding),
    Mmio(PhyTxPowerMmioBinding),
    ToneSar(crate::phy_tx_cal::PhyToneSarExternalBinding),
    Point(PhyPowerControlPointExternalBinding),
}

impl PhyTxPowerExternalBinding {
    pub fn lower(action: PhyTxPowerAction) -> Result<Self, PhyTxPowerExternalBindingError> {
        match action {
            PhyTxPowerAction::Environment(action) => {
                crate::phy_tx_cal::PhyTxCalibrationEnvironmentExternalBinding::lower(action)
                    .map(Self::Environment)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyTxPowerAction::Rfpll(action) => {
                crate::phy_rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyTxPowerAction::WriteI2c { .. } => PhyTxPowerI2cBinding::new(action).map(Self::I2c),
            PhyTxPowerAction::ConfigureTone { .. }
            | PhyTxPowerAction::WriteReferenceControl { .. } => PhyTxPowerMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction),
            PhyTxPowerAction::ToneSar(action) => {
                crate::phy_tx_cal::PhyToneSarExternalBinding::lower(action)
                    .map(Self::ToneSar)
                    .map_err(|_| PhyTxPowerExternalBindingError::UnsupportedAction)
            }
            PhyTxPowerAction::Point(action) => {
                PhyPowerControlPointExternalBinding::lower(action).map(Self::Point)
            }
            PhyTxPowerAction::Complete(_) | PhyTxPowerAction::Failed(_) => {
                Err(PhyTxPowerExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_profile(
        maximum: i8,
        targets: [i8; PHY_TX_TARGET_POWER_COUNT],
        regulatory_override: bool,
    ) -> PhyTxTargetPowerProfile {
        let mut parameter = [0_u8; PHY_PARAM_LEN];
        parameter[0x06] = maximum as u8;
        for (destination, source) in parameter[0x50..0x62].iter_mut().zip(targets) {
            *destination = source as u8;
        }
        parameter[0x64] = regulatory_override as u8;
        PhyTxTargetPowerProfile::from_parameter_image(&parameter)
    }

    #[test]
    fn target_profile_matches_every_recovered_rate_mapping_class() {
        let profile = target_profile(
            100,
            core::array::from_fn(|index| 8 + (index as i8 * 4)),
            false,
        );
        assert_eq!(
            profile.pair(0),
            PhyTxTargetPowerPair {
                primary: 2,
                alternate: 2,
            }
        );
        assert_eq!(
            profile.pair(2),
            PhyTxTargetPowerPair {
                primary: 3,
                alternate: 3,
            }
        );
        assert_eq!(
            profile.pair(8),
            PhyTxTargetPowerPair {
                primary: 7,
                alternate: 7,
            }
        );
        assert_eq!(
            profile.pair(16),
            PhyTxTargetPowerPair {
                primary: 8,
                alternate: 16,
            }
        );
        assert_eq!(
            profile.pair(22),
            PhyTxTargetPowerPair {
                primary: 11,
                alternate: 19,
            }
        );
        assert_eq!(
            profile.pair(24),
            PhyTxTargetPowerPair {
                primary: 12,
                alternate: 12,
            }
        );
        assert_eq!(
            profile.pair(31),
            PhyTxTargetPowerPair {
                primary: 19,
                alternate: 19,
            }
        );
        assert_eq!(profile.pair(41), profile.pair(0));
        assert_eq!(profile.pair(42), profile.pair(0));
        assert_eq!(profile.pair(32), PhyTxTargetPowerPair::ZERO);
        assert_eq!(profile.pair(40), PhyTxTargetPowerPair::ZERO);
        assert_eq!(profile.pair(43), PhyTxTargetPowerPair::ZERO);
    }

    #[test]
    fn target_profile_applies_default_fcc_and_calibrated_maximum_bounds() {
        let mut targets = [0_i8; PHY_TX_TARGET_POWER_COUNT];
        targets[0] = 120;
        assert_eq!(target_profile(100, targets, false).pair(0).primary, 21);
        assert_eq!(target_profile(80, targets, false).pair(0).primary, 20);
        assert_eq!(target_profile(100, targets, true).pair(0).primary, 25);
        targets[0] = -12;
        assert_eq!(target_profile(100, targets, false).pair(0).primary, -3);
    }

    #[test]
    fn runtime_quarter_dbm_limit_matches_vendor_mac_power_code() {
        let targets = [80_i8; PHY_TX_TARGET_POWER_COUNT];
        let profile = target_profile(84, targets, false).with_maximum_quarter_dbm(20);
        assert_eq!(
            profile.pair(0),
            PhyTxTargetPowerPair {
                primary: 5,
                alternate: 5,
            }
        );
    }

    #[test]
    fn cold_state_exports_an_owned_target_profile_snapshot() {
        let mut parameter = [0_u8; PHY_PARAM_LEN];
        parameter[0x06] = 80;
        parameter[0x50] = 44;
        let state = crate::phy_cold::PhyColdState::from_parameter_image(parameter);
        let profile = state.tx_target_power_profile();
        drop(state);
        assert_eq!(
            profile.pair(0),
            PhyTxTargetPowerPair {
                primary: 11,
                alternate: 11,
            }
        );
    }

    fn tone_sar_completion(action: PhyToneSarAction, value: u16) -> PhyToneSarCompletion {
        match action {
            PhyToneSarAction::ArmTone {
                measurement,
                sample,
            } => PhyToneSarCompletion::ToneArmed {
                measurement,
                sample,
            },
            PhyToneSarAction::DelayMicros {
                measurement,
                sample,
                phase,
                micros,
            } => PhyToneSarCompletion::DelayElapsed {
                measurement,
                sample,
                phase,
                micros,
            },
            PhyToneSarAction::TriggerSar {
                measurement,
                sample,
            } => PhyToneSarCompletion::SarTriggered {
                measurement,
                sample,
            },
            PhyToneSarAction::PollReady {
                measurement,
                sample,
                address,
                ..
            } => PhyToneSarCompletion::ReadySampled {
                measurement,
                sample,
                address,
                register_value: crate::phy_pwdet::PHY_PWDET_READY_VALUE,
            },
            PhyToneSarAction::ClearTone {
                measurement,
                sample,
            } => PhyToneSarCompletion::ToneCleared {
                measurement,
                sample,
            },
            PhyToneSarAction::ReadSar {
                measurement,
                sample,
                address,
            } => PhyToneSarCompletion::SarRead {
                measurement,
                sample,
                address,
                register_value: u32::from(value) << 17,
            },
            terminal => panic!("unexpected terminal action {terminal:?}"),
        }
    }

    #[test]
    fn tx_cap_selection_uses_three_recovered_channel_bands() {
        let cap = [1, 2, 3, 4, 5, 6];
        assert_eq!(tx_cap_value(cap, 1), 0xc1);
        assert_eq!(tx_cap_value(cap, 6), 0xc3);
        assert_eq!(tx_cap_value(cap, 11), 0xc5);
    }

    #[test]
    fn point_power_average_matches_rom_zero_floor() {
        assert_eq!(average_measured_power(-20, -20), 0);
        assert_eq!(average_measured_power(20, 24), 6);
    }

    #[test]
    fn point_search_keeps_the_rom_i16_serial_error_width() {
        let request = PhyPowerControlPointRequest {
            identity: 1,
            target: 0,
            tone_selector: 0x80,
            base_attenuation: 100,
            initial_serial_error: 130,
            power_offset: 0,
            reference_codes: [0, 100],
            clear_tone_after_ready: false,
        };
        let transition = PhyPowerControlPointTransition::new(request);
        assert!(matches!(
            transition.action(),
            PhyPowerControlPointAction::ConfigureTone {
                attenuation: 100,
                ..
            }
        ));
    }

    #[test]
    fn point_search_is_bounded_to_ten_iterations_and_four_sar_samples_each() {
        let mut transition = PhyPowerControlPointTransition::new(PhyPowerControlPointRequest {
            identity: 1,
            target: -100,
            tone_selector: 0x80,
            base_attenuation: 52,
            initial_serial_error: 0,
            power_offset: 0,
            reference_codes: [0, 100],
            clear_tone_after_ready: false,
        });
        let mut reads = 0;
        loop {
            let completion = match transition.action() {
                PhyPowerControlPointAction::ConfigureTone {
                    identity,
                    iteration,
                    selector,
                    attenuation,
                } => PhyPowerControlPointCompletion::ToneConfigured {
                    identity,
                    iteration,
                    selector,
                    attenuation,
                },
                PhyPowerControlPointAction::ToneSar(action) => {
                    if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                        reads += 1;
                    }
                    PhyPowerControlPointCompletion::ToneSar(tone_sar_completion(action, 50))
                }
                PhyPowerControlPointAction::StopTone { identity } => {
                    PhyPowerControlPointCompletion::ToneStopped { identity }
                }
                PhyPowerControlPointAction::Complete(outcome) => {
                    assert!(outcome.iterations <= 10);
                    break;
                }
                PhyPowerControlPointAction::Failed(failure) => {
                    panic!("unexpected failure {failure:?}")
                }
            };
            transition.advance(completion).unwrap();
        }
        assert!(reads <= 40);
    }

    #[test]
    fn already_calibrated_root_emits_no_hardware_action() {
        let transition = PhyTxPowerTransition::new(PhyTxPowerParameters {
            already_calibrated: true,
            crystal_selector: 0,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: 0,
                pbus_rx_path_value: 0,
                dco: [0; 4],
            },
            capacitance: [0; 6],
            target_adjustment: 0,
            power_offset: 0,
            initial_attenuation: 0,
            clear_tone_after_ready: false,
        });
        assert!(matches!(
            transition.action(),
            PhyTxPowerAction::Complete(PhyTxPowerOutcome {
                calibration_performed: false,
                ..
            })
        ));
    }

    #[test]
    fn external_lowering_covers_every_tx_power_operation_class() {
        let i2c = PhyI2cAddress::new_internal(0x62, 1);
        assert!(matches!(
            PhyTxPowerExternalBinding::lower(PhyTxPowerAction::Environment(
                PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector
            )),
            Ok(PhyTxPowerExternalBinding::Environment(_))
        ));
        assert!(matches!(
            PhyTxPowerExternalBinding::lower(PhyTxPowerAction::Rfpll(
                RfpllFrequencyAction::DelayMicros(5)
            )),
            Ok(PhyTxPowerExternalBinding::Rfpll(_))
        ));
        assert!(matches!(
            PhyTxPowerExternalBinding::lower(PhyTxPowerAction::WriteI2c {
                address: i2c,
                value: 7,
            }),
            Ok(PhyTxPowerExternalBinding::I2c(_))
        ));
        assert!(matches!(
            PhyTxPowerExternalBinding::lower(PhyTxPowerAction::WriteReferenceControl { value: 1 }),
            Ok(PhyTxPowerExternalBinding::Mmio(_))
        ));
        assert!(matches!(
            PhyTxPowerExternalBinding::lower(PhyTxPowerAction::ToneSar(
                PhyToneSarAction::DelayMicros {
                    measurement: 0,
                    sample: 0,
                    phase: crate::phy_tx_cal::PhyToneSarDelayPhase::SarTriggered,
                    micros: 2,
                }
            )),
            Ok(PhyTxPowerExternalBinding::ToneSar(_))
        ));
        assert!(matches!(
            PhyTxPowerExternalBinding::lower(PhyTxPowerAction::Point(
                PhyPowerControlPointAction::StopTone { identity: 1 }
            )),
            Ok(PhyTxPowerExternalBinding::Point(
                PhyPowerControlPointExternalBinding::Mmio(_)
            ))
        ));
        assert!(matches!(
            PhyTxPowerExternalBinding::lower(PhyTxPowerAction::Complete(PhyTxPowerOutcome {
                reference_codes: [0; 2],
                power_curve: [0; 3],
                point_corrections: [0; 3],
                power_adjustment: 0,
                final_attenuation: 0,
                current_channel: 0,
                calibration_performed: false,
            })),
            Err(PhyTxPowerExternalBindingError::UnsupportedAction)
        ));
    }
}
