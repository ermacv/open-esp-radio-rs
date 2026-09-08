//! Direct Test Mode event, storage and stop ownership.

#[cfg(feature = "dtm-diagnostics")]
pub mod diagnostics;

#[cfg(target_arch = "riscv32")]
pub(crate) mod active;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod command;
pub(crate) mod event;
pub(crate) mod link_state;
pub(crate) mod parameters;
pub(crate) mod payload;
pub(crate) mod post_unlink;
#[cfg(target_arch = "riscv32")]
pub(crate) mod quiescence;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod quiescence_policy;
#[cfg(target_arch = "riscv32")]
pub(crate) mod reset;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod reset_order;
#[cfg(target_arch = "riscv32")]
pub(crate) mod runner;
pub(crate) mod rx;
pub(crate) mod scheduler;
pub(crate) mod session;
#[cfg(target_arch = "riscv32")]
pub(crate) mod stopping;
pub(crate) mod timing;
pub(crate) mod tx;

#[cfg(target_arch = "riscv32")]
pub use active::{
    DtmActiveCompletion, DtmActiveCompletionFault, DtmActiveCompletionFaultCause,
    DtmActiveCompletionStep, DtmActiveCpuOwned, DtmActivePostUnlinkWait, DtmActiveReceiverReady,
    DtmActiveSchedulerWait, DtmActiveTransmitterReady, DtmRecurringCancellationDrain,
    DtmRecurringCancellationDrainStep, DtmRecurringControllerTimeWait, DtmRecurringFault,
    DtmRecurringFaultCause, DtmRecurringRetry, DtmRecurringRetryCause, DtmRecurringRunner,
    DtmRecurringRunnerCancel, DtmRecurringRunnerStep,
    session::{
        DtmActiveCommandIntake, DtmActiveCommandMismatch, DtmActiveControllerCommandRoute,
        DtmActiveRadioWait, DtmActiveResetBarrier, DtmActiveSession, DtmActiveSessionFault,
        DtmActiveSessionFaultCause, DtmActiveSessionRadioStep, DtmCommandReadySession,
        DtmOrderReady, DtmResponsePending, DtmResponsePendingSession, DtmResponsePublication,
    },
};
#[cfg(target_arch = "riscv32")]
pub use event::prepare::{
    DtmActiveReceiverCpuOwned, DtmActiveTransmitterCpuOwned, DtmRecycledEvent, DtmRxRearmedEvent,
    DtmTestEndReport, DtmTestEndedCpuOwned,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use event::prepare::{
    DtmReceiverCpuOwned, DtmReceiverEvent, DtmSchedulerItemPhase, DtmTransmitterEvent,
};
#[cfg(target_arch = "riscv32")]
pub use post_unlink::BluetoothPostUnlinkAwaiting;

pub use link_state::{DtmDefaultTxPowerDbm, DtmLinkStateReviewedWords, DtmRole};

pub use parameters::{DtmChannel, DtmChannelError, DtmPhy, DtmPhyError, DtmPhyRoleError};

pub use payload::{
    DtmPayloadLength, DtmPayloadPattern, DtmPayloadPatternError, DtmPayloadPreparationError,
    DtmPreparedPayload,
};

pub use post_unlink::{
    DtmPostUnlinkMailboxPublication, DtmPostUnlinkWakeCell, PrimaryOrdinaryPublication,
    PrimarySerializedServiceStep,
};
#[cfg(target_arch = "riscv32")]
pub use reset::{
    DtmResetComplete, DtmResetCompletionReady, DtmResetCompletionStart, DtmResetResponsePending,
    DtmResetResponsePublication, DtmResetRestoreFailure, DtmResetRestoreStep,
    DtmResetStoppingFault, DtmResetStoppingFaultCause, DtmResetStoppingRetryCause,
    DtmResetStoppingRunner, DtmResetStoppingStep, DtmResetStoppingWait,
};
#[cfg(target_arch = "riscv32")]
pub use runner::{
    ControllerIdleCommandMismatch, ControllerIdleCommandRoute, DtmDeferredStart,
    DtmFirstAcceptedFailure, DtmFirstCancellationCleanTask, DtmFirstCancellationEpoch,
    DtmFirstCancellationFailStop, DtmFirstCancellationFailStopReason,
    DtmFirstCancellationPreparationCleanup, DtmFirstCancellationPreparationCleanupStep,
    DtmFirstColdTimeDrain, DtmFirstColdTimeDrainStep, DtmFirstIdleRestore, DtmFirstIdleRestoreStep,
    DtmFirstInvariantFault, DtmFirstPreparationCleanTask, DtmFirstPreparationCleanup,
    DtmFirstPreparationCleanupStep, DtmFirstPreparationCompletion, DtmFirstPreparationFailStop,
    DtmFirstRunner, DtmFirstRunnerCancel, DtmFirstRunnerFailure, DtmFirstRunnerRetry,
    DtmFirstRunnerRetryCause, DtmFirstRunnerStep, DtmFirstRunning, DtmFirstWarmTimeDrain,
    DtmFirstWarmTimeDrainStep,
};
#[cfg(target_arch = "riscv32")]
pub use stopping::{
    BluetoothDtmStoppingWait, DtmStoppingFault, DtmStoppingFaultCause, DtmStoppingRetryCause,
    DtmStoppingRunner, DtmStoppingStep, DtmTestEndComplete, DtmTestEndReady,
    DtmTestEndResponsePending, DtmTestEndResponsePublication, DtmTestEndRestoreFailure,
    DtmTestEndRestoreStep,
};

pub use rx::{DtmReceiverSession, DtmRxCompletionOutcome};

pub use scheduler::item::{
    DtmSchedulerItemEvent, DtmSchedulerItemEventError, DtmSchedulerItemReviewedWords,
};

pub use session::{
    DtmRuntimeConfig, DtmRuntimeResources, DtmRuntimeSessionBeginError, DtmSessionIdle,
};

pub use timing::{DtmTxSchedulerTiming, DtmTxTimingMicros};

pub use tx::{
    BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES, BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES,
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, DtmPreparedTxGraph, DtmTxGraphPrepare,
};
