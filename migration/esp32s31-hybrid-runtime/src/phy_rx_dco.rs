//! Event-driven RX DC-offset calibration for ESP32-S31 rev0.
//!
//! Primary reference: `esp32s31_rev0_rom.elf::phy_pbus_rx_dco_cal` at
//! `0x2f82_8f44`, size `0x228`. The crystal-duty caller uses the fixed
//! argument tuple `(0x0fa0, configuration, 10, 0, 0)`. This module owns the
//! complete reachable twelve-iteration control loop. Its `phy_dc_iq_est`
//! child is another Rust-owned transition, so the ROM hardware-ready spin and
//! synchronous delays are absent from the complete reachable graph.

use crate::phy_dc_iq::{
    PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateTransition, PhyDcIqFailure,
};
pub use crate::phy_dc_iq::{PhyDcIqEstimate, PhyDcIqEstimateRequest};
use crate::phy_pbus::PhyPbusForceTest;

pub const RX_DCO_CONTROL_ADDRESS: usize = 0x2010_0434;
pub const RX_DCO_CONTROL_FIELD_MASK: u32 = 0x00c0_0000;

const MAX_ITERATIONS: u8 = 12;

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
    MaskRxDcoControl {
        address: usize,
        clear_mask: u32,
    },
    ReadPbus {
        selector: u8,
        path: u8,
    },
    ForcePbus(PhyPbusForceTest),
    DelayMicros {
        iteration: u8,
        micros: u32,
    },
    DcIq(PhyDcIqAction),
    RestoreRxDcoControl {
        address: usize,
        field_mask: u32,
        saved_field: u32,
    },
    Complete(PhyRxDcoOutcome),
    Failed(PhyRxDcoFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoCompletion {
    RxDcoControlMasked { address: usize, saved_field: u32 },
    PbusRead { selector: u8, path: u8, value: u32 },
    PbusForceCompleted(PhyPbusForceTest),
    PbusForceTimedOut(PhyPbusForceTest),
    DelayElapsed { iteration: u8, micros: u32 },
    DcIq(PhyDcIqCompletion),
    RxDcoControlRestored { address: usize, saved_field: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcoTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRxDcoStep {
    MaskControl,
    ReadPbus {
        saved_field: u32,
    },
    SetupPbus {
        saved_field: u32,
        index: u8,
    },
    ForceI {
        saved_field: u32,
    },
    ForceQ {
        saved_field: u32,
    },
    Delay {
        saved_field: u32,
    },
    Measure {
        saved_field: u32,
        transition: PhyDcIqEstimateTransition,
    },
    RestoreSuccess {
        saved_field: u32,
        outcome: PhyRxDcoOutcome,
    },
    RestoreFailure {
        saved_field: u32,
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
        if measurement > 0 {
            1
        } else {
            -1
        }
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
            step: PhyRxDcoStep::MaskControl,
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
            PhyRxDcoStep::MaskControl => PhyRxDcoAction::MaskRxDcoControl {
                address: RX_DCO_CONTROL_ADDRESS,
                clear_mask: RX_DCO_CONTROL_FIELD_MASK,
            },
            PhyRxDcoStep::ReadPbus { .. } => PhyRxDcoAction::ReadPbus {
                selector: 1,
                path: 2,
            },
            PhyRxDcoStep::SetupPbus { index, .. } => PhyRxDcoAction::ForcePbus(setup_transaction(
                index,
                initial_i(self.request.configuration),
                initial_q(self.request.configuration),
            )),
            PhyRxDcoStep::ForceI { .. } => {
                PhyRxDcoAction::ForcePbus(PhyPbusForceTest::new(2, 1, self.current_i as u16))
            }
            PhyRxDcoStep::ForceQ { .. } => {
                PhyRxDcoAction::ForcePbus(PhyPbusForceTest::new(3, 1, self.current_q as u16))
            }
            PhyRxDcoStep::Delay { .. } => PhyRxDcoAction::DelayMicros {
                iteration: self.iteration,
                micros: self.request.delay_micros,
            },
            PhyRxDcoStep::Measure { transition, .. } => PhyRxDcoAction::DcIq(transition.action()),
            PhyRxDcoStep::RestoreSuccess { saved_field, .. }
            | PhyRxDcoStep::RestoreFailure { saved_field, .. } => {
                PhyRxDcoAction::RestoreRxDcoControl {
                    address: RX_DCO_CONTROL_ADDRESS,
                    field_mask: RX_DCO_CONTROL_FIELD_MASK,
                    saved_field,
                }
            }
            PhyRxDcoStep::Complete(outcome) => PhyRxDcoAction::Complete(outcome),
            PhyRxDcoStep::Failed(failure) => PhyRxDcoAction::Failed(failure),
        }
    }

    fn pbus_failure(&mut self, saved_field: u32, transaction: PhyPbusForceTest) {
        self.step = PhyRxDcoStep::RestoreFailure {
            saved_field,
            failure: PhyRxDcoFailure::PbusForceTimedOut(transaction),
        };
    }

    fn accept_estimate(&mut self, saved_field: u32, estimate: PhyDcIqEstimate) {
        let i_within = estimate.i.wrapping_abs() <= self.threshold;
        let q_within = estimate.q.wrapping_abs() <= self.threshold;
        let completed_iterations = self.iteration + 1;

        if i_within && q_within {
            self.step = PhyRxDcoStep::RestoreSuccess {
                saved_field,
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
                saved_field,
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
            self.step = PhyRxDcoStep::ForceI { saved_field };
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxDcoCompletion,
    ) -> Result<(), PhyRxDcoTransitionError> {
        match (self.step, completion) {
            (
                PhyRxDcoStep::MaskControl,
                PhyRxDcoCompletion::RxDcoControlMasked {
                    address: RX_DCO_CONTROL_ADDRESS,
                    saved_field,
                },
            ) if saved_field & !RX_DCO_CONTROL_FIELD_MASK == 0 => {
                self.step = PhyRxDcoStep::ReadPbus { saved_field };
            }
            (
                PhyRxDcoStep::ReadPbus { saved_field },
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
                self.step = PhyRxDcoStep::SetupPbus {
                    saved_field,
                    index: 0,
                };
            }
            (
                PhyRxDcoStep::SetupPbus { saved_field, index },
                PhyRxDcoCompletion::PbusForceCompleted(transaction),
            ) if transaction
                == setup_transaction(
                    index,
                    initial_i(self.request.configuration),
                    initial_q(self.request.configuration),
                ) =>
            {
                self.step = if index == 3 {
                    PhyRxDcoStep::ForceI { saved_field }
                } else {
                    PhyRxDcoStep::SetupPbus {
                        saved_field,
                        index: index + 1,
                    }
                };
            }
            (
                PhyRxDcoStep::SetupPbus { saved_field, index },
                PhyRxDcoCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == setup_transaction(
                    index,
                    initial_i(self.request.configuration),
                    initial_q(self.request.configuration),
                ) =>
            {
                self.pbus_failure(saved_field, transaction);
            }
            (
                PhyRxDcoStep::ForceI { saved_field },
                PhyRxDcoCompletion::PbusForceCompleted(transaction),
            ) if transaction == PhyPbusForceTest::new(2, 1, self.current_i as u16) => {
                self.step = PhyRxDcoStep::ForceQ { saved_field };
            }
            (
                PhyRxDcoStep::ForceI { saved_field },
                PhyRxDcoCompletion::PbusForceTimedOut(transaction),
            ) if transaction == PhyPbusForceTest::new(2, 1, self.current_i as u16) => {
                self.pbus_failure(saved_field, transaction);
            }
            (
                PhyRxDcoStep::ForceQ { saved_field },
                PhyRxDcoCompletion::PbusForceCompleted(transaction),
            ) if transaction == PhyPbusForceTest::new(3, 1, self.current_q as u16) => {
                self.step = PhyRxDcoStep::Delay { saved_field };
            }
            (
                PhyRxDcoStep::ForceQ { saved_field },
                PhyRxDcoCompletion::PbusForceTimedOut(transaction),
            ) if transaction == PhyPbusForceTest::new(3, 1, self.current_q as u16) => {
                self.pbus_failure(saved_field, transaction);
            }
            (
                PhyRxDcoStep::Delay { saved_field },
                PhyRxDcoCompletion::DelayElapsed { iteration, micros },
            ) if iteration == self.iteration && micros == self.request.delay_micros => {
                self.step = PhyRxDcoStep::Measure {
                    saved_field,
                    transition: PhyDcIqEstimateTransition::new(self.estimate_request()),
                };
            }
            (
                PhyRxDcoStep::Measure {
                    saved_field,
                    mut transition,
                },
                PhyRxDcoCompletion::DcIq(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxDcoTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyDcIqAction::Complete(outcome) => {
                        self.accept_estimate(saved_field, outcome.estimate);
                    }
                    PhyDcIqAction::Failed(failure) => {
                        self.step = PhyRxDcoStep::RestoreFailure {
                            saved_field,
                            failure: PhyRxDcoFailure::DcIq(failure),
                        };
                    }
                    _ => {
                        self.step = PhyRxDcoStep::Measure {
                            saved_field,
                            transition,
                        };
                    }
                }
            }
            (
                PhyRxDcoStep::RestoreSuccess {
                    saved_field,
                    outcome,
                },
                PhyRxDcoCompletion::RxDcoControlRestored {
                    address: RX_DCO_CONTROL_ADDRESS,
                    saved_field: completed_field,
                },
            ) if saved_field == completed_field => {
                self.step = PhyRxDcoStep::Complete(outcome);
            }
            (
                PhyRxDcoStep::RestoreFailure {
                    saved_field,
                    failure,
                },
                PhyRxDcoCompletion::RxDcoControlRestored {
                    address: RX_DCO_CONTROL_ADDRESS,
                    saved_field: completed_field,
                },
            ) if saved_field == completed_field => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqReadinessSnapshot,
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

    fn complete_until_measurement(transition: &mut PhyRxDcoTransition, saved_field: u32) {
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlMasked {
                address: RX_DCO_CONTROL_ADDRESS,
                saved_field,
            })
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
        complete_until_measurement(&mut transition, 0x0080_0000);
        complete_one_measurement(
            &mut transition,
            PhyDcIqEstimate {
                i: 2,
                q: -2,
                power: 0,
            },
        );

        let PhyRxDcoAction::RestoreRxDcoControl {
            address,
            field_mask,
            saved_field,
        } = transition.action()
        else {
            panic!("RX-DCO control restoration was not requested");
        };
        assert_eq!(address, RX_DCO_CONTROL_ADDRESS);
        assert_eq!(field_mask, RX_DCO_CONTROL_FIELD_MASK);
        assert_eq!(saved_field, 0x0080_0000);
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlRestored {
                address,
                saved_field,
            })
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
        complete_until_measurement(&mut transition, 0);
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
        let PhyRxDcoAction::RestoreRxDcoControl { saved_field: 0, .. } = transition.action() else {
            panic!("bounded RX-DCO loop did not request restoration");
        };
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlRestored {
                address: RX_DCO_CONTROL_ADDRESS,
                saved_field: 0,
            })
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
            .advance(PhyRxDcoCompletion::RxDcoControlMasked {
                address: RX_DCO_CONTROL_ADDRESS,
                saved_field: 0x0040_0000,
            })
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
            PhyRxDcoAction::RestoreRxDcoControl {
                saved_field: 0x0040_0000,
                ..
            }
        ));
        transition
            .advance(PhyRxDcoCompletion::RxDcoControlRestored {
                address: RX_DCO_CONTROL_ADDRESS,
                saved_field: 0x0040_0000,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRxDcoAction::Failed(PhyRxDcoFailure::PbusForceTimedOut(transaction))
        );
    }
}
