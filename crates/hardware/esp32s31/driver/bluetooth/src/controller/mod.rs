//! Controller bring-up, HAL readiness and controller-time ownership.

pub(crate) mod boot;
pub use boot::{ControllerRoleRetirementError, ControllerTaskRetirementError};
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod hal;
pub(crate) mod time;
pub use oer_esp32s31_hal::bluetooth::BluetoothControllerOutputReleaseError;
pub use time::ControllerTimeRetirementError;

#[cfg(target_arch = "riscv32")]
pub use boot::{
    AlwaysAwakePostEnableTimeBeginError, AlwaysAwakePostEnableTimeBeginFailure,
    AlwaysAwakePostEnableTimeError, AlwaysAwakePostEnableTimeFailure,
    AlwaysAwakePostEnableTimeOrphanDrainStep, AlwaysAwakePostEnableTimePending,
    AlwaysAwakePostEnableTimeStep, AlwaysAwakeTimeObservedAfterEnable, ControllerColdReleased,
    ControllerIdleCommandIntake, ControllerIdleCommandTask, ControllerIdleResetBarrier,
    ControllerIdleResetCompletion, ControllerIdleResponsePending,
    ControllerIdleResponsePublication, ControllerInterruptOwnerPublicationFailure,
    ControllerInterruptOwnersPublished, ControllerInterruptOwnersReady, ControllerModemTimerBegin,
    ControllerModemTimerReadiness, ControllerModemTimerReadinessClass, ControllerModemTimerRearm,
    ControllerModemTimerRetired, ControllerModemTimerRetirementError, ControllerModemTimerStep,
    ControllerModemTimerTask, ControllerOutputTimerStarted, ControllerPhyMaintained,
    ControllerPhyMaintenanceError, ControllerPhyMaintenanceFailure,
    ControllerPhysicalShutdownError, ControllerPhysicalShutdownFailure,
    ControllerPublishedInterruptService, ControllerPublishedRuntimeEndpoints,
    ControllerPublishedRuntimeSplit, ControllerPublishedRuntimeSplitFailure,
    ControllerPublishedTaskService, ControllerRestartError, ControllerRestartFailure,
    ControllerRestarted, ControllerRetiredStorage, ControllerSchedulerCurrentBeginError,
    ControllerSchedulerCurrentBeginFailure, ControllerSchedulerCurrentError,
    ControllerSchedulerCurrentFailure, ControllerSchedulerCurrentPending,
    ControllerSchedulerCurrentStep, ControllerSchedulerEpochRetained,
    ControllerSchedulerEpochUnavailable, ControllerSchedulerNowReady, ControllerTaskHciRetired,
    ControllerTimeOrphanDrainStep, DtmControllerInitialPreparationFailure,
    DtmControllerPreparationOutcome, DtmControllerPreparationPending, DtmControllerPreparationStep,
    DtmControllerPreparationTerminal, DtmPostUnlinkArmStep, DtmSchedulerStartFailure,
    DtmSoftwareListRemovalPublishedStep, InterruptOwnerRestartStorage, InterruptOwnerStorage,
    LePacketStartTimingError, LegacyAdvertisingControllerCancellationPending,
    LegacyAdvertisingControllerCancellationStep, LegacyAdvertisingControllerPreparationError,
    LegacyAdvertisingControllerPreparationFailStop,
    LegacyAdvertisingControllerPreparationFailStopCause,
    LegacyAdvertisingControllerPreparationOutcome, LegacyAdvertisingControllerPreparationPending,
    LegacyAdvertisingControllerPreparationStep, LegacyAdvertisingControllerPreparationTerminal,
    LegacyAdvertisingSchedulerStartFailure, ModemLpTimerInterruptDispatchStorage,
    ModemLpTimerRetirementStorage, ModemLpTimerSoftwareOwnerStorage,
    PassiveScanControllerCancellationPending, PassiveScanControllerCancellationStep,
    PassiveScanControllerPreparationError, PassiveScanControllerPreparationFailStop,
    PassiveScanControllerPreparationFailStopCause, PassiveScanControllerPreparationOutcome,
    PassiveScanControllerPreparationPending, PassiveScanControllerPreparationStep,
    PassiveScanControllerPreparationTerminal, PassiveScanSchedulerStartFailure,
    PeripheralConnectionSchedulerStartFailure, SchedulerRunInterruptStorage,
    SharedInterruptDispatchStorage,
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
