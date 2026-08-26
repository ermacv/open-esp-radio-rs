//! Event-driven ESP32-S31 PHY PBus initialization.
//!
//! The rev0 ROM `phy_pbus_clear_reg` body contains twelve calls to
//! `phy_pbus_force_test`. Every call publishes one transaction and then
//! busy-waits on `PHY_PBUS.STATUS_CLOCK_FORCE.BUSY`. The final work-mode path
//! can also execute synchronous one- and two-microsecond delays. This module
//! retains the exact command order while making all readiness and timer edges
//! explicit.

const PHY_PBUS_CLEAR_TRANSACTIONS: [PhyPbusForceTest; 12] = [
    PhyPbusForceTest::new(4, 1, 0),
    PhyPbusForceTest::new(4, 2, 0),
    PhyPbusForceTest::new(5, 1, 0),
    PhyPbusForceTest::new(5, 2, 0),
    PhyPbusForceTest::new(0, 1, 0),
    PhyPbusForceTest::new(0, 2, 0),
    PhyPbusForceTest::new(1, 1, 0),
    PhyPbusForceTest::new(1, 2, 0),
    PhyPbusForceTest::new(2, 1, 0x100),
    PhyPbusForceTest::new(3, 1, 0x100),
    PhyPbusForceTest::new(2, 2, 0x100),
    PhyPbusForceTest::new(3, 2, 0x100),
];

const fn clear_transaction(index: usize) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 0),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(5, 2, 0),
        4 => PhyPbusForceTest::new(0, 1, 0),
        5 => PhyPbusForceTest::new(0, 2, 0),
        6 => PhyPbusForceTest::new(1, 1, 0),
        7 => PhyPbusForceTest::new(1, 2, 0),
        8 => PhyPbusForceTest::new(2, 1, 0x100),
        9 => PhyPbusForceTest::new(3, 1, 0x100),
        10 => PhyPbusForceTest::new(2, 2, 0x100),
        _ => PhyPbusForceTest::new(3, 2, 0x100),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusForceTest {
    selector: u8,
    path: u8,
    value: u16,
}

impl PhyPbusForceTest {
    pub const fn new(selector: u8, path: u8, value: u16) -> Self {
        Self {
            selector,
            path,
            value,
        }
    }

    pub const fn selector(self) -> u8 {
        self.selector
    }

    pub const fn path(self) -> u8 {
        self.path
    }

