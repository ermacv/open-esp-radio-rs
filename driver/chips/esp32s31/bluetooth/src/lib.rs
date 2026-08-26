//! ESP32-S31 Bluetooth controller hardware boundary.
//!
//! This crate is intentionally below HCI and the Bluetooth Link Layer. The
//! implemented slices establish lossless physical ownership, the route-owned
//! clock/reset prerequisite and the first finite scheduler-table transaction
//! of controller init. Common PHY, BT-baseband and the complete 50-operation
//! BTDM HAL-init body are implemented as later enable-stage components. The
//! two Controller interrupt sources, level/residency policies, baseline masks,
//! snapshot modes, positional dynamic scheduler classifier, coalesced wake
//! state, affine ISR scheduler-register staging and one live finite
//! scheduler-lock/modify PAC/HAL publication with a durable event worker are
//! also represented. A sampled
//! sixteen-list finished mask can be drained one bit per bounded event step.
//! Controller-SRAM allocation geometry and result parsing live in the separate
//! `open-esp-radio-esp32s31-bluetooth-memory` layer below this LLL boundary;
//! one bounded DTM RX transition accounts a result word without claiming its
//! still-missing completed-header ownership or visibility fence.
//! These components are deliberately not connected across the missing
//! selector-6 invariant, affine item/completion-list owner, primary-ISR worker
//! composition, LP, BLE, feature-specific NRT classification and live-route
//! prerequisites. No current finite state claims that the complete controller
//! lifecycle, HCI transport, task or live interrupt epoch has completed.
//! The public lifecycle begins with one [`BluetoothStopped`] aggregate; the
//! platform lease and neutral radio root cannot be split across clock enable,
//! rollback, or clean reversible shutdown.

#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
mod baseband;
mod clock;
#[cfg(any(target_arch = "riscv32", test))]
mod common_phy_state;
mod controller_time;
mod dtm_event_prepare;
mod dtm_event_timing;
mod dtm_link_state;
mod dtm_parameters;
mod dtm_payload;
mod dtm_rx_completion;
mod dtm_scheduler_item;
mod dtm_timing;
mod dtm_tx_packet;
mod interrupt;
mod interrupt_classifier;
mod interrupt_wake;
mod modem_lp_timer_queue;
mod nrt_interrupt;
#[cfg(target_arch = "riscv32")]
mod phy;
mod primary_interrupt;
mod resources;
mod runtime_resources;
#[cfg(any(target_arch = "riscv32", test))]
mod scheduler;
mod scheduler_finished_lists;
mod scheduler_lock_modify;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

