//! ESP32-S31 Bluetooth controller-memory boundary.
//!
//! The restricted PAC below this crate owns positional MMIO transactions and
//! compressed-address encoding. This crate owns controller-SRAM layouts and
//! role-to-list routing that are consumed above the register boundary. It is
//! intentionally sparse: static graph location, allocation-time links, the
//! fixed DTM allocator prefix and the first empty-list item-link transform are
//! bound. A matching affine PAC head-publication token can then consume every
//! rollback image into a controller-visible graph. The exact RUN proof then
//! advances it to a running graph. An affine fenced finished-list
//! observation can then drive one volatile semantic item-status read without
//! granting CPU ownership. Matching empty-head and post-unlink removal proofs
//! authorize the reviewed cleanup. RX-success additionally validates the
//! exact two-header private chain and performs its bounded volatile swap/re-arm
//! rotation before CPU ownership returns.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

mod ble_phy_engine;
mod connectable_advertising_memory;
mod direction_finding_workspace;
mod dtm_event_image;
mod dtm_rx_result;
mod dtm_storage;
mod le_phy_packet;
mod le_rx_packet;
mod le_tx_packet;
mod le_tx_power;
mod legacy_advertising_event_image;
mod legacy_advertising_storage;
mod non_scanning_rx_memory;
mod passive_scanning_event_image;
mod passive_scanning_memory;
mod peripheral_connection_memory;
mod rx_memory_list;
mod scheduler_context;
mod sram_link;

#[cfg(not(target_arch = "riscv32"))]
pub use ble_phy_engine::BlePhyEngineModelAddress;

pub use ble_phy_engine::{
    BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES, BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES,
    BlePhyEngineBindError, BlePhyEngineBindFailure, BlePhyEngineBinding, BlePhyEngineCpuOwned,
    BlePhyEngineStorage, BlePhyLe1MPacketStartCalibration,
};

#[cfg(not(target_arch = "riscv32"))]
pub use connectable_advertising_memory::LegacyConnectableAdvertisingMemoryGraphModelAddress;

pub use connectable_advertising_memory::{
    LegacyConnectableAdvIndPacketInput, LegacyConnectableAdvertisingMemoryGraphBindError,
    LegacyConnectableAdvertisingMemoryGraphBindFailure,
    LegacyConnectableAdvertisingMemoryGraphCompletionObservation,
    LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    LegacyConnectableAdvertisingMemoryGraphCpuOwned,
    LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared,
    LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure,
    LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
    LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch,
    LegacyConnectableAdvertisingMemoryGraphHeadPublished,
    LegacyConnectableAdvertisingMemoryGraphIdentity,
    LegacyConnectableAdvertisingMemoryGraphPrepareError,
    LegacyConnectableAdvertisingMemoryGraphPrepareFailure,
    LegacyConnectableAdvertisingMemoryGraphPrepared,
    LegacyConnectableAdvertisingMemoryGraphPublicationError,
    LegacyConnectableAdvertisingMemoryGraphPublicationMismatch,
    LegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
    LegacyConnectableAdvertisingMemoryGraphRecycleError,
    LegacyConnectableAdvertisingMemoryGraphRecycleFailure,
    LegacyConnectableAdvertisingMemoryGraphRecyclePrepared,
    LegacyConnectableAdvertisingMemoryGraphRecycled,
    LegacyConnectableAdvertisingMemoryGraphRunMismatch,
    LegacyConnectableAdvertisingMemoryGraphRunning,
    LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked,
    LegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared,
    LegacyConnectableAdvertisingMemoryGraphRxExtracted,
    LegacyConnectableAdvertisingMemoryGraphRxExtractionFailure,
    LegacyConnectableAdvertisingMemoryGraphRxPublished,
    LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    LegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
    LegacyConnectableAdvertisingMemoryGraphStorage, LegacyConnectableAdvertisingMemoryInput,
    LegacyConnectableAdvertisingOwnAddress, LegacyConnectableAdvertisingPduFitError,
    LegacyConnectableAdvertisingPostAnchorDuration,
    LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    LegacyConnectableScanResponsePacketInput,
};

#[cfg(not(target_arch = "riscv32"))]
pub use direction_finding_workspace::DirectionFindingWorkspaceModelAddress;
#[cfg(not(target_arch = "riscv32"))]
pub use dtm_storage::DtmMemoryGraphModelAddress;

pub use direction_finding_workspace::{
    BLUETOOTH_DIRECTION_FINDING_WORKSPACE_BYTES, DirectionFindingWorkspaceBindError,
    DirectionFindingWorkspaceBindFailure, DirectionFindingWorkspaceBinding,
    DirectionFindingWorkspaceCpuOwned, DirectionFindingWorkspaceLink,
    DirectionFindingWorkspaceStorage,
};

