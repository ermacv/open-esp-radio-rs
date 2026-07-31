//! Rust-owned ESP32-S31 RX gain DC-calibration primitives.
//!
//! The control flow is recovered from rev0 ROM
//! `phy_pbus_rx_dco_cal_1step` at `0x2f82_91ba`, size `0x2f0`.
//! Debug-only `ets_printf` branches are intentionally absent. Hardware
//! commands, ten-microsecond intervals, and DC/IQ readiness are explicit
//! caller-driven actions.

use crate::phy_i2c::{
    MaskedI2cWriteAction, MaskedI2cWriteCompletion, MaskedI2cWriteTransition, PhyI2cAddress,
};
use crate::phy_pbus::PhyPbusForceTest;
use crate::phy_rfpll::{
    RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure, RfpllFrequencyRequest,
    RfpllFrequencyTransition,
};
use crate::phy_rx_dco::{
    PhyRxDcMinimumAction, PhyRxDcMinimumCompletion, PhyRxDcMinimumFailure, PhyRxDcMinimumOutcome,
    PhyRxDcMinimumRequest, PhyRxDcMinimumTransition,
};

pub const PHY_RX_DC_CONTROL_ADDRESS: usize = crate::phy_rx_dco::RX_DCO_CONTROL_ADDRESS;
pub const PHY_RX_DC_CONTROL_FIELD_MASK: u32 = crate::phy_rx_dco::RX_DCO_CONTROL_FIELD_MASK;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationStage {
    Radio,
    Baseband,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationRequest {
    /// Selects the Wi-Fi (`false`) or shared-radio (`true`) calibration bank.
    pub shared_radio: bool,
    pub stage: PhyRxDcCalibrationStage,
    pub control: u16,
    pub initial: [u16; 2],
    /// Low-to-high estimator delta measured before the per-gain search.
    ///
    /// ROM `phy_pbus_rx_dco_cal_1step_new` subtracts this pair from every
    /// baseband-stage observation. Radio-stage searches leave it at zero.
    pub reference_delta: [i16; 2],
    pub gain_index: u8,
    pub rx_saturation_detected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationOutcome {
    pub request: PhyRxDcCalibrationRequest,
    pub configuration: [u16; 2],
    pub iterations: u8,
    pub converged: bool,
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationFailure {
    PbusForceTimedOut(PhyPbusForceTest),
    Minimum(PhyRxDcMinimumFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationAction {
    MaskControl {
        address: usize,
        clear_mask: u32,
    },
    ReadPbus {
        selector: u8,
        path: u8,
    },
    ForcePbus(PhyPbusForceTest),
    DelayMicros {
        measurement: u8,
        micros: u32,
    },
    Minimum(PhyRxDcMinimumAction),
    RestoreControl {
        address: usize,
        field_mask: u32,
        saved_field: u32,
    },
    Complete(PhyRxDcCalibrationOutcome),
    Failed(PhyRxDcCalibrationFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationCompletion {
    ControlMasked { address: usize, saved_field: u32 },
    PbusRead { selector: u8, path: u8, value: u32 },
    PbusForceCompleted(PhyPbusForceTest),
    PbusForceTimedOut(PhyPbusForceTest),
    DelayElapsed { measurement: u8, micros: u32 },
    Minimum(PhyRxDcMinimumCompletion),
    ControlRestored { address: usize, saved_field: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Terminal {
    Complete(PhyRxDcCalibrationOutcome),
    Failed(PhyRxDcCalibrationFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    MaskControl,
    ReadPbus {
        saved_field: u32,
    },
    ForceI {
        saved_field: u32,
    },
    ForceQ {
        saved_field: u32,
    },
    ForceRadioLevel {
        saved_field: u32,
        high: bool,
    },
    Delay {
        saved_field: u32,
        high: bool,
    },
    Minimum {
        saved_field: u32,
        high: bool,
        transition: PhyRxDcMinimumTransition,
    },
    CleanupI {
        saved_field: u32,
        terminal: Terminal,
    },
    CleanupQ {
        saved_field: u32,
        terminal: Terminal,
    },
    Restore {
        saved_field: u32,
        terminal: Terminal,
    },
    Complete(PhyRxDcCalibrationOutcome),
    Failed(PhyRxDcCalibrationFailure),
}

const fn saturate_9bit(value: i32) -> u16 {
    if value < 0 {
        0
    } else if value > 0x1ff {
        0x1ff
    } else {
        value as u16
    }
}

const fn saturate_signed_5(value: i32) -> i32 {
    if value < -5 {
        -5
    } else if value > 5 {
        5
    } else {
        value
    }
}

const fn signed_unit(value: i32) -> i32 {
    if value > 0 {
        1
    } else if value < 0 {
        -1
    } else {
        0
    }
}

/// Exact non-I/O correction arithmetic from `phy_pbus_rx_dco_cal_1step`.
pub const fn rx_dc_calibration_correction(delta: i32, low: i32, threshold: i32, shift: u8) -> i32 {
    let mut correction = if delta.wrapping_abs() >= threshold {
        delta >> shift
    } else {
        0
    };
    if correction == 0 {
        correction = if low.wrapping_abs() < 50 {
            signed_unit(delta)
        } else {
            low >> shift
        };
    }
    correction
}

/// Heap-free translation of one complete RX DC-calibration step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationTransition {
    request: PhyRxDcCalibrationRequest,
    step: Step,
    initial: [u16; 2],
    current: [u16; 2],
    population: u8,
    threshold: i32,
    iteration: u8,
    low: crate::phy_dc_iq::PhyDcIqEstimate,
    readiness_activity_edges: u16,
}

impl PhyRxDcCalibrationTransition {
    pub const fn new(request: PhyRxDcCalibrationRequest) -> Self {
        Self {
            request,
            step: Step::MaskControl,
            initial: request.initial,
            current: request.initial,
            population: 0,
            threshold: 0,
            iteration: 0,
            low: crate::phy_dc_iq::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            readiness_activity_edges: 0,
        }
    }

    const fn path(self) -> u8 {
        match self.request.stage {
            PhyRxDcCalibrationStage::Radio => 2,
            PhyRxDcCalibrationStage::Baseband => 1,
        }
    }

    const fn max_iterations(self) -> u8 {
        match self.request.stage {
            PhyRxDcCalibrationStage::Radio => 8,
            PhyRxDcCalibrationStage::Baseband => 12,
        }
    }

    const fn measurement_identity(self, high: bool) -> u8 {
        self.iteration.wrapping_mul(2).wrapping_add(high as u8)
    }

    const fn minimum_request(self, high: bool) -> PhyRxDcMinimumRequest {
        PhyRxDcMinimumRequest {
            measurement: self.measurement_identity(high),
            control: self.request.control,
            // The archive passes zero as the fourth `phy_rxdc_est_min`
            // argument. ROM forwards that fourth argument to
            // `phy_dc_iq_est`; its second argument is unused.
            mode: 0,
            rx_saturation_detected: self.request.rx_saturation_detected,
        }
    }

    const fn current_force(self, selector: u8) -> PhyPbusForceTest {
        let value = if selector == 2 {
            self.current[0]
        } else {
            self.current[1]
        };
        PhyPbusForceTest::new(selector, self.path(), value)
    }

    const fn cleanup_configuration(self, terminal: Terminal) -> [u16; 2] {
        match terminal {
            Terminal::Complete(outcome) => outcome.configuration,
            Terminal::Failed(_) => self.initial,
        }
    }

    pub const fn action(self) -> PhyRxDcCalibrationAction {
        match self.step {
            Step::MaskControl => PhyRxDcCalibrationAction::MaskControl {
                address: PHY_RX_DC_CONTROL_ADDRESS,
                clear_mask: PHY_RX_DC_CONTROL_FIELD_MASK,
            },
            Step::ReadPbus { .. } => PhyRxDcCalibrationAction::ReadPbus {
                selector: 1,
                path: 2,
            },
            Step::ForceI { .. } => PhyRxDcCalibrationAction::ForcePbus(self.current_force(2)),
            Step::ForceQ { .. } => PhyRxDcCalibrationAction::ForcePbus(self.current_force(3)),
            Step::ForceRadioLevel { high, .. } => PhyRxDcCalibrationAction::ForcePbus(
                PhyPbusForceTest::new(1, 2, if high { 0x20 } else { 0 }),
            ),
            Step::Delay { high, .. } => PhyRxDcCalibrationAction::DelayMicros {
                measurement: self.measurement_identity(high),
                micros: 10,
            },
            Step::Minimum { transition, .. } => {
                PhyRxDcCalibrationAction::Minimum(transition.action())
            }
            Step::CleanupI { terminal, .. } => PhyRxDcCalibrationAction::ForcePbus(
                PhyPbusForceTest::new(2, self.path(), self.cleanup_configuration(terminal)[0]),
            ),
            Step::CleanupQ { terminal, .. } => PhyRxDcCalibrationAction::ForcePbus(
                PhyPbusForceTest::new(3, self.path(), self.cleanup_configuration(terminal)[1]),
            ),
            Step::Restore { saved_field, .. } => PhyRxDcCalibrationAction::RestoreControl {
                address: PHY_RX_DC_CONTROL_ADDRESS,
                field_mask: PHY_RX_DC_CONTROL_FIELD_MASK,
                saved_field,
            },
            Step::Complete(outcome) => PhyRxDcCalibrationAction::Complete(outcome),
            Step::Failed(failure) => PhyRxDcCalibrationAction::Failed(failure),
        }
    }

    fn begin_cleanup(&mut self, saved_field: u32, terminal: Terminal) {
        self.step = Step::CleanupI {
            saved_field,
            terminal,
        };
    }

    fn fail(&mut self, saved_field: u32, failure: PhyRxDcCalibrationFailure) {
        self.begin_cleanup(saved_field, Terminal::Failed(failure));
    }

    fn accept_measurement(&mut self, saved_field: u32, high: bool, outcome: PhyRxDcMinimumOutcome) {
        self.readiness_activity_edges = self
            .readiness_activity_edges
            .wrapping_add(outcome.readiness_activity_edges);
        if self.request.stage == PhyRxDcCalibrationStage::Baseband && !high {
            self.low = outcome.estimate;
            self.step = Step::ForceRadioLevel {
                saved_field,
                high: true,
            };
            return;
        }

        let (delta_i, delta_q, power, shift) = match self.request.stage {
            PhyRxDcCalibrationStage::Radio => (
                outcome.estimate.i,
                outcome.estimate.q,
                outcome.estimate.power,
                self.population.max(2) - 2,
            ),
            PhyRxDcCalibrationStage::Baseband => (
                outcome
                    .estimate
                    .i
                    .wrapping_sub(self.low.i)
                    .wrapping_sub(i32::from(self.request.reference_delta[0])),
                outcome
                    .estimate
                    .q
                    .wrapping_sub(self.low.q)
                    .wrapping_sub(i32::from(self.request.reference_delta[1])),
                outcome.estimate.power.max(self.low.power),
                if self.request.shared_radio {
                    3
                } else if self.iteration >= 2 {
                    // ROM `.L11`: Wi-Fi baseband correction switches from
                    // shift zero to shift one after the first two attempts.
                    1
                } else {
                    0
                },
            ),
        };

        let mut correction_i =
            rx_dc_calibration_correction(delta_i, self.low.i, self.threshold, shift);
        let mut correction_q =
            rx_dc_calibration_correction(delta_q, self.low.q, self.threshold, shift);
        if self.request.stage == PhyRxDcCalibrationStage::Baseband {
            if power >= 45 {
                correction_i = 0;
                correction_q = 0;
            }
            // ROM clamps only the Wi-Fi-bank correction for gain indices
            // above one.  The shared-radio path deliberately retains the
            // full correction.
            if !self.request.shared_radio && self.request.gain_index > 1 {
                correction_i = saturate_signed_5(correction_i);
                correction_q = saturate_signed_5(correction_q);
            }
        }

        let converged = delta_i.wrapping_abs() <= self.threshold
            && delta_q.wrapping_abs() <= self.threshold
            && power < 46;
        if !converged {
            if delta_i.wrapping_abs() > self.threshold {
                self.current[0] =
                    saturate_9bit(i32::from(self.current[0]).wrapping_sub(correction_i));
            }
            if delta_q.wrapping_abs() > self.threshold {
                self.current[1] =
                    saturate_9bit(i32::from(self.current[1]).wrapping_sub(correction_q));
            }
        }

        let iterations = self.iteration + 1;
        if converged || iterations == self.max_iterations() {
            let configuration =
                if !converged && self.request.stage == PhyRxDcCalibrationStage::Baseband {
                    self.initial
                } else {
                    self.current
                };
            self.begin_cleanup(
                saved_field,
                Terminal::Complete(PhyRxDcCalibrationOutcome {
                    request: self.request,
                    configuration,
                    iterations,
                    converged,
                    readiness_activity_edges: self.readiness_activity_edges,
                }),
            );
        } else {
            self.iteration = iterations;
            self.step = Step::ForceI { saved_field };
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxDcCalibrationCompletion,
    ) -> Result<(), PhyRxDcCalibrationTransitionError> {
        match (self.step, completion) {
            (
                Step::MaskControl,
                PhyRxDcCalibrationCompletion::ControlMasked {
                    address: PHY_RX_DC_CONTROL_ADDRESS,
                    saved_field,
                },
            ) if saved_field & !PHY_RX_DC_CONTROL_FIELD_MASK == 0 => {
                self.step = Step::ReadPbus { saved_field };
            }
            (
                Step::ReadPbus { saved_field },
                PhyRxDcCalibrationCompletion::PbusRead {
                    selector: 1,
                    path: 2,
                    value,
                },
            ) => {
                self.population = (value as u8 & 0x3f).count_ones() as u8;
                self.threshold = match self.request.stage {
                    PhyRxDcCalibrationStage::Radio => i32::from(self.population.max(2) - 1),
                    PhyRxDcCalibrationStage::Baseband => {
                        if self.request.shared_radio {
                            6
                        } else {
                            1
                        }
                    }
                };
                self.step = Step::ForceI { saved_field };
            }
            (
                Step::ForceI { saved_field },
                PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction),
            ) if transaction == self.current_force(2) => {
                self.step = Step::ForceQ { saved_field };
            }
            (
                Step::ForceQ { saved_field },
                PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction),
            ) if transaction == self.current_force(3) => {
                self.step = match self.request.stage {
                    PhyRxDcCalibrationStage::Radio => Step::Delay {
                        saved_field,
                        high: false,
                    },
                    PhyRxDcCalibrationStage::Baseband => Step::ForceRadioLevel {
                        saved_field,
                        high: false,
                    },
                };
            }
            (
                Step::ForceRadioLevel { saved_field, high },
                PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction),
            ) if transaction == PhyPbusForceTest::new(1, 2, if high { 0x20 } else { 0 }) => {
                self.step = Step::Delay { saved_field, high };
            }
            (
                Step::Delay { saved_field, high },
                PhyRxDcCalibrationCompletion::DelayElapsed {
                    measurement,
                    micros: 10,
                },
            ) if measurement == self.measurement_identity(high) => {
                self.step = Step::Minimum {
                    saved_field,
                    high,
                    transition: PhyRxDcMinimumTransition::new(self.minimum_request(high)),
                };
            }
            (
                Step::Minimum {
                    saved_field,
                    high,
                    mut transition,
                },
                PhyRxDcCalibrationCompletion::Minimum(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxDcCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxDcMinimumAction::Complete(outcome) => {
                        self.accept_measurement(saved_field, high, outcome);
                    }
                    PhyRxDcMinimumAction::Failed(failure) => {
                        self.fail(saved_field, PhyRxDcCalibrationFailure::Minimum(failure));
                    }
                    _ => {
                        self.step = Step::Minimum {
                            saved_field,
                            high,
                            transition,
                        };
                    }
                }
            }
            (
                Step::ForceI { saved_field }
                | Step::ForceQ { saved_field }
                | Step::ForceRadioLevel { saved_field, .. },
                PhyRxDcCalibrationCompletion::PbusForceTimedOut(transaction),
            ) => self.fail(
                saved_field,
                PhyRxDcCalibrationFailure::PbusForceTimedOut(transaction),
            ),
            (
                Step::CleanupI {
                    saved_field,
                    terminal,
                },
                PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction)
                | PhyRxDcCalibrationCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == PhyPbusForceTest::new(
                    2,
                    self.path(),
                    self.cleanup_configuration(terminal)[0],
                ) =>
            {
                self.step = Step::CleanupQ {
                    saved_field,
                    terminal,
                };
            }
            (
                Step::CleanupQ {
                    saved_field,
                    terminal,
                },
                PhyRxDcCalibrationCompletion::PbusForceCompleted(transaction)
                | PhyRxDcCalibrationCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == PhyPbusForceTest::new(
                    3,
                    self.path(),
                    self.cleanup_configuration(terminal)[1],
                ) =>
            {
                self.step = Step::Restore {
                    saved_field,
                    terminal,
                };
            }
            (
                Step::Restore {
                    saved_field,
                    terminal,
                },
                PhyRxDcCalibrationCompletion::ControlRestored {
                    address: PHY_RX_DC_CONTROL_ADDRESS,
                    saved_field: completed_field,
                },
            ) if saved_field == completed_field => {
                self.step = match terminal {
                    Terminal::Complete(outcome) => Step::Complete(outcome),
                    Terminal::Failed(failure) => Step::Failed(failure),
                };
            }
            (Step::Complete(_), _) | (Step::Failed(_), _) => {
                return Err(PhyRxDcCalibrationTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxDcCalibrationTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

// Exact halfword tables at `.LANCHOR0 + 0x08` and `.LANCHOR0 + 0x18`
// in vendor `phy_rx_cal.o`.  The final shared entries select the extended
// 0x027f and 0x017f gain encodings; truncating them to 0x007f skips four
// calibration points later consumed by the RX gain-memory setup.
const WIFI_CALIBRATION_GAIN: [u16; 8] = [0x40, 0x41, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x7f];
const SHARED_CALIBRATION_GAIN: [u16; 11] = [
    0x40, 0x41, 0x42, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x027f, 0x017f, 0x007f,
];
const RX_ON_COUNT: u8 = 7;
const RX_OFF_COUNT: u8 = 3;
const RX_GAIN_I2C_ADDRESS: PhyI2cAddress = PhyI2cAddress::new_internal(0x67, 3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcBank {
    Wifi,
    Shared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainDcParameters {
    pub crystal_selector: u8,
    pub pbus_rx_path_value: u8,
    pub rx_saturation_detected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainDcOutcome {
    pub wifi_index_dc: [[u16; 2]; 8],
    pub wifi_dc_base: [u16; 2],
    /// Eleven calibrated entries beginning at `phy_param[0x1b4]`.
    pub shared_index_dc: [[u16; 2]; 11],
    /// Six fine RX-baseband corrections beginning at `phy_param[0x1e0]`.
    pub rxbb_dc_adjustments: [[u16; 2]; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcFailure {
    Rfpll(RfpllFrequencyFailure),
    Pbus {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    Calibration(PhyRxDcCalibrationFailure),
    Minimum(PhyRxDcMinimumFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcClock {
    Rx,
    Tx,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcDelayPhase {
    ReferenceLow,
    ReferenceHigh,
    PbusWorkMode,
    PbusWorkModePulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcAction {
    ConfigureRegisters {
        enabled: bool,
    },
    Rfpll(RfpllFrequencyAction),
    ConfigurePbusDebugMode,
    ForcePbus {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    ConfigureClock {
        clock: PhyRxGainDcClock,
        enabled: bool,
    },
    ReadPbus {
        selector: u8,
        path: u8,
    },
    I2c(MaskedI2cWriteAction),
    Calibration(PhyRxDcCalibrationAction),
    Minimum(PhyRxDcMinimumAction),
    ConfigurePbusWorkMode,
    DelayMicros {
        phase: PhyRxGainDcDelayPhase,
        micros: u32,
    },
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete(PhyRxGainDcOutcome),
    Failed(PhyRxGainDcFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcCompletion {
    RegistersConfigured {
        enabled: bool,
    },
    Rfpll(RfpllFrequencyCompletion),
    PbusDebugModeConfigured,
    PbusCompleted {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    PbusTimedOut {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    ClockConfigured {
        clock: PhyRxGainDcClock,
        enabled: bool,
    },
    PbusRead {
        selector: u8,
        path: u8,
        value: u32,
    },
    I2c(MaskedI2cWriteCompletion),
    Calibration(PhyRxDcCalibrationCompletion),
    Minimum(PhyRxDcMinimumCompletion),
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    DelayElapsed {
        phase: PhyRxGainDcDelayPhase,
        micros: u32,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DcTerminal {
    ContinueWifi,
    Complete,
    Failed(PhyRxGainDcFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DcStep {
    ConfigureRegisters,
    Rfpll(RfpllFrequencyTransition),
    Debug,
    WifiConfigureRegisters,
    WifiRfpll(RfpllFrequencyTransition),
    WifiDebug,
    RxOn {
        index: u8,
    },
    Clock {
        clock: PhyRxGainDcClock,
        enabled: bool,
    },
    SharedReadPbus,
    SharedForcePbus {
        value: u16,
    },
    SharedI2c(MaskedI2cWriteTransition),
    ReferenceSetup {
        bank: PhyRxGainDcBank,
        index: u8,
    },
    ReferenceDelay {
        bank: PhyRxGainDcBank,
        high: bool,
    },
    ReferenceMinimum {
        bank: PhyRxGainDcBank,
        high: bool,
        transition: PhyRxDcMinimumTransition,
    },
    ReferenceHigh {
        bank: PhyRxGainDcBank,
    },
    FineSetup {
        index: u8,
    },
    FineCode {
        index: u8,
    },
    FineCalibration {
        index: u8,
        transition: PhyRxDcCalibrationTransition,
    },
    SetupRadioI {
        bank: PhyRxGainDcBank,
        index: u8,
    },
    SetupRadioQ {
        bank: PhyRxGainDcBank,
        index: u8,
    },
    SetupBasebandI {
        bank: PhyRxGainDcBank,
        index: u8,
        value: [u16; 2],
    },
    SetupBasebandQ {
        bank: PhyRxGainDcBank,
        index: u8,
        value: [u16; 2],
    },
    SetGain {
        bank: PhyRxGainDcBank,
        index: u8,
        value: [u16; 2],
        transaction: u8,
    },
    SetSharedMixerDgain {
        index: u8,
    },
    CalibrateBaseband {
        bank: PhyRxGainDcBank,
        index: u8,
        transition: PhyRxDcCalibrationTransition,
    },
    CalibrateWifiRadio(PhyRxDcCalibrationTransition),
    SharedRestoreI2c(MaskedI2cWriteTransition),
    WifiRxOn {
        index: u8,
    },
    WifiClock {
        clock: PhyRxGainDcClock,
    },
    ClockOff(PhyRxGainDcClock, DcTerminal),
    RxOff {
        index: u8,
        terminal: DcTerminal,
    },
    WorkMode(DcTerminal),
    WorkModeDelay(DcTerminal),
    WorkModePulse(DcTerminal),
    WorkModePulseDelay(DcTerminal),
    WorkModePulseClear(DcTerminal),
    ClearRegisters(DcTerminal),
    Complete,
    Failed(PhyRxGainDcFailure),
}

const fn rx_on(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

const fn rx_off(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(1, 1, 0),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

const fn reference_setup(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(2, 1, 0x100),
        2 => PhyPbusForceTest::new(3, 1, 0x100),
        3 => PhyPbusForceTest::new(2, 2, 0x100),
        4 => PhyPbusForceTest::new(3, 2, 0x100),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

const fn fine_setup(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(2, 1, 0x100),
        2 => PhyPbusForceTest::new(3, 1, 0x100),
        3 => PhyPbusForceTest::new(2, 2, 0x100),
        _ => PhyPbusForceTest::new(3, 2, 0x100),
    }
}

const fn fine_code(index: u8) -> u16 {
    [0x00, 0x20, 0x30, 0x38, 0x3c, 0x3e][index as usize]
}

const fn bank_count(bank: PhyRxGainDcBank) -> u8 {
    match bank {
        PhyRxGainDcBank::Wifi => WIFI_CALIBRATION_GAIN.len() as u8,
        PhyRxGainDcBank::Shared => SHARED_CALIBRATION_GAIN.len() as u8,
    }
}

const fn gain(bank: PhyRxGainDcBank, index: u8) -> u16 {
    match bank {
        PhyRxGainDcBank::Wifi => WIFI_CALIBRATION_GAIN[index as usize],
        PhyRxGainDcBank::Shared => SHARED_CALIBRATION_GAIN[index as usize],
    }
}

/// Expand complete ROM `phy_pbus_set_rxgain`, size 92, into its three
/// independently completed PBus commands. The final command's former
/// `phy_param[0x002]` dependency is an explicit Rust-owned input.
const fn set_rx_gain_transaction(
    encoded_gain: u32,
    pbus_rx_path_value: u8,
    transaction: u8,
) -> PhyPbusForceTest {
    match transaction {
        0 => PhyPbusForceTest::new(
            1,
            2,
            (((encoded_gain << 6) & 0x1c0) | ((encoded_gain >> 4) & 0x3f)) as u16,
        ),
        1 => PhyPbusForceTest::new(
            0,
            1,
            (((encoded_gain >> 12) & 0x38) | ((encoded_gain >> 12) & 7) | 0x40) as u16,
        ),
        _ => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
    }
}

/// Exact stack-local lookup constructed by vendor `phy_bt_rx_mx_dgain`.
///
/// Indices 0..=8 select zero, index 9 selects four, and index 10 (plus the
/// out-of-range fallback) selects seven.
const fn shared_mixer_dgain(index: u8) -> u8 {
    if index >= 10 {
        7
    } else if index == 9 {
        4
    } else {
        0
    }
}

const fn shared_mixer_dgain_transaction(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
    PhyPbusForceTest::new(
        0,
        2,
        ((pbus_rx_path_value & 0xf8) | shared_mixer_dgain(index)) as u16,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainDcTransition {
    parameters: PhyRxGainDcParameters,
    step: DcStep,
    wifi_index_dc: [[u16; 2]; 8],
    wifi_dc_base: [u16; 2],
    shared_index_dc: [[u16; 2]; 11],
    rxbb_dc_adjustments: [[u16; 2]; 6],
    fine_current: [u16; 2],
    fine_base: [u16; 2],
    reference_low: crate::phy_dc_iq::PhyDcIqEstimate,
    reference_delta: [i16; 2],
}

impl PhyRxGainDcTransition {
    pub const fn new(parameters: PhyRxGainDcParameters) -> Self {
        Self {
            parameters,
            step: DcStep::ConfigureRegisters,
            wifi_index_dc: [[0; 2]; 8],
            wifi_dc_base: [0; 2],
            shared_index_dc: [[0; 2]; 11],
            rxbb_dc_adjustments: [[0; 2]; 6],
            fine_current: [0x100; 2],
            fine_base: [0; 2],
            reference_low: crate::phy_dc_iq::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            reference_delta: [0; 2],
        }
    }

    const fn outcome(self) -> PhyRxGainDcOutcome {
        PhyRxGainDcOutcome {
            wifi_index_dc: self.wifi_index_dc,
            wifi_dc_base: self.wifi_dc_base,
            shared_index_dc: self.shared_index_dc,
            rxbb_dc_adjustments: self.rxbb_dc_adjustments,
        }
    }

    const fn previous(self, bank: PhyRxGainDcBank, index: u8) -> [u16; 2] {
        if index == 0 {
            [0x100; 2]
        } else {
            // Vendor keeps a2 fixed at the beginning of the output bank and
            // reloads that first pair for every later gain. Its advancing s5
            // pointer is output-only; this is not a previous-index chain.
            match bank {
                PhyRxGainDcBank::Wifi => self.wifi_index_dc[0],
                PhyRxGainDcBank::Shared => self.shared_index_dc[0],
            }
        }
    }

    const fn baseband(self, bank: PhyRxGainDcBank, index: u8) -> [u16; 2] {
        match bank {
            PhyRxGainDcBank::Wifi if index != 0 => self.wifi_dc_base,
            _ => [0x100; 2],
        }
    }

    pub const fn action(self) -> PhyRxGainDcAction {
        match self.step {
            DcStep::ConfigureRegisters | DcStep::WifiConfigureRegisters => {
                PhyRxGainDcAction::ConfigureRegisters { enabled: true }
            }
            DcStep::Rfpll(transition) | DcStep::WifiRfpll(transition) => {
                PhyRxGainDcAction::Rfpll(transition.action())
            }
            DcStep::Debug | DcStep::WifiDebug => PhyRxGainDcAction::ConfigurePbusDebugMode,
            DcStep::RxOn { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Shared,
                transaction: rx_on(index, self.parameters.pbus_rx_path_value),
            },
            DcStep::Clock { clock, enabled } => {
                PhyRxGainDcAction::ConfigureClock { clock, enabled }
            }
            DcStep::ClockOff(clock, _) => PhyRxGainDcAction::ConfigureClock {
                clock,
                enabled: false,
            },
            DcStep::SharedReadPbus => PhyRxGainDcAction::ReadPbus {
                selector: 1,
                path: 1,
            },
            DcStep::SharedForcePbus { value } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Shared,
                transaction: PhyPbusForceTest::new(1, 1, value),
            },
            DcStep::SharedI2c(transition) | DcStep::SharedRestoreI2c(transition) => {
                PhyRxGainDcAction::I2c(transition.action())
            }
            DcStep::ReferenceSetup { bank, index } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: reference_setup(index),
            },
            DcStep::ReferenceDelay { high, .. } => PhyRxGainDcAction::DelayMicros {
                phase: if high {
                    PhyRxGainDcDelayPhase::ReferenceHigh
                } else {
                    PhyRxGainDcDelayPhase::ReferenceLow
                },
                micros: 10,
            },
            DcStep::ReferenceMinimum { transition, .. } => {
                PhyRxGainDcAction::Minimum(transition.action())
            }
            DcStep::ReferenceHigh { bank } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(1, 2, 0x20),
            },
            DcStep::FineSetup { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Wifi,
                transaction: fine_setup(index),
            },
            DcStep::WifiRxOn { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Wifi,
                transaction: rx_on(index, self.parameters.pbus_rx_path_value),
            },
            DcStep::WifiClock { clock } => PhyRxGainDcAction::ConfigureClock {
                clock,
                enabled: true,
            },
            DcStep::FineCode { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Wifi,
                transaction: PhyPbusForceTest::new(1, 2, fine_code(index)),
            },
            DcStep::FineCalibration { transition, .. } => {
                PhyRxGainDcAction::Calibration(transition.action())
            }
            DcStep::SetupRadioI { bank, .. } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(2, 1, 0x100),
            },
            DcStep::SetupRadioQ { bank, .. } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(3, 1, 0x100),
            },
            DcStep::SetupBasebandI { bank, value, .. } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(2, 2, value[0]),
            },
            DcStep::SetupBasebandQ { bank, value, .. } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: PhyPbusForceTest::new(3, 2, value[1]),
            },
            DcStep::SetGain {
                bank,
                index,
                transaction,
                ..
            } => PhyRxGainDcAction::ForcePbus {
                bank,
                transaction: set_rx_gain_transaction(
                    (gain(bank, index) as u32) << 12,
                    self.parameters.pbus_rx_path_value,
                    transaction,
                ),
            },
            DcStep::SetSharedMixerDgain { index } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Shared,
                transaction: shared_mixer_dgain_transaction(
                    index,
                    self.parameters.pbus_rx_path_value,
                ),
            },
            DcStep::CalibrateBaseband { transition, .. }
            | DcStep::CalibrateWifiRadio(transition) => {
                PhyRxGainDcAction::Calibration(transition.action())
            }
            DcStep::RxOff { index, .. } => PhyRxGainDcAction::ForcePbus {
                bank: PhyRxGainDcBank::Shared,
                transaction: rx_off(index),
            },
            DcStep::WorkMode(_) => PhyRxGainDcAction::ConfigurePbusWorkMode,
            DcStep::WorkModeDelay(_) => PhyRxGainDcAction::DelayMicros {
                phase: PhyRxGainDcDelayPhase::PbusWorkMode,
                micros: 1,
            },
            DcStep::WorkModePulse(_) => PhyRxGainDcAction::ConfigurePbusWorkModePulse,
            DcStep::WorkModePulseDelay(_) => PhyRxGainDcAction::DelayMicros {
                phase: PhyRxGainDcDelayPhase::PbusWorkModePulse,
                micros: 1,
            },
            DcStep::WorkModePulseClear(_) => PhyRxGainDcAction::ClearPbusWorkModePulse,
            DcStep::ClearRegisters(_) => PhyRxGainDcAction::ConfigureRegisters { enabled: false },
            DcStep::Complete => PhyRxGainDcAction::Complete(self.outcome()),
            DcStep::Failed(failure) => PhyRxGainDcAction::Failed(failure),
        }
    }

    fn cleanup(&mut self, terminal: DcTerminal) {
        self.step = match terminal {
            DcTerminal::ContinueWifi | DcTerminal::Complete | DcTerminal::Failed(_) => {
                DcStep::ClockOff(PhyRxGainDcClock::Rx, terminal)
            }
        };
    }

    fn fail(&mut self, failure: PhyRxGainDcFailure) {
        self.cleanup(DcTerminal::Failed(failure));
    }

    fn store_calibration(
        &mut self,
        bank: PhyRxGainDcBank,
        index: u8,
        outcome: PhyRxDcCalibrationOutcome,
    ) {
        match bank {
            PhyRxGainDcBank::Wifi => self.wifi_index_dc[index as usize] = outcome.configuration,
            PhyRxGainDcBank::Shared => self.shared_index_dc[index as usize] = outcome.configuration,
        }
        if bank == PhyRxGainDcBank::Wifi && index == 0 {
            self.step = DcStep::CalibrateWifiRadio(PhyRxDcCalibrationTransition::new(
                PhyRxDcCalibrationRequest {
                    shared_radio: false,
                    stage: PhyRxDcCalibrationStage::Radio,
                    control: 0x800,
                    initial: [0x100; 2],
                    reference_delta: [0; 2],
                    gain_index: 0,
                    rx_saturation_detected: self.parameters.rx_saturation_detected,
                },
            ));
        } else {
            self.next_gain(bank, index);
        }
    }

    fn next_gain(&mut self, bank: PhyRxGainDcBank, index: u8) {
        if index + 1 != bank_count(bank) {
            self.step = DcStep::SetupRadioI {
                bank,
                index: index + 1,
            };
        } else if bank == PhyRxGainDcBank::Shared {
            self.step = DcStep::SharedRestoreI2c(
                MaskedI2cWriteTransition::new(RX_GAIN_I2C_ADDRESS, 2, 2, 1).unwrap(),
            );
        } else {
            self.cleanup(DcTerminal::Complete);
        }
    }

    fn baseband_calibration_step(&self, bank: PhyRxGainDcBank, index: u8) -> DcStep {
        DcStep::CalibrateBaseband {
            bank,
            index,
            transition: PhyRxDcCalibrationTransition::new(PhyRxDcCalibrationRequest {
                shared_radio: bank == PhyRxGainDcBank::Shared,
                stage: PhyRxDcCalibrationStage::Baseband,
                control: 0x800,
                initial: self.previous(bank, index),
                reference_delta: self.reference_delta,
                gain_index: index,
                rx_saturation_detected: self.parameters.rx_saturation_detected,
            }),
        }
    }

    const fn reference_minimum_request(bank: PhyRxGainDcBank, high: bool) -> PhyRxDcMinimumRequest {
        PhyRxDcMinimumRequest {
            measurement: match (bank, high) {
                (PhyRxGainDcBank::Shared, false) => 0,
                (PhyRxGainDcBank::Shared, true) => 1,
                (PhyRxGainDcBank::Wifi, false) => 2,
                (PhyRxGainDcBank::Wifi, true) => 3,
            },
            control: 0x800,
            // `phy_rxdc_est_delta` passes zero in a3 to both
            // `phy_rxdc_est_min` calls.  Its a1 value is a separate,
            // unused-by-the-child argument and must not be confused with
            // the estimator mode.
            mode: 0,
            rx_saturation_detected: false,
        }
    }

    fn accept_reference(
        &mut self,
        bank: PhyRxGainDcBank,
        high: bool,
        outcome: PhyRxDcMinimumOutcome,
    ) {
        if !high {
            self.reference_low = outcome.estimate;
            self.step = DcStep::ReferenceHigh { bank };
            return;
        }
        self.reference_delta = [
            outcome.estimate.i.wrapping_sub(self.reference_low.i) as i16,
            outcome.estimate.q.wrapping_sub(self.reference_low.q) as i16,
        ];
        self.step = DcStep::SetupRadioI { bank, index: 0 };
    }

    fn after_work_mode(terminal: DcTerminal) -> DcStep {
        DcStep::ClearRegisters(terminal)
    }

    pub fn advance(
        &mut self,
        completion: PhyRxGainDcCompletion,
    ) -> Result<(), PhyRxGainDcTransitionError> {
        match (self.step, completion) {
            (
                DcStep::ConfigureRegisters,
                PhyRxGainDcCompletion::RegistersConfigured { enabled: true },
            ) => {
                self.step = DcStep::Rfpll(RfpllFrequencyTransition::new(RfpllFrequencyRequest {
                    crystal_selector: self.parameters.crystal_selector,
                    frequency_code: 0x9b4,
                    offset: 0,
                }));
            }
            (DcStep::Rfpll(mut transition), PhyRxGainDcCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                self.step = match transition.action() {
                    RfpllFrequencyAction::Complete(_) => DcStep::Debug,
                    RfpllFrequencyAction::Failed(failure) => {
                        self.fail(PhyRxGainDcFailure::Rfpll(failure));
                        return Ok(());
                    }
                    _ => DcStep::Rfpll(transition),
                };
            }
            (DcStep::Debug, PhyRxGainDcCompletion::PbusDebugModeConfigured) => {
                self.step = DcStep::RxOn { index: 0 };
            }
            (
                DcStep::WifiConfigureRegisters,
                PhyRxGainDcCompletion::RegistersConfigured { enabled: true },
            ) => {
                self.step =
                    DcStep::WifiRfpll(RfpllFrequencyTransition::new(RfpllFrequencyRequest {
                        crystal_selector: self.parameters.crystal_selector,
                        frequency_code: 0x9b4,
                        offset: 0,
                    }));
            }
            (DcStep::WifiRfpll(mut transition), PhyRxGainDcCompletion::Rfpll(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                self.step = match transition.action() {
                    RfpllFrequencyAction::Complete(_) => DcStep::WifiDebug,
                    RfpllFrequencyAction::Failed(failure) => {
                        self.fail(PhyRxGainDcFailure::Rfpll(failure));
                        return Ok(());
                    }
                    _ => DcStep::WifiRfpll(transition),
                };
            }
            (DcStep::WifiDebug, PhyRxGainDcCompletion::PbusDebugModeConfigured) => {
                self.step = DcStep::WifiRxOn { index: 0 };
            }
            (
                DcStep::RxOn { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction == rx_on(index, self.parameters.pbus_rx_path_value) => {
                self.step = if index + 1 == RX_ON_COUNT {
                    DcStep::Clock {
                        clock: PhyRxGainDcClock::Rx,
                        enabled: true,
                    }
                } else {
                    DcStep::RxOn { index: index + 1 }
                };
            }
            (
                DcStep::Clock {
                    clock: PhyRxGainDcClock::Rx,
                    enabled: true,
                },
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Rx,
                    enabled: true,
                },
            ) => {
                self.step = DcStep::Clock {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: true,
                };
            }
            (
                DcStep::Clock {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: true,
                },
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: true,
                },
            ) => self.step = DcStep::SharedReadPbus,
            (
                DcStep::SharedReadPbus,
                PhyRxGainDcCompletion::PbusRead {
                    selector: 1,
                    path: 1,
                    value,
                },
            ) => {
                self.step = DcStep::SharedForcePbus {
                    value: value as u16 | 2,
                }
            }
            (
                DcStep::SharedForcePbus { value },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction == PhyPbusForceTest::new(1, 1, value) => {
                self.step = DcStep::SharedI2c(
                    MaskedI2cWriteTransition::new(RX_GAIN_I2C_ADDRESS, 2, 2, 0).unwrap(),
                );
            }
            (DcStep::SharedI2c(mut transition), PhyRxGainDcCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                self.step = if transition.action() == MaskedI2cWriteAction::Complete {
                    DcStep::ReferenceSetup {
                        bank: PhyRxGainDcBank::Shared,
                        index: 0,
                    }
                } else {
                    DcStep::SharedI2c(transition)
                };
            }
            (
                DcStep::ReferenceSetup { bank, index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == reference_setup(index) => {
                self.step = if index == 5 {
                    DcStep::ReferenceDelay { bank, high: false }
                } else {
                    DcStep::ReferenceSetup {
                        bank,
                        index: index + 1,
                    }
                };
            }
            (
                DcStep::ReferenceDelay { bank, high },
                PhyRxGainDcCompletion::DelayElapsed { phase, micros: 10 },
            ) if phase
                == if high {
                    PhyRxGainDcDelayPhase::ReferenceHigh
                } else {
                    PhyRxGainDcDelayPhase::ReferenceLow
                } =>
            {
                self.step = DcStep::ReferenceMinimum {
                    bank,
                    high,
                    transition: PhyRxDcMinimumTransition::new(Self::reference_minimum_request(
                        bank, high,
                    )),
                };
            }
            (
                DcStep::ReferenceMinimum {
                    bank,
                    high,
                    mut transition,
                },
                PhyRxGainDcCompletion::Minimum(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxDcMinimumAction::Complete(outcome) => {
                        self.accept_reference(bank, high, outcome);
                    }
                    PhyRxDcMinimumAction::Failed(failure) => {
                        self.fail(PhyRxGainDcFailure::Minimum(failure));
                    }
                    _ => {
                        self.step = DcStep::ReferenceMinimum {
                            bank,
                            high,
                            transition,
                        };
                    }
                }
            }
            (
                DcStep::ReferenceHigh { bank },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(1, 2, 0x20) => {
                self.step = DcStep::ReferenceDelay { bank, high: true };
            }
            (
                DcStep::FineSetup { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                },
            ) if transaction == fine_setup(index) => {
                self.step = if index == 4 {
                    DcStep::FineCode { index: 0 }
                } else {
                    DcStep::FineSetup { index: index + 1 }
                };
            }
            (
                DcStep::FineCode { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                },
            ) if transaction == PhyPbusForceTest::new(1, 2, fine_code(index)) => {
                self.step = DcStep::FineCalibration {
                    index,
                    transition: PhyRxDcCalibrationTransition::new(PhyRxDcCalibrationRequest {
                        shared_radio: false,
                        stage: PhyRxDcCalibrationStage::Radio,
                        control: 0x800,
                        initial: self.fine_current,
                        reference_delta: [0; 2],
                        gain_index: 0,
                        rx_saturation_detected: self.parameters.rx_saturation_detected,
                    }),
                };
            }
            (
                DcStep::FineCalibration {
                    index,
                    mut transition,
                },
                PhyRxGainDcCompletion::Calibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxDcCalibrationAction::Complete(outcome) => {
                        self.fine_current = outcome.configuration;
                        if index == 0 {
                            self.fine_base = outcome.configuration;
                            self.rxbb_dc_adjustments[0] = [0; 2];
                        } else {
                            self.rxbb_dc_adjustments[index as usize] = [
                                outcome.configuration[0].wrapping_sub(self.fine_base[0]),
                                outcome.configuration[1].wrapping_sub(self.fine_base[1]),
                            ];
                        }
                        self.step = if index == 5 {
                            DcStep::ReferenceSetup {
                                bank: PhyRxGainDcBank::Wifi,
                                index: 0,
                            }
                        } else {
                            DcStep::FineCode { index: index + 1 }
                        };
                    }
                    PhyRxDcCalibrationAction::Failed(failure) => {
                        self.fail(PhyRxGainDcFailure::Calibration(failure));
                    }
                    _ => self.step = DcStep::FineCalibration { index, transition },
                }
            }
            (
                DcStep::SetupRadioI { bank, index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(2, 1, 0x100) => {
                self.step = DcStep::SetupRadioQ { bank, index };
            }
            (
                DcStep::SetupRadioQ { bank, index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(3, 1, 0x100) => {
                self.step = DcStep::SetupBasebandI {
                    bank,
                    index,
                    value: self.baseband(bank, index),
                };
            }
            (
                DcStep::SetupBasebandI { bank, index, value },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(2, 2, value[0]) => {
                self.step = DcStep::SetupBasebandQ { bank, index, value };
            }
            (
                DcStep::SetupBasebandQ { bank, index, value },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank && transaction == PhyPbusForceTest::new(3, 2, value[1]) => {
                self.step = DcStep::SetGain {
                    bank,
                    index,
                    value,
                    transaction: 0,
                };
            }
            (
                DcStep::SetGain {
                    bank,
                    index,
                    value,
                    transaction: transaction_index,
                },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank
                && transaction
                    == set_rx_gain_transaction(
                        u32::from(gain(bank, index)) << 12,
                        self.parameters.pbus_rx_path_value,
                        transaction_index,
                    ) =>
            {
                if transaction_index == 2 {
                    self.step = if bank == PhyRxGainDcBank::Shared {
                        DcStep::SetSharedMixerDgain { index }
                    } else {
                        self.baseband_calibration_step(bank, index)
                    };
                } else {
                    self.step = DcStep::SetGain {
                        bank,
                        index,
                        value,
                        transaction: transaction_index + 1,
                    };
                }
            }
            (
                DcStep::SetSharedMixerDgain { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction
                == shared_mixer_dgain_transaction(index, self.parameters.pbus_rx_path_value) =>
            {
                self.step = self.baseband_calibration_step(PhyRxGainDcBank::Shared, index);
            }
            (
                DcStep::CalibrateBaseband {
                    bank,
                    index,
                    mut transition,
                },
                PhyRxGainDcCompletion::Calibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxDcCalibrationAction::Complete(outcome) => {
                        self.store_calibration(bank, index, outcome);
                    }
                    PhyRxDcCalibrationAction::Failed(failure) => {
                        self.fail(PhyRxGainDcFailure::Calibration(failure));
                    }
                    _ => {
                        self.step = DcStep::CalibrateBaseband {
                            bank,
                            index,
                            transition,
                        };
                    }
                }
            }
            (
                DcStep::CalibrateWifiRadio(mut transition),
                PhyRxGainDcCompletion::Calibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxDcCalibrationAction::Complete(outcome) => {
                        self.wifi_dc_base = outcome.configuration;
                        self.next_gain(PhyRxGainDcBank::Wifi, 0);
                    }
                    PhyRxDcCalibrationAction::Failed(failure) => {
                        self.fail(PhyRxGainDcFailure::Calibration(failure));
                    }
                    _ => self.step = DcStep::CalibrateWifiRadio(transition),
                }
            }
            (DcStep::SharedRestoreI2c(mut transition), PhyRxGainDcCompletion::I2c(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainDcTransitionError::WrongCompletion)?;
                if transition.action() == MaskedI2cWriteAction::Complete {
                    self.cleanup(DcTerminal::ContinueWifi);
                } else {
                    self.step = DcStep::SharedRestoreI2c(transition);
                }
            }
            (
                DcStep::WifiRxOn { index },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                },
            ) if transaction == rx_on(index, self.parameters.pbus_rx_path_value) => {
                self.step = if index + 1 == RX_ON_COUNT {
                    DcStep::WifiClock {
                        clock: PhyRxGainDcClock::Rx,
                    }
                } else {
                    DcStep::WifiRxOn { index: index + 1 }
                };
            }
            (
                DcStep::WifiClock {
                    clock: PhyRxGainDcClock::Rx,
                },
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Rx,
                    enabled: true,
                },
            ) => {
                self.step = DcStep::WifiClock {
                    clock: PhyRxGainDcClock::Tx,
                };
            }
            (
                DcStep::WifiClock {
                    clock: PhyRxGainDcClock::Tx,
                },
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: true,
                },
            ) => self.step = DcStep::FineSetup { index: 0 },
            (
                DcStep::RxOn { index },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction == rx_on(index, self.parameters.pbus_rx_path_value) => {
                self.fail(PhyRxGainDcFailure::Pbus {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                });
            }
            (
                DcStep::ReferenceSetup { bank, .. }
                | DcStep::ReferenceHigh { bank }
                | DcStep::SetupRadioI { bank, .. }
                | DcStep::SetupRadioQ { bank, .. }
                | DcStep::SetupBasebandI { bank, .. }
                | DcStep::SetupBasebandQ { bank, .. }
                | DcStep::SetGain { bank, .. },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank => {
                self.fail(PhyRxGainDcFailure::Pbus {
                    bank: completed_bank,
                    transaction,
                });
            }
            (
                DcStep::SetSharedMixerDgain { .. },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) => {
                self.fail(PhyRxGainDcFailure::Pbus {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                });
            }
            (
                DcStep::WifiRxOn { .. } | DcStep::FineSetup { .. } | DcStep::FineCode { .. },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                },
            ) => {
                self.fail(PhyRxGainDcFailure::Pbus {
                    bank: PhyRxGainDcBank::Wifi,
                    transaction,
                });
            }
            (
                DcStep::SharedForcePbus { .. },
                PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) => self.fail(PhyRxGainDcFailure::Pbus {
                bank: PhyRxGainDcBank::Shared,
                transaction,
            }),
            (
                DcStep::ClockOff(PhyRxGainDcClock::Rx, terminal),
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Rx,
                    enabled: false,
                },
            ) => self.step = DcStep::ClockOff(PhyRxGainDcClock::Tx, terminal),
            (
                DcStep::ClockOff(PhyRxGainDcClock::Tx, terminal),
                PhyRxGainDcCompletion::ClockConfigured {
                    clock: PhyRxGainDcClock::Tx,
                    enabled: false,
                },
            ) => self.step = DcStep::RxOff { index: 0, terminal },
            (
                DcStep::RxOff { index, terminal },
                PhyRxGainDcCompletion::PbusCompleted {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                }
                | PhyRxGainDcCompletion::PbusTimedOut {
                    bank: PhyRxGainDcBank::Shared,
                    transaction,
                },
            ) if transaction == rx_off(index) => {
                self.step = if index + 1 == RX_OFF_COUNT {
                    DcStep::WorkMode(terminal)
                } else {
                    DcStep::RxOff {
                        index: index + 1,
                        terminal,
                    }
                };
            }
            (
                DcStep::WorkMode(terminal),
                PhyRxGainDcCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => self.step = Self::after_work_mode(terminal),
            (
                DcStep::WorkMode(terminal),
                PhyRxGainDcCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => self.step = DcStep::WorkModeDelay(terminal),
            (
                DcStep::WorkModeDelay(terminal),
                PhyRxGainDcCompletion::DelayElapsed {
                    phase: PhyRxGainDcDelayPhase::PbusWorkMode,
                    micros: 1,
                },
            ) => self.step = DcStep::WorkModePulse(terminal),
            (
                DcStep::WorkModePulse(terminal),
                PhyRxGainDcCompletion::PbusWorkModePulseConfigured,
            ) => self.step = DcStep::WorkModePulseDelay(terminal),
            (
                DcStep::WorkModePulseDelay(terminal),
                PhyRxGainDcCompletion::DelayElapsed {
                    phase: PhyRxGainDcDelayPhase::PbusWorkModePulse,
                    micros: 1,
                },
            ) => self.step = DcStep::WorkModePulseClear(terminal),
            (
                DcStep::WorkModePulseClear(terminal),
                PhyRxGainDcCompletion::PbusWorkModePulseCleared,
            ) => self.step = Self::after_work_mode(terminal),
            (
                DcStep::ClearRegisters(terminal),
                PhyRxGainDcCompletion::RegistersConfigured { enabled: false },
            ) => {
                self.step = match terminal {
                    DcTerminal::ContinueWifi => DcStep::WifiConfigureRegisters,
                    DcTerminal::Complete => DcStep::Complete,
                    DcTerminal::Failed(failure) => DcStep::Failed(failure),
                };
            }
            (DcStep::Complete, _) | (DcStep::Failed(_), _) => {
                return Err(PhyRxGainDcTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxGainDcTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainCalibrationBindingError {
    NotDirectMmio,
    UnsupportedAction,
    Pbus(crate::phy_pbus::PhyPbusHardwareBindingError),
}

/// Non-cloneable token for one direct MMIO edge of
/// `phy_pbus_rx_dco_cal_1step`.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationMmioBinding {
    action: PhyRxDcCalibrationAction,
}

impl PhyRxDcCalibrationMmioBinding {
    pub fn new(action: PhyRxDcCalibrationAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        match action {
            PhyRxDcCalibrationAction::MaskControl { .. }
            | PhyRxDcCalibrationAction::ReadPbus { .. }
            | PhyRxDcCalibrationAction::RestoreControl { .. } => Ok(Self { action }),
            _ => Err(PhyRxGainCalibrationBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyRxDcCalibrationAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> PhyRxDcCalibrationCompletion {
        match self.action {
            PhyRxDcCalibrationAction::MaskControl {
                address,
                clear_mask,
            } => PhyRxDcCalibrationCompletion::ControlMasked {
                address,
                saved_field: {
                    debug_assert_eq!(address, PHY_RX_DC_CONTROL_ADDRESS);
                    debug_assert_eq!(clear_mask, PHY_RX_DC_CONTROL_FIELD_MASK);
                    open_esp_radio_esp32s31_hal::phy_rx_dco::capture_and_clear_control(registers)
                },
            },
            PhyRxDcCalibrationAction::ReadPbus { selector, path } => {
                PhyRxDcCalibrationCompletion::PbusRead {
                    selector,
                    path,
                    value: {
                        let result = open_esp_radio_esp32s31_hal::pbus::read_result(
                            registers, selector, path,
                        );
                        debug_assert!(
                            result.is_some(),
                            "RX-DC calibration emitted an unrecovered PBus selector"
                        );
                        u32::from(result.unwrap_or(0))
                    },
                }
            }
            PhyRxDcCalibrationAction::RestoreControl {
                address,
                field_mask,
                saved_field,
            } => {
                debug_assert_eq!(address, PHY_RX_DC_CONTROL_ADDRESS);
                debug_assert_eq!(field_mask, PHY_RX_DC_CONTROL_FIELD_MASK);
                open_esp_radio_esp32s31_hal::phy_rx_dco::restore_control(registers, saved_field);
                PhyRxDcCalibrationCompletion::ControlRestored {
                    address,
                    saved_field,
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Non-cloneable token for one direct MMIO edge of the composed RX-DC gain
/// calibration. PBus force commands, I2C commands, timers, RFPLL and nested
/// estimator observations retain their separate executor bindings.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainDcMmioBinding {
    action: PhyRxGainDcAction,
}

impl PhyRxGainDcMmioBinding {
    pub fn new(action: PhyRxGainDcAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        match action {
            PhyRxGainDcAction::ConfigureRegisters { .. }
            | PhyRxGainDcAction::ConfigurePbusDebugMode
            | PhyRxGainDcAction::ConfigureClock { .. }
            | PhyRxGainDcAction::ReadPbus { .. }
            | PhyRxGainDcAction::ConfigurePbusWorkMode
            | PhyRxGainDcAction::ConfigurePbusWorkModePulse
            | PhyRxGainDcAction::ClearPbusWorkModePulse => Ok(Self { action }),
            _ => Err(PhyRxGainCalibrationBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyRxGainDcAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut open_esp_radio_esp32s31_hal::RadioRegisters,
    ) -> PhyRxGainDcCompletion {
        match self.action {
            PhyRxGainDcAction::ConfigureRegisters { enabled } => {
                crate::radio_hal::configure_phy_rx_gain_dc_registers(registers, enabled);
                PhyRxGainDcCompletion::RegistersConfigured { enabled }
            }
            PhyRxGainDcAction::ConfigurePbusDebugMode => {
                open_esp_radio_esp32s31_hal::pbus::configure_debug_mode(registers);
                PhyRxGainDcCompletion::PbusDebugModeConfigured
            }
            PhyRxGainDcAction::ConfigureClock { clock, enabled } => {
                match clock {
                    PhyRxGainDcClock::Rx => {
                        open_esp_radio_esp32s31_hal::pbus::configure_rx_clock(registers, enabled)
                    }
                    PhyRxGainDcClock::Tx => {
                        open_esp_radio_esp32s31_hal::pbus::configure_tx_clock(registers, enabled)
                    }
                }
                PhyRxGainDcCompletion::ClockConfigured { clock, enabled }
            }
            PhyRxGainDcAction::ReadPbus { selector, path } => PhyRxGainDcCompletion::PbusRead {
                selector,
                path,
                value: {
                    let result =
                        open_esp_radio_esp32s31_hal::pbus::read_result(registers, selector, path);
                    debug_assert!(
                        result.is_some(),
                        "RX-gain DC transition emitted an unrecovered PBus selector"
                    );
                    u32::from(result.unwrap_or(0))
                },
            },
            PhyRxGainDcAction::ConfigurePbusWorkMode => {
                PhyRxGainDcCompletion::PbusWorkModeConfigured {
                    settle_required: open_esp_radio_esp32s31_hal::pbus::configure_work_mode(
                        registers,
                    ),
                }
            }
            PhyRxGainDcAction::ConfigurePbusWorkModePulse => {
                open_esp_radio_esp32s31_hal::phy_agc::configure_pbus_work_mode_pulse(registers);
                PhyRxGainDcCompletion::PbusWorkModePulseConfigured
            }
            PhyRxGainDcAction::ClearPbusWorkModePulse => {
                open_esp_radio_esp32s31_hal::phy_agc::clear_pbus_work_mode_pulse(registers);
                PhyRxGainDcCompletion::PbusWorkModePulseCleared
            }
            _ => unreachable!(),
        }
    }
}

/// Linear owner of one PBus edge from `phy_pbus_rx_dco_cal_1step`.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::phy_pbus::PhyPbusHardwareBinding,
}

impl PhyRxDcCalibrationPbusBinding {
    pub fn new(action: PhyRxDcCalibrationAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        let PhyRxDcCalibrationAction::ForcePbus(transaction) = action else {
            return Err(PhyRxGainCalibrationBindingError::UnsupportedAction);
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
    ) -> Result<PhyRxDcCalibrationCompletion, PhyRxGainCalibrationBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyRxDcCalibrationCompletion::PbusForceCompleted)
            .map_err(PhyRxGainCalibrationBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxDcCalibrationCompletion {
        PhyRxDcCalibrationCompletion::PbusForceTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationTimerBinding {
    measurement: u8,
    micros: u32,
}

impl PhyRxDcCalibrationTimerBinding {
    pub fn new(action: PhyRxDcCalibrationAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        match action {
            PhyRxDcCalibrationAction::DelayMicros {
                measurement,
                micros,
            } => Ok(Self {
                measurement,
                micros,
            }),
            _ => Err(PhyRxGainCalibrationBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxDcCalibrationCompletion {
        PhyRxDcCalibrationCompletion::DelayElapsed {
            measurement: self.measurement,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationExternalBinding {
    Mmio(PhyRxDcCalibrationMmioBinding),
    Pbus(PhyRxDcCalibrationPbusBinding),
    Timer(PhyRxDcCalibrationTimerBinding),
    Minimum(crate::phy_rx_dco::PhyRxDcMinimumExternalBinding),
}

impl PhyRxDcCalibrationExternalBinding {
    pub fn lower(
        action: PhyRxDcCalibrationAction,
    ) -> Result<Self, PhyRxGainCalibrationBindingError> {
        if let Ok(binding) = PhyRxDcCalibrationMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxDcCalibrationPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxDcCalibrationTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        if let PhyRxDcCalibrationAction::Minimum(action) = action {
            return crate::phy_rx_dco::PhyRxDcMinimumExternalBinding::lower(action)
                .map(Self::Minimum)
                .map_err(|_| PhyRxGainCalibrationBindingError::UnsupportedAction);
        }
        Err(PhyRxGainCalibrationBindingError::UnsupportedAction)
    }
}

/// Linear owner of one bank-qualified PBus edge in the composed RX-gain
/// calibration. The bank is retained in both normal and timeout completions.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainDcPbusBinding {
    bank: PhyRxGainDcBank,
    transaction: PhyPbusForceTest,
    hardware: crate::phy_pbus::PhyPbusHardwareBinding,
}

impl PhyRxGainDcPbusBinding {
    pub fn new(action: PhyRxGainDcAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        let PhyRxGainDcAction::ForcePbus { bank, transaction } = action else {
            return Err(PhyRxGainCalibrationBindingError::UnsupportedAction);
        };
        Ok(Self {
            bank,
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
    ) -> Result<PhyRxGainDcCompletion, PhyRxGainCalibrationBindingError> {
        let bank = self.bank;
        self.hardware
            .into_transaction()
            .map(|transaction| PhyRxGainDcCompletion::PbusCompleted { bank, transaction })
            .map_err(PhyRxGainCalibrationBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxGainDcCompletion {
        PhyRxGainDcCompletion::PbusTimedOut {
            bank: self.bank,
            transaction: self.transaction,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainDcTimerBinding {
    phase: PhyRxGainDcDelayPhase,
    micros: u32,
}

impl PhyRxGainDcTimerBinding {
    pub fn new(action: PhyRxGainDcAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        match action {
            PhyRxGainDcAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyRxGainCalibrationBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxGainDcCompletion {
        PhyRxGainDcCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Exhaustive lowering of every non-terminal composed RX-gain DC action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxGainDcExternalBinding {
    Mmio(PhyRxGainDcMmioBinding),
    Rfpll(crate::phy_rfpll::RfpllFrequencyExternalBinding),
    Pbus(PhyRxGainDcPbusBinding),
    I2c(crate::phy_i2c::MaskedI2cWriteBinding),
    Calibration(PhyRxDcCalibrationExternalBinding),
    Minimum(crate::phy_rx_dco::PhyRxDcMinimumExternalBinding),
    Timer(PhyRxGainDcTimerBinding),
}

impl PhyRxGainDcExternalBinding {
    pub fn lower(action: PhyRxGainDcAction) -> Result<Self, PhyRxGainCalibrationBindingError> {
        if let Ok(binding) = PhyRxGainDcMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxGainDcPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxGainDcTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        match action {
            PhyRxGainDcAction::Rfpll(action) => {
                crate::phy_rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyRxGainCalibrationBindingError::UnsupportedAction)
            }
            PhyRxGainDcAction::I2c(action) => crate::phy_i2c::MaskedI2cWriteBinding::new(action)
                .map(Self::I2c)
                .map_err(|_| PhyRxGainCalibrationBindingError::UnsupportedAction),
            PhyRxGainDcAction::Calibration(action) => {
                PhyRxDcCalibrationExternalBinding::lower(action).map(Self::Calibration)
            }
            PhyRxGainDcAction::Minimum(action) => {
                crate::phy_rx_dco::PhyRxDcMinimumExternalBinding::lower(action)
                    .map(Self::Minimum)
                    .map_err(|_| PhyRxGainCalibrationBindingError::UnsupportedAction)
            }
            _ => Err(PhyRxGainCalibrationBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy_dc_iq::{PhyDcIqEstimate, PhyDcIqEstimateOutcome};

    const RADIO: PhyRxDcCalibrationRequest = PhyRxDcCalibrationRequest {
        shared_radio: false,
        stage: PhyRxDcCalibrationStage::Radio,
        control: 0x800,
        initial: [0x100, 0x100],
        reference_delta: [0, 0],
        gain_index: 0,
        rx_saturation_detected: false,
    };

    fn outcome(
        request: PhyRxDcMinimumRequest,
        i: i32,
        q: i32,
        power: i32,
    ) -> PhyRxDcMinimumOutcome {
        PhyRxDcMinimumOutcome {
            request,
            estimate: PhyDcIqEstimate { i, q, power },
            attempts: 1,
            readiness_activity_edges: 0,
        }
    }

    #[test]
    fn correction_matches_sign_and_large_low_fallbacks() {
        assert_eq!(rx_dc_calibration_correction(0, 0, 2, 0), 0);
        assert_eq!(rx_dc_calibration_correction(1, 0, 2, 0), 1);
        assert_eq!(rx_dc_calibration_correction(-1, 0, 2, 0), -1);
        assert_eq!(rx_dc_calibration_correction(0, 64, 2, 3), 8);
        assert_eq!(rx_dc_calibration_correction(24, 0, 2, 3), 3);
    }

    #[test]
    fn gain_tables_and_reference_mode_match_vendor_rodata_and_calls() {
        assert_eq!(
            WIFI_CALIBRATION_GAIN,
            [0x40, 0x41, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x7f]
        );
        assert_eq!(
            SHARED_CALIBRATION_GAIN,
            [
                0x40, 0x41, 0x42, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x027f, 0x017f, 0x007f,
            ]
        );
        assert_eq!(bank_count(PhyRxGainDcBank::Wifi), 8);
        assert_eq!(bank_count(PhyRxGainDcBank::Shared), 11);
        assert_eq!(shared_mixer_dgain(0), 0);
        assert_eq!(shared_mixer_dgain(8), 0);
        assert_eq!(shared_mixer_dgain(9), 4);
        assert_eq!(shared_mixer_dgain(10), 7);
        assert_eq!(
            PhyRxGainDcTransition::reference_minimum_request(PhyRxGainDcBank::Shared, false).mode,
            0
        );
    }

    #[test]
    fn shared_gain_adds_vendor_mixer_dgain_command_before_calibration() {
        let mut transition = PhyRxGainDcTransition::new(PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0xbf,
            rx_saturation_detected: false,
        });
        transition.step = DcStep::SetGain {
            bank: PhyRxGainDcBank::Shared,
            index: 9,
            value: [0x100; 2],
            transaction: 2,
        };

        let PhyRxGainDcAction::ForcePbus { bank, transaction } = transition.action() else {
            panic!("missing final generic set-rx-gain command");
        };
        assert_eq!(bank, PhyRxGainDcBank::Shared);
        assert_eq!(transaction, PhyPbusForceTest::new(0, 2, 0xbf));
        transition
            .advance(PhyRxGainDcCompletion::PbusCompleted { bank, transaction })
            .unwrap();

        let PhyRxGainDcAction::ForcePbus { bank, transaction } = transition.action() else {
            panic!("missing shared mixer-dgain command");
        };
        assert_eq!(bank, PhyRxGainDcBank::Shared);
        assert_eq!(transaction, PhyPbusForceTest::new(0, 2, 0xbc));
        transition
            .advance(PhyRxGainDcCompletion::PbusCompleted { bank, transaction })
            .unwrap();
        assert!(matches!(
            transition.step,
            DcStep::CalibrateBaseband {
                bank: PhyRxGainDcBank::Shared,
                index: 9,
                ..
            }
        ));
    }

    #[test]
    fn later_gain_searches_restart_from_the_first_bank_pair() {
        let mut transition = PhyRxGainDcTransition::new(PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0,
            rx_saturation_detected: false,
        });
        transition.wifi_index_dc[0] = [0x101, 0x102];
        transition.wifi_index_dc[1] = [0x111, 0x112];
        transition.shared_index_dc[0] = [0x121, 0x122];
        transition.shared_index_dc[1] = [0x131, 0x132];

        assert_eq!(
            transition.previous(PhyRxGainDcBank::Wifi, 2),
            [0x101, 0x102]
        );
        assert_eq!(
            transition.previous(PhyRxGainDcBank::Shared, 2),
            [0x121, 0x122]
        );
    }

    #[test]
    fn converged_radio_measurement_restores_field_after_final_pbus_values() {
        let mut transition = PhyRxDcCalibrationTransition::new(RADIO);
        transition
            .advance(PhyRxDcCalibrationCompletion::ControlMasked {
                address: PHY_RX_DC_CONTROL_ADDRESS,
                saved_field: 0x0080_0000,
            })
            .unwrap();
        transition
            .advance(PhyRxDcCalibrationCompletion::PbusRead {
                selector: 1,
                path: 2,
                value: 3,
            })
            .unwrap();
        for selector in [2, 3] {
            let PhyRxDcCalibrationAction::ForcePbus(transaction) = transition.action() else {
                panic!("missing setup PBus action");
            };
            assert_eq!(transaction.selector(), selector);
            transition
                .advance(PhyRxDcCalibrationCompletion::PbusForceCompleted(
                    transaction,
                ))
                .unwrap();
        }
        let PhyRxDcCalibrationAction::DelayMicros {
            measurement,
            micros,
        } = transition.action()
        else {
            panic!("missing timer action");
        };
        transition
            .advance(PhyRxDcCalibrationCompletion::DelayElapsed {
                measurement,
                micros,
            })
            .unwrap();
        let request = transition.minimum_request(false);
        transition.accept_measurement(0x0080_0000, false, outcome(request, 1, -1, 20));
        for selector in [2, 3] {
            let PhyRxDcCalibrationAction::ForcePbus(transaction) = transition.action() else {
                panic!("missing cleanup PBus action");
            };
            assert_eq!(transaction.selector(), selector);
            transition
                .advance(PhyRxDcCalibrationCompletion::PbusForceCompleted(
                    transaction,
                ))
                .unwrap();
        }
        assert!(matches!(
            transition.action(),
            PhyRxDcCalibrationAction::RestoreControl {
                saved_field: 0x0080_0000,
                ..
            }
        ));
        transition
            .advance(PhyRxDcCalibrationCompletion::ControlRestored {
                address: PHY_RX_DC_CONTROL_ADDRESS,
                saved_field: 0x0080_0000,
            })
            .unwrap();
        let PhyRxDcCalibrationAction::Complete(outcome) = transition.action() else {
            panic!("calibration did not complete");
        };
        assert!(outcome.converged);
        assert_eq!(outcome.iterations, 1);
        assert_eq!(outcome.configuration, RADIO.initial);
    }

    #[test]
    fn child_failure_uses_initial_configuration_for_cleanup() {
        let mut transition = PhyRxDcCalibrationTransition::new(RADIO);
        transition.step = Step::Minimum {
            saved_field: 0,
            high: false,
            transition: PhyRxDcMinimumTransition::new(transition.minimum_request(false)),
        };
        transition.fail(
            0,
            PhyRxDcCalibrationFailure::Minimum(PhyRxDcMinimumFailure::DcIq(
                crate::phy_dc_iq::PhyDcIqFailure::ReadinessTimedOut {
                    request: crate::phy_dc_iq::PhyDcIqEstimateRequest {
                        iteration: 0,
                        chain: 1,
                        control: RADIO.control,
                        mode: 0,
                    },
                    readiness_activity_edges: 0,
                },
            )),
        );
        assert_eq!(
            transition.action(),
            PhyRxDcCalibrationAction::ForcePbus(PhyPbusForceTest::new(2, 2, RADIO.initial[0]))
        );
    }

    #[test]
    fn minimum_outcome_shape_is_owned_not_pointer_backed() {
        let request = PhyRxDcMinimumRequest {
            measurement: 0,
            control: RADIO.control,
            mode: 0,
            rx_saturation_detected: false,
        };
        let child = PhyDcIqEstimateOutcome {
            request: crate::phy_dc_iq::PhyDcIqEstimateRequest {
                iteration: 0,
                chain: 1,
                control: RADIO.control,
                mode: 0,
            },
            estimate: PhyDcIqEstimate {
                i: 1,
                q: 2,
                power: 3,
            },
            readiness_activity_edges: 0,
        };
        assert_eq!(request.measurement, child.request.iteration);
    }
}
