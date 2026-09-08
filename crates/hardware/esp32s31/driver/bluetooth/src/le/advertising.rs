//! Legacy advertising event lifecycles.

pub(crate) mod connectable;
pub(crate) mod legacy;

#[cfg(feature = "dtm-diagnostics")]
pub mod diagnostics;

pub use connectable::LegacyConnectableAdvertisingRuntimeResources;
#[cfg(target_arch = "riscv32")]
pub use connectable::{
    active::{
        LegacyConnectableAdvertisingActiveFailStop,
        LegacyConnectableAdvertisingActiveFailStopCause, LegacyConnectableAdvertisingActiveSession,
        LegacyConnectableAdvertisingActiveWait,
        LegacyConnectableAdvertisingAwaitingPeripheralStart,
        LegacyConnectableAdvertisingAwaitingRecurrence,
        LegacyConnectableAdvertisingRadioContinuations,
    },
    hci::{
        LegacyConnectableAdvertisingActivePendingFailStop,
        LegacyConnectableAdvertisingActiveResponsePending,
        LegacyConnectableAdvertisingActiveResponsePublication,
        LegacyConnectableAdvertisingCommandIntake, LegacyConnectableAdvertisingCommandMismatch,
        LegacyConnectableAdvertisingCommandRoute,
        LegacyConnectableAdvertisingConnectionAcceptedReady,
        LegacyConnectableAdvertisingConnectionAcceptedResponsePending,
        LegacyConnectableAdvertisingConnectionAcceptedResponsePublication,
        LegacyConnectableAdvertisingConnectionAcceptedStopping,
        LegacyConnectableAdvertisingHciActiveFailStop,
        LegacyConnectableAdvertisingHciActiveSession, LegacyConnectableAdvertisingHciActiveStep,
        LegacyConnectableAdvertisingNoConnectionReady,
        LegacyConnectableAdvertisingNoConnectionResponsePending,
        LegacyConnectableAdvertisingNoConnectionResponsePublication,
        LegacyConnectableAdvertisingNoConnectionStopping, LegacyConnectableAdvertisingStopKind,
        LegacyConnectableAdvertisingStopOrder, LegacyConnectableAdvertisingStopping,
        LegacyConnectableAdvertisingStoppingFailStop, LegacyConnectableAdvertisingStoppingStep,
    },
    recurring::{
        hci::{
            BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
            LegacyConnectableAdvertisingRecurringCommandHandler,
            LegacyConnectableAdvertisingRecurringCommandMismatch,
            LegacyConnectableAdvertisingRecurringCommandReady,
            LegacyConnectableAdvertisingRecurringHci,
            LegacyConnectableAdvertisingRecurringHciCancellationPending,
            LegacyConnectableAdvertisingRecurringHciFailStop,
            LegacyConnectableAdvertisingRecurringHciRetry,
            LegacyConnectableAdvertisingRecurringResponsePending,
            LegacyConnectableAdvertisingRecurringStopping,
        },
        sequence::{
            BluetoothLegacyConnectableAdvertisingRecurringFailStop,
            LegacyConnectableAdvertisingRecurrenceCancellationPending,
            LegacyConnectableAdvertisingRecurrenceCancelled,
            LegacyConnectableAdvertisingRecurrenceCandidate,
            LegacyConnectableAdvertisingRecurrenceGraphPrepared,
            LegacyConnectableAdvertisingRecurrenceMerged,
            LegacyConnectableAdvertisingRecurrencePrepared,
            LegacyConnectableAdvertisingRecurrenceScheduled,
            LegacyConnectableAdvertisingRecurrenceSequencePending,
            LegacyConnectableAdvertisingRecurrenceSequenceReady,
            LegacyConnectableAdvertisingRecurringFailStopCause,
            LegacyConnectableAdvertisingRecurringRetry,
            LegacyConnectableAdvertisingRecurringRetryCause,
        },
    },
    runner::{
        LegacyConnectableAdvertisingAtomicStartFailStopCause,
        LegacyConnectableAdvertisingConfigurationError, LegacyConnectableAdvertisingFirstRunner,
        LegacyConnectableAdvertisingFirstRunnerFailStop,
        LegacyConnectableAdvertisingFirstRunnerFailStopCause,
        LegacyConnectableAdvertisingFirstRunnerFailure,
        LegacyConnectableAdvertisingFirstRunnerRecovered,
        LegacyConnectableAdvertisingFirstRunnerRecoveredError,
        LegacyConnectableAdvertisingFirstRunnerRetry,
        LegacyConnectableAdvertisingFirstRunnerRetryCause,
        LegacyConnectableAdvertisingFirstRunnerStep, LegacyConnectableAdvertisingFirstRunning,
        LegacyConnectableAdvertisingPreparationFailStopCause,
        LegacyConnectableAdvertisingResponsePending,
        LegacyConnectableAdvertisingResponsePublication,
        LegacyConnectableAdvertisingRollbackFailStopCause,
    },
};

