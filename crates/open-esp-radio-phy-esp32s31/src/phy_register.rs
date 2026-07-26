//! Rust-owned composition of ESP32-S31 `register_chipv7_phy`.
//!
//! The pinned archive parent is 486 bytes. The primary profile deliberately
//! performs full calibration on every cold start: NVS, calibration-record
//! validation, recovery, backup and formatting are outside the radio porting
//! set. All retained radio work is represented by owned state plus one
//! externally completed MMIO, PHY-I2C, timer or observation edge.

pub const PHY_REGISTER_INIT_PROFILE_LEN: usize = 0x80;
pub const PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT: u16 = 1_000;
pub const PHY_REGISTER_I2C_STATUS_0_ADDRESS: usize = 0x2010_f800;
pub const PHY_REGISTER_I2C_STATUS_1_ADDRESS: usize = 0x2010_f804;

const PHY_REGISTER_FINAL_I2C_ADDRESS: crate::phy_i2c::PhyI2cAddress =
    crate::phy_i2c::PhyI2cAddress::new_internal(0x63, 0);

/// Exact 128-byte local fallback constructed by the pinned parent when its
/// init-profile pointer is null.
pub const fn default_phy_register_init_profile() -> [u8; PHY_REGISTER_INIT_PROFILE_LEN] {
    let mut profile = [0_u8; PHY_REGISTER_INIT_PROFILE_LEN];
    profile[0x00] = 1;
    profile[0x02] = 0x54;
    profile[0x03] = 0x54;
    profile[0x04] = 0x50;
    profile[0x05] = 0x50;
    profile[0x06] = 0x4c;
    profile[0x07] = 0x48;
    profile[0x08] = 0x50;
    profile[0x09] = 0x50;
    profile[0x0a] = 0x4c;
    profile[0x0b] = 0x44;
    profile[0x0c] = 0x40;
    profile[0x0d] = 0x38;
    profile[0x0e] = 0x38;
    profile[0x0f] = 0x38;
    profile[0x10] = 0x4c;
    profile[0x11] = 0x4c;
    profile[0x12] = 0x48;
    profile[0x13] = 0x44;
    profile
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterDelayPhase {
    ForceTxRx { enabled: bool, completed_phase: u8 },
    HardwareFrequencyDisabled,
    I2cMasterReset { index: u8, sample: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterMmioAction {
    PrepareColdStart,
    ConfigureForceTxRx { enabled: bool, phase: u8 },
    ResetFrequencyModule,
    SetHardwareFrequencyControl { enabled: bool },
    PulseI2cMasterReset { index: u8 },
    ConfigureXtal40Mhz,
    SetCalibrationClock { enabled: bool },
    SetBbpllCalibration { enabled: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterMmioCompletion {
    pub action: PhyRegisterMmioAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterAction {
    Mmio(PhyRegisterMmioAction),
    DelayMicros {
        phase: PhyRegisterDelayPhase,
        micros: u32,
    },
    SampleI2cMasterReset {
        index: u8,
        address: usize,
        busy_mask: u32,
        sample: u16,
    },
    Rf(crate::phy_i2c::PhyRfInitPrefixAction),
    Baseband(crate::phy_bb::PhyBbInitAction),
    Temperature(crate::phy_temperature::PhyTemperatureAction),
    ReadFinalI2c {
        address: crate::phy_i2c::PhyI2cAddress,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterCompletion {
    Mmio(PhyRegisterMmioCompletion),
    DelayElapsed {
        phase: PhyRegisterDelayPhase,
        micros: u32,
    },
    I2cMasterResetSampled {
        index: u8,
        address: usize,
        sample: u16,
        value: u32,
    },
    Rf(crate::phy_i2c::PhyRfInitPrefixCompletion),
    Baseband(crate::phy_bb::PhyBbInitCompletion),
    Temperature(crate::phy_temperature::PhyTemperatureCompletion),
    FinalI2cRead {
        address: crate::phy_i2c::PhyI2cAddress,
        value: u8,
    },
    FinalI2cDeadlineExceeded {
        address: crate::phy_i2c::PhyI2cAddress,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterFailure {
    I2cMasterResetTimedOut { index: u8, samples: u16 },
    Rf(crate::phy_i2c::PhyRfInitPrefixOutcome),
    Baseband(crate::phy_bb::PhyBbInitFailure),
    Temperature(crate::phy_temperature::PhyTemperatureFailure),
    FinalI2cDeadlineExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterOutcome {
    pub full_calibration_performed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterLocalStep {
    StateAdvanced,
    External(PhyRegisterAction),
    Complete(PhyRegisterOutcome),
    Failed(PhyRegisterFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterTransitionError {
    WrongCompletion,
    AlreadyComplete,
    MissingStateOwner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreludeStep {
    Prepare,
    ForceFirst,
    ForceFirstDelay,
    ForceSecond,
    ForceSecondDelay,
    FrequencyReset,
    DisableHardwareFrequency,
    DisableHardwareFrequencyDelay,
    I2cInitialSample { index: u8 },
    I2cResetPulse { index: u8 },
    I2cResetDelay { index: u8, sample: u16 },
    I2cResetSample { index: u8, sample: u16 },
    ApplyProfile,
    ConfigureXtal,
    CalibrationClockOn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TailStep {
    CalibrationClockOff,
    Temperature,
    BbpllOff,
    FinalI2cRead,
    MarkRegistered,
    EnableHardwareFrequency,
    ReleaseFirst,
    ReleaseFirstDelay,
    ReleaseSecond,
    ReleaseSecondDelay,
}

enum Phase {
    Prelude(PreludeStep),
    Rf(crate::phy_cold::PhyRfColdInit),
    Baseband(crate::phy_bb::PhyBbInitTransition),
    Temperature(crate::phy_temperature::PhyTemperatureTransition),
    Tail(TailStep),
    Cleanup {
        step: TailStep,
        failure: PhyRegisterFailure,
    },
    Complete(PhyRegisterOutcome),
    Failed(PhyRegisterFailure),
}

/// Complete full-calibration state machine replacing the stateful vendor
/// parent. Only one phase owns `PhyColdState` at a time: the outer transition,
/// `PhyRfColdInit`, or `PhyBbInitTransition`.
pub struct PhyRegisterTransition {
    state: Option<crate::phy_cold::PhyColdState>,
    profile: Option<[u8; PHY_REGISTER_INIT_PROFILE_LEN]>,
    phase: Option<Phase>,
}

impl PhyRegisterTransition {
    pub const fn new(profile: [u8; PHY_REGISTER_INIT_PROFILE_LEN]) -> Self {
        Self {
            state: Some(crate::phy_cold::PhyColdState::new()),
            profile: Some(profile),
            phase: Some(Phase::Prelude(PreludeStep::Prepare)),
        }
    }

    pub const fn with_default_profile() -> Self {
        Self::new(default_phy_register_init_profile())
    }

    pub fn state(&self) -> Option<&crate::phy_cold::PhyColdState> {
        self.state.as_ref().or_else(|| match self.phase.as_ref()? {
            Phase::Rf(transition) => Some(transition.state()),
            Phase::Baseband(transition) => Some(transition.state()),
            _ => None,
        })
    }

    pub fn into_state(mut self) -> Result<crate::phy_cold::PhyColdState, Self> {
        match self.phase {
            Some(Phase::Complete(_)) | Some(Phase::Failed(_)) => {
                if let Some(state) = self.state.take() {
                    Ok(state)
                } else {
                    Err(self)
                }
            }
            _ => Err(self),
        }
    }

    fn reset_address(index: u8) -> usize {
        if index == 0 {
            PHY_REGISTER_I2C_STATUS_0_ADDRESS
        } else {
            PHY_REGISTER_I2C_STATUS_1_ADDRESS
        }
    }

    fn begin_cleanup(&mut self, failure: PhyRegisterFailure) {
        self.phase = Some(Phase::Cleanup {
            step: TailStep::CalibrationClockOff,
            failure,
        });
    }

    fn tail_action(step: TailStep) -> Option<PhyRegisterAction> {
        match step {
            TailStep::CalibrationClockOff => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::SetCalibrationClock { enabled: false },
            )),
            TailStep::BbpllOff => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::SetBbpllCalibration { enabled: false },
            )),
            TailStep::FinalI2cRead => Some(PhyRegisterAction::ReadFinalI2c {
                address: PHY_REGISTER_FINAL_I2C_ADDRESS,
            }),
            TailStep::EnableHardwareFrequency => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true },
            )),
            TailStep::ReleaseFirst => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::ConfigureForceTxRx {
                    enabled: false,
                    phase: 0,
                },
            )),
            TailStep::ReleaseFirstDelay => Some(PhyRegisterAction::DelayMicros {
                phase: PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 0,
                },
                micros: 1,
            }),
            TailStep::ReleaseSecond => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::ConfigureForceTxRx {
                    enabled: false,
                    phase: 1,
                },
            )),
            TailStep::ReleaseSecondDelay => Some(PhyRegisterAction::DelayMicros {
                phase: PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 1,
                },
                micros: 1,
            }),
            TailStep::Temperature | TailStep::MarkRegistered => None,
        }
    }

    pub fn step_local(&mut self) -> Result<PhyRegisterLocalStep, PhyRegisterTransitionError> {
        let phase = self
            .phase
            .take()
            .ok_or(PhyRegisterTransitionError::AlreadyComplete)?;
        match phase {
            Phase::Prelude(PreludeStep::ApplyProfile) => {
                let profile = self
                    .profile
                    .take()
                    .ok_or(PhyRegisterTransitionError::MissingStateOwner)?;
                self.state
                    .as_mut()
                    .ok_or(PhyRegisterTransitionError::MissingStateOwner)?
                    .begin_full_wifi_calibration(&profile);
                self.phase = Some(Phase::Prelude(PreludeStep::ConfigureXtal));
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Prelude(step) => {
                let action = match step {
                    PreludeStep::Prepare => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::PrepareColdStart)
                    }
                    PreludeStep::ForceFirst => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ConfigureForceTxRx {
                            enabled: true,
                            phase: 0,
                        })
                    }
                    PreludeStep::ForceFirstDelay => PhyRegisterAction::DelayMicros {
                        phase: PhyRegisterDelayPhase::ForceTxRx {
                            enabled: true,
                            completed_phase: 0,
                        },
                        micros: 1,
                    },
                    PreludeStep::ForceSecond => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ConfigureForceTxRx {
                            enabled: true,
                            phase: 1,
                        })
                    }
                    PreludeStep::ForceSecondDelay => PhyRegisterAction::DelayMicros {
                        phase: PhyRegisterDelayPhase::ForceTxRx {
                            enabled: true,
                            completed_phase: 1,
                        },
                        micros: 1,
                    },
                    PreludeStep::FrequencyReset => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ResetFrequencyModule)
                    }
                    PreludeStep::DisableHardwareFrequency => PhyRegisterAction::Mmio(
                        PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: false },
                    ),
                    PreludeStep::DisableHardwareFrequencyDelay => PhyRegisterAction::DelayMicros {
                        phase: PhyRegisterDelayPhase::HardwareFrequencyDisabled,
                        micros: 2,
                    },
                    PreludeStep::I2cInitialSample { index }
                    | PreludeStep::I2cResetSample { index, sample: 0 } => {
                        PhyRegisterAction::SampleI2cMasterReset {
                            index,
                            address: Self::reset_address(index),
                            busy_mask: 1 << 25,
                            sample: 0,
                        }
                    }
                    PreludeStep::I2cResetSample { index, sample } => {
                        PhyRegisterAction::SampleI2cMasterReset {
                            index,
                            address: Self::reset_address(index),
                            busy_mask: 1 << 25,
                            sample,
                        }
                    }
                    PreludeStep::I2cResetPulse { index } => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::PulseI2cMasterReset {
                            index,
                        })
                    }
                    PreludeStep::I2cResetDelay { index, sample } => {
                        PhyRegisterAction::DelayMicros {
                            phase: PhyRegisterDelayPhase::I2cMasterReset { index, sample },
                            micros: 1,
                        }
                    }
                    PreludeStep::ConfigureXtal => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ConfigureXtal40Mhz)
                    }
                    PreludeStep::CalibrationClockOn => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::SetCalibrationClock {
                            enabled: true,
                        })
                    }
                    PreludeStep::ApplyProfile => unreachable!(),
                };
                self.phase = Some(Phase::Prelude(step));
                Ok(PhyRegisterLocalStep::External(action))
            }
            Phase::Rf(mut transition) => match transition.step_local()? {
                crate::phy_cold::PhyColdLocalStep::StateAdvanced => {
                    self.phase = Some(Phase::Rf(transition));
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
                crate::phy_cold::PhyColdLocalStep::External(action) => {
                    self.phase = Some(Phase::Rf(transition));
                    Ok(PhyRegisterLocalStep::External(PhyRegisterAction::Rf(
                        action,
                    )))
                }
                crate::phy_cold::PhyColdLocalStep::Complete(outcome) => {
                    let successful = matches!(
                        outcome,
                        crate::phy_i2c::PhyRfInitPrefixOutcome::ChannelFrequencyInitialized { .. }
                    );
                    let state = transition.into_state();
                    if successful {
                        self.phase = Some(Phase::Baseband(
                            crate::phy_bb::PhyBbInitTransition::new(state),
                        ));
                    } else {
                        self.state = Some(state);
                        self.begin_cleanup(PhyRegisterFailure::Rf(outcome));
                    }
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
            },
            Phase::Baseband(mut transition) => match transition.step_local()? {
                crate::phy_bb::PhyBbInitLocalStep::StateAdvanced => {
                    self.phase = Some(Phase::Baseband(transition));
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
                crate::phy_bb::PhyBbInitLocalStep::External(action) => {
                    self.phase = Some(Phase::Baseband(transition));
                    Ok(PhyRegisterLocalStep::External(PhyRegisterAction::Baseband(
                        action,
                    )))
                }
                crate::phy_bb::PhyBbInitLocalStep::Complete(_) => {
                    self.state = Some(transition.into_state());
                    self.phase = Some(Phase::Tail(TailStep::CalibrationClockOff));
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
                crate::phy_bb::PhyBbInitLocalStep::Failed(failure) => {
                    self.state = Some(transition.into_state());
                    self.begin_cleanup(PhyRegisterFailure::Baseband(failure));
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
            },
            Phase::Temperature(transition) => match transition.action() {
                crate::phy_temperature::PhyTemperatureAction::Complete(outcome) => {
                    self.state
                        .as_mut()
                        .ok_or(PhyRegisterTransitionError::MissingStateOwner)?
                        .apply_full_calibration_temperature(outcome);
                    self.phase = Some(Phase::Tail(TailStep::BbpllOff));
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
                crate::phy_temperature::PhyTemperatureAction::Failed(failure) => {
                    self.begin_cleanup(PhyRegisterFailure::Temperature(failure));
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
                action => {
                    self.phase = Some(Phase::Temperature(transition));
                    Ok(PhyRegisterLocalStep::External(
                        PhyRegisterAction::Temperature(action),
                    ))
                }
            },
            Phase::Tail(TailStep::Temperature) => {
                self.phase = Some(Phase::Temperature(
                    crate::phy_temperature::PhyTemperatureTransition::new(),
                ));
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Tail(TailStep::MarkRegistered) => {
                self.state
                    .as_mut()
                    .ok_or(PhyRegisterTransitionError::MissingStateOwner)?
                    .mark_phy_registered();
                self.phase = Some(Phase::Tail(TailStep::EnableHardwareFrequency));
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Tail(step) => {
                let action =
                    Self::tail_action(step).ok_or(PhyRegisterTransitionError::WrongCompletion)?;
                self.phase = Some(Phase::Tail(step));
                Ok(PhyRegisterLocalStep::External(action))
            }
            Phase::Cleanup {
                step: TailStep::Temperature | TailStep::FinalI2cRead | TailStep::MarkRegistered,
                failure,
            } => {
                self.phase = Some(Phase::Cleanup {
                    step: TailStep::BbpllOff,
                    failure,
                });
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Cleanup { step, failure } => {
                let action =
                    Self::tail_action(step).ok_or(PhyRegisterTransitionError::WrongCompletion)?;
                self.phase = Some(Phase::Cleanup { step, failure });
                Ok(PhyRegisterLocalStep::External(action))
            }
            Phase::Complete(outcome) => {
                self.phase = Some(Phase::Complete(outcome));
                Ok(PhyRegisterLocalStep::Complete(outcome))
            }
            Phase::Failed(failure) => {
                self.phase = Some(Phase::Failed(failure));
                Ok(PhyRegisterLocalStep::Failed(failure))
            }
        }
    }

    pub fn advance_external(
        &mut self,
        completion: PhyRegisterCompletion,
    ) -> Result<(), PhyRegisterTransitionError> {
        let phase = self
            .phase
            .take()
            .ok_or(PhyRegisterTransitionError::AlreadyComplete)?;
        let next = match (phase, completion) {
            (Phase::Prelude(PreludeStep::Prepare), PhyRegisterCompletion::Mmio(completed))
                if completed.action == PhyRegisterMmioAction::PrepareColdStart =>
            {
                Phase::Prelude(PreludeStep::ForceFirst)
            }
            (Phase::Prelude(PreludeStep::ForceFirst), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        enabled: true,
                        phase: 0,
                    }) =>
            {
                Phase::Prelude(PreludeStep::ForceFirstDelay)
            }
            (
                Phase::Prelude(PreludeStep::ForceFirstDelay),
                PhyRegisterCompletion::DelayElapsed { phase, micros: 1 },
            ) if phase
                == (PhyRegisterDelayPhase::ForceTxRx {
                    enabled: true,
                    completed_phase: 0,
                }) =>
            {
                Phase::Prelude(PreludeStep::ForceSecond)
            }
            (Phase::Prelude(PreludeStep::ForceSecond), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        enabled: true,
                        phase: 1,
                    }) =>
            {
                Phase::Prelude(PreludeStep::ForceSecondDelay)
            }
            (
                Phase::Prelude(PreludeStep::ForceSecondDelay),
                PhyRegisterCompletion::DelayElapsed { phase, micros: 1 },
            ) if phase
                == (PhyRegisterDelayPhase::ForceTxRx {
                    enabled: true,
                    completed_phase: 1,
                }) =>
            {
                Phase::Prelude(PreludeStep::FrequencyReset)
            }
            (
                Phase::Prelude(PreludeStep::FrequencyReset),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action == PhyRegisterMmioAction::ResetFrequencyModule => {
                Phase::Prelude(PreludeStep::DisableHardwareFrequency)
            }
            (
                Phase::Prelude(PreludeStep::DisableHardwareFrequency),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: false }) =>
            {
                Phase::Prelude(PreludeStep::DisableHardwareFrequencyDelay)
            }
            (
                Phase::Prelude(PreludeStep::DisableHardwareFrequencyDelay),
                PhyRegisterCompletion::DelayElapsed {
                    phase: PhyRegisterDelayPhase::HardwareFrequencyDisabled,
                    micros: 2,
                },
            ) => Phase::Prelude(PreludeStep::I2cInitialSample { index: 0 }),
            (
                Phase::Prelude(PreludeStep::I2cInitialSample { index }),
                PhyRegisterCompletion::I2cMasterResetSampled {
                    index: completed_index,
                    address,
                    sample: 0,
                    value,
                },
            ) if completed_index == index && address == Self::reset_address(index) => {
                if crate::radio_hal::phy_i2c_master_reset_busy(value) {
                    Phase::Prelude(PreludeStep::I2cResetPulse { index })
                } else if index == 0 {
                    Phase::Prelude(PreludeStep::I2cInitialSample { index: 1 })
                } else {
                    Phase::Prelude(PreludeStep::ApplyProfile)
                }
            }
            (
                Phase::Prelude(PreludeStep::I2cResetPulse { index }),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action == PhyRegisterMmioAction::PulseI2cMasterReset { index } => {
                Phase::Prelude(PreludeStep::I2cResetDelay { index, sample: 0 })
            }
            (
                Phase::Prelude(PreludeStep::I2cResetDelay { index, sample }),
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::I2cMasterReset {
                            index: completed_index,
                            sample: completed_sample,
                        },
                    micros: 1,
                },
            ) if completed_index == index && completed_sample == sample => {
                Phase::Prelude(PreludeStep::I2cResetSample { index, sample })
            }
            (
                Phase::Prelude(PreludeStep::I2cResetSample { index, sample }),
                PhyRegisterCompletion::I2cMasterResetSampled {
                    index: completed_index,
                    address,
                    sample: completed_sample,
                    value,
                },
            ) if completed_index == index
                && completed_sample == sample
                && address == Self::reset_address(index) =>
            {
                if !crate::radio_hal::phy_i2c_master_reset_busy(value) {
                    if index == 0 {
                        Phase::Prelude(PreludeStep::I2cInitialSample { index: 1 })
                    } else {
                        Phase::Prelude(PreludeStep::ApplyProfile)
                    }
                } else if sample + 1 == PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT {
                    self.begin_cleanup(PhyRegisterFailure::I2cMasterResetTimedOut {
                        index,
                        samples: sample + 1,
                    });
                    return Ok(());
                } else {
                    Phase::Prelude(PreludeStep::I2cResetDelay {
                        index,
                        sample: sample + 1,
                    })
                }
            }
            (
                Phase::Prelude(PreludeStep::ConfigureXtal),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action == PhyRegisterMmioAction::ConfigureXtal40Mhz => {
                Phase::Prelude(PreludeStep::CalibrationClockOn)
            }
            (
                Phase::Prelude(PreludeStep::CalibrationClockOn),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetCalibrationClock { enabled: true }) =>
            {
                let state = self
                    .state
                    .take()
                    .ok_or(PhyRegisterTransitionError::MissingStateOwner)?;
                Phase::Rf(crate::phy_cold::PhyRfColdInit::new(state))
            }
            (Phase::Rf(mut transition), PhyRegisterCompletion::Rf(completed)) => {
                transition.advance_external(completed)?;
                Phase::Rf(transition)
            }
            (Phase::Baseband(mut transition), PhyRegisterCompletion::Baseband(completed)) => {
                transition.advance_external(completed)?;
                Phase::Baseband(transition)
            }
            (
                Phase::Tail(TailStep::CalibrationClockOff),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetCalibrationClock { enabled: false }) =>
            {
                Phase::Tail(TailStep::Temperature)
            }
            (Phase::Temperature(mut transition), PhyRegisterCompletion::Temperature(completed)) => {
                transition
                    .advance(completed)
                    .map_err(|_| PhyRegisterTransitionError::WrongCompletion)?;
                Phase::Temperature(transition)
            }
            (Phase::Tail(TailStep::BbpllOff), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::SetBbpllCalibration { enabled: false }) =>
            {
                Phase::Tail(TailStep::FinalI2cRead)
            }
            (
                Phase::Tail(TailStep::FinalI2cRead),
                PhyRegisterCompletion::FinalI2cRead { address, .. },
            ) if address == PHY_REGISTER_FINAL_I2C_ADDRESS => Phase::Tail(TailStep::MarkRegistered),
            (
                Phase::Tail(TailStep::FinalI2cRead),
                PhyRegisterCompletion::FinalI2cDeadlineExceeded { address },
            ) if address == PHY_REGISTER_FINAL_I2C_ADDRESS => {
                self.begin_cleanup(PhyRegisterFailure::FinalI2cDeadlineExceeded);
                return Ok(());
            }
            (
                Phase::Tail(TailStep::EnableHardwareFrequency),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true }) =>
            {
                Phase::Tail(TailStep::ReleaseFirst)
            }
            (Phase::Tail(TailStep::ReleaseFirst), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        enabled: false,
                        phase: 0,
                    }) =>
            {
                Phase::Tail(TailStep::ReleaseFirstDelay)
            }
            (
                Phase::Tail(TailStep::ReleaseFirstDelay),
                PhyRegisterCompletion::DelayElapsed { phase, micros: 1 },
            ) if phase
                == (PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 0,
                }) =>
            {
                Phase::Tail(TailStep::ReleaseSecond)
            }
            (Phase::Tail(TailStep::ReleaseSecond), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        enabled: false,
                        phase: 1,
                    }) =>
            {
                Phase::Tail(TailStep::ReleaseSecondDelay)
            }
            (
                Phase::Tail(TailStep::ReleaseSecondDelay),
                PhyRegisterCompletion::DelayElapsed { phase, micros: 1 },
            ) if phase
                == (PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 1,
                }) =>
            {
                Phase::Complete(PhyRegisterOutcome {
                    full_calibration_performed: true,
                })
            }
            (
                Phase::Cleanup {
                    step: TailStep::CalibrationClockOff,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetCalibrationClock { enabled: false }) =>
            {
                Phase::Cleanup {
                    step: TailStep::BbpllOff,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::BbpllOff,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetBbpllCalibration { enabled: false }) =>
            {
                Phase::Cleanup {
                    step: TailStep::EnableHardwareFrequency,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::EnableHardwareFrequency,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true }) =>
            {
                Phase::Cleanup {
                    step: TailStep::ReleaseFirst,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::ReleaseFirst,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::ConfigureForceTxRx {
                    enabled: false,
                    phase: 0,
                }) =>
            {
                Phase::Cleanup {
                    step: TailStep::ReleaseFirstDelay,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::ReleaseFirstDelay,
                    failure,
                },
                PhyRegisterCompletion::DelayElapsed { phase, micros: 1 },
            ) if phase
                == (PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 0,
                }) =>
            {
                Phase::Cleanup {
                    step: TailStep::ReleaseSecond,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::ReleaseSecond,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::ConfigureForceTxRx {
                    enabled: false,
                    phase: 1,
                }) =>
            {
                Phase::Cleanup {
                    step: TailStep::ReleaseSecondDelay,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::ReleaseSecondDelay,
                    failure,
                },
                PhyRegisterCompletion::DelayElapsed { phase, micros: 1 },
            ) if phase
                == (PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 1,
                }) =>
            {
                Phase::Failed(failure)
            }
            (Phase::Complete(outcome), _) => {
                self.phase = Some(Phase::Complete(outcome));
                return Err(PhyRegisterTransitionError::AlreadyComplete);
            }
            (Phase::Failed(failure), _) => {
                self.phase = Some(Phase::Failed(failure));
                return Err(PhyRegisterTransitionError::AlreadyComplete);
            }
            (phase, _) => {
                self.phase = Some(phase);
                return Err(PhyRegisterTransitionError::WrongCompletion);
            }
        };
        self.phase = Some(next);
        Ok(())
    }
}

impl From<crate::phy_cold::PhyColdTransitionError> for PhyRegisterTransitionError {
    fn from(error: crate::phy_cold::PhyColdTransitionError) -> Self {
        match error {
            crate::phy_cold::PhyColdTransitionError::WrongCompletion => Self::WrongCompletion,
            crate::phy_cold::PhyColdTransitionError::AlreadyComplete => Self::AlreadyComplete,
        }
    }
}

impl From<crate::phy_bb::PhyBbInitTransitionError> for PhyRegisterTransitionError {
    fn from(error: crate::phy_bb::PhyBbInitTransitionError) -> Self {
        match error {
            crate::phy_bb::PhyBbInitTransitionError::WrongCompletion => Self::WrongCompletion,
            crate::phy_bb::PhyBbInitTransitionError::AlreadyComplete => Self::AlreadyComplete,
        }
    }
}

/// Non-cloneable identity token for one finite parent MMIO operation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterMmioBinding {
    action: PhyRegisterMmioAction,
}

impl PhyRegisterMmioBinding {
    pub const fn new(action: PhyRegisterMmioAction) -> Self {
        Self { action }
    }

    pub const fn action(&self) -> PhyRegisterMmioAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyRegisterMmioCompletion {
        match self.action {
            PhyRegisterMmioAction::PrepareColdStart => {
                crate::radio_hal::prepare_phy_register_cold_start()
            }
            PhyRegisterMmioAction::ConfigureForceTxRx { enabled, phase } => {
                crate::radio_hal::configure_phy_register_force_txrx(enabled, phase)
            }
            PhyRegisterMmioAction::ResetFrequencyModule => {
                crate::radio_hal::reset_phy_frequency_module()
            }
            PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled } => {
                crate::radio_hal::set_phy_hardware_frequency_control(enabled)
            }
            PhyRegisterMmioAction::PulseI2cMasterReset { index } => {
                crate::radio_hal::pulse_phy_i2c_master_reset(index)
            }
            PhyRegisterMmioAction::ConfigureXtal40Mhz => {
                crate::radio_hal::configure_phy_register_xtal_frequency()
            }
            PhyRegisterMmioAction::SetCalibrationClock { enabled } => {
                crate::radio_hal::set_phy_register_calibration_clock(enabled)
            }
            PhyRegisterMmioAction::SetBbpllCalibration { enabled } => {
                crate::radio_hal::wifi_strict_phy_bbpll_cal(u32::from(enabled))
            }
        }
        PhyRegisterMmioCompletion {
            action: self.action,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterResetSampleBinding {
    index: u8,
    address: usize,
    sample: u16,
}

impl PhyRegisterResetSampleBinding {
    pub fn new(action: PhyRegisterAction) -> Option<Self> {
        match action {
            PhyRegisterAction::SampleI2cMasterReset {
                index,
                address,
                busy_mask,
                sample,
            } if address == PhyRegisterTransition::reset_address(index) && busy_mask == 1 << 25 => {
                Some(Self {
                    index,
                    address,
                    sample,
                })
            }
            _ => None,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyRegisterCompletion {
        PhyRegisterCompletion::I2cMasterResetSampled {
            index: self.index,
            address: self.address,
            sample: self.sample,
            value: crate::radio_hal::sample_phy_i2c_master_reset(self.index),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Consumed identity for one parent delay completed by the Rust executor.
///
/// This type contains no clock source and cannot expire itself. The executor
/// owns the deadline and consumes this token only after its async timer edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterTimerBinding {
    phase: PhyRegisterDelayPhase,
    micros: u32,
}

impl PhyRegisterTimerBinding {
    pub fn new(action: PhyRegisterAction) -> Result<Self, PhyRegisterBindingError> {
        match action {
            PhyRegisterAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyRegisterBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRegisterCompletion {
        PhyRegisterCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Non-cloneable owner of the final one-byte PHY-I2C read.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterFinalI2cBinding {
    address: crate::phy_i2c::PhyI2cAddress,
    transaction: crate::phy_cold::PhyColdI2cTransaction,
}

impl PhyRegisterFinalI2cBinding {
    pub fn new(action: PhyRegisterAction) -> Result<Self, PhyRegisterBindingError> {
        let PhyRegisterAction::ReadFinalI2c { address } = action else {
            return Err(PhyRegisterBindingError::UnsupportedAction);
        };
        Ok(Self {
            address,
            transaction: crate::phy_cold::PhyColdI2cTransaction::new(
                crate::phy_cold::PhyColdI2cRequest::read_byte(address),
            ),
        })
    }

    pub const fn action(&self) -> crate::phy_cold::PhyColdI2cAction {
        self.transaction.action()
    }

    pub fn read_started(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, crate::phy_i2c::PhyI2cError>,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_read_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn start_target(&mut self) -> Result<(), crate::phy_cold::PhyColdI2cError> {
        self.transaction.start_target()
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn observe_target_edge(
        &mut self,
    ) -> Result<crate::phy_cold::PhyColdI2cObservation, crate::phy_cold::PhyColdI2cError> {
        self.transaction.observe_target_edge()
    }

    pub fn into_completion(self) -> Result<PhyRegisterCompletion, PhyRegisterBindingError> {
        match self.transaction.action() {
            crate::phy_cold::PhyColdI2cAction::Complete(
                crate::phy_cold::PhyColdI2cOutcome::Read { address, value },
            ) if address == self.address => {
                Ok(PhyRegisterCompletion::FinalI2cRead { address, value })
            }
            crate::phy_cold::PhyColdI2cAction::Complete(_) => {
                Err(PhyRegisterBindingError::UnexpectedOutcome)
            }
            _ => Err(PhyRegisterBindingError::IncompleteTransaction),
        }
    }

    pub const fn into_deadline_completion(self) -> PhyRegisterCompletion {
        PhyRegisterCompletion::FinalI2cDeadlineExceeded {
            address: self.address,
        }
    }
}

/// Exhaustive source-owned lowering of `register_chipv7_phy`.
///
/// A successful construction proves that the next parent operation can be
/// driven without calling the vendor parent, consulting its callback table,
/// or performing a synchronous wait. Terminal state is intentionally not an
/// external binding.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRegisterExternalBinding {
    Mmio(PhyRegisterMmioBinding),
    Timer(PhyRegisterTimerBinding),
    ResetSample(PhyRegisterResetSampleBinding),
    Rf(crate::phy_cold::PhyColdExternalBinding),
    Baseband(crate::phy_bb::PhyBbExternalBinding),
    Temperature(crate::phy_temperature::PhyTemperatureExternalBinding),
    FinalI2c(PhyRegisterFinalI2cBinding),
}

impl PhyRegisterExternalBinding {
    pub fn lower(action: PhyRegisterAction) -> Result<Self, PhyRegisterBindingError> {
        match action {
            PhyRegisterAction::Mmio(action) => Ok(Self::Mmio(PhyRegisterMmioBinding::new(action))),
            PhyRegisterAction::DelayMicros { .. } => {
                PhyRegisterTimerBinding::new(action).map(Self::Timer)
            }
            PhyRegisterAction::SampleI2cMasterReset { .. } => {
                PhyRegisterResetSampleBinding::new(action)
                    .map(Self::ResetSample)
                    .ok_or(PhyRegisterBindingError::UnsupportedAction)
            }
            PhyRegisterAction::Rf(action) => crate::phy_cold::PhyColdExternalBinding::lower(action)
                .map(Self::Rf)
                .map_err(|_| PhyRegisterBindingError::UnsupportedAction),
            PhyRegisterAction::Baseband(action) => {
                crate::phy_bb::PhyBbExternalBinding::lower(action)
                    .map(Self::Baseband)
                    .map_err(|_| PhyRegisterBindingError::UnsupportedAction)
            }
            PhyRegisterAction::Temperature(action) => {
                crate::phy_temperature::PhyTemperatureExternalBinding::lower(action)
                    .map(Self::Temperature)
                    .map_err(|_| PhyRegisterBindingError::UnsupportedAction)
            }
            PhyRegisterAction::ReadFinalI2c { .. } => {
                PhyRegisterFinalI2cBinding::new(action).map(Self::FinalI2c)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        default_phy_register_init_profile, PhyRegisterAction, PhyRegisterBindingError,
        PhyRegisterCompletion, PhyRegisterDelayPhase, PhyRegisterFailure,
        PhyRegisterFinalI2cBinding, PhyRegisterLocalStep, PhyRegisterMmioAction,
        PhyRegisterMmioCompletion, PhyRegisterOutcome, PhyRegisterResetSampleBinding,
        PhyRegisterTimerBinding, PhyRegisterTransition, PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT,
        PHY_REGISTER_I2C_STATUS_0_ADDRESS,
    };

    fn complete_mmio(transition: &mut PhyRegisterTransition, action: PhyRegisterMmioAction) {
        transition
            .advance_external(PhyRegisterCompletion::Mmio(PhyRegisterMmioCompletion {
                action,
            }))
            .unwrap();
    }

    fn complete_delay(
        transition: &mut PhyRegisterTransition,
        phase: PhyRegisterDelayPhase,
        micros: u32,
    ) {
        transition
            .advance_external(PhyRegisterCompletion::DelayElapsed { phase, micros })
            .unwrap();
    }

    #[test]
    fn default_profile_matches_the_parent_stack_image() {
        let profile = default_phy_register_init_profile();
        assert_eq!(
            &profile[..0x14],
            &[
                1, 0, 0x54, 0x54, 0x50, 0x50, 0x4c, 0x48, 0x50, 0x50, 0x4c, 0x44, 0x40, 0x38, 0x38,
                0x38, 0x4c, 0x4c, 0x48, 0x44,
            ]
        );
        assert!(profile[0x14..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn state_owner_cannot_escape_before_a_terminal_parent_outcome() {
        let transition = PhyRegisterTransition::with_default_profile();
        let transition = match transition.into_state() {
            Ok(_) => panic!("an active cold initializer must retain its unique state owner"),
            Err(transition) => transition,
        };
        assert!(transition.state().is_some());
    }

    #[test]
    fn parent_timer_and_final_i2c_bindings_preserve_action_identity() {
        let phase = PhyRegisterDelayPhase::HardwareFrequencyDisabled;
        let timer =
            PhyRegisterTimerBinding::new(PhyRegisterAction::DelayMicros { phase, micros: 2 })
                .unwrap();
        assert_eq!(timer.micros(), 2);
        assert_eq!(
            timer.into_completion(),
            PhyRegisterCompletion::DelayElapsed { phase, micros: 2 }
        );

        let address = super::PHY_REGISTER_FINAL_I2C_ADDRESS;
        let mut i2c =
            PhyRegisterFinalI2cBinding::new(PhyRegisterAction::ReadFinalI2c { address }).unwrap();
        assert_eq!(
            i2c.action(),
            crate::phy_cold::PhyColdI2cAction::StartRead { address }
        );
        i2c.read_started().unwrap();
        i2c.observe_read_result(Ok(0x5a)).unwrap();
        assert_eq!(
            i2c.into_completion().unwrap(),
            PhyRegisterCompletion::FinalI2cRead {
                address,
                value: 0x5a,
            }
        );
        assert_eq!(
            PhyRegisterTimerBinding::new(PhyRegisterAction::ReadFinalI2c { address }),
            Err(PhyRegisterBindingError::UnsupportedAction)
        );
    }

    #[test]
    fn prelude_has_async_delay_and_reset_sample_edges() {
        let mut transition = PhyRegisterTransition::with_default_profile();
        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::External(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::PrepareColdStart
            ))
        );
        complete_mmio(&mut transition, PhyRegisterMmioAction::PrepareColdStart);
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::ConfigureForceTxRx {
                enabled: true,
                phase: 0,
            },
        );
        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::External(PhyRegisterAction::DelayMicros {
                phase: PhyRegisterDelayPhase::ForceTxRx {
                    enabled: true,
                    completed_phase: 0,
                },
                micros: 1,
            })
        );
        complete_delay(
            &mut transition,
            PhyRegisterDelayPhase::ForceTxRx {
                enabled: true,
                completed_phase: 0,
            },
            1,
        );
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::ConfigureForceTxRx {
                enabled: true,
                phase: 1,
            },
        );
        complete_delay(
            &mut transition,
            PhyRegisterDelayPhase::ForceTxRx {
                enabled: true,
                completed_phase: 1,
            },
            1,
        );
        complete_mmio(&mut transition, PhyRegisterMmioAction::ResetFrequencyModule);
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: false },
        );
        complete_delay(
            &mut transition,
            PhyRegisterDelayPhase::HardwareFrequencyDisabled,
            2,
        );
        let action = transition.step_local().unwrap();
        assert_eq!(
            action,
            PhyRegisterLocalStep::External(PhyRegisterAction::SampleI2cMasterReset {
                index: 0,
                address: PHY_REGISTER_I2C_STATUS_0_ADDRESS,
                busy_mask: 1 << 25,
                sample: 0,
            })
        );
        assert!(PhyRegisterResetSampleBinding::new(match action {
            PhyRegisterLocalStep::External(action) => action,
            _ => unreachable!(),
        })
        .is_some());
    }

    #[test]
    fn stuck_i2c_reset_fails_only_after_bounded_async_samples_and_cleans_up() {
        let mut transition = PhyRegisterTransition::with_default_profile();
        transition.phase = Some(super::Phase::Prelude(super::PreludeStep::I2cResetSample {
            index: 0,
            sample: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT - 1,
        }));
        transition
            .advance_external(PhyRegisterCompletion::I2cMasterResetSampled {
                index: 0,
                address: PHY_REGISTER_I2C_STATUS_0_ADDRESS,
                sample: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT - 1,
                value: 1 << 25,
            })
            .unwrap();
        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::External(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::SetCalibrationClock { enabled: false }
            ))
        );
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::SetCalibrationClock { enabled: false },
        );
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::SetBbpllCalibration { enabled: false },
        );
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true },
        );
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::ConfigureForceTxRx {
                enabled: false,
                phase: 0,
            },
        );
        complete_delay(
            &mut transition,
            PhyRegisterDelayPhase::ForceTxRx {
                enabled: false,
                completed_phase: 0,
            },
            1,
        );
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::ConfigureForceTxRx {
                enabled: false,
                phase: 1,
            },
        );
        complete_delay(
            &mut transition,
            PhyRegisterDelayPhase::ForceTxRx {
                enabled: false,
                completed_phase: 1,
            },
            1,
        );
        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::Failed(PhyRegisterFailure::I2cMasterResetTimedOut {
                index: 0,
                samples: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT,
            })
        );
    }

    #[test]
    fn success_tail_marks_owned_state_before_releasing_radio() {
        let mut transition = PhyRegisterTransition::with_default_profile();
        transition.profile = None;
        transition.phase = Some(super::Phase::Tail(super::TailStep::MarkRegistered));
        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::StateAdvanced
        );
        assert!(transition.state().unwrap().phy_registered());
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true },
        );
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::ConfigureForceTxRx {
                enabled: false,
                phase: 0,
            },
        );
        complete_delay(
            &mut transition,
            PhyRegisterDelayPhase::ForceTxRx {
                enabled: false,
                completed_phase: 0,
            },
            1,
        );
        complete_mmio(
            &mut transition,
            PhyRegisterMmioAction::ConfigureForceTxRx {
                enabled: false,
                phase: 1,
            },
        );
        complete_delay(
            &mut transition,
            PhyRegisterDelayPhase::ForceTxRx {
                enabled: false,
                completed_phase: 1,
            },
            1,
        );
        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::Complete(PhyRegisterOutcome {
                full_calibration_performed: true,
            })
        );
        let state = match transition.into_state() {
            Ok(state) => state,
            Err(_) => panic!("a completed cold initializer must release its state owner"),
        };
        assert!(state.phy_registered());
    }
}
