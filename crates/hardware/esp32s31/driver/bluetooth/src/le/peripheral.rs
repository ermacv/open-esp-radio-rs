//! Peripheral connection establishment and completion.

#[cfg(target_arch = "riscv32")]
pub(crate) mod completion;
pub(crate) mod connection;
#[cfg(target_arch = "riscv32")]
pub(crate) mod first_hci;
#[cfg(target_arch = "riscv32")]
pub(crate) mod start;

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
