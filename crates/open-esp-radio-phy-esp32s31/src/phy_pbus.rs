//! Event-driven ESP32-S31 PHY PBus initialization.
//!
//! The rev0 ROM `phy_pbus_clear_reg` body contains twelve calls to
//! `phy_pbus_force_test`. Every call publishes one transaction and then
//! busy-waits on the sign bit of `0x2010_0890`. The final work-mode path can
//! also execute synchronous one- and two-microsecond delays. This module
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
    BusyAtStart,
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
    pub unsafe fn start_target(&mut self) -> Result<(), PhyPbusHardwareBindingError> {
        if self.phase != PhyPbusHardwarePhase::Start {
            return Err(if self.phase == PhyPbusHardwarePhase::Complete {
                PhyPbusHardwareBindingError::AlreadyComplete
            } else {
                PhyPbusHardwareBindingError::WrongEdge
            });
        }
        crate::radio_hal::try_start_phy_pbus_force_test(self.transaction).map_err(
            |crate::radio_hal::PhyPbusError::Busy| PhyPbusHardwareBindingError::BusyAtStart,
        )?;
        self.started()
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn observe_target_edge(
        &mut self,
    ) -> Result<PhyPbusHardwareObservation, PhyPbusHardwareBindingError> {
        if self.phase != PhyPbusHardwarePhase::AwaitCompletionEdge {
            return Err(if self.phase == PhyPbusHardwarePhase::Complete {
                PhyPbusHardwareBindingError::AlreadyComplete
            } else {
                PhyPbusHardwareBindingError::WrongEdge
            });
        }
        self.observe_completed(crate::radio_hal::try_finish_phy_pbus_force_test().is_ok())
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

#[cfg(test)]
mod tests {
    use super::{
        PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusClearOutcome, PhyPbusClearTransition,
        PhyPbusClearTransitionError, PhyPbusForceTest, PhyPbusHardwareAction,
        PhyPbusHardwareBinding, PhyPbusHardwareBindingError, PhyPbusHardwareObservation,
        PHY_PBUS_CLEAR_TRANSACTIONS,
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
}