pub use dtm_event_image::{
    DtmLinkStateReviewedWords, DtmPositionalEventWords, DtmReceiverEventPhase, DtmRole,
    DtmRxHeaderTailProjection, DtmSchedulerItemEventType, DtmSchedulerItemReviewedWords,
    DtmSchedulerReceiverPhy, DtmSchedulerTransmitterPhy, DtmTxHeaderHeadProjection,
};

pub use dtm_rx_result::{DtmRxResultProjection, DtmRxResultProjectionError, DtmRxRssi};

#[cfg(not(target_arch = "riscv32"))]
pub use legacy_advertising_storage::LegacyAdvertisingMemoryGraphModelAddress;

pub use dtm_storage::{
    BLUETOOTH_DTM_LINK_STATE_BYTES, BLUETOOTH_DTM_MAX_PACKET_CAPACITY,
    BLUETOOTH_DTM_RX_PACKET_BYTES, BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES,
    BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, BLUETOOTH_DTM_TX_PACKET_BYTES, DtmMemoryGraphBindError,
    DtmMemoryGraphBindFailure, DtmMemoryGraphCompletionObservation,
    DtmMemoryGraphCompletionObserved, DtmMemoryGraphCpuOwned, DtmMemoryGraphEmptyListLinkPrepared,
    DtmMemoryGraphHeadPublished, DtmMemoryGraphIdentity, DtmMemoryGraphPositionalEventPrepared,
    DtmMemoryGraphPrepareError, DtmMemoryGraphPrepareFailure, DtmMemoryGraphReclaimed,
    DtmMemoryGraphRecycleCleaned, DtmMemoryGraphRecycleError, DtmMemoryGraphRecycleFailure,
    DtmMemoryGraphRecyclePrepared, DtmMemoryGraphRecycled, DtmMemoryGraphRunning,
    DtmMemoryGraphRxSuccessObserved, DtmMemoryGraphRxSuccessRecycleError,
    DtmMemoryGraphRxSuccessRecycleFailure, DtmMemoryGraphRxSuccessRecyclePrepared,
    DtmMemoryGraphSchedulerBookkeepingPrepared, DtmMemoryGraphStorage,
    DtmMemoryGraphTxPacketPrepareFailure, DtmMemoryGraphTxPacketPrepared, DtmPositionalEventSeed,
    DtmSchedulerAllocationConfig, DtmSchedulerItemCompletionStatus, DtmTxPacketPrepareError,
};

pub use le_rx_packet::{LePacketCapturedTime, LeReceivedBatch, LeReceivedPdu, LeRxError};

pub use le_tx_packet::{
    BLUETOOTH_LE_BUFFER_HEADER_BYTES, BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxPacketPrepareError,
    LeTxPacketPreparedLength, LeTxPacketStorage,
};

pub use legacy_advertising_event_image::{
    LegacyAdvertisingPduError, LegacyAdvertisingPrimaryChannel, LegacyAdvertisingPrimaryChannelPlan,
};

pub use legacy_advertising_storage::{
    BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES, BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES,
    BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY,
    BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES, LegacyAdvertisingEventCompletionStatuses,
    LegacyAdvertisingMemoryGraphBindError, LegacyAdvertisingMemoryGraphBindFailure,
    LegacyAdvertisingMemoryGraphBinding, LegacyAdvertisingMemoryGraphCompletionObservation,
    LegacyAdvertisingMemoryGraphCompletionObserved, LegacyAdvertisingMemoryGraphCpuOwned,
    LegacyAdvertisingMemoryGraphEmptyListLinkPrepared,
    LegacyAdvertisingMemoryGraphEventPrepareError, LegacyAdvertisingMemoryGraphEventPrepareFailure,
    LegacyAdvertisingMemoryGraphEventPrepared, LegacyAdvertisingMemoryGraphHeadPublished,
    LegacyAdvertisingMemoryGraphIdentity, LegacyAdvertisingMemoryGraphLinkStateReset,
    LegacyAdvertisingMemoryGraphLinkStateResetFailure,
    LegacyAdvertisingMemoryGraphPacketPrepareFailure, LegacyAdvertisingMemoryGraphPacketPrepared,
    LegacyAdvertisingMemoryGraphRecycleError, LegacyAdvertisingMemoryGraphRecycleFailure,
    LegacyAdvertisingMemoryGraphRecyclePrepared, LegacyAdvertisingMemoryGraphRecycled,
    LegacyAdvertisingMemoryGraphRunning, LegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    LegacyAdvertisingMemoryGraphStorage, LegacyAdvertisingSchedulerItemCompletionStatus,
};
#[cfg(not(target_arch = "riscv32"))]
pub use non_scanning_rx_memory::NonScanningRxMemoryModelAddress;
#[cfg(not(target_arch = "riscv32"))]
pub use passive_scanning_memory::PassiveScanMemoryGraphModelAddress;

