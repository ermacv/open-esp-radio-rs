//! Scheduler configuration, time policy and affine hardware transactions.
//!
//! Portable policies remain available on hosts; hardware transactions and
//! timeline admission retain their target-or-test availability.

#[cfg(any(test, target_arch = "riscv32"))]
pub(crate) mod completion;

pub(crate) mod config;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod core;
pub(crate) mod finished_lists;
pub(crate) mod insertion;
pub(crate) mod lock_modify;
pub(crate) mod time;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod timeline;

pub use config::SchedulerSoftwareConfig;
#[cfg(target_arch = "riscv32")]
pub use core::DtmSchedulerRecycleStep;
#[cfg(any(target_arch = "riscv32", test))]
pub use core::{
    ControllerTimeAcquisitionError, DtmControllerEventPreparationError, DtmSchedulerRunning,
    SchedulerEmptyListMergeError,
};
#[cfg(target_arch = "riscv32")]
pub use core::{
    DtmControllerRxPreparationFailure, DtmControllerRxRecurringPreparationFailure,
    DtmControllerTxPreparationFailure, DtmControllerTxRecurringPreparationFailure,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use core::{
    DtmEmptySchedulerMergePrepared, DtmInitialSchedulerItemPhase, DtmRecurringSchedulerItemPhase,
};
#[cfg(target_arch = "riscv32")]
pub use core::{
    DtmSchedulerCompletionObserved, DtmSchedulerCompletionObservedDrainStep,
    DtmSchedulerCompletionStep, DtmSchedulerHardwareHeadEmptyObserved,
    DtmSchedulerHardwareHeadRetirementStep,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use core::{DtmSchedulerHeadPublicationFailure, DtmSchedulerHeadPublished};
#[cfg(target_arch = "riscv32")]
pub use core::{
    DtmSchedulerRunningDrainStep, DtmSchedulerRxSuccessRecycleStep,
    DtmSchedulerSoftwareListRemovalReady, DtmSchedulerSoftwareListUnlinked,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use core::{
    LegacyAdvertisingAdmissionObservation, LegacyAdvertisingEmptySchedulerMergeFailure,
    LegacyAdvertisingEmptySchedulerMergePrepared, LegacyAdvertisingEventPrepared,
    LegacyAdvertisingFirstEventPreparationError, LegacyAdvertisingFirstEventPreparationFailure,
    LegacyAdvertisingFirstPreSequence,
};
#[cfg(target_arch = "riscv32")]
pub use core::{
    LegacyAdvertisingRecurringEventPreparationError,
    LegacyAdvertisingRecurringEventPreparationFailure, LegacyAdvertisingRecurringPreSequence,
    LegacyAdvertisingSchedulerHeadPublicationFailure, LegacyAdvertisingSchedulerHeadPublished,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use core::{
    LegacyAdvertisingSequenceObservation, PassiveScanAdmissionObservation,
    PassiveScanEmptySchedulerMergeFailure, PassiveScanEmptySchedulerMergePrepared,
    PassiveScanEventPrepared, PassiveScanFirstEventCandidate,
    PassiveScanFirstEventPreparationError, PassiveScanFirstEventPreparationFailure,
    PassiveScanFirstPreSequence,
};
#[cfg(target_arch = "riscv32")]
pub use core::{PassiveScanSchedulerHeadPublicationFailure, PassiveScanSchedulerHeadPublished};
#[cfg(any(target_arch = "riscv32", test))]
pub use core::{
    PassiveScanSequenceObservation, PeripheralConnectionEmptySchedulerMergePrepared,
    PeripheralConnectionFirstEventPreparationError,
};
#[cfg(target_arch = "riscv32")]
pub use core::{
    PeripheralConnectionRecurringCandidateError,
    PeripheralConnectionRecurringEmptySchedulerMergeFailure,
    PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    PeripheralConnectionRecurringEventCandidate,
    PeripheralConnectionRecurringEventPreparationError,
    PeripheralConnectionRecurringEventPreparationFailure,
    PeripheralConnectionRecurringEventPrepared, PeripheralConnectionRecurringPreSequence,
    PeripheralConnectionSchedulerCompleted, PeripheralConnectionSchedulerHeadPublicationFailure,
    PeripheralConnectionSchedulerHeadPublished, PeripheralConnectionSchedulerRecycled,
    SchedulerFinishedListDrainPending, SchedulerFinishedListDrainState,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use core::{SchedulerHeadPublicationError, SchedulerInitialized};
#[cfg(any(target_arch = "riscv32", test))]
pub use timeline::{
    SchedulerRawWindow, SchedulerReservationError, SchedulerReservationReleaseError,
    SchedulerReservationReleaseFailure, SchedulerSequenceAuthorizationError,
    SchedulerSequenceReady, SchedulerTimingPolicy, SchedulerWindowReservation,
};

pub use finished_lists::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    SchedulerFinishedListCaptureError, SchedulerFinishedListWorker,
    SchedulerFinishedListWorkerStep,
};

pub use insertion::{
    SchedulerInsertionBeginOutcome, SchedulerInsertionBusyDecision, SchedulerInsertionEndPrelude,
    SchedulerInsertionFinalAction, SchedulerInsertionItemStatusGate,
    SchedulerInsertionLockModifyGate, SchedulerInsertionSleepDecision, SchedulerInsertionSleepGate,
};

pub use lock_modify::{
    BluetoothSchedulerLockModifyInterruptObservation, SchedulerLockModifyBeginError,
    SchedulerLockModifyEvent, SchedulerLockModifyEventCell, SchedulerLockModifyEventPublication,
    SchedulerLockModifyPublicationResult, SchedulerLockModifyWorker, SchedulerLockModifyWorkerStep,
};