pub use legacy::{LegacyAdvertisingCancelled, LegacyAdvertisingDefaultTxPowerDbm};
#[cfg(target_arch = "riscv32")]
pub use legacy::{LegacyAdvertisingEventCompleted, LegacyAdvertisingEventScheduleFailure};
#[cfg(any(target_arch = "riscv32", test))]
pub use legacy::{
    LegacyAdvertisingFirstEventCandidate, LegacyAdvertisingFirstEventCandidateOutcome,
};

#[cfg(target_arch = "riscv32")]
pub use legacy::LegacyAdvertisingNextEventScheduled;

pub use legacy::{
    LegacyAdvertisingLinkStateReset, LegacyAdvertisingLinkStateResetOutcome,
    LegacyAdvertisingPreparationError, LegacyAdvertisingPreparationErrorKind,
    LegacyAdvertisingPrepared,
};
#[cfg(target_arch = "riscv32")]
pub use legacy::{
    LegacyAdvertisingRecurringCancelled, LegacyAdvertisingRecurringEventCandidate,
    LegacyAdvertisingRecurringPreparationError, LegacyAdvertisingRecurringPreparationFailure,
    active::{
        LegacyAdvertisingActiveCommandIntake, LegacyAdvertisingActiveCommandMismatch,
        LegacyAdvertisingActiveCommandRoute, LegacyAdvertisingActiveFault,
        LegacyAdvertisingActiveFaultCause, LegacyAdvertisingActivePendingFault,
        LegacyAdvertisingActivePendingRadioStep, LegacyAdvertisingActiveResponsePending,
        LegacyAdvertisingActiveResponsePublication, LegacyAdvertisingActiveSession,
        LegacyAdvertisingActiveStep, LegacyAdvertisingActiveWait,
        LegacyAdvertisingCpuOwnedCommandIntake, LegacyAdvertisingCpuOwnedCommandMismatch,
        LegacyAdvertisingCpuOwnedCommandRoute, LegacyAdvertisingCpuOwnedResetBarrier,
        LegacyAdvertisingCpuOwnedResponsePending, LegacyAdvertisingCpuOwnedResponsePublication,
        LegacyAdvertisingDisableResponsePending, LegacyAdvertisingDisableResponsePublication,
        LegacyAdvertisingDisableRestore, LegacyAdvertisingDisableRestoreStep,
        LegacyAdvertisingEventCpuOwned, LegacyAdvertisingResetCompletion,
        LegacyAdvertisingResetCompletionReady, LegacyAdvertisingResetResponsePending,
        LegacyAdvertisingResetResponsePublication, LegacyAdvertisingResetRestore,
        LegacyAdvertisingResetRestoreStep, LegacyAdvertisingResponsePendingSession,
        LegacyAdvertisingResponsePublication, LegacyAdvertisingStopping,
        LegacyAdvertisingStoppingFault, LegacyAdvertisingStoppingStep,
    },
};

#[cfg(any(target_arch = "riscv32", test))]
pub use legacy::timing::{LegacyAdvertisingEventPhase, LegacyAdvertisingTimingObservation};
pub use legacy::{
    LegacyAdvertisingRuntimeBeginError, LegacyAdvertisingRuntimeResources,
    LegacyAdvertisingSetError, prepare_legacy_advertising_set,
};
#[cfg(target_arch = "riscv32")]
pub use legacy::{
    recurring::{
        LegacyAdvertisingRecurringCommandIntake, LegacyAdvertisingRecurringCommandMismatch,
        LegacyAdvertisingRecurringCommandRoute, LegacyAdvertisingRecurringFault,
        LegacyAdvertisingRecurringFaultCause, LegacyAdvertisingRecurringOrderProgress,
        LegacyAdvertisingRecurringOrderState, LegacyAdvertisingRecurringResponsePublication,
        LegacyAdvertisingRecurringRetry, LegacyAdvertisingRecurringRetryCause,
        LegacyAdvertisingRecurringRunner, LegacyAdvertisingRecurringRunnerStep,
        LegacyAdvertisingRecurringStart, LegacyAdvertisingRecurringStopBegin,
        LegacyAdvertisingRecurringStopFault, LegacyAdvertisingRecurringStopRestore,
        LegacyAdvertisingRecurringStopRestoreStep,
    },
    runner::{
        LegacyAdvertisingDeferredStart, LegacyAdvertisingFirstRunner,
        LegacyAdvertisingFirstRunnerFailure, LegacyAdvertisingFirstRunnerRetry,
        LegacyAdvertisingFirstRunnerRetryCause, LegacyAdvertisingFirstRunnerStep,
        LegacyAdvertisingFirstRunning,
    },
};
