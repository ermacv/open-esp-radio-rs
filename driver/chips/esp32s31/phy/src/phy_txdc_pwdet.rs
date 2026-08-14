//! Rust-owned TX-DC calibration through the power detector.
//!
//! This is the mandatory 520-byte archive root
//! `phy_txdc_cal_pwdet_init` and its 954-byte archive search child
//! `phy_txdc_cal_pwdet_new`. The vendor diagnostic branches are removed.
//! Each former delay, SAR observation and PBus force is represented by one
//! externally completed action; the two 50-point scans are finite bounds, not
//! executor polling loops.

use crate::{
    phy_pbus::PhyPbusForceTest,
    phy_tx_cal::{
        PhyToneSarAction, PhyToneSarCompletion, PhyToneSarFailure, PhyToneSarRequest,
        PhyToneSarTransition,
    },
};

const COMPONENT_COUNT: u8 = 2;
const SCAN_DIRECTION_COUNT: u8 = 2;
const SCAN_POINT_LIMIT: u8 = 50;
const PRECHECK_LIMIT: u8 = 2;
const MEASUREMENT_CAPACITY: usize = 100;
const DEFAULT_DCO: [u16; 4] = [0x100; 4];
const TX_BB_GAIN: [u16; 3] = [0, 0x80, 0x100];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchRequest {
    pub identity: u8,
    pub initial: [u16; 4],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchOutcome {
    pub identity: u8,
    pub dco: [u16; 4],
    pub measurements: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchAction {
    ForcePbus(PhyPbusForceTest),
    DelayMicros {
        identity: u8,
        component: u8,
        measurement: u8,
        micros: u32,
    },
    ToneSar(PhyToneSarAction),
    Complete(PhyTxDcPwdetSearchOutcome),
    Failed(PhyTxDcPwdetSearchFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchCompletion {
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    DelayElapsed {
        identity: u8,
        component: u8,
        measurement: u8,
        micros: u32,
    },
    ToneSar(PhyToneSarCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchFailure {
    PbusTimedOut(PhyPbusForceTest),
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MeasurementKind {
    PrecheckPositive,
    PrecheckNegative,
    Scan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchStep {
    ProgramDco {
        transaction: u8,
        kind: MeasurementKind,
    },
    Delay(MeasurementKind),
    Measure {
        kind: MeasurementKind,
        transition: PhyToneSarTransition,
    },
    CommitDco {
        transaction: u8,
    },
    Complete,
    Failed(PhyTxDcPwdetSearchFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchTransition {
    request: PhyTxDcPwdetSearchRequest,
    step: SearchStep,
    working: [u16; 4],
    component: u8,
    precheck: u8,
    positive_step: u8,
    negative_step: u8,
    direction: u8,
    scan_offset: u8,
    boundary: u8,
    minimum: u16,
    samples: [u16; MEASUREMENT_CAPACITY],
    sample_count: u8,
    total_measurements: u8,
    positive_sample: u16,
}

impl PhyTxDcPwdetSearchTransition {
    pub const fn new(request: PhyTxDcPwdetSearchRequest) -> Self {
        let mut working = request.initial;
        working[2] = working[2].wrapping_add(10);
        Self {
            request,
            step: SearchStep::ProgramDco {
                transaction: 0,
                kind: MeasurementKind::PrecheckPositive,
            },
            working,
            component: 0,
            precheck: 0,
            positive_step: 10,
            negative_step: 10,
            direction: 0,
            scan_offset: 0,
            boundary: 0,
            minimum: u16::MAX,
            samples: [0; MEASUREMENT_CAPACITY],
            sample_count: 0,
            total_measurements: 0,
            positive_sample: 0,
        }
    }

    const fn component_index(&self) -> usize {
        2 + self.component as usize
    }

    const fn dco_transaction(&self, transaction: u8) -> PhyPbusForceTest {
        match transaction {
            0 => PhyPbusForceTest::new(2, 1, self.working[0]),
            1 => PhyPbusForceTest::new(3, 1, self.working[1]),
            2 => PhyPbusForceTest::new(2, 2, self.working[2]),
            _ => PhyPbusForceTest::new(3, 2, self.working[3]),
        }
    }

    fn tone_sar(&self) -> PhyToneSarTransition {
        PhyToneSarTransition::new(PhyToneSarRequest {
            measurement: self
                .request
                .identity
                .wrapping_mul(64)
                .wrapping_add(self.total_measurements),
            samples: 2,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
        .expect("two samples are nonzero")
    }

    fn set_component_delta(&mut self, delta: i16) {
        let index = self.component_index();
        self.working[index] = self.request.initial[index].wrapping_add(delta as u16);
    }

    fn program(&mut self, kind: MeasurementKind) {
        self.step = SearchStep::ProgramDco {
            transaction: 0,
            kind,
        };
    }

    fn begin_scan(&mut self) {
        self.direction = 0;
        self.scan_offset = 0;
        self.boundary = 0;
        self.minimum = u16::MAX;
        self.sample_count = 0;
        self.set_component_delta(self.positive_step as i16);
        self.program(MeasurementKind::Scan);
    }

    fn after_precheck(&mut self, negative_sample: u16) {
        if negative_sample.wrapping_add(15) < self.positive_sample {
            self.positive_step = self.positive_step.wrapping_sub(5);
        } else if negative_sample > self.positive_sample.wrapping_add(15) {
            self.negative_step = self.negative_step.wrapping_sub(5);
        } else {
            self.begin_scan();
            return;
        }
        self.precheck += 1;
        if self.precheck == PRECHECK_LIMIT {
            self.begin_scan();
        } else {
            self.set_component_delta(self.positive_step as i16);
            self.program(MeasurementKind::PrecheckPositive);
        }
    }

    fn scan_sample(&mut self, sample: u16) {
        let index = self.sample_count as usize;
        self.samples[index] = sample;
        self.sample_count += 1;
        if sample < self.minimum {
            self.minimum = sample;
        }
        let keep_scanning = (self.scan_offset < 6 || sample <= self.minimum.wrapping_add(30))
            && self.scan_offset + 1 != SCAN_POINT_LIMIT;
        if keep_scanning {
            self.scan_offset += 1;
            let delta = if self.direction == 0 {
                self.positive_step as i16 + self.scan_offset as i16
            } else {
                -(self.negative_step as i16 + self.scan_offset as i16)
            };
            self.set_component_delta(delta);
            self.program(MeasurementKind::Scan);
        } else if self.direction + 1 != SCAN_DIRECTION_COUNT {
            self.boundary = self.scan_offset;
            self.direction = 1;
            self.scan_offset = 0;
            self.minimum = u16::MAX;
            self.set_component_delta(-(self.negative_step as i16));
            self.program(MeasurementKind::Scan);
        } else {
            self.finish_component();
        }
    }

    fn finish_component(&mut self) {
        let count = self.sample_count as usize;
        let boundary = self.boundary as usize;
        let mut index = 0;
        while index != count {
            if index == 0 || index == boundary + 1 {
                if index + 1 < count && self.samples[index + 1] < self.samples[index] {
                    self.samples[index] = self.samples[index + 1];
                }
            } else if self.samples[index] < self.samples[index - 1] {
                self.samples[index] = self.samples[index - 1];
            }
            index += 1;
        }

        let first = self.samples[0];
        let first_negative = self.samples[(boundary + 1).min(count - 1)];
        let threshold = first.abs_diff(first_negative).max(20);
        let minimum = first.min(first_negative);
        let base = self.request.initial[self.component_index()] as i16;
        let mut upper = base.wrapping_add(self.positive_step as i16);
        let low_bound = base.wrapping_sub(self.negative_step as i16);
        let mut lower = low_bound;
        index = 0;
        while index != count {
            if self.samples[index] <= minimum.wrapping_add(threshold) {
                let mut candidate = base
                    .wrapping_add(boundary as i16)
                    .wrapping_add(1)
                    .wrapping_sub(self.negative_step as i16)
                    .wrapping_sub(index as i16);
                if index <= boundary {
                    upper = base
                        .wrapping_add(self.positive_step as i16)
                        .wrapping_add(index as i16);
                    candidate = low_bound;
                }
                lower = candidate;
            }
            index += 1;
        }
        let result = upper.wrapping_add(lower).wrapping_add(1) / 2;
        let component_index = self.component_index();
        self.working[component_index] = result as u16;

        self.component += 1;
        if self.component == COMPONENT_COUNT {
            self.step = SearchStep::CommitDco { transaction: 0 };
        } else {
            self.precheck = 0;
            self.positive_step = 10;
            self.negative_step = 10;
            self.direction = 0;
            self.scan_offset = 0;
            self.boundary = 0;
            self.minimum = u16::MAX;
            self.sample_count = 0;
            self.set_component_delta(10);
            self.program(MeasurementKind::PrecheckPositive);
        }
    }

    pub const fn action(&self) -> PhyTxDcPwdetSearchAction {
        match self.step {
            SearchStep::ProgramDco { transaction, .. } => {
                PhyTxDcPwdetSearchAction::ForcePbus(self.dco_transaction(transaction))
            }
            SearchStep::Delay(_) => PhyTxDcPwdetSearchAction::DelayMicros {
                identity: self.request.identity,
                component: self.component,
                measurement: self.total_measurements,
                micros: 10,
            },
            SearchStep::Measure { transition, .. } => {
                PhyTxDcPwdetSearchAction::ToneSar(transition.action())
            }
            SearchStep::CommitDco { transaction } => {
                PhyTxDcPwdetSearchAction::ForcePbus(self.dco_transaction(transaction))
            }
            SearchStep::Complete => PhyTxDcPwdetSearchAction::Complete(PhyTxDcPwdetSearchOutcome {
                identity: self.request.identity,
                dco: self.working,
                measurements: self.total_measurements,
            }),
            SearchStep::Failed(failure) => PhyTxDcPwdetSearchAction::Failed(failure),
        }
    }

    pub const fn failure(&self) -> Option<PhyTxDcPwdetSearchFailure> {
        match self.step {
            SearchStep::Failed(failure) => Some(failure),
            _ => None,
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxDcPwdetSearchCompletion,
    ) -> Result<(), PhyTxDcPwdetSearchTransitionError> {
        match (self.step, completion) {
            (
                SearchStep::ProgramDco { transaction, kind },
                PhyTxDcPwdetSearchCompletion::PbusCompleted(completed),
            ) if completed == self.dco_transaction(transaction) => {
                self.step = if transaction == 3 {
                    SearchStep::Delay(kind)
                } else {
                    SearchStep::ProgramDco {
                        transaction: transaction + 1,
                        kind,
                    }
                };
            }
            (
                SearchStep::ProgramDco { transaction, .. },
                PhyTxDcPwdetSearchCompletion::PbusTimedOut(completed),
            ) if completed == self.dco_transaction(transaction) => {
                self.step = SearchStep::Failed(PhyTxDcPwdetSearchFailure::PbusTimedOut(completed));
            }
            (
                SearchStep::CommitDco { transaction },
                PhyTxDcPwdetSearchCompletion::PbusCompleted(completed),
            ) if completed == self.dco_transaction(transaction) => {
                self.step = if transaction == 3 {
                    SearchStep::Complete
                } else {
                    SearchStep::CommitDco {
                        transaction: transaction + 1,
                    }
                };
            }
            (
                SearchStep::CommitDco { transaction },
                PhyTxDcPwdetSearchCompletion::PbusTimedOut(completed),
            ) if completed == self.dco_transaction(transaction) => {
                self.step = SearchStep::Failed(PhyTxDcPwdetSearchFailure::PbusTimedOut(completed));
            }
            (
                SearchStep::Delay(kind),
                PhyTxDcPwdetSearchCompletion::DelayElapsed {
                    identity,
                    component,
                    measurement,
                    micros: 10,
                },
            ) if identity == self.request.identity
                && component == self.component
                && measurement == self.total_measurements =>
            {
                self.step = SearchStep::Measure {
                    kind,
                    transition: self.tone_sar(),
                };
            }
            (
                SearchStep::Measure {
                    kind,
                    mut transition,
                },
                PhyTxDcPwdetSearchCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxDcPwdetSearchTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        self.total_measurements = self.total_measurements.wrapping_add(1);
                        match kind {
                            MeasurementKind::PrecheckPositive => {
                                self.positive_sample = outcome.sample;
                                self.set_component_delta(-(self.negative_step as i16));
                                self.program(MeasurementKind::PrecheckNegative);
                            }
                            MeasurementKind::PrecheckNegative => {
                                self.after_precheck(outcome.sample);
                            }
                            MeasurementKind::Scan => self.scan_sample(outcome.sample),
                        }
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.step = SearchStep::Failed(PhyTxDcPwdetSearchFailure::ToneSar(failure));
                    }
                    _ => self.step = SearchStep::Measure { kind, transition },
                }
            }
            (SearchStep::Complete | SearchStep::Failed(_), _) => {
                return Err(PhyTxDcPwdetSearchTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxDcPwdetSearchTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetParameters {
    pub dco: [[u16; 4]; 3],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetOutcome {
    pub dco: [[u16; 4]; 3],
    pub total_measurements: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetFailure {
    PbusTimedOut(PhyPbusForceTest),
    Search(PhyTxDcPwdetSearchFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetDelayPhase {
    InitialTone,
    WorkMode,
    WorkModePulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetAction {
    CaptureRegisters,
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigurePowerDetector,
    ConfigurePbusDebugMode,
    ReadPbus {
        selector: u8,
        path: u8,
    },
    ForcePbus(PhyPbusForceTest),
    ConfigureTone {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    DelayMicros {
        phase: PhyTxDcPwdetDelayPhase,
        micros: u32,
    },
    ConfigureSarCalibration,
    Search(PhyTxDcPwdetSearchAction),
    ConfigurePbusWorkMode,
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    RestoreRegisters {
        power_table_low: u8,
        power_control_field: u32,
    },
    Complete(PhyTxDcPwdetOutcome),
    Failed(PhyTxDcPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetCompletion {
    RegistersCaptured {
        power_table_low: u8,
        power_control_field: u32,
    },
    TxClockConfigured {
        enabled: bool,
    },
    PowerDetectorConfigured,
    PbusDebugModeConfigured,
    PbusRead {
        selector: u8,
        path: u8,
        value: u16,
    },
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    ToneConfigured {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    DelayElapsed {
        phase: PhyTxDcPwdetDelayPhase,
        micros: u32,
    },
    SarCalibrationConfigured,
    Search(PhyTxDcPwdetSearchCompletion),
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
    RegistersRestored {
        power_table_low: u8,
        power_control_field: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootTerminal {
    Complete,
    Failed(PhyTxDcPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free calibration root retains one bounded search transition"
)]
enum RootStep {
    Capture,
    ClockOn,
    PowerDetector,
    Debug,
    TxOff { index: u8 },
    RxOff { index: u8 },
    ToneOn,
    InitialDelay,
    TxOn { index: u8 },
    BluetoothReadForcedPath,
    BluetoothForcePath { value: u16 },
    BluetoothForceTxPath,
    Sar,
    Gain,
    Search(PhyTxDcPwdetSearchTransition),
    CleanupDco { index: u8, terminal: RootTerminal },
    CleanupTxOff { index: u8, terminal: RootTerminal },
    ToneOff(RootTerminal),
    WorkMode(RootTerminal),
    WorkModeDelay(RootTerminal),
    WorkModePulse(RootTerminal),
    WorkModePulseDelay(RootTerminal),
    WorkModePulseClear(RootTerminal),
    ClockOff(RootTerminal),
    Restore(RootTerminal),
    Complete,
    Failed(PhyTxDcPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetTransition {
    parameters: PhyTxDcPwdetParameters,
    mode: PhyTxDcPwdetMode,
    step: RootStep,
    dco: [[u16; 4]; 3],
    row: u8,
    total_measurements: u16,
    saved_power_table_low: u8,
    saved_power_control_field: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxDcPwdetMode {
    Wifi,
    Bluetooth { tx_path_value: u8 },
}

const fn tx_off(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(5, 1, 0),
        2 => PhyPbusForceTest::new(1, 1, 0),
        3 => PhyPbusForceTest::new(1, 2, 0),
        _ => PhyPbusForceTest::new(0, 1, 0),
    }
}

const fn rx_off(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(1, 1, 0),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

const fn tx_on(index: u8) -> PhyPbusForceTest {
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
        _ => PhyPbusForceTest::new(5, 1, 0x1ef),
    }
}

const fn default_dco(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(2, 1, DEFAULT_DCO[0]),
        1 => PhyPbusForceTest::new(3, 1, DEFAULT_DCO[1]),
        2 => PhyPbusForceTest::new(2, 2, DEFAULT_DCO[2]),
        _ => PhyPbusForceTest::new(3, 2, DEFAULT_DCO[3]),
    }
}

impl PhyTxDcPwdetTransition {
    pub const fn new(parameters: PhyTxDcPwdetParameters) -> Self {
        Self::new_with_mode(parameters, PhyTxDcPwdetMode::Wifi)
    }

    /// Build the Bluetooth use of archive `phy_txdc_cal_pwdet_init(1, 0, 1)`.
    ///
    /// The common search remains shared, while the Bluetooth-only PBus read
    /// and TX-path force stay explicit in the action graph.
    pub const fn new_bluetooth(parameters: PhyTxDcPwdetParameters, tx_path_value: u8) -> Self {
        Self::new_with_mode(parameters, PhyTxDcPwdetMode::Bluetooth { tx_path_value })
    }

    const fn new_with_mode(parameters: PhyTxDcPwdetParameters, mode: PhyTxDcPwdetMode) -> Self {
        Self {
            parameters,
            mode,
            step: RootStep::Capture,
            dco: parameters.dco,
            row: 0,
            total_measurements: 0,
            saved_power_table_low: 0,
            saved_power_control_field: 0,
        }
    }

    fn cleanup(&mut self, terminal: RootTerminal) {
        self.step = RootStep::CleanupDco { index: 0, terminal };
    }

    fn fail(&mut self, failure: PhyTxDcPwdetFailure) {
        self.cleanup(RootTerminal::Failed(failure));
    }

    const fn outcome(&self) -> PhyTxDcPwdetOutcome {
        PhyTxDcPwdetOutcome {
            dco: self.dco,
            total_measurements: self.total_measurements,
        }
    }

    pub const fn action(&self) -> PhyTxDcPwdetAction {
        match self.step {
            RootStep::Capture => PhyTxDcPwdetAction::CaptureRegisters,
            RootStep::ClockOn => PhyTxDcPwdetAction::ConfigureTxClock { enabled: true },
            RootStep::PowerDetector => PhyTxDcPwdetAction::ConfigurePowerDetector,
            RootStep::Debug => PhyTxDcPwdetAction::ConfigurePbusDebugMode,
            RootStep::BluetoothReadForcedPath => PhyTxDcPwdetAction::ReadPbus {
                selector: 1,
                path: 1,
            },
            RootStep::BluetoothForcePath { value } => {
                PhyTxDcPwdetAction::ForcePbus(PhyPbusForceTest::new(1, 1, value | 2))
            }
            RootStep::BluetoothForceTxPath => {
                let PhyTxDcPwdetMode::Bluetooth { tx_path_value } = self.mode else {
                    unreachable!()
                };
                PhyTxDcPwdetAction::ForcePbus(PhyPbusForceTest::new(
                    4,
                    2,
                    (tx_path_value as u16) << 3,
                ))
            }
            RootStep::TxOff { index } | RootStep::CleanupTxOff { index, .. } => {
                PhyTxDcPwdetAction::ForcePbus(tx_off(index))
            }
            RootStep::RxOff { index } => PhyTxDcPwdetAction::ForcePbus(rx_off(index)),
            RootStep::ToneOn => PhyTxDcPwdetAction::ConfigureTone {
                enabled: true,
                selector: 0x200,
                attenuation: 0x78,
            },
            RootStep::InitialDelay => PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::InitialTone,
                micros: 1,
            },
            RootStep::TxOn { index } => PhyTxDcPwdetAction::ForcePbus(tx_on(index)),
            RootStep::Sar => PhyTxDcPwdetAction::ConfigureSarCalibration,
            RootStep::Gain => PhyTxDcPwdetAction::ForcePbus(PhyPbusForceTest::new(
                1,
                2,
                TX_BB_GAIN[self.row as usize],
            )),
            RootStep::Search(transition) => PhyTxDcPwdetAction::Search(transition.action()),
            RootStep::CleanupDco { index, .. } => PhyTxDcPwdetAction::ForcePbus(default_dco(index)),
            RootStep::ToneOff(_) => PhyTxDcPwdetAction::ConfigureTone {
                enabled: false,
                selector: 0x80,
                attenuation: 0x78,
            },
            RootStep::WorkMode(_) => PhyTxDcPwdetAction::ConfigurePbusWorkMode,
            RootStep::WorkModeDelay(_) => PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::WorkMode,
                micros: 1,
            },
            RootStep::WorkModePulse(_) => PhyTxDcPwdetAction::ConfigurePbusWorkModePulse,
            RootStep::WorkModePulseDelay(_) => {
                // SOURCE: complete rev0 ROM
                // `esp32s31_rev0_rom.elf::phy_pbus_force_mode(0)`
                // holds the second work-mode pulse for two microseconds. This
                // cleanup is reached by archive `phy_txdc_cal_pwdet_init`.
                PhyTxDcPwdetAction::DelayMicros {
                    phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                    micros: 2,
                }
            }
            RootStep::WorkModePulseClear(_) => PhyTxDcPwdetAction::ClearPbusWorkModePulse,
            RootStep::ClockOff(_) => PhyTxDcPwdetAction::ConfigureTxClock { enabled: false },
            RootStep::Restore(_) => PhyTxDcPwdetAction::RestoreRegisters {
                power_table_low: self.saved_power_table_low,
                power_control_field: self.saved_power_control_field,
            },
            RootStep::Complete => PhyTxDcPwdetAction::Complete(self.outcome()),
            RootStep::Failed(failure) => PhyTxDcPwdetAction::Failed(failure),
        }
    }

    fn pbus_completion(
        &mut self,
        completed: PhyPbusForceTest,
        timed_out: bool,
    ) -> Result<(), PhyTxDcPwdetTransitionError> {
        let (expected, next) = match self.step {
            RootStep::TxOff { index } => (
                tx_off(index),
                if index == 4 {
                    RootStep::RxOff { index: 0 }
                } else {
                    RootStep::TxOff { index: index + 1 }
                },
            ),
            RootStep::RxOff { index } => (
                rx_off(index),
                if index == 2 {
                    RootStep::ToneOn
                } else {
                    RootStep::RxOff { index: index + 1 }
                },
            ),
            RootStep::TxOn { index } => (
                tx_on(index),
                if index == 10 {
                    match self.mode {
                        PhyTxDcPwdetMode::Wifi => RootStep::Sar,
                        PhyTxDcPwdetMode::Bluetooth { .. } => RootStep::BluetoothReadForcedPath,
                    }
                } else {
                    RootStep::TxOn { index: index + 1 }
                },
            ),
            RootStep::BluetoothForcePath { value } => (
                PhyPbusForceTest::new(1, 1, value | 2),
                RootStep::BluetoothForceTxPath,
            ),
            RootStep::BluetoothForceTxPath => {
                let PhyTxDcPwdetMode::Bluetooth { tx_path_value } = self.mode else {
                    return Err(PhyTxDcPwdetTransitionError::WrongCompletion);
                };
                (
                    PhyPbusForceTest::new(4, 2, u16::from(tx_path_value) << 3),
                    RootStep::Sar,
                )
            }
            RootStep::Gain => (
                PhyPbusForceTest::new(1, 2, TX_BB_GAIN[self.row as usize]),
                RootStep::Search(PhyTxDcPwdetSearchTransition::new(
                    PhyTxDcPwdetSearchRequest {
                        identity: self.row,
                        initial: self.dco[self.row as usize],
                        clear_tone_after_ready: self.parameters.clear_tone_after_ready,
                    },
                )),
            ),
            RootStep::CleanupDco { index, terminal } => (
                default_dco(index),
                if index == 3 {
                    RootStep::CleanupTxOff { index: 0, terminal }
                } else {
                    RootStep::CleanupDco {
                        index: index + 1,
                        terminal,
                    }
                },
            ),
            RootStep::CleanupTxOff { index, terminal } => (
                tx_off(index),
                if index == 4 {
                    RootStep::ToneOff(terminal)
                } else {
                    RootStep::CleanupTxOff {
                        index: index + 1,
                        terminal,
                    }
                },
            ),
            _ => return Err(PhyTxDcPwdetTransitionError::WrongCompletion),
        };
        if completed != expected {
            return Err(PhyTxDcPwdetTransitionError::WrongCompletion);
        }
        if timed_out {
            match self.step {
                RootStep::CleanupDco { .. } | RootStep::CleanupTxOff { .. } => {
                    // Cleanup is best effort, but retains a previously
                    // selected terminal outcome.
                    self.step = next;
                }
                _ => self.fail(PhyTxDcPwdetFailure::PbusTimedOut(completed)),
            }
        } else {
            self.step = next;
        }
        Ok(())
    }

    pub fn advance(
        &mut self,
        completion: PhyTxDcPwdetCompletion,
    ) -> Result<(), PhyTxDcPwdetTransitionError> {
        match (self.step, completion) {
            (
                RootStep::Capture,
                PhyTxDcPwdetCompletion::RegistersCaptured {
                    power_table_low,
                    power_control_field,
                },
            ) => {
                self.saved_power_table_low = power_table_low;
                self.saved_power_control_field = power_control_field;
                self.step = RootStep::ClockOn;
            }
            (RootStep::ClockOn, PhyTxDcPwdetCompletion::TxClockConfigured { enabled: true }) => {
                self.step = RootStep::PowerDetector
            }
            (RootStep::PowerDetector, PhyTxDcPwdetCompletion::PowerDetectorConfigured) => {
                self.step = RootStep::Debug;
            }
            (RootStep::Debug, PhyTxDcPwdetCompletion::PbusDebugModeConfigured) => {
                self.step = RootStep::TxOff { index: 0 };
            }
            (
                RootStep::ToneOn,
                PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled: true,
                    selector: 0x200,
                    attenuation: 0x78,
                },
            ) => self.step = RootStep::InitialDelay,
            (
                RootStep::InitialDelay,
                PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::InitialTone,
                    micros: 1,
                },
            ) => self.step = RootStep::TxOn { index: 0 },
            (RootStep::Sar, PhyTxDcPwdetCompletion::SarCalibrationConfigured) => {
                self.step = RootStep::Gain;
            }
            (
                RootStep::BluetoothReadForcedPath,
                PhyTxDcPwdetCompletion::PbusRead {
                    selector: 1,
                    path: 1,
                    value,
                },
            ) => self.step = RootStep::BluetoothForcePath { value },
            (RootStep::Search(mut transition), PhyTxDcPwdetCompletion::Search(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxDcPwdetTransitionError::WrongCompletion)?;
                if let Some(failure) = transition.failure() {
                    self.fail(PhyTxDcPwdetFailure::Search(failure));
                } else {
                    match transition.action() {
                        PhyTxDcPwdetSearchAction::Complete(outcome) => {
                            self.dco[self.row as usize] = outcome.dco;
                            self.total_measurements = self
                                .total_measurements
                                .wrapping_add(u16::from(outcome.measurements));
                            self.row += 1;
                            if self.row == 3 {
                                self.cleanup(RootTerminal::Complete);
                            } else {
                                self.step = RootStep::Gain;
                            }
                        }
                        _ => self.step = RootStep::Search(transition),
                    }
                }
            }
            (
                RootStep::ToneOff(terminal),
                PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled: false,
                    selector: 0x80,
                    attenuation: 0x78,
                },
            ) => self.step = RootStep::WorkMode(terminal),
            (
                RootStep::WorkMode(terminal),
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => self.step = RootStep::ClockOff(terminal),
            (
                RootStep::WorkMode(terminal),
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => self.step = RootStep::WorkModeDelay(terminal),
            (
                RootStep::WorkModeDelay(terminal),
                PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::WorkMode,
                    micros: 1,
                },
            ) => self.step = RootStep::WorkModePulse(terminal),
            (
                RootStep::WorkModePulse(terminal),
                PhyTxDcPwdetCompletion::PbusWorkModePulseConfigured,
            ) => self.step = RootStep::WorkModePulseDelay(terminal),
            (
                RootStep::WorkModePulseDelay(terminal),
                PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                    micros: 2,
                },
            ) => self.step = RootStep::WorkModePulseClear(terminal),
            (
                RootStep::WorkModePulseClear(terminal),
                PhyTxDcPwdetCompletion::PbusWorkModePulseCleared,
            ) => self.step = RootStep::ClockOff(terminal),
            (
                RootStep::ClockOff(terminal),
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled: false },
            ) => self.step = RootStep::Restore(terminal),
            (
                RootStep::Restore(terminal),
                PhyTxDcPwdetCompletion::RegistersRestored {
                    power_table_low,
                    power_control_field,
                },
            ) if power_table_low == self.saved_power_table_low
                && power_control_field == self.saved_power_control_field =>
            {
                self.step = match terminal {
                    RootTerminal::Complete => RootStep::Complete,
                    RootTerminal::Failed(failure) => RootStep::Failed(failure),
                };
            }
            (
                RootStep::TxOff { .. }
                | RootStep::RxOff { .. }
                | RootStep::TxOn { .. }
                | RootStep::BluetoothForcePath { .. }
                | RootStep::BluetoothForceTxPath
                | RootStep::Gain
                | RootStep::CleanupDco { .. }
                | RootStep::CleanupTxOff { .. },
                PhyTxDcPwdetCompletion::PbusCompleted(transaction),
            ) => return self.pbus_completion(transaction, false),
            (
                RootStep::TxOff { .. }
                | RootStep::RxOff { .. }
                | RootStep::TxOn { .. }
                | RootStep::BluetoothForcePath { .. }
                | RootStep::BluetoothForceTxPath
                | RootStep::Gain
                | RootStep::CleanupDco { .. }
                | RootStep::CleanupTxOff { .. },
                PhyTxDcPwdetCompletion::PbusTimedOut(transaction),
            ) => return self.pbus_completion(transaction, true),
            (RootStep::Complete | RootStep::Failed(_), _) => {
                return Err(PhyTxDcPwdetTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxDcPwdetTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetBindingError {
    NotDirectMmio,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetMmioBinding {
    action: PhyTxDcPwdetAction,
}

impl PhyTxDcPwdetMmioBinding {
    pub fn new(action: PhyTxDcPwdetAction) -> Result<Self, PhyTxDcPwdetBindingError> {
        match action {
            PhyTxDcPwdetAction::CaptureRegisters
            | PhyTxDcPwdetAction::ConfigureTxClock { .. }
            | PhyTxDcPwdetAction::ConfigurePowerDetector
            | PhyTxDcPwdetAction::ConfigurePbusDebugMode
            | PhyTxDcPwdetAction::ReadPbus { .. }
            | PhyTxDcPwdetAction::ConfigureTone { .. }
            | PhyTxDcPwdetAction::ConfigureSarCalibration
            | PhyTxDcPwdetAction::ConfigurePbusWorkMode
            | PhyTxDcPwdetAction::ConfigurePbusWorkModePulse
            | PhyTxDcPwdetAction::ClearPbusWorkModePulse
            | PhyTxDcPwdetAction::RestoreRegisters { .. } => Ok(Self { action }),
            _ => Err(PhyTxDcPwdetBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<
        P: open_esp_radio_esp32s31_hal::power_detector_platform::PhyPowerDetectorPlatformControl,
    >(
        self,
        platform: &mut P,
        registers: &mut open_esp_radio_esp32s31_hal::PhyHal,
    ) -> PhyTxDcPwdetCompletion {
        match self.action {
            PhyTxDcPwdetAction::CaptureRegisters => {
                let (power_table_low, power_control_field) =
                    open_esp_radio_esp32s31_hal::phy_power_detector::capture_txdc_fields(registers);
                PhyTxDcPwdetCompletion::RegistersCaptured {
                    power_table_low,
                    power_control_field,
                }
            }
            PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                open_esp_radio_esp32s31_hal::pbus::configure_tx_clock(registers, enabled);
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
            }
            PhyTxDcPwdetAction::ConfigurePowerDetector => {
                open_esp_radio_esp32s31_hal::phy_power_detector::configure_enabled(
                    platform, registers,
                );
                PhyTxDcPwdetCompletion::PowerDetectorConfigured
            }
            PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                open_esp_radio_esp32s31_hal::pbus::configure_debug_mode(registers);
                PhyTxDcPwdetCompletion::PbusDebugModeConfigured
            }
            PhyTxDcPwdetAction::ReadPbus { selector, path } => PhyTxDcPwdetCompletion::PbusRead {
                selector,
                path,
                value: {
                    let result =
                        open_esp_radio_esp32s31_hal::pbus::read_result(registers, selector, path);
                    debug_assert!(
                        result.is_some(),
                        "TX-DC PWDET transition emitted an unrecovered PBus selector"
                    );
                    result.unwrap_or(0)
                },
            },
            PhyTxDcPwdetAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => {
                crate::phy_hardware::configure_phy_calibration_tone_wide(
                    registers,
                    enabled,
                    selector,
                    attenuation,
                );
                PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled,
                    selector,
                    attenuation,
                }
            }
            PhyTxDcPwdetAction::ConfigureSarCalibration => {
                open_esp_radio_esp32s31_hal::phy_power_detector::configure_txdc_sar(registers);
                PhyTxDcPwdetCompletion::SarCalibrationConfigured
            }
            PhyTxDcPwdetAction::ConfigurePbusWorkMode => {
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: open_esp_radio_esp32s31_hal::pbus::configure_work_mode(
                        registers,
                    ),
                }
            }
            PhyTxDcPwdetAction::ConfigurePbusWorkModePulse => {
                open_esp_radio_esp32s31_hal::phy_agc::configure_pbus_work_mode_pulse(registers);
                PhyTxDcPwdetCompletion::PbusWorkModePulseConfigured
            }
            PhyTxDcPwdetAction::ClearPbusWorkModePulse => {
                open_esp_radio_esp32s31_hal::phy_agc::clear_pbus_work_mode_pulse(registers);
                PhyTxDcPwdetCompletion::PbusWorkModePulseCleared
            }
            PhyTxDcPwdetAction::RestoreRegisters {
                power_table_low,
                power_control_field,
            } => {
                open_esp_radio_esp32s31_hal::phy_power_detector::restore_txdc_fields(
                    registers,
                    power_table_low,
                    power_control_field,
                );
                PhyTxDcPwdetCompletion::RegistersRestored {
                    power_table_low,
                    power_control_field,
                }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetExternalBindingError {
    UnsupportedAction,
    Pbus(crate::phy_pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::phy_pbus::PhyPbusHardwareBinding,
}

impl PhyTxDcPwdetSearchPbusBinding {
    pub fn new(action: PhyTxDcPwdetSearchAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        let PhyTxDcPwdetSearchAction::ForcePbus(transaction) = action else {
            return Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction);
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
        registers: &mut open_esp_radio_esp32s31_hal::PhyHal,
    ) -> Result<(), crate::phy_pbus::PhyPbusHardwareBindingError> {
        self.hardware.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut open_esp_radio_esp32s31_hal::PhyHal,
    ) -> Result<
        crate::phy_pbus::PhyPbusHardwareObservation,
        crate::phy_pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyTxDcPwdetSearchCompletion, PhyTxDcPwdetExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxDcPwdetSearchCompletion::PbusCompleted)
            .map_err(PhyTxDcPwdetExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyTxDcPwdetSearchCompletion {
        PhyTxDcPwdetSearchCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchTimerBinding {
    identity: u8,
    component: u8,
    measurement: u8,
    micros: u32,
}

impl PhyTxDcPwdetSearchTimerBinding {
    pub fn new(action: PhyTxDcPwdetSearchAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        match action {
            PhyTxDcPwdetSearchAction::DelayMicros {
                identity,
                component,
                measurement,
                micros,
            } => Ok(Self {
                identity,
                component,
                measurement,
                micros,
            }),
            _ => Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxDcPwdetSearchCompletion {
        PhyTxDcPwdetSearchCompletion::DelayElapsed {
            identity: self.identity,
            component: self.component,
            measurement: self.measurement,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchExternalBinding {
    Pbus(PhyTxDcPwdetSearchPbusBinding),
    Timer(PhyTxDcPwdetSearchTimerBinding),
    ToneSar(crate::phy_tx_cal::PhyToneSarExternalBinding),
}

impl PhyTxDcPwdetSearchExternalBinding {
    pub fn lower(
        action: PhyTxDcPwdetSearchAction,
    ) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        match action {
            PhyTxDcPwdetSearchAction::ForcePbus(_) => {
                PhyTxDcPwdetSearchPbusBinding::new(action).map(Self::Pbus)
            }
            PhyTxDcPwdetSearchAction::DelayMicros { .. } => {
                PhyTxDcPwdetSearchTimerBinding::new(action).map(Self::Timer)
            }
            PhyTxDcPwdetSearchAction::ToneSar(action) => {
                crate::phy_tx_cal::PhyToneSarExternalBinding::lower(action)
                    .map(Self::ToneSar)
                    .map_err(|_| PhyTxDcPwdetExternalBindingError::UnsupportedAction)
            }
            PhyTxDcPwdetSearchAction::Complete(_) | PhyTxDcPwdetSearchAction::Failed(_) => {
                Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::phy_pbus::PhyPbusHardwareBinding,
}

impl PhyTxDcPwdetPbusBinding {
    pub fn new(action: PhyTxDcPwdetAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        let PhyTxDcPwdetAction::ForcePbus(transaction) = action else {
            return Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction);
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
        registers: &mut open_esp_radio_esp32s31_hal::PhyHal,
    ) -> Result<(), crate::phy_pbus::PhyPbusHardwareBindingError> {
        self.hardware.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut open_esp_radio_esp32s31_hal::PhyHal,
    ) -> Result<
        crate::phy_pbus::PhyPbusHardwareObservation,
        crate::phy_pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyTxDcPwdetCompletion, PhyTxDcPwdetExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxDcPwdetCompletion::PbusCompleted)
            .map_err(PhyTxDcPwdetExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyTxDcPwdetCompletion {
        PhyTxDcPwdetCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetTimerBinding {
    phase: PhyTxDcPwdetDelayPhase,
    micros: u32,
}

impl PhyTxDcPwdetTimerBinding {
    pub fn new(action: PhyTxDcPwdetAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        match action {
            PhyTxDcPwdetAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxDcPwdetCompletion {
        PhyTxDcPwdetCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_txdc_cal_pwdet_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetExternalBinding {
    Mmio(PhyTxDcPwdetMmioBinding),
    Pbus(PhyTxDcPwdetPbusBinding),
    Timer(PhyTxDcPwdetTimerBinding),
    Search(PhyTxDcPwdetSearchExternalBinding),
}

impl PhyTxDcPwdetExternalBinding {
    pub fn lower(action: PhyTxDcPwdetAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        if let Ok(binding) = PhyTxDcPwdetMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyTxDcPwdetPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyTxDcPwdetTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        if let PhyTxDcPwdetAction::Search(action) = action {
            return PhyTxDcPwdetSearchExternalBinding::lower(action).map(Self::Search);
        }
        Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                value,
            },
            terminal => panic!("unexpected terminal action {terminal:?}"),
        }
    }

    #[test]
    fn child_scan_is_finite_and_commits_only_selected_dco_pair() {
        let initial = [1, 2, 0x100, 0x100];
        let mut transition = PhyTxDcPwdetSearchTransition::new(PhyTxDcPwdetSearchRequest {
            identity: 0,
            initial,
            clear_tone_after_ready: false,
        });
        let mut samples = 0_u16;
        loop {
            let completion = match transition.action() {
                PhyTxDcPwdetSearchAction::ForcePbus(transaction) => {
                    PhyTxDcPwdetSearchCompletion::PbusCompleted(transaction)
                }
                PhyTxDcPwdetSearchAction::DelayMicros {
                    identity,
                    component,
                    measurement,
                    micros,
                } => PhyTxDcPwdetSearchCompletion::DelayElapsed {
                    identity,
                    component,
                    measurement,
                    micros,
                },
                PhyTxDcPwdetSearchAction::ToneSar(action) => {
                    if matches!(action, PhyToneSarAction::ReadSar { .. }) {
                        samples += 1;
                    }
                    PhyTxDcPwdetSearchCompletion::ToneSar(tone_sar_completion(action, 100))
                }
                PhyTxDcPwdetSearchAction::Complete(outcome) => {
                    assert_eq!(outcome.dco[0..2], initial[0..2]);
                    assert!(outcome.measurements <= 208);
                    break;
                }
                PhyTxDcPwdetSearchAction::Failed(failure) => {
                    panic!("unexpected failure {failure:?}")
                }
            };
            transition.advance(completion).unwrap();
        }
        assert!(samples <= 416);
    }

    #[test]
    fn root_gain_sequence_matches_wifi_indices() {
        assert_eq!(TX_BB_GAIN, [0, 0x80, 0x100]);
        for (index, expected) in TX_BB_GAIN.into_iter().enumerate() {
            assert_eq!(
                PhyPbusForceTest::new(1, 2, expected),
                PhyPbusForceTest::new(1, 2, TX_BB_GAIN[index])
            );
        }
    }

    #[test]
    fn bluetooth_mode_reads_and_forces_the_bluetooth_tx_path_before_sar_setup() {
        let mut transition = PhyTxDcPwdetTransition::new_bluetooth(
            PhyTxDcPwdetParameters {
                dco: [[0; 4]; 3],
                clear_tone_after_ready: false,
            },
            0x12,
        );

        loop {
            let completion = match transition.action() {
                PhyTxDcPwdetAction::CaptureRegisters => PhyTxDcPwdetCompletion::RegistersCaptured {
                    power_table_low: 0,
                    power_control_field: 0,
                },
                PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                    PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
                }
                PhyTxDcPwdetAction::ConfigurePowerDetector => {
                    PhyTxDcPwdetCompletion::PowerDetectorConfigured
                }
                PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                    PhyTxDcPwdetCompletion::PbusDebugModeConfigured
                }
                PhyTxDcPwdetAction::ForcePbus(transaction) => {
                    PhyTxDcPwdetCompletion::PbusCompleted(transaction)
                }
                PhyTxDcPwdetAction::ConfigureTone {
                    enabled,
                    selector,
                    attenuation,
                } => PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled,
                    selector,
                    attenuation,
                },
                PhyTxDcPwdetAction::DelayMicros { phase, micros } => {
                    PhyTxDcPwdetCompletion::DelayElapsed { phase, micros }
                }
                PhyTxDcPwdetAction::ReadPbus { selector, path } => {
                    assert_eq!((selector, path), (1, 1));
                    transition
                        .advance(PhyTxDcPwdetCompletion::PbusRead {
                            selector,
                            path,
                            value: 0x34,
                        })
                        .unwrap();
                    break;
                }
                action => panic!("unexpected Bluetooth prefix action {action:?}"),
            };
            transition.advance(completion).unwrap();
        }

        let forced_path = PhyPbusForceTest::new(1, 1, 0x36);
        assert_eq!(
            transition.action(),
            PhyTxDcPwdetAction::ForcePbus(forced_path)
        );
        transition
            .advance(PhyTxDcPwdetCompletion::PbusCompleted(forced_path))
            .unwrap();

        let forced_tx_path = PhyPbusForceTest::new(4, 2, 0x90);
        assert_eq!(
            transition.action(),
            PhyTxDcPwdetAction::ForcePbus(forced_tx_path)
        );
        transition
            .advance(PhyTxDcPwdetCompletion::PbusCompleted(forced_tx_path))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyTxDcPwdetAction::ConfigureSarCalibration
        );
    }

    #[test]
    fn root_cleanup_uses_the_complete_rom_work_mode_pulse() {
        let mut transition = PhyTxDcPwdetTransition::new(PhyTxDcPwdetParameters {
            dco: [[0; 4]; 3],
            clear_tone_after_ready: false,
        });
        let mut inject_initial_failure = true;
        loop {
            let completion = match transition.action() {
                PhyTxDcPwdetAction::CaptureRegisters => PhyTxDcPwdetCompletion::RegistersCaptured {
                    power_table_low: 0,
                    power_control_field: 0,
                },
                PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                    PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
                }
                PhyTxDcPwdetAction::ConfigurePowerDetector => {
                    PhyTxDcPwdetCompletion::PowerDetectorConfigured
                }
                PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                    PhyTxDcPwdetCompletion::PbusDebugModeConfigured
                }
                PhyTxDcPwdetAction::ForcePbus(transaction) if inject_initial_failure => {
                    inject_initial_failure = false;
                    PhyTxDcPwdetCompletion::PbusTimedOut(transaction)
                }
                PhyTxDcPwdetAction::ForcePbus(transaction) => {
                    PhyTxDcPwdetCompletion::PbusCompleted(transaction)
                }
                PhyTxDcPwdetAction::ConfigureTone {
                    enabled,
                    selector,
                    attenuation,
                } => PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled,
                    selector,
                    attenuation,
                },
                PhyTxDcPwdetAction::ConfigurePbusWorkMode => {
                    PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                        settle_required: true,
                    }
                }
                PhyTxDcPwdetAction::DelayMicros {
                    phase: PhyTxDcPwdetDelayPhase::WorkMode,
                    micros,
                } => PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::WorkMode,
                    micros,
                },
                PhyTxDcPwdetAction::ConfigurePbusWorkModePulse => {
                    PhyTxDcPwdetCompletion::PbusWorkModePulseConfigured
                }
                PhyTxDcPwdetAction::DelayMicros {
                    phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                    micros,
                } => {
                    assert_eq!(micros, 2);
                    break;
                }
                action => panic!("unexpected cleanup action {action:?}"),
            };
            transition.advance(completion).unwrap();
        }
    }

    #[test]
    fn external_lowering_covers_root_and_search_operation_classes() {
        let transaction = PhyPbusForceTest::new(1, 2, 0x80);
        assert!(matches!(
            PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::CaptureRegisters),
            Ok(PhyTxDcPwdetExternalBinding::Mmio(_))
        ));
        assert!(matches!(
            PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::ForcePbus(transaction)),
            Ok(PhyTxDcPwdetExternalBinding::Pbus(_))
        ));
        assert!(matches!(
            PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::InitialTone,
                micros: 1,
            }),
            Ok(PhyTxDcPwdetExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::Search(
                PhyTxDcPwdetSearchAction::ForcePbus(transaction)
            )),
            Ok(PhyTxDcPwdetExternalBinding::Search(
                PhyTxDcPwdetSearchExternalBinding::Pbus(_)
            ))
        ));
        assert!(matches!(
            PhyTxDcPwdetSearchExternalBinding::lower(PhyTxDcPwdetSearchAction::DelayMicros {
                identity: 1,
                component: 2,
                measurement: 3,
                micros: 2,
            }),
            Ok(PhyTxDcPwdetSearchExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyTxDcPwdetSearchExternalBinding::lower(PhyTxDcPwdetSearchAction::ToneSar(
                PhyToneSarAction::ClearTone {
                    measurement: 0,
                    sample: 0,
                }
            )),
            Ok(PhyTxDcPwdetSearchExternalBinding::ToneSar(_))
        ));
        assert!(matches!(
            PhyTxDcPwdetExternalBinding::lower(PhyTxDcPwdetAction::Complete(PhyTxDcPwdetOutcome {
                dco: [[0; 4]; 3],
                total_measurements: 0,
            })),
            Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction)
        ));
    }
}
