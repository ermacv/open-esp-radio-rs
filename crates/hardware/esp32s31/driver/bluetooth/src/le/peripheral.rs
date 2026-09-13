//! Peripheral connection establishment and completion.

#[cfg(target_arch = "riscv32")]
mod active;
#[cfg(all(test, not(target_arch = "riscv32")))]
#[path = "peripheral/active/acl.rs"]
mod active_acl;
#[cfg(all(test, not(target_arch = "riscv32")))]
#[path = "peripheral/active/host_events.rs"]
mod active_host_events;
#[cfg(target_arch = "riscv32")]
pub(crate) mod completion;
pub(crate) mod connection;
#[cfg(target_arch = "riscv32")]
pub(crate) mod first_hci;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod procedure;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod recovery;
#[cfg(target_arch = "riscv32")]
pub(crate) mod start;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod supervision;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod termination;

#[cfg(target_arch = "riscv32")]
pub use connection::PeripheralConnectionPacketStartTiming;
#[cfg(any(target_arch = "riscv32", test))]
pub use connection::PeripheralConnectionRecurringTimingError;

pub use connection::{
    Le1MPacketStartTiming, PeripheralConnectionFirstEventPrepared,
    PeripheralConnectionRuntimeAllocation, PeripheralConnectionRuntimeBeginError,
    PeripheralConnectionRuntimeClaimError, PeripheralConnectionRuntimeConfig,
    PeripheralConnectionRuntimeResources,
};
#[cfg(target_arch = "riscv32")]
pub use first_hci::{
    LegacyConnectablePeripheralFirstHciAxis, LegacyConnectablePeripheralFirstHciFailStop,
    LegacyConnectablePeripheralFirstHciProgress, LegacyConnectablePeripheralFirstHciRecovered,
    LegacyConnectablePeripheralFirstHciResetEvidence,
    LegacyConnectablePeripheralFirstHciResetFailStop,
    LegacyConnectablePeripheralFirstHciResetFailStopCause,
    LegacyConnectablePeripheralFirstHciResetOutcome, LegacyConnectablePeripheralFirstHciResetReady,
    LegacyConnectablePeripheralFirstHciResponsePublication,
    LegacyConnectablePeripheralFirstHciResponseWait, LegacyConnectablePeripheralFirstHciRetry,
    LegacyConnectablePeripheralFirstHciRunner, LegacyConnectablePeripheralFirstHciRunning,
    LegacyConnectablePeripheralFirstHciRunningOrder, LegacyConnectablePeripheralFirstHciStep,
    LegacyConnectablePeripheralFirstHciStoppingStep,
};
#[cfg(target_arch = "riscv32")]
pub use start::{
    BluetoothLegacyConnectablePeripheralFirstRetry, LegacyConnectablePeripheralFirstBeginStep,
    LegacyConnectablePeripheralFirstCompleted, LegacyConnectablePeripheralFirstCompletionFailStop,
    LegacyConnectablePeripheralFirstCompletionFailStopCause,
    LegacyConnectablePeripheralFirstCurrentFailStop, LegacyConnectablePeripheralFirstFailStop,
    LegacyConnectablePeripheralFirstFailStopCause, LegacyConnectablePeripheralFirstHeadPublished,
    LegacyConnectablePeripheralFirstNormalizationUnavailable,
    LegacyConnectablePeripheralFirstPreparationFailStop,
    LegacyConnectablePeripheralFirstPreparationPending,
    LegacyConnectablePeripheralFirstPreparationStep, LegacyConnectablePeripheralFirstPrepared,
    LegacyConnectablePeripheralFirstPublicationFailStop,
    LegacyConnectablePeripheralFirstPublicationStep, LegacyConnectablePeripheralFirstRecovered,
    LegacyConnectablePeripheralFirstRecycleFailStop,
    LegacyConnectablePeripheralFirstRecycleFailStopCause,
    LegacyConnectablePeripheralFirstRetryCause, LegacyConnectablePeripheralFirstRetryStep,
    LegacyConnectablePeripheralFirstRunStep, LegacyConnectablePeripheralFirstRunner,
    LegacyConnectablePeripheralFirstRunnerStep, LegacyConnectablePeripheralFirstRunning,
    LegacyConnectablePeripheralFirstRunningContinuations,
    LegacyConnectablePeripheralFirstRunningEvidence, LegacyConnectablePeripheralFirstRunningWait,
};

#[cfg(target_arch = "riscv32")]
pub(crate) use active::acl::PeripheralConnectionAcl;
#[cfg(target_arch = "riscv32")]
pub use active::{
    PeripheralConnectionActiveFault, PeripheralConnectionActiveFaultCause,
    PeripheralConnectionActiveSession, PeripheralConnectionActiveStep,
    PeripheralConnectionActiveWait, PeripheralConnectionCommandIntake,
    PeripheralConnectionCommandMismatch, PeripheralConnectionCommandRoute,
    PeripheralConnectionHostEventPublication, PeripheralConnectionResetBarrier,
    PeripheralConnectionResetFault, PeripheralConnectionResetStep,
};

#[cfg(feature = "dtm-diagnostics")]
pub mod diagnostics;