pub use non_scanning_rx_memory::{
    BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, NonScanningRxMemoryBindError,
    NonScanningRxMemoryBindFailure, NonScanningRxMemoryCpuOwned, NonScanningRxMemoryIdentity,
    NonScanningRxMemoryStorage,
};

pub use passive_scanning_event_image::{
    PassiveScanDefaultTxPowerDbm, PassiveScanPrimaryChannel, PassiveScanResetConfig,
    PassiveScanSchedulerWindow, PassiveScanStartSelection,
};

pub use passive_scanning_memory::{
    BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT, BLUETOOTH_PASSIVE_SCAN_RX_PACKET_BYTES,
    BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES, BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY,
    BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT, PassiveScanMemoryGraphBindError,
    PassiveScanMemoryGraphBindFailure, PassiveScanMemoryGraphCommandPublished,
    PassiveScanMemoryGraphCompletionObservation, PassiveScanMemoryGraphCompletionObserved,
    PassiveScanMemoryGraphCpuOwned, PassiveScanMemoryGraphEventPrepared,
    PassiveScanMemoryGraphPublicationError, PassiveScanMemoryGraphPublicationMismatch,
    PassiveScanMemoryGraphPublicationPrepared, PassiveScanMemoryGraphPublished,
    PassiveScanMemoryGraphRecycleError, PassiveScanMemoryGraphRecycleFailure,
    PassiveScanMemoryGraphRecyclePrepared, PassiveScanMemoryGraphRecycled,
    PassiveScanMemoryGraphRunning, PassiveScanMemoryGraphRxExtracted,
    PassiveScanMemoryGraphRxExtractionFailure, PassiveScanMemoryGraphSchedulerAdmissionPrepared,
    PassiveScanMemoryGraphStorage, PassiveScanSchedulerAllocationConfig,
    PassiveScanSchedulerItemCompletionStatus,
};
#[cfg(not(target_arch = "riscv32"))]
pub use peripheral_connection_memory::PeripheralConnectionMemoryGraphModelAddress;

pub use peripheral_connection_memory::{
    BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT,
    BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES,
    PeripheralConnectionCapturedAnchorAvailability, PeripheralConnectionCapturedAnchorTime,
    PeripheralConnectionDataChannel, PeripheralConnectionDefaultTxPowerDbm,
    PeripheralConnectionEventSpan, PeripheralConnectionIdentity, PeripheralConnectionIntervalTicks,
    PeripheralConnectionMemoryGraphActiveCpuOwned, PeripheralConnectionMemoryGraphBindError,
    PeripheralConnectionMemoryGraphBindFailure,
    PeripheralConnectionMemoryGraphCompletionObservation,
    PeripheralConnectionMemoryGraphCompletionObserved, PeripheralConnectionMemoryGraphCpuOwned,
    PeripheralConnectionMemoryGraphDirectionFindingPrepared,
    PeripheralConnectionMemoryGraphEventFieldsPrepared, PeripheralConnectionMemoryGraphIdentity,
    PeripheralConnectionMemoryGraphIdentityPrepared,
    PeripheralConnectionMemoryGraphPublicationError,
    PeripheralConnectionMemoryGraphPublicationMismatch,
    PeripheralConnectionMemoryGraphPublicationPrepared,
    PeripheralConnectionMemoryGraphReceivePrepared,
    PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
    PeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared,
    PeripheralConnectionMemoryGraphRecycleError, PeripheralConnectionMemoryGraphRecycleFailure,
    PeripheralConnectionMemoryGraphRecyclePrepared, PeripheralConnectionMemoryGraphRecycled,
    PeripheralConnectionMemoryGraphRunning, PeripheralConnectionMemoryGraphRxExtracted,
    PeripheralConnectionMemoryGraphRxExtractionFailure, PeripheralConnectionMemoryGraphRxPublished,
    PeripheralConnectionMemoryGraphSchedulerAdmissionPrepared,
    PeripheralConnectionMemoryGraphStorage, PeripheralConnectionReceiveWait,
    PeripheralConnectionRecurringReceiveWait, PeripheralConnectionSchedulerItemCompletionStatus,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
};

pub use rx_memory_list::RxMemoryListClass;

pub use scheduler_context::{BLUETOOTH_SCHEDULER_CONTEXT_BYTES, SchedulerContextStorage};

pub use sram_link::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
    ControllerSramLinkAddress, ControllerSramLinkAddressError,
};
