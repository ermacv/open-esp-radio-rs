//! Rust-owned composition of ESP32-S31 `register_chipv7_phy`.
//!
//! The pinned archive parent is 486 bytes. Persistence remains outside the
//! radio driver: callers may provide and retrieve a typed calibration cache,
//! while this transition owns validation, recovery and fallback to full
//! calibration. All retained radio work is represented by owned state plus
//! one externally completed MMIO, PHY-I2C, timer or observation edge.

pub const PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT: u16 = 1_000;

const PHY_REGISTER_FINAL_I2C_ADDRESS: crate::phy_i2c::PhyI2cAddress =
    crate::phy_i2c::PhyI2cAddress::new_internal(0x63, 0);

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
        sample: u16,
        busy: bool,
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
pub struct PhyCalibrationIdentity {
    pub rf_cal_version: u32,
    pub mac_sys0: u32,
    pub mac_sys1: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationPath {
    /// No caller-owned persistence was requested.
    FullUncached,
    /// No retained cache was supplied; a fresh cache is produced.
    FullForCache,
    /// A supplied cache could not be safely replayed; full calibration
    /// replaces it.
    FullAfterRejectedCache,
}

impl PhyCalibrationPath {
    pub const fn cache_available(self) -> bool {
        !matches!(self, Self::FullUncached)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterOutcome {
    pub full_calibration_performed: bool,
    pub calibration_path: PhyCalibrationPath,
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
    BackupCalibration,
    BbpllOff,
    FinalI2cRead,
    MarkRegistered,
    EnableHardwareFrequency,
    ReleaseFirst,
    ReleaseFirstDelay,
    ReleaseSecond,
    ReleaseSecondDelay,
}

#[allow(
    clippy::large_enum_variant,
    reason = "the heap-free registration owner retains exactly one complete calibration phase"
)]
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
/// parent. Only one phase owns `PhyState` at a time: the outer transition,
/// `PhyRfColdInit`, or `PhyBbInitTransition`.
pub struct PhyRegisterTransition {
    state: Option<crate::phy_state::PhyState>,
    config: Option<crate::phy_state::PhyConfig>,
    channel_or_frequency: u16,
    calibration_identity: Option<PhyCalibrationIdentity>,
    calibration_cache: Option<crate::phy_state::PhyCalibrationCache>,
    calibration_candidate: bool,
    calibration_cache_ready: bool,
    calibration_path: PhyCalibrationPath,
    temperature_control: Option<crate::phy_cold::PhyRegisterTemperatureControl>,
    phase: Option<Phase>,
}

impl PhyRegisterTransition {
    pub const fn new(config: crate::phy_state::PhyConfig) -> Self {
        Self::new_on_channel(config, 11)
    }

    pub const fn new_on_channel(
        config: crate::phy_state::PhyConfig,
        channel_or_frequency: u16,
    ) -> Self {
        Self {
            state: Some(crate::phy_state::PhyState::new(
                crate::phy_state::PhyConfig::esp32s31_default(),
            )),
            config: Some(config),
            channel_or_frequency,
            calibration_identity: None,
            calibration_cache: None,
            calibration_candidate: false,
            calibration_cache_ready: false,
            calibration_path: PhyCalibrationPath::FullUncached,
            temperature_control: None,
            phase: Some(Phase::Prelude(PreludeStep::Prepare)),
        }
    }

    /// Construct a cold transition with caller-owned calibration persistence.
    ///
    /// `identity` is obtained by the platform layer (RF-calibration ABI
    /// version plus the two eFuse identity words). `cache` may come from any
    /// caller-selected backend. This type never reads eFuse, NVS or flash.
    pub const fn new_on_channel_with_calibration(
        config: crate::phy_state::PhyConfig,
        channel_or_frequency: u16,
        identity: PhyCalibrationIdentity,
        cache: Option<crate::phy_state::PhyCalibrationCache>,
    ) -> Self {
        let calibration_candidate = cache.is_some();
        Self {
            state: Some(crate::phy_state::PhyState::new(
                crate::phy_state::PhyConfig::esp32s31_default(),
            )),
            config: Some(config),
            channel_or_frequency,
            calibration_identity: Some(identity),
            calibration_cache: cache,
            calibration_candidate,
            calibration_cache_ready: false,
            calibration_path: if calibration_candidate {
                PhyCalibrationPath::FullAfterRejectedCache
            } else {
                PhyCalibrationPath::FullForCache
            },
            temperature_control: None,
            phase: Some(Phase::Prelude(PreludeStep::Prepare)),
        }
    }

