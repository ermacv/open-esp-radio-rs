//! Rust-owned ESP32-S31 transmit-IQ calibration.
//!
//! The mandatory archive root is `phy_txiq_cal_init`, size 332 bytes. Its
//! algorithmic ROM graph is `phy_rfcal_txiq`, `phy_txiq_cover`,
//! `phy_txiq_get_mis_pwr`, `phy_txtone_linear_pwr`, and the finite register,
//! PBus and I2C leaves named by their actions below. Formatting-only branches
//! are deliberately absent.
//!
//! Every SAR acquisition, two-microsecond interval, PHY-I2C access, PBus
//! command and RFPLL operation is an externally completed edge. The seven
//! cover iterations are explicit state, so advancing this module never polls,
//! sleeps, allocates, or runs an unbounded loop.

use crate::{
    phy_i2c::{
        MaskedI2cWriteAction, MaskedI2cWriteCompletion, MaskedI2cWriteTransition, PhyI2cAddress,
        analog_registers,
    },
    phy_pbus::PhyPbusForceTest,
    phy_pwdet::sar_signal_reference,
    phy_rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyRequest, RfpllFrequencyTransition,
    },
    phy_temperature::{
        PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureFailure,
        PhyTemperatureOutcome, PhyTemperatureTransition,
    },
    phy_tx_cal::{
        PhyPowerAttenuationAction, PhyPowerAttenuationCompletion, PhyPowerAttenuationRequest,
        PhyPowerAttenuationTransition, PhyToneSarAction, PhyToneSarCompletion, PhyToneSarFailure,
        PhyToneSarRequest, PhyToneSarTransition, PhyTxCalibrationEnvironment,
        PhyTxCalibrationEnvironmentAction, PhyTxCalibrationEnvironmentCompletion,
        PhyTxCalibrationEnvironmentFailure, PhyTxCalibrationEnvironmentTransition,
        PhyTxCalibrationParameters,
    },
};

const TX_CAP_ADDRESS: PhyI2cAddress = analog_registers::TX_CAPACITOR_BANKS;
const D_CODE_0_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x62, 0x13);
const D_CODE_1_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x62, 0x14);
const INTERNAL_D_CODE_0_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x62, 0x11);
const INTERNAL_D_CODE_1_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x62, 0x12);
const LOOPBACK_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x67, 0);
const TXIQ_COVER_ITERATIONS: u8 = 7;

const fn clamp_i16(value: i16, low: i16, high: i16) -> i16 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

