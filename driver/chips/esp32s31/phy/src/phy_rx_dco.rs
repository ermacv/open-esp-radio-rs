//! Event-driven RX DC-offset calibration for ESP32-S31 rev0.
//!
//! Primary reference: `esp32s31_rev0_rom.elf::phy_pbus_rx_dco_cal` at
//! `0x2f82_8f44`, size `0x228`. The crystal-duty caller uses the fixed
//! argument tuple `(0x0fa0, configuration, 10, 0, 0)`. This module owns the
//! complete reachable twelve-iteration control loop. Its `phy_dc_iq_est`
//! child is another Rust-owned transition, so the ROM hardware-ready spin and
//! synchronous delays are absent from the complete reachable graph.

use crate::phy_dc_iq::{
    PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateOutcome, PhyDcIqEstimateTransition,
    PhyDcIqFailure,
};
pub use crate::phy_dc_iq::{PhyDcIqEstimate, PhyDcIqEstimateRequest};
use crate::phy_pbus::PhyPbusForceTest;

const MAX_ITERATIONS: u8 = 12;
const RX_DC_MINIMUM_MAX_ATTEMPTS: u8 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcoRequest {
    /// Measurement control passed as the second argument to
    /// `phy_dc_iq_est(1, control, ...)`.
    pub control: u16,
    /// Two words initialized by the caller. ROM treats the low and high
    /// halfwords of word zero as signed I/Q compensation values. Word one is
    /// retained byte-for-byte even though the reachable ROM body does not
    /// access it.
    pub configuration: [u32; 2],
    /// Timer interval before each IQ measurement.
    pub delay_micros: u32,
}

