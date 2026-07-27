//! Rust-owned RX-gain table generation and hardware-memory publication.
//!
//! This module covers the pure ROM `phy_gen_rx_gain_table` child and the
//! complete pinned `phy_wr_rx_gain_mem_new` loop used by
//! `phy_set_rx_gain_table(0x985, 0)`. RX-DC and RX-IQ calibration remain
//! separate typed predecessors; this publisher accepts only copied outcomes.

use crate::{
    phy_bb::{
        generate_phy_rx_gain_table, phy_generated_rx_gain_memory_entry, PhyGainMemoryEntry,
        PhyGeneratedRxGainTable, PhyRxGainBank, PhyRxGainMemoryParameters,
    },
    phy_pbus::PhyPbusForceTest,
    phy_rx_gain_cal::{
        PhyRxGainDcAction, PhyRxGainDcCompletion, PhyRxGainDcFailure, PhyRxGainDcOutcome,
        PhyRxGainDcParameters, PhyRxGainDcTransition,
    },
};

const PBUS_RX_ON_COUNT: u8 = 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainClock {
    Rx,
    Tx,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDelayPhase {
    PbusWorkMode { bank: PhyRxGainBank },
    PbusWorkModePulse { bank: PhyRxGainBank },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishOutcome {
    pub wifi_entries: u8,
    pub shared_entries: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishFailure {
    PbusTimedOut {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishAction {
    ConfigurePbusDebugMode {
        bank: PhyRxGainBank,
    },
    ForcePbus {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
    ConfigureClock {
        bank: PhyRxGainBank,
        clock: PhyRxGainClock,
        enabled: bool,
    },
    ProgramEntry {
        bank: PhyRxGainBank,
        entry: PhyGainMemoryEntry,
    },
    ConfigurePbusWorkMode {
        bank: PhyRxGainBank,
    },
    DelayMicros {
        phase: PhyRxGainDelayPhase,
        micros: u32,
    },
    ConfigurePbusWorkModePulse {
        bank: PhyRxGainBank,
    },
    ClearPbusWorkModePulse {
        bank: PhyRxGainBank,
    },
    Complete(PhyRxGainPublishOutcome),
    Failed(PhyRxGainPublishFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishCompletion {
    PbusDebugModeConfigured {
        bank: PhyRxGainBank,
    },
    PbusCompleted {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
    PbusTimedOut {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
    ClockConfigured {
        bank: PhyRxGainBank,
        clock: PhyRxGainClock,
        enabled: bool,
    },
    EntryProgrammed {
        bank: PhyRxGainBank,
        entry: PhyGainMemoryEntry,
    },
    PbusWorkModeConfigured {
        bank: PhyRxGainBank,
        settle_required: bool,
    },
    DelayElapsed {
        phase: PhyRxGainDelayPhase,
        micros: u32,
    },
    PbusWorkModePulseConfigured {
        bank: PhyRxGainBank,
    },
    PbusWorkModePulseCleared {
        bank: PhyRxGainBank,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishBindingError {
    NotDirectMmio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Debug(PhyRxGainBank),
    RxOn {
        bank: PhyRxGainBank,
        index: u8,
        restore: bool,
    },
    Clock {
        bank: PhyRxGainBank,
        clock: PhyRxGainClock,
        enabled: bool,
    },
    Entries {
        bank: PhyRxGainBank,
        index: u8,
    },
    WorkMode {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    WorkModeDelay {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    WorkModePulse {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    WorkModePulseDelay {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    WorkModePulseClear {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    Complete,
    Failed(PhyRxGainPublishFailure),
}

const fn pbus_rx_on(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
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

const fn next_bank(bank: PhyRxGainBank) -> Step {
    match bank {
        PhyRxGainBank::Wifi => Step::Debug(PhyRxGainBank::Shared),
        PhyRxGainBank::Shared => Step::Complete,
    }
}

const fn after_work_mode(bank: PhyRxGainBank, failure: Option<PhyRxGainPublishFailure>) -> Step {
    match failure {
        Some(failure) => Step::Failed(failure),
        None => next_bank(bank),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishTransition {
    parameters: PhyRxGainMemoryParameters,
    wifi: PhyGeneratedRxGainTable,
    shared: PhyGeneratedRxGainTable,
    step: Step,
}

impl PhyRxGainPublishTransition {
    pub fn new(parameters: PhyRxGainMemoryParameters) -> Self {
        Self {
            parameters,
            wifi: generate_phy_rx_gain_table(PhyRxGainBank::Wifi),
            shared: generate_phy_rx_gain_table(PhyRxGainBank::Shared),
            step: Step::Debug(PhyRxGainBank::Wifi),
        }
    }

    const fn table(&self, bank: PhyRxGainBank) -> &PhyGeneratedRxGainTable {
        match bank {
            PhyRxGainBank::Wifi => &self.wifi,
            PhyRxGainBank::Shared => &self.shared,
        }
    }

    pub fn action(&self) -> PhyRxGainPublishAction {
        match self.step {
            Step::Debug(bank) => PhyRxGainPublishAction::ConfigurePbusDebugMode { bank },
            Step::RxOn {
                bank,
                index,
                restore: _,
            } => PhyRxGainPublishAction::ForcePbus {
                bank,
                transaction: pbus_rx_on(index, self.parameters.parameter_002),
            },
            Step::Clock {
                bank,
                clock,
                enabled,
            } => PhyRxGainPublishAction::ConfigureClock {
                bank,
                clock,
                enabled,
            },
            Step::Entries { bank, index } => PhyRxGainPublishAction::ProgramEntry {
                bank,
                entry: phy_generated_rx_gain_memory_entry(
                    self.parameters,
                    bank,
                    self.table(bank),
                    index,
                ),
            },
            Step::WorkMode { bank, .. } => PhyRxGainPublishAction::ConfigurePbusWorkMode { bank },
            Step::WorkModeDelay { bank, .. } => PhyRxGainPublishAction::DelayMicros {
                phase: PhyRxGainDelayPhase::PbusWorkMode { bank },
                micros: 1,
            },
            Step::WorkModePulse { bank, .. } => {
                PhyRxGainPublishAction::ConfigurePbusWorkModePulse { bank }
            }
            Step::WorkModePulseDelay { bank, .. } => PhyRxGainPublishAction::DelayMicros {
                phase: PhyRxGainDelayPhase::PbusWorkModePulse { bank },
                micros: 1,
            },
            Step::WorkModePulseClear { bank, .. } => {
                PhyRxGainPublishAction::ClearPbusWorkModePulse { bank }
            }
            Step::Complete => PhyRxGainPublishAction::Complete(PhyRxGainPublishOutcome {
                wifi_entries: self.wifi.last_index + 1,
                shared_entries: self.shared.last_index + 1,
            }),
            Step::Failed(failure) => PhyRxGainPublishAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxGainPublishCompletion,
    ) -> Result<(), PhyRxGainPublishTransitionError> {
        self.step = match (self.step, completion) {
            (
                Step::Debug(expected),
                PhyRxGainPublishCompletion::PbusDebugModeConfigured { bank },
            ) if bank == expected => Step::RxOn {
                bank,
                index: 0,
                restore: false,
            },
            (
                Step::RxOn {
                    bank,
                    index,
                    restore,
                },
                PhyRxGainPublishCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank
                && transaction == pbus_rx_on(index, self.parameters.parameter_002) =>
            {
                if index + 1 == PBUS_RX_ON_COUNT && restore {
                    Step::WorkMode {
                        bank,
                        failure: None,
                    }
                } else if index + 1 == PBUS_RX_ON_COUNT {
                    Step::Clock {
                        bank,
                        clock: PhyRxGainClock::Rx,
                        enabled: true,
                    }
                } else {
                    Step::RxOn {
                        bank,
                        index: index + 1,
                        restore,
                    }
                }
            }
            (
                Step::RxOn {
                    bank,
                    index,
                    restore: _,
                },
                PhyRxGainPublishCompletion::PbusTimedOut {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank
                && transaction == pbus_rx_on(index, self.parameters.parameter_002) =>
            {
                Step::WorkMode {
                    bank,
                    failure: Some(PhyRxGainPublishFailure::PbusTimedOut { bank, transaction }),
                }
            }
            (
                Step::Clock {
                    bank,
                    clock: PhyRxGainClock::Rx,
                    enabled: true,
                },
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank: completed_bank,
                    clock: PhyRxGainClock::Rx,
                    enabled: true,
                },
            ) if bank == completed_bank => Step::Clock {
                bank,
                clock: PhyRxGainClock::Tx,
                enabled: true,
            },
            (
                Step::Clock {
                    bank,
                    clock: PhyRxGainClock::Tx,
                    enabled: true,
                },
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank: completed_bank,
                    clock: PhyRxGainClock::Tx,
                    enabled: true,
                },
            ) if bank == completed_bank => Step::Entries { bank, index: 0 },
            (
                Step::Entries { bank, index },
                PhyRxGainPublishCompletion::EntryProgrammed {
                    bank: completed_bank,
                    entry,
                },
            ) if bank == completed_bank
                && entry
                    == phy_generated_rx_gain_memory_entry(
                        self.parameters,
                        bank,
                        self.table(bank),
                        index,
                    ) =>
            {
                if index == self.table(bank).last_index {
                    Step::Clock {
                        bank,
                        clock: PhyRxGainClock::Rx,
                        enabled: false,
                    }
                } else {
                    Step::Entries {
                        bank,
                        index: index + 1,
                    }
                }
            }
            (
                Step::Clock {
                    bank,
                    clock: PhyRxGainClock::Rx,
                    enabled: false,
                },
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank: completed_bank,
                    clock: PhyRxGainClock::Rx,
                    enabled: false,
                },
            ) if bank == completed_bank => Step::Clock {
                bank,
                clock: PhyRxGainClock::Tx,
                enabled: false,
            },
            (
                Step::Clock {
                    bank,
                    clock: PhyRxGainClock::Tx,
                    enabled: false,
                },
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank: completed_bank,
                    clock: PhyRxGainClock::Tx,
                    enabled: false,
                },
            ) if bank == completed_bank => Step::RxOn {
                bank,
                index: 0,
                restore: true,
            },
            (
                Step::WorkMode { bank, failure },
                PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                    bank: completed_bank,
                    settle_required,
                },
            ) if bank == completed_bank => {
                if settle_required {
                    Step::WorkModeDelay { bank, failure }
                } else {
                    after_work_mode(bank, failure)
                }
            }
            (
                Step::WorkModeDelay { bank, failure },
                PhyRxGainPublishCompletion::DelayElapsed {
                    phase:
                        PhyRxGainDelayPhase::PbusWorkMode {
                            bank: completed_bank,
                        },
                    micros: 1,
                },
            ) if bank == completed_bank => Step::WorkModePulse { bank, failure },
            (
                Step::WorkModePulse { bank, failure },
                PhyRxGainPublishCompletion::PbusWorkModePulseConfigured {
                    bank: completed_bank,
                },
            ) if bank == completed_bank => Step::WorkModePulseDelay { bank, failure },
            (
                Step::WorkModePulseDelay { bank, failure },
                PhyRxGainPublishCompletion::DelayElapsed {
                    phase:
                        PhyRxGainDelayPhase::PbusWorkModePulse {
                            bank: completed_bank,
                        },
                    micros: 1,
                },
            ) if bank == completed_bank => Step::WorkModePulseClear { bank, failure },
            (
                Step::WorkModePulseClear { bank, failure },
                PhyRxGainPublishCompletion::PbusWorkModePulseCleared {
                    bank: completed_bank,
                },
            ) if bank == completed_bank => after_work_mode(bank, failure),
            (Step::Complete | Step::Failed(_), _) => {
                return Err(PhyRxGainPublishTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxGainPublishTransitionError::WrongCompletion),
        };

        Ok(())
    }
}

/// Non-cloneable token for one finite non-PBus RX-gain operation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishMmioBinding {
    action: PhyRxGainPublishAction,
}

impl PhyRxGainPublishMmioBinding {
    pub fn new(action: PhyRxGainPublishAction) -> Result<Self, PhyRxGainPublishBindingError> {
        match action {
            PhyRxGainPublishAction::ConfigurePbusDebugMode { .. }
            | PhyRxGainPublishAction::ConfigureClock { .. }
            | PhyRxGainPublishAction::ProgramEntry { .. }
            | PhyRxGainPublishAction::ConfigurePbusWorkMode { .. }
            | PhyRxGainPublishAction::ConfigurePbusWorkModePulse { .. }
            | PhyRxGainPublishAction::ClearPbusWorkModePulse { .. } => Ok(Self { action }),
            _ => Err(PhyRxGainPublishBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(
        self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> PhyRxGainPublishCompletion {
        match self.action {
            PhyRxGainPublishAction::ConfigurePbusDebugMode { bank } => {
                open_esp_radio_hal_esp32s31::pbus::configure_debug_mode(registers);
                PhyRxGainPublishCompletion::PbusDebugModeConfigured { bank }
            }
            PhyRxGainPublishAction::ConfigureClock {
                bank,
                clock,
                enabled,
            } => {
                match clock {
                    PhyRxGainClock::Rx => crate::radio_hal::configure_phy_rx_clock(enabled),
                    PhyRxGainClock::Tx => crate::radio_hal::configure_phy_tx_clock(enabled),
                }
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank,
                    clock,
                    enabled,
                }
            }
            PhyRxGainPublishAction::ProgramEntry { bank, entry } => {
                open_esp_radio_hal_esp32s31::phy_memory::program_gain_memory_entry(
                    registers,
                    [entry.word0, entry.word1, entry.word2],
                    entry.index,
                );
                PhyRxGainPublishCompletion::EntryProgrammed { bank, entry }
            }
            PhyRxGainPublishAction::ConfigurePbusWorkMode { bank } => {
                PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                    bank,
                    settle_required: open_esp_radio_hal_esp32s31::pbus::configure_work_mode(
                        registers,
                    ),
                }
            }
            PhyRxGainPublishAction::ConfigurePbusWorkModePulse { bank } => {
                open_esp_radio_hal_esp32s31::phy_agc::configure_pbus_work_mode_pulse(registers);
                PhyRxGainPublishCompletion::PbusWorkModePulseConfigured { bank }
            }
            PhyRxGainPublishAction::ClearPbusWorkModePulse { bank } => {
                open_esp_radio_hal_esp32s31::phy_agc::clear_pbus_work_mode_pulse(registers);
                PhyRxGainPublishCompletion::PbusWorkModePulseCleared { bank }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainExternalBindingError {
    UnsupportedAction,
    Pbus(crate::phy_pbus::PhyPbusHardwareBindingError),
}

/// Linear owner of one bank-qualified PBus publication edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishPbusBinding {
    bank: PhyRxGainBank,
    transaction: PhyPbusForceTest,
    hardware: crate::phy_pbus::PhyPbusHardwareBinding,
}

impl PhyRxGainPublishPbusBinding {
    pub fn new(action: PhyRxGainPublishAction) -> Result<Self, PhyRxGainExternalBindingError> {
        let PhyRxGainPublishAction::ForcePbus { bank, transaction } = action else {
            return Err(PhyRxGainExternalBindingError::UnsupportedAction);
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
    ) -> Result<PhyRxGainPublishCompletion, PhyRxGainExternalBindingError> {
        let bank = self.bank;
        self.hardware
            .into_transaction()
            .map(|transaction| PhyRxGainPublishCompletion::PbusCompleted { bank, transaction })
            .map_err(PhyRxGainExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxGainPublishCompletion {
        PhyRxGainPublishCompletion::PbusTimedOut {
            bank: self.bank,
            transaction: self.transaction,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishTimerBinding {
    phase: PhyRxGainDelayPhase,
    micros: u32,
}

impl PhyRxGainPublishTimerBinding {
    pub fn new(action: PhyRxGainPublishAction) -> Result<Self, PhyRxGainExternalBindingError> {
        match action {
            PhyRxGainPublishAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyRxGainExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxGainPublishCompletion {
        PhyRxGainPublishCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishExternalBinding {
    Mmio(PhyRxGainPublishMmioBinding),
    Pbus(PhyRxGainPublishPbusBinding),
    Timer(PhyRxGainPublishTimerBinding),
}

impl PhyRxGainPublishExternalBinding {
    pub fn lower(action: PhyRxGainPublishAction) -> Result<Self, PhyRxGainExternalBindingError> {
        if let Ok(binding) = PhyRxGainPublishMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxGainPublishPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxGainPublishTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyRxGainExternalBindingError::UnsupportedAction)
    }
}

/// Explicit former `phy_param` inputs of complete
/// `libphy.a[phy_rx_gain.o]::phy_set_rx_gain_table`, size 650.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainInitParameters {
    pub dc_calibrated: bool,
    pub tables_initialized: bool,
    pub dc: PhyRxGainDcParameters,
    pub memory: PhyRxGainMemoryParameters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainInitOutcome {
    pub dc: Option<PhyRxGainDcOutcome>,
    pub generated_tables: bool,
    pub wifi_last_index: u8,
    pub shared_last_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitFailure {
    Dc(PhyRxGainDcFailure),
    Publish(PhyRxGainPublishFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitAction {
    CaptureAndClearDcControl {
        address: usize,
        field_mask: u32,
    },
    Dc(PhyRxGainDcAction),
    RestoreDcControl {
        address: usize,
        field_mask: u32,
        saved_field: u32,
    },
    Publish(PhyRxGainPublishAction),
    ConfigureLimits {
        wifi_last_index: u8,
    },
    EnableIqCorrection,
    Complete(PhyRxGainInitOutcome),
    Failed(PhyRxGainInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitCompletion {
    DcControlCleared { address: usize, saved_field: u32 },
    Dc(PhyRxGainDcCompletion),
    DcControlRestored { address: usize, saved_field: u32 },
    Publish(PhyRxGainPublishCompletion),
    LimitsConfigured { wifi_last_index: u8 },
    IqCorrectionEnabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitStep {
    CaptureDcControl,
    Dc {
        saved_field: u32,
        transition: PhyRxGainDcTransition,
    },
    RestoreDcControl {
        saved_field: u32,
        outcome: PhyRxGainDcOutcome,
    },
    Publish(PhyRxGainPublishTransition),
    Limits,
    IqCorrection,
    Complete,
    Failed(PhyRxGainInitFailure),
}

/// Complete heap-free composition of the RX-gain root.
///
/// The reference calibration guard and table-generation guard are explicit
/// owned booleans. A successful DC outcome is held inside this transition
/// until the enclosing `PhyColdState` commits it; no raw parameter pointer is
/// exposed while hardware calibration is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainInitTransition {
    parameters: PhyRxGainInitParameters,
    step: InitStep,
    dc_outcome: Option<PhyRxGainDcOutcome>,
    wifi_last_index: u8,
    shared_last_index: u8,
    generated_tables: bool,
}

impl PhyRxGainInitTransition {
    pub fn new(parameters: PhyRxGainInitParameters) -> Self {
        let wifi_last_index = generate_phy_rx_gain_table(PhyRxGainBank::Wifi).last_index;
        let shared_last_index = generate_phy_rx_gain_table(PhyRxGainBank::Shared).last_index;
        let step = if parameters.tables_initialized {
            InitStep::Limits
        } else if parameters.dc_calibrated {
            InitStep::Publish(PhyRxGainPublishTransition::new(parameters.memory))
        } else {
            InitStep::CaptureDcControl
        };
        Self {
            parameters,
            step,
            dc_outcome: None,
            wifi_last_index,
            shared_last_index,
            generated_tables: !parameters.tables_initialized,
        }
    }

    fn memory_with_dc(&self, outcome: PhyRxGainDcOutcome) -> PhyRxGainMemoryParameters {
        let mut memory = self.parameters.memory;
        memory.wifi_index_dc = outcome.wifi_index_dc;
        memory.wifi_dc_base = outcome.wifi_dc_base;
        let mut index = 0;
        while index != outcome.shared_index_dc.len() {
            memory.shared_index_dc[index] = outcome.shared_index_dc[index];
            index += 1;
        }
        memory
    }

    pub fn action(&self) -> PhyRxGainInitAction {
        match self.step {
            InitStep::CaptureDcControl => PhyRxGainInitAction::CaptureAndClearDcControl {
                address: crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_ADDRESS,
                field_mask: crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_FIELD_MASK,
            },
            InitStep::Dc { transition, .. } => PhyRxGainInitAction::Dc(transition.action()),
            InitStep::RestoreDcControl { saved_field, .. } => {
                PhyRxGainInitAction::RestoreDcControl {
                    address: crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_ADDRESS,
                    field_mask: crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_FIELD_MASK,
                    saved_field,
                }
            }
            InitStep::Publish(transition) => PhyRxGainInitAction::Publish(transition.action()),
            InitStep::Limits => PhyRxGainInitAction::ConfigureLimits {
                wifi_last_index: self.wifi_last_index,
            },
            InitStep::IqCorrection => PhyRxGainInitAction::EnableIqCorrection,
            InitStep::Complete => PhyRxGainInitAction::Complete(PhyRxGainInitOutcome {
                dc: self.dc_outcome,
                generated_tables: self.generated_tables,
                wifi_last_index: self.wifi_last_index,
                shared_last_index: self.shared_last_index,
            }),
            InitStep::Failed(failure) => PhyRxGainInitAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxGainInitCompletion,
    ) -> Result<(), PhyRxGainInitTransitionError> {
        match (self.step, completion) {
            (
                InitStep::CaptureDcControl,
                PhyRxGainInitCompletion::DcControlCleared {
                    address,
                    saved_field,
                },
            ) if address == crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_ADDRESS => {
                self.step = InitStep::Dc {
                    saved_field,
                    transition: PhyRxGainDcTransition::new(self.parameters.dc),
                };
            }
            (
                InitStep::Dc {
                    saved_field,
                    mut transition,
                },
                PhyRxGainInitCompletion::Dc(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxGainDcAction::Complete(outcome) => {
                        self.step = InitStep::RestoreDcControl {
                            saved_field,
                            outcome,
                        };
                    }
                    PhyRxGainDcAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyRxGainInitFailure::Dc(failure));
                    }
                    _ => {
                        self.step = InitStep::Dc {
                            saved_field,
                            transition,
                        };
                    }
                }
            }
            (
                InitStep::RestoreDcControl {
                    saved_field,
                    outcome,
                },
                PhyRxGainInitCompletion::DcControlRestored {
                    address,
                    saved_field: completed_field,
                },
            ) if address == crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_ADDRESS
                && completed_field == saved_field =>
            {
                self.dc_outcome = Some(outcome);
                self.step = InitStep::Publish(PhyRxGainPublishTransition::new(
                    self.memory_with_dc(outcome),
                ));
            }
            (InitStep::Publish(mut transition), PhyRxGainInitCompletion::Publish(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxGainPublishAction::Complete(outcome) => {
                        self.wifi_last_index = outcome.wifi_entries - 1;
                        self.shared_last_index = outcome.shared_entries - 1;
                        self.step = InitStep::Limits;
                    }
                    PhyRxGainPublishAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyRxGainInitFailure::Publish(failure));
                    }
                    _ => self.step = InitStep::Publish(transition),
                }
            }
            (InitStep::Limits, PhyRxGainInitCompletion::LimitsConfigured { wifi_last_index })
                if wifi_last_index == self.wifi_last_index =>
            {
                self.step = InitStep::IqCorrection;
            }
            (InitStep::IqCorrection, PhyRxGainInitCompletion::IqCorrectionEnabled) => {
                self.step = InitStep::Complete;
            }
            (InitStep::Complete | InitStep::Failed(_), _) => {
                return Err(PhyRxGainInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxGainInitTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitBindingError {
    NotDirectMmio,
}

/// Non-cloneable identity token for the three direct-MMIO root operations.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainInitMmioBinding {
    action: PhyRxGainInitAction,
}

impl PhyRxGainInitMmioBinding {
    pub fn new(action: PhyRxGainInitAction) -> Result<Self, PhyRxGainInitBindingError> {
        match action {
            PhyRxGainInitAction::CaptureAndClearDcControl { .. }
            | PhyRxGainInitAction::RestoreDcControl { .. }
            | PhyRxGainInitAction::ConfigureLimits { .. }
            | PhyRxGainInitAction::EnableIqCorrection => Ok(Self { action }),
            _ => Err(PhyRxGainInitBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyRxGainInitAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(
        self,
        registers: &mut open_esp_radio_hal_esp32s31::RadioRegisters,
    ) -> PhyRxGainInitCompletion {
        match self.action {
            PhyRxGainInitAction::CaptureAndClearDcControl {
                address,
                field_mask,
            } => {
                let saved_field =
                    crate::radio_hal::capture_and_clear_phy_register_field(address, field_mask);
                PhyRxGainInitCompletion::DcControlCleared {
                    address,
                    saved_field,
                }
            }
            PhyRxGainInitAction::RestoreDcControl {
                address,
                field_mask,
                saved_field,
            } => {
                crate::radio_hal::restore_phy_register_field(address, field_mask, saved_field);
                PhyRxGainInitCompletion::DcControlRestored {
                    address,
                    saved_field,
                }
            }
            PhyRxGainInitAction::ConfigureLimits { wifi_last_index } => {
                open_esp_radio_hal_esp32s31::phy_agc::configure_rx_gain_limits(
                    registers,
                    wifi_last_index,
                );
                PhyRxGainInitCompletion::LimitsConfigured { wifi_last_index }
            }
            PhyRxGainInitAction::EnableIqCorrection => {
                crate::radio_hal::enable_phy_iq_correction();
                PhyRxGainInitCompletion::IqCorrectionEnabled
            }
            _ => unreachable!(),
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_set_rx_gain_table` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxGainInitExternalBinding {
    Mmio(PhyRxGainInitMmioBinding),
    Dc(crate::phy_rx_gain_cal::PhyRxGainDcExternalBinding),
    Publish(PhyRxGainPublishExternalBinding),
}

impl PhyRxGainInitExternalBinding {
    pub fn lower(action: PhyRxGainInitAction) -> Result<Self, PhyRxGainExternalBindingError> {
        if let Ok(binding) = PhyRxGainInitMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        match action {
            PhyRxGainInitAction::Dc(action) => {
                crate::phy_rx_gain_cal::PhyRxGainDcExternalBinding::lower(action)
                    .map(Self::Dc)
                    .map_err(|_| PhyRxGainExternalBindingError::UnsupportedAction)
            }
            PhyRxGainInitAction::Publish(action) => {
                PhyRxGainPublishExternalBinding::lower(action).map(Self::Publish)
            }
            _ => Err(PhyRxGainExternalBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameters() -> PhyRxGainMemoryParameters {
        PhyRxGainMemoryParameters {
            parameter_002: 0xbf,
            wifi_index_dc: [[0x100; 2]; 8],
            wifi_dc_base: [0x100; 2],
            shared_index_dc: [[0x100; 2]; 11],
            rxbb_dc_adjustments: [[0; 2]; 6],
            wifi_auxiliary: 0,
        }
    }

    fn complete(action: PhyRxGainPublishAction) -> PhyRxGainPublishCompletion {
        match action {
            PhyRxGainPublishAction::ConfigurePbusDebugMode { bank } => {
                PhyRxGainPublishCompletion::PbusDebugModeConfigured { bank }
            }
            PhyRxGainPublishAction::ForcePbus { bank, transaction } => {
                PhyRxGainPublishCompletion::PbusCompleted { bank, transaction }
            }
            PhyRxGainPublishAction::ConfigureClock {
                bank,
                clock,
                enabled,
            } => PhyRxGainPublishCompletion::ClockConfigured {
                bank,
                clock,
                enabled,
            },
            PhyRxGainPublishAction::ProgramEntry { bank, entry } => {
                PhyRxGainPublishCompletion::EntryProgrammed { bank, entry }
            }
            PhyRxGainPublishAction::ConfigurePbusWorkMode { bank } => {
                PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                    bank,
                    settle_required: false,
                }
            }
            PhyRxGainPublishAction::ConfigurePbusWorkModePulse { bank } => {
                PhyRxGainPublishCompletion::PbusWorkModePulseConfigured { bank }
            }
            PhyRxGainPublishAction::ClearPbusWorkModePulse { bank } => {
                PhyRxGainPublishCompletion::PbusWorkModePulseCleared { bank }
            }
            PhyRxGainPublishAction::DelayMicros { phase, micros } => {
                PhyRxGainPublishCompletion::DelayElapsed { phase, micros }
            }
            PhyRxGainPublishAction::Complete(_) | PhyRxGainPublishAction::Failed(_) => {
                panic!("terminal action")
            }
        }
    }

    #[test]
    fn complete_publisher_emits_70_wifi_and_76_shared_entries() {
        let mut transition = PhyRxGainPublishTransition::new(parameters());
        let mut wifi_entries = 0;
        let mut shared_entries = 0;
        loop {
            let action = transition.action();
            match action {
                PhyRxGainPublishAction::ProgramEntry {
                    bank: PhyRxGainBank::Wifi,
                    ..
                } => wifi_entries += 1,
                PhyRxGainPublishAction::ProgramEntry {
                    bank: PhyRxGainBank::Shared,
                    ..
                } => shared_entries += 1,
                PhyRxGainPublishAction::Complete(outcome) => {
                    assert_eq!(outcome.wifi_entries, 70);
                    assert_eq!(outcome.shared_entries, 76);
                    break;
                }
                _ => {}
            }
            transition.advance(complete(action)).unwrap();
        }
        assert_eq!(wifi_entries, 70);
        assert_eq!(shared_entries, 76);
    }

    #[test]
    fn pbus_failure_is_terminal_and_preserves_operation_identity() {
        let mut transition = PhyRxGainPublishTransition::new(parameters());
        transition
            .advance(PhyRxGainPublishCompletion::PbusDebugModeConfigured {
                bank: PhyRxGainBank::Wifi,
            })
            .unwrap();
        let PhyRxGainPublishAction::ForcePbus { bank, transaction } = transition.action() else {
            panic!("expected PBus");
        };
        transition
            .advance(PhyRxGainPublishCompletion::PbusTimedOut { bank, transaction })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRxGainPublishAction::ConfigurePbusWorkMode { bank }
        );
        transition
            .advance(PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                bank,
                settle_required: false,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyRxGainPublishAction::Failed(PhyRxGainPublishFailure::PbusTimedOut {
                bank,
                transaction,
            })
        );
    }
}
