//! ESP32-S31 Bluetooth hardware backend and executor-neutral radio sessions.
//!
//! PAC/HAL owns MMIO, and the separate memory crate owns controller-SRAM
//! layouts and CPU/hardware ownership. This crate joins those contracts to
//! portable LL policy, controller time, scheduler admission and publication.
//! Its HCI composition preserves command order while radio events progress;
//! the Embassy adapter owns executor waits and task storage.
//!
//! DTM, advertising and scanning have event lifecycles. Peripheral connection
//! has a causal first-event path, completion/recycle and lower recurrence
//! operations, but the active connection loop and reliable ACL dataplane are
//! not yet integrated. Initialization and scheduler RUN are not RF evidence.
//!
//! Shared single-item completion and timed preparation engines implement the
//! common hardware protocol; RX/recycle and packet policy remain role-specific.
//! The public lifecycle begins with one [`BluetoothStopped`] aggregate retaining
//! the platform lease and neutral radio root. Complete powered teardown and
//! long-running PHY maintenance remain separate requirements.
//!
//! See `FEATURES.md` for the implementation frontier and ordered closure plan.

#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
mod baseband;
mod ble_phy;
mod clock;
#[cfg(target_arch = "riscv32")]
mod common_phy_state;
mod connectable_advertising;
#[cfg(any(target_arch = "riscv32", test))]
mod controller_hal;
mod controller_start;
mod controller_time;
#[cfg(target_arch = "riscv32")]
mod dtm_active;
#[cfg(target_arch = "riscv32")]
mod dtm_active_session;
#[cfg(any(target_arch = "riscv32", test))]
mod dtm_command;
#[cfg(any(target_arch = "riscv32", test))]
mod dtm_event_prepare;
mod dtm_event_timing;
mod dtm_link_state;
mod dtm_parameters;
mod dtm_payload;
mod dtm_post_unlink;
#[cfg(target_arch = "riscv32")]
mod dtm_quiescence;
#[cfg(any(target_arch = "riscv32", test))]
mod dtm_quiescence_policy;
#[cfg(target_arch = "riscv32")]
mod dtm_reset;
#[cfg(any(target_arch = "riscv32", test))]
mod dtm_reset_order;
#[cfg(target_arch = "riscv32")]
mod dtm_runner;
mod dtm_rx_completion;
mod dtm_scheduler_item;
#[cfg(any(target_arch = "riscv32", test))]
mod dtm_scheduler_reservation;
mod dtm_session;
#[cfg(target_arch = "riscv32")]
mod dtm_stopping;
mod dtm_timing;
mod dtm_tx_packet;
#[cfg(any(target_arch = "riscv32", test))]
mod hci;
mod interrupt;
mod interrupt_classifier;
mod interrupt_wake;
mod legacy_advertising;
#[cfg(target_arch = "riscv32")]
mod legacy_advertising_active;
#[cfg(target_arch = "riscv32")]
mod legacy_advertising_completion;
#[cfg(target_arch = "riscv32")]
mod legacy_advertising_recurring;
#[cfg(target_arch = "riscv32")]
mod legacy_advertising_runner;
#[cfg(any(target_arch = "riscv32", test))]
mod legacy_advertising_timing;
#[cfg(target_arch = "riscv32")]
mod legacy_connectable_advertising_active;
#[cfg(target_arch = "riscv32")]
mod legacy_connectable_advertising_completion;
#[cfg(target_arch = "riscv32")]
mod legacy_connectable_advertising_hci;
#[cfg(target_arch = "riscv32")]
mod legacy_connectable_advertising_recurring;
#[cfg(target_arch = "riscv32")]
mod legacy_connectable_advertising_recurring_hci;
#[cfg(any(target_arch = "riscv32", test))]
mod legacy_connectable_advertising_recurring_hci_state;
#[cfg(target_arch = "riscv32")]
mod legacy_connectable_advertising_runner;
#[cfg(target_arch = "riscv32")]
mod legacy_connectable_peripheral_first_hci;
#[cfg(target_arch = "riscv32")]
mod legacy_connectable_peripheral_start_runner;
#[cfg(any(target_arch = "riscv32", test))]
mod low_power;
mod modem_lp_timer_queue;
mod nrt_interrupt;
mod passive_scanning;
#[cfg(target_arch = "riscv32")]
mod passive_scanning_active;
#[cfg(target_arch = "riscv32")]
mod passive_scanning_hci;
#[cfg(target_arch = "riscv32")]
mod passive_scanning_runner;
#[cfg(any(target_arch = "riscv32", test))]
mod passive_scanning_timing;
mod peripheral_connection;
#[cfg(target_arch = "riscv32")]
mod peripheral_connection_completion;
#[cfg(target_arch = "riscv32")]
mod phy;
mod primary_interrupt;
mod resources;
mod runtime_resources;
#[cfg(any(target_arch = "riscv32", test))]
mod scheduler;
mod scheduler_config;
mod scheduler_finished_lists;
mod scheduler_insertion;
mod scheduler_lock_modify;
mod scheduler_time;
#[cfg(any(target_arch = "riscv32", test))]
mod scheduler_timeline;
#[cfg(any(test, target_arch = "riscv32"))]
mod single_item_completion;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

