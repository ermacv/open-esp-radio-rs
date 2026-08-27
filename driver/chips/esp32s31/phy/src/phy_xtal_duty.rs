//! Event-driven ESP32-S31 crystal-duty search.
//!
//! Reference: complete `libphy.a[phy_rx_cal.o]::phy_xtal_duty_cal`, size
//! `0x392`. The pinned cold caller always passes `debug = 0`, so both vendor
//! `phy_printf` branches are dead and are intentionally absent here.

use crate::{
    phy_i2c::PhyI2cAddress,
    phy_pbus::PhyPbusForceTest,
    phy_rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyFailure,
        RfpllFrequencyRequest, RfpllFrequencyTransition,
    },
    phy_rx_dco::{
        PhyRxDcoAction, PhyRxDcoCompletion, PhyRxDcoFailure, PhyRxDcoOutcome, PhyRxDcoRequest,
        PhyRxDcoTransition,
    },
    phy_signal_power::{
        PhySignalPowerAction, PhySignalPowerCompletion, PhySignalPowerFailure,
        PhySignalPowerRequest, PhySignalPowerTransition,
    },
};

const FIRST_CANDIDATE: u8 = 0x20;
const LAST_CANDIDATE: u8 = 0x3e;
const INITIAL_SAMPLE_COUNT: u8 = 4;
const SIGNAL_POWER_SHIFT: u8 = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutySampleKind {
    Initial(u8),
    FirstReplacement(u8),
    SecondReplacement(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutySearchAction {
    WriteCandidate {
        address: PhyI2cAddress,
        candidate: u8,
    },
    DelayMicros {
        candidate: u8,
        micros: u32,
    },
    SignalPower(PhySignalPowerAction),
    Complete(XtalDutySearchOutcome),
    Failed(PhySignalPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutySearchCompletion {
    CandidateWritten {
        address: PhyI2cAddress,
        candidate: u8,
    },
    DelayElapsed {
        candidate: u8,
    },
    SignalPower(PhySignalPowerCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutySearchTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutySearchOutcome {
    pub best_candidate: u8,
    pub best_filtered_power: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutySearchStep {
    WriteCandidate {
        candidate: u8,
    },
    Delay {
        candidate: u8,
    },
    InitialSamples {
        candidate: u8,
        samples: [i64; INITIAL_SAMPLE_COUNT as usize],
        count: u8,
    },
    Review {
        candidate: u8,
        samples: [i64; INITIAL_SAMPLE_COUNT as usize],
        lower: i64,
        upper: i64,
        index: u8,
        filtered_sum: i64,
    },
    FirstReplacement {
        candidate: u8,
        samples: [i64; INITIAL_SAMPLE_COUNT as usize],
        lower: i64,
        upper: i64,
        index: u8,
        filtered_sum: i64,
    },
    SecondReplacement {
        candidate: u8,
        samples: [i64; INITIAL_SAMPLE_COUNT as usize],
        lower: i64,
        upper: i64,
        index: u8,
        filtered_sum: i64,
    },
    Complete(XtalDutySearchOutcome),
    Failed(PhySignalPowerFailure),
}

/// Fixed-size translation of the vendor crystal-duty candidate search.
///
/// The transition owns all samples. It can request at most six measurements
/// for each of the 31 candidates and cannot advance from `poll`: every
/// hardware measurement and 20-microsecond interval requires an explicit
/// external completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutySearchTransition {
    step: XtalDutySearchStep,
    signal_power: Option<PhySignalPowerTransition>,
    best_candidate: u8,
    best_filtered_power: i64,
    has_best: bool,
}

impl XtalDutySearchTransition {
    const DUTY_ADDRESS: PhyI2cAddress = crate::phy_i2c::analog_registers::XTAL_DUTY_CANDIDATE;

    pub const fn new() -> Self {
        Self {
            step: XtalDutySearchStep::WriteCandidate {
                candidate: FIRST_CANDIDATE,
            },
            signal_power: None,
            best_candidate: FIRST_CANDIDATE,
            best_filtered_power: 0,
            has_best: false,
        }
    }

    pub const fn action(self) -> XtalDutySearchAction {
        if let Some(transition) = self.signal_power {
            return XtalDutySearchAction::SignalPower(transition.action());
        }
        match self.step {
            XtalDutySearchStep::WriteCandidate { candidate } => {
                XtalDutySearchAction::WriteCandidate {
                    address: Self::DUTY_ADDRESS,
                    candidate,
                }
            }
            XtalDutySearchStep::Delay { candidate } => XtalDutySearchAction::DelayMicros {
                candidate,
                micros: 20,
            },
            XtalDutySearchStep::InitialSamples { .. }
            | XtalDutySearchStep::Review { .. }
            | XtalDutySearchStep::FirstReplacement { .. }
            | XtalDutySearchStep::SecondReplacement { .. } => {
                panic!()
            }
            XtalDutySearchStep::Complete(outcome) => XtalDutySearchAction::Complete(outcome),
            XtalDutySearchStep::Failed(failure) => XtalDutySearchAction::Failed(failure),
        }
    }

    const fn measurement_request(candidate: u8, kind: XtalDutySampleKind) -> PhySignalPowerRequest {
        let kind = match kind {
            XtalDutySampleKind::Initial(index) => index,
            XtalDutySampleKind::FirstReplacement(index) => 4 + index,
            XtalDutySampleKind::SecondReplacement(index) => 8 + index,
        };
        PhySignalPowerRequest {
            measurement: ((candidate as u16) << 4) | kind as u16,
            shift: SIGNAL_POWER_SHIFT,
        }
    }

    fn arm_signal_power(&mut self) {
        if self.signal_power.is_some() {
            return;
        }
        let request = match self.step {
            XtalDutySearchStep::InitialSamples {
                candidate, count, ..
            } => Self::measurement_request(candidate, XtalDutySampleKind::Initial(count)),
            XtalDutySearchStep::FirstReplacement {
                candidate, index, ..
            } => Self::measurement_request(candidate, XtalDutySampleKind::FirstReplacement(index)),
            XtalDutySearchStep::SecondReplacement {
                candidate, index, ..
            } => Self::measurement_request(candidate, XtalDutySampleKind::SecondReplacement(index)),
            _ => return,
        };
        self.signal_power = Some(PhySignalPowerTransition::new(request));
    }

    fn outlier(value: i64, lower: i64, upper: i64) -> bool {
        value < lower || value > upper
    }

    fn finish_candidate(&mut self, candidate: u8, filtered_sum: i64) {
        let filtered_power = filtered_sum / i64::from(INITIAL_SAMPLE_COUNT);
        if !self.has_best || filtered_power < self.best_filtered_power {
            self.has_best = true;
            self.best_candidate = candidate;
            self.best_filtered_power = filtered_power;
        }
        self.step = if candidate == LAST_CANDIDATE {
            XtalDutySearchStep::Complete(XtalDutySearchOutcome {
                best_candidate: self.best_candidate,
                best_filtered_power: self.best_filtered_power,
            })
        } else {
            XtalDutySearchStep::WriteCandidate {
                candidate: candidate + 1,
            }
        };
    }

    fn normalize_review(&mut self) {
        loop {
            let XtalDutySearchStep::Review {
                candidate,
                samples,
                lower,
                upper,
                index,
                filtered_sum,
            } = self.step
            else {
                return;
            };
            if index == INITIAL_SAMPLE_COUNT {
                self.finish_candidate(candidate, filtered_sum);
                return;
            }
            let sample = samples[index as usize];
            if Self::outlier(sample, lower, upper) {
                self.step = XtalDutySearchStep::FirstReplacement {
                    candidate,
                    samples,
                    lower,
                    upper,
                    index,
                    filtered_sum,
                };
                return;
            }
            self.step = XtalDutySearchStep::Review {
                candidate,
                samples,
                lower,
                upper,
                index: index + 1,
                filtered_sum: filtered_sum.wrapping_add(sample),
            };
        }
    }

    fn accept_signal_power(&mut self, value: i64) -> Result<(), XtalDutySearchTransitionError> {
        self.step = match self.step {
            XtalDutySearchStep::InitialSamples {
                candidate,
                mut samples,
                count,
            } => {
                samples[count as usize] = value;
                if count + 1 != INITIAL_SAMPLE_COUNT {
                    XtalDutySearchStep::InitialSamples {
                        candidate,
                        samples,
                        count: count + 1,
                    }
                } else {
                    let sum = samples
                        .into_iter()
                        .fold(0_i64, |sum, sample| sum.wrapping_add(sample));
                    let mean = sum / i64::from(INITIAL_SAMPLE_COUNT);
                    XtalDutySearchStep::Review {
                        candidate,
                        samples,
                        lower: mean.wrapping_mul(2) / 3,
                        upper: mean.wrapping_mul(3) / 2,
                        index: 0,
                        filtered_sum: 0,
                    }
                }
            }
            XtalDutySearchStep::FirstReplacement {
                candidate,
                samples,
                lower,
                upper,
                index,
                filtered_sum,
            } => {
                if Self::outlier(value, lower, upper) {
                    XtalDutySearchStep::SecondReplacement {
                        candidate,
                        samples,
                        lower,
                        upper,
                        index,
                        filtered_sum,
                    }
                } else {
                    XtalDutySearchStep::Review {
                        candidate,
                        samples,
                        lower,
                        upper,
                        index: index + 1,
                        filtered_sum: filtered_sum.wrapping_add(value),
                    }
                }
            }
            XtalDutySearchStep::SecondReplacement {
                candidate,
                samples,
                lower,
                upper,
                index,
                filtered_sum,
            } => XtalDutySearchStep::Review {
                candidate,
                samples,
                lower,
                upper,
                index: index + 1,
                filtered_sum: filtered_sum.wrapping_add(value),
            },
            _ => return Err(XtalDutySearchTransitionError::WrongCompletion),
        };
        self.normalize_review();
        self.arm_signal_power();
        Ok(())
    }

    pub fn advance(
        &mut self,
        completion: XtalDutySearchCompletion,
    ) -> Result<(), XtalDutySearchTransitionError> {
        if let Some(mut transition) = self.signal_power {
            let XtalDutySearchCompletion::SignalPower(completion) = completion else {
                return Err(XtalDutySearchTransitionError::WrongCompletion);
            };
            transition
                .advance(completion)
                .map_err(|_| XtalDutySearchTransitionError::WrongCompletion)?;
            return match transition.action() {
                PhySignalPowerAction::Complete(outcome) => {
                    self.signal_power = None;
                    self.accept_signal_power(outcome.value)
                }
                PhySignalPowerAction::Failed(failure) => {
                    self.signal_power = None;
                    self.step = XtalDutySearchStep::Failed(failure);
                    Ok(())
                }
                _ => {
                    self.signal_power = Some(transition);
                    Ok(())
                }
            };
        }
        self.step = match (self.step, completion) {
            (
                XtalDutySearchStep::WriteCandidate { candidate },
                XtalDutySearchCompletion::CandidateWritten {
                    address,
                    candidate: completed,
                },
            ) if address == Self::DUTY_ADDRESS && candidate == completed => {
                XtalDutySearchStep::Delay { candidate }
            }
            (
                XtalDutySearchStep::Delay { candidate },
                XtalDutySearchCompletion::DelayElapsed {
                    candidate: completed,
                },
            ) if candidate == completed => XtalDutySearchStep::InitialSamples {
                candidate,
                samples: [0; INITIAL_SAMPLE_COUNT as usize],
                count: 0,
            },
            (XtalDutySearchStep::Complete(_), _) | (XtalDutySearchStep::Failed(_), _) => {
                return Err(XtalDutySearchTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutySearchTransitionError::WrongCompletion),
        };
        self.normalize_review();
        self.arm_signal_power();
        Ok(())
    }
}

impl Default for XtalDutySearchTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyCalibrationParameters {
    /// `phy_param[0x4f]`, passed to the RFPLL frequency calculation.
    pub rf_frequency_offset_base: u8,
    /// Byte two of the parameter image published through the rev0 ROM
    /// `phy_param_rom` pointer cell at `0x2f07_fc40`.
    ///
    /// ROM `phy_pbus_xpd_rx_on` forwards this value to PBus selector zero,
    /// path two. Its electrical meaning is not yet evidenced, so the name
    /// deliberately describes the observed use rather than guessing.
    pub pbus_rx_path_value: u8,
}

const fn prepare_pbus_transaction(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        6 => PhyPbusForceTest::new(1, 2, 0xf0),
        7 => PhyPbusForceTest::new(0, 1, 0x43),
        8 => PhyPbusForceTest::new(1, 1, 0x38),
        _ => PhyPbusForceTest::new(1, 1, 0x189),
    }
}

const fn restore_pbus_transaction(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(1, 1, 0),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyHardwareFailure {
    Rfpll(RfpllFrequencyFailure),
    PbusForceTestTimedOut(PhyPbusForceTest),
    RxDco(PhyRxDcoFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPrepareAction {
    /// Complete Rust-owned RFPLL I2C/timer transition.
    Rfpll(RfpllFrequencyAction),
    /// Complete Rust MMIO replacement for
    /// `phy_start_tx_tone_step_new(1, 0x80, 0, 0, 0, 0)`, including both
    /// former `g_phyFuns + 0x30` callback invocations.
    ConfigureCalibrationTone {
        enabled: bool,
        selector: u8,
        step: u8,
    },
    ConfigureRxClock {
        enabled: bool,
    },
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigurePbusDebugMode,
    ForcePbus(PhyPbusForceTest),
    /// Retain the field inside PAC and clear it in the same serialized
    /// radio-owner operation.
    PrepareRxDcoControlRestore,
    /// Complete Rust-owned RX-DCO and IQ-estimator graph.
    RxDco(PhyRxDcoAction),
    RestoreRxDcoControl,
    Complete(PhyRxDcoOutcome),
    Failed(XtalDutyHardwareFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPrepareCompletion {
    Rfpll(RfpllFrequencyCompletion),
    CalibrationToneConfigured {
        enabled: bool,
        selector: u8,
        step: u8,
    },
    RxClockConfigured {
        enabled: bool,
    },
    TxClockConfigured {
        enabled: bool,
    },
    PbusDebugModeConfigured,
    PbusForceCompleted(PhyPbusForceTest),
    PbusForceTimedOut(PhyPbusForceTest),
    RxDcoControlRestorePrepared,
    RxDco(PhyRxDcoCompletion),
    RxDcoControlRestored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPrepareTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutyPrepareStep {
    Rfpll(RfpllFrequencyTransition),
    StartTone,
    EnableRxClock,
    EnableTxClock,
    PbusDebugMode,
    PbusForce(u8),
    PrepareRxDcoControlRestore,
    RxDco { transition: PhyRxDcoTransition },
    RestoreRxDcoControl { outcome: PhyRxDcoOutcome },
    RestoreRxDcoControlAfterFailure { failure: PhyRxDcoFailure },
    Complete(PhyRxDcoOutcome),
    Failed(XtalDutyHardwareFailure),
}

/// Exact finite preparation order before the crystal-duty candidate search.
///
/// PBus commands are individual externally completed operations. Nothing in
/// this transition retries a busy register, polls, delays, allocates, or
/// invokes a callback. RFPLL is a complete nested I2C/timer transition, tone
/// is a complete Rust MMIO leaf, and RX-DCO with its IQ estimator is a
/// complete nested register/timer transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyPrepareTransition {
    parameter: XtalDutyCalibrationParameters,
    step: XtalDutyPrepareStep,
}

impl XtalDutyPrepareTransition {
    pub const fn new(frequency_code: u16, parameter: XtalDutyCalibrationParameters) -> Self {
        Self {
            parameter,
            step: XtalDutyPrepareStep::Rfpll(RfpllFrequencyTransition::new(
                RfpllFrequencyRequest {
                    crystal_selector: parameter.rf_frequency_offset_base,
                    frequency_code: frequency_code.wrapping_sub(5),
                    offset: 0,
                },
            )),
        }
    }

    pub const fn action(self) -> XtalDutyPrepareAction {
        match self.step {
            XtalDutyPrepareStep::Rfpll(transition) => {
                XtalDutyPrepareAction::Rfpll(transition.action())
            }
            XtalDutyPrepareStep::StartTone => XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled: true,
                selector: 0x80,
                step: 0,
            },
            XtalDutyPrepareStep::EnableRxClock => {
                XtalDutyPrepareAction::ConfigureRxClock { enabled: true }
            }
            XtalDutyPrepareStep::EnableTxClock => {
                XtalDutyPrepareAction::ConfigureTxClock { enabled: true }
            }
            XtalDutyPrepareStep::PbusDebugMode => XtalDutyPrepareAction::ConfigurePbusDebugMode,
            XtalDutyPrepareStep::PbusForce(index) => XtalDutyPrepareAction::ForcePbus(
                prepare_pbus_transaction(index, self.parameter.pbus_rx_path_value),
            ),
            XtalDutyPrepareStep::PrepareRxDcoControlRestore => {
                XtalDutyPrepareAction::PrepareRxDcoControlRestore
            }
            XtalDutyPrepareStep::RxDco { transition, .. } => {
                XtalDutyPrepareAction::RxDco(transition.action())
            }
            XtalDutyPrepareStep::RestoreRxDcoControl { .. }
            | XtalDutyPrepareStep::RestoreRxDcoControlAfterFailure { .. } => {
                XtalDutyPrepareAction::RestoreRxDcoControl
            }
            XtalDutyPrepareStep::Complete(outcome) => XtalDutyPrepareAction::Complete(outcome),
            XtalDutyPrepareStep::Failed(failure) => XtalDutyPrepareAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: XtalDutyPrepareCompletion,
    ) -> Result<(), XtalDutyPrepareTransitionError> {
        self.step = match (self.step, completion) {
            (
                XtalDutyPrepareStep::Rfpll(mut transition),
                XtalDutyPrepareCompletion::Rfpll(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyPrepareTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllFrequencyAction::Complete(_) => XtalDutyPrepareStep::StartTone,
                    RfpllFrequencyAction::Failed(failure) => {
                        XtalDutyPrepareStep::Failed(XtalDutyHardwareFailure::Rfpll(failure))
                    }
                    _ => XtalDutyPrepareStep::Rfpll(transition),
                }
            }
            (
                XtalDutyPrepareStep::StartTone,
                XtalDutyPrepareCompletion::CalibrationToneConfigured {
                    enabled: true,
                    selector: 0x80,
                    step: 0,
                },
            ) => XtalDutyPrepareStep::EnableRxClock,
            (
                XtalDutyPrepareStep::EnableRxClock,
                XtalDutyPrepareCompletion::RxClockConfigured { enabled: true },
            ) => XtalDutyPrepareStep::EnableTxClock,
            (
                XtalDutyPrepareStep::EnableTxClock,
                XtalDutyPrepareCompletion::TxClockConfigured { enabled: true },
            ) => XtalDutyPrepareStep::PbusDebugMode,
            (
                XtalDutyPrepareStep::PbusDebugMode,
                XtalDutyPrepareCompletion::PbusDebugModeConfigured,
            ) => XtalDutyPrepareStep::PbusForce(0),
            (
                XtalDutyPrepareStep::PbusForce(index),
                XtalDutyPrepareCompletion::PbusForceCompleted(transaction),
            ) if transaction
                == prepare_pbus_transaction(index, self.parameter.pbus_rx_path_value) =>
            {
                if index == 9 {
                    XtalDutyPrepareStep::PrepareRxDcoControlRestore
                } else {
                    XtalDutyPrepareStep::PbusForce(index + 1)
                }
            }
            (
                XtalDutyPrepareStep::PbusForce(index),
                XtalDutyPrepareCompletion::PbusForceTimedOut(transaction),
            ) if transaction
                == prepare_pbus_transaction(index, self.parameter.pbus_rx_path_value) =>
            {
                XtalDutyPrepareStep::Failed(XtalDutyHardwareFailure::PbusForceTestTimedOut(
                    transaction,
                ))
            }
            (
                XtalDutyPrepareStep::PrepareRxDcoControlRestore,
                XtalDutyPrepareCompletion::RxDcoControlRestorePrepared,
            ) => XtalDutyPrepareStep::RxDco {
                transition: PhyRxDcoTransition::new(PhyRxDcoRequest::XTAL_DUTY),
            },
            (
                XtalDutyPrepareStep::RxDco { mut transition },
                XtalDutyPrepareCompletion::RxDco(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyPrepareTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxDcoAction::Complete(outcome) => {
                        XtalDutyPrepareStep::RestoreRxDcoControl { outcome }
                    }
                    PhyRxDcoAction::Failed(failure) => {
                        XtalDutyPrepareStep::RestoreRxDcoControlAfterFailure { failure }
                    }
                    _ => XtalDutyPrepareStep::RxDco { transition },
                }
            }
            (
                XtalDutyPrepareStep::RestoreRxDcoControl { outcome },
                XtalDutyPrepareCompletion::RxDcoControlRestored,
            ) => XtalDutyPrepareStep::Complete(outcome),
            (
                XtalDutyPrepareStep::RestoreRxDcoControlAfterFailure { failure },
                XtalDutyPrepareCompletion::RxDcoControlRestored,
            ) => XtalDutyPrepareStep::Failed(XtalDutyHardwareFailure::RxDco(failure)),
            (XtalDutyPrepareStep::Complete(_), _) | (XtalDutyPrepareStep::Failed(_), _) => {
                return Err(XtalDutyPrepareTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutyPrepareTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyRestoreAction {
    /// Complete Rust MMIO replacement for
    /// `phy_start_tx_tone_step_new(0, 0x80, 0x28, 0, 0, 0)`, including both
    /// former `g_phyFuns + 0x30` callback invocations.
    ConfigureCalibrationTone {
        enabled: bool,
        selector: u8,
        step: u8,
    },
    ConfigureRxClock {
        enabled: bool,
    },
    ConfigureTxClock {
        enabled: bool,
    },
    ForcePbus(PhyPbusForceTest),
    ConfigurePbusWorkMode,
    DelayMicros(u32),
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete,
    Failed(XtalDutyHardwareFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyRestoreCompletion {
    CalibrationToneConfigured {
        enabled: bool,
        selector: u8,
        step: u8,
    },
    RxClockConfigured {
        enabled: bool,
    },
    TxClockConfigured {
        enabled: bool,
    },
    PbusForceCompleted(PhyPbusForceTest),
    PbusForceTimedOut(PhyPbusForceTest),
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    DelayElapsed {
        micros: u32,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyRestoreTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutyRestoreStep {
    StopTone,
    DisableRxClock,
    DisableTxClock,
    PbusForce(u8),
    WorkMode,
    SettleDelay,
    WorkModePulse,
    PulseDelay,
    ClearWorkModePulse,
    Complete,
    Failed(XtalDutyHardwareFailure),
}

/// Exact finite restoration tail of one crystal-duty pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyRestoreTransition {
    step: XtalDutyRestoreStep,
}

impl XtalDutyRestoreTransition {
    pub const fn new() -> Self {
        Self {
            step: XtalDutyRestoreStep::StopTone,
        }
    }

    pub const fn action(self) -> XtalDutyRestoreAction {
        match self.step {
            XtalDutyRestoreStep::StopTone => XtalDutyRestoreAction::ConfigureCalibrationTone {
                enabled: false,
                selector: 0x80,
                step: 0x28,
            },
            XtalDutyRestoreStep::DisableRxClock => {
                XtalDutyRestoreAction::ConfigureRxClock { enabled: false }
            }
            XtalDutyRestoreStep::DisableTxClock => {
                XtalDutyRestoreAction::ConfigureTxClock { enabled: false }
            }
            XtalDutyRestoreStep::PbusForce(index) => {
                XtalDutyRestoreAction::ForcePbus(restore_pbus_transaction(index))
            }
            XtalDutyRestoreStep::WorkMode => XtalDutyRestoreAction::ConfigurePbusWorkMode,
            XtalDutyRestoreStep::SettleDelay => XtalDutyRestoreAction::DelayMicros(1),
            XtalDutyRestoreStep::WorkModePulse => XtalDutyRestoreAction::ConfigurePbusWorkModePulse,
            XtalDutyRestoreStep::PulseDelay => XtalDutyRestoreAction::DelayMicros(2),
            XtalDutyRestoreStep::ClearWorkModePulse => {
                XtalDutyRestoreAction::ClearPbusWorkModePulse
            }
            XtalDutyRestoreStep::Complete => XtalDutyRestoreAction::Complete,
            XtalDutyRestoreStep::Failed(failure) => XtalDutyRestoreAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: XtalDutyRestoreCompletion,
    ) -> Result<(), XtalDutyRestoreTransitionError> {
        self.step = match (self.step, completion) {
            (
                XtalDutyRestoreStep::StopTone,
                XtalDutyRestoreCompletion::CalibrationToneConfigured {
                    enabled: false,
                    selector: 0x80,
                    step: 0x28,
                },
            ) => XtalDutyRestoreStep::DisableRxClock,
            (
                XtalDutyRestoreStep::DisableRxClock,
                XtalDutyRestoreCompletion::RxClockConfigured { enabled: false },
            ) => XtalDutyRestoreStep::DisableTxClock,
            (
                XtalDutyRestoreStep::DisableTxClock,
                XtalDutyRestoreCompletion::TxClockConfigured { enabled: false },
            ) => XtalDutyRestoreStep::PbusForce(0),
            (
                XtalDutyRestoreStep::PbusForce(index),
                XtalDutyRestoreCompletion::PbusForceCompleted(transaction),
            ) if transaction == restore_pbus_transaction(index) => {
                if index == 2 {
                    XtalDutyRestoreStep::WorkMode
                } else {
                    XtalDutyRestoreStep::PbusForce(index + 1)
                }
            }
            (
                XtalDutyRestoreStep::PbusForce(index),
                XtalDutyRestoreCompletion::PbusForceTimedOut(transaction),
            ) if transaction == restore_pbus_transaction(index) => XtalDutyRestoreStep::Failed(
                XtalDutyHardwareFailure::PbusForceTestTimedOut(transaction),
            ),
            (
                XtalDutyRestoreStep::WorkMode,
                XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => XtalDutyRestoreStep::Complete,
            (
                XtalDutyRestoreStep::WorkMode,
                XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => XtalDutyRestoreStep::SettleDelay,
            (
                XtalDutyRestoreStep::SettleDelay,
                XtalDutyRestoreCompletion::DelayElapsed { micros: 1 },
            ) => XtalDutyRestoreStep::WorkModePulse,
            (
                XtalDutyRestoreStep::WorkModePulse,
                XtalDutyRestoreCompletion::PbusWorkModePulseConfigured,
            ) => XtalDutyRestoreStep::PulseDelay,
            (
                XtalDutyRestoreStep::PulseDelay,
                XtalDutyRestoreCompletion::DelayElapsed { micros: 2 },
            ) => XtalDutyRestoreStep::ClearWorkModePulse,
            (
                XtalDutyRestoreStep::ClearWorkModePulse,
                XtalDutyRestoreCompletion::PbusWorkModePulseCleared,
            ) => XtalDutyRestoreStep::Complete,
            (XtalDutyRestoreStep::Complete, _) | (XtalDutyRestoreStep::Failed(_), _) => {
                return Err(XtalDutyRestoreTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutyRestoreTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for XtalDutyRestoreTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyPassOutcome {
    pub frequency_code: u16,
    pub best_candidate: u8,
    pub best_filtered_power: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPassAction {
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    WriteByte {
        address: PhyI2cAddress,
        value: u8,
    },
    Prepare(XtalDutyPrepareAction),
    Search(XtalDutySearchAction),
    Restore(XtalDutyRestoreAction),
    Complete(XtalDutyPassOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPassCompletion {
    MaskedWrite { address: PhyI2cAddress },
    ByteWrite { address: PhyI2cAddress },
    Prepare(XtalDutyPrepareCompletion),
    Search(XtalDutySearchCompletion),
    Restore(XtalDutyRestoreCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyPassTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutyPassStep {
    DisablePath,
    WriteInitialDuty,
    Prepare(XtalDutyPrepareTransition),
    Search(XtalDutySearchTransition),
    RestoreInitialDuty(XtalDutySearchOutcome),
    Restore {
        transition: XtalDutyRestoreTransition,
        search: XtalDutySearchOutcome,
    },
    Complete(XtalDutyPassOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyPassTransition {
    frequency_code: u16,
    initial_duty: u8,
    parameter: XtalDutyCalibrationParameters,
    step: XtalDutyPassStep,
}

impl XtalDutyPassTransition {
    const CONTROL_ADDRESS: PhyI2cAddress = PhyI2cAddress::new(0x61, 7).unwrap();
    const DUTY_ADDRESS: PhyI2cAddress = crate::phy_i2c::analog_registers::XTAL_DUTY_CANDIDATE;

    pub const fn new(
        frequency_code: u16,
        initial_duty: u8,
        parameter: XtalDutyCalibrationParameters,
    ) -> Self {
        Self {
            frequency_code,
            initial_duty,
            parameter,
            step: XtalDutyPassStep::DisablePath,
        }
    }

    pub const fn action(self) -> XtalDutyPassAction {
        match self.step {
            XtalDutyPassStep::DisablePath => XtalDutyPassAction::WriteMasked {
                address: Self::CONTROL_ADDRESS,
                high_bit: 5,
                low_bit: 5,
                value: 0,
            },
            XtalDutyPassStep::WriteInitialDuty => XtalDutyPassAction::WriteByte {
                address: Self::DUTY_ADDRESS,
                value: self.initial_duty,
            },
            XtalDutyPassStep::Prepare(transition) => {
                XtalDutyPassAction::Prepare(transition.action())
            }
            XtalDutyPassStep::Search(transition) => XtalDutyPassAction::Search(transition.action()),
            XtalDutyPassStep::RestoreInitialDuty(_) => XtalDutyPassAction::WriteByte {
                address: Self::DUTY_ADDRESS,
                value: self.initial_duty,
            },
            XtalDutyPassStep::Restore { transition, .. } => {
                XtalDutyPassAction::Restore(transition.action())
            }
            XtalDutyPassStep::Complete(outcome) => XtalDutyPassAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: XtalDutyPassCompletion,
    ) -> Result<(), XtalDutyPassTransitionError> {
        self.step =
            match (self.step, completion) {
                (
                    XtalDutyPassStep::DisablePath,
                    XtalDutyPassCompletion::MaskedWrite { address },
                ) if address == Self::CONTROL_ADDRESS => XtalDutyPassStep::WriteInitialDuty,
                (
                    XtalDutyPassStep::WriteInitialDuty,
                    XtalDutyPassCompletion::ByteWrite { address },
                ) if address == Self::DUTY_ADDRESS => XtalDutyPassStep::Prepare(
                    XtalDutyPrepareTransition::new(self.frequency_code, self.parameter),
                ),
                (
                    XtalDutyPassStep::Prepare(mut transition),
                    XtalDutyPassCompletion::Prepare(completion),
                ) => {
                    transition
                        .advance(completion)
                        .map_err(|_| XtalDutyPassTransitionError::WrongCompletion)?;
                    match transition.action() {
                        XtalDutyPrepareAction::Complete(_) => {
                            XtalDutyPassStep::Search(XtalDutySearchTransition::new())
                        }
                        _ => XtalDutyPassStep::Prepare(transition),
                    }
                }
                (
                    XtalDutyPassStep::Search(mut transition),
                    XtalDutyPassCompletion::Search(completion),
                ) => {
                    transition
                        .advance(completion)
                        .map_err(|_| XtalDutyPassTransitionError::WrongCompletion)?;
                    match transition.action() {
                        XtalDutySearchAction::Complete(outcome) => {
                            XtalDutyPassStep::RestoreInitialDuty(outcome)
                        }
                        _ => XtalDutyPassStep::Search(transition),
                    }
                }
                (
                    XtalDutyPassStep::RestoreInitialDuty(outcome),
                    XtalDutyPassCompletion::ByteWrite { address },
                ) if address == Self::DUTY_ADDRESS => XtalDutyPassStep::Restore {
                    transition: XtalDutyRestoreTransition::new(),
                    search: outcome,
                },
                (
                    XtalDutyPassStep::Restore {
                        mut transition,
                        search,
                    },
                    XtalDutyPassCompletion::Restore(completion),
                ) => {
                    transition
                        .advance(completion)
                        .map_err(|_| XtalDutyPassTransitionError::WrongCompletion)?;
                    match transition.action() {
                        XtalDutyRestoreAction::Complete => {
                            XtalDutyPassStep::Complete(XtalDutyPassOutcome {
                                frequency_code: self.frequency_code,
                                best_candidate: search.best_candidate,
                                best_filtered_power: search.best_filtered_power,
                            })
                        }
                        _ => XtalDutyPassStep::Restore { transition, search },
                    }
                }
                (XtalDutyPassStep::Complete(_), _) => {
                    return Err(XtalDutyPassTransitionError::AlreadyComplete);
                }
                _ => return Err(XtalDutyPassTransitionError::WrongCompletion),
            };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyCalibrationOutcome {
    pub initial_duty: u8,
    pub low_frequency: XtalDutyPassOutcome,
    pub high_frequency: XtalDutyPassOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyCalibrationAction {
    ReadInitialDuty {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    DisableCalibrationPath {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
    Pass(XtalDutyPassAction),
    Complete(XtalDutyCalibrationOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyCalibrationCompletion {
    InitialDutyRead { address: PhyI2cAddress, value: u8 },
    CalibrationPathDisabled { address: PhyI2cAddress },
    Pass(XtalDutyPassCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XtalDutyCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XtalDutyCalibrationStep {
    ReadInitialDuty,
    DisableCalibrationPath {
        initial_duty: u8,
    },
    LowFrequencyPass(XtalDutyPassTransition),
    HighFrequencyPass {
        transition: XtalDutyPassTransition,
        low_frequency: XtalDutyPassOutcome,
        initial_duty: u8,
    },
    Complete(XtalDutyCalibrationOutcome),
}

/// Complete wrapper order for pinned `phy_xtal_duty_cal_init(0)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XtalDutyCalibrationTransition {
    parameter: XtalDutyCalibrationParameters,
    step: XtalDutyCalibrationStep,
}

impl XtalDutyCalibrationTransition {
    const INITIAL_DUTY_ADDRESS: PhyI2cAddress = crate::phy_i2c::analog_registers::XTAL_DUTY_SEED;
    const CONTROL_ADDRESS: PhyI2cAddress = PhyI2cAddress::new(0x61, 7).unwrap();

    pub const fn new(parameter: XtalDutyCalibrationParameters) -> Self {
        Self {
            parameter,
            step: XtalDutyCalibrationStep::ReadInitialDuty,
        }
    }

    pub const fn action(self) -> XtalDutyCalibrationAction {
        match self.step {
            XtalDutyCalibrationStep::ReadInitialDuty => {
                XtalDutyCalibrationAction::ReadInitialDuty {
                    address: Self::INITIAL_DUTY_ADDRESS,
                    high_bit: 5,
                    low_bit: 0,
                }
            }
            XtalDutyCalibrationStep::DisableCalibrationPath { .. } => {
                XtalDutyCalibrationAction::DisableCalibrationPath {
                    address: Self::CONTROL_ADDRESS,
                    high_bit: 5,
                    low_bit: 5,
                    value: 0,
                }
            }
            XtalDutyCalibrationStep::LowFrequencyPass(transition)
            | XtalDutyCalibrationStep::HighFrequencyPass { transition, .. } => {
                XtalDutyCalibrationAction::Pass(transition.action())
            }
            XtalDutyCalibrationStep::Complete(outcome) => {
                XtalDutyCalibrationAction::Complete(outcome)
            }
        }
    }

    pub fn advance(
        &mut self,
        completion: XtalDutyCalibrationCompletion,
    ) -> Result<(), XtalDutyCalibrationTransitionError> {
        self.step = match (self.step, completion) {
            (
                XtalDutyCalibrationStep::ReadInitialDuty,
                XtalDutyCalibrationCompletion::InitialDutyRead { address, value },
            ) if address == Self::INITIAL_DUTY_ADDRESS => {
                XtalDutyCalibrationStep::DisableCalibrationPath {
                    initial_duty: value,
                }
            }
            (
                XtalDutyCalibrationStep::DisableCalibrationPath { initial_duty },
                XtalDutyCalibrationCompletion::CalibrationPathDisabled { address },
            ) if address == Self::CONTROL_ADDRESS => XtalDutyCalibrationStep::LowFrequencyPass(
                XtalDutyPassTransition::new(0x988, initial_duty, self.parameter),
            ),
            (
                XtalDutyCalibrationStep::LowFrequencyPass(mut transition),
                XtalDutyCalibrationCompletion::Pass(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutyPassAction::Complete(low_frequency) => {
                        XtalDutyCalibrationStep::HighFrequencyPass {
                            transition: XtalDutyPassTransition::new(
                                0x9b0,
                                transition.initial_duty,
                                self.parameter,
                            ),
                            low_frequency,
                            initial_duty: transition.initial_duty,
                        }
                    }
                    _ => XtalDutyCalibrationStep::LowFrequencyPass(transition),
                }
            }
            (
                XtalDutyCalibrationStep::HighFrequencyPass {
                    mut transition,
                    low_frequency,
                    initial_duty,
                },
                XtalDutyCalibrationCompletion::Pass(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| XtalDutyCalibrationTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutyPassAction::Complete(high_frequency) => {
                        XtalDutyCalibrationStep::Complete(XtalDutyCalibrationOutcome {
                            initial_duty,
                            low_frequency,
                            high_frequency,
                        })
                    }
                    _ => XtalDutyCalibrationStep::HighFrequencyPass {
                        transition,
                        low_frequency,
                        initial_duty,
                    },
                }
            }
            (XtalDutyCalibrationStep::Complete(_), _) => {
                return Err(XtalDutyCalibrationTransitionError::AlreadyComplete);
            }
            _ => return Err(XtalDutyCalibrationTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PhyI2cAddress, XtalDutyCalibrationAction, XtalDutyCalibrationCompletion,
        XtalDutyCalibrationOutcome, XtalDutyCalibrationParameters, XtalDutyCalibrationTransition,
        XtalDutyHardwareFailure, XtalDutyPassAction, XtalDutyPassCompletion, XtalDutyPassOutcome,
        XtalDutyPassTransition, XtalDutyPassTransitionError, XtalDutyPrepareAction,
        XtalDutyPrepareCompletion, XtalDutyPrepareTransition, XtalDutyRestoreAction,
        XtalDutyRestoreCompletion, XtalDutyRestoreTransition, XtalDutySearchAction,
        XtalDutySearchCompletion, XtalDutySearchOutcome, XtalDutySearchTransition,
        XtalDutySearchTransitionError,
    };
    use crate::phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqReadinessSnapshot,
    };
    use crate::phy_pbus::PhyPbusForceTest;
    use crate::phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};
    use crate::phy_rx_dco::{PhyRxDcoAction, PhyRxDcoCompletion};
    use crate::phy_signal_power::{
        PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerCompletion,
    };

    fn signal_components(value: i64) -> (i32, i32) {
        for first in 0..=512_i32 {
            for second in 0..=512_i32 {
                if i64::from(first * first + second * second) == value {
                    return (first, second);
                }
            }
        }
        panic!("test signal power is not a bounded sum of two squares");
    }

    fn complete_signal_power_action(
        action: PhySignalPowerAction,
        value: i64,
    ) -> PhySignalPowerCompletion {
        match action {
            PhySignalPowerAction::ConfigureClock {
                request,
                clock,
                enabled,
            } => PhySignalPowerCompletion::ClockConfigured {
                request,
                clock,
                enabled,
            },
            PhySignalPowerAction::SetEstimatorEnable {
                request,
                phase,
                enabled,
            } => PhySignalPowerCompletion::EstimatorEnableSet {
                request,
                phase,
                enabled,
            },
            PhySignalPowerAction::DelayMicros {
                request,
                phase,
                micros,
            } => PhySignalPowerCompletion::DelayElapsed {
                request,
                phase,
                micros,
            },
            PhySignalPowerAction::ConfigureEstimator { request, control } => {
                PhySignalPowerCompletion::EstimatorConfigured { request, control }
            }
            PhySignalPowerAction::AwaitReadinessEdge { request, .. } => {
                PhySignalPowerCompletion::ReadinessObserved {
                    request,
                    snapshot: PhyDcIqReadinessSnapshot {
                        ready: true,
                        activity: false,
                    },
                }
            }
            PhySignalPowerAction::ReadAccumulators(request) => {
                let (sum, difference) = signal_components(value);
                let shift = u32::from(request.shift.wrapping_sub(2)) & 0x1f;
                PhySignalPowerCompletion::AccumulatorsRead {
                    request,
                    snapshot: PhySignalPowerAccumulatorSnapshot {
                        sum_i: sum.wrapping_shl(shift),
                        difference_i: difference.wrapping_shl(shift),
                        difference_q: 0,
                        sum_q: 0,
                    },
                }
            }
            action => panic!("unexpected terminal signal-power action: {action:?}"),
        }
    }

    fn signal_power_request(
        action: PhySignalPowerAction,
    ) -> crate::phy_signal_power::PhySignalPowerRequest {
        match action {
            PhySignalPowerAction::ConfigureClock { request, .. }
            | PhySignalPowerAction::SetEstimatorEnable { request, .. }
            | PhySignalPowerAction::DelayMicros { request, .. }
            | PhySignalPowerAction::ConfigureEstimator { request, .. }
            | PhySignalPowerAction::AwaitReadinessEdge { request, .. }
            | PhySignalPowerAction::ReadAccumulators(request) => request,
            action => panic!("unexpected terminal signal-power action: {action:?}"),
        }
    }

    fn complete_search_measurement(transition: &mut XtalDutySearchTransition, value: i64) {
        let XtalDutySearchAction::SignalPower(first_action) = transition.action() else {
            panic!("signal-power measurement was not armed");
        };
        let request = signal_power_request(first_action);
        loop {
            let XtalDutySearchAction::SignalPower(action) = transition.action() else {
                return;
            };
            if signal_power_request(action) != request {
                return;
            }
            transition
                .advance(XtalDutySearchCompletion::SignalPower(
                    complete_signal_power_action(action, value),
                ))
                .unwrap();
        }
    }

    fn complete_dc_iq_action(action: PhyDcIqAction) -> PhyDcIqCompletion {
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
            PhyDcIqAction::ReadAccumulators(request) => PhyDcIqCompletion::AccumulatorsRead {
                request,
                snapshot: PhyDcIqAccumulatorSnapshot {
                    i: 0,
                    q: 0,
                    power: 0,
                },
            },
            action => panic!("unexpected terminal DC/IQ action: {action:?}"),
        }
    }

    fn complete_rx_dco_action(action: PhyRxDcoAction) -> PhyRxDcoCompletion {
        match action {
            PhyRxDcoAction::PrepareRxDcoControlRestore => {
                PhyRxDcoCompletion::RxDcoControlRestorePrepared
            }
            PhyRxDcoAction::ReadPbus { selector, path } => PhyRxDcoCompletion::PbusRead {
                selector,
                path,
                value: 0,
            },
            PhyRxDcoAction::ForcePbus(transaction) => {
                PhyRxDcoCompletion::PbusForceCompleted(transaction)
            }
            PhyRxDcoAction::DelayMicros { iteration, micros } => {
                PhyRxDcoCompletion::DelayElapsed { iteration, micros }
            }
            PhyRxDcoAction::DcIq(action) => PhyRxDcoCompletion::DcIq(complete_dc_iq_action(action)),
            PhyRxDcoAction::RestoreRxDcoControl => PhyRxDcoCompletion::RxDcoControlRestored,
            action => panic!("unexpected terminal RX-DCO action: {action:?}"),
        }
    }

    fn complete_rfpll_action(
        action: RfpllFrequencyAction,
        cap_status_reads: &mut u8,
    ) -> RfpllFrequencyCompletion {
        match action {
            RfpllFrequencyAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                ..
            } => RfpllFrequencyCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            },
            RfpllFrequencyAction::WriteByte { address, .. } => {
                RfpllFrequencyCompletion::ByteWrite { address }
            }
            RfpllFrequencyAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => RfpllFrequencyCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: if high_bit == 1 { 1 } else { 0 },
            },
            RfpllFrequencyAction::ReadByte { address } => {
                let value = if address
                    == crate::phy_i2c::analog_registers::RFPLL_CALIBRATED_CAPACITOR_LOW
                {
                    100
                } else {
                    let value = if (*cap_status_reads).is_multiple_of(3) {
                        0
                    } else {
                        1 << 2
                    };
                    *cap_status_reads = (*cap_status_reads).wrapping_add(1);
                    value
                };
                RfpllFrequencyCompletion::ByteRead { address, value }
            }
            RfpllFrequencyAction::DelayMicros(micros) => {
                RfpllFrequencyCompletion::DelayElapsed(micros)
            }
            action => panic!("unexpected terminal RFPLL action: {action:?}"),
        }
    }

    fn complete_prepare_action(
        action: XtalDutyPrepareAction,
        rfpll_cap_status_reads: &mut u8,
    ) -> XtalDutyPrepareCompletion {
        match action {
            XtalDutyPrepareAction::Rfpll(action) => XtalDutyPrepareCompletion::Rfpll(
                complete_rfpll_action(action, rfpll_cap_status_reads),
            ),
            XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled,
                selector,
                step,
            } => XtalDutyPrepareCompletion::CalibrationToneConfigured {
                enabled,
                selector,
                step,
            },
            XtalDutyPrepareAction::ConfigureRxClock { enabled } => {
                XtalDutyPrepareCompletion::RxClockConfigured { enabled }
            }
            XtalDutyPrepareAction::ConfigureTxClock { enabled } => {
                XtalDutyPrepareCompletion::TxClockConfigured { enabled }
            }
            XtalDutyPrepareAction::ConfigurePbusDebugMode => {
                XtalDutyPrepareCompletion::PbusDebugModeConfigured
            }
            XtalDutyPrepareAction::ForcePbus(transaction) => {
                XtalDutyPrepareCompletion::PbusForceCompleted(transaction)
            }
            XtalDutyPrepareAction::PrepareRxDcoControlRestore => {
                XtalDutyPrepareCompletion::RxDcoControlRestorePrepared
            }
            XtalDutyPrepareAction::RxDco(action) => {
                XtalDutyPrepareCompletion::RxDco(complete_rx_dco_action(action))
            }
            XtalDutyPrepareAction::RestoreRxDcoControl => {
                XtalDutyPrepareCompletion::RxDcoControlRestored
            }
            action => panic!("unexpected terminal preparation action: {action:?}"),
        }
    }

    fn complete_restore_action(action: XtalDutyRestoreAction) -> XtalDutyRestoreCompletion {
        match action {
            XtalDutyRestoreAction::ConfigureCalibrationTone {
                enabled,
                selector,
                step,
            } => XtalDutyRestoreCompletion::CalibrationToneConfigured {
                enabled,
                selector,
                step,
            },
            XtalDutyRestoreAction::ConfigureRxClock { enabled } => {
                XtalDutyRestoreCompletion::RxClockConfigured { enabled }
            }
            XtalDutyRestoreAction::ConfigureTxClock { enabled } => {
                XtalDutyRestoreCompletion::TxClockConfigured { enabled }
            }
            XtalDutyRestoreAction::ForcePbus(transaction) => {
                XtalDutyRestoreCompletion::PbusForceCompleted(transaction)
            }
            XtalDutyRestoreAction::ConfigurePbusWorkMode => {
                XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                }
            }
            action => panic!("unexpected restoration action: {action:?}"),
        }
    }

    fn drive_pass(
        transition: &mut XtalDutyCalibrationTransition,
        _expected_frequency_code: u16,
        initial_duty: u8,
    ) {
        let mut current_candidate = None;
        let mut rfpll_cap_status_reads = 0;
        loop {
            match transition.action() {
                XtalDutyCalibrationAction::Pass(XtalDutyPassAction::WriteMasked {
                    address,
                    high_bit: 5,
                    low_bit: 5,
                    value: 0,
                }) => {
                    transition
                        .advance(XtalDutyCalibrationCompletion::Pass(
                            XtalDutyPassCompletion::MaskedWrite { address },
                        ))
                        .unwrap();
                }
                XtalDutyCalibrationAction::Pass(XtalDutyPassAction::WriteByte {
                    address,
                    value,
                }) => {
                    assert_eq!(value, initial_duty);
                    transition
                        .advance(XtalDutyCalibrationCompletion::Pass(
                            XtalDutyPassCompletion::ByteWrite { address },
                        ))
                        .unwrap();
                }
                XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Prepare(action)) => {
                    transition
                        .advance(XtalDutyCalibrationCompletion::Pass(
                            XtalDutyPassCompletion::Prepare(complete_prepare_action(
                                action,
                                &mut rfpll_cap_status_reads,
                            )),
                        ))
                        .unwrap();
                }
                XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Search(
                    XtalDutySearchAction::WriteCandidate { address, candidate },
                )) => {
                    current_candidate = Some(candidate);
                    transition
                        .advance(XtalDutyCalibrationCompletion::Pass(
                            XtalDutyPassCompletion::Search(
                                XtalDutySearchCompletion::CandidateWritten { address, candidate },
                            ),
                        ))
                        .unwrap();
                }
                XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Search(
                    XtalDutySearchAction::DelayMicros {
                        candidate,
                        micros: 20,
                    },
                )) => {
                    assert_eq!(current_candidate, Some(candidate));
                    transition
                        .advance(XtalDutyCalibrationCompletion::Pass(
                            XtalDutyPassCompletion::Search(
                                XtalDutySearchCompletion::DelayElapsed { candidate },
                            ),
                        ))
                        .unwrap();
                }
                XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Search(
                    XtalDutySearchAction::SignalPower(action),
                )) => {
                    let candidate = current_candidate.unwrap();
                    let component = i64::from(0x80 - candidate);
                    transition
                        .advance(XtalDutyCalibrationCompletion::Pass(
                            XtalDutyPassCompletion::Search(XtalDutySearchCompletion::SignalPower(
                                complete_signal_power_action(
                                    action,
                                    component.wrapping_mul(component),
                                ),
                            )),
                        ))
                        .unwrap();
                }
                XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Restore(action)) => {
                    let pass_complete =
                        matches!(action, XtalDutyRestoreAction::ConfigurePbusWorkMode);
                    transition
                        .advance(XtalDutyCalibrationCompletion::Pass(
                            XtalDutyPassCompletion::Restore(complete_restore_action(action)),
                        ))
                        .unwrap();
                    if pass_complete {
                        break;
                    }
                }
                action => panic!("unexpected pass action: {action:?}"),
            }
        }
    }

    #[test]
    fn evaluates_all_31_candidates_only_after_timer_and_measurement_edges() {
        let mut transition = XtalDutySearchTransition::new();
        let mut writes = 0;
        let mut delays = 0;
        let mut measurements = 0;
        loop {
            match transition.action() {
                XtalDutySearchAction::WriteCandidate { address, candidate } => {
                    writes += 1;
                    transition
                        .advance(XtalDutySearchCompletion::CandidateWritten { address, candidate })
                        .unwrap();
                }
                XtalDutySearchAction::DelayMicros {
                    candidate,
                    micros: 20,
                } => {
                    delays += 1;
                    assert_eq!(candidate, 0x20 + delays - 1);
                    transition
                        .advance(XtalDutySearchCompletion::DelayElapsed { candidate })
                        .unwrap();
                }
                XtalDutySearchAction::SignalPower(_) => {
                    measurements += 1;
                    let candidate = 0x20 + writes - 1;
                    let component = i64::from(0x80 - candidate);
                    complete_search_measurement(&mut transition, component.wrapping_mul(component));
                }
                XtalDutySearchAction::Complete(outcome) => {
                    assert_eq!(
                        outcome,
                        XtalDutySearchOutcome {
                            best_candidate: 0x3e,
                            best_filtered_power: 0x42 * 0x42,
                        }
                    );
                    break;
                }
                action => panic!("unexpected action: {action:?}"),
            }
        }
        assert_eq!(writes, 31);
        assert_eq!(delays, 31);
        assert_eq!(measurements, 31 * 4);
    }

    #[test]
    fn each_outlier_uses_at_most_two_identity_bound_replacements() {
        let mut transition = XtalDutySearchTransition::new();
        let duty_address = PhyI2cAddress::new(0x61, 0x0a).unwrap();
        transition
            .advance(XtalDutySearchCompletion::CandidateWritten {
                address: duty_address,
                candidate: 0x20,
            })
            .unwrap();
        transition
            .advance(XtalDutySearchCompletion::DelayElapsed { candidate: 0x20 })
            .unwrap();
        for value in [1, 100, 100, 100] {
            complete_search_measurement(&mut transition, value);
        }
        assert!(matches!(
            transition.action(),
            XtalDutySearchAction::SignalPower(_)
        ));
        assert_eq!(
            transition.advance(XtalDutySearchCompletion::CandidateWritten {
                address: duty_address,
                candidate: 0x21,
            }),
            Err(XtalDutySearchTransitionError::WrongCompletion)
        );
        complete_search_measurement(&mut transition, 200);
        assert!(matches!(
            transition.action(),
            XtalDutySearchAction::SignalPower(_)
        ));
        complete_search_measurement(&mut transition, 64);
        assert_eq!(
            transition.action(),
            XtalDutySearchAction::WriteCandidate {
                address: duty_address,
                candidate: 0x21,
            }
        );
    }

    #[test]
    fn preparation_exposes_all_ten_pbus_commands_and_owned_rx_dco_field() {
        let parameter = XtalDutyCalibrationParameters {
            rf_frequency_offset_base: 0x31,
            pbus_rx_path_value: 0x42,
        };
        let mut transition = XtalDutyPrepareTransition::new(0x988, parameter);
        let mut rfpll_cap_status_reads = 0;

        while let XtalDutyPrepareAction::Rfpll(action) = transition.action() {
            transition
                .advance(XtalDutyPrepareCompletion::Rfpll(complete_rfpll_action(
                    action,
                    &mut rfpll_cap_status_reads,
                )))
                .unwrap();
        }

        for expected in [
            XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled: true,
                selector: 0x80,
                step: 0,
            },
            XtalDutyPrepareAction::ConfigureRxClock { enabled: true },
            XtalDutyPrepareAction::ConfigureTxClock { enabled: true },
            XtalDutyPrepareAction::ConfigurePbusDebugMode,
        ] {
            assert_eq!(transition.action(), expected);
            transition
                .advance(complete_prepare_action(
                    expected,
                    &mut rfpll_cap_status_reads,
                ))
                .unwrap();
        }

        let expected_pbus = [
            PhyPbusForceTest::new(4, 1, 0),
            PhyPbusForceTest::new(4, 2, 1),
            PhyPbusForceTest::new(5, 1, 0),
            PhyPbusForceTest::new(0, 1, 0x40),
            PhyPbusForceTest::new(0, 2, 0x42),
            PhyPbusForceTest::new(1, 1, 0x189),
            PhyPbusForceTest::new(1, 2, 0xf0),
            PhyPbusForceTest::new(0, 1, 0x43),
            PhyPbusForceTest::new(1, 1, 0x38),
            PhyPbusForceTest::new(1, 1, 0x189),
        ];
        for transaction in expected_pbus {
            assert_eq!(
                transition.action(),
                XtalDutyPrepareAction::ForcePbus(transaction)
            );
            transition
                .advance(XtalDutyPrepareCompletion::PbusForceCompleted(transaction))
                .unwrap();
        }

        assert_eq!(
            transition.action(),
            XtalDutyPrepareAction::PrepareRxDcoControlRestore
        );
        transition
            .advance(XtalDutyPrepareCompletion::RxDcoControlRestorePrepared)
            .unwrap();
        assert_eq!(
            transition.action(),
            XtalDutyPrepareAction::RxDco(PhyRxDcoAction::PrepareRxDcoControlRestore)
        );
        while let XtalDutyPrepareAction::RxDco(action) = transition.action() {
            transition
                .advance(XtalDutyPrepareCompletion::RxDco(complete_rx_dco_action(
                    action,
                )))
                .unwrap();
        }
        let outcome = crate::phy_rx_dco::PhyRxDcoOutcome {
            configuration: [0x0100_0100; 2],
            iterations: 1,
            converged: true,
            last_estimate: crate::phy_dc_iq::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
        };
        assert_eq!(
            transition.action(),
            XtalDutyPrepareAction::RestoreRxDcoControl
        );
        transition
            .advance(XtalDutyPrepareCompletion::RxDcoControlRestored)
            .unwrap();
        assert_eq!(
            transition.action(),
            XtalDutyPrepareAction::Complete(outcome)
        );
    }

    #[test]
    fn restoration_requires_external_pbus_and_timer_completions() {
        let mut transition = XtalDutyRestoreTransition::new();
        for expected in [
            XtalDutyRestoreAction::ConfigureCalibrationTone {
                enabled: false,
                selector: 0x80,
                step: 0x28,
            },
            XtalDutyRestoreAction::ConfigureRxClock { enabled: false },
            XtalDutyRestoreAction::ConfigureTxClock { enabled: false },
        ] {
            assert_eq!(transition.action(), expected);
            transition
                .advance(complete_restore_action(expected))
                .unwrap();
        }

        for transaction in [
            PhyPbusForceTest::new(0, 1, 0),
            PhyPbusForceTest::new(1, 1, 0),
            PhyPbusForceTest::new(1, 2, 0),
        ] {
            assert_eq!(
                transition.action(),
                XtalDutyRestoreAction::ForcePbus(transaction)
            );
            transition
                .advance(XtalDutyRestoreCompletion::PbusForceCompleted(transaction))
                .unwrap();
        }

        assert_eq!(
            transition.action(),
            XtalDutyRestoreAction::ConfigurePbusWorkMode
        );
        transition
            .advance(XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                settle_required: true,
            })
            .unwrap();
        assert_eq!(transition.action(), XtalDutyRestoreAction::DelayMicros(1));
        assert!(
            transition
                .advance(XtalDutyRestoreCompletion::DelayElapsed { micros: 2 })
                .is_err()
        );
        transition
            .advance(XtalDutyRestoreCompletion::DelayElapsed { micros: 1 })
            .unwrap();
        assert_eq!(
            transition.action(),
            XtalDutyRestoreAction::ConfigurePbusWorkModePulse
        );
        transition
            .advance(XtalDutyRestoreCompletion::PbusWorkModePulseConfigured)
            .unwrap();
        assert_eq!(transition.action(), XtalDutyRestoreAction::DelayMicros(2));
        transition
            .advance(XtalDutyRestoreCompletion::DelayElapsed { micros: 2 })
            .unwrap();
        assert_eq!(
            transition.action(),
            XtalDutyRestoreAction::ClearPbusWorkModePulse
        );
        transition
            .advance(XtalDutyRestoreCompletion::PbusWorkModePulseCleared)
            .unwrap();
        assert_eq!(transition.action(), XtalDutyRestoreAction::Complete);

        let mut timed_out = XtalDutyRestoreTransition::new();
        for _ in 0..3 {
            let action = timed_out.action();
            timed_out.advance(complete_restore_action(action)).unwrap();
        }
        let XtalDutyRestoreAction::ForcePbus(transaction) = timed_out.action() else {
            panic!("expected first restore PBus command");
        };
        timed_out
            .advance(XtalDutyRestoreCompletion::PbusForceTimedOut(transaction))
            .unwrap();
        assert_eq!(
            timed_out.action(),
            XtalDutyRestoreAction::Failed(XtalDutyHardwareFailure::PbusForceTestTimedOut(
                transaction
            ))
        );
    }

    #[test]
    fn pass_rejects_wrong_address_and_stale_parameter_completion() {
        let parameter = XtalDutyCalibrationParameters {
            rf_frequency_offset_base: 0x31,
            pbus_rx_path_value: 0x42,
        };
        let mut transition = XtalDutyPassTransition::new(0x988, 0x2a, parameter);
        let XtalDutyPassAction::WriteMasked { address, .. } = transition.action() else {
            panic!("expected path-disable write");
        };
        assert_eq!(
            transition.advance(XtalDutyPassCompletion::MaskedWrite {
                address: PhyI2cAddress::new(0x62, 7).unwrap(),
            }),
            Err(XtalDutyPassTransitionError::WrongCompletion)
        );
        transition
            .advance(XtalDutyPassCompletion::MaskedWrite { address })
            .unwrap();

        let XtalDutyPassAction::WriteByte { address, .. } = transition.action() else {
            panic!("expected initial-duty write");
        };
        transition
            .advance(XtalDutyPassCompletion::ByteWrite { address })
            .unwrap();
        assert_eq!(
            transition.advance(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(20))
            )),
            Err(XtalDutyPassTransitionError::WrongCompletion)
        );
        assert!(matches!(
            transition.action(),
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::WriteMasked { .. }
            ))
        ));
    }

    #[test]
    fn wrapper_orders_both_frequency_passes_without_hidden_progress() {
        let initial_duty = 0x2a;
        let mut transition = XtalDutyCalibrationTransition::new(XtalDutyCalibrationParameters {
            rf_frequency_offset_base: 0x31,
            pbus_rx_path_value: 0x42,
        });

        let XtalDutyCalibrationAction::ReadInitialDuty {
            address,
            high_bit: 5,
            low_bit: 0,
        } = transition.action()
        else {
            panic!("expected the initial duty read");
        };
        transition
            .advance(XtalDutyCalibrationCompletion::InitialDutyRead {
                address,
                value: initial_duty,
            })
            .unwrap();

        let XtalDutyCalibrationAction::DisableCalibrationPath {
            address,
            high_bit: 5,
            low_bit: 5,
            value: 0,
        } = transition.action()
        else {
            panic!("expected the calibration-path write");
        };
        transition
            .advance(XtalDutyCalibrationCompletion::CalibrationPathDisabled { address })
            .unwrap();

        drive_pass(&mut transition, 0x988, initial_duty);
        drive_pass(&mut transition, 0x9b0, initial_duty);

        assert_eq!(
            transition.action(),
            XtalDutyCalibrationAction::Complete(XtalDutyCalibrationOutcome {
                initial_duty,
                low_frequency: XtalDutyPassOutcome {
                    frequency_code: 0x988,
                    best_candidate: 0x3e,
                    best_filtered_power: 0x42 * 0x42,
                },
                high_frequency: XtalDutyPassOutcome {
                    frequency_code: 0x9b0,
                    best_candidate: 0x3e,
                    best_filtered_power: 0x42 * 0x42,
                },
            })
        );
    }
}