const fn txiq_coefficient(value: i16, kind: PhyTxIqCoefficientKind) -> i8 {
    match kind {
        PhyTxIqCoefficientKind::Gain => clamp_i16(value, -31, 31) as i8,
        PhyTxIqCoefficientKind::Phase => clamp_i16(value, -63, 63) as i8,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqLinearPowerRequest {
    pub identity: u8,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqLinearPowerOutcome {
    pub identity: u8,
    pub power: i16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLinearPowerAction {
    ToneSar(PhyToneSarAction),
    Complete(PhyTxIqLinearPowerOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLinearPowerCompletion {
    ToneSar(PhyToneSarCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLinearPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinearPowerStep {
    Sample {
        pass: u8,
        transition: PhyToneSarTransition,
    },
    Complete,
    Failed(PhyToneSarFailure),
}

/// Complete bounded translation of ROM `phy_txtone_linear_pwr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqLinearPowerTransition {
    request: PhyTxIqLinearPowerRequest,
    step: LinearPowerStep,
    accumulator: i16,
}

impl PhyTxIqLinearPowerTransition {
    fn sample(request: PhyTxIqLinearPowerRequest, pass: u8) -> PhyToneSarTransition {
        PhyToneSarTransition::new_nonzero(PhyToneSarRequest {
            measurement: request.identity.wrapping_mul(2).wrapping_add(pass),
            samples: 2,
            clear_tone_after_ready: request.clear_tone_after_ready,
        })
    }

    pub fn new(request: PhyTxIqLinearPowerRequest) -> Self {
        Self {
            request,
            step: LinearPowerStep::Sample {
                pass: 0,
                transition: Self::sample(request, 0),
            },
            accumulator: 0,
        }
    }

    pub const fn action(&self) -> PhyTxIqLinearPowerAction {
        match self.step {
            LinearPowerStep::Sample { transition, .. } => {
                PhyTxIqLinearPowerAction::ToneSar(transition.action())
            }
            LinearPowerStep::Complete => {
                PhyTxIqLinearPowerAction::Complete(PhyTxIqLinearPowerOutcome {
                    identity: self.request.identity,
                    power: self.accumulator,
                })
            }
            LinearPowerStep::Failed(failure) => PhyTxIqLinearPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqLinearPowerCompletion,
    ) -> Result<(), PhyTxIqLinearPowerTransitionError> {
        match (self.step, completion) {
            (
                LinearPowerStep::Sample {
                    pass,
                    mut transition,
                },
                PhyTxIqLinearPowerCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqLinearPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        let signal =
                            sar_signal_reference(outcome.sample, self.request.reference_codes);
                        let denominator = if signal[1] == 0 { 1 } else { signal[1] };
                        let ratio = ((i32::from(signal[0]) << 10) / i32::from(denominator)) as i16;
                        self.accumulator = self.accumulator.wrapping_add(ratio);
                        self.step = if pass == 1 {
                            LinearPowerStep::Complete
                        } else {
                            LinearPowerStep::Sample {
                                pass: 1,
                                transition: Self::sample(self.request, 1),
                            }
                        };
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.step = LinearPowerStep::Failed(failure);
                    }
                    _ => self.step = LinearPowerStep::Sample { pass, transition },
                }
            }
            (LinearPowerStep::Complete | LinearPowerStep::Failed(_), _) => {
                return Err(PhyTxIqLinearPowerTransitionError::AlreadyComplete);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerRequest {
    pub identity: u8,
    pub polarity: bool,
    pub attenuation: u8,
    pub selector: u16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerOutcome {
    pub identity: u8,
    pub power: [i16; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerDelayPhase {
    FirstPolarity,
    SecondPolarity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerAction {
    Configure {
        identity: u8,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    },
    DelayMicros {
        identity: u8,
        phase: PhyTxIqMisPowerDelayPhase,
        micros: u32,
    },
    LinearPower(PhyTxIqLinearPowerAction),
    Complete(PhyTxIqMisPowerOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerCompletion {
    Configured {
        identity: u8,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    },
    DelayElapsed {
        identity: u8,
        phase: PhyTxIqMisPowerDelayPhase,
        micros: u32,
    },
    LinearPower(PhyTxIqLinearPowerCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MisPowerStep {
    ConfigureFirst,
    DelayFirst,
    First(PhyTxIqLinearPowerTransition),
    ConfigureSecond,
    DelaySecond,
    Second(PhyTxIqLinearPowerTransition),
    Complete,
    Failed(PhyToneSarFailure),
}

/// Complete translation of ROM `phy_txiq_get_mis_pwr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerTransition {
    request: PhyTxIqMisPowerRequest,
    step: MisPowerStep,
    power: [i16; 2],
}

impl PhyTxIqMisPowerTransition {
    pub const fn new(request: PhyTxIqMisPowerRequest) -> Self {
        Self {
            request,
            step: MisPowerStep::ConfigureFirst,
            power: [0; 2],
        }
    }

    fn linear(&self, phase: u8) -> PhyTxIqLinearPowerTransition {
        PhyTxIqLinearPowerTransition::new(PhyTxIqLinearPowerRequest {
            identity: self.request.identity.wrapping_mul(2).wrapping_add(phase),
            reference_codes: self.request.reference_codes,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
    }

    pub const fn action(&self) -> PhyTxIqMisPowerAction {
        match self.step {
            MisPowerStep::ConfigureFirst => PhyTxIqMisPowerAction::Configure {
                identity: self.request.identity,
                first: true,
                polarity: self.request.polarity,
                attenuation: self.request.attenuation,
                selector: self.request.selector,
            },
            MisPowerStep::DelayFirst => PhyTxIqMisPowerAction::DelayMicros {
                identity: self.request.identity,
                phase: PhyTxIqMisPowerDelayPhase::FirstPolarity,
                micros: 2,
            },
            MisPowerStep::First(transition) | MisPowerStep::Second(transition) => {
                PhyTxIqMisPowerAction::LinearPower(transition.action())
            }
            MisPowerStep::ConfigureSecond => PhyTxIqMisPowerAction::Configure {
                identity: self.request.identity,
                first: false,
                polarity: self.request.polarity,
                attenuation: self.request.attenuation,
                selector: self.request.selector,
            },
            MisPowerStep::DelaySecond => PhyTxIqMisPowerAction::DelayMicros {
                identity: self.request.identity,
                phase: PhyTxIqMisPowerDelayPhase::SecondPolarity,
                micros: 2,
            },
            MisPowerStep::Complete => PhyTxIqMisPowerAction::Complete(PhyTxIqMisPowerOutcome {
                identity: self.request.identity,
                power: self.power,
            }),
            MisPowerStep::Failed(failure) => PhyTxIqMisPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqMisPowerCompletion,
    ) -> Result<(), PhyTxIqMisPowerTransitionError> {
        match (self.step, completion) {
            (
                MisPowerStep::ConfigureFirst,
                PhyTxIqMisPowerCompletion::Configured {
                    identity,
                    first: true,
                    polarity,
                    attenuation,
                    selector,
                },
            ) if identity == self.request.identity
                && polarity == self.request.polarity
                && attenuation == self.request.attenuation
                && selector == self.request.selector =>
            {
                self.step = MisPowerStep::DelayFirst;
            }
            (
                MisPowerStep::DelayFirst,
                PhyTxIqMisPowerCompletion::DelayElapsed {
                    identity,
                    phase: PhyTxIqMisPowerDelayPhase::FirstPolarity,
                    micros: 2,
                },
            ) if identity == self.request.identity => {
                self.step = MisPowerStep::First(self.linear(0));
            }
            (
                MisPowerStep::First(mut transition),
                PhyTxIqMisPowerCompletion::LinearPower(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqMisPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqLinearPowerAction::Complete(outcome) => {
                        self.power[0] = outcome.power;
                        self.step = MisPowerStep::ConfigureSecond;
                    }
                    PhyTxIqLinearPowerAction::Failed(failure) => {
                        self.step = MisPowerStep::Failed(failure);
                    }
                    _ => self.step = MisPowerStep::First(transition),
                }
            }
            (
                MisPowerStep::ConfigureSecond,
                PhyTxIqMisPowerCompletion::Configured {
                    identity,
                    first: false,
                    polarity,
                    attenuation,
                    selector,
                },
            ) if identity == self.request.identity
                && polarity == self.request.polarity
                && attenuation == self.request.attenuation
                && selector == self.request.selector =>
            {
                self.step = MisPowerStep::DelaySecond;
            }
            (
                MisPowerStep::DelaySecond,
                PhyTxIqMisPowerCompletion::DelayElapsed {
                    identity,
                    phase: PhyTxIqMisPowerDelayPhase::SecondPolarity,
                    micros: 2,
                },
            ) if identity == self.request.identity => {
                self.step = MisPowerStep::Second(self.linear(1));
            }
            (
                MisPowerStep::Second(mut transition),
                PhyTxIqMisPowerCompletion::LinearPower(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqMisPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqLinearPowerAction::Complete(outcome) => {
                        self.power[1] = outcome.power;
                        self.step = MisPowerStep::Complete;
                    }
                    PhyTxIqLinearPowerAction::Failed(failure) => {
                        self.step = MisPowerStep::Failed(failure);
                    }
                    _ => self.step = MisPowerStep::Second(transition),
                }
            }
            (MisPowerStep::Complete | MisPowerStep::Failed(_), _) => {
                return Err(PhyTxIqMisPowerTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqMisPowerTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCoefficientKind {
    Gain,
    Phase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCoverRequest {
    pub identity: u8,
    pub attenuation: u8,
    pub selector: u16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCoverOutcome {
    pub identity: u8,
    pub gain: i8,
    pub phase: i8,
    pub iterations: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCoverAction {
    ConfigureCoefficient {
        identity: u8,
        iteration: u8,
        kind: PhyTxIqCoefficientKind,
        value: i8,
    },
    MisPower(PhyTxIqMisPowerAction),
    Complete(PhyTxIqCoverOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCoverCompletion {
    CoefficientConfigured {
        identity: u8,
        iteration: u8,
        kind: PhyTxIqCoefficientKind,
        value: i8,
    },
    MisPower(PhyTxIqMisPowerCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCoverTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CoverStep {
    Gain,
    Phase,
    GainMis(PhyTxIqMisPowerTransition),
    PhaseMis(PhyTxIqMisPowerTransition),
    FinalGain,
    FinalPhase,
    Complete,
    Failed(PhyToneSarFailure),
}

/// Exact seven-iteration coefficient solver from ROM `phy_txiq_cover`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCoverTransition {
    request: PhyTxIqCoverRequest,
    step: CoverStep,
    iteration: u8,
    gain: i8,
    phase: i8,
    gain_accumulator: i8,
    phase_accumulator: i8,
    gain_delta: i8,
}

impl PhyTxIqCoverTransition {
    pub const fn new(request: PhyTxIqCoverRequest) -> Self {
        Self {
            request,
            step: CoverStep::Gain,
            iteration: 0,
            gain: 0,
            phase: 0,
            gain_accumulator: 0,
            phase_accumulator: 0,
            gain_delta: 0,
        }
    }

    const fn mis_identity(&self, phase: u8) -> u8 {
        self.request
            .identity
            .wrapping_mul(16)
            .wrapping_add(self.iteration.wrapping_mul(2))
            .wrapping_add(phase)
    }

    fn mis(&self, phase: u8) -> PhyTxIqMisPowerTransition {
        let gain_path = phase == 0;
        PhyTxIqMisPowerTransition::new(PhyTxIqMisPowerRequest {
            identity: self.mis_identity(phase),
            polarity: gain_path,
            attenuation: if gain_path {
                self.request.attenuation.saturating_sub(12)
            } else {
                self.request.attenuation
            },
            selector: self.request.selector,
            reference_codes: self.request.reference_codes,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
    }

    pub const fn action(&self) -> PhyTxIqCoverAction {
        match self.step {
            CoverStep::Gain | CoverStep::FinalGain => PhyTxIqCoverAction::ConfigureCoefficient {
                identity: self.request.identity,
                iteration: self.iteration,
                kind: PhyTxIqCoefficientKind::Gain,
                value: txiq_coefficient(self.gain as i16, PhyTxIqCoefficientKind::Gain),
            },
            CoverStep::Phase | CoverStep::FinalPhase => PhyTxIqCoverAction::ConfigureCoefficient {
                identity: self.request.identity,
                iteration: self.iteration,
                kind: PhyTxIqCoefficientKind::Phase,
                value: txiq_coefficient(self.phase as i16, PhyTxIqCoefficientKind::Phase),
            },
            CoverStep::GainMis(transition) | CoverStep::PhaseMis(transition) => {
                PhyTxIqCoverAction::MisPower(transition.action())
            }
            CoverStep::Complete => PhyTxIqCoverAction::Complete(PhyTxIqCoverOutcome {
                identity: self.request.identity,
                gain: self.gain,
                phase: self.phase,
                iterations: TXIQ_COVER_ITERATIONS,
            }),
            CoverStep::Failed(failure) => PhyTxIqCoverAction::Failed(failure),
        }
    }

    fn finish_iteration(&mut self, phase_delta: i8) {
        if self.iteration < 3 {
            self.gain = self.gain.wrapping_sub(self.gain_delta);
            self.phase = self.phase.wrapping_sub(phase_delta);
        } else {
            self.gain_accumulator = self.gain_accumulator.wrapping_add(self.gain_delta);
            self.phase_accumulator = self.phase_accumulator.wrapping_add(phase_delta);
            if self.iteration == 6 {
                let gain_average = self.gain_accumulator.wrapping_add(2) >> 2;
                let phase_average = self.phase_accumulator.wrapping_add(2) >> 2;
                self.gain = self.gain.wrapping_sub(gain_average);
                self.phase = self.phase.wrapping_sub(phase_average);
                self.gain = txiq_coefficient(self.gain as i16, PhyTxIqCoefficientKind::Gain);
                self.phase = txiq_coefficient(self.phase as i16, PhyTxIqCoefficientKind::Phase);
                self.step = CoverStep::FinalGain;
                return;
            }
        }
        self.iteration += 1;
        self.step = CoverStep::Gain;
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqCoverCompletion,
    ) -> Result<(), PhyTxIqCoverTransitionError> {
        match (self.step, completion) {
            (
                CoverStep::Gain,
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind: PhyTxIqCoefficientKind::Gain,
                    value,
                },
            ) if identity == self.request.identity
                && iteration == self.iteration
                && value == txiq_coefficient(self.gain as i16, PhyTxIqCoefficientKind::Gain) =>
            {
                self.gain = value;
                self.step = CoverStep::Phase;
            }
            (
                CoverStep::Phase,
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind: PhyTxIqCoefficientKind::Phase,
                    value,
                },
            ) if identity == self.request.identity
                && iteration == self.iteration
                && value == txiq_coefficient(self.phase as i16, PhyTxIqCoefficientKind::Phase) =>
            {
                self.phase = value;
                self.step = CoverStep::GainMis(self.mis(0));
            }
            (CoverStep::GainMis(mut transition), PhyTxIqCoverCompletion::MisPower(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCoverTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqMisPowerAction::Complete(outcome) => {
                        let minimum = i32::from(outcome.power[0].min(outcome.power[1]));
                        let minimum = if minimum == 0 { 1 } else { minimum };
                        let delta = ((i32::from(outcome.power[1]) - i32::from(outcome.power[0]))
                            * 0x800
                            / minimum
                            + 0x10)
                            >> 5;
                        self.gain_delta = delta as i8;
                        self.step = CoverStep::PhaseMis(self.mis(1));
                    }
                    PhyTxIqMisPowerAction::Failed(failure) => {
                        self.step = CoverStep::Failed(failure);
                    }
                    _ => self.step = CoverStep::GainMis(transition),
                }
            }
            (CoverStep::PhaseMis(mut transition), PhyTxIqCoverCompletion::MisPower(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCoverTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqMisPowerAction::Complete(outcome) => {
                        let sum = outcome.power[0].wrapping_add(outcome.power[1]);
                        let denominator = if sum == 0 { 1 } else { sum };
                        let delta = ((i32::from(outcome.power[0]) - i32::from(outcome.power[1]))
                            * 0x1000
                            / i32::from(denominator)
                            + 8)
                            >> 4;
                        self.finish_iteration(delta as i8);
                    }
                    PhyTxIqMisPowerAction::Failed(failure) => {
                        self.step = CoverStep::Failed(failure);
                    }
                    _ => self.step = CoverStep::PhaseMis(transition),
                }
            }
            (
                CoverStep::FinalGain,
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration: 6,
                    kind: PhyTxIqCoefficientKind::Gain,
                    value,
                },
            ) if identity == self.request.identity && value == self.gain => {
                self.step = CoverStep::FinalPhase;
            }
            (
                CoverStep::FinalPhase,
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration: 6,
                    kind: PhyTxIqCoefficientKind::Phase,
                    value,
                },
            ) if identity == self.request.identity && value == self.phase => {
                self.step = CoverStep::Complete;
            }
            (CoverStep::Complete | CoverStep::Failed(_), _) => {
                return Err(PhyTxIqCoverTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqCoverTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLoopbackAction {
    I2c(MaskedI2cWriteAction),
    ConfigureTxClock { enabled: bool },
    ConfigureRxClock { enabled: bool },
    Complete { enabled: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLoopbackCompletion {
    I2c(MaskedI2cWriteCompletion),
    TxClockConfigured { enabled: bool },
    RxClockConfigured { enabled: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqLoopbackTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoopbackStep {
    I2c(MaskedI2cWriteTransition),
    TxClock,
    RxClock,
    Complete,
}

/// Exact three-edge expansion of ROM `phy_loopback_mode_en`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqLoopbackTransition {
    enabled: bool,
    step: LoopbackStep,
}

impl PhyTxIqLoopbackTransition {
    pub fn new(enabled: bool) -> Self {
        let transition = match MaskedI2cWriteTransition::new(LOOPBACK_ADDRESS, 6, 6, enabled as u8)
        {
            Some(transition) => transition,
            None => unreachable!(),
        };
        Self {
            enabled,
            step: LoopbackStep::I2c(transition),
        }
    }

    pub const fn action(&self) -> PhyTxIqLoopbackAction {
        match self.step {
            LoopbackStep::I2c(transition) => PhyTxIqLoopbackAction::I2c(transition.action()),
            LoopbackStep::TxClock => PhyTxIqLoopbackAction::ConfigureTxClock {
                enabled: self.enabled,
            },
            LoopbackStep::RxClock => PhyTxIqLoopbackAction::ConfigureRxClock {
                enabled: self.enabled,
            },
            LoopbackStep::Complete => PhyTxIqLoopbackAction::Complete {
                enabled: self.enabled,
            },
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqLoopbackCompletion,
    ) -> Result<(), PhyTxIqLoopbackTransitionError> {
        match (self.step, completion) {
            (LoopbackStep::I2c(mut transition), PhyTxIqLoopbackCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqLoopbackTransitionError::WrongCompletion)?;
                self.step = if transition.action() == MaskedI2cWriteAction::Complete {
                    LoopbackStep::TxClock
                } else {
                    LoopbackStep::I2c(transition)
                };
            }
            (LoopbackStep::TxClock, PhyTxIqLoopbackCompletion::TxClockConfigured { enabled })
                if enabled == self.enabled =>
            {
                self.step = LoopbackStep::RxClock;
            }
            (LoopbackStep::RxClock, PhyTxIqLoopbackCompletion::RxClockConfigured { enabled })
                if enabled == self.enabled =>
            {
                self.step = LoopbackStep::Complete;
            }
            (LoopbackStep::Complete, _) => {
                return Err(PhyTxIqLoopbackTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqLoopbackTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationVariant {
    Initial,
    Loopback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCalibrationRequest {
    pub identity: u8,
    pub variant: PhyTxIqCalibrationVariant,
    pub environment: PhyTxCalibrationParameters,
    pub attenuation: u8,
    pub selector: u16,
    pub power_offset: i16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCalibrationOutcome {
    pub identity: u8,
    pub coefficient: u16,
    pub gain: i8,
    pub phase: i8,
    pub attenuation: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationFailure {
    Environment(PhyTxCalibrationEnvironmentFailure),
    PbusTimedOut(PhyPbusForceTest),
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationAction {
    ConfigureCorrection { begin: bool },
    ConfigurePbusDebugMode,
    Loopback(PhyTxIqLoopbackAction),
    ForcePbus(PhyPbusForceTest),
    Environment(PhyTxCalibrationEnvironmentAction),
    CaptureToneControl,
    PowerAttenuation(PhyPowerAttenuationAction),
    Cover(PhyTxIqCoverAction),
    RestoreToneControl { saved: u32 },
    Complete(PhyTxIqCalibrationOutcome),
    Failed(PhyTxIqCalibrationFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationCompletion {
    CorrectionConfigured { begin: bool },
    PbusDebugModeConfigured,
    Loopback(PhyTxIqLoopbackCompletion),
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    Environment(PhyTxCalibrationEnvironmentCompletion),
    ToneControlCaptured { value: u32 },
    PowerAttenuation(PhyPowerAttenuationCompletion),
    Cover(PhyTxIqCoverCompletion),
    ToneControlRestored { saved: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CalibrationTerminal {
    Complete,
    Failed(PhyTxIqCalibrationFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CalibrationStep {
    Begin,
    Enter(PhyTxCalibrationEnvironmentTransition),
    PbusDebug,
    EnableLoopback(PhyTxIqLoopbackTransition),
    LoopbackGain {
        index: u8,
    },
    Capture,
    PowerAttenuation(PhyPowerAttenuationTransition),
    Cover(PhyTxIqCoverTransition),
    Exit {
        terminal: CalibrationTerminal,
        transition: PhyTxCalibrationEnvironmentTransition,
    },
    Restore {
        terminal: CalibrationTerminal,
    },
    DisableLoopback {
        terminal: CalibrationTerminal,
        transition: PhyTxIqLoopbackTransition,
    },
    Finish {
        terminal: CalibrationTerminal,
    },
    Complete,
    Failed(PhyTxIqCalibrationFailure),
}

const fn loopback_gain_transaction(index: u8, parameter_002: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 2, 1),
        1 => PhyPbusForceTest::new(4, 1, 0x83),
        2 => PhyPbusForceTest::new(5, 1, 0x1df),
        3 => PhyPbusForceTest::new(0, 1, 0),
        4 => PhyPbusForceTest::new(0, 2, parameter_002 as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x1f9),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

/// Complete heap-free composition of ROM `phy_rfcal_txiq`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqCalibrationTransition {
    request: PhyTxIqCalibrationRequest,
    step: CalibrationStep,
    saved_tone_control: u32,
    tone_control_captured: bool,
    attenuation: u8,
    gain: i8,
    phase: i8,
}

impl PhyTxIqCalibrationTransition {
    pub const fn new(request: PhyTxIqCalibrationRequest) -> Self {
        Self {
            request,
            step: CalibrationStep::Begin,
            saved_tone_control: 0,
            tone_control_captured: false,
            attenuation: request.attenuation,
            gain: 0,
            phase: 0,
        }
    }

    const fn coefficient(&self) -> u16 {
        ((self.gain as u8 as u16 & 0x3f) << 7) | (self.phase as u8 as u16 & 0x7f)
    }

    fn fail(&mut self, failure: PhyTxIqCalibrationFailure) {
        self.step = CalibrationStep::Exit {
            terminal: CalibrationTerminal::Failed(failure),
            transition: PhyTxCalibrationEnvironmentTransition::exit(self.request.environment),
        };
    }

    fn after_exit(&mut self, terminal: CalibrationTerminal) {
        self.step = if self.tone_control_captured {
            CalibrationStep::Restore { terminal }
        } else if self.request.variant == PhyTxIqCalibrationVariant::Loopback {
            CalibrationStep::DisableLoopback {
                terminal,
                transition: PhyTxIqLoopbackTransition::new(false),
            }
        } else {
            CalibrationStep::Finish { terminal }
        };
    }

    pub const fn action(&self) -> PhyTxIqCalibrationAction {
        match self.step {
            CalibrationStep::Begin => PhyTxIqCalibrationAction::ConfigureCorrection { begin: true },
            CalibrationStep::Enter(transition) | CalibrationStep::Exit { transition, .. } => {
                PhyTxIqCalibrationAction::Environment(transition.action())
            }
            CalibrationStep::PbusDebug => PhyTxIqCalibrationAction::ConfigurePbusDebugMode,
            CalibrationStep::EnableLoopback(transition)
            | CalibrationStep::DisableLoopback { transition, .. } => {
                PhyTxIqCalibrationAction::Loopback(transition.action())
            }
            CalibrationStep::LoopbackGain { index } => PhyTxIqCalibrationAction::ForcePbus(
                loopback_gain_transaction(index, self.request.environment.pbus_rx_path_value),
            ),
            CalibrationStep::Capture => PhyTxIqCalibrationAction::CaptureToneControl,
            CalibrationStep::PowerAttenuation(transition) => {
                PhyTxIqCalibrationAction::PowerAttenuation(transition.action())
            }
            CalibrationStep::Cover(transition) => {
                PhyTxIqCalibrationAction::Cover(transition.action())
            }
            CalibrationStep::Restore { .. } => PhyTxIqCalibrationAction::RestoreToneControl {
                saved: self.saved_tone_control,
            },
            CalibrationStep::Finish { .. } => {
                PhyTxIqCalibrationAction::ConfigureCorrection { begin: false }
            }
            CalibrationStep::Complete => {
                PhyTxIqCalibrationAction::Complete(PhyTxIqCalibrationOutcome {
                    identity: self.request.identity,
                    coefficient: self.coefficient(),
                    gain: self.gain,
                    phase: self.phase,
                    attenuation: self.attenuation,
                })
            }
            CalibrationStep::Failed(failure) => PhyTxIqCalibrationAction::Failed(failure),
        }
    }

    fn power_attenuation(&self) -> PhyPowerAttenuationTransition {
        PhyPowerAttenuationTransition::new(PhyPowerAttenuationRequest {
            tone_selector: self.request.selector,
            initial_attenuation: self.request.attenuation,
            target_power: 0x40,
            power_offset: self.request.power_offset,
            reference_codes: self.request.reference_codes,
        })
    }

    fn cover(&self) -> PhyTxIqCoverTransition {
        PhyTxIqCoverTransition::new(PhyTxIqCoverRequest {
            identity: self.request.identity,
            attenuation: self.attenuation,
            selector: self.request.selector,
            reference_codes: self.request.reference_codes,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqCalibrationCompletion,
    ) -> Result<(), PhyTxIqCalibrationTransitionError> {
        match (self.step, completion) {
            (
                CalibrationStep::Begin,
                PhyTxIqCalibrationCompletion::CorrectionConfigured { begin: true },
            ) => {
                self.step = match self.request.variant {
                    PhyTxIqCalibrationVariant::Initial => CalibrationStep::Enter(
                        PhyTxCalibrationEnvironmentTransition::enter(self.request.environment),
                    ),
                    PhyTxIqCalibrationVariant::Loopback => CalibrationStep::PbusDebug,
                };
            }
            (
                CalibrationStep::Enter(mut transition),
                PhyTxIqCalibrationCompletion::Environment(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Debug,
                    ) => self.step = CalibrationStep::Capture,
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.fail(PhyTxIqCalibrationFailure::Environment(failure));
                    }
                    _ => self.step = CalibrationStep::Enter(transition),
                }
            }
            (CalibrationStep::PbusDebug, PhyTxIqCalibrationCompletion::PbusDebugModeConfigured) => {
                self.step = CalibrationStep::EnableLoopback(PhyTxIqLoopbackTransition::new(true));
            }
            (
                CalibrationStep::EnableLoopback(mut transition),
                PhyTxIqCalibrationCompletion::Loopback(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                self.step = if matches!(
                    transition.action(),
                    PhyTxIqLoopbackAction::Complete { enabled: true }
                ) {
                    CalibrationStep::LoopbackGain { index: 0 }
                } else {
                    CalibrationStep::EnableLoopback(transition)
                };
            }
            (
                CalibrationStep::LoopbackGain { index },
                PhyTxIqCalibrationCompletion::PbusCompleted(transaction),
            ) if transaction
                == loopback_gain_transaction(
                    index,
                    self.request.environment.pbus_rx_path_value,
                ) =>
            {
                self.step = if index == 6 {
                    CalibrationStep::Capture
                } else {
                    CalibrationStep::LoopbackGain { index: index + 1 }
                };
            }
            (
                CalibrationStep::LoopbackGain { index },
                PhyTxIqCalibrationCompletion::PbusTimedOut(transaction),
            ) if transaction
                == loopback_gain_transaction(
                    index,
                    self.request.environment.pbus_rx_path_value,
                ) =>
            {
                self.fail(PhyTxIqCalibrationFailure::PbusTimedOut(transaction));
            }
            (
                CalibrationStep::Capture,
                PhyTxIqCalibrationCompletion::ToneControlCaptured { value },
            ) => {
                self.saved_tone_control = value;
                self.tone_control_captured = true;
                self.step = CalibrationStep::PowerAttenuation(self.power_attenuation());
            }
            (
                CalibrationStep::PowerAttenuation(mut transition),
                PhyTxIqCalibrationCompletion::PowerAttenuation(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyPowerAttenuationAction::Complete(outcome) => {
                        self.attenuation = outcome.attenuation;
                        self.step = CalibrationStep::Cover(self.cover());
                    }
                    PhyPowerAttenuationAction::Failed(failure) => {
                        self.fail(PhyTxIqCalibrationFailure::ToneSar(failure));
                    }
                    _ => self.step = CalibrationStep::PowerAttenuation(transition),
                }
            }
            (
                CalibrationStep::Cover(mut transition),
                PhyTxIqCalibrationCompletion::Cover(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqCoverAction::Complete(outcome) => {
                        self.gain =
                            txiq_coefficient(outcome.gain as i16, PhyTxIqCoefficientKind::Gain);
                        self.phase =
                            txiq_coefficient(outcome.phase as i16, PhyTxIqCoefficientKind::Phase);
                        self.step = CalibrationStep::Exit {
                            terminal: CalibrationTerminal::Complete,
                            transition: PhyTxCalibrationEnvironmentTransition::exit(
                                self.request.environment,
                            ),
                        };
                    }
                    PhyTxIqCoverAction::Failed(failure) => {
                        self.fail(PhyTxIqCalibrationFailure::ToneSar(failure));
                    }
                    _ => self.step = CalibrationStep::Cover(transition),
                }
            }
            (
                CalibrationStep::Exit {
                    terminal,
                    mut transition,
                },
                PhyTxIqCalibrationCompletion::Environment(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Work,
                    ) => self.after_exit(terminal),
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.after_exit(CalibrationTerminal::Failed(
                            PhyTxIqCalibrationFailure::Environment(failure),
                        ));
                    }
                    _ => {
                        self.step = CalibrationStep::Exit {
                            terminal,
                            transition,
                        }
                    }
                }
            }
            (
                CalibrationStep::Restore { terminal },
                PhyTxIqCalibrationCompletion::ToneControlRestored { saved },
            ) if saved == self.saved_tone_control => {
                self.step = if self.request.variant == PhyTxIqCalibrationVariant::Loopback {
                    CalibrationStep::DisableLoopback {
                        terminal,
                        transition: PhyTxIqLoopbackTransition::new(false),
                    }
                } else {
                    CalibrationStep::Finish { terminal }
                };
            }
            (
                CalibrationStep::DisableLoopback {
                    terminal,
                    mut transition,
                },
                PhyTxIqCalibrationCompletion::Loopback(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqCalibrationTransitionError::WrongCompletion)?;
                self.step = if matches!(
                    transition.action(),
                    PhyTxIqLoopbackAction::Complete { enabled: false }
                ) {
                    CalibrationStep::Finish { terminal }
                } else {
                    CalibrationStep::DisableLoopback {
                        terminal,
                        transition,
                    }
                };
            }
            (
                CalibrationStep::Finish { terminal },
                PhyTxIqCalibrationCompletion::CorrectionConfigured { begin: false },
            ) => {
                self.step = match terminal {
                    CalibrationTerminal::Complete => CalibrationStep::Complete,
                    CalibrationTerminal::Failed(failure) => CalibrationStep::Failed(failure),
                };
            }
            (CalibrationStep::Complete | CalibrationStep::Failed(_), _) => {
                return Err(PhyTxIqCalibrationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqCalibrationTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqInitParameters {
    pub already_calibrated: bool,
    pub crystal_selector: u8,
    pub environment: PhyTxCalibrationParameters,
    pub capacitance: [u8; 6],
    pub channel_6_dcode: [u8; 2],
    pub initial_attenuation: i8,
    pub power_offset: i16,
    pub reference_codes: [i16; 2],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqInitOutcome {
    pub coefficient: [u16; 2],
    pub external_dcode: [u8; 2],
    pub temperature: Option<PhyTemperatureOutcome>,
    pub calibration_performed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqInitFailure {
    Rfpll(RfpllFrequencyFailure),
    Calibration(PhyTxIqCalibrationFailure),
    Temperature(PhyTemperatureFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqInitAction {
    Rfpll(RfpllFrequencyAction),
    WriteI2c {
        address: PhyI2cAddress,
        value: u8,
    },
    ReadI2cMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    Calibration(PhyTxIqCalibrationAction),
    Temperature(PhyTemperatureAction),
    Complete(PhyTxIqInitOutcome),
    Failed(PhyTxIqInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqInitCompletion {
    Rfpll(RfpllFrequencyCompletion),
    I2cWritten {
        address: PhyI2cAddress,
        value: u8,
    },
    I2cMaskedRead {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    Calibration(PhyTxIqCalibrationCompletion),
    Temperature(PhyTemperatureCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitStep {
    Rfpll(RfpllFrequencyTransition),
    TxCap,
    Dcode0,
    Dcode1,
    ReadDcodeMode,
    ReadDcode0 {
        external: bool,
    },
    ReadDcode1 {
        external: bool,
    },
    Calibration {
        phase: u8,
        transition: PhyTxIqCalibrationTransition,
    },
    Temperature(PhyTemperatureTransition),
    Complete,
    Failed(PhyTxIqInitFailure),
}

/// Complete Rust-owned composition of archive `phy_txiq_cal_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxIqInitTransition {
    parameters: PhyTxIqInitParameters,
    step: InitStep,
    attenuation: u8,
    coefficient: [u16; 2],
    external_dcode: [u8; 2],
    temperature: Option<PhyTemperatureOutcome>,
}

impl PhyTxIqInitTransition {
    pub const fn new(parameters: PhyTxIqInitParameters) -> Self {
        let attenuation = clamp_i16(parameters.initial_attenuation as i16, 0, 0x78) as u8;
        let step = if parameters.already_calibrated {
            InitStep::Complete
        } else {
            InitStep::Rfpll(RfpllFrequencyTransition::new(RfpllFrequencyRequest {
                crystal_selector: parameters.crystal_selector,
                frequency_code: 0x985,
                offset: 0,
            }))
        };
        Self {
            parameters,
            step,
            attenuation,
            coefficient: [0; 2],
            external_dcode: [0; 2],
            temperature: None,
        }
    }

    const fn outcome(&self) -> PhyTxIqInitOutcome {
        PhyTxIqInitOutcome {
            coefficient: self.coefficient,
            external_dcode: self.external_dcode,
            temperature: self.temperature,
            calibration_performed: !self.parameters.already_calibrated,
        }
    }

    fn calibration(&self, phase: u8) -> PhyTxIqCalibrationTransition {
        PhyTxIqCalibrationTransition::new(PhyTxIqCalibrationRequest {
            identity: phase,
            variant: if phase == 0 {
                PhyTxIqCalibrationVariant::Initial
            } else {
                PhyTxIqCalibrationVariant::Loopback
            },
            environment: self.parameters.environment,
            attenuation: if phase == 0 {
                self.attenuation
            } else {
                self.attenuation.saturating_sub(0x28)
            },
            selector: 0x80,
            power_offset: self.parameters.power_offset,
            reference_codes: self.parameters.reference_codes,
            clear_tone_after_ready: self.parameters.clear_tone_after_ready,
        })
    }

    pub const fn action(&self) -> PhyTxIqInitAction {
        match self.step {
            InitStep::Rfpll(transition) => PhyTxIqInitAction::Rfpll(transition.action()),
            InitStep::TxCap => PhyTxIqInitAction::WriteI2c {
                address: TX_CAP_ADDRESS,
                value: self.parameters.capacitance[2] | 0xc0,
            },
            InitStep::Dcode0 => PhyTxIqInitAction::WriteI2c {
                address: D_CODE_0_ADDRESS,
                value: (self.parameters.channel_6_dcode[0] & 0x3f) | 0x40,
            },
            InitStep::Dcode1 => PhyTxIqInitAction::WriteI2c {
                address: D_CODE_1_ADDRESS,
                value: (self.parameters.channel_6_dcode[1] & 0x3f) | 0x40,
            },
            InitStep::ReadDcodeMode => PhyTxIqInitAction::ReadI2cMasked {
                address: D_CODE_0_ADDRESS,
                high_bit: 6,
                low_bit: 6,
            },
            InitStep::ReadDcode0 { external } => PhyTxIqInitAction::ReadI2cMasked {
                address: if external {
                    D_CODE_0_ADDRESS
                } else {
                    INTERNAL_D_CODE_0_ADDRESS
                },
                high_bit: 5,
                low_bit: 0,
            },
            InitStep::ReadDcode1 { external } => PhyTxIqInitAction::ReadI2cMasked {
                address: if external {
                    D_CODE_1_ADDRESS
                } else {
                    INTERNAL_D_CODE_1_ADDRESS
                },
                high_bit: 5,
                low_bit: 0,
            },
            InitStep::Calibration { transition, .. } => {
                PhyTxIqInitAction::Calibration(transition.action())
            }
            InitStep::Temperature(transition) => {
                PhyTxIqInitAction::Temperature(transition.action())
            }
            InitStep::Complete => PhyTxIqInitAction::Complete(self.outcome()),
            InitStep::Failed(failure) => PhyTxIqInitAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxIqInitCompletion,
    ) -> Result<(), PhyTxIqInitTransitionError> {
        match (self.step, completion) {
            (InitStep::Rfpll(mut transition), PhyTxIqInitCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => self.step = InitStep::TxCap,
                    RfpllFrequencyAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyTxIqInitFailure::Rfpll(failure));
                    }
                    _ => self.step = InitStep::Rfpll(transition),
                }
            }
            (
                InitStep::TxCap,
                PhyTxIqInitCompletion::I2cWritten {
                    address: TX_CAP_ADDRESS,
                    value,
                },
            ) if value == self.parameters.capacitance[2] | 0xc0 => {
                self.step = if self.parameters.channel_6_dcode[0] != 0
                    && self.parameters.channel_6_dcode[1] != 0
                {
                    InitStep::Dcode0
                } else {
                    InitStep::ReadDcodeMode
                };
            }
            (
                InitStep::Dcode0,
                PhyTxIqInitCompletion::I2cWritten {
                    address: D_CODE_0_ADDRESS,
                    value,
                },
            ) if value == (self.parameters.channel_6_dcode[0] & 0x3f) | 0x40 => {
                self.step = InitStep::Dcode1;
            }
            (
                InitStep::Dcode1,
                PhyTxIqInitCompletion::I2cWritten {
                    address: D_CODE_1_ADDRESS,
                    value,
                },
            ) if value == (self.parameters.channel_6_dcode[1] & 0x3f) | 0x40 => {
                self.step = InitStep::ReadDcodeMode;
            }
            (
                InitStep::ReadDcodeMode,
                PhyTxIqInitCompletion::I2cMaskedRead {
                    address: D_CODE_0_ADDRESS,
                    high_bit: 6,
                    low_bit: 6,
                    value,
                },
            ) => {
                self.step = InitStep::ReadDcode0 {
                    external: value & 1 != 0,
                };
            }
            (
                InitStep::ReadDcode0 { external },
                PhyTxIqInitCompletion::I2cMaskedRead {
                    address,
                    high_bit: 5,
                    low_bit: 0,
                    value,
                },
            ) if address
                == if external {
                    D_CODE_0_ADDRESS
                } else {
                    INTERNAL_D_CODE_0_ADDRESS
                } =>
            {
                self.external_dcode[0] = value & 0x3f;
                self.step = InitStep::ReadDcode1 { external };
            }
            (
                InitStep::ReadDcode1 { external },
                PhyTxIqInitCompletion::I2cMaskedRead {
                    address,
                    high_bit: 5,
                    low_bit: 0,
                    value,
                },
            ) if address
                == if external {
                    D_CODE_1_ADDRESS
                } else {
                    INTERNAL_D_CODE_1_ADDRESS
                } =>
            {
                self.external_dcode[1] = value & 0x3f;
                self.step = InitStep::Calibration {
                    phase: 0,
                    transition: self.calibration(0),
                };
            }
            (
                InitStep::Calibration {
                    phase,
                    mut transition,
                },
                PhyTxIqInitCompletion::Calibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxIqCalibrationAction::Complete(outcome) => {
                        self.coefficient[phase as usize] = outcome.coefficient;
                        if phase == 0 {
                            self.step = InitStep::Temperature(PhyTemperatureTransition::new());
                        } else {
                            self.step = InitStep::Complete;
                        }
                    }
                    PhyTxIqCalibrationAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyTxIqInitFailure::Calibration(failure));
                    }
                    _ => self.step = InitStep::Calibration { phase, transition },
                }
            }
            (
                InitStep::Temperature(mut transition),
                PhyTxIqInitCompletion::Temperature(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxIqInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTemperatureAction::Complete(outcome) => {
                        self.temperature = Some(outcome);
                        self.step = InitStep::Calibration {
                            phase: 1,
                            transition: self.calibration(1),
                        };
                    }
                    PhyTemperatureAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyTxIqInitFailure::Temperature(failure));
                    }
                    _ => self.step = InitStep::Temperature(transition),
                }
            }
            (InitStep::Complete | InitStep::Failed(_), _) => {
                return Err(PhyTxIqInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxIqInitTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqBindingError {
    NotDirectMmio,
}

/// Non-cloneable direct-MMIO token for TXIQ-specific register edges.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqMmioBinding {
    action: PhyTxIqCalibrationAction,
}

impl PhyTxIqMmioBinding {
    pub fn new(action: PhyTxIqCalibrationAction) -> Result<Self, PhyTxIqBindingError> {
        match action {
            PhyTxIqCalibrationAction::ConfigureCorrection { .. }
            | PhyTxIqCalibrationAction::ConfigurePbusDebugMode
            | PhyTxIqCalibrationAction::CaptureToneControl
            | PhyTxIqCalibrationAction::RestoreToneControl { .. } => Ok(Self { action }),
            _ => Err(PhyTxIqBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyTxIqCalibrationAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> PhyTxIqCalibrationCompletion {
        match self.action {
            PhyTxIqCalibrationAction::ConfigureCorrection { begin } => {
                crate::radio_hal::configure_phy_txiq_correction(registers, begin);
                PhyTxIqCalibrationCompletion::CorrectionConfigured { begin }
            }
            PhyTxIqCalibrationAction::ConfigurePbusDebugMode => {
                open_esp_radio_esp32s31_hal::pbus::configure_debug_mode(registers);
                PhyTxIqCalibrationCompletion::PbusDebugModeConfigured
            }
            PhyTxIqCalibrationAction::CaptureToneControl => {
                PhyTxIqCalibrationCompletion::ToneControlCaptured {
                    value: crate::radio_hal::read_phy_txiq_tone_control(registers),
                }
            }
            PhyTxIqCalibrationAction::RestoreToneControl { saved } => {
                crate::radio_hal::restore_phy_txiq_tone_control(registers, saved);
                PhyTxIqCalibrationCompletion::ToneControlRestored { saved }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerMmioBinding {
    action: PhyTxIqMisPowerAction,
}

impl PhyTxIqMisPowerMmioBinding {
    pub fn new(action: PhyTxIqMisPowerAction) -> Result<Self, PhyTxIqBindingError> {
        match action {
            PhyTxIqMisPowerAction::Configure { .. } => Ok(Self { action }),
            _ => Err(PhyTxIqBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> PhyTxIqMisPowerCompletion {
        match self.action {
            PhyTxIqMisPowerAction::Configure {
                identity,
                first,
                polarity,
                attenuation,
                selector,
            } => {
                crate::radio_hal::configure_phy_txiq_mis_power(
                    registers,
                    first,
                    polarity,
                    attenuation,
                    selector,
                );
                PhyTxIqMisPowerCompletion::Configured {
                    identity,
                    first,
                    polarity,
                    attenuation,
                    selector,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqCoverMmioBinding {
    action: PhyTxIqCoverAction,
}

impl PhyTxIqCoverMmioBinding {
    pub fn new(action: PhyTxIqCoverAction) -> Result<Self, PhyTxIqBindingError> {
        match action {
            PhyTxIqCoverAction::ConfigureCoefficient { .. } => Ok(Self { action }),
            _ => Err(PhyTxIqBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> PhyTxIqCoverCompletion {
        match self.action {
            PhyTxIqCoverAction::ConfigureCoefficient {
                identity,
                iteration,
                kind,
                value,
            } => {
                crate::radio_hal::configure_phy_txiq_coefficient(registers, kind, value);
                PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind,
                    value,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqLoopbackMmioBinding {
    action: PhyTxIqLoopbackAction,
}

impl PhyTxIqLoopbackMmioBinding {
    pub fn new(action: PhyTxIqLoopbackAction) -> Result<Self, PhyTxIqBindingError> {
        match action {
            PhyTxIqLoopbackAction::ConfigureTxClock { .. }
            | PhyTxIqLoopbackAction::ConfigureRxClock { .. } => Ok(Self { action }),
            _ => Err(PhyTxIqBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> PhyTxIqLoopbackCompletion {
        match self.action {
            PhyTxIqLoopbackAction::ConfigureTxClock { enabled } => {
                open_esp_radio_esp32s31_hal::pbus::configure_tx_clock(registers, enabled);
                PhyTxIqLoopbackCompletion::TxClockConfigured { enabled }
            }
            PhyTxIqLoopbackAction::ConfigureRxClock { enabled } => {
                open_esp_radio_esp32s31_hal::pbus::configure_rx_clock(registers, enabled);
                PhyTxIqLoopbackCompletion::RxClockConfigured { enabled }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxIqExternalBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
    Pbus(crate::phy_pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqLinearPowerExternalBinding {
    ToneSar(crate::phy_tx_cal::PhyToneSarExternalBinding),
}

impl PhyTxIqLinearPowerExternalBinding {
    pub fn lower(action: PhyTxIqLinearPowerAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqLinearPowerAction::ToneSar(action) => {
                crate::phy_tx_cal::PhyToneSarExternalBinding::lower(action)
                    .map(Self::ToneSar)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqLinearPowerAction::Complete(_) | PhyTxIqLinearPowerAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqMisPowerTimerBinding {
    identity: u8,
    phase: PhyTxIqMisPowerDelayPhase,
    micros: u32,
}

impl PhyTxIqMisPowerTimerBinding {
    pub fn new(action: PhyTxIqMisPowerAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqMisPowerAction::DelayMicros {
                identity,
                phase,
                micros,
            } => Ok(Self {
                identity,
                phase,
                micros,
            }),
            _ => Err(PhyTxIqExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxIqMisPowerCompletion {
        PhyTxIqMisPowerCompletion::DelayElapsed {
            identity: self.identity,
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqMisPowerExternalBinding {
    Mmio(PhyTxIqMisPowerMmioBinding),
    Timer(PhyTxIqMisPowerTimerBinding),
    LinearPower(PhyTxIqLinearPowerExternalBinding),
}

impl PhyTxIqMisPowerExternalBinding {
    pub fn lower(action: PhyTxIqMisPowerAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqMisPowerAction::Configure { .. } => PhyTxIqMisPowerMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction),
            PhyTxIqMisPowerAction::DelayMicros { .. } => {
                PhyTxIqMisPowerTimerBinding::new(action).map(Self::Timer)
            }
            PhyTxIqMisPowerAction::LinearPower(action) => {
                PhyTxIqLinearPowerExternalBinding::lower(action).map(Self::LinearPower)
            }
            PhyTxIqMisPowerAction::Complete(_) | PhyTxIqMisPowerAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqCoverExternalBinding {
    Mmio(PhyTxIqCoverMmioBinding),
    MisPower(PhyTxIqMisPowerExternalBinding),
}

impl PhyTxIqCoverExternalBinding {
    pub fn lower(action: PhyTxIqCoverAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqCoverAction::ConfigureCoefficient { .. } => PhyTxIqCoverMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction),
            PhyTxIqCoverAction::MisPower(action) => {
                PhyTxIqMisPowerExternalBinding::lower(action).map(Self::MisPower)
            }
            PhyTxIqCoverAction::Complete(_) | PhyTxIqCoverAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqLoopbackExternalBinding {
    I2c(crate::phy_i2c::MaskedI2cWriteBinding),
    Mmio(PhyTxIqLoopbackMmioBinding),
}

impl PhyTxIqLoopbackExternalBinding {
    pub fn lower(action: PhyTxIqLoopbackAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqLoopbackAction::I2c(action) => {
                crate::phy_i2c::MaskedI2cWriteBinding::new(action)
                    .map(Self::I2c)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqLoopbackAction::ConfigureTxClock { .. }
            | PhyTxIqLoopbackAction::ConfigureRxClock { .. } => {
                PhyTxIqLoopbackMmioBinding::new(action)
                    .map(Self::Mmio)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqLoopbackAction::Complete { .. } => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::phy_pbus::PhyPbusHardwareBinding,
}

impl PhyTxIqPbusBinding {
    pub fn new(action: PhyTxIqCalibrationAction) -> Result<Self, PhyTxIqExternalBindingError> {
        let PhyTxIqCalibrationAction::ForcePbus(transaction) = action else {
            return Err(PhyTxIqExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
            transaction,
            hardware: crate::phy_pbus::PhyPbusHardwareBinding::new(transaction),
        })
    }

    pub const fn action(&self) -> crate::phy_pbus::PhyPbusHardwareAction {
        self.hardware.action()
    }

    pub fn started(&mut self) -> Result<(), crate::phy_pbus::PhyPbusHardwareBindingError> {
        self.hardware.started()
    }

    pub fn observe_completed(
        &mut self,
        completed: bool,
    ) -> Result<
        crate::phy_pbus::PhyPbusHardwareObservation,
        crate::phy_pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_completed(completed)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> Result<(), crate::phy_pbus::PhyPbusHardwareBindingError> {
        self.hardware.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> Result<
        crate::phy_pbus::PhyPbusHardwareObservation,
        crate::phy_pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyTxIqCalibrationCompletion, PhyTxIqExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxIqCalibrationCompletion::PbusCompleted)
            .map_err(PhyTxIqExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyTxIqCalibrationCompletion {
        PhyTxIqCalibrationCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqCalibrationExternalBinding {
    Mmio(PhyTxIqMmioBinding),
    Loopback(PhyTxIqLoopbackExternalBinding),
    Pbus(PhyTxIqPbusBinding),
    Environment(crate::phy_tx_cal::PhyTxCalibrationEnvironmentExternalBinding),
    PowerAttenuation(crate::phy_tx_cal::PhyPowerAttenuationExternalBinding),
    Cover(PhyTxIqCoverExternalBinding),
}

impl PhyTxIqCalibrationExternalBinding {
    pub fn lower(action: PhyTxIqCalibrationAction) -> Result<Self, PhyTxIqExternalBindingError> {
        if let Ok(binding) = PhyTxIqMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        match action {
            PhyTxIqCalibrationAction::Loopback(action) => {
                PhyTxIqLoopbackExternalBinding::lower(action).map(Self::Loopback)
            }
            PhyTxIqCalibrationAction::ForcePbus(_) => {
                PhyTxIqPbusBinding::new(action).map(Self::Pbus)
            }
            PhyTxIqCalibrationAction::Environment(action) => {
                crate::phy_tx_cal::PhyTxCalibrationEnvironmentExternalBinding::lower(action)
                    .map(Self::Environment)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqCalibrationAction::PowerAttenuation(action) => {
                crate::phy_tx_cal::PhyPowerAttenuationExternalBinding::lower(action)
                    .map(Self::PowerAttenuation)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqCalibrationAction::Cover(action) => {
                PhyTxIqCoverExternalBinding::lower(action).map(Self::Cover)
            }
            PhyTxIqCalibrationAction::Complete(_) | PhyTxIqCalibrationAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
            _ => Err(PhyTxIqExternalBindingError::UnsupportedAction),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxIqInitI2cBinding {
    outer_action: PhyTxIqInitAction,
    transaction: crate::phy_cold::PhyColdI2cTransaction,
}

impl PhyTxIqInitI2cBinding {
    pub fn new(action: PhyTxIqInitAction) -> Result<Self, PhyTxIqExternalBindingError> {
        let request = match action {
            PhyTxIqInitAction::WriteI2c { address, value } => {
                crate::phy_cold::PhyColdI2cRequest::write_byte(address, value)
            }
            PhyTxIqInitAction::ReadI2cMasked {
                address,
                high_bit,
                low_bit,
            } => crate::phy_cold::PhyColdI2cRequest::read_masked(address, high_bit, low_bit)
                .ok_or(PhyTxIqExternalBindingError::UnsupportedAction)?,
            _ => return Err(PhyTxIqExternalBindingError::UnsupportedAction),
        };
        Ok(Self {
            outer_action: action,
            transaction: crate::phy_cold::PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn action(&self) -> crate::phy_cold::PhyColdI2cAction {
        self.transaction.action()
    }

    pub fn read_started(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    pub fn write_started(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, crate::phy_i2c::PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_read_result(result)
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

    pub fn into_completion(self) -> Result<PhyTxIqInitCompletion, PhyTxIqExternalBindingError> {
        let crate::phy_cold::PhyColdI2cAction::Complete(outcome) = self.transaction.action() else {
            return Err(PhyTxIqExternalBindingError::IncompleteTransaction);
        };
        match (self.outer_action, outcome) {
            (
                PhyTxIqInitAction::WriteI2c { address, value },
                crate::phy_cold::PhyColdI2cOutcome::Written { address: completed },
            ) if completed == address => Ok(PhyTxIqInitCompletion::I2cWritten { address, value }),
            (
                PhyTxIqInitAction::ReadI2cMasked {
                    address,
                    high_bit,
                    low_bit,
                },
                crate::phy_cold::PhyColdI2cOutcome::Read {
                    address: completed,
                    value,
                },
            ) if completed == address => Ok(PhyTxIqInitCompletion::I2cMaskedRead {
                address,
                high_bit,
                low_bit,
                value,
            }),
            _ => Err(PhyTxIqExternalBindingError::UnexpectedOutcome),
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_txiq_cal_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxIqInitExternalBinding {
    Rfpll(crate::phy_rfpll::RfpllFrequencyExternalBinding),
    I2c(PhyTxIqInitI2cBinding),
    Calibration(PhyTxIqCalibrationExternalBinding),
    Temperature(crate::phy_temperature::PhyTemperatureExternalBinding),
}

impl PhyTxIqInitExternalBinding {
    pub fn lower(action: PhyTxIqInitAction) -> Result<Self, PhyTxIqExternalBindingError> {
        match action {
            PhyTxIqInitAction::Rfpll(action) => {
                crate::phy_rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqInitAction::WriteI2c { .. } | PhyTxIqInitAction::ReadI2cMasked { .. } => {
                PhyTxIqInitI2cBinding::new(action).map(Self::I2c)
            }
            PhyTxIqInitAction::Calibration(action) => {
                PhyTxIqCalibrationExternalBinding::lower(action).map(Self::Calibration)
            }
            PhyTxIqInitAction::Temperature(action) => {
                crate::phy_temperature::PhyTemperatureExternalBinding::lower(action)
                    .map(Self::Temperature)
                    .map_err(|_| PhyTxIqExternalBindingError::UnsupportedAction)
            }
            PhyTxIqInitAction::Complete(_) | PhyTxIqInitAction::Failed(_) => {
                Err(PhyTxIqExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_completion(action: PhyToneSarAction, sample_value: u16) -> PhyToneSarCompletion {
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
            } => PhyToneSarCompletion::ReadySampled {
                measurement,
                sample,
                ready: true,
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
            } => PhyToneSarCompletion::SarRead {
                measurement,
                sample,
                value: sample_value,
            },
            terminal => panic!("unexpected tone terminal: {terminal:?}"),
        }
    }

    fn linear_completion(
        action: PhyTxIqLinearPowerAction,
        sample: u16,
    ) -> PhyTxIqLinearPowerCompletion {
        match action {
            PhyTxIqLinearPowerAction::ToneSar(action) => {
                PhyTxIqLinearPowerCompletion::ToneSar(tone_completion(action, sample))
            }
            terminal => panic!("unexpected linear terminal: {terminal:?}"),
        }
    }

    fn mis_completion(action: PhyTxIqMisPowerAction, sample: u16) -> PhyTxIqMisPowerCompletion {
        match action {
            PhyTxIqMisPowerAction::Configure {
                identity,
                first,
                polarity,
                attenuation,
                selector,
            } => PhyTxIqMisPowerCompletion::Configured {
                identity,
                first,
                polarity,
                attenuation,
                selector,
            },
            PhyTxIqMisPowerAction::DelayMicros {
                identity,
                phase,
                micros,
            } => PhyTxIqMisPowerCompletion::DelayElapsed {
                identity,
                phase,
                micros,
            },
            PhyTxIqMisPowerAction::LinearPower(action) => {
                PhyTxIqMisPowerCompletion::LinearPower(linear_completion(action, sample))
            }
            terminal => panic!("unexpected mis-power terminal: {terminal:?}"),
        }
    }

    fn cover_completion(action: PhyTxIqCoverAction, sample: u16) -> PhyTxIqCoverCompletion {
        match action {
            PhyTxIqCoverAction::ConfigureCoefficient {
                identity,
                iteration,
                kind,
                value,
            } => PhyTxIqCoverCompletion::CoefficientConfigured {
                identity,
                iteration,
                kind,
                value,
            },
            PhyTxIqCoverAction::MisPower(action) => {
                PhyTxIqCoverCompletion::MisPower(mis_completion(action, sample))
            }
            terminal => panic!("unexpected cover terminal: {terminal:?}"),
        }
    }

    fn environment_completion(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> PhyTxCalibrationEnvironmentCompletion {
        match action {
            PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode => {
                PhyTxCalibrationEnvironmentCompletion::PbusDebugModeConfigured
            }
            PhyTxCalibrationEnvironmentAction::ForcePbus(transaction) => {
                PhyTxCalibrationEnvironmentCompletion::PbusCompleted(transaction)
            }
            PhyTxCalibrationEnvironmentAction::ConfigureTxClock { enabled } => {
                PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled }
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector => {
                PhyTxCalibrationEnvironmentCompletion::PowerDetectorConfigured
            }
            PhyTxCalibrationEnvironmentAction::ConfigureCalibrationMode => {
                PhyTxCalibrationEnvironmentCompletion::CalibrationModeConfigured
            }
            PhyTxCalibrationEnvironmentAction::StopTone => {
                PhyTxCalibrationEnvironmentCompletion::ToneStopped
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkMode => {
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                }
            }
            PhyTxCalibrationEnvironmentAction::DelayMicros { phase, micros } => {
                PhyTxCalibrationEnvironmentCompletion::DelayElapsed { phase, micros }
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkModePulse => {
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseConfigured
            }
            PhyTxCalibrationEnvironmentAction::ClearPbusWorkModePulse => {
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseCleared
            }
            terminal => panic!("unexpected environment terminal: {terminal:?}"),
        }
    }

    fn loopback_completion(action: PhyTxIqLoopbackAction) -> PhyTxIqLoopbackCompletion {
        match action {
            PhyTxIqLoopbackAction::I2c(MaskedI2cWriteAction::ReadByte { address }) => {
                PhyTxIqLoopbackCompletion::I2c(MaskedI2cWriteCompletion::I2cReadCompleted {
                    address,
                    value: 0,
                })
            }
            PhyTxIqLoopbackAction::I2c(MaskedI2cWriteAction::WriteByte { address, .. }) => {
                PhyTxIqLoopbackCompletion::I2c(MaskedI2cWriteCompletion::I2cWriteCompleted {
                    address,
                })
            }
            PhyTxIqLoopbackAction::ConfigureTxClock { enabled } => {
                PhyTxIqLoopbackCompletion::TxClockConfigured { enabled }
            }
            PhyTxIqLoopbackAction::ConfigureRxClock { enabled } => {
                PhyTxIqLoopbackCompletion::RxClockConfigured { enabled }
            }
            terminal => panic!("unexpected loopback terminal: {terminal:?}"),
        }
    }

    fn power_attenuation_completion(
        action: PhyPowerAttenuationAction,
        sample: u16,
    ) -> PhyPowerAttenuationCompletion {
        match action {
            PhyPowerAttenuationAction::ConfigureTone {
                iteration,
                selector,
                attenuation,
            } => PhyPowerAttenuationCompletion::ToneConfigured {
                iteration,
                selector,
                attenuation,
            },
            PhyPowerAttenuationAction::ToneSar(action) => {
                PhyPowerAttenuationCompletion::ToneSar(tone_completion(action, sample))
            }
            terminal => panic!("unexpected attenuation terminal: {terminal:?}"),
        }
    }

    fn calibration_completion(
        action: PhyTxIqCalibrationAction,
        sample: u16,
    ) -> PhyTxIqCalibrationCompletion {
        match action {
            PhyTxIqCalibrationAction::ConfigureCorrection { begin } => {
                PhyTxIqCalibrationCompletion::CorrectionConfigured { begin }
            }
            PhyTxIqCalibrationAction::ConfigurePbusDebugMode => {
                PhyTxIqCalibrationCompletion::PbusDebugModeConfigured
            }
            PhyTxIqCalibrationAction::Loopback(action) => {
                PhyTxIqCalibrationCompletion::Loopback(loopback_completion(action))
            }
            PhyTxIqCalibrationAction::ForcePbus(transaction) => {
                PhyTxIqCalibrationCompletion::PbusCompleted(transaction)
            }
            PhyTxIqCalibrationAction::Environment(action) => {
                PhyTxIqCalibrationCompletion::Environment(environment_completion(action))
            }
            PhyTxIqCalibrationAction::CaptureToneControl => {
                PhyTxIqCalibrationCompletion::ToneControlCaptured { value: 0xa5a5_5a5a }
            }
            PhyTxIqCalibrationAction::PowerAttenuation(action) => {
                PhyTxIqCalibrationCompletion::PowerAttenuation(power_attenuation_completion(
                    action, sample,
                ))
            }
            PhyTxIqCalibrationAction::Cover(action) => {
                PhyTxIqCalibrationCompletion::Cover(cover_completion(action, sample))
            }
            PhyTxIqCalibrationAction::RestoreToneControl { saved } => {
                PhyTxIqCalibrationCompletion::ToneControlRestored { saved }
            }
            terminal => panic!("unexpected calibration terminal: {terminal:?}"),
        }
    }

    #[test]
    fn linear_power_has_exact_four_sar_samples_and_wrapping_sum() {
        let request = PhyTxIqLinearPowerRequest {
            identity: 2,
            reference_codes: [80, 120],
            clear_tone_after_ready: false,
        };
        let mut transition = PhyTxIqLinearPowerTransition::new(request);
        let mut reads = 0;
        loop {
            let action = transition.action();
            if let PhyTxIqLinearPowerAction::Complete(outcome) = action {
                assert_eq!(outcome.identity, 2);
                assert!(outcome.power > 0);
                break;
            }
            if matches!(
                action,
                PhyTxIqLinearPowerAction::ToneSar(PhyToneSarAction::ReadSar { .. })
            ) {
                reads += 1;
            }
            transition.advance(linear_completion(action, 100)).unwrap();
        }
        assert_eq!(reads, 4);
    }

    #[test]
    fn cover_has_seven_iterations_and_exact_112_sar_samples() {
        let request = PhyTxIqCoverRequest {
            identity: 3,
            attenuation: 80,
            selector: 0x80,
            reference_codes: [80, 120],
            clear_tone_after_ready: false,
        };
        let mut transition = PhyTxIqCoverTransition::new(request);
        let mut reads = 0;
        loop {
            let action = transition.action();
            let completion = match action {
                PhyTxIqCoverAction::ConfigureCoefficient {
                    identity,
                    iteration,
                    kind,
                    value,
                } => PhyTxIqCoverCompletion::CoefficientConfigured {
                    identity,
                    iteration,
                    kind,
                    value,
                },
                PhyTxIqCoverAction::MisPower(action) => {
                    if matches!(
                        action,
                        PhyTxIqMisPowerAction::LinearPower(PhyTxIqLinearPowerAction::ToneSar(
                            PhyToneSarAction::ReadSar { .. }
                        ))
                    ) {
                        reads += 1;
                    }
                    PhyTxIqCoverCompletion::MisPower(mis_completion(action, 100))
                }
                PhyTxIqCoverAction::Complete(outcome) => {
                    assert_eq!(outcome.iterations, 7);
                    break;
                }
                PhyTxIqCoverAction::Failed(failure) => {
                    panic!("unexpected failure: {failure:?}")
                }
            };
            transition.advance(completion).unwrap();
        }
        assert_eq!(reads, 112);
    }

    #[test]
    fn both_rfcal_variants_traverse_cleanup_and_finish_with_bounded_coefficients() {
        for variant in [
            PhyTxIqCalibrationVariant::Initial,
            PhyTxIqCalibrationVariant::Loopback,
        ] {
            let mut transition = PhyTxIqCalibrationTransition::new(PhyTxIqCalibrationRequest {
                identity: variant as u8,
                variant,
                environment: PhyTxCalibrationParameters {
                    pbus_tx_path_value: 0x2f,
                    pbus_rx_path_value: 0xbf,
                    dco: [0x100; 4],
                },
                attenuation: 80,
                selector: 0x80,
                power_offset: 0,
                reference_codes: [80, 120],
                clear_tone_after_ready: false,
            });
            let mut edges = 0_u16;
            loop {
                let action = transition.action();
                match action {
                    PhyTxIqCalibrationAction::Complete(outcome) => {
                        assert!((-31..=31).contains(&outcome.gain));
                        assert!((-63..=63).contains(&outcome.phase));
                        assert!(edges < 2_000);
                        break;
                    }
                    PhyTxIqCalibrationAction::Failed(failure) => {
                        panic!("unexpected calibration failure: {failure:?}")
                    }
                    _ => {
                        transition
                            .advance(calibration_completion(action, 100))
                            .unwrap();
                        edges += 1;
                    }
                }
            }
        }
    }

    #[test]
    fn init_skip_does_not_emit_any_hardware_action() {
        let transition = PhyTxIqInitTransition::new(PhyTxIqInitParameters {
            already_calibrated: true,
            crystal_selector: 0,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: 0,
                pbus_rx_path_value: 0,
                dco: [0; 4],
            },
            capacitance: [0; 6],
            channel_6_dcode: [0; 2],
            initial_attenuation: 0,
            power_offset: 0,
            reference_codes: [0; 2],
            clear_tone_after_ready: false,
        });
        assert_eq!(
            transition.action(),
            PhyTxIqInitAction::Complete(PhyTxIqInitOutcome {
                coefficient: [0; 2],
                external_dcode: [0; 2],
                temperature: None,
                calibration_performed: false,
            })
        );
    }

    #[test]
    fn external_lowering_covers_every_txiq_operation_layer() {
        let i2c = PhyI2cAddress::new_internal(0x62, 1);
        assert!(matches!(
            PhyTxIqInitExternalBinding::lower(PhyTxIqInitAction::Rfpll(
                RfpllFrequencyAction::DelayMicros(5)
            )),
            Ok(PhyTxIqInitExternalBinding::Rfpll(_))
        ));
        assert!(matches!(
            PhyTxIqInitExternalBinding::lower(PhyTxIqInitAction::WriteI2c {
                address: i2c,
                value: 1,
            }),
            Ok(PhyTxIqInitExternalBinding::I2c(_))
        ));
        assert!(matches!(
            PhyTxIqInitExternalBinding::lower(PhyTxIqInitAction::Temperature(
                PhyTemperatureTransition::new().action()
            )),
            Ok(PhyTxIqInitExternalBinding::Temperature(_))
        ));
        assert!(matches!(
            PhyTxIqCalibrationExternalBinding::lower(
                PhyTxIqCalibrationAction::ConfigureCorrection { begin: true }
            ),
            Ok(PhyTxIqCalibrationExternalBinding::Mmio(_))
        ));
        assert!(matches!(
            PhyTxIqCalibrationExternalBinding::lower(PhyTxIqCalibrationAction::ForcePbus(
                PhyPbusForceTest::new(1, 2, 0)
            )),
            Ok(PhyTxIqCalibrationExternalBinding::Pbus(_))
        ));
        assert!(matches!(
            PhyTxIqCalibrationExternalBinding::lower(PhyTxIqCalibrationAction::Loopback(
                PhyTxIqLoopbackAction::I2c(MaskedI2cWriteAction::ReadByte {
                    address: LOOPBACK_ADDRESS,
                })
            )),
            Ok(PhyTxIqCalibrationExternalBinding::Loopback(
                PhyTxIqLoopbackExternalBinding::I2c(_)
            ))
        ));
        assert!(matches!(
            PhyTxIqMisPowerExternalBinding::lower(PhyTxIqMisPowerAction::DelayMicros {
                identity: 0,
                phase: PhyTxIqMisPowerDelayPhase::FirstPolarity,
                micros: 2,
            }),
            Ok(PhyTxIqMisPowerExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyTxIqCoverExternalBinding::lower(PhyTxIqCoverAction::ConfigureCoefficient {
                identity: 0,
                iteration: 0,
                kind: PhyTxIqCoefficientKind::Gain,
                value: 0,
            }),
            Ok(PhyTxIqCoverExternalBinding::Mmio(_))
        ));
        assert!(matches!(
            PhyTxIqInitExternalBinding::lower(PhyTxIqInitAction::Complete(PhyTxIqInitOutcome {
                coefficient: [0; 2],
                external_dcode: [0; 2],
                temperature: None,
                calibration_performed: false,
            })),
            Err(PhyTxIqExternalBindingError::UnsupportedAction)
        ));
    }
}
