//! Event-driven ESP32-S31 DC/IQ estimator.
//!
//! Primary references in `esp32s31_rev0_rom.elf`:
//!
//! - `phy_iq_est_enable` at `0x2f82_89d4`, size `0xb4`;
//! - `phy_iq_est_disable` at `0x2f82_8a88`, size `0x2c`;
//! - `phy_dc_iq_est` at `0x2f82_8ab4`, size `0x84`;
//! - `phy_linear_to_db` at `0x2f82_6542`, size `0x7c`.
//!
//! ROM spins on a hardware-ready bit and implements both one-microsecond
//! intervals with synchronous `ets_delay_us`. This module retains the exact
//! register ordering and arithmetic as caller-driven actions. It can advance
//! only from externally delivered readiness/timer completions.

const LINEAR_TO_DB_FRACTION: [u8; 16] =
    [0, 4, 8, 12, 16, 19, 22, 25, 28, 31, 34, 36, 39, 41, 44, 46];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqEstimateRequest {
    /// Parent-owned identity. ROM does not receive this field.
    pub iteration: u8,
    pub chain: u8,
    pub control: u16,
    pub mode: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqAccumulatorSnapshot {
    pub i: i32,
    pub q: i32,
    pub power: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqReadinessSnapshot {
    /// Bit 16 of `0x2010_047c`.
    pub ready: bool,
    /// Whether bits 21:20 of `0x2010_08d0` are nonzero.
    pub activity: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqEstimate {
    pub i: i32,
    pub q: i32,
    pub power: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqEstimateOutcome {
    pub request: PhyDcIqEstimateRequest,
    pub estimate: PhyDcIqEstimate,
    /// Rust-owned replacement for the diagnostic halfword that ROM mutates
    /// through `phy_param_rom + 0x1ac`.
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqFailure {
    ReadinessTimedOut {
        request: PhyDcIqEstimateRequest,
        readiness_activity_edges: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqEnablePhase {
    Start,
    Measurement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqDelayPhase {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqAction {
    Configure(PhyDcIqEstimateRequest),
    SetEnable {
        request: PhyDcIqEstimateRequest,
        phase: PhyDcIqEnablePhase,
        enabled: bool,
    },
    DelayMicros {
        request: PhyDcIqEstimateRequest,
        phase: PhyDcIqDelayPhase,
        micros: u32,
    },
    AwaitReadinessEdge {
        request: PhyDcIqEstimateRequest,
        readiness_activity_edges: u16,
    },
    ReadAccumulators(PhyDcIqEstimateRequest),
    Complete(PhyDcIqEstimateOutcome),
    Failed(PhyDcIqFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqCompletion {
    Configured(PhyDcIqEstimateRequest),
    EnableSet {
        request: PhyDcIqEstimateRequest,
        phase: PhyDcIqEnablePhase,
        enabled: bool,
    },
    DelayElapsed {
        request: PhyDcIqEstimateRequest,
        phase: PhyDcIqDelayPhase,
        micros: u32,
    },
    ReadinessObserved {
        request: PhyDcIqEstimateRequest,
        snapshot: PhyDcIqReadinessSnapshot,
    },
    ReadinessTimedOut(PhyDcIqEstimateRequest),
    AccumulatorsRead {
        request: PhyDcIqEstimateRequest,
        snapshot: PhyDcIqAccumulatorSnapshot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyDcIqTerminal {
    Complete(PhyDcIqEstimateOutcome),
    Failed(PhyDcIqFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyDcIqStep {
    Configure,
    EnableStart,
    StartDelay,
    EnableMeasurement,
    AwaitReadiness,
    ReadAccumulators,
    DisableMeasurement(PhyDcIqTerminal),
    StopDelay(PhyDcIqTerminal),
    DisableStart(PhyDcIqTerminal),
    Complete(PhyDcIqEstimateOutcome),
    Failed(PhyDcIqFailure),
}

/// Exact stateless translation of rev0 ROM `phy_linear_to_db`.
///
/// The lookup table is the sixteen-byte image at ROM address `0x2f84_832c`.
/// Shift counts retain RISC-V's low-five-bit behavior.
pub fn phy_linear_to_db(value: i32, scale: u8) -> i32 {
    let scaled = if scale <= 2 {
        value.wrapping_shl(u32::from(3 - scale))
    } else {
        value >> (u32::from(scale - 3) & 0x1f)
    };
    let exponent = (28_i32.wrapping_sub((scaled as u32).leading_zeros() as i32)) as i8 as i32;
    let (exponent, fraction) = if exponent > 0 {
        (
            exponent,
            ((scaled >> ((exponent - 1) as u32)) & 0x0f) as usize,
        )
    } else {
        (0, (scaled & 0x0f) as usize)
    };
    let result = (exponent as u16)
        .wrapping_mul(48)
        .wrapping_add(u16::from(LINEAR_TO_DB_FRACTION[fraction]));
    result as i16 as i32
}

/// Convert the three exact accumulator words read after the ready edge.
pub fn calculate_dc_iq_estimate(
    request: PhyDcIqEstimateRequest,
    snapshot: PhyDcIqAccumulatorSnapshot,
) -> PhyDcIqEstimate {
    let shift = if request.mode == 0 { 6 } else { 4 };
    let divisor = i32::from(request.control) + 1;
    let i = (snapshot.i >> shift) / divisor;
    let q = (snapshot.q >> shift) / divisor;
    let squares = i.wrapping_mul(i).wrapping_add(q.wrapping_mul(q));
    let squares = if request.mode == 0 {
        squares
    } else {
        squares >> 4
    };
    let linear = (snapshot.power / divisor)
        .wrapping_shl(3)
        .wrapping_sub(squares);
    let linear = if linear < 0 { 0 } else { linear };
    let power = phy_linear_to_db(linear, 0).wrapping_add(8) >> 4;
    PhyDcIqEstimate { i, q, power }
}

/// Heap-free, externally driven replacement for the complete DC/IQ estimator.
///
/// A false readiness observation represents an independently delivered
/// hardware edge; it never schedules or performs another observation. The
/// owner can instead deliver `ReadinessTimedOut`, after which the transition
/// executes the same disable/one-microsecond/disable tail before returning a
/// typed failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDcIqEstimateTransition {
    request: PhyDcIqEstimateRequest,
    readiness_activity_edges: u16,
    step: PhyDcIqStep,
}

impl PhyDcIqEstimateTransition {
    pub const fn new(request: PhyDcIqEstimateRequest) -> Self {
        Self {
            request,
            readiness_activity_edges: 0,
            step: PhyDcIqStep::Configure,
        }
    }

    pub const fn action(self) -> PhyDcIqAction {
        match self.step {
            PhyDcIqStep::Configure => PhyDcIqAction::Configure(self.request),
            PhyDcIqStep::EnableStart => PhyDcIqAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Start,
                enabled: true,
            },
            PhyDcIqStep::StartDelay => PhyDcIqAction::DelayMicros {
                request: self.request,
                phase: PhyDcIqDelayPhase::Start,
                micros: 1,
            },
            PhyDcIqStep::EnableMeasurement => PhyDcIqAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: true,
            },
            PhyDcIqStep::AwaitReadiness => PhyDcIqAction::AwaitReadinessEdge {
                request: self.request,
                readiness_activity_edges: self.readiness_activity_edges,
            },
            PhyDcIqStep::ReadAccumulators => PhyDcIqAction::ReadAccumulators(self.request),
            PhyDcIqStep::DisableMeasurement(_) => PhyDcIqAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: false,
            },
            PhyDcIqStep::StopDelay(_) => PhyDcIqAction::DelayMicros {
                request: self.request,
                phase: PhyDcIqDelayPhase::Stop,
                micros: 1,
            },
            PhyDcIqStep::DisableStart(_) => PhyDcIqAction::SetEnable {
                request: self.request,
                phase: PhyDcIqEnablePhase::Start,
                enabled: false,
            },
            PhyDcIqStep::Complete(outcome) => PhyDcIqAction::Complete(outcome),
            PhyDcIqStep::Failed(failure) => PhyDcIqAction::Failed(failure),
        }
    }

    pub fn advance(&mut self, completion: PhyDcIqCompletion) -> Result<(), PhyDcIqTransitionError> {
        self.step = match (self.step, completion) {
            (PhyDcIqStep::Configure, PhyDcIqCompletion::Configured(request))
                if request == self.request =>
            {
                PhyDcIqStep::EnableStart
            }
            (
                PhyDcIqStep::EnableStart,
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: true,
                },
            ) if request == self.request => PhyDcIqStep::StartDelay,
            (
                PhyDcIqStep::StartDelay,
                PhyDcIqCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Start,
                    micros: 1,
                },
            ) if request == self.request => PhyDcIqStep::EnableMeasurement,
            (
                PhyDcIqStep::EnableMeasurement,
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: true,
                },
            ) if request == self.request => PhyDcIqStep::AwaitReadiness,
            (
                PhyDcIqStep::AwaitReadiness,
                PhyDcIqCompletion::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot { ready: true, .. },
                },
            ) if request == self.request => PhyDcIqStep::ReadAccumulators,
            (
                PhyDcIqStep::AwaitReadiness,
                PhyDcIqCompletion::ReadinessObserved {
                    request,
                    snapshot:
                        PhyDcIqReadinessSnapshot {
                            ready: false,
                            activity,
                        },
                },
            ) if request == self.request => {
                if activity {
                    self.readiness_activity_edges = self.readiness_activity_edges.wrapping_add(1);
                }
                PhyDcIqStep::AwaitReadiness
            }
            (PhyDcIqStep::AwaitReadiness, PhyDcIqCompletion::ReadinessTimedOut(request))
                if request == self.request =>
            {
                PhyDcIqStep::DisableMeasurement(PhyDcIqTerminal::Failed(
                    PhyDcIqFailure::ReadinessTimedOut {
                        request,
                        readiness_activity_edges: self.readiness_activity_edges,
                    },
                ))
            }
            (
                PhyDcIqStep::ReadAccumulators,
                PhyDcIqCompletion::AccumulatorsRead { request, snapshot },
            ) if request == self.request => {
                PhyDcIqStep::DisableMeasurement(PhyDcIqTerminal::Complete(PhyDcIqEstimateOutcome {
                    request,
                    estimate: calculate_dc_iq_estimate(request, snapshot),
                    readiness_activity_edges: self.readiness_activity_edges,
                }))
            }
            (
                PhyDcIqStep::DisableMeasurement(terminal),
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: false,
                },
            ) if request == self.request => PhyDcIqStep::StopDelay(terminal),
            (
                PhyDcIqStep::StopDelay(terminal),
                PhyDcIqCompletion::DelayElapsed {
                    request,
                    phase: PhyDcIqDelayPhase::Stop,
                    micros: 1,
                },
            ) if request == self.request => PhyDcIqStep::DisableStart(terminal),
            (
                PhyDcIqStep::DisableStart(terminal),
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase: PhyDcIqEnablePhase::Start,
                    enabled: false,
                },
            ) if request == self.request => match terminal {
                PhyDcIqTerminal::Complete(outcome) => PhyDcIqStep::Complete(outcome),
                PhyDcIqTerminal::Failed(failure) => PhyDcIqStep::Failed(failure),
            },
            (PhyDcIqStep::Complete(_), _) | (PhyDcIqStep::Failed(_), _) => {
                return Err(PhyDcIqTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyDcIqTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDcIqBindingError {
    UnsupportedAction,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyDcIqMmioBinding {
    action: PhyDcIqAction,
}

impl PhyDcIqMmioBinding {
    pub fn new(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        match action {
            PhyDcIqAction::Configure(_)
            | PhyDcIqAction::SetEnable { .. }
            | PhyDcIqAction::AwaitReadinessEdge { .. }
            | PhyDcIqAction::ReadAccumulators(_) => Ok(Self { action }),
            _ => Err(PhyDcIqBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> PhyDcIqAction {
        self.action
    }

    pub fn into_timeout_completion(self) -> Result<PhyDcIqCompletion, PhyDcIqBindingError> {
        match self.action {
            PhyDcIqAction::AwaitReadinessEdge { request, .. } => {
                Ok(PhyDcIqCompletion::ReadinessTimedOut(request))
            }
            _ => Err(PhyDcIqBindingError::UnsupportedAction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(self) -> PhyDcIqCompletion {
        match self.action {
            PhyDcIqAction::Configure(request) => {
                crate::radio_hal::configure_phy_dc_iq_estimator(request.control);
                PhyDcIqCompletion::Configured(request)
            }
            PhyDcIqAction::SetEnable {
                request,
                phase,
                enabled,
            } => {
                crate::radio_hal::set_phy_dc_iq_estimator_enable(phase, enabled);
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase,
                    enabled,
                }
            }
            PhyDcIqAction::AwaitReadinessEdge { request, .. } => {
                PhyDcIqCompletion::ReadinessObserved {
                    request,
                    snapshot: crate::radio_hal::sample_phy_dc_iq_readiness(),
                }
            }
            PhyDcIqAction::ReadAccumulators(request) => PhyDcIqCompletion::AccumulatorsRead {
                request,
                snapshot: crate::radio_hal::read_phy_dc_iq_accumulators(),
            },
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyDcIqTimerBinding {
    request: PhyDcIqEstimateRequest,
    phase: PhyDcIqDelayPhase,
    micros: u32,
}

impl PhyDcIqTimerBinding {
    pub fn new(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        match action {
            PhyDcIqAction::DelayMicros {
                request,
                phase,
                micros,
            } => Ok(Self {
                request,
                phase,
                micros,
            }),
            _ => Err(PhyDcIqBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyDcIqCompletion {
        PhyDcIqCompletion::DelayElapsed {
            request: self.request,
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyDcIqExternalBinding {
    Mmio(PhyDcIqMmioBinding),
    Timer(PhyDcIqTimerBinding),
}

impl PhyDcIqExternalBinding {
    pub fn lower(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        if let Ok(binding) = PhyDcIqMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyDcIqTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyDcIqBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: PhyDcIqEstimateRequest = PhyDcIqEstimateRequest {
        iteration: 3,
        chain: 1,
        control: 0x0fa0,
        mode: 0,
    };

    fn reach_readiness(transition: &mut PhyDcIqEstimateTransition) {
        transition
            .advance(PhyDcIqCompletion::Configured(REQUEST))
            .unwrap();
        transition
            .advance(PhyDcIqCompletion::EnableSet {
                request: REQUEST,
                phase: PhyDcIqEnablePhase::Start,
                enabled: true,
            })
            .unwrap();
        transition
            .advance(PhyDcIqCompletion::DelayElapsed {
                request: REQUEST,
                phase: PhyDcIqDelayPhase::Start,
                micros: 1,
            })
            .unwrap();
        transition
            .advance(PhyDcIqCompletion::EnableSet {
                request: REQUEST,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: true,
            })
            .unwrap();
    }

    fn finish_disable_tail(transition: &mut PhyDcIqEstimateTransition) {
        transition
            .advance(PhyDcIqCompletion::EnableSet {
                request: REQUEST,
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: false,
            })
            .unwrap();
        transition
            .advance(PhyDcIqCompletion::DelayElapsed {
                request: REQUEST,
                phase: PhyDcIqDelayPhase::Stop,
                micros: 1,
            })
            .unwrap();
        transition
            .advance(PhyDcIqCompletion::EnableSet {
                request: REQUEST,
                phase: PhyDcIqEnablePhase::Start,
                enabled: false,
            })
            .unwrap();
    }

    #[test]
    fn linear_to_db_matches_rom_table_boundaries() {
        assert_eq!(phy_linear_to_db(0, 0), 0);
        assert_eq!(phy_linear_to_db(1, 0), 28);
        assert_eq!(phy_linear_to_db(2, 0), 48);
        assert_eq!(phy_linear_to_db(3, 0), 76);
        assert_eq!(phy_linear_to_db(1, 3), 4);
    }

    #[test]
    fn accumulator_transform_matches_reachable_mode_zero_equations() {
        assert_eq!(
            calculate_dc_iq_estimate(
                REQUEST,
                PhyDcIqAccumulatorSnapshot {
                    i: 4001 * 64 * 2,
                    q: -(4001 * 64 * 3),
                    power: 4001 * 32,
                },
            ),
            PhyDcIqEstimate {
                i: 2,
                q: -3,
                power: 24,
            }
        );
    }

    #[test]
    fn readiness_requires_external_edges_and_owns_diagnostic_count() {
        let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
        reach_readiness(&mut transition);
        for activity in [false, true, true] {
            transition
                .advance(PhyDcIqCompletion::ReadinessObserved {
                    request: REQUEST,
                    snapshot: PhyDcIqReadinessSnapshot {
                        ready: false,
                        activity,
                    },
                })
                .unwrap();
        }
        assert_eq!(
            transition.action(),
            PhyDcIqAction::AwaitReadinessEdge {
                request: REQUEST,
                readiness_activity_edges: 2,
            }
        );
        transition
            .advance(PhyDcIqCompletion::ReadinessObserved {
                request: REQUEST,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: true,
                    activity: true,
                },
            })
            .unwrap();
        transition
            .advance(PhyDcIqCompletion::AccumulatorsRead {
                request: REQUEST,
                snapshot: PhyDcIqAccumulatorSnapshot {
                    i: 0,
                    q: 0,
                    power: 0,
                },
            })
            .unwrap();
        finish_disable_tail(&mut transition);
        let PhyDcIqAction::Complete(outcome) = transition.action() else {
            panic!("DC/IQ transition did not complete");
        };
        assert_eq!(outcome.readiness_activity_edges, 2);
        assert_eq!(
            outcome.estimate,
            PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            }
        );
    }

    #[test]
    fn timeout_runs_complete_disable_tail_before_failure() {
        let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
        reach_readiness(&mut transition);
        transition
            .advance(PhyDcIqCompletion::ReadinessTimedOut(REQUEST))
            .unwrap();
        assert!(matches!(
            transition.action(),
            PhyDcIqAction::SetEnable {
                phase: PhyDcIqEnablePhase::Measurement,
                enabled: false,
                ..
            }
        ));
        finish_disable_tail(&mut transition);
        assert_eq!(
            transition.action(),
            PhyDcIqAction::Failed(PhyDcIqFailure::ReadinessTimedOut {
                request: REQUEST,
                readiness_activity_edges: 0,
            })
        );
    }

    #[test]
    fn external_lowering_covers_mmio_timer_and_rejects_terminal() {
        assert!(matches!(
            PhyDcIqExternalBinding::lower(PhyDcIqAction::Configure(REQUEST)),
            Ok(PhyDcIqExternalBinding::Mmio(_))
        ));
        assert!(matches!(
            PhyDcIqExternalBinding::lower(PhyDcIqAction::DelayMicros {
                request: REQUEST,
                phase: PhyDcIqDelayPhase::Start,
                micros: 1,
            }),
            Ok(PhyDcIqExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyDcIqExternalBinding::lower(PhyDcIqAction::Failed(
                PhyDcIqFailure::ReadinessTimedOut {
                    request: REQUEST,
                    readiness_activity_edges: 0,
                }
            )),
            Err(PhyDcIqBindingError::UnsupportedAction)
        ));
    }
}