pub use clock::{
    BluetoothClockCheckpoint, BluetoothClockEnableFailure, BluetoothClockError,
    BluetoothClockState, BluetoothClockedResources,
};
pub use controller_time::{BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample};
pub use dtm_event_prepare::{
    BluetoothDtmReviewedEventWordsPlan, BluetoothDtmReviewedEventWordsPlanError,
    BluetoothDtmReviewedEventWordsPrepared,
};
pub use dtm_event_timing::{
    BluetoothDtmSchedulerInstant, BluetoothDtmSchedulerMargin, BluetoothDtmTxEventAdvance,
    BluetoothDtmTxEventWindow,
};
pub use dtm_link_state::{
    BluetoothDtmLinkStateReset, BluetoothDtmLinkStateResetError,
    BluetoothDtmLinkStateReviewedWords, BluetoothDtmRole,
};
pub use dtm_parameters::{
    BluetoothDtmChannel, BluetoothDtmChannelError, BluetoothDtmPhy, BluetoothDtmPhyError,
    BluetoothDtmPhyRoleError,
};
pub use dtm_payload::{
    BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern, BluetoothDtmPayloadPatternError,
    BluetoothDtmPayloadPreparationError, BluetoothDtmPreparedPayload,
};
pub use dtm_rx_completion::{
    BLUETOOTH_DTM_RX_INITIAL_RETURNED_BYTE, BluetoothDtmRxAccountingOutcome,
    BluetoothDtmRxCompletionState,
};
pub use dtm_scheduler_item::{
    BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerItemEventError,
    BluetoothDtmSchedulerItemReviewedWords,
};
pub use dtm_timing::{BluetoothDtmTxSchedulerTiming, BluetoothDtmTxTimingMicros};
pub use dtm_tx_packet::{
    BLUETOOTH_DTM_TX_MAX_PAYLOAD_BYTES, BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES,
    BLUETOOTH_DTM_TX_PACKET_STORAGE_BYTES, BluetoothDtmPreparedTxPacket,
    BluetoothDtmTxBufferHeaderImage, BluetoothDtmTxPacketAddress, BluetoothDtmTxPacketAddressError,
    BluetoothDtmTxPacketPrepare, BluetoothDtmTxPacketStorage,
};
pub use interrupt::{
    BluetoothCpuInterruptRoutePolicy, BluetoothCpuInterruptSource,
    BluetoothInterruptHandlerResidency,
};
pub use interrupt_classifier::{
    BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK, BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK,
    BluetoothPrimaryControllerFault, BluetoothPrimaryInterruptClassification,
    BluetoothPrimarySchedulerTrigger, BluetoothSchedulerReferenceAction,
    BluetoothSchedulerReferenceGate, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerWorkClassifier, BluetoothSchedulerWorkObservation,
    BluetoothSchedulerWorkerWake, BluetoothSchedulerWorkerWakeClass,
};
pub use interrupt_wake::{
    BluetoothSchedulerWakeBatch, BluetoothSchedulerWakeCell, BluetoothSchedulerWakePublication,
};
pub use modem_lp_timer_queue::{
    BluetoothModemLpTimerEventCell, BluetoothModemLpTimerEventPublication,
    BluetoothModemLpTimerExpiration, BluetoothModemLpTimerExpirationPending,
    BluetoothModemLpTimerInterruptRuntimeStep, BluetoothModemLpTimerQueue,
    BluetoothModemLpTimerScheduleError, BluetoothModemLpTimerSoftwareStep,
    BluetoothModemLpTimerSoftwareWork, BluetoothModemLpTimerToken, step_modem_lp_timer_interrupt,
};
pub use nrt_interrupt::{BluetoothNrtDefaultInterruptEpoch, step_nrt_default_interrupt};
pub use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmBoundSramLinkAddress, BluetoothDtmBoundSramLinkAddressError,
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphPrepareError,
    BluetoothDtmMemoryGraphPrepareFailure, BluetoothDtmPositionalEventWords,
    BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError,
    BluetoothRxMemoryListClass,
};
pub use primary_interrupt::{
    BluetoothPrimaryInterruptStep, BluetoothPrimaryNoSchedulerWork,
    BluetoothPrimaryReferenceRecoveryRequired, BluetoothPrimarySchedulerEvent,
    step_primary_interrupt,
};
pub use resources::BluetoothStopped;
pub use runtime_resources::{
    BluetoothControllerInterruptRuntime, BluetoothControllerRuntimeResources,
    BluetoothControllerTaskRuntime,
};
#[cfg(target_arch = "riscv32")]
pub use scheduler::{
    BluetoothSchedulerRuntimeResourcesBound, BluetoothSchedulerTableLowBitsCleared,
};
pub use scheduler_finished_lists::{
    BluetoothSchedulerFinishedListCaptureError, BluetoothSchedulerFinishedListIndex,
    BluetoothSchedulerFinishedListWorker, BluetoothSchedulerFinishedListWorkerStep,
};
pub use scheduler_lock_modify::{
    BluetoothSchedulerLockModifyAwaitingPublication, BluetoothSchedulerLockModifyBeginError,
    BluetoothSchedulerLockModifyEvent, BluetoothSchedulerLockModifyEventCell,
    BluetoothSchedulerLockModifyEventPublication, BluetoothSchedulerLockModifyInFlight,
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyProgress,
    BluetoothSchedulerLockModifyPublication, BluetoothSchedulerLockModifyPublicationResult,
    BluetoothSchedulerLockModifyWorker, BluetoothSchedulerLockModifyWorkerStep,
};
