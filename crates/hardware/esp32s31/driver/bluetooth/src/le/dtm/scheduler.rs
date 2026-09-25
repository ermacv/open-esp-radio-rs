//! Related scheduler operations.

pub(crate) mod item;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod reservation;

/// DTM preparation, publication and completion over the hardware scheduler.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod lifecycle;
#[cfg(target_arch = "riscv32")]
pub(crate) use lifecycle::DtmSchedulerStopStep;

#[cfg(any(target_arch = "riscv32", test))]
pub use lifecycle::{
    DtmControllerEventPreparationError, DtmEmptySchedulerMergePrepared,
    DtmInitialSchedulerItemPhase, DtmRecurringSchedulerItemPhase,
    DtmSchedulerHeadPublicationFailure, DtmSchedulerHeadPublished, DtmSchedulerRunning,
};
#[cfg(target_arch = "riscv32")]
pub use lifecycle::{
    DtmControllerRxPreparationFailure, DtmControllerRxRecurringPreparationFailure,
    DtmControllerTxPreparationFailure, DtmControllerTxRecurringPreparationFailure,
    DtmSchedulerCompletionObserved, DtmSchedulerCompletionObservedDrainStep,
    DtmSchedulerCompletionStep, DtmSchedulerHardwareHeadEmptyObserved,
    DtmSchedulerHardwareHeadRetirementStep, DtmSchedulerRecycleStep, DtmSchedulerRunningDrainStep,
    DtmSchedulerRxSuccessRecycleStep, DtmSchedulerSoftwareListRemovalReady,
    DtmSchedulerSoftwareListUnlinkStep, DtmSchedulerSoftwareListUnlinked,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use lifecycle::{
    DtmFirstPreparationCompletionClass, DtmReceiverFirstPreSequence, DtmReceiverFirstStaged,
    DtmReceiverRecurringPreSequence, DtmSchedulerSoftwareListRemovalJoin,
    DtmSchedulerSoftwareListRemovalRecheck, DtmTransmitterFirstPreSequence,
    DtmTransmitterFirstStaged, DtmTransmitterRecurringPreSequence,
    classify_dtm_first_preparation_completion,
};

#[cfg(test)]
mod scheduler_tests;