    pub const fn with_production_config() -> Self {
        Self::new(crate::phy_state::PhyConfig::production())
    }

    pub const fn with_production_config_on_channel(channel_or_frequency: u16) -> Self {
        Self::new_on_channel(
            crate::phy_state::PhyConfig::production(),
            channel_or_frequency,
        )
    }

    pub const fn with_production_config_and_calibration(
        identity: PhyCalibrationIdentity,
        cache: Option<crate::phy_state::PhyCalibrationCache>,
    ) -> Self {
        Self::new_on_channel_with_calibration(
            crate::phy_state::PhyConfig::production(),
            11,
            identity,
            cache,
        )
    }

    pub const fn with_production_config_on_channel_and_calibration(
        channel_or_frequency: u16,
        identity: PhyCalibrationIdentity,
        cache: Option<crate::phy_state::PhyCalibrationCache>,
    ) -> Self {
        Self::new_on_channel_with_calibration(
            crate::phy_state::PhyConfig::production(),
            channel_or_frequency,
            identity,
            cache,
        )
    }

    /// Return the freshly completed full-calibration cache.
    ///
    /// A supplied cache is validation input only until complete hardware
    /// replay exists. Failed/in-progress transitions never expose a cache as
    /// persistable.
    pub fn calibration_cache(&self) -> Option<&crate::phy_state::PhyCalibrationCache> {
        self.calibration_cache_ready
            .then_some(())
            .and(self.calibration_cache.as_ref())
    }

    pub fn take_calibration_cache(&mut self) -> Option<crate::phy_state::PhyCalibrationCache> {
        if self.calibration_cache_ready {
            self.calibration_cache_ready = false;
            self.calibration_cache.take()
        } else {
            None
        }
    }

    pub fn state(&self) -> Option<&crate::phy_state::PhyState> {
        self.state.as_ref().or_else(|| match self.phase.as_ref()? {
            Phase::Rf(transition) => Some(transition.state()),
            Phase::Baseband(transition) => Some(transition.state()),
            _ => None,
        })
    }