    pub const fn value(self) -> u16 {
        self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusClearAction {
    ConfigureDebugMode,
    ForceTest(PhyPbusForceTest),
    ConfigureWorkMode,
    DelayMicros(u32),
    ConfigureWorkModePulse,
    ClearWorkModePulse,
    Complete(PhyPbusClearOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusClearCompletion {
    DebugModeConfigured,
    ForceTestCompleted(PhyPbusForceTest),
    ForceTestTimedOut(PhyPbusForceTest),
    WorkModeConfigured { settle_required: bool },
    DelayElapsed,
    WorkModePulseConfigured,
    WorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusClearOutcome {
    Cleared,
    ForceTestTimedOut(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusClearTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyPbusClearStep {
    DebugMode,
    ForceTest(usize),
    WorkMode,
    SettleDelay,
    WorkModePulse,
    PulseDelay,
    ClearWorkModePulse,
    Complete(PhyPbusClearOutcome),
}

/// Async-capable replacement plan for the complete rev0 ROM
/// `phy_pbus_clear_reg` body at `0x2f82_4572`, size `0x90`.
///
/// A force-test completion must identify the exact transaction at the current
/// cursor. The transition never samples the hardware itself and never
/// advances from a poll; an outer single radio owner performs one start and
/// one completion observation around an independently delivered edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusClearTransition {
    step: PhyPbusClearStep,
}

impl PhyPbusClearTransition {
    pub const fn new() -> Self {
        Self {
            step: PhyPbusClearStep::DebugMode,
        }
    }

    pub const fn action(self) -> PhyPbusClearAction {
        match self.step {
            PhyPbusClearStep::DebugMode => PhyPbusClearAction::ConfigureDebugMode,
            PhyPbusClearStep::ForceTest(index) => {
                PhyPbusClearAction::ForceTest(clear_transaction(index))
            }
            PhyPbusClearStep::WorkMode => PhyPbusClearAction::ConfigureWorkMode,
            PhyPbusClearStep::SettleDelay => PhyPbusClearAction::DelayMicros(1),
            PhyPbusClearStep::WorkModePulse => PhyPbusClearAction::ConfigureWorkModePulse,
            PhyPbusClearStep::PulseDelay => PhyPbusClearAction::DelayMicros(2),
            PhyPbusClearStep::ClearWorkModePulse => PhyPbusClearAction::ClearWorkModePulse,
            PhyPbusClearStep::Complete(outcome) => PhyPbusClearAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyPbusClearCompletion,
    ) -> Result<(), PhyPbusClearTransitionError> {
        self.step = match (self.step, completion) {
            (PhyPbusClearStep::DebugMode, PhyPbusClearCompletion::DebugModeConfigured) => {
                PhyPbusClearStep::ForceTest(0)
            }
            (
                PhyPbusClearStep::ForceTest(index),
                PhyPbusClearCompletion::ForceTestCompleted(transaction),
            ) if transaction == clear_transaction(index) => {
                if index + 1 == PHY_PBUS_CLEAR_TRANSACTIONS.len() {
                    PhyPbusClearStep::WorkMode
                } else {
                    PhyPbusClearStep::ForceTest(index + 1)
                }
            }
            (
                PhyPbusClearStep::ForceTest(index),
                PhyPbusClearCompletion::ForceTestTimedOut(transaction),
            ) if transaction == clear_transaction(index) => {
                PhyPbusClearStep::Complete(PhyPbusClearOutcome::ForceTestTimedOut(transaction))
            }
            (
                PhyPbusClearStep::WorkMode,
                PhyPbusClearCompletion::WorkModeConfigured {
                    settle_required: false,
                },
            ) => PhyPbusClearStep::Complete(PhyPbusClearOutcome::Cleared),
            (
                PhyPbusClearStep::WorkMode,
                PhyPbusClearCompletion::WorkModeConfigured {
                    settle_required: true,
                },
            ) => PhyPbusClearStep::SettleDelay,
            (PhyPbusClearStep::SettleDelay, PhyPbusClearCompletion::DelayElapsed) => {
                PhyPbusClearStep::WorkModePulse
            }
            (PhyPbusClearStep::WorkModePulse, PhyPbusClearCompletion::WorkModePulseConfigured) => {
                PhyPbusClearStep::PulseDelay
            }
            (PhyPbusClearStep::PulseDelay, PhyPbusClearCompletion::DelayElapsed) => {
                PhyPbusClearStep::ClearWorkModePulse
            }
            (
                PhyPbusClearStep::ClearWorkModePulse,
                PhyPbusClearCompletion::WorkModePulseCleared,
            ) => PhyPbusClearStep::Complete(PhyPbusClearOutcome::Cleared),
            (PhyPbusClearStep::Complete(_), _) => {
                return Err(PhyPbusClearTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyPbusClearTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for PhyPbusClearTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusHardwareAction {
    Start(PhyPbusForceTest),
    AwaitCompletionEdge(PhyPbusForceTest),
    Complete(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusHardwareObservation {
    StillPending,
    EdgeConsumed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusHardwareBindingError {
    WrongEdge,
    AlreadyComplete,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyPbusHardwarePhase {
    Start,
    AwaitCompletionEdge,
    Complete,
}

/// Generic non-cloneable owner of one PHY PBus command.
///
/// There is no proven completion interrupt for this block. The executor may
/// therefore submit one command and perform one status observation whenever
/// its Rust timer/deadline wakes it. A busy observation retains ownership and
/// never loops or wakes itself.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyPbusHardwareBinding {
    transaction: PhyPbusForceTest,
    phase: PhyPbusHardwarePhase,
}

impl PhyPbusHardwareBinding {
    pub const fn new(transaction: PhyPbusForceTest) -> Self {
        Self {
            transaction,
            phase: PhyPbusHardwarePhase::Start,
        }
    }

    pub const fn action(&self) -> PhyPbusHardwareAction {
        match self.phase {
            PhyPbusHardwarePhase::Start => PhyPbusHardwareAction::Start(self.transaction),
            PhyPbusHardwarePhase::AwaitCompletionEdge => {
                PhyPbusHardwareAction::AwaitCompletionEdge(self.transaction)
            }
            PhyPbusHardwarePhase::Complete => PhyPbusHardwareAction::Complete(self.transaction),
        }
    }

    pub fn started(&mut self) -> Result<(), PhyPbusHardwareBindingError> {
        match self.phase {
            PhyPbusHardwarePhase::Start => {
                self.phase = PhyPbusHardwarePhase::AwaitCompletionEdge;
                Ok(())
            }
            PhyPbusHardwarePhase::AwaitCompletionEdge => {
                Err(PhyPbusHardwareBindingError::WrongEdge)
            }
            PhyPbusHardwarePhase::Complete => Err(PhyPbusHardwareBindingError::AlreadyComplete),
        }
    }

    pub fn observe_completed(
        &mut self,
        completed: bool,
    ) -> Result<PhyPbusHardwareObservation, PhyPbusHardwareBindingError> {
        match (self.phase, completed) {
            (PhyPbusHardwarePhase::AwaitCompletionEdge, false) => {
                Ok(PhyPbusHardwareObservation::StillPending)
            }
            (PhyPbusHardwarePhase::AwaitCompletionEdge, true) => {
                self.phase = PhyPbusHardwarePhase::Complete;
                Ok(PhyPbusHardwareObservation::EdgeConsumed)
            }
            (PhyPbusHardwarePhase::Start, _) => Err(PhyPbusHardwareBindingError::WrongEdge),
            (PhyPbusHardwarePhase::Complete, _) => {
                Err(PhyPbusHardwareBindingError::AlreadyComplete)
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> Result<(), PhyPbusHardwareBindingError> {
        if self.phase != PhyPbusHardwarePhase::Start {
            return Err(if self.phase == PhyPbusHardwarePhase::Complete {
                PhyPbusHardwareBindingError::AlreadyComplete
            } else {
                PhyPbusHardwareBindingError::WrongEdge
            });
        }
        open_esp_radio_esp32s31_hal::pbus::start_force_test(
            registers,
            self.transaction.selector(),
            self.transaction.path(),
            self.transaction.value(),
        );
        self.started()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> Result<PhyPbusHardwareObservation, PhyPbusHardwareBindingError> {
        if self.phase != PhyPbusHardwarePhase::AwaitCompletionEdge {
            return Err(if self.phase == PhyPbusHardwarePhase::Complete {
                PhyPbusHardwareBindingError::AlreadyComplete
            } else {
                PhyPbusHardwareBindingError::WrongEdge
            });
        }
        match open_esp_radio_esp32s31_hal::pbus::try_finish_force_test(registers) {
            Ok(()) => self.observe_completed(true),
            Err(open_esp_radio_esp32s31_hal::pbus::PbusError::Busy) => {
                self.observe_completed(false)
            }
        }
    }

    pub fn into_transaction(self) -> Result<PhyPbusForceTest, PhyPbusHardwareBindingError> {
        if self.phase == PhyPbusHardwarePhase::Complete {
            Ok(self.transaction)
        } else {
            Err(PhyPbusHardwareBindingError::Incomplete)
        }
    }

    pub const fn into_timeout_transaction(self) -> PhyPbusForceTest {
        self.transaction
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyForceTxRxAction {
    Configure {
        enabled: bool,
        phase: u8,
    },
    DelayMicros {
        enabled: bool,
        completed_phase: u8,
        micros: u32,
    },
    Complete {
        enabled: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyForceTxRxCompletion {
    Configured {
        enabled: bool,
        phase: u8,
    },
    DelayElapsed {
        enabled: bool,
        completed_phase: u8,
        micros: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyForceTxRxTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyForceTxRxStep {
    ConfigureFirst,
    DelayFirst,
    ConfigureSecond,
    DelaySecond,
    Complete,
}

/// Async-capable owner of complete rev0 ROM `phy_force_txrx_off`.
///
/// Both branches perform two distinct force-mode writes and retain the
/// one-microsecond delay following each write as an external timer edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyForceTxRxTransition {
    enabled: bool,
    step: PhyForceTxRxStep,
}

impl PhyForceTxRxTransition {
    pub const fn new(enabled: bool) -> Self {
        Self {
            enabled,
            step: PhyForceTxRxStep::ConfigureFirst,
        }
    }

    pub const fn action(self) -> PhyForceTxRxAction {
        match self.step {
            PhyForceTxRxStep::ConfigureFirst => PhyForceTxRxAction::Configure {
                enabled: self.enabled,
                phase: 0,
            },
            PhyForceTxRxStep::DelayFirst => PhyForceTxRxAction::DelayMicros {
                enabled: self.enabled,
                completed_phase: 0,
                micros: 1,
            },
            PhyForceTxRxStep::ConfigureSecond => PhyForceTxRxAction::Configure {
                enabled: self.enabled,
                phase: 1,
            },
            PhyForceTxRxStep::DelaySecond => PhyForceTxRxAction::DelayMicros {
                enabled: self.enabled,
                completed_phase: 1,
                micros: 1,
            },
            PhyForceTxRxStep::Complete => PhyForceTxRxAction::Complete {
                enabled: self.enabled,
            },
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyForceTxRxCompletion,
    ) -> Result<(), PhyForceTxRxTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyForceTxRxStep::ConfigureFirst,
                PhyForceTxRxCompletion::Configured { enabled, phase: 0 },
            ) if enabled == self.enabled => PhyForceTxRxStep::DelayFirst,
            (
                PhyForceTxRxStep::DelayFirst,
                PhyForceTxRxCompletion::DelayElapsed {
                    enabled,
                    completed_phase: 0,
                    micros: 1,
                },
            ) if enabled == self.enabled => PhyForceTxRxStep::ConfigureSecond,
            (
                PhyForceTxRxStep::ConfigureSecond,
                PhyForceTxRxCompletion::Configured { enabled, phase: 1 },
            ) if enabled == self.enabled => PhyForceTxRxStep::DelaySecond,
            (
                PhyForceTxRxStep::DelaySecond,
                PhyForceTxRxCompletion::DelayElapsed {
                    enabled,
                    completed_phase: 1,
                    micros: 1,
                },
            ) if enabled == self.enabled => PhyForceTxRxStep::Complete,
            (PhyForceTxRxStep::Complete, _) => {
                return Err(PhyForceTxRxTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyForceTxRxTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyForceTxRxBindingError {
    UnsupportedAction,
}

/// Non-cloneable identity for one force-mode register edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyForceTxRxMmioBinding {
    enabled: bool,
    phase: u8,
}

impl PhyForceTxRxMmioBinding {
    pub const fn new(action: PhyForceTxRxAction) -> Result<Self, PhyForceTxRxBindingError> {
        match action {
            PhyForceTxRxAction::Configure { enabled, phase } => Ok(Self { enabled, phase }),
            _ => Err(PhyForceTxRxBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> PhyForceTxRxAction {
        PhyForceTxRxAction::Configure {
            enabled: self.enabled,
            phase: self.phase,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl open_esp_radio_esp32s31_hal::SharedPhyAccess,
    ) -> PhyForceTxRxCompletion {
        open_esp_radio_esp32s31_hal::pbus::configure_force_txrx(
            registers,
            self.enabled,
            self.phase,
        );
        PhyForceTxRxCompletion::Configured {
            enabled: self.enabled,
            phase: self.phase,
        }
    }
}

/// Consumed identity for one caller-driven one-microsecond timer edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyForceTxRxTimerBinding {
    enabled: bool,
    completed_phase: u8,
    micros: u32,
}

impl PhyForceTxRxTimerBinding {
    pub const fn new(action: PhyForceTxRxAction) -> Result<Self, PhyForceTxRxBindingError> {
        match action {
            PhyForceTxRxAction::DelayMicros {
                enabled,
                completed_phase,
                micros,
            } => Ok(Self {
                enabled,
                completed_phase,
                micros,
            }),
            _ => Err(PhyForceTxRxBindingError::UnsupportedAction),
        }
    }

    pub const fn action(&self) -> PhyForceTxRxAction {
        PhyForceTxRxAction::DelayMicros {
            enabled: self.enabled,
            completed_phase: self.completed_phase,
            micros: self.micros,
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyForceTxRxCompletion {
        PhyForceTxRxCompletion::DelayElapsed {
            enabled: self.enabled,
            completed_phase: self.completed_phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyForceTxRxExternalBinding {
    Mmio(PhyForceTxRxMmioBinding),
    Timer(PhyForceTxRxTimerBinding),
}

impl PhyForceTxRxExternalBinding {
    pub const fn lower(action: PhyForceTxRxAction) -> Result<Self, PhyForceTxRxBindingError> {
        match action {
            PhyForceTxRxAction::Configure { .. } => match PhyForceTxRxMmioBinding::new(action) {
                Ok(binding) => Ok(Self::Mmio(binding)),
                Err(error) => Err(error),
            },
            PhyForceTxRxAction::DelayMicros { .. } => match PhyForceTxRxTimerBinding::new(action) {
                Ok(binding) => Ok(Self::Timer(binding)),
                Err(error) => Err(error),
            },
            PhyForceTxRxAction::Complete { .. } => Err(PhyForceTxRxBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PHY_PBUS_CLEAR_TRANSACTIONS, PhyForceTxRxAction, PhyForceTxRxCompletion,
        PhyForceTxRxExternalBinding, PhyForceTxRxTransition, PhyForceTxRxTransitionError,
        PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusClearOutcome, PhyPbusClearTransition,
        PhyPbusClearTransitionError, PhyPbusForceTest, PhyPbusHardwareAction,
        PhyPbusHardwareBinding, PhyPbusHardwareBindingError, PhyPbusHardwareObservation,
    };

    fn reach_work_mode(transition: &mut PhyPbusClearTransition) {
        transition
            .advance(PhyPbusClearCompletion::DebugModeConfigured)
            .unwrap();
        for transaction in PHY_PBUS_CLEAR_TRANSACTIONS {
            assert_eq!(
                transition.action(),
                PhyPbusClearAction::ForceTest(transaction)
            );
            transition
                .advance(PhyPbusClearCompletion::ForceTestCompleted(transaction))
                .unwrap();
        }
        assert_eq!(transition.action(), PhyPbusClearAction::ConfigureWorkMode);
    }

    #[test]
    fn clear_sequence_matches_all_twelve_rom_transactions() {
        assert_eq!(
            PHY_PBUS_CLEAR_TRANSACTIONS,
            [
                PhyPbusForceTest::new(4, 1, 0),
                PhyPbusForceTest::new(4, 2, 0),
                PhyPbusForceTest::new(5, 1, 0),
                PhyPbusForceTest::new(5, 2, 0),
                PhyPbusForceTest::new(0, 1, 0),
                PhyPbusForceTest::new(0, 2, 0),
                PhyPbusForceTest::new(1, 1, 0),
                PhyPbusForceTest::new(1, 2, 0),
                PhyPbusForceTest::new(2, 1, 0x100),
                PhyPbusForceTest::new(3, 1, 0x100),
                PhyPbusForceTest::new(2, 2, 0x100),
                PhyPbusForceTest::new(3, 2, 0x100),
            ]
        );
    }

    #[test]
    fn force_test_completion_is_bound_to_the_current_transaction() {
        let mut transition = PhyPbusClearTransition::new();
        transition
            .advance(PhyPbusClearCompletion::DebugModeConfigured)
            .unwrap();
        let wrong = PHY_PBUS_CLEAR_TRANSACTIONS[1];
        assert_eq!(
            transition.advance(PhyPbusClearCompletion::ForceTestCompleted(wrong)),
            Err(PhyPbusClearTransitionError::WrongCompletion)
        );
        assert_eq!(
            transition.action(),
            PhyPbusClearAction::ForceTest(PHY_PBUS_CLEAR_TRANSACTIONS[0])
        );
    }

    #[test]
    fn work_mode_without_settle_path_finishes_immediately() {
        let mut transition = PhyPbusClearTransition::new();
        reach_work_mode(&mut transition);
        transition
            .advance(PhyPbusClearCompletion::WorkModeConfigured {
                settle_required: false,
            })
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared)
        );
    }

    #[test]
    fn work_mode_settle_path_requires_both_async_timer_edges() {
        let mut transition = PhyPbusClearTransition::new();
        reach_work_mode(&mut transition);
        transition
            .advance(PhyPbusClearCompletion::WorkModeConfigured {
                settle_required: true,
            })
            .unwrap();
        assert_eq!(transition.action(), PhyPbusClearAction::DelayMicros(1));
        assert_eq!(
            transition.advance(PhyPbusClearCompletion::WorkModePulseConfigured),
            Err(PhyPbusClearTransitionError::WrongCompletion)
        );
        transition
            .advance(PhyPbusClearCompletion::DelayElapsed)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyPbusClearAction::ConfigureWorkModePulse
        );
        transition
            .advance(PhyPbusClearCompletion::WorkModePulseConfigured)
            .unwrap();
        assert_eq!(transition.action(), PhyPbusClearAction::DelayMicros(2));
        transition
            .advance(PhyPbusClearCompletion::DelayElapsed)
            .unwrap();
        assert_eq!(transition.action(), PhyPbusClearAction::ClearWorkModePulse);
        transition
            .advance(PhyPbusClearCompletion::WorkModePulseCleared)
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared)
        );
        assert_eq!(
            transition.advance(PhyPbusClearCompletion::WorkModePulseCleared),
            Err(PhyPbusClearTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn busy_force_test_timeout_is_terminal_without_retry() {
        let mut transition = PhyPbusClearTransition::new();
        transition
            .advance(PhyPbusClearCompletion::DebugModeConfigured)
            .unwrap();
        let transaction = PHY_PBUS_CLEAR_TRANSACTIONS[0];
        transition
            .advance(PhyPbusClearCompletion::ForceTestTimedOut(transaction))
            .unwrap();
        assert_eq!(
            transition.action(),
            PhyPbusClearAction::Complete(PhyPbusClearOutcome::ForceTestTimedOut(transaction))
        );
        assert_eq!(
            transition.advance(PhyPbusClearCompletion::ForceTestCompleted(transaction)),
            Err(PhyPbusClearTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn generic_hardware_binding_never_advances_from_a_busy_observation() {
        let transaction = PhyPbusForceTest::new(4, 1, 0x123);
        let mut binding = PhyPbusHardwareBinding::new(transaction);
        assert_eq!(binding.action(), PhyPbusHardwareAction::Start(transaction));
        binding.started().unwrap();
        assert_eq!(
            binding.observe_completed(false).unwrap(),
            PhyPbusHardwareObservation::StillPending
        );
        assert_eq!(
            binding.action(),
            PhyPbusHardwareAction::AwaitCompletionEdge(transaction)
        );
        assert_eq!(
            binding.into_transaction(),
            Err(PhyPbusHardwareBindingError::Incomplete)
        );

        let mut binding = PhyPbusHardwareBinding::new(transaction);
        binding.started().unwrap();
        assert_eq!(
            binding.observe_completed(true).unwrap(),
            PhyPbusHardwareObservation::EdgeConsumed
        );
        assert_eq!(binding.into_transaction().unwrap(), transaction);
    }

    #[test]
    fn force_txrx_requires_both_writes_and_both_async_delays_for_each_branch() {
        for enabled in [false, true] {
            let mut transition = PhyForceTxRxTransition::new(enabled);
            let mut actions = std::vec::Vec::new();
            loop {
                let action = transition.action();
                actions.push(action);
                let completion = match PhyForceTxRxExternalBinding::lower(action) {
                    Ok(PhyForceTxRxExternalBinding::Mmio(binding)) => {
                        let PhyForceTxRxAction::Configure { enabled, phase } = binding.action()
                        else {
                            panic!("MMIO binding lost its action identity")
                        };
                        PhyForceTxRxCompletion::Configured { enabled, phase }
                    }
                    Ok(PhyForceTxRxExternalBinding::Timer(binding)) => {
                        assert_eq!(binding.micros(), 1);
                        binding.into_completion()
                    }
                    Err(_) => break,
                };
                transition.advance(completion).unwrap();
            }
            assert_eq!(
                actions,
                [
                    PhyForceTxRxAction::Configure { enabled, phase: 0 },
                    PhyForceTxRxAction::DelayMicros {
                        enabled,
                        completed_phase: 0,
                        micros: 1,
                    },
                    PhyForceTxRxAction::Configure { enabled, phase: 1 },
                    PhyForceTxRxAction::DelayMicros {
                        enabled,
                        completed_phase: 1,
                        micros: 1,
                    },
                    PhyForceTxRxAction::Complete { enabled },
                ]
            );
        }
    }

    #[test]
    fn force_txrx_rejects_a_stale_or_early_timer_edge() {
        let mut transition = PhyForceTxRxTransition::new(true);
        assert_eq!(
            transition.advance(PhyForceTxRxCompletion::DelayElapsed {
                enabled: true,
                completed_phase: 0,
                micros: 1,
            }),
            Err(PhyForceTxRxTransitionError::WrongCompletion)
        );
        assert_eq!(
            transition.action(),
            PhyForceTxRxAction::Configure {
                enabled: true,
                phase: 0,
            }
        );
    }
}
