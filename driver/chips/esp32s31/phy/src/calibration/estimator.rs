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

/// Split one packed DC value exactly as pinned archive `get_dc_value`.
#[inline]
pub fn get_dc_value(output: &mut [u16; 2], value: u32) {
    output[0] = (value >> 16) as u16;
    output[1] = value as u16;
}

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
    /// PAC `ESTIMATOR_READY_STATUS.READY`.
    pub ready: bool,
    /// Whether PAC `ESTIMATOR_ACTIVITY_STATUS.ACTIVITY_UNKNOWN` is nonzero.
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

#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_target(
    registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    control: u16,
) {
    open_esp_radio_esp32s31_hal::phy_iq_estimator::configure(registers, control);
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn set_enable_target(
    registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    phase: PhyDcIqEnablePhase,
    enabled: bool,
) {
    match phase {
        PhyDcIqEnablePhase::Start => {
            open_esp_radio_esp32s31_hal::phy_iq_estimator::set_start_enabled(registers, enabled);
        }
        PhyDcIqEnablePhase::Measurement => {
            open_esp_radio_esp32s31_hal::phy_iq_estimator::set_measurement_enabled(
                registers, enabled,
            );
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn sample_readiness_target(
    registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
) -> PhyDcIqReadinessSnapshot {
    let snapshot = open_esp_radio_esp32s31_hal::phy_iq_estimator::sample_readiness(registers);
    PhyDcIqReadinessSnapshot {
        ready: snapshot.ready,
        activity: snapshot.activity,
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn read_accumulators_target(
    registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
) -> PhyDcIqAccumulatorSnapshot {
    let snapshot =
        open_esp_radio_esp32s31_hal::phy_iq_estimator::read_dc_iq_accumulators(registers);
    PhyDcIqAccumulatorSnapshot {
        i: snapshot.i,
        q: snapshot.q,
        power: snapshot.power,
    }
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
        readiness_samples: u16,
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
    readiness_samples: u16,
    step: PhyDcIqStep,
}

impl PhyDcIqEstimateTransition {
    pub const fn new(request: PhyDcIqEstimateRequest) -> Self {
        Self {
            request,
            readiness_activity_edges: 0,
            readiness_samples: 0,
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
                readiness_samples: self.readiness_samples,
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
            ) if request == self.request => {
                self.readiness_samples = self.readiness_samples.saturating_add(1);
                PhyDcIqStep::ReadAccumulators
            }
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
                self.readiness_samples = self.readiness_samples.saturating_add(1);
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
            | PhyDcIqAction::ReadAccumulators(_) => Ok(Self { action }),
            _ => Err(PhyDcIqBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> PhyDcIqAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> PhyDcIqCompletion {
        match self.action {
            PhyDcIqAction::Configure(request) => {
                configure_target(registers, request.control);
                PhyDcIqCompletion::Configured(request)
            }
            PhyDcIqAction::SetEnable {
                request,
                phase,
                enabled,
            } => {
                set_enable_target(registers, phase, enabled);
                PhyDcIqCompletion::EnableSet {
                    request,
                    phase,
                    enabled,
                }
            }
            PhyDcIqAction::ReadAccumulators(request) => PhyDcIqCompletion::AccumulatorsRead {
                request,
                snapshot: read_accumulators_target(registers),
            },
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable async boundary for one scheduled readiness observation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyDcIqReadinessBinding {
    action: PhyDcIqAction,
}

impl PhyDcIqReadinessBinding {
    pub fn new(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        match action {
            PhyDcIqAction::AwaitReadinessEdge { .. } => Ok(Self { action }),
            _ => Err(PhyDcIqBindingError::UnsupportedAction),
        }
    }

    pub const fn samples(&self) -> u16 {
        match self.action {
            PhyDcIqAction::AwaitReadinessEdge {
                readiness_samples, ..
            } => readiness_samples,
            _ => unreachable!(),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> PhyDcIqCompletion {
        let PhyDcIqAction::AwaitReadinessEdge { request, .. } = self.action else {
            unreachable!();
        };
        PhyDcIqCompletion::ReadinessObserved {
            request,
            snapshot: sample_readiness_target(registers),
        }
    }

    pub fn into_timeout_completion(self) -> PhyDcIqCompletion {
        let PhyDcIqAction::AwaitReadinessEdge { request, .. } = self.action else {
            unreachable!();
        };
        PhyDcIqCompletion::ReadinessTimedOut(request)
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
    Readiness(PhyDcIqReadinessBinding),
}

impl PhyDcIqExternalBinding {
    pub fn lower(action: PhyDcIqAction) -> Result<Self, PhyDcIqBindingError> {
        if let Ok(binding) = PhyDcIqReadinessBinding::new(action) {
            return Ok(Self::Readiness(binding));
        }
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
mod tests;
