//! Shared heap-free primitives for ESP32-S31 TX calibration.
//!
//! The first recovered child is complete rev0 ROM `phy_get_power_atten`,
//! size 278 bytes. Its six-iteration search used a blocking two-microsecond
//! delay and a synchronous SAR callback. Rust exposes both as one externally
//! completed operation per transition step.

use crate::{
    phy_dc_iq::phy_linear_to_db,
    phy_i2c::{
        analog_registers, MaskedI2cWriteAction, MaskedI2cWriteCompletion, MaskedI2cWriteTransition,
        PhyI2cAddress,
    },
    phy_pbus::PhyPbusForceTest,
    phy_pwdet::sar_signal_reference,
    phy_rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyRequest, RfpllFrequencyTransition,
    },
};

pub const PHY_POWER_ATTENUATION_MAX_ITERATIONS: u8 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationParameters {
    pub pbus_tx_path_value: u8,
    pub pbus_rx_path_value: u8,
    pub dco: [u16; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironment {
    Debug,
    Work,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentFailure {
    PbusTimedOut(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentDelayPhase {
    PbusWorkMode,
    PbusWorkModePulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentAction {
    ConfigurePbusDebugMode,
    ForcePbus(PhyPbusForceTest),
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigurePowerDetector,
    ConfigureCalibrationMode,
    StopTone,
    ConfigurePbusWorkMode,
    DelayMicros {
        phase: PhyTxCalibrationEnvironmentDelayPhase,
        micros: u32,
    },
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete(PhyTxCalibrationEnvironment),
    Failed(PhyTxCalibrationEnvironmentFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentCompletion {
    PbusDebugModeConfigured,
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    TxClockConfigured {
        enabled: bool,
    },
    PowerDetectorConfigured,
    CalibrationModeConfigured,
    ToneStopped,
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    DelayElapsed {
        phase: PhyTxCalibrationEnvironmentDelayPhase,
        micros: u32,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnvironmentStep {
    Debug,
    EnterPbus(u8),
    EnableClock,
    PowerDetector,
    CalibrationMode,
    StopTone,
    DisableClock,
    ExitPbus(u8),
    WorkMode,
    WorkModeDelay,
    WorkModePulse,
    WorkModePulseDelay,
    WorkModePulseClear,
    Complete(PhyTxCalibrationEnvironment),
    Failed(PhyTxCalibrationEnvironmentFailure),
}

const fn tx_debug_pbus(index: u8, parameters: PhyTxCalibrationParameters) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0x80),
        1 => PhyPbusForceTest::new(0, 2, 0),
        2 => PhyPbusForceTest::new(4, 2, 0),
        3 => PhyPbusForceTest::new(1, 1, 0x7c),
        4 => PhyPbusForceTest::new(2, 1, 0x100),
        5 => PhyPbusForceTest::new(3, 1, 0x100),
        6 => PhyPbusForceTest::new(2, 2, 0x100),
        7 => PhyPbusForceTest::new(3, 2, 0x100),
        8 => PhyPbusForceTest::new(1, 2, 0),
        9 => PhyPbusForceTest::new(4, 1, 0x0b),
        10 => PhyPbusForceTest::new(5, 1, parameters.pbus_tx_path_value as u16 + 0x1c0),
        11 => PhyPbusForceTest::new(2, 1, parameters.dco[0]),
        12 => PhyPbusForceTest::new(3, 1, parameters.dco[1]),
        13 => PhyPbusForceTest::new(2, 2, parameters.dco[2]),
        _ => PhyPbusForceTest::new(3, 2, parameters.dco[3]),
    }
}

const fn tx_work_pbus(index: u8, parameters: PhyTxCalibrationParameters) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, parameters.pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationEnvironmentTransition {
    parameters: PhyTxCalibrationParameters,
    step: EnvironmentStep,
}

impl PhyTxCalibrationEnvironmentTransition {
    pub const fn enter(parameters: PhyTxCalibrationParameters) -> Self {
        Self {
            parameters,
            step: EnvironmentStep::Debug,
        }
    }

    pub const fn exit(parameters: PhyTxCalibrationParameters) -> Self {
        Self {
            parameters,
            step: EnvironmentStep::StopTone,
        }
    }

    pub const fn action(self) -> PhyTxCalibrationEnvironmentAction {
        match self.step {
            EnvironmentStep::Debug => PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode,
            EnvironmentStep::EnterPbus(index) => {
                PhyTxCalibrationEnvironmentAction::ForcePbus(tx_debug_pbus(index, self.parameters))
            }
            EnvironmentStep::EnableClock => {
                PhyTxCalibrationEnvironmentAction::ConfigureTxClock { enabled: true }
            }
            EnvironmentStep::PowerDetector => {
                PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector
            }
            EnvironmentStep::CalibrationMode => {
                PhyTxCalibrationEnvironmentAction::ConfigureCalibrationMode
            }
            EnvironmentStep::StopTone => PhyTxCalibrationEnvironmentAction::StopTone,
            EnvironmentStep::DisableClock => {
                PhyTxCalibrationEnvironmentAction::ConfigureTxClock { enabled: false }
            }
            EnvironmentStep::ExitPbus(index) => {
                PhyTxCalibrationEnvironmentAction::ForcePbus(tx_work_pbus(index, self.parameters))
            }
            EnvironmentStep::WorkMode => PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkMode,
            EnvironmentStep::WorkModeDelay => PhyTxCalibrationEnvironmentAction::DelayMicros {
                phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkMode,
                micros: 1,
            },
            EnvironmentStep::WorkModePulse => {
                PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkModePulse
            }
            EnvironmentStep::WorkModePulseDelay => PhyTxCalibrationEnvironmentAction::DelayMicros {
                phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkModePulse,
                micros: 1,
            },
            EnvironmentStep::WorkModePulseClear => {
                PhyTxCalibrationEnvironmentAction::ClearPbusWorkModePulse
            }
            EnvironmentStep::Complete(mode) => PhyTxCalibrationEnvironmentAction::Complete(mode),
            EnvironmentStep::Failed(failure) => PhyTxCalibrationEnvironmentAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxCalibrationEnvironmentCompletion,
    ) -> Result<(), PhyTxCalibrationEnvironmentTransitionError> {
        self.step = match (self.step, completion) {
            (
                EnvironmentStep::Debug,
                PhyTxCalibrationEnvironmentCompletion::PbusDebugModeConfigured,
            ) => EnvironmentStep::EnterPbus(0),
            (
                EnvironmentStep::EnterPbus(index),
                PhyTxCalibrationEnvironmentCompletion::PbusCompleted(transaction),
            ) if transaction == tx_debug_pbus(index, self.parameters) => {
                if index == 14 {
                    EnvironmentStep::EnableClock
                } else {
                    EnvironmentStep::EnterPbus(index + 1)
                }
            }
            (
                EnvironmentStep::EnterPbus(index),
                PhyTxCalibrationEnvironmentCompletion::PbusTimedOut(transaction),
            ) if transaction == tx_debug_pbus(index, self.parameters) => EnvironmentStep::Failed(
                PhyTxCalibrationEnvironmentFailure::PbusTimedOut(transaction),
            ),
            (
                EnvironmentStep::EnableClock,
                PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled: true },
            ) => EnvironmentStep::PowerDetector,
            (
                EnvironmentStep::PowerDetector,
                PhyTxCalibrationEnvironmentCompletion::PowerDetectorConfigured,
            ) => EnvironmentStep::CalibrationMode,
            (
                EnvironmentStep::CalibrationMode,
                PhyTxCalibrationEnvironmentCompletion::CalibrationModeConfigured,
            ) => EnvironmentStep::Complete(PhyTxCalibrationEnvironment::Debug),
            (EnvironmentStep::StopTone, PhyTxCalibrationEnvironmentCompletion::ToneStopped) => {
                EnvironmentStep::DisableClock
            }
            (
                EnvironmentStep::DisableClock,
                PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled: false },
            ) => EnvironmentStep::ExitPbus(0),
            (
                EnvironmentStep::ExitPbus(index),
                PhyTxCalibrationEnvironmentCompletion::PbusCompleted(transaction),
            ) if transaction == tx_work_pbus(index, self.parameters) => {
                if index == 6 {
                    EnvironmentStep::WorkMode
                } else {
                    EnvironmentStep::ExitPbus(index + 1)
                }
            }
            (
                EnvironmentStep::ExitPbus(index),
                PhyTxCalibrationEnvironmentCompletion::PbusTimedOut(transaction),
            ) if transaction == tx_work_pbus(index, self.parameters) => EnvironmentStep::Failed(
                PhyTxCalibrationEnvironmentFailure::PbusTimedOut(transaction),
            ),
            (
                EnvironmentStep::WorkMode,
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => EnvironmentStep::Complete(PhyTxCalibrationEnvironment::Work),
            (
                EnvironmentStep::WorkMode,
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => EnvironmentStep::WorkModeDelay,
            (
                EnvironmentStep::WorkModeDelay,
                PhyTxCalibrationEnvironmentCompletion::DelayElapsed {
                    phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkMode,
                    micros: 1,
                },
            ) => EnvironmentStep::WorkModePulse,
            (
                EnvironmentStep::WorkModePulse,
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseConfigured,
            ) => EnvironmentStep::WorkModePulseDelay,
            (
                EnvironmentStep::WorkModePulseDelay,
                PhyTxCalibrationEnvironmentCompletion::DelayElapsed {
                    phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkModePulse,
                    micros: 1,
                },
            ) => EnvironmentStep::WorkModePulseClear,
            (
                EnvironmentStep::WorkModePulseClear,
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseCleared,
            ) => EnvironmentStep::Complete(PhyTxCalibrationEnvironment::Work),
            (EnvironmentStep::Complete(_), _) | (EnvironmentStep::Failed(_), _) => {
                return Err(PhyTxCalibrationEnvironmentTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxCalibrationEnvironmentTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyToneSarRequest {
    pub measurement: u8,
    pub samples: u8,
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyToneSarOutcome {
    pub request: PhyToneSarRequest,
    pub sample: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarFailure {
    ReadyDeadlineElapsed { measurement: u8, sample: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarDelayPhase {
    ToneArmed,
    SarTriggered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarAction {
    ArmTone {
        measurement: u8,
        sample: u8,
    },
    DelayMicros {
        measurement: u8,
        sample: u8,
        phase: PhyToneSarDelayPhase,
        micros: u32,
    },
    TriggerSar {
        measurement: u8,
        sample: u8,
    },
    PollReady {
        measurement: u8,
        sample: u8,
        address: usize,
        mask: u32,
        expected: u32,
    },
    ClearTone {
        measurement: u8,
        sample: u8,
    },
    ReadSar {
        measurement: u8,
        sample: u8,
        address: usize,
    },
    Complete(PhyToneSarOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarCompletion {
    ToneArmed {
        measurement: u8,
        sample: u8,
    },
    DelayElapsed {
        measurement: u8,
        sample: u8,
        phase: PhyToneSarDelayPhase,
        micros: u32,
    },
    SarTriggered {
        measurement: u8,
        sample: u8,
    },
    ReadySampled {
        measurement: u8,
        sample: u8,
        address: usize,
        register_value: u32,
    },
    ReadyDeadlineElapsed {
        measurement: u8,
        sample: u8,
    },
    ToneCleared {
        measurement: u8,
        sample: u8,
    },
    SarRead {
        measurement: u8,
        sample: u8,
        address: usize,
        register_value: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarTransitionError {
    WrongCompletion,
    InvalidSampleCount,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToneSarStep {
    Arm,
    ArmDelay,
    Trigger,
    TriggerDelay,
    Poll,
    Clear,
    Read,
    Complete(PhyToneSarOutcome),
    Failed(PhyToneSarFailure),
}

/// Complete event-driven translation of `phy_get_tone_sar_dout_`.
///
/// No completion interrupt is evidenced for the ready field. Each `PollReady`
/// action is exactly one volatile sample; the async owner supplies a finite
/// external deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyToneSarTransition {
    request: PhyToneSarRequest,
    step: ToneSarStep,
    sample: u8,
    sum: u32,
}

impl PhyToneSarTransition {
    /// Construct a transition when the caller proves the sample count in its
    /// type-local control flow.
    ///
    /// Calibration roots use fixed counts of two or four. Keeping this
    /// constructor separate removes runtime `unwrap`/`expect` paths without
    /// weakening the checked public constructor used for dynamic requests.
    pub const fn new_nonzero(request: PhyToneSarRequest) -> Self {
        debug_assert!(request.samples != 0);
        Self {
            request,
            step: ToneSarStep::Arm,
            sample: 0,
            sum: 0,
        }
    }

    pub const fn new(request: PhyToneSarRequest) -> Result<Self, PhyToneSarTransitionError> {
        if request.samples == 0 {
            Err(PhyToneSarTransitionError::InvalidSampleCount)
        } else {
            Ok(Self::new_nonzero(request))
        }
    }

    pub const fn action(self) -> PhyToneSarAction {
        let measurement = self.request.measurement;
        let sample = self.sample;
        match self.step {
            ToneSarStep::Arm => PhyToneSarAction::ArmTone {
                measurement,
                sample,
            },
            ToneSarStep::ArmDelay => PhyToneSarAction::DelayMicros {
                measurement,
                sample,
                phase: PhyToneSarDelayPhase::ToneArmed,
                micros: 1,
            },
            ToneSarStep::Trigger => PhyToneSarAction::TriggerSar {
                measurement,
                sample,
            },
            ToneSarStep::TriggerDelay => PhyToneSarAction::DelayMicros {
                measurement,
                sample,
                phase: PhyToneSarDelayPhase::SarTriggered,
                micros: 2,
            },
            ToneSarStep::Poll => PhyToneSarAction::PollReady {
                measurement,
                sample,
                address: crate::phy_pwdet::PHY_PWDET_READY_ADDRESS,
                mask: crate::phy_pwdet::PHY_PWDET_READY_MASK,
                expected: crate::phy_pwdet::PHY_PWDET_READY_VALUE,
            },
            ToneSarStep::Clear => PhyToneSarAction::ClearTone {
                measurement,
                sample,
            },
            ToneSarStep::Read => PhyToneSarAction::ReadSar {
                measurement,
                sample,
                address: crate::phy_pwdet::PHY_PWDET_SAR_SAMPLE_ADDRESS,
            },
            ToneSarStep::Complete(outcome) => PhyToneSarAction::Complete(outcome),
            ToneSarStep::Failed(failure) => PhyToneSarAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyToneSarCompletion,
    ) -> Result<(), PhyToneSarTransitionError> {
        let measurement = self.request.measurement;
        let sample = self.sample;
        self.step = match (self.step, completion) {
            (
                ToneSarStep::Arm,
                PhyToneSarCompletion::ToneArmed {
                    measurement: completed,
                    sample: completed_sample,
                },
            ) if completed == measurement && completed_sample == sample => ToneSarStep::ArmDelay,
            (
                ToneSarStep::ArmDelay,
                PhyToneSarCompletion::DelayElapsed {
                    measurement: completed,
                    sample: completed_sample,
                    phase: PhyToneSarDelayPhase::ToneArmed,
                    micros: 1,
                },
            ) if completed == measurement && completed_sample == sample => ToneSarStep::Trigger,
            (
                ToneSarStep::Trigger,
                PhyToneSarCompletion::SarTriggered {
                    measurement: completed,
                    sample: completed_sample,
                },
            ) if completed == measurement && completed_sample == sample => {
                ToneSarStep::TriggerDelay
            }
            (
                ToneSarStep::TriggerDelay,
                PhyToneSarCompletion::DelayElapsed {
                    measurement: completed,
                    sample: completed_sample,
                    phase: PhyToneSarDelayPhase::SarTriggered,
                    micros: 2,
                },
            ) if completed == measurement && completed_sample == sample => ToneSarStep::Poll,
            (
                ToneSarStep::Poll,
                PhyToneSarCompletion::ReadySampled {
                    measurement: completed,
                    sample: completed_sample,
                    address: crate::phy_pwdet::PHY_PWDET_READY_ADDRESS,
                    register_value,
                },
            ) if completed == measurement && completed_sample == sample => {
                if register_value & crate::phy_pwdet::PHY_PWDET_READY_MASK
                    == crate::phy_pwdet::PHY_PWDET_READY_VALUE
                {
                    if self.request.clear_tone_after_ready {
                        ToneSarStep::Clear
                    } else {
                        ToneSarStep::Read
                    }
                } else {
                    ToneSarStep::Poll
                }
            }
            (
                ToneSarStep::Poll,
                PhyToneSarCompletion::ReadyDeadlineElapsed {
                    measurement: completed,
                    sample: completed_sample,
                },
            ) if completed == measurement && completed_sample == sample => {
                ToneSarStep::Failed(PhyToneSarFailure::ReadyDeadlineElapsed {
                    measurement,
                    sample,
                })
            }
            (
                ToneSarStep::Clear,
                PhyToneSarCompletion::ToneCleared {
                    measurement: completed,
                    sample: completed_sample,
                },
            ) if completed == measurement && completed_sample == sample => ToneSarStep::Read,
            (
                ToneSarStep::Read,
                PhyToneSarCompletion::SarRead {
                    measurement: completed,
                    sample: completed_sample,
                    address: crate::phy_pwdet::PHY_PWDET_SAR_SAMPLE_ADDRESS,
                    register_value,
                },
            ) if completed == measurement && completed_sample == sample => {
                self.sum =
                    self.sum
                        .wrapping_add(u32::from(crate::phy_pwdet::sar_sample_from_register(
                            register_value,
                        )));
                if self.sample + 1 == self.request.samples {
                    ToneSarStep::Complete(PhyToneSarOutcome {
                        request: self.request,
                        sample: (self.sum / u32::from(self.request.samples)) as u16,
                    })
                } else {
                    self.sample += 1;
                    ToneSarStep::Arm
                }
            }
            (ToneSarStep::Complete(_), _) | (ToneSarStep::Failed(_), _) => {
                return Err(PhyToneSarTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyToneSarTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

pub fn phy_tx_power_db(sample: u16, reference_codes: [i16; 2], offset: i16) -> i16 {
    let signal = sar_signal_reference(sample, reference_codes);
    phy_linear_to_db(signal[0] as i32, 3)
        .wrapping_add(offset as i32)
        .wrapping_sub(phy_linear_to_db(signal[1] as i32, 3)) as i16
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerAttenuationRequest {
    pub tone_selector: u16,
    pub initial_attenuation: u8,
    pub target_power: i16,
    pub power_offset: i16,
    pub reference_codes: [i16; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerAttenuationOutcome {
    pub attenuation: u8,
    pub iterations: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationAction {
    ConfigureTone {
        iteration: u8,
        selector: u16,
        attenuation: u8,
    },
    ToneSar(PhyToneSarAction),
    Complete(PhyPowerAttenuationOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationCompletion {
    ToneConfigured {
        iteration: u8,
        selector: u16,
        attenuation: u8,
    },
    ToneSar(PhyToneSarCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Tone,
    ToneSar(PhyToneSarTransition),
    Complete(PhyPowerAttenuationOutcome),
    Failed(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPowerAttenuationTransition {
    request: PhyPowerAttenuationRequest,
    iteration: u8,
    attenuation: i16,
    previous_attenuation: i16,
    previous_power: i16,
    step: Step,
}

impl PhyPowerAttenuationTransition {
    pub const fn new(request: PhyPowerAttenuationRequest) -> Self {
        Self {
            request,
            iteration: 0,
            attenuation: request.initial_attenuation as i16,
            previous_attenuation: 0,
            previous_power: 0,
            step: Step::Tone,
        }
    }

    pub const fn action(self) -> PhyPowerAttenuationAction {
        match self.step {
            Step::Tone => PhyPowerAttenuationAction::ConfigureTone {
                iteration: self.iteration,
                selector: self.request.tone_selector,
                attenuation: self.attenuation as u8,
            },
            Step::ToneSar(transition) => PhyPowerAttenuationAction::ToneSar(transition.action()),
            Step::Complete(outcome) => PhyPowerAttenuationAction::Complete(outcome),
            Step::Failed(failure) => PhyPowerAttenuationAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyPowerAttenuationCompletion,
    ) -> Result<(), PhyPowerAttenuationTransitionError> {
        self.step = match (self.step, completion) {
            (
                Step::Tone,
                PhyPowerAttenuationCompletion::ToneConfigured {
                    iteration,
                    selector,
                    attenuation,
                },
            ) if iteration == self.iteration
                && selector == self.request.tone_selector
                && attenuation == self.attenuation as u8 =>
            {
                Step::ToneSar(
                    PhyToneSarTransition::new(PhyToneSarRequest {
                        measurement: self.iteration,
                        samples: 2,
                        clear_tone_after_ready: false,
                    })
                    .unwrap(),
                )
            }
            (Step::ToneSar(mut transition), PhyPowerAttenuationCompletion::ToneSar(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyPowerAttenuationTransitionError::WrongCompletion)?;
                let PhyToneSarAction::Complete(sample) = transition.action() else {
                    if let PhyToneSarAction::Failed(failure) = transition.action() {
                        return {
                            self.step = Step::Failed(failure);
                            Ok(())
                        };
                    }
                    self.step = Step::ToneSar(transition);
                    return Ok(());
                };
                let power = phy_tx_power_db(
                    sample.sample,
                    self.request.reference_codes,
                    self.request.power_offset,
                ) >> 2;
                let error = power.wrapping_sub(self.request.target_power);
                if self.iteration != 0
                    && self.previous_attenuation < self.attenuation
                    && self.previous_power < power
                {
                    self.attenuation = self.previous_attenuation.wrapping_sub(20);
                }
                let output = self.attenuation;
                if (-3..=3).contains(&error) {
                    Step::Complete(PhyPowerAttenuationOutcome {
                        attenuation: output as u8,
                        iterations: self.iteration + 1,
                    })
                } else {
                    let delta = if error < 1 {
                        error.wrapping_mul(3) / 4
                    } else {
                        error
                    };
                    let next = output.wrapping_add(delta).clamp(0, 120);
                    self.previous_attenuation = output;
                    self.previous_power = power;
                    self.attenuation = next;
                    self.iteration += 1;
                    if self.iteration == PHY_POWER_ATTENUATION_MAX_ITERATIONS {
                        Step::Complete(PhyPowerAttenuationOutcome {
                            attenuation: next as u8,
                            iterations: self.iteration,
                        })
                    } else {
                        Step::Tone
                    }
                }
            }
            (Step::Complete(_), _) | (Step::Failed(_), _) => {
                return Err(PhyPowerAttenuationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyPowerAttenuationTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

const TX_CAP_I2C_ADDRESS: PhyI2cAddress = analog_registers::TX_CAPACITOR_BANKS;
const TX_CAP_CHANNELS: [u16; 3] = [1, 6, 11];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapSearchRequest {
    pub tone_selector: u16,
    pub attenuation: u8,
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapSearchOutcome {
    pub capacitance: [u8; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchFailure {
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchAction {
    ConfigureTone {
        selector: u16,
        attenuation: u8,
        enabled: bool,
    },
    I2c(MaskedI2cWriteAction),
    ToneSar(PhyToneSarAction),
    Complete(PhyTxCapSearchOutcome),
    Failed(PhyTxCapSearchFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchCompletion {
    ToneConfigured {
        selector: u16,
        attenuation: u8,
        enabled: bool,
    },
    I2c(MaskedI2cWriteCompletion),
    ToneSar(PhyToneSarCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxCapSearchStep {
    ConfigureTone,
    WriteCandidate(MaskedI2cWriteTransition),
    Measure(PhyToneSarTransition),
    WriteBest(MaskedI2cWriteTransition),
    StopTone,
    Complete(PhyTxCapSearchOutcome),
    Failed(PhyTxCapSearchFailure),
}

const fn tx_cap_field(bank: u8) -> (u8, u8) {
    if bank == 0 {
        (
            analog_registers::TX_CAPACITOR_LOW.high_bit,
            analog_registers::TX_CAPACITOR_LOW.low_bit,
        )
    } else {
        (
            analog_registers::TX_CAPACITOR_HIGH.high_bit,
            analog_registers::TX_CAPACITOR_HIGH.low_bit,
        )
    }
}

const fn tx_cap_max(bank: u8) -> u8 {
    if bank == 0 {
        7
    } else {
        13
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapSearchTransition {
    request: PhyTxCapSearchRequest,
    step: TxCapSearchStep,
    bank: u8,
    candidate: u8,
    best: [u8; 2],
    best_sample: u16,
}

impl PhyTxCapSearchTransition {
    pub const fn new(request: PhyTxCapSearchRequest) -> Self {
        Self {
            request,
            step: TxCapSearchStep::ConfigureTone,
            bank: 0,
            candidate: tx_cap_max(0),
            best: [tx_cap_max(0), tx_cap_max(1)],
            best_sample: 0,
        }
    }

    const fn write(self, value: u8) -> MaskedI2cWriteTransition {
        let (high, low) = tx_cap_field(self.bank);
        MaskedI2cWriteTransition::new(TX_CAP_I2C_ADDRESS, high, low, value).unwrap()
    }

    pub const fn action(self) -> PhyTxCapSearchAction {
        match self.step {
            TxCapSearchStep::ConfigureTone => PhyTxCapSearchAction::ConfigureTone {
                selector: self.request.tone_selector,
                attenuation: self.request.attenuation,
                enabled: true,
            },
            TxCapSearchStep::WriteCandidate(transition)
            | TxCapSearchStep::WriteBest(transition) => {
                PhyTxCapSearchAction::I2c(transition.action())
            }
            TxCapSearchStep::Measure(transition) => {
                PhyTxCapSearchAction::ToneSar(transition.action())
            }
            TxCapSearchStep::StopTone => PhyTxCapSearchAction::ConfigureTone {
                selector: self.request.tone_selector,
                attenuation: self.request.attenuation,
                enabled: false,
            },
            TxCapSearchStep::Complete(outcome) => PhyTxCapSearchAction::Complete(outcome),
            TxCapSearchStep::Failed(failure) => PhyTxCapSearchAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxCapSearchCompletion,
    ) -> Result<(), PhyTxCapSearchTransitionError> {
        match (self.step, completion) {
            (
                TxCapSearchStep::ConfigureTone,
                PhyTxCapSearchCompletion::ToneConfigured {
                    selector,
                    attenuation,
                    enabled: true,
                },
            ) if selector == self.request.tone_selector
                && attenuation == self.request.attenuation =>
            {
                self.step = TxCapSearchStep::WriteCandidate(self.write(self.candidate));
            }
            (
                TxCapSearchStep::WriteCandidate(mut transition),
                PhyTxCapSearchCompletion::I2c(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapSearchTransitionError::WrongCompletion)?;
                self.step = if transition.action() == MaskedI2cWriteAction::Complete {
                    TxCapSearchStep::Measure(
                        PhyToneSarTransition::new(PhyToneSarRequest {
                            measurement: self.bank.wrapping_mul(16).wrapping_add(self.candidate),
                            samples: 1,
                            clear_tone_after_ready: self.request.clear_tone_after_ready,
                        })
                        .unwrap(),
                    )
                } else {
                    TxCapSearchStep::WriteCandidate(transition)
                };
            }
            (
                TxCapSearchStep::Measure(mut transition),
                PhyTxCapSearchCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapSearchTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        if outcome.sample > self.best_sample {
                            self.best_sample = outcome.sample;
                            self.best[self.bank as usize] = self.candidate;
                        }
                        if self.candidate == 0 {
                            self.step = TxCapSearchStep::WriteBest(
                                self.write(self.best[self.bank as usize]),
                            );
                        } else {
                            self.candidate -= 1;
                            self.step = TxCapSearchStep::WriteCandidate(self.write(self.candidate));
                        }
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.step =
                            TxCapSearchStep::Failed(PhyTxCapSearchFailure::ToneSar(failure));
                    }
                    _ => self.step = TxCapSearchStep::Measure(transition),
                }
            }
            (
                TxCapSearchStep::WriteBest(mut transition),
                PhyTxCapSearchCompletion::I2c(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapSearchTransitionError::WrongCompletion)?;
                if transition.action() == MaskedI2cWriteAction::Complete {
                    if self.bank == 0 {
                        self.bank = 1;
                        self.candidate = tx_cap_max(1);
                        self.best_sample = 0;
                        self.step = TxCapSearchStep::WriteCandidate(self.write(self.candidate));
                    } else {
                        self.step = TxCapSearchStep::StopTone;
                    }
                } else {
                    self.step = TxCapSearchStep::WriteBest(transition);
                }
            }
            (
                TxCapSearchStep::StopTone,
                PhyTxCapSearchCompletion::ToneConfigured {
                    selector,
                    attenuation,
                    enabled: false,
                },
            ) if selector == self.request.tone_selector
                && attenuation == self.request.attenuation =>
            {
                self.step = TxCapSearchStep::Complete(PhyTxCapSearchOutcome {
                    capacitance: self.best,
                });
            }
            (TxCapSearchStep::Complete(_), _) | (TxCapSearchStep::Failed(_), _) => {
                return Err(PhyTxCapSearchTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxCapSearchTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapParameters {
    pub crystal_selector: u8,
    pub environment: PhyTxCalibrationParameters,
    pub clear_tone_after_ready: bool,
    pub reference_codes: [i16; 2],
    pub power_offset: i16,
    pub initial_attenuation: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapOutcome {
    pub capacitance: [u8; 6],
    pub attenuation: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapFailure {
    Environment(PhyTxCalibrationEnvironmentFailure),
    Rfpll(RfpllFrequencyFailure),
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapAction {
    Environment(PhyTxCalibrationEnvironmentAction),
    Rfpll(RfpllFrequencyAction),
    I2c(MaskedI2cWriteAction),
    Attenuation(PhyPowerAttenuationAction),
    Search(PhyTxCapSearchAction),
    Complete(PhyTxCapOutcome),
    Failed(PhyTxCapFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapCompletion {
    Environment(PhyTxCalibrationEnvironmentCompletion),
    Rfpll(RfpllFrequencyCompletion),
    I2c(MaskedI2cWriteCompletion),
    Attenuation(PhyPowerAttenuationCompletion),
    Search(PhyTxCapSearchCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxCapTerminal {
    Complete,
    Failed(PhyTxCapFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxCapStep {
    Enter(PhyTxCalibrationEnvironmentTransition),
    Rfpll(RfpllFrequencyTransition),
    SetLow(MaskedI2cWriteTransition),
    SetHigh(MaskedI2cWriteTransition),
    Attenuation(PhyPowerAttenuationTransition),
    Search(PhyTxCapSearchTransition),
    Exit {
        terminal: TxCapTerminal,
        transition: PhyTxCalibrationEnvironmentTransition,
    },
    Complete,
    Failed(PhyTxCapFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxCapTransition {
    parameters: PhyTxCapParameters,
    step: TxCapStep,
    channel: u8,
    attenuation: u8,
    capacitance: [u8; 6],
}

impl PhyTxCapTransition {
    pub const fn new(parameters: PhyTxCapParameters) -> Self {
        Self {
            parameters,
            step: TxCapStep::Enter(PhyTxCalibrationEnvironmentTransition::enter(
                parameters.environment,
            )),
            channel: 0,
            attenuation: parameters.initial_attenuation,
            capacitance: [0; 6],
        }
    }

    fn exit(&mut self, terminal: TxCapTerminal) {
        self.step = TxCapStep::Exit {
            terminal,
            transition: PhyTxCalibrationEnvironmentTransition::exit(self.parameters.environment),
        };
    }

    fn fail(&mut self, failure: PhyTxCapFailure) {
        self.exit(TxCapTerminal::Failed(failure));
    }

    pub const fn action(self) -> PhyTxCapAction {
        match self.step {
            TxCapStep::Enter(transition) | TxCapStep::Exit { transition, .. } => {
                PhyTxCapAction::Environment(transition.action())
            }
            TxCapStep::Rfpll(transition) => PhyTxCapAction::Rfpll(transition.action()),
            TxCapStep::SetLow(transition) | TxCapStep::SetHigh(transition) => {
                PhyTxCapAction::I2c(transition.action())
            }
            TxCapStep::Attenuation(transition) => PhyTxCapAction::Attenuation(transition.action()),
            TxCapStep::Search(transition) => PhyTxCapAction::Search(transition.action()),
            TxCapStep::Complete => PhyTxCapAction::Complete(PhyTxCapOutcome {
                capacitance: self.capacitance,
                attenuation: self.attenuation,
            }),
            TxCapStep::Failed(failure) => PhyTxCapAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxCapCompletion,
    ) -> Result<(), PhyTxCapTransitionError> {
        match (self.step, completion) {
            (TxCapStep::Enter(mut transition), PhyTxCapCompletion::Environment(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Debug,
                    ) => {
                        self.step = TxCapStep::Rfpll(RfpllFrequencyTransition::new(
                            RfpllFrequencyRequest {
                                crystal_selector: self.parameters.crystal_selector,
                                frequency_code: TX_CAP_CHANNELS[self.channel as usize],
                                offset: 0,
                            },
                        ));
                    }
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.fail(PhyTxCapFailure::Environment(failure));
                    }
                    _ => self.step = TxCapStep::Enter(transition),
                }
            }
            (TxCapStep::Rfpll(mut transition), PhyTxCapCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => {
                        self.step = TxCapStep::SetLow(
                            MaskedI2cWriteTransition::new(
                                TX_CAP_I2C_ADDRESS,
                                analog_registers::TX_CAPACITOR_LOW.high_bit,
                                analog_registers::TX_CAPACITOR_LOW.low_bit,
                                7,
                            )
                            .unwrap(),
                        );
                    }
                    RfpllFrequencyAction::Failed(failure) => {
                        self.fail(PhyTxCapFailure::Rfpll(failure));
                    }
                    _ => self.step = TxCapStep::Rfpll(transition),
                }
            }
            (TxCapStep::SetLow(mut transition), PhyTxCapCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                self.step = if transition.action() == MaskedI2cWriteAction::Complete {
                    TxCapStep::SetHigh(
                        MaskedI2cWriteTransition::new(
                            TX_CAP_I2C_ADDRESS,
                            analog_registers::TX_CAPACITOR_HIGH.high_bit,
                            analog_registers::TX_CAPACITOR_HIGH.low_bit,
                            13,
                        )
                        .unwrap(),
                    )
                } else {
                    TxCapStep::SetLow(transition)
                };
            }
            (TxCapStep::SetHigh(mut transition), PhyTxCapCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                if transition.action() == MaskedI2cWriteAction::Complete {
                    self.step = if self.channel == 0 {
                        TxCapStep::Attenuation(PhyPowerAttenuationTransition::new(
                            PhyPowerAttenuationRequest {
                                tone_selector: 0x80,
                                initial_attenuation: self.attenuation,
                                target_power: 0x28,
                                power_offset: self.parameters.power_offset,
                                reference_codes: self.parameters.reference_codes,
                            },
                        ))
                    } else {
                        TxCapStep::Search(PhyTxCapSearchTransition::new(PhyTxCapSearchRequest {
                            tone_selector: 0x80,
                            attenuation: self.attenuation,
                            clear_tone_after_ready: self.parameters.clear_tone_after_ready,
                        }))
                    };
                } else {
                    self.step = TxCapStep::SetHigh(transition);
                }
            }
            (
                TxCapStep::Attenuation(mut transition),
                PhyTxCapCompletion::Attenuation(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyPowerAttenuationAction::Complete(outcome) => {
                        self.attenuation = outcome.attenuation;
                        self.step = TxCapStep::Search(PhyTxCapSearchTransition::new(
                            PhyTxCapSearchRequest {
                                tone_selector: 0x80,
                                attenuation: self.attenuation,
                                clear_tone_after_ready: self.parameters.clear_tone_after_ready,
                            },
                        ));
                    }
                    PhyPowerAttenuationAction::Failed(failure) => {
                        self.fail(PhyTxCapFailure::ToneSar(failure));
                    }
                    _ => self.step = TxCapStep::Attenuation(transition),
                }
            }
            (TxCapStep::Search(mut transition), PhyTxCapCompletion::Search(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCapSearchAction::Complete(outcome) => {
                        let offset = self.channel as usize * 2;
                        self.capacitance[offset] = outcome.capacitance[0];
                        self.capacitance[offset + 1] = outcome.capacitance[1];
                        self.channel += 1;
                        if self.channel == 3 {
                            self.exit(TxCapTerminal::Complete);
                        } else {
                            self.step = TxCapStep::Rfpll(RfpllFrequencyTransition::new(
                                RfpllFrequencyRequest {
                                    crystal_selector: self.parameters.crystal_selector,
                                    frequency_code: TX_CAP_CHANNELS[self.channel as usize],
                                    offset: 0,
                                },
                            ));
                        }
                    }
                    PhyTxCapSearchAction::Failed(PhyTxCapSearchFailure::ToneSar(failure)) => {
                        self.fail(PhyTxCapFailure::ToneSar(failure));
                    }
                    _ => self.step = TxCapStep::Search(transition),
                }
            }
            (
                TxCapStep::Exit {
                    terminal,
                    mut transition,
                },
                PhyTxCapCompletion::Environment(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxCapTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyTxCalibrationEnvironmentAction::Complete(
                        PhyTxCalibrationEnvironment::Work,
                    ) => {
                        self.step = match terminal {
                            TxCapTerminal::Complete => TxCapStep::Complete,
                            TxCapTerminal::Failed(failure) => TxCapStep::Failed(failure),
                        };
                    }
                    PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.step = TxCapStep::Failed(PhyTxCapFailure::Environment(failure));
                    }
                    _ => {
                        self.step = TxCapStep::Exit {
                            terminal,
                            transition,
                        };
                    }
                }
            }
            (TxCapStep::Complete, _) | (TxCapStep::Failed(_), _) => {
                return Err(PhyTxCapTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxCapTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationBindingError {
    NotDirectMmio,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyPowerAttenuationMmioBinding {
    action: PhyPowerAttenuationAction,
}

impl PhyPowerAttenuationMmioBinding {
    pub fn new(action: PhyPowerAttenuationAction) -> Result<Self, PhyPowerAttenuationBindingError> {
        match action {
            PhyPowerAttenuationAction::ConfigureTone { .. } => Ok(Self { action }),
            _ => Err(PhyPowerAttenuationBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyPowerAttenuationCompletion {
        match self.action {
            PhyPowerAttenuationAction::ConfigureTone {
                iteration,
                selector,
                attenuation,
            } => {
                crate::radio_hal::configure_phy_power_control_tone(selector, attenuation);
                PhyPowerAttenuationCompletion::ToneConfigured {
                    iteration,
                    selector,
                    attenuation,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyToneSarBindingError {
    NotDirectMmio,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyToneSarMmioBinding {
    action: PhyToneSarAction,
}

impl PhyToneSarMmioBinding {
    pub fn new(action: PhyToneSarAction) -> Result<Self, PhyToneSarBindingError> {
        match action {
            PhyToneSarAction::ArmTone { .. }
            | PhyToneSarAction::TriggerSar { .. }
            | PhyToneSarAction::PollReady { .. }
            | PhyToneSarAction::ClearTone { .. }
            | PhyToneSarAction::ReadSar { .. } => Ok(Self { action }),
            _ => Err(PhyToneSarBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(
        self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> PhyToneSarCompletion {
        match self.action {
            PhyToneSarAction::ArmTone {
                measurement,
                sample,
            } => {
                crate::radio_hal::arm_phy_power_detector_tone();
                PhyToneSarCompletion::ToneArmed {
                    measurement,
                    sample,
                }
            }
            PhyToneSarAction::TriggerSar {
                measurement,
                sample,
            } => {
                open_esp_radio_hal_esp32s31::phy_power_detector::trigger_sar(registers);
                PhyToneSarCompletion::SarTriggered {
                    measurement,
                    sample,
                }
            }
            PhyToneSarAction::PollReady {
                measurement,
                sample,
                address,
                ..
            } => PhyToneSarCompletion::ReadySampled {
                measurement,
                sample,
                address,
                register_value: open_esp_radio_hal_esp32s31::phy_power_detector::sample_ready(
                    registers,
                ),
            },
            PhyToneSarAction::ClearTone {
                measurement,
                sample,
            } => {
                crate::radio_hal::clear_phy_power_detector_tone_arm();
                PhyToneSarCompletion::ToneCleared {
                    measurement,
                    sample,
                }
            }
            PhyToneSarAction::ReadSar {
                measurement,
                sample,
                address,
            } => PhyToneSarCompletion::SarRead {
                measurement,
                sample,
                address,
                register_value: open_esp_radio_hal_esp32s31::phy_power_detector::sample_sar(
                    registers,
                ),
            },
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationBindingError {
    NotDirectMmio,
}

/// Non-cloneable direct-MMIO token for entering or leaving TX calibration.
/// PBus force commands and both one-microsecond intervals stay separately
/// bound to the async executor.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationEnvironmentMmioBinding {
    action: PhyTxCalibrationEnvironmentAction,
}

impl PhyTxCalibrationEnvironmentMmioBinding {
    pub fn new(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> Result<Self, PhyTxCalibrationBindingError> {
        match action {
            PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode
            | PhyTxCalibrationEnvironmentAction::ConfigureTxClock { .. }
            | PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector
            | PhyTxCalibrationEnvironmentAction::ConfigureCalibrationMode
            | PhyTxCalibrationEnvironmentAction::StopTone
            | PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkMode
            | PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkModePulse
            | PhyTxCalibrationEnvironmentAction::ClearPbusWorkModePulse => Ok(Self { action }),
            _ => Err(PhyTxCalibrationBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyTxCalibrationEnvironmentAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(
        self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> PhyTxCalibrationEnvironmentCompletion {
        match self.action {
            PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode => {
                open_esp_radio_hal_esp32s31::pbus::configure_debug_mode(registers);
                PhyTxCalibrationEnvironmentCompletion::PbusDebugModeConfigured
            }
            PhyTxCalibrationEnvironmentAction::ConfigureTxClock { enabled } => {
                open_esp_radio_hal_esp32s31::pbus::configure_tx_clock(registers, enabled);
                PhyTxCalibrationEnvironmentCompletion::TxClockConfigured { enabled }
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePowerDetector => {
                open_esp_radio_hal_esp32s31::phy_power_detector::configure_enabled(registers);
                PhyTxCalibrationEnvironmentCompletion::PowerDetectorConfigured
            }
            PhyTxCalibrationEnvironmentAction::ConfigureCalibrationMode => {
                open_esp_radio_hal_esp32s31::phy_power_detector::configure_calibration_mode(
                    registers,
                );
                PhyTxCalibrationEnvironmentCompletion::CalibrationModeConfigured
            }
            PhyTxCalibrationEnvironmentAction::StopTone => {
                crate::radio_hal::stop_phy_power_detector_tone();
                PhyTxCalibrationEnvironmentCompletion::ToneStopped
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkMode => {
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModeConfigured {
                    settle_required: open_esp_radio_hal_esp32s31::pbus::configure_work_mode(
                        registers,
                    ),
                }
            }
            PhyTxCalibrationEnvironmentAction::ConfigurePbusWorkModePulse => {
                open_esp_radio_hal_esp32s31::phy_agc::configure_pbus_work_mode_pulse(registers);
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseConfigured
            }
            PhyTxCalibrationEnvironmentAction::ClearPbusWorkModePulse => {
                open_esp_radio_hal_esp32s31::phy_agc::clear_pbus_work_mode_pulse(registers);
                PhyTxCalibrationEnvironmentCompletion::PbusWorkModePulseCleared
            }
            _ => unreachable!(),
        }
    }
}

/// Direct tone-programming edge of the TX-capacitance search.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCapSearchMmioBinding {
    action: PhyTxCapSearchAction,
}

impl PhyTxCapSearchMmioBinding {
    pub fn new(action: PhyTxCapSearchAction) -> Result<Self, PhyTxCalibrationBindingError> {
        match action {
            PhyTxCapSearchAction::ConfigureTone { .. } => Ok(Self { action }),
            _ => Err(PhyTxCalibrationBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyTxCapSearchAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyTxCapSearchCompletion {
        match self.action {
            PhyTxCapSearchAction::ConfigureTone {
                selector,
                attenuation,
                enabled,
            } => {
                if enabled {
                    crate::radio_hal::configure_phy_power_control_tone(selector, attenuation);
                } else {
                    crate::radio_hal::stop_phy_power_detector_tone();
                }
                PhyTxCapSearchCompletion::ToneConfigured {
                    selector,
                    attenuation,
                    enabled,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxCapExternalBindingError {
    UnsupportedAction,
    Pbus(crate::phy_pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationEnvironmentPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::phy_pbus::PhyPbusHardwareBinding,
}

impl PhyTxCalibrationEnvironmentPbusBinding {
    pub fn new(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> Result<Self, PhyTxCapExternalBindingError> {
        let PhyTxCalibrationEnvironmentAction::ForcePbus(transaction) = action else {
            return Err(PhyTxCapExternalBindingError::UnsupportedAction);
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
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> Result<(), crate::phy_pbus::PhyPbusHardwareBindingError> {
        self.hardware.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> Result<
        crate::phy_pbus::PhyPbusHardwareObservation,
        crate::phy_pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyTxCalibrationEnvironmentCompletion, PhyTxCapExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxCalibrationEnvironmentCompletion::PbusCompleted)
            .map_err(PhyTxCapExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyTxCalibrationEnvironmentCompletion {
        PhyTxCalibrationEnvironmentCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxCalibrationEnvironmentTimerBinding {
    phase: PhyTxCalibrationEnvironmentDelayPhase,
    micros: u32,
}

impl PhyTxCalibrationEnvironmentTimerBinding {
    pub fn new(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyTxCalibrationEnvironmentAction::DelayMicros { phase, micros } => {
                Ok(Self { phase, micros })
            }
            _ => Err(PhyTxCapExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxCalibrationEnvironmentCompletion {
        PhyTxCalibrationEnvironmentCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxCalibrationEnvironmentExternalBinding {
    Mmio(PhyTxCalibrationEnvironmentMmioBinding),
    Pbus(PhyTxCalibrationEnvironmentPbusBinding),
    Timer(PhyTxCalibrationEnvironmentTimerBinding),
}

impl PhyTxCalibrationEnvironmentExternalBinding {
    pub fn lower(
        action: PhyTxCalibrationEnvironmentAction,
    ) -> Result<Self, PhyTxCapExternalBindingError> {
        if let Ok(binding) = PhyTxCalibrationEnvironmentMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyTxCalibrationEnvironmentPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyTxCalibrationEnvironmentTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyTxCapExternalBindingError::UnsupportedAction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyToneSarTimerBinding {
    measurement: u8,
    sample: u8,
    phase: PhyToneSarDelayPhase,
    micros: u32,
}

impl PhyToneSarTimerBinding {
    pub fn new(action: PhyToneSarAction) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyToneSarAction::DelayMicros {
                measurement,
                sample,
                phase,
                micros,
            } => Ok(Self {
                measurement,
                sample,
                phase,
                micros,
            }),
            _ => Err(PhyTxCapExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyToneSarCompletion {
        PhyToneSarCompletion::DelayElapsed {
            measurement: self.measurement,
            sample: self.sample,
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyToneSarExternalBinding {
    Mmio(PhyToneSarMmioBinding),
    Timer(PhyToneSarTimerBinding),
}

impl PhyToneSarExternalBinding {
    pub fn lower(action: PhyToneSarAction) -> Result<Self, PhyTxCapExternalBindingError> {
        if let Ok(binding) = PhyToneSarMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyToneSarTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyTxCapExternalBindingError::UnsupportedAction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyPowerAttenuationExternalBinding {
    Mmio(PhyPowerAttenuationMmioBinding),
    ToneSar(PhyToneSarExternalBinding),
}

impl PhyPowerAttenuationExternalBinding {
    pub fn lower(action: PhyPowerAttenuationAction) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyPowerAttenuationAction::ConfigureTone { .. } => {
                PhyPowerAttenuationMmioBinding::new(action)
                    .map(Self::Mmio)
                    .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction)
            }
            PhyPowerAttenuationAction::ToneSar(action) => {
                PhyToneSarExternalBinding::lower(action).map(Self::ToneSar)
            }
            PhyPowerAttenuationAction::Complete(_) | PhyPowerAttenuationAction::Failed(_) => {
                Err(PhyTxCapExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxCapSearchExternalBinding {
    Mmio(PhyTxCapSearchMmioBinding),
    I2c(crate::phy_i2c::MaskedI2cWriteBinding),
    ToneSar(PhyToneSarExternalBinding),
}

impl PhyTxCapSearchExternalBinding {
    pub fn lower(action: PhyTxCapSearchAction) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyTxCapSearchAction::ConfigureTone { .. } => PhyTxCapSearchMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction),
            PhyTxCapSearchAction::I2c(action) => crate::phy_i2c::MaskedI2cWriteBinding::new(action)
                .map(Self::I2c)
                .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction),
            PhyTxCapSearchAction::ToneSar(action) => {
                PhyToneSarExternalBinding::lower(action).map(Self::ToneSar)
            }
            PhyTxCapSearchAction::Complete(_) | PhyTxCapSearchAction::Failed(_) => {
                Err(PhyTxCapExternalBindingError::UnsupportedAction)
            }
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_tx_cap_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxCapExternalBinding {
    Environment(PhyTxCalibrationEnvironmentExternalBinding),
    Rfpll(crate::phy_rfpll::RfpllFrequencyExternalBinding),
    I2c(crate::phy_i2c::MaskedI2cWriteBinding),
    Attenuation(PhyPowerAttenuationExternalBinding),
    Search(PhyTxCapSearchExternalBinding),
}

impl PhyTxCapExternalBinding {
    pub fn lower(action: PhyTxCapAction) -> Result<Self, PhyTxCapExternalBindingError> {
        match action {
            PhyTxCapAction::Environment(action) => {
                PhyTxCalibrationEnvironmentExternalBinding::lower(action).map(Self::Environment)
            }
            PhyTxCapAction::Rfpll(action) => {
                crate::phy_rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction)
            }
            PhyTxCapAction::I2c(action) => crate::phy_i2c::MaskedI2cWriteBinding::new(action)
                .map(Self::I2c)
                .map_err(|_| PhyTxCapExternalBindingError::UnsupportedAction),
            PhyTxCapAction::Attenuation(action) => {
                PhyPowerAttenuationExternalBinding::lower(action).map(Self::Attenuation)
            }
            PhyTxCapAction::Search(action) => {
                PhyTxCapSearchExternalBinding::lower(action).map(Self::Search)
            }
            PhyTxCapAction::Complete(_) | PhyTxCapAction::Failed(_) => {
                Err(PhyTxCapExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register(sample: u16) -> u32 {
        u32::from(sample) << 17
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
                register_value: register(value),
            },
            action => panic!("unexpected terminal tone/SAR action: {action:?}"),
        }
    }

    fn complete_tone_sar(transition: &mut PhyPowerAttenuationTransition, value: u16) {
        loop {
            let PhyPowerAttenuationAction::ToneSar(action) = transition.action() else {
                break;
            };
            transition
                .advance(PhyPowerAttenuationCompletion::ToneSar(tone_sar_completion(
                    action, value,
                )))
                .unwrap();
        }
    }

    #[test]
    fn power_db_uses_explicit_reference_codes() {
        assert_eq!(phy_tx_power_db(100, [90, 110], 4), 40);
    }

    #[test]
    fn search_completes_without_polling_when_error_is_in_window() {
        let request = PhyPowerAttenuationRequest {
            tone_selector: 0x80,
            initial_attenuation: 80,
            target_power: 13,
            power_offset: 4,
            reference_codes: [90, 110],
        };
        let mut transition = PhyPowerAttenuationTransition::new(request);
        transition
            .advance(PhyPowerAttenuationCompletion::ToneConfigured {
                iteration: 0,
                selector: 0x80,
                attenuation: 80,
            })
            .unwrap();
        complete_tone_sar(&mut transition, 100);
        assert_eq!(
            transition.action(),
            PhyPowerAttenuationAction::Complete(PhyPowerAttenuationOutcome {
                attenuation: 80,
                iterations: 1,
            })
        );
    }

    #[test]
    fn search_has_exact_six_sample_bound() {
        let request = PhyPowerAttenuationRequest {
            tone_selector: 0x80,
            initial_attenuation: 80,
            target_power: -100,
            power_offset: 4,
            reference_codes: [90, 110],
        };
        let mut transition = PhyPowerAttenuationTransition::new(request);
        let mut samples = 0;
        loop {
            let completion = match transition.action() {
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
                    if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                        samples += 1;
                    }
                    PhyPowerAttenuationCompletion::ToneSar(tone_sar_completion(action, 100))
                }
                PhyPowerAttenuationAction::Complete(outcome) => {
                    assert_eq!(outcome.iterations, 6);
                    break;
                }
                PhyPowerAttenuationAction::Failed(failure) => {
                    panic!("unexpected tone/SAR failure: {failure:?}")
                }
            };
            transition.advance(completion).unwrap();
        }
        assert_eq!(samples, 12);
    }

    #[test]
    fn tone_sar_uses_two_samples_and_one_poll_per_external_edge() {
        let request = PhyToneSarRequest {
            measurement: 7,
            samples: 2,
            clear_tone_after_ready: true,
        };
        let mut transition = PhyToneSarTransition::new(request).unwrap();
        for value in [100, 104] {
            loop {
                let action = transition.action();
                if matches!(
                    action,
                    PhyToneSarAction::Complete(_) | PhyToneSarAction::Failed(_)
                ) {
                    break;
                }
                transition
                    .advance(tone_sar_completion(action, value))
                    .unwrap();
                if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                    break;
                }
            }
        }
        assert_eq!(
            transition.action(),
            PhyToneSarAction::Complete(PhyToneSarOutcome {
                request,
                sample: 102,
            })
        );
    }

    #[test]
    fn tx_cap_lowering_covers_every_nested_operation_class() {
        let i2c = PhyI2cAddress::new_internal(0x62, 1);
        assert!(matches!(
            PhyTxCapExternalBinding::lower(PhyTxCapAction::Environment(
                PhyTxCalibrationEnvironmentAction::ConfigurePbusDebugMode
            )),
            Ok(PhyTxCapExternalBinding::Environment(
                PhyTxCalibrationEnvironmentExternalBinding::Mmio(_)
            ))
        ));
        assert!(matches!(
            PhyTxCalibrationEnvironmentExternalBinding::lower(
                PhyTxCalibrationEnvironmentAction::ForcePbus(PhyPbusForceTest::new(1, 1, 0))
            ),
            Ok(PhyTxCalibrationEnvironmentExternalBinding::Pbus(_))
        ));
        assert!(matches!(
            PhyTxCalibrationEnvironmentExternalBinding::lower(
                PhyTxCalibrationEnvironmentAction::DelayMicros {
                    phase: PhyTxCalibrationEnvironmentDelayPhase::PbusWorkMode,
                    micros: 1,
                }
            ),
            Ok(PhyTxCalibrationEnvironmentExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyTxCapExternalBinding::lower(PhyTxCapAction::Rfpll(
                RfpllFrequencyAction::DelayMicros(20)
            )),
            Ok(PhyTxCapExternalBinding::Rfpll(
                crate::phy_rfpll::RfpllFrequencyExternalBinding::Timer(_)
            ))
        ));
        assert!(matches!(
            PhyTxCapExternalBinding::lower(PhyTxCapAction::I2c(MaskedI2cWriteAction::ReadByte {
                address: i2c
            })),
            Ok(PhyTxCapExternalBinding::I2c(_))
        ));
        assert!(matches!(
            PhyTxCapExternalBinding::lower(PhyTxCapAction::Attenuation(
                PhyPowerAttenuationAction::ConfigureTone {
                    iteration: 0,
                    selector: 0x80,
                    attenuation: 1,
                }
            )),
            Ok(PhyTxCapExternalBinding::Attenuation(
                PhyPowerAttenuationExternalBinding::Mmio(_)
            ))
        ));
        assert!(matches!(
            PhyTxCapExternalBinding::lower(PhyTxCapAction::Search(PhyTxCapSearchAction::I2c(
                MaskedI2cWriteAction::WriteByte {
                    address: i2c,
                    value: 3,
                }
            ))),
            Ok(PhyTxCapExternalBinding::Search(
                PhyTxCapSearchExternalBinding::I2c(_)
            ))
        ));
        assert!(matches!(
            PhyToneSarExternalBinding::lower(PhyToneSarAction::PollReady {
                measurement: 0,
                sample: 0,
                address: 0x2010_0858,
                mask: 1,
                expected: 1,
            }),
            Ok(PhyToneSarExternalBinding::Mmio(_))
        ));
        assert!(matches!(
            PhyToneSarExternalBinding::lower(PhyToneSarAction::DelayMicros {
                measurement: 0,
                sample: 0,
                phase: PhyToneSarDelayPhase::ToneArmed,
                micros: 2,
            }),
            Ok(PhyToneSarExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyTxCapExternalBinding::lower(PhyTxCapAction::Complete(PhyTxCapOutcome {
                capacitance: [0; 6],
                attenuation: 0,
            })),
            Err(PhyTxCapExternalBindingError::UnsupportedAction)
        ));
    }
}
