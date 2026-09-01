//! ESP32-S31 Bluetooth controller hardware boundary.
//!
//! This crate is intentionally below operational HCI and the Bluetooth Link
//! Layer. It binds portable bootstrap resources only to preserve one powered
//! ownership epoch. The implemented slices establish lossless physical
//! ownership, the route-owned
//! clock/reset prerequisite, the complete 50-operation BTDM HAL-init body and
//! the following complete scheduler-init component. The latter retains its
//! source-owned software policy and replaces vendor broker/event containers
//! with one pristine static Rust runtime epoch. The following HCI software
//! initialization binds that powered epoch to one bounded portable transport
//! and bootstrap dispatcher. The following modem low-power, common-PHY,
//! finite BT-baseband, BLE-PHY, controller-output and runtime-timer transitions
//! are now one connected affine enable chain; the terminal state retains the
//! complete static BLE environment graph and advances both disjoint interrupt
//! owners into their final movable pre-route states. It still does not claim
//! operational Link-Layer work. The
//! three Controller interrupt sources, level/residency policies, baseline masks,
//! snapshot modes, positional dynamic scheduler classifier, coalesced wake
//! state, affine ISR scheduler-register staging and one live finite
//! scheduler-lock/modify PAC/HAL publication with a durable event worker are
//! also represented. The terminal pre-route Controller can additionally
//! consume one identity-checked DTM graph through descriptor/head visibility,
//! stable-owner dynamic interrupt preparation, the synchronous BTMAC event
//! and RUN as one affine chain. A sampled sixteen-list finished mask can be
//! drained one bit per bounded event step. The terminal Controller can also
//! perform one fresh transfer and immediately join list zero to its exact
//! running DTM epoch; a non-sentinel status advances to a completion-observed
//! completion observation. A second affine operation performs the mandatory
//! fresh post-picker head read and advances only after list zero is empty.
//! The exact item can then leave the source-owned software list once. A sealed
//! lower consumer accepts only an opaque pairing of that graph with an already
//! published later primary event; no public constructor exists until the
//! session runtime can prove the post-unlink cutoff. Busy or command-pending
//! events retain ownership without polling, while ready permits TX and
//! RX-non-success recycle to release the exact timeline reservation before
//! returning the CPU graph. A specialized RX-success path
//! validates the bounded returned-header pair, accounts its graph-bound typed
//! result, performs the corresponding append/re-arm rotation, releases the
//! timeline and source list, and returns one non-copyable receiver session.
//! Controller-SRAM allocation geometry and result parsing live in the separate
//! `open-esp-radio-esp32s31-bluetooth-memory` layer below this LLL boundary;
//! one bounded DTM RX transaction now owns the completed-header visibility and
//! exact `observe -> account -> append/re-arm` sequence.
//! The initialized scheduler now joins its software task endpoint to the exact
//! task-side HAL owner, so one lock/modify event step can reach the restricted
//! PAC without exporting register authority. The remaining components are not
//! connected across the missing live primary-ISR/executor composition,
//! feature-specific NRT classification and live-route
//! prerequisites. Recurring event preparation retains exact active role
//! ownership, and bounded first-event, active, recurring and terminal-neutral
//! quiescence runners preserve it. A target-only sole Embassy actor composes
//! idle command intake, first-event cleanup, active progression, Test End and
//! Reset, but production final-split/spawn and live route/waker composition
//! remain absent. Stable two-owner ISR publication is connected, but no
//! current finite state claims that the complete controller lifecycle, HCI
//! transport, task or live interrupt epoch has completed.
//! The public lifecycle begins with one [`BluetoothStopped`] aggregate; the
//! platform lease and neutral radio root cannot be split across clock enable,
//! rollback, or clean reversible shutdown.

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
mod legacy_advertising_runner;
#[cfg(any(target_arch = "riscv32", test))]
mod legacy_advertising_timing;
mod modem_lp_timer_queue;
mod nrt_interrupt;
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
#[cfg(target_arch = "riscv32")]
pub use controller_hal::BluetoothControllerHalInitialized;
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
    BluetoothInterruptOwnerStorage, BluetoothLegacyAdvertisingControllerPreparationError,
    BluetoothLegacyAdvertisingControllerPreparationOutcome,
    BluetoothLegacyAdvertisingControllerPreparationPending,
    BluetoothLegacyAdvertisingControllerPreparationStep,
    BluetoothLegacyAdvertisingControllerPreparationTerminal,
    BluetoothLegacyAdvertisingPostUnlinkArmStep, BluetoothLegacyAdvertisingSchedulerStartFailure,
    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep,
    BluetoothModemLpTimerInterruptDispatchStorage, BluetoothModemLpTimerSoftwareOwnerStorage,
    BluetoothSchedulerRunInterruptStorage, BluetoothSharedInterruptDispatchStorage,
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
pub use dtm_post_unlink::{
    BluetoothDtmPostUnlinkAwaiting, BluetoothDtmPostUnlinkCancelStep,
    BluetoothLegacyAdvertisingPostUnlinkAwaiting, BluetoothLegacyAdvertisingPostUnlinkCancelStep,
};
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
#[cfg(any(target_arch = "riscv32", test))]
pub use hci::{
    BluetoothControllerHciInitializationError, BluetoothControllerHciInitializationFailure,
    BluetoothControllerHciInitialized, BluetoothControllerLowPowerHardwareInitializationFailure,
    BluetoothControllerLowPowerHardwareInitialized, BluetoothControllerRuntimeEndpoints,
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
pub use legacy_advertising::{
    BluetoothLegacyAdvertisingCancelled, BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    BluetoothLegacyAdvertisingLinkStateReset, BluetoothLegacyAdvertisingLinkStateResetError,
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
    BluetoothLegacyAdvertisingFirstEventTimingFailure,
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
pub use legacy_advertising_timing::BluetoothLegacyAdvertisingTimingObservation;
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
    BluetoothLegacyAdvertisingEmptySchedulerMergePrepared, BluetoothSchedulerEmptyListMergeError,
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
    BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved,
    BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep,
    BluetoothLegacyAdvertisingSchedulerHeadPublicationFailure,
    BluetoothLegacyAdvertisingSchedulerHeadPublished,
    BluetoothLegacyAdvertisingSchedulerRecycleStep, BluetoothLegacyAdvertisingSchedulerRecycled,
    BluetoothLegacyAdvertisingSchedulerRunning,
    BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin,
    BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady,
    BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck,
    BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinkStep,
    BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked,
};
#[cfg(target_arch = "riscv32")]
pub use scheduler::{
    BluetoothDtmSchedulerCompletionObserved, BluetoothDtmSchedulerCompletionObservedDrainStep,
    BluetoothDtmSchedulerCompletionStep, BluetoothDtmSchedulerRunningDrainStep,
    BluetoothLegacyAdvertisingRecurringEventPreparationError,
    BluetoothLegacyAdvertisingRecurringEventPreparationFailure,
    BluetoothLegacyAdvertisingRecurringPreSequence,
    BluetoothLegacyAdvertisingSchedulerCompletionObserved,
    BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep,
    BluetoothLegacyAdvertisingSchedulerCompletionStep,
    BluetoothLegacyAdvertisingSchedulerRunningDrainStep,
    BluetoothSchedulerFinishedListDrainPending, BluetoothSchedulerFinishedListDrainState,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use scheduler::{
    BluetoothLegacyAdvertisingAdmissionObservation, BluetoothLegacyAdvertisingEventPrepared,
    BluetoothLegacyAdvertisingFirstEventPreparationError,
    BluetoothLegacyAdvertisingFirstEventPreparationFailure,
    BluetoothLegacyAdvertisingFirstPreSequence, BluetoothLegacyAdvertisingSequenceObservation,
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
