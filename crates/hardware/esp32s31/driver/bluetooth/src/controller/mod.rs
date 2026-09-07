//! Controller bring-up, HAL readiness and controller-time ownership.

pub(crate) mod boot;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod hal;
pub(crate) mod time;

#[cfg(target_arch = "riscv32")]
pub use boot::{
    AlwaysAwakePostEnableTimeBeginError, AlwaysAwakePostEnableTimeBeginFailure,
    AlwaysAwakePostEnableTimeError, AlwaysAwakePostEnableTimeFailure,
    AlwaysAwakePostEnableTimeOrphanDrainStep, AlwaysAwakePostEnableTimePending,
    AlwaysAwakePostEnableTimeStep, AlwaysAwakeTimeObservedAfterEnable, ControllerIdleCommandIntake,
    ControllerIdleCommandTask, ControllerIdleResetBarrier, ControllerIdleResetCompletion,
    ControllerIdleResponsePending, ControllerIdleResponsePublication,
    ControllerInterruptOwnerPublicationFailure, ControllerInterruptOwnersPublished,
    ControllerInterruptOwnersReady, ControllerModemTimerBegin, ControllerModemTimerReadiness,
    ControllerModemTimerReadinessClass, ControllerModemTimerRearm, ControllerModemTimerStep,
    ControllerModemTimerTask, ControllerOutputTimerStarted, ControllerPublishedInterruptService,
    ControllerPublishedRuntimeEndpoints, ControllerPublishedRuntimeSplit,
    ControllerPublishedRuntimeSplitFailure, ControllerPublishedTaskService,
    ControllerSchedulerCurrentBeginError, ControllerSchedulerCurrentBeginFailure,
    ControllerSchedulerCurrentError, ControllerSchedulerCurrentFailure,
    ControllerSchedulerCurrentPending, ControllerSchedulerCurrentStep,
    ControllerSchedulerEpochRetained, ControllerSchedulerEpochUnavailable,
    ControllerSchedulerNowReady, ControllerTimeOrphanDrainStep,
    DtmControllerInitialPreparationFailure, DtmControllerPreparationOutcome,
    DtmControllerPreparationPending, DtmControllerPreparationStep,
    DtmControllerPreparationTerminal, DtmPostUnlinkArmStep, DtmSchedulerStartFailure,
    DtmSoftwareListRemovalPublishedStep, InterruptOwnerStorage, LePacketStartTimingError,
    LegacyAdvertisingControllerCancellationPending, LegacyAdvertisingControllerCancellationStep,
    LegacyAdvertisingControllerPreparationError, LegacyAdvertisingControllerPreparationFailStop,
    LegacyAdvertisingControllerPreparationFailStopCause,
    LegacyAdvertisingControllerPreparationOutcome, LegacyAdvertisingControllerPreparationPending,
    LegacyAdvertisingControllerPreparationStep, LegacyAdvertisingControllerPreparationTerminal,
    LegacyAdvertisingSchedulerStartFailure, ModemLpTimerInterruptDispatchStorage,
    ModemLpTimerSoftwareOwnerStorage, PassiveScanControllerCancellationPending,
    PassiveScanControllerCancellationStep, PassiveScanControllerPreparationError,
    PassiveScanControllerPreparationFailStop, PassiveScanControllerPreparationFailStopCause,
    PassiveScanControllerPreparationOutcome, PassiveScanControllerPreparationPending,
    PassiveScanControllerPreparationStep, PassiveScanControllerPreparationTerminal,
    PassiveScanSchedulerStartFailure, PeripheralConnectionSchedulerStartFailure,
    SchedulerRunInterruptStorage, SharedInterruptDispatchStorage,
    peripheral_connection::{
        PeripheralConnectionCompletionStep, PeripheralConnectionControllerPreparationError,
        PeripheralConnectionRecurringCandidateStep, PeripheralConnectionRecurringRetry,
        PeripheralConnectionRecurringSequenceCompletion,
    },
};
#[cfg(target_arch = "riscv32")]
pub use hal::ControllerHalInitialized;

/// HCI queue binding to the controller epoch, without executor ownership.
#[cfg(any(target_arch = "riscv32", test))]
pub mod hci;