#[cfg(target_arch = "riscv32")]
pub use baseband::{BluetoothBasebandInitializationReport, BluetoothControllerBasebandInitialized};
#[cfg(target_arch = "riscv32")]
pub(crate) use ble_phy::BluetoothAlwaysAwakeTimingReady;
pub use ble_phy::BluetoothBlePhyInitializationReport;
#[cfg(target_arch = "riscv32")]
pub use ble_phy::BluetoothControllerBlePhyEngineInitialized;
pub use clock::{
    BluetoothClockCheckpoint, BluetoothClockEnableFailure, BluetoothClockError,
    BluetoothClockState, BluetoothClockedResources,
};
#[cfg(target_arch = "riscv32")]
pub use common_phy_state::{
    BluetoothControllerPhyInitialized, BluetoothControllerPhyRegistered,
    BluetoothPhyInitializationReport,
};
pub use connectable_advertising::BluetoothLegacyConnectableAdvertisingRuntimeResources;
#[cfg(target_arch = "riscv32")]
pub use controller_hal::BluetoothControllerHalInitialized;
#[cfg(target_arch = "riscv32")]
pub use controller_start::peripheral_connection::{
    BluetoothPeripheralConnectionCompletionStep,
    BluetoothPeripheralConnectionControllerPreparationError,
    BluetoothPeripheralConnectionRecurringCandidateStep,
    BluetoothPeripheralConnectionRecurringRetry,
    BluetoothPeripheralConnectionRecurringSequenceCompletion,
};
#[cfg(target_arch = "riscv32")]
pub use controller_start::{
    BluetoothAlwaysAwakePostEnableTimeBeginError, BluetoothAlwaysAwakePostEnableTimeBeginFailure,
    BluetoothAlwaysAwakePostEnableTimeError, BluetoothAlwaysAwakePostEnableTimeFailure,
    BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep, BluetoothAlwaysAwakePostEnableTimePending,
    BluetoothAlwaysAwakePostEnableTimeStep, BluetoothAlwaysAwakeTimeObservedAfterEnable,
    BluetoothControllerIdleCommandIntake, BluetoothControllerIdleCommandTask,
    BluetoothControllerIdleResetBarrier, BluetoothControllerIdleResetCompletion,
    BluetoothControllerIdleResponsePending, BluetoothControllerIdleResponsePublication,
    BluetoothControllerInterruptOwnerPublicationFailure,
    BluetoothControllerInterruptOwnersPublished, BluetoothControllerInterruptOwnersReady,
    BluetoothControllerModemTimerBegin, BluetoothControllerModemTimerReadiness,
    BluetoothControllerModemTimerReadinessClass, BluetoothControllerModemTimerRearm,
    BluetoothControllerModemTimerStep, BluetoothControllerModemTimerTask,
    BluetoothControllerOutputTimerStarted, BluetoothControllerPublishedInterruptService,
    BluetoothControllerPublishedRuntimeEndpoints, BluetoothControllerPublishedRuntimeSplit,
    BluetoothControllerPublishedRuntimeSplitFailure, BluetoothControllerPublishedTaskService,
    BluetoothControllerSchedulerCurrentBeginError, BluetoothControllerSchedulerCurrentBeginFailure,
    BluetoothControllerSchedulerCurrentError, BluetoothControllerSchedulerCurrentFailure,
    BluetoothControllerSchedulerCurrentPending, BluetoothControllerSchedulerCurrentStep,
    BluetoothControllerSchedulerEpochRetained, BluetoothControllerSchedulerEpochUnavailable,
    BluetoothControllerSchedulerNowReady, BluetoothControllerTimeOrphanDrainStep,
    BluetoothDtmControllerInitialPreparationFailure, BluetoothDtmControllerPreparationOutcome,
    BluetoothDtmControllerPreparationPending, BluetoothDtmControllerPreparationStep,
    BluetoothDtmControllerPreparationTerminal, BluetoothDtmPostUnlinkArmStep,
    BluetoothDtmSchedulerStartFailure, BluetoothDtmSoftwareListRemovalPublishedStep,
    BluetoothInterruptOwnerStorage, BluetoothLePacketStartTimingError,
    BluetoothLegacyAdvertisingControllerCancellationPending,
    BluetoothLegacyAdvertisingControllerCancellationStep,
    BluetoothLegacyAdvertisingControllerPreparationError,
    BluetoothLegacyAdvertisingControllerPreparationFailStop,
    BluetoothLegacyAdvertisingControllerPreparationFailStopCause,
    BluetoothLegacyAdvertisingControllerPreparationOutcome,
    BluetoothLegacyAdvertisingControllerPreparationPending,
    BluetoothLegacyAdvertisingControllerPreparationStep,
    BluetoothLegacyAdvertisingControllerPreparationTerminal,
    BluetoothLegacyAdvertisingSchedulerStartFailure, BluetoothModemLpTimerInterruptDispatchStorage,
    BluetoothModemLpTimerSoftwareOwnerStorage, BluetoothPassiveScanControllerCancellationPending,
    BluetoothPassiveScanControllerCancellationStep, BluetoothPassiveScanControllerPreparationError,
    BluetoothPassiveScanControllerPreparationFailStop,
    BluetoothPassiveScanControllerPreparationFailStopCause,
    BluetoothPassiveScanControllerPreparationOutcome,
    BluetoothPassiveScanControllerPreparationPending,
    BluetoothPassiveScanControllerPreparationStep,
    BluetoothPassiveScanControllerPreparationTerminal, BluetoothPassiveScanSchedulerStartFailure,
    BluetoothPeripheralConnectionSchedulerStartFailure, BluetoothSchedulerRunInterruptStorage,
    BluetoothSharedInterruptDispatchStorage,
};
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use controller_time::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_active::{
    BluetoothDtmActiveCompletion, BluetoothDtmActiveCompletionFault,
    BluetoothDtmActiveCompletionFaultCause, BluetoothDtmActiveCompletionStep,
    BluetoothDtmActiveCpuOwned, BluetoothDtmActivePostUnlinkWait, BluetoothDtmActiveReceiverReady,
    BluetoothDtmActiveSchedulerWait, BluetoothDtmActiveTransmitterReady,
    BluetoothDtmRecurringCancellationDrain, BluetoothDtmRecurringCancellationDrainStep,
    BluetoothDtmRecurringControllerTimeWait, BluetoothDtmRecurringFault,
    BluetoothDtmRecurringFaultCause, BluetoothDtmRecurringRetry, BluetoothDtmRecurringRetryCause,
    BluetoothDtmRecurringRunner, BluetoothDtmRecurringRunnerCancel,
    BluetoothDtmRecurringRunnerStep,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_active_session::{
    BluetoothDtmActiveCommandIntake, BluetoothDtmActiveCommandMismatch,
    BluetoothDtmActiveControllerCommandRoute, BluetoothDtmActiveRadioWait,
    BluetoothDtmActiveResetBarrier, BluetoothDtmActiveSession, BluetoothDtmActiveSessionFault,
    BluetoothDtmActiveSessionFaultCause, BluetoothDtmActiveSessionRadioStep,
    BluetoothDtmCommandReadySession, BluetoothDtmOrderReady, BluetoothDtmResponsePending,
    BluetoothDtmResponsePendingSession, BluetoothDtmResponsePublication,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_event_prepare::{
    BluetoothDtmActiveReceiverCpuOwned, BluetoothDtmActiveTransmitterCpuOwned,
    BluetoothDtmRecycledEvent, BluetoothDtmRxRearmedEvent, BluetoothDtmTestEndReport,
    BluetoothDtmTestEndedCpuOwned,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use dtm_event_prepare::{
    BluetoothDtmReceiverCpuOwned, BluetoothDtmReceiverEvent, BluetoothDtmSchedulerItemPhase,
    BluetoothDtmTransmitterEvent,
};
pub(crate) use dtm_event_timing::{
    BluetoothDtmRxInitialEventWindow, BluetoothDtmRxRecurringEventWindow, BluetoothDtmTxEventWindow,
};
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use dtm_link_state::BluetoothDtmLinkStateReset;
pub use dtm_link_state::{
    BluetoothDtmDefaultTxPowerDbm, BluetoothDtmLinkStateReviewedWords, BluetoothDtmRole,
};
pub use dtm_parameters::{
    BluetoothDtmChannel, BluetoothDtmChannelError, BluetoothDtmPhy, BluetoothDtmPhyError,
    BluetoothDtmPhyRoleError,
};
pub use dtm_payload::{
    BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern, BluetoothDtmPayloadPatternError,
    BluetoothDtmPayloadPreparationError, BluetoothDtmPreparedPayload,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_post_unlink::BluetoothPostUnlinkAwaiting;
pub use dtm_post_unlink::{
    BluetoothDtmPostUnlinkMailboxPublication, BluetoothDtmPostUnlinkWakeCell,
    BluetoothPrimaryOrdinaryPublication, BluetoothPrimarySerializedServiceStep,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_reset::{
    BluetoothDtmResetComplete, BluetoothDtmResetCompletionReady, BluetoothDtmResetCompletionStart,
    BluetoothDtmResetResponsePending, BluetoothDtmResetResponsePublication,
    BluetoothDtmResetRestoreFailure, BluetoothDtmResetRestoreStep, BluetoothDtmResetStoppingFault,
    BluetoothDtmResetStoppingFaultCause, BluetoothDtmResetStoppingRetryCause,
    BluetoothDtmResetStoppingRunner, BluetoothDtmResetStoppingStep, BluetoothDtmResetStoppingWait,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_runner::{
    BluetoothControllerIdleCommandMismatch, BluetoothControllerIdleCommandRoute,
    BluetoothDtmDeferredStart, BluetoothDtmFirstAcceptedFailure,
    BluetoothDtmFirstCancellationCleanTask, BluetoothDtmFirstCancellationEpoch,
    BluetoothDtmFirstCancellationFailStop, BluetoothDtmFirstCancellationFailStopReason,
    BluetoothDtmFirstCancellationPreparationCleanup,
    BluetoothDtmFirstCancellationPreparationCleanupStep, BluetoothDtmFirstColdTimeDrain,
    BluetoothDtmFirstColdTimeDrainStep, BluetoothDtmFirstIdleRestore,
    BluetoothDtmFirstIdleRestoreStep, BluetoothDtmFirstInvariantFault,
    BluetoothDtmFirstPreparationCleanTask, BluetoothDtmFirstPreparationCleanup,
    BluetoothDtmFirstPreparationCleanupStep, BluetoothDtmFirstPreparationCompletion,
    BluetoothDtmFirstPreparationFailStop, BluetoothDtmFirstRunner, BluetoothDtmFirstRunnerCancel,
    BluetoothDtmFirstRunnerFailure, BluetoothDtmFirstRunnerRetry,
    BluetoothDtmFirstRunnerRetryCause, BluetoothDtmFirstRunnerStep, BluetoothDtmFirstRunning,
    BluetoothDtmFirstWarmTimeDrain, BluetoothDtmFirstWarmTimeDrainStep,
};
pub use dtm_rx_completion::{BluetoothDtmReceiverSession, BluetoothDtmRxCompletionOutcome};
pub use dtm_scheduler_item::{
    BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerItemEventError,
    BluetoothDtmSchedulerItemReviewedWords,
};
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use dtm_scheduler_reservation::{
    BluetoothDtmSchedulerReservation, BluetoothDtmSchedulerSequenceAuthorizationFailure,
};
#[cfg(test)]
pub(crate) use dtm_session::BluetoothDtmSessionStopping;
pub use dtm_session::{
    BluetoothDtmRuntimeConfig, BluetoothDtmRuntimeResources, BluetoothDtmRuntimeSessionBeginError,
    BluetoothDtmSessionIdle,
};
#[cfg(target_arch = "riscv32")]
pub use dtm_stopping::{
    BluetoothDtmStoppingFault, BluetoothDtmStoppingFaultCause, BluetoothDtmStoppingRetryCause,
    BluetoothDtmStoppingRunner, BluetoothDtmStoppingStep, BluetoothDtmStoppingWait,
    BluetoothDtmTestEndComplete, BluetoothDtmTestEndReady, BluetoothDtmTestEndResponsePending,
    BluetoothDtmTestEndResponsePublication, BluetoothDtmTestEndRestoreFailure,
    BluetoothDtmTestEndRestoreStep,
};
pub use dtm_timing::{BluetoothDtmTxSchedulerTiming, BluetoothDtmTxTimingMicros};
pub use dtm_tx_packet::{
    BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES, BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES,
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, BluetoothDtmPreparedTxGraph, BluetoothDtmTxGraphPrepare,
};
#[cfg(target_arch = "riscv32")]
pub use hci::{
    BluetoothControllerHciBindError, BluetoothControllerHciBindFailure, BluetoothControllerHciBound,
};
pub use interrupt::{
    BluetoothCpuInterruptRoutePolicy, BluetoothCpuInterruptSource,
    BluetoothInterruptHandlerResidency,
};
pub use interrupt_classifier::{
    BluetoothPrimaryControllerFault, BluetoothPrimaryInterruptClassification,
    BluetoothPrimarySchedulerTrigger, BluetoothSchedulerReferenceAction,
    BluetoothSchedulerReferenceGate, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerWorkClassifier, BluetoothSchedulerWorkObservation,
    BluetoothSchedulerWorkerWake, BluetoothSchedulerWorkerWakeClass,
};
pub use interrupt_wake::{
    BluetoothSchedulerWakeBatch, BluetoothSchedulerWakeCell, BluetoothSchedulerWakePublication,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use legacy_advertising::BluetoothLegacyAdvertisingCancelledRestoreOutcome;
pub use legacy_advertising::{
    BluetoothLegacyAdvertisingCancelled, BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    BluetoothLegacyAdvertisingLinkStateReset, BluetoothLegacyAdvertisingLinkStateResetOutcome,
    BluetoothLegacyAdvertisingPreparationError, BluetoothLegacyAdvertisingPreparationErrorKind,
    BluetoothLegacyAdvertisingPrepared, BluetoothLegacyAdvertisingRuntimeBeginError,
    BluetoothLegacyAdvertisingRuntimeResources, BluetoothLegacyAdvertisingSetError,
    prepare_legacy_advertising_set,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_advertising::{
    BluetoothLegacyAdvertisingEventCompleted, BluetoothLegacyAdvertisingEventScheduleFailure,
    BluetoothLegacyAdvertisingNextEventScheduled, BluetoothLegacyAdvertisingRecurringCancelled,
    BluetoothLegacyAdvertisingRecurringEventCandidate,
    BluetoothLegacyAdvertisingRecurringPreparationError,
    BluetoothLegacyAdvertisingRecurringPreparationFailure,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use legacy_advertising::{
    BluetoothLegacyAdvertisingFirstEventCandidate,
    BluetoothLegacyAdvertisingFirstEventCandidateOutcome,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_advertising_active::{
    BluetoothLegacyAdvertisingActiveCommandIntake, BluetoothLegacyAdvertisingActiveCommandMismatch,
    BluetoothLegacyAdvertisingActiveCommandRoute, BluetoothLegacyAdvertisingActiveFault,
    BluetoothLegacyAdvertisingActiveFaultCause, BluetoothLegacyAdvertisingActivePendingFault,
    BluetoothLegacyAdvertisingActivePendingRadioStep,
    BluetoothLegacyAdvertisingActiveResponsePending,
    BluetoothLegacyAdvertisingActiveResponsePublication, BluetoothLegacyAdvertisingActiveSession,
    BluetoothLegacyAdvertisingActiveStep, BluetoothLegacyAdvertisingActiveWait,
    BluetoothLegacyAdvertisingCpuOwnedCommandIntake,
    BluetoothLegacyAdvertisingCpuOwnedCommandMismatch,
    BluetoothLegacyAdvertisingCpuOwnedCommandRoute, BluetoothLegacyAdvertisingCpuOwnedResetBarrier,
    BluetoothLegacyAdvertisingCpuOwnedResponsePending,
    BluetoothLegacyAdvertisingCpuOwnedResponsePublication,
    BluetoothLegacyAdvertisingDisableResponsePending,
    BluetoothLegacyAdvertisingDisableResponsePublication, BluetoothLegacyAdvertisingDisableRestore,
    BluetoothLegacyAdvertisingDisableRestoreStep, BluetoothLegacyAdvertisingEventCpuOwned,
    BluetoothLegacyAdvertisingResetCompletion, BluetoothLegacyAdvertisingResetCompletionReady,
    BluetoothLegacyAdvertisingResetResponsePending,
    BluetoothLegacyAdvertisingResetResponsePublication, BluetoothLegacyAdvertisingResetRestore,
    BluetoothLegacyAdvertisingResetRestoreStep, BluetoothLegacyAdvertisingResponsePendingSession,
    BluetoothLegacyAdvertisingResponsePublication, BluetoothLegacyAdvertisingStopping,
    BluetoothLegacyAdvertisingStoppingFault, BluetoothLegacyAdvertisingStoppingStep,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_advertising_recurring::{
    BluetoothLegacyAdvertisingRecurringCommandIntake,
    BluetoothLegacyAdvertisingRecurringCommandMismatch,
    BluetoothLegacyAdvertisingRecurringCommandRoute, BluetoothLegacyAdvertisingRecurringFault,
    BluetoothLegacyAdvertisingRecurringFaultCause,
    BluetoothLegacyAdvertisingRecurringOrderProgress,
    BluetoothLegacyAdvertisingRecurringOrderState,
    BluetoothLegacyAdvertisingRecurringResponsePublication,
    BluetoothLegacyAdvertisingRecurringRetry, BluetoothLegacyAdvertisingRecurringRetryCause,
    BluetoothLegacyAdvertisingRecurringRunner, BluetoothLegacyAdvertisingRecurringRunnerStep,
    BluetoothLegacyAdvertisingRecurringStart, BluetoothLegacyAdvertisingRecurringStopBegin,
    BluetoothLegacyAdvertisingRecurringStopFault, BluetoothLegacyAdvertisingRecurringStopRestore,
    BluetoothLegacyAdvertisingRecurringStopRestoreStep,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_advertising_runner::{
    BluetoothLegacyAdvertisingDeferredStart, BluetoothLegacyAdvertisingFirstRunner,
    BluetoothLegacyAdvertisingFirstRunnerFailure, BluetoothLegacyAdvertisingFirstRunnerRetry,
    BluetoothLegacyAdvertisingFirstRunnerRetryCause, BluetoothLegacyAdvertisingFirstRunnerStep,
    BluetoothLegacyAdvertisingFirstRunning,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use legacy_advertising_timing::BluetoothLegacyAdvertisingEventPhase;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use legacy_advertising_timing::BluetoothLegacyAdvertisingEventWindow;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use legacy_advertising_timing::BluetoothLegacyAdvertisingRecurringTimingObservation;
#[cfg(any(target_arch = "riscv32", test))]
pub use legacy_advertising_timing::BluetoothLegacyAdvertisingTimingObservation;
#[cfg(target_arch = "riscv32")]
pub use legacy_connectable_advertising_active::{
    BluetoothLegacyConnectableAdvertisingActiveFailStop,
    BluetoothLegacyConnectableAdvertisingActiveFailStopCause,
    BluetoothLegacyConnectableAdvertisingActiveSession,
    BluetoothLegacyConnectableAdvertisingActiveWait,
    BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart,
    BluetoothLegacyConnectableAdvertisingAwaitingRecurrence,
    BluetoothLegacyConnectableAdvertisingRadioContinuations,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_connectable_advertising_hci::{
    BluetoothLegacyConnectableAdvertisingActivePendingFailStop,
    BluetoothLegacyConnectableAdvertisingActiveResponsePending,
    BluetoothLegacyConnectableAdvertisingActiveResponsePublication,
    BluetoothLegacyConnectableAdvertisingCommandIntake,
    BluetoothLegacyConnectableAdvertisingCommandMismatch,
    BluetoothLegacyConnectableAdvertisingCommandRoute,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePublication,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping,
    BluetoothLegacyConnectableAdvertisingHciActiveFailStop,
    BluetoothLegacyConnectableAdvertisingHciActiveSession,
    BluetoothLegacyConnectableAdvertisingHciActiveStep,
    BluetoothLegacyConnectableAdvertisingNoConnectionReady,
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending,
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePublication,
    BluetoothLegacyConnectableAdvertisingNoConnectionStopping,
    BluetoothLegacyConnectableAdvertisingStopKind, BluetoothLegacyConnectableAdvertisingStopOrder,
    BluetoothLegacyConnectableAdvertisingStopping,
    BluetoothLegacyConnectableAdvertisingStoppingFailStop,
    BluetoothLegacyConnectableAdvertisingStoppingStep,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_connectable_advertising_recurring::{
    BluetoothLegacyConnectableAdvertisingRecurrenceCancellationPending,
    BluetoothLegacyConnectableAdvertisingRecurrenceCancelled,
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate,
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceScheduled,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady,
    BluetoothLegacyConnectableAdvertisingRecurringFailStop,
    BluetoothLegacyConnectableAdvertisingRecurringFailStopCause,
    BluetoothLegacyConnectableAdvertisingRecurringRetry,
    BluetoothLegacyConnectableAdvertisingRecurringRetryCause,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_connectable_advertising_recurring_hci::{
    BluetoothLegacyConnectableAdvertisingRecurringCommandHandler,
    BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch,
    BluetoothLegacyConnectableAdvertisingRecurringCommandReady,
    BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
    BluetoothLegacyConnectableAdvertisingRecurringHci,
    BluetoothLegacyConnectableAdvertisingRecurringHciCancellationPending,
    BluetoothLegacyConnectableAdvertisingRecurringHciFailStop,
    BluetoothLegacyConnectableAdvertisingRecurringHciRetry,
    BluetoothLegacyConnectableAdvertisingRecurringResponsePending,
    BluetoothLegacyConnectableAdvertisingRecurringStopping,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_connectable_advertising_runner::{
    BluetoothLegacyConnectableAdvertisingAtomicStartFailStopCause,
    BluetoothLegacyConnectableAdvertisingConfigurationError,
    BluetoothLegacyConnectableAdvertisingFirstRunner,
    BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop,
    BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause,
    BluetoothLegacyConnectableAdvertisingFirstRunnerFailure,
    BluetoothLegacyConnectableAdvertisingFirstRunnerRecovered,
    BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError,
    BluetoothLegacyConnectableAdvertisingFirstRunnerRetry,
    BluetoothLegacyConnectableAdvertisingFirstRunnerRetryCause,
    BluetoothLegacyConnectableAdvertisingFirstRunnerStep,
    BluetoothLegacyConnectableAdvertisingFirstRunning,
    BluetoothLegacyConnectableAdvertisingPreparationFailStopCause,
    BluetoothLegacyConnectableAdvertisingResponsePending,
    BluetoothLegacyConnectableAdvertisingResponsePublication,
    BluetoothLegacyConnectableAdvertisingRollbackFailStopCause,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_connectable_peripheral_first_hci::{
    BluetoothLegacyConnectablePeripheralFirstHciAxis,
    BluetoothLegacyConnectablePeripheralFirstHciFailStop,
    BluetoothLegacyConnectablePeripheralFirstHciProgress,
    BluetoothLegacyConnectablePeripheralFirstHciRecovered,
    BluetoothLegacyConnectablePeripheralFirstHciResetEvidence,
    BluetoothLegacyConnectablePeripheralFirstHciResetFailStop,
    BluetoothLegacyConnectablePeripheralFirstHciResetFailStopCause,
    BluetoothLegacyConnectablePeripheralFirstHciResetOutcome,
    BluetoothLegacyConnectablePeripheralFirstHciResetReady,
    BluetoothLegacyConnectablePeripheralFirstHciResponsePublication,
    BluetoothLegacyConnectablePeripheralFirstHciResponseWait,
    BluetoothLegacyConnectablePeripheralFirstHciRetry,
    BluetoothLegacyConnectablePeripheralFirstHciRunner,
    BluetoothLegacyConnectablePeripheralFirstHciRunning,
    BluetoothLegacyConnectablePeripheralFirstHciRunningOrder,
    BluetoothLegacyConnectablePeripheralFirstHciStep,
    BluetoothLegacyConnectablePeripheralFirstHciStoppingStep,
};
#[cfg(target_arch = "riscv32")]
pub use legacy_connectable_peripheral_start_runner::{
    BluetoothLegacyConnectablePeripheralFirstBeginStep,
    BluetoothLegacyConnectablePeripheralFirstCompleted,
    BluetoothLegacyConnectablePeripheralFirstCompletionFailStop,
    BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause,
    BluetoothLegacyConnectablePeripheralFirstCurrentFailStop,
    BluetoothLegacyConnectablePeripheralFirstFailStop,
    BluetoothLegacyConnectablePeripheralFirstFailStopCause,
    BluetoothLegacyConnectablePeripheralFirstHeadPublished,
    BluetoothLegacyConnectablePeripheralFirstNormalizationUnavailable,
    BluetoothLegacyConnectablePeripheralFirstPreparationFailStop,
    BluetoothLegacyConnectablePeripheralFirstPreparationPending,
    BluetoothLegacyConnectablePeripheralFirstPreparationStep,
    BluetoothLegacyConnectablePeripheralFirstPrepared,
    BluetoothLegacyConnectablePeripheralFirstPublicationFailStop,
    BluetoothLegacyConnectablePeripheralFirstPublicationStep,
    BluetoothLegacyConnectablePeripheralFirstRecovered,
    BluetoothLegacyConnectablePeripheralFirstRecycleFailStop,
    BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause,
    BluetoothLegacyConnectablePeripheralFirstRetry,
    BluetoothLegacyConnectablePeripheralFirstRetryCause,
    BluetoothLegacyConnectablePeripheralFirstRetryStep,
    BluetoothLegacyConnectablePeripheralFirstRunStep,
    BluetoothLegacyConnectablePeripheralFirstRunner,
    BluetoothLegacyConnectablePeripheralFirstRunnerStep,
    BluetoothLegacyConnectablePeripheralFirstRunning,
    BluetoothLegacyConnectablePeripheralFirstRunningContinuations,
    BluetoothLegacyConnectablePeripheralFirstRunningEvidence,
    BluetoothLegacyConnectablePeripheralFirstRunningWait,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use low_power::{
    BluetoothControllerLowPowerHardwareInitializationFailure,
    BluetoothControllerLowPowerHardwareInitialized, BluetoothControllerRuntimeEndpoints,
};
pub use modem_lp_timer_queue::{
    BluetoothModemLpTimerEventCell, BluetoothModemLpTimerEventPublication,
    BluetoothModemLpTimerExpiration, BluetoothModemLpTimerExpirationPending,
    BluetoothModemLpTimerInterruptRuntimeStep, BluetoothModemLpTimerPublishedInterruptStep,
    BluetoothModemLpTimerQueue, BluetoothModemLpTimerScheduleError,
    BluetoothModemLpTimerSoftwareStep, BluetoothModemLpTimerSoftwareWork,
    BluetoothModemLpTimerStableInterruptStep, BluetoothModemLpTimerToken,
    BluetoothModemLpTimerWorkerWakeCell, BluetoothModemLpTimerWorkerWakePublication,
    step_modem_lp_timer_interrupt,
};
pub use nrt_interrupt::{BluetoothNrtDefaultInterruptEpoch, step_nrt_default_interrupt};
#[cfg(target_arch = "riscv32")]
pub use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemCompletionStatus;
#[cfg(not(target_arch = "riscv32"))]
pub use open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingMemoryGraphModelAddress;
pub use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothBlePhyEngineBindError, BluetoothBlePhyEngineBindFailure,
    BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineStorage,
    BluetoothControllerSramLinkAddress, BluetoothControllerSramLinkAddressError,
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphPrepareError,
    BluetoothDtmMemoryGraphPrepareFailure, BluetoothDtmMemoryGraphReclaimed,
    BluetoothDtmPositionalEventWords, BluetoothDtmRxResultProjection,
    BluetoothDtmRxResultProjectionError, BluetoothDtmRxRssi,
    BluetoothLegacyAdvertisingMemoryGraphBindError,
    BluetoothLegacyAdvertisingMemoryGraphBindFailure,
    BluetoothLegacyAdvertisingMemoryGraphCpuOwned, BluetoothLegacyAdvertisingMemoryGraphStorage,
    BluetoothRxMemoryListClass,
};
pub use passive_scanning::{
    BluetoothPassiveScanRuntimeBeginError, BluetoothPassiveScanRuntimeConfig,
    BluetoothPassiveScanRuntimeResources,
};
#[cfg(target_arch = "riscv32")]
pub use passive_scanning_active::{
    BluetoothPassiveScanActiveFault, BluetoothPassiveScanActiveFaultCause,
    BluetoothPassiveScanActiveSession, BluetoothPassiveScanActiveStep,
    BluetoothPassiveScanActiveWait, BluetoothPassiveScanEventCpuOwned,
};
#[cfg(target_arch = "riscv32")]
pub use passive_scanning_hci::{
    BluetoothPassiveScanHciActiveCommandIntake, BluetoothPassiveScanHciActiveCommandMismatch,
    BluetoothPassiveScanHciActiveCommandRoute, BluetoothPassiveScanHciActiveFault,
    BluetoothPassiveScanHciActivePendingFault, BluetoothPassiveScanHciActivePendingRadioStep,
    BluetoothPassiveScanHciActiveResponsePending, BluetoothPassiveScanHciActiveResponsePublication,
    BluetoothPassiveScanHciActiveSession, BluetoothPassiveScanHciActiveStep,
    BluetoothPassiveScanHciCommandIntake, BluetoothPassiveScanHciCommandMismatch,
    BluetoothPassiveScanHciCommandRoute, BluetoothPassiveScanHciCpuResponsePending,
    BluetoothPassiveScanHciCpuResponsePublication, BluetoothPassiveScanHciFirstRunner,
    BluetoothPassiveScanHciFirstRunnerFailure, BluetoothPassiveScanHciFirstRunnerStep,
    BluetoothPassiveScanHciFirstRunning, BluetoothPassiveScanHciRecurringFailure,
    BluetoothPassiveScanHciRecurringRunner, BluetoothPassiveScanHciRecurringRunnerStep,
    BluetoothPassiveScanHciReportStep, BluetoothPassiveScanHciReportsComplete,
    BluetoothPassiveScanHciReportsPending, BluetoothPassiveScanHciResponsePendingSession,
    BluetoothPassiveScanHciResponsePublication, BluetoothPassiveScanHciStopping,
    BluetoothPassiveScanHciStoppingFault, BluetoothPassiveScanHciStoppingStep,
};
#[cfg(target_arch = "riscv32")]
pub use passive_scanning_runner::{
    BluetoothPassiveScanFirstRunner, BluetoothPassiveScanFirstRunnerFailure,
    BluetoothPassiveScanFirstRunnerPublicationFailStop, BluetoothPassiveScanFirstRunnerRetry,
    BluetoothPassiveScanFirstRunnerRetryCause, BluetoothPassiveScanFirstRunnerStep,
    BluetoothPassiveScanFirstRunning,
};
#[cfg(target_arch = "riscv32")]
pub use passive_scanning_timing::BluetoothPassiveScanEventPhase;
#[cfg(target_arch = "riscv32")]
pub use peripheral_connection::BluetoothPeripheralConnectionPacketStartTiming;
#[cfg(any(target_arch = "riscv32", test))]
pub use peripheral_connection::BluetoothPeripheralConnectionRecurringTimingError;
pub use peripheral_connection::{
    BluetoothLe1MPacketStartTiming, BluetoothPeripheralConnectionFirstEventPrepared,
    BluetoothPeripheralConnectionRuntimeAllocation, BluetoothPeripheralConnectionRuntimeBeginError,
    BluetoothPeripheralConnectionRuntimeClaimError, BluetoothPeripheralConnectionRuntimeConfig,
    BluetoothPeripheralConnectionRuntimeResources,
};
#[cfg(target_arch = "riscv32")]
pub use phy::{
    BluetoothControllerPhyClientAcquire, BluetoothControllerPhyClientAcquireFailure,
    BluetoothControllerPhyInitializationFailure, BluetoothControllerPhyPendingTrack,
    BluetoothControllerPhyPendingTracking, BluetoothControllerPhyTrackingFailure,
    BluetoothPhyInitializationConfig,
};
pub use primary_interrupt::{
    BluetoothPrimaryInterruptStep, BluetoothPrimaryNoSchedulerWork,
    BluetoothPrimaryPublishedInterruptStep, BluetoothPrimarySchedulerEvent, step_primary_interrupt,
};
pub use resources::{BluetoothRadioHardware, BluetoothStopped, BluetoothStoppedReleaseFailure};
#[cfg(any(target_arch = "riscv32", test))]
pub use runtime_resources::BluetoothControllerPoweredTaskRuntime;
pub use runtime_resources::{
    BluetoothControllerInterruptRuntime, BluetoothControllerModemTimerRuntime,
    BluetoothControllerRuntimeResources, BluetoothControllerTaskRuntime,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use scheduler::{
    BluetoothControllerTimeAcquisitionError, BluetoothDtmControllerEventPreparationError,
    BluetoothDtmEmptySchedulerMergePrepared, BluetoothDtmInitialSchedulerItemPhase,
    BluetoothDtmRecurringSchedulerItemPhase, BluetoothDtmSchedulerHeadPublicationFailure,
    BluetoothDtmSchedulerHeadPublished, BluetoothDtmSchedulerRunning,
    BluetoothLegacyAdvertisingEmptySchedulerMergeFailure,
    BluetoothLegacyAdvertisingEmptySchedulerMergePrepared,
    BluetoothPassiveScanEmptySchedulerMergeFailure,
    BluetoothPassiveScanEmptySchedulerMergePrepared,
    BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    BluetoothPeripheralConnectionFirstEventPreparationError, BluetoothSchedulerEmptyListMergeError,
    BluetoothSchedulerHeadPublicationError, BluetoothSchedulerInitialized,
};
#[cfg(target_arch = "riscv32")]
pub use scheduler::{
    BluetoothDtmControllerRxPreparationFailure,
    BluetoothDtmControllerRxRecurringPreparationFailure,
    BluetoothDtmControllerTxPreparationFailure,
    BluetoothDtmControllerTxRecurringPreparationFailure,
    BluetoothDtmSchedulerHardwareHeadEmptyObserved,
    BluetoothDtmSchedulerHardwareHeadRetirementStep, BluetoothDtmSchedulerRecycleStep,
    BluetoothDtmSchedulerRxSuccessRecycleStep, BluetoothDtmSchedulerSoftwareListRemovalReady,
    BluetoothDtmSchedulerSoftwareListUnlinked,
    BluetoothLegacyAdvertisingSchedulerHeadPublicationFailure,
    BluetoothLegacyAdvertisingSchedulerHeadPublished,
    BluetoothPassiveScanSchedulerHeadPublicationFailure,
    BluetoothPassiveScanSchedulerHeadPublished,
    BluetoothPeripheralConnectionRecurringCandidateError,
    BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure,
    BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    BluetoothPeripheralConnectionRecurringEventCandidate,
    BluetoothPeripheralConnectionRecurringEventPreparationError,
    BluetoothPeripheralConnectionRecurringEventPreparationFailure,
    BluetoothPeripheralConnectionRecurringEventPrepared,
    BluetoothPeripheralConnectionRecurringPreSequence,
    BluetoothPeripheralConnectionSchedulerCompleted,
    BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
    BluetoothPeripheralConnectionSchedulerHeadPublished,
    BluetoothPeripheralConnectionSchedulerRecycled,
};
#[cfg(target_arch = "riscv32")]
pub use scheduler::{
    BluetoothDtmSchedulerCompletionObserved, BluetoothDtmSchedulerCompletionObservedDrainStep,
    BluetoothDtmSchedulerCompletionStep, BluetoothDtmSchedulerRunningDrainStep,
    BluetoothLegacyAdvertisingRecurringEventPreparationError,
    BluetoothLegacyAdvertisingRecurringEventPreparationFailure,
    BluetoothLegacyAdvertisingRecurringPreSequence, BluetoothSchedulerFinishedListDrainPending,
    BluetoothSchedulerFinishedListDrainState,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use scheduler::{
    BluetoothLegacyAdvertisingAdmissionObservation, BluetoothLegacyAdvertisingEventPrepared,
    BluetoothLegacyAdvertisingFirstEventPreparationError,
    BluetoothLegacyAdvertisingFirstEventPreparationFailure,
    BluetoothLegacyAdvertisingFirstPreSequence, BluetoothLegacyAdvertisingSequenceObservation,
    BluetoothPassiveScanAdmissionObservation, BluetoothPassiveScanEventPrepared,
    BluetoothPassiveScanFirstEventCandidate, BluetoothPassiveScanFirstEventPreparationError,
    BluetoothPassiveScanFirstEventPreparationFailure, BluetoothPassiveScanFirstPreSequence,
    BluetoothPassiveScanSequenceObservation,
};
pub use scheduler_config::BluetoothSchedulerSoftwareConfig;
pub use scheduler_finished_lists::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerFinishedListCaptureError,
    BluetoothSchedulerFinishedListWorker, BluetoothSchedulerFinishedListWorkerStep,
    BluetoothSchedulerHardwareListIndex,
};
pub use scheduler_insertion::{
    BluetoothSchedulerInsertionBeginOutcome, BluetoothSchedulerInsertionBusyDecision,
    BluetoothSchedulerInsertionEndPrelude, BluetoothSchedulerInsertionFinalAction,
    BluetoothSchedulerInsertionItemStatusGate, BluetoothSchedulerInsertionLockModifyGate,
    BluetoothSchedulerInsertionSleepDecision, BluetoothSchedulerInsertionSleepGate,
};
pub use scheduler_lock_modify::{
    BluetoothSchedulerLockModifyBeginError, BluetoothSchedulerLockModifyEvent,
    BluetoothSchedulerLockModifyEventCell, BluetoothSchedulerLockModifyEventPublication,
    BluetoothSchedulerLockModifyInterruptObservation,
    BluetoothSchedulerLockModifyPublicationResult, BluetoothSchedulerLockModifyWorker,
    BluetoothSchedulerLockModifyWorkerStep,
};
pub(crate) use scheduler_time::BluetoothSchedulerInstant;
#[cfg(any(target_arch = "riscv32", test))]
pub use scheduler_timeline::{
    BluetoothSchedulerRawWindow, BluetoothSchedulerReservationError,
    BluetoothSchedulerReservationReleaseError, BluetoothSchedulerReservationReleaseFailure,
    BluetoothSchedulerSequenceAuthorizationError, BluetoothSchedulerSequenceReady,
    BluetoothSchedulerTimingPolicy, BluetoothSchedulerWindowReservation,
};
