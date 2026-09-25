//! Scanning event lifecycles.

pub(crate) mod passive;

pub use passive::{
    PassiveScanRuntimeBeginError, PassiveScanRuntimeConfig, PassiveScanRuntimeResources,
};
#[cfg(target_arch = "riscv32")]
pub use passive::{
    active::{
        PassiveScanActiveFault, PassiveScanActiveFaultCause, PassiveScanActiveSession,
        PassiveScanActiveStep, PassiveScanActiveWait, PassiveScanEventCpuOwned,
    },
    hci::{
        PassiveScanHciActiveCommandIntake, PassiveScanHciActiveCommandMismatch,
        PassiveScanHciActiveCommandRoute, PassiveScanHciActiveFault,
        PassiveScanHciActivePendingFault, PassiveScanHciActivePendingRadioStep,
        PassiveScanHciActiveResponsePending, PassiveScanHciActiveResponsePublication,
        PassiveScanHciActiveSession, PassiveScanHciActiveStep, PassiveScanHciCommandIntake,
        PassiveScanHciCommandMismatch, PassiveScanHciCommandRoute,
        PassiveScanHciCpuResponsePending, PassiveScanHciCpuResponsePublication,
        PassiveScanHciFirstRunner, PassiveScanHciFirstRunnerFailure, PassiveScanHciFirstRunnerStep,
        PassiveScanHciFirstRunning, PassiveScanHciRecurringFailure, PassiveScanHciRecurringRunner,
        PassiveScanHciRecurringRunnerStep, PassiveScanHciReportStep, PassiveScanHciReportsComplete,
        PassiveScanHciReportsPending, PassiveScanHciResponsePendingSession,
        PassiveScanHciResponsePublication, PassiveScanHciStopping, PassiveScanHciStoppingFault,
        PassiveScanHciStoppingStep,
    },
    runner::{
        PassiveScanFirstRunner, PassiveScanFirstRunnerFailure,
        PassiveScanFirstRunnerPublicationFailStop, PassiveScanFirstRunnerRetry,
        PassiveScanFirstRunnerRetryCause, PassiveScanFirstRunnerStep, PassiveScanFirstRunning,
    },
    timing::PassiveScanEventPhase,
};

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod scheduler;

// Public scheduler integration, formerly exported from `scheduler`.
#[cfg(any(target_arch = "riscv32", test))]
pub use scheduler::PassiveScanSequenceObservation;
#[cfg(any(target_arch = "riscv32", test))]
pub use scheduler::{
    PassiveScanAdmissionObservation, PassiveScanEmptySchedulerMergeFailure,
    PassiveScanEmptySchedulerMergePrepared, PassiveScanEventPrepared,
    PassiveScanFirstEventCandidate, PassiveScanFirstEventPreparationError,
    PassiveScanFirstEventPreparationFailure, PassiveScanFirstPreSequence,
};
#[cfg(target_arch = "riscv32")]
pub use scheduler::{
    PassiveScanSchedulerHeadPublicationFailure, PassiveScanSchedulerHeadPublished,
};