    #[allow(
        clippy::result_large_err,
        reason = "failure must return the unique allocation-free registration owner"
    )]
    pub fn into_state(mut self) -> Result<crate::phy_state::PhyState, Self> {
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
            TailStep::Temperature | TailStep::BackupCalibration | TailStep::MarkRegistered => None,
        }
    }

    pub fn step_local(&mut self) -> Result<PhyRegisterLocalStep, PhyRegisterTransitionError> {
        let phase = self
            .phase
            .take()
            .ok_or(PhyRegisterTransitionError::AlreadyComplete)?;
        match phase {
            Phase::Prelude(PreludeStep::ApplyProfile) => {
                let config = self
                    .config
                    .take()
                    .ok_or(PhyRegisterTransitionError::MissingStateOwner)?;
                let state = self
                    .state
                    .as_mut()
                    .ok_or(PhyRegisterTransitionError::MissingStateOwner)?;
                // A retained snapshot owns calibrated values, but the current
                // driver does not yet own the complete hardware replay which
                // republishes every skipped RF/baseband register after reset.
                // HIL proved that restoring software flags alone produces an
                // unstable link. Until that replay is a typed transition, a
                // supplied cache is untrusted for cold admission and is
                // replaced by a complete calibration.
                if self.calibration_candidate {
                    self.calibration_path = PhyCalibrationPath::FullAfterRejectedCache;
                }
                self.calibration_cache_ready = false;
                state.begin_full_wifi_calibration(config);
                // Pinned parent saves this flag word before either child can
                // mutate it and uses the snapshot after both return.
                self.temperature_control = Some(state.register_temperature_control());
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
                        PhyRegisterAction::SampleI2cMasterReset { index, sample: 0 }
                    }
                    PreludeStep::I2cResetSample { index, sample } => {
                        PhyRegisterAction::SampleI2cMasterReset { index, sample }
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
                            crate::phy_bb::PhyBbInitTransition::new_on_channel(
                                state,
                                self.channel_or_frequency,
                            ),
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
                    let control = self
                        .temperature_control
                        .ok_or(PhyRegisterTransitionError::MissingStateOwner)?;
                    self.state
                        .as_mut()
                        .ok_or(PhyRegisterTransitionError::MissingStateOwner)?
                        .apply_register_temperature_outcome(control, outcome);
                    // Cache replay is intentionally not exposed until the
                    // driver owns restoration of every retained RF/baseband
                    // hardware value. Every supported path therefore backs up
                    // the fresh full-calibration result.
                    self.phase = Some(Phase::Tail(TailStep::BackupCalibration));
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
            Phase::Tail(TailStep::BackupCalibration) => {
                if let Some(identity) = self.calibration_identity {
                    let state = self
                        .state
                        .as_ref()
                        .ok_or(PhyRegisterTransitionError::MissingStateOwner)?;
                    self.calibration_cache = Some(state.calibration_cache(identity));
                    self.calibration_cache_ready = true;
                }
                self.phase = Some(Phase::Tail(TailStep::BbpllOff));
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
                step:
                    TailStep::Temperature
                    | TailStep::BackupCalibration
                    | TailStep::FinalI2cRead
                    | TailStep::MarkRegistered,
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
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: true,
                            completed_phase: 0,
                        },
                    micros: 1,
                },
            ) => Phase::Prelude(PreludeStep::ForceSecond),
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
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: true,
                            completed_phase: 1,
                        },
                    micros: 1,
                },
            ) => Phase::Prelude(PreludeStep::FrequencyReset),
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
                    sample: 0,
                    busy,
                },
            ) if completed_index == index => {
                if busy {
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
                    sample: completed_sample,
                    busy,
                },
            ) if completed_index == index && completed_sample == sample => {
                if !busy {
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
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: false,
                            completed_phase: 0,
                        },
                    micros: 1,
                },
            ) => Phase::Tail(TailStep::ReleaseSecond),
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
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: false,
                            completed_phase: 1,
                        },
                    micros: 1,
                },
            ) => Phase::Complete(PhyRegisterOutcome {
                full_calibration_performed: true,
                calibration_path: self.calibration_path,
            }),
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
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: false,
                            completed_phase: 0,
                        },
                    micros: 1,
                },
            ) => Phase::Cleanup {
                step: TailStep::ReleaseSecond,
                failure,
            },
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
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: false,
                            completed_phase: 1,
                        },
                    micros: 1,
                },
            ) => Phase::Failed(failure),
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
    pub fn execute_target<
        P: open_esp_radio_esp32s31_hal::phy_prelude::PhyPreludePlatformControl
            + open_esp_radio_esp32s31_hal::wifi_bb::PhyWifiBbControl
            + open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl,
    >(
        self,
        radio: &mut open_esp_radio_esp32s31_hal::Radio<
            P,
            open_esp_radio_esp32s31_hal::state::Powered,
        >,
    ) -> PhyRegisterMmioCompletion {
        match self.action {
            PhyRegisterMmioAction::PrepareColdStart => {
                let (platform, registers) = radio.phy_hal_parts();
                open_esp_radio_esp32s31_hal::phy_frequency::prepare_wifi_control(
                    platform, registers,
                )
            }
            PhyRegisterMmioAction::ConfigureForceTxRx { enabled, phase } => {
                open_esp_radio_esp32s31_hal::pbus::configure_force_txrx(
                    radio.phy_hal_mut(),
                    enabled,
                    phase,
                )
            }
            PhyRegisterMmioAction::ResetFrequencyModule => {
                open_esp_radio_esp32s31_hal::phy_frequency::reset_module(radio.phy_hal_mut())
            }
            PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled } => {
                open_esp_radio_esp32s31_hal::phy_frequency::set_hardware_control(
                    radio.phy_hal_mut(),
                    enabled,
                )
            }
            PhyRegisterMmioAction::PulseI2cMasterReset { index } => {
                let host = if index == 0 {
                    open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cHost::Host0
                } else {
                    open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cHost::Host1
                };
                let (platform, _) = radio.phy_hal_parts();
                open_esp_radio_esp32s31_hal::phy_i2c::pulse_master_reset(platform, host)
            }
            PhyRegisterMmioAction::ConfigureXtal40Mhz => {
                let (platform, _) = radio.phy_hal_parts();
                open_esp_radio_esp32s31_hal::phy_prelude::configure_fixed_xtal_40mhz(platform)
            }
            PhyRegisterMmioAction::SetCalibrationClock { enabled } => {
                crate::phy_hardware::set_phy_register_calibration_clock(
                    radio.phy_hal_mut(),
                    enabled,
                )
            }
            PhyRegisterMmioAction::SetBbpllCalibration { enabled } => {
                let (platform, _) = radio.phy_hal_parts();
                open_esp_radio_esp32s31_hal::phy_i2c::configure_bbpll_calibration(platform, enabled)
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
    sample: u16,
}

impl PhyRegisterResetSampleBinding {
    pub fn new(action: PhyRegisterAction) -> Option<Self> {
        match action {
            PhyRegisterAction::SampleI2cMasterReset { index, sample } if index <= 1 => {
                Some(Self { index, sample })
            }
            _ => None,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P: open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cMasterControl>(
        self,
        radio: &mut open_esp_radio_esp32s31_hal::Radio<
            P,
            open_esp_radio_esp32s31_hal::state::Powered,
        >,
    ) -> PhyRegisterCompletion {
        let host = if self.index == 0 {
            open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cHost::Host0
        } else {
            open_esp_radio_esp32s31_hal::phy_i2c::PhyI2cHost::Host1
        };
        let (platform, _) = radio.phy_hal_parts();
        PhyRegisterCompletion::I2cMasterResetSampled {
            index: self.index,
            sample: self.sample,
            busy: open_esp_radio_esp32s31_hal::phy_i2c::sample_master_reset_busy(platform, host),
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
        PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT, PhyCalibrationIdentity, PhyCalibrationPath,
        PhyRegisterAction, PhyRegisterBindingError, PhyRegisterCompletion, PhyRegisterDelayPhase,
        PhyRegisterFailure, PhyRegisterFinalI2cBinding, PhyRegisterLocalStep,
        PhyRegisterMmioAction, PhyRegisterMmioCompletion, PhyRegisterOutcome,
        PhyRegisterResetSampleBinding, PhyRegisterTimerBinding, PhyRegisterTransition,
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
    fn production_config_contains_only_the_qualified_tx_power_policy() {
        let state = crate::phy_state::PhyState::new(crate::phy_state::PhyConfig::production());
        let profile = state.tx_target_power_profile();
        assert_eq!(
            profile,
            crate::phy_tx_power::PhyTxTargetPowerProfile::new(
                0x54,
                [
                    0x50, 0x50, 0x50, 0x50, 0x4c, 0x48, 0x50, 0x50, 0x4c, 0x48, 0x40, 0x3c, 0x3c,
                    0x3c, 0x4c, 0x4c, 0x48, 0x44,
                ],
                false,
            )
        );
    }

    #[test]
    fn state_owner_cannot_escape_before_a_terminal_parent_outcome() {
        let transition = PhyRegisterTransition::with_production_config();
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
        let mut transition = PhyRegisterTransition::with_production_config();
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
                sample: 0,
            })
        );
        assert!(
            PhyRegisterResetSampleBinding::new(match action {
                PhyRegisterLocalStep::External(action) => action,
                _ => unreachable!(),
            })
            .is_some()
        );
    }

    #[test]
    fn stuck_i2c_reset_fails_only_after_bounded_async_samples_and_cleans_up() {
        let mut transition = PhyRegisterTransition::with_production_config();
        transition.phase = Some(super::Phase::Prelude(super::PreludeStep::I2cResetSample {
            index: 0,
            sample: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT - 1,
        }));
        transition
            .advance_external(PhyRegisterCompletion::I2cMasterResetSampled {
                index: 0,
                sample: PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT - 1,
                busy: true,
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
        let mut transition = PhyRegisterTransition::with_production_config();
        transition.config = None;
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
                calibration_path: PhyCalibrationPath::FullUncached,
            })
        );
        let state = match transition.into_state() {
            Ok(state) => state,
            Err(_) => panic!("a completed cold initializer must release its state owner"),
        };
        assert!(state.phy_registered());
    }

    const CALIBRATION_IDENTITY: PhyCalibrationIdentity = PhyCalibrationIdentity {
        rf_cal_version: 0x1234_5678,
        mac_sys0: 0xa1b2_c3d4,
        mac_sys1: 0xe5f6_0718,
    };

    fn retained_cache(identity: PhyCalibrationIdentity) -> crate::phy_state::PhyCalibrationCache {
        let mut state = crate::phy_state::PhyState::new(crate::phy_state::PhyConfig::production());
        state.mark_baseband_calibration_complete();
        state.apply_tx_power_outcome(crate::phy_tx_power::PhyTxPowerOutcome {
            reference_codes: [80, 120],
            power_curve: [-3, 4, 5],
            point_corrections: [6, -7, 8],
            power_adjustment: -9,
            final_attenuation: 13,
            current_channel: 11,
            calibration_performed: true,
        });
        state.calibration_cache(identity)
    }

    #[test]
    fn structurally_valid_cache_is_replaced_until_hardware_replay_is_owned() {
        let cache = retained_cache(CALIBRATION_IDENTITY);
        let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
            CALIBRATION_IDENTITY,
            Some(cache),
        );
        transition.phase = Some(super::Phase::Prelude(super::PreludeStep::ApplyProfile));

        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::StateAdvanced
        );
        assert_eq!(
            transition.calibration_path,
            PhyCalibrationPath::FullAfterRejectedCache
        );
        assert!(transition.calibration_cache().is_none());
        assert!(!transition.state().unwrap().baseband_calibration_complete());
        assert!(
            !transition
                .state()
                .unwrap()
                .tx_power_parameters()
                .already_calibrated
        );
        let temperature = transition.temperature_control.unwrap();
        assert!(temperature.updates_offset_130());
        assert!(temperature.updates_reference_copies());
    }

    #[test]
    fn rejected_caller_cache_falls_back_to_full_calibration() {
        let cache = retained_cache(PhyCalibrationIdentity {
            rf_cal_version: CALIBRATION_IDENTITY.rf_cal_version + 1,
            ..CALIBRATION_IDENTITY
        });
        let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
            CALIBRATION_IDENTITY,
            Some(cache),
        );
        transition.phase = Some(super::Phase::Prelude(super::PreludeStep::ApplyProfile));

        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::StateAdvanced
        );
        assert_eq!(
            transition.calibration_path,
            PhyCalibrationPath::FullAfterRejectedCache
        );
        assert!(transition.calibration_cache().is_none());
        assert!(!transition.state().unwrap().baseband_calibration_complete());
        assert!(
            !transition
                .state()
                .unwrap()
                .tx_power_parameters()
                .already_calibrated
        );
        let temperature = transition.temperature_control.unwrap();
        assert!(temperature.updates_offset_130());
        assert!(temperature.updates_reference_copies());
    }

    #[test]
    fn completed_full_path_exposes_a_caller_persistable_cache() {
        let mut transition = PhyRegisterTransition::with_production_config_and_calibration(
            CALIBRATION_IDENTITY,
            None,
        );
        transition.phase = Some(super::Phase::Tail(super::TailStep::BackupCalibration));

        assert_eq!(
            transition.step_local().unwrap(),
            PhyRegisterLocalStep::StateAdvanced
        );
        let cache = transition.take_calibration_cache().unwrap();
        assert!(cache.matches(CALIBRATION_IDENTITY));
    }
}