impl PhyRxDcoRequest {
    pub const XTAL_DUTY: Self = Self {
        control: 0x0fa0,
        configuration: [0x0100_0100; 2],
        delay_micros: 10,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcoOutcome {
    pub configuration: [u32; 2],
    /// Number of completed IQ measurements, in `1..=12`.
    pub iterations: u8,
    /// False means ROM's bounded iteration limit was reached. It is still a
    /// normal vendor-compatible outcome, not an implicit retry request.
    pub converged: bool,
    pub last_estimate: PhyDcIqEstimate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoFailure {
    PbusForceTimedOut(PhyPbusForceTest),
    DcIq(PhyDcIqFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoAction {
    PrepareRxDcoControlRestore,
    ReadPbus { selector: u8, path: u8 },
    ForcePbus(PhyPbusForceTest),
    DelayMicros { iteration: u8, micros: u32 },
    DcIq(PhyDcIqAction),
    RestoreRxDcoControl,
    Complete(PhyRxDcoOutcome),
    Failed(PhyRxDcoFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoCompletion {
    RxDcoControlRestorePrepared,
    PbusRead { selector: u8, path: u8, value: u32 },
    PbusForceCompleted(PhyPbusForceTest),
    PbusForceTimedOut(PhyPbusForceTest),
    DelayElapsed { iteration: u8, micros: u32 },
    DcIq(PhyDcIqCompletion),
    RxDcoControlRestored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcMinimumRequest {
    /// Parent-owned identity. The ROM function has no corresponding field.
    pub measurement: u8,
    pub control: u16,
    pub mode: u8,
    /// Rust-owned replacement for `phy_param[0x1ae] == 1`.
    pub rx_saturation_detected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcMinimumOutcome {
    pub request: PhyRxDcMinimumRequest,
    pub estimate: PhyDcIqEstimate,
    pub attempts: u8,
    /// Rust-owned replacement for the cumulative diagnostic halfword at
    /// `phy_param[0x1ac]`.
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumFailure {
    DcIq(PhyDcIqFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumAction {
    DcIq(PhyDcIqAction),
    Complete(PhyRxDcMinimumOutcome),
    Failed(PhyRxDcMinimumFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumCompletion {
    DcIq(PhyDcIqCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRxDcMinimumStep {
    Measure(PhyDcIqEstimateTransition),
    Complete(PhyRxDcMinimumOutcome),
    Failed(PhyRxDcMinimumFailure),
}

/// Heap-free translation of rev0 ROM `phy_rxdc_est_min`.
///
/// ROM may invoke `phy_dc_iq_est` eight times. The only apparent polling in
/// that child is represented by externally delivered readiness edges with a
/// finite owner-supplied deadline. This parent performs no hidden retry,
/// delay, allocation, or global-state access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcMinimumTransition {
    request: PhyRxDcMinimumRequest,
    step: PhyRxDcMinimumStep,
    attempt: u8,
    best: PhyDcIqEstimate,
    minimum_power: i32,
    readiness_activity_edges: u16,
}

impl PhyRxDcMinimumTransition {
    pub const fn new(request: PhyRxDcMinimumRequest) -> Self {
        Self {
            request,
            step: PhyRxDcMinimumStep::Measure(PhyDcIqEstimateTransition::new(
                Self::estimate_request(request, 0),
            )),
            attempt: 0,
            best: PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 100,
            },
            minimum_power: 100,
            readiness_activity_edges: 0,
        }
    }

    const fn estimate_request(
        request: PhyRxDcMinimumRequest,
        attempt: u8,
    ) -> PhyDcIqEstimateRequest {
        PhyDcIqEstimateRequest {
            iteration: attempt,
            chain: 1,
            control: request.control,
            mode: request.mode,
        }
    }

    pub const fn action(self) -> PhyRxDcMinimumAction {
        match self.step {
            PhyRxDcMinimumStep::Measure(transition) => {
                PhyRxDcMinimumAction::DcIq(transition.action())
            }
            PhyRxDcMinimumStep::Complete(outcome) => PhyRxDcMinimumAction::Complete(outcome),
            PhyRxDcMinimumStep::Failed(failure) => PhyRxDcMinimumAction::Failed(failure),
        }
    }

    fn accept_outcome(&mut self, outcome: PhyDcIqEstimateOutcome) {
        self.readiness_activity_edges = self
            .readiness_activity_edges
            .wrapping_add(outcome.readiness_activity_edges);

        // `phy_iq_est_enable` clears `phy_param[0x1ac]` before every child
        // estimate.  The minimum selector therefore tests only this
        // attempt's activity count.  Keep the sum solely as an owned
        // diagnostic; using it as the acceptance gate would permanently
        // reject clean later attempts after one active sample.
        if outcome.estimate.power < self.minimum_power
            && (outcome.readiness_activity_edges == 0 || self.request.rx_saturation_detected)
        {
            self.best = outcome.estimate;
            self.minimum_power = outcome.estimate.power;
        }

        let attempts = self.attempt + 1;
        if self.minimum_power < 36 || (attempts >= 3 && self.minimum_power < 48) {
            self.step = PhyRxDcMinimumStep::Complete(PhyRxDcMinimumOutcome {
                request: self.request,
                estimate: self.best,
                attempts,
                readiness_activity_edges: self.readiness_activity_edges,
            });
        } else if attempts == RX_DC_MINIMUM_MAX_ATTEMPTS {
            // The ROM unconditionally overwrites only output word two with
            // 0x38 on this path. Rust keeps deterministic I/Q ownership and
            // reproduces that final power sentinel.
            let mut estimate = self.best;
            estimate.power = 0x38;
            self.step = PhyRxDcMinimumStep::Complete(PhyRxDcMinimumOutcome {
                request: self.request,
                estimate,
                attempts,
                readiness_activity_edges: self.readiness_activity_edges,
            });
        } else {
            self.attempt = attempts;
            self.step = PhyRxDcMinimumStep::Measure(PhyDcIqEstimateTransition::new(
                Self::estimate_request(self.request, attempts),
            ));
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxDcMinimumCompletion,
    ) -> Result<(), PhyRxDcMinimumTransitionError> {
        match (self.step, completion) {
            (
                PhyRxDcMinimumStep::Measure(mut transition),
                PhyRxDcMinimumCompletion::DcIq(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxDcMinimumTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyDcIqAction::Complete(outcome) => self.accept_outcome(outcome),
                    PhyDcIqAction::Failed(failure) => {
                        self.step =
                            PhyRxDcMinimumStep::Failed(PhyRxDcMinimumFailure::DcIq(failure));
                    }
                    _ => self.step = PhyRxDcMinimumStep::Measure(transition),
                }
            }
            (PhyRxDcMinimumStep::Complete(_), _) | (PhyRxDcMinimumStep::Failed(_), _) => {
                return Err(PhyRxDcMinimumTransitionError::AlreadyComplete);
            }
        }
        Ok(())
    }
}

/// Exhaustive lowering of the non-terminal `phy_rxdc_est_min` surface.
///
/// The minimum selector owns no hardware edge of its own: every observable
/// operation belongs to the nested DC/IQ estimator. Keeping this wrapper
/// explicit prevents the cold-init executor from reaching into child state.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxDcMinimumExternalBinding {
    DcIq(crate::phy_dc_iq::PhyDcIqExternalBinding),
}

impl PhyRxDcMinimumExternalBinding {
    pub fn lower(action: PhyRxDcMinimumAction) -> Result<Self, PhyRxDcoBindingError> {
        let PhyRxDcMinimumAction::DcIq(action) = action else {
            return Err(PhyRxDcoBindingError::UnsupportedAction);
        };
        crate::phy_dc_iq::PhyDcIqExternalBinding::lower(action)
            .map(Self::DcIq)
            .map_err(|_| PhyRxDcoBindingError::UnsupportedAction)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRxDcoStep {
    PrepareControlRestore,
    ReadPbus,
    SetupPbus {
        index: u8,
    },
    ForceI,
    ForceQ,
    Delay,
    Measure {
        transition: PhyDcIqEstimateTransition,
    },
    RestoreSuccess {
        outcome: PhyRxDcoOutcome,
    },
    RestoreFailure {
        failure: PhyRxDcoFailure,
    },
    Complete(PhyRxDcoOutcome),
    Failed(PhyRxDcoFailure),
}

/// Exact fixed-input compensation helper recovered from
/// `phy_get_dco_comp(1, 0, measurement, previous, population, upper)`.
///
/// All arithmetic is explicit and stateless. `upper` is the upper byte of
/// the PBus value after shifting it right by six; its physical field name is
/// intentionally not guessed.
pub fn rx_dco_compensation_step(measurement: i32, previous: i32, population: u8, upper: u8) -> i32 {
    let upper_group = u32::from(upper >= 4);
    let shift = u32::from(population) + upper_group;
    let mut compensation = measurement >> shift;
    let delta = measurement.wrapping_sub(previous).wrapping_abs();
    let reference = measurement.wrapping_mul(3).wrapping_div(2).wrapping_abs();
    if reference < delta {
        compensation >>= 1;
    }
    if compensation == 0 {
        if measurement > 0 { 1 } else { -1 }
    } else {
        compensation
    }
}

const fn setup_transaction(index: u8, initial_i: i16, initial_q: i16) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(2, 1, initial_i as u16),
        1 => PhyPbusForceTest::new(3, 1, initial_q as u16),
        2 => PhyPbusForceTest::new(2, 2, 0x100),
        _ => PhyPbusForceTest::new(3, 2, 0x100),
    }
}

const fn threshold(population: u8) -> i32 {
    if population <= 1 {
        2
    } else if population <= 3 {
        4
    } else {
        10
    }
}

const fn initial_i(configuration: [u32; 2]) -> i16 {
    configuration[0] as u16 as i16
}

const fn initial_q(configuration: [u32; 2]) -> i16 {
    (configuration[0] >> 16) as u16 as i16
}

const fn output_configuration(configuration: [u32; 2], i: i16, q: i16) -> [u32; 2] {
    [
        (i as u16 as u32) | ((q as u16 as u32) << 16),
        configuration[1],
    ]
}

/// Caller-driven, heap-free translation of the reachable RX-DCO loop.
///
/// It has no future, waker, allocation, callback, internal delay, readiness
/// poll, or retry. Each PBus command, timer expiry, and IQ measurement is
/// accepted only through an identity-bound completion. Both success and
/// typed child failure restore the saved RX-DCO control field before becoming
/// terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcoTransition {
    request: PhyRxDcoRequest,
    step: PhyRxDcoStep,
    population: u8,
    upper: u8,
    threshold: i32,
    current_i: i16,
    current_q: i16,
    previous: PhyDcIqEstimate,
    iteration: u8,
}

impl PhyRxDcoTransition {
    pub const fn new(request: PhyRxDcoRequest) -> Self {
        Self {
            request,
            step: PhyRxDcoStep::PrepareControlRestore,
            population: 0,
            upper: 0,
            threshold: 0,
            current_i: initial_i(request.configuration),
            current_q: initial_q(request.configuration),
            previous: PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            iteration: 0,
        }
    }

    const fn estimate_request(self) -> PhyDcIqEstimateRequest {
        PhyDcIqEstimateRequest {
            iteration: self.iteration,
            chain: 1,
            control: self.request.control,
            mode: 0,
        }
    }

    pub const fn action(self) -> PhyRxDcoAction {
        match self.step {
            PhyRxDcoStep::PrepareControlRestore => PhyRxDcoAction::PrepareRxDcoControlRestore,
            PhyRxDcoStep::ReadPbus => PhyRxDcoAction::ReadPbus {
                selector: 1,
                path: 2,
            },
            PhyRxDcoStep::SetupPbus { index, .. } => PhyRxDcoAction::ForcePbus(setup_transaction(
                index,
                initial_i(self.request.configuration),
                initial_q(self.request.configuration),
            )),
            PhyRxDcoStep::ForceI => {
                PhyRxDcoAction::ForcePbus(PhyPbusForceTest::new(2, 1, self.current_i as u16))
            }
            PhyRxDcoStep::ForceQ => {
                PhyRxDcoAction::ForcePbus(PhyPbusForceTest::new(3, 1, self.current_q as u16))
            }
            PhyRxDcoStep::Delay => PhyRxDcoAction::DelayMicros {
                iteration: self.iteration,
                micros: self.request.delay_micros,
            },
            PhyRxDcoStep::Measure { transition } => PhyRxDcoAction::DcIq(transition.action()),
            PhyRxDcoStep::RestoreSuccess { .. } | PhyRxDcoStep::RestoreFailure { .. } => {
                PhyRxDcoAction::RestoreRxDcoControl
            }
            PhyRxDcoStep::Complete(outcome) => PhyRxDcoAction::Complete(outcome),
            PhyRxDcoStep::Failed(failure) => PhyRxDcoAction::Failed(failure),
        }
    }

    fn pbus_failure(&mut self, transaction: PhyPbusForceTest) {
        self.step = PhyRxDcoStep::RestoreFailure {
            failure: PhyRxDcoFailure::PbusForceTimedOut(transaction),
        };
    }

    fn accept_estimate(&mut self, estimate: PhyDcIqEstimate) {
        let i_within = estimate.i.wrapping_abs() <= self.threshold;
        let q_within = estimate.q.wrapping_abs() <= self.threshold;
        let completed_iterations = self.iteration + 1;

        if i_within && q_within {
            self.step = PhyRxDcoStep::RestoreSuccess {
                outcome: PhyRxDcoOutcome {
                    configuration: output_configuration(
                        self.request.configuration,
                        self.current_i,
                        self.current_q,
                    ),
                    iterations: completed_iterations,
                    converged: true,
                    last_estimate: estimate,
                },
            };
            return;
        }

        let previous = if self.iteration == 0 {
            estimate
        } else {
            self.previous
        };
        if !i_within {
            let compensation =
                rx_dco_compensation_step(estimate.i, previous.i, self.population, self.upper);
            self.current_i = i32::from(self.current_i).wrapping_sub(compensation) as i16;
        }
        if !q_within {
            let compensation =
                rx_dco_compensation_step(estimate.q, previous.q, self.population, self.upper);
            self.current_q = i32::from(self.current_q).wrapping_sub(compensation) as i16;
        }
        self.previous = estimate;

        if completed_iterations == MAX_ITERATIONS {
            self.step = PhyRxDcoStep::RestoreSuccess {
                outcome: PhyRxDcoOutcome {
                    configuration: output_configuration(
                        self.request.configuration,
                        self.current_i,
                        self.current_q,
                    ),
                    iterations: completed_iterations,
                    converged: false,
                    last_estimate: estimate,
                },
            };
        } else {
            self.iteration = completed_iterations;
            self.step = PhyRxDcoStep::ForceI;
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxDcoCompletion,
    ) -> Result<(), PhyRxDcoTransitionError> {
        match (self.step, completion) {
            (
                PhyRxDcoStep::PrepareControlRestore,
                PhyRxDcoCompletion::RxDcoControlRestorePrepared,
            ) => {
                self.step = PhyRxDcoStep::ReadPbus;
            }
            (
                PhyRxDcoStep::ReadPbus,
                PhyRxDcoCompletion::PbusRead {
                    selector: 1,
                    path: 2,
                    value,
                },
            ) => {
                let low = (value & 0x3f) as u8;
                self.population = low.count_ones() as u8;
                self.upper = ((value >> 6) & 0xff) as u8;
                self.threshold = threshold(self.population);
                self.step = PhyRxDcoStep::SetupPbus { index: 0 };
            }
            (
                PhyRxDcoStep::SetupPbus { index },
                PhyRxDcoCompletion::PbusForceCompleted(transaction),
            ) if transaction
                == setup_transaction(
                    index,
                    initial_i(self.request.configuration),
                    initial_q(self.request.configuration),
                ) =>
            {
                self.step = if index == 3 {
                    PhyRxDcoStep::ForceI
                } else {
                    PhyRxDcoStep::SetupPbus { index: index + 1 }
                };
            }
            (
                PhyRxDcoStep::SetupPbus { index },
                PhyRxDcoCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == setup_transaction(
                    index,
                    initial_i(self.request.configuration),
                    initial_q(self.request.configuration),
                ) =>
            {
                self.pbus_failure(transaction);
            }
            (PhyRxDcoStep::ForceI, PhyRxDcoCompletion::PbusForceCompleted(transaction))
                if transaction == PhyPbusForceTest::new(2, 1, self.current_i as u16) =>
            {
                self.step = PhyRxDcoStep::ForceQ;
            }
            (PhyRxDcoStep::ForceI, PhyRxDcoCompletion::PbusForceTimedOut(transaction))
                if transaction == PhyPbusForceTest::new(2, 1, self.current_i as u16) =>
            {
                self.pbus_failure(transaction);
            }
            (PhyRxDcoStep::ForceQ, PhyRxDcoCompletion::PbusForceCompleted(transaction))
                if transaction == PhyPbusForceTest::new(3, 1, self.current_q as u16) =>
            {
                self.step = PhyRxDcoStep::Delay;
            }
            (PhyRxDcoStep::ForceQ, PhyRxDcoCompletion::PbusForceTimedOut(transaction))
                if transaction == PhyPbusForceTest::new(3, 1, self.current_q as u16) =>
            {
                self.pbus_failure(transaction);
            }
            (PhyRxDcoStep::Delay, PhyRxDcoCompletion::DelayElapsed { iteration, micros })
                if iteration == self.iteration && micros == self.request.delay_micros =>
            {
                self.step = PhyRxDcoStep::Measure {
                    transition: PhyDcIqEstimateTransition::new(self.estimate_request()),
                };
            }
            (PhyRxDcoStep::Measure { mut transition }, PhyRxDcoCompletion::DcIq(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxDcoTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyDcIqAction::Complete(outcome) => {
                        self.accept_estimate(outcome.estimate);
                    }
                    PhyDcIqAction::Failed(failure) => {
                        self.step = PhyRxDcoStep::RestoreFailure {
                            failure: PhyRxDcoFailure::DcIq(failure),
                        };
                    }
                    _ => {
                        self.step = PhyRxDcoStep::Measure { transition };
                    }
                }
            }
            (
                PhyRxDcoStep::RestoreSuccess { outcome },
                PhyRxDcoCompletion::RxDcoControlRestored,
            ) => {
                self.step = PhyRxDcoStep::Complete(outcome);
            }
            (
                PhyRxDcoStep::RestoreFailure { failure },
                PhyRxDcoCompletion::RxDcoControlRestored,
            ) => {
                self.step = PhyRxDcoStep::Failed(failure);
            }
            (PhyRxDcoStep::Complete(_), _) | (PhyRxDcoStep::Failed(_), _) => {
                return Err(PhyRxDcoTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxDcoTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoBindingError {
    UnsupportedAction,
    Pbus(crate::phy_pbus::PhyPbusHardwareBindingError),
}

/// An RX-DCO restore-stack invariant was violated by target execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoHardwareInvariant {
    /// Another calibration owns the shared PAC restore slot.
    RestoreOwnedByOtherCalibration,
    /// More than the reviewed outer-plus-inner nesting was attempted.
    RestoreNestingExceeded,
    /// Cleanup reached restore without a matching successful prepare.
    RestoreNotPending,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcoMmioBinding {
    action: PhyRxDcoAction,
}

impl PhyRxDcoMmioBinding {
    pub fn new(action: PhyRxDcoAction) -> Result<Self, PhyRxDcoBindingError> {
        match action {
            PhyRxDcoAction::PrepareRxDcoControlRestore
            | PhyRxDcoAction::ReadPbus { .. }
            | PhyRxDcoAction::RestoreRxDcoControl => Ok(Self { action }),
            _ => Err(PhyRxDcoBindingError::UnsupportedAction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> Result<PhyRxDcoCompletion, PhyRxDcoHardwareInvariant> {
        match self.action {
            PhyRxDcoAction::PrepareRxDcoControlRestore => {
                open_esp_radio_esp32s31_hal::phy_rx_dco::prepare_control_restore(registers)
                    .map_err(|error| match error {
                        open_esp_radio_esp32s31_hal::types::RxDcoControlPrepareError::RestorePending => {
                            PhyRxDcoHardwareInvariant::RestoreOwnedByOtherCalibration
                        }
                        open_esp_radio_esp32s31_hal::types::RxDcoControlPrepareError::RestoreStackFull => {
                            PhyRxDcoHardwareInvariant::RestoreNestingExceeded
                        }
                    })?;
                Ok(PhyRxDcoCompletion::RxDcoControlRestorePrepared)
            }
            PhyRxDcoAction::ReadPbus { selector, path } => Ok(PhyRxDcoCompletion::PbusRead {
                selector,
                path,
                value: {
                    let result =
                        open_esp_radio_esp32s31_hal::pbus::read_result(registers, selector, path);
                    debug_assert!(
                        result.is_some(),
                        "RX-DCO transition emitted an unrecovered PBus selector"
                    );
                    u32::from(result.unwrap_or(0))
                },
            }),
            PhyRxDcoAction::RestoreRxDcoControl => {
                open_esp_radio_esp32s31_hal::phy_rx_dco::restore_control(registers)
                    .map_err(|_| PhyRxDcoHardwareInvariant::RestoreNotPending)?;
                Ok(PhyRxDcoCompletion::RxDcoControlRestored)
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcoPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::phy_pbus::PhyPbusHardwareBinding,
}

impl PhyRxDcoPbusBinding {
    pub fn new(action: PhyRxDcoAction) -> Result<Self, PhyRxDcoBindingError> {
        let PhyRxDcoAction::ForcePbus(transaction) = action else {
            return Err(PhyRxDcoBindingError::UnsupportedAction);
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
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> Result<(), crate::phy_pbus::PhyPbusHardwareBindingError> {
        self.hardware.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> Result<
        crate::phy_pbus::PhyPbusHardwareObservation,
        crate::phy_pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_target_edge(registers)
    }

    pub fn into_completion(self) -> Result<PhyRxDcoCompletion, PhyRxDcoBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyRxDcoCompletion::PbusForceCompleted)
            .map_err(PhyRxDcoBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxDcoCompletion {
        PhyRxDcoCompletion::PbusForceTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcoTimerBinding {
    iteration: u8,
    micros: u32,
}

impl PhyRxDcoTimerBinding {
    pub fn new(action: PhyRxDcoAction) -> Result<Self, PhyRxDcoBindingError> {
        match action {
            PhyRxDcoAction::DelayMicros { iteration, micros } => Ok(Self { iteration, micros }),
            _ => Err(PhyRxDcoBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxDcoCompletion {
        PhyRxDcoCompletion::DelayElapsed {
            iteration: self.iteration,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxDcoExternalBinding {
    Mmio(PhyRxDcoMmioBinding),
    Pbus(PhyRxDcoPbusBinding),
    Timer(PhyRxDcoTimerBinding),
    DcIq(crate::phy_dc_iq::PhyDcIqExternalBinding),
}

impl PhyRxDcoExternalBinding {
    pub fn lower(action: PhyRxDcoAction) -> Result<Self, PhyRxDcoBindingError> {
        if let Ok(binding) = PhyRxDcoMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxDcoPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxDcoTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        if let PhyRxDcoAction::DcIq(action) = action {
            return crate::phy_dc_iq::PhyDcIqExternalBinding::lower(action)
                .map(Self::DcIq)
                .map_err(|_| PhyRxDcoBindingError::UnsupportedAction);
        }
        Err(PhyRxDcoBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateOutcome,
        PhyDcIqReadinessSnapshot,
    };

    fn complete_dc_iq_action(
        action: PhyDcIqAction,
        estimate: PhyDcIqEstimate,
    ) -> PhyDcIqCompletion {
        match action {
            PhyDcIqAction::Configure(request) => PhyDcIqCompletion::Configured(request),
            PhyDcIqAction::SetEnable {
                request,
                phase,
                enabled,
            } => PhyDcIqCompletion::EnableSet {
                request,
                phase,
                enabled,
            },
            PhyDcIqAction::DelayMicros {
                request,
                phase,
                micros,
            } => PhyDcIqCompletion::DelayElapsed {
                request,
                phase,
                micros,
            },
            PhyDcIqAction::AwaitReadinessEdge { request, .. } => {
                PhyDcIqCompletion::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot {
                        ready: true,
                        activity: false,
                    },
                }
            }
            PhyDcIqAction::ReadAccumulators(request) => {
                let divisor = i32::from(request.control) + 1;
                PhyDcIqCompletion::AccumulatorsRead {
                    request,
                    snapshot: PhyDcIqAccumulatorSnapshot {
                        i: estimate.i.wrapping_mul(divisor).wrapping_shl(6),
                        q: estimate.q.wrapping_mul(divisor).wrapping_shl(6),
                        power: 0,
                    },
                }
            }
            action => panic!("unexpected terminal DC/IQ action: {action:?}"),
        }
    }

    fn complete_until_measurement(transition: &mut PhyRxDcoTransition) {
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlRestorePrepared)
            .unwrap();
        transition
            .advance(PhyRxDcoCompletion::PbusRead {
                selector: 1,
                path: 2,
                value: 0,
            })
            .unwrap();
        loop {
            match transition.action() {
                PhyRxDcoAction::ForcePbus(transaction) => transition
                    .advance(PhyRxDcoCompletion::PbusForceCompleted(transaction))
                    .unwrap(),
                PhyRxDcoAction::DelayMicros { iteration, micros } => transition
                    .advance(PhyRxDcoCompletion::DelayElapsed { iteration, micros })
                    .unwrap(),
                PhyRxDcoAction::DcIq(_) => return,
                action => panic!("unexpected RX-DCO setup action: {action:?}"),
            }
        }
    }

    fn complete_one_measurement(transition: &mut PhyRxDcoTransition, estimate: PhyDcIqEstimate) {
        loop {
            let PhyRxDcoAction::DcIq(action) = transition.action() else {
                return;
            };
            transition
                .advance(PhyRxDcoCompletion::DcIq(complete_dc_iq_action(
                    action, estimate,
                )))
                .unwrap();
        }
    }

    fn advance_to_next_measurement(transition: &mut PhyRxDcoTransition) {
        loop {
            match transition.action() {
                PhyRxDcoAction::ForcePbus(transaction) => transition
                    .advance(PhyRxDcoCompletion::PbusForceCompleted(transaction))
                    .unwrap(),
                PhyRxDcoAction::DelayMicros { iteration, micros } => transition
                    .advance(PhyRxDcoCompletion::DelayElapsed { iteration, micros })
                    .unwrap(),
                PhyRxDcoAction::DcIq(_) => return,
                action => panic!("unexpected RX-DCO loop action: {action:?}"),
            }
        }
    }

    #[test]
    fn compensation_matches_reachable_rom_branches() {
        assert_eq!(rx_dco_compensation_step(16, 16, 0, 0), 16);
        assert_eq!(rx_dco_compensation_step(16, 16, 2, 4), 2);
        assert_eq!(rx_dco_compensation_step(16, -16, 0, 0), 8);
        assert_eq!(rx_dco_compensation_step(0, 0, 6, 0), -1);
        assert_eq!(rx_dco_compensation_step(1, 1, 6, 4), 1);
    }

    #[test]
    fn early_success_restores_control_field_before_completion() {
        let mut transition = PhyRxDcoTransition::new(PhyRxDcoRequest::XTAL_DUTY);
        complete_until_measurement(&mut transition);
        complete_one_measurement(
            &mut transition,
            PhyDcIqEstimate {
                i: 2,
                q: -2,
                power: 0,
            },
        );

        let PhyRxDcoAction::RestoreRxDcoControl = transition.action() else {
            panic!("RX-DCO control restoration was not requested");
        };
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlRestored)
            .unwrap();

        let PhyRxDcoAction::Complete(outcome) = transition.action() else {
            panic!("RX-DCO did not complete");
        };
        assert!(outcome.converged);
        assert_eq!(outcome.iterations, 1);
        assert_eq!(outcome.configuration, [0x0100_0100; 2]);
    }

    #[test]
    fn non_converging_measurement_stops_after_twelve_iterations() {
        let mut transition = PhyRxDcoTransition::new(PhyRxDcoRequest::XTAL_DUTY);
        complete_until_measurement(&mut transition);
        for iteration in 0..MAX_ITERATIONS {
            complete_one_measurement(
                &mut transition,
                PhyDcIqEstimate {
                    i: 100,
                    q: -100,
                    power: 0,
                },
            );
            if iteration + 1 != MAX_ITERATIONS {
                advance_to_next_measurement(&mut transition);
            }
        }
        let PhyRxDcoAction::RestoreRxDcoControl = transition.action() else {
            panic!("bounded RX-DCO loop did not request restoration");
        };
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlRestored)
            .unwrap();
        let PhyRxDcoAction::Complete(outcome) = transition.action() else {
            panic!("bounded RX-DCO loop did not complete");
        };
        assert_eq!(outcome.iterations, MAX_ITERATIONS);
        assert!(!outcome.converged);
    }

    #[test]
    fn pbus_timeout_restores_control_field_before_typed_failure() {
        let mut transition = PhyRxDcoTransition::new(PhyRxDcoRequest::XTAL_DUTY);
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlRestorePrepared)
            .unwrap();
        transition
            .advance(PhyRxDcoCompletion::PbusRead {
                selector: 1,
                path: 2,
                value: 0,
            })
            .unwrap();
        let PhyRxDcoAction::ForcePbus(transaction) = transition.action() else {
            panic!("setup PBus command was not requested");
        };
        transition
            .advance(PhyRxDcoCompletion::PbusForceTimedOut(transaction))
            .unwrap();
        assert!(matches!(
            transition.action(),
            PhyRxDcoAction::RestoreRxDcoControl
        ));
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlRestored)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRxDcoAction::Failed(PhyRxDcoFailure::PbusForceTimedOut(transaction))
        );
    }

    const MINIMUM_REQUEST: PhyRxDcMinimumRequest = PhyRxDcMinimumRequest {
        measurement: 4,
        control: 0x0fa0,
        mode: 0,
        rx_saturation_detected: false,
    };

    fn minimum_outcome(
        attempt: u8,
        power: i32,
        readiness_activity_edges: u16,
    ) -> PhyDcIqEstimateOutcome {
        PhyDcIqEstimateOutcome {
            request: PhyDcIqEstimateRequest {
                iteration: attempt,
                chain: 1,
                control: MINIMUM_REQUEST.control,
                mode: MINIMUM_REQUEST.mode,
            },
            estimate: PhyDcIqEstimate {
                i: i32::from(attempt),
                q: -i32::from(attempt),
                power,
            },
            readiness_activity_edges,
        }
    }

    #[test]
    fn rx_dc_minimum_completes_immediately_below_36() {
        let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
        transition.accept_outcome(minimum_outcome(0, 35, 0));
        assert_eq!(
            transition.action(),
            PhyRxDcMinimumAction::Complete(PhyRxDcMinimumOutcome {
                request: MINIMUM_REQUEST,
                estimate: PhyDcIqEstimate {
                    i: 0,
                    q: 0,
                    power: 35,
                },
                attempts: 1,
                readiness_activity_edges: 0,
            })
        );
    }

    #[test]
    fn rx_dc_minimum_requires_three_attempts_between_36_and_47() {
        let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
        transition.accept_outcome(minimum_outcome(0, 40, 0));
        transition.accept_outcome(minimum_outcome(1, 42, 0));
        transition.accept_outcome(minimum_outcome(2, 41, 0));
        let PhyRxDcMinimumAction::Complete(outcome) = transition.action() else {
            panic!("RX-DC minimum did not complete after three attempts");
        };
        assert_eq!(outcome.attempts, 3);
        assert_eq!(outcome.estimate.power, 40);
    }

    #[test]
    fn rx_dc_minimum_accepts_a_clean_attempt_after_prior_activity() {
        let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
        transition.accept_outcome(minimum_outcome(0, 20, 1));
        transition.accept_outcome(minimum_outcome(1, 35, 0));
        let PhyRxDcMinimumAction::Complete(outcome) = transition.action() else {
            panic!("clean second attempt was not accepted");
        };
        assert_eq!(outcome.attempts, 2);
        assert_eq!(
            outcome.estimate,
            PhyDcIqEstimate {
                i: 1,
                q: -1,
                power: 35,
            }
        );
        assert_eq!(outcome.readiness_activity_edges, 1);
    }

    #[test]
    fn rx_dc_minimum_uses_rom_power_sentinel_after_eight_rejected_samples() {
        let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
        for attempt in 0..RX_DC_MINIMUM_MAX_ATTEMPTS {
            transition.accept_outcome(minimum_outcome(attempt, 20, 1));
        }
        let PhyRxDcMinimumAction::Complete(outcome) = transition.action() else {
            panic!("bounded RX-DC minimum did not complete");
        };
        assert_eq!(outcome.attempts, RX_DC_MINIMUM_MAX_ATTEMPTS);
        assert_eq!(
            outcome.estimate,
            PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0x38
            }
        );
        assert_eq!(
            outcome.readiness_activity_edges,
            u16::from(RX_DC_MINIMUM_MAX_ATTEMPTS)
        );
    }

    #[test]
    fn rx_dc_minimum_propagates_child_timeout_after_disable_tail() {
        let mut transition = PhyRxDcMinimumTransition::new(MINIMUM_REQUEST);
        while let PhyRxDcMinimumAction::DcIq(action) = transition.action() {
            let completion = match action {
                PhyDcIqAction::Configure(request) => PhyDcIqCompletion::Configured(request),
                PhyDcIqAction::SetEnable {
                    request,
                    phase,
                    enabled,
                } => PhyDcIqCompletion::EnableSet {
                    request,
                    phase,
                    enabled,
                },
                PhyDcIqAction::DelayMicros {
                    request,
                    phase,
                    micros,
                } => PhyDcIqCompletion::DelayElapsed {
                    request,
                    phase,
                    micros,
                },
                PhyDcIqAction::AwaitReadinessEdge { request, .. } => {
                    PhyDcIqCompletion::ReadinessTimedOut(request)
                }
                action => panic!("unexpected child action: {action:?}"),
            };
            transition
                .advance(PhyRxDcMinimumCompletion::DcIq(completion))
                .unwrap();
        }
        let PhyRxDcMinimumAction::Failed(PhyRxDcMinimumFailure::DcIq(
            PhyDcIqFailure::ReadinessTimedOut {
                request,
                readiness_activity_edges: 0,
            },
        )) = transition.action()
        else {
            panic!("typed child timeout was not propagated");
        };
        assert_eq!(
            request,
            PhyDcIqEstimateRequest {
                iteration: 0,
                chain: 1,
                control: MINIMUM_REQUEST.control,
                mode: MINIMUM_REQUEST.mode,
            }
        );
    }
}
