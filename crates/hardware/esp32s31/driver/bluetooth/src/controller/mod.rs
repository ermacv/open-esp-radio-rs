//! Controller bring-up and HCI composition over the hardware scheduler.

pub(crate) mod boot;
// Role modules reach the hardware scheduler port through the controller.
#[cfg(target_arch = "riscv32")]
pub(crate) use crate::scheduler::SchedulerRunInterruptStorage;
pub use boot::{ControllerRoleRetirementError, ControllerTaskRetirementError};
pub use oer_esp32s31_hal::bluetooth::BluetoothControllerOutputReleaseError;

#[cfg(target_arch = "riscv32")]
pub use boot::{
    AlwaysAwakePostEnableTimeBeginError, AlwaysAwakePostEnableTimeBeginFailure,
    AlwaysAwakePostEnableTimeError, AlwaysAwakePostEnableTimeFailure,
    AlwaysAwakePostEnableTimeOrphanDrainStep, AlwaysAwakePostEnableTimePending,
    AlwaysAwakePostEnableTimeStep, AlwaysAwakeTimeObservedAfterEnable, ControllerColdReleased,
    ControllerIdleCommandIntake, ControllerIdleCommandTask, ControllerIdleResetBarrier,
    ControllerIdleResetCompletion, ControllerIdleResponsePending,
    ControllerIdleResponsePublication, ControllerInterruptOwnerPublicationFailure,
    ControllerInterruptOwnersPublished, ControllerInterruptOwnersReady,
    ControllerOutputTimerStarted, ControllerPhyMaintained, ControllerPhyMaintenanceError,
    ControllerPhyMaintenanceFailure, ControllerPhysicalShutdownError,
    ControllerPhysicalShutdownFailure, ControllerPublishedInterruptService,
    ControllerPublishedRuntimeEndpoints, ControllerPublishedRuntimeSplit,
    ControllerPublishedRuntimeSplitFailure, ControllerPublishedTaskService, ControllerRestartError,
    ControllerRestartFailure, ControllerRestarted, ControllerRetiredStorage,
    ControllerSchedulerCurrentBeginError, ControllerSchedulerCurrentBeginFailure,
    ControllerSchedulerCurrentError, ControllerSchedulerCurrentFailure,
    ControllerSchedulerCurrentPending, ControllerSchedulerCurrentStep,
    ControllerSchedulerEpochRetained, ControllerSchedulerEpochUnavailable,
    ControllerSchedulerNowReady, ControllerTaskHciRetired, ControllerTimeOrphanDrainStep,
    DtmControllerInitialPreparationFailure, DtmControllerPreparationOutcome,
    DtmControllerPreparationPending, DtmControllerPreparationStep,
    DtmControllerPreparationTerminal, DtmPostUnlinkArmStep, DtmSchedulerStartFailure,
    DtmSoftwareListRemovalPublishedStep, LePacketStartTimingError,
    LegacyAdvertisingControllerCancellationPending, LegacyAdvertisingControllerCancellationStep,
    LegacyAdvertisingControllerPreparationError, LegacyAdvertisingControllerPreparationFailStop,
    LegacyAdvertisingControllerPreparationFailStopCause,
    LegacyAdvertisingControllerPreparationOutcome, LegacyAdvertisingControllerPreparationPending,
    LegacyAdvertisingControllerPreparationStep, LegacyAdvertisingControllerPreparationTerminal,
    LegacyAdvertisingSchedulerStartFailure, PassiveScanControllerCancellationPending,
    PassiveScanControllerCancellationStep, PassiveScanControllerPreparationError,
    PassiveScanControllerPreparationFailStop, PassiveScanControllerPreparationFailStopCause,
    PassiveScanControllerPreparationOutcome, PassiveScanControllerPreparationPending,
    PassiveScanControllerPreparationStep, PassiveScanControllerPreparationTerminal,
    PassiveScanSchedulerStartFailure, PeripheralConnectionSchedulerStartFailure,
    peripheral_connection::{
        PeripheralConnectionCompletionStep, PeripheralConnectionControllerPreparationError,
        PeripheralConnectionRecurringCandidateStep, PeripheralConnectionRecurringRetry,
        PeripheralConnectionRecurringSequenceCompletion,
    },
};

/// HCI queue binding to the controller epoch, without executor ownership.
#[cfg(any(target_arch = "riscv32", test))]
pub mod hci;
