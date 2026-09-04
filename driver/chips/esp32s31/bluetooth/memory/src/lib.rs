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
pub use ble_phy_engine::BluetoothBlePhyEngineModelAddress;
pub use ble_phy_engine::{
    BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES, BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES,
    BluetoothBlePhyEngineBindError, BluetoothBlePhyEngineBindFailure, BluetoothBlePhyEngineBinding,
    BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineStorage,
    BluetoothBlePhyLe1MPacketStartCalibration,
};

#[cfg(not(target_arch = "riscv32"))]
pub use connectable_advertising_memory::BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress;
pub use connectable_advertising_memory::{
    BluetoothLegacyConnectableAdvIndPacketInput,
    BluetoothLegacyConnectableAdvertisingMemoryGraphBindError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure,
    BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
    BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
    BluetoothLegacyConnectableAdvertisingMemoryInput,
    BluetoothLegacyConnectableAdvertisingOwnAddress,
    BluetoothLegacyConnectableAdvertisingPduFitError,
    BluetoothLegacyConnectableAdvertisingSchedulerSpan,
    BluetoothLegacyConnectableScanResponsePacketInput,
};

#[cfg(not(target_arch = "riscv32"))]
pub use direction_finding_workspace::BluetoothDirectionFindingWorkspaceModelAddress;
pub use direction_finding_workspace::{
    BLUETOOTH_DIRECTION_FINDING_WORKSPACE_BYTES, BluetoothDirectionFindingWorkspaceBindError,
    BluetoothDirectionFindingWorkspaceBindFailure, BluetoothDirectionFindingWorkspaceBinding,
    BluetoothDirectionFindingWorkspaceCpuOwned, BluetoothDirectionFindingWorkspaceLink,
    BluetoothDirectionFindingWorkspaceStorage,
};

pub use dtm_event_image::{
    BluetoothDtmLinkStateReviewedWords, BluetoothDtmPositionalEventWords,
    BluetoothDtmReceiverEventPhase, BluetoothDtmRole, BluetoothDtmRxHeaderTailProjection,
    BluetoothDtmSchedulerItemEventType, BluetoothDtmSchedulerItemReviewedWords,
    BluetoothDtmSchedulerReceiverPhy, BluetoothDtmSchedulerTransmitterPhy,
    BluetoothDtmTxHeaderHeadProjection,
};
pub use dtm_rx_result::{
    BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError, BluetoothDtmRxRssi,
};
#[cfg(not(target_arch = "riscv32"))]
pub use dtm_storage::BluetoothDtmMemoryGraphModelAddress;
pub use dtm_storage::{
    BLUETOOTH_DTM_LINK_STATE_BYTES, BLUETOOTH_DTM_MAX_PACKET_CAPACITY,
    BLUETOOTH_DTM_RX_PACKET_BYTES, BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES,
    BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, BLUETOOTH_DTM_TX_PACKET_BYTES,
    BluetoothDtmMemoryGraphBindError, BluetoothDtmMemoryGraphBindFailure,
    BluetoothDtmMemoryGraphCompletionObservation, BluetoothDtmMemoryGraphCompletionObserved,
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphEmptyListLinkPrepared,
    BluetoothDtmMemoryGraphHeadPublished, BluetoothDtmMemoryGraphIdentity,
    BluetoothDtmMemoryGraphPositionalEventPrepared, BluetoothDtmMemoryGraphPrepareError,
    BluetoothDtmMemoryGraphPrepareFailure, BluetoothDtmMemoryGraphReclaimed,
    BluetoothDtmMemoryGraphRecycleCleaned, BluetoothDtmMemoryGraphRecycleError,
    BluetoothDtmMemoryGraphRecycleFailure, BluetoothDtmMemoryGraphRecyclePrepared,
    BluetoothDtmMemoryGraphRecycled, BluetoothDtmMemoryGraphRunning,
    BluetoothDtmMemoryGraphRxSuccessObserved, BluetoothDtmMemoryGraphRxSuccessRecycleError,
    BluetoothDtmMemoryGraphRxSuccessRecycleFailure,
    BluetoothDtmMemoryGraphRxSuccessRecyclePrepared,
    BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared, BluetoothDtmMemoryGraphStorage,
    BluetoothDtmMemoryGraphTxPacketPrepareFailure, BluetoothDtmMemoryGraphTxPacketPrepared,
    BluetoothDtmPositionalEventSeed, BluetoothDtmSchedulerAllocationConfig,
    BluetoothDtmSchedulerItemCompletionStatus, BluetoothDtmTxPacketPrepareError,
};
pub use le_rx_packet::{
    BluetoothLePacketCapturedTime, BluetoothLeReceivedBatch, BluetoothLeReceivedPdu,
    BluetoothLeRxError,
};
pub use le_tx_packet::{
    BLUETOOTH_LE_BUFFER_HEADER_BYTES, BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES,
    BluetoothLeTxPacketPrepareError, BluetoothLeTxPacketPreparedLength, BluetoothLeTxPacketStorage,
};
pub use legacy_advertising_event_image::{
    BluetoothLegacyAdvertisingPduError, BluetoothLegacyAdvertisingPrimaryChannel,
    BluetoothLegacyAdvertisingPrimaryChannelPlan,
};
#[cfg(not(target_arch = "riscv32"))]
pub use legacy_advertising_storage::BluetoothLegacyAdvertisingMemoryGraphModelAddress;
pub use legacy_advertising_storage::{
    BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES, BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES,
    BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY,
    BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES,
    BluetoothLegacyAdvertisingEventCompletionStatuses,
    BluetoothLegacyAdvertisingMemoryGraphBindError,
    BluetoothLegacyAdvertisingMemoryGraphBindFailure, BluetoothLegacyAdvertisingMemoryGraphBinding,
    BluetoothLegacyAdvertisingMemoryGraphCompletionObservation,
    BluetoothLegacyAdvertisingMemoryGraphCompletionObserved,
    BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    BluetoothLegacyAdvertisingMemoryGraphEmptyListLinkPrepared,
    BluetoothLegacyAdvertisingMemoryGraphEventPrepareError,
    BluetoothLegacyAdvertisingMemoryGraphEventPrepareFailure,
    BluetoothLegacyAdvertisingMemoryGraphEventPrepared,
    BluetoothLegacyAdvertisingMemoryGraphHeadPublished,
    BluetoothLegacyAdvertisingMemoryGraphIdentity,
    BluetoothLegacyAdvertisingMemoryGraphLinkStateReset,
    BluetoothLegacyAdvertisingMemoryGraphLinkStateResetFailure,
    BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure,
    BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
    BluetoothLegacyAdvertisingMemoryGraphRecycleError,
    BluetoothLegacyAdvertisingMemoryGraphRecycleFailure,
    BluetoothLegacyAdvertisingMemoryGraphRecyclePrepared,
    BluetoothLegacyAdvertisingMemoryGraphRecycled, BluetoothLegacyAdvertisingMemoryGraphRunning,
    BluetoothLegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    BluetoothLegacyAdvertisingMemoryGraphStorage,
    BluetoothLegacyAdvertisingSchedulerItemCompletionStatus,
};
#[cfg(not(target_arch = "riscv32"))]
pub use non_scanning_rx_memory::BluetoothNonScanningRxMemoryModelAddress;
pub use non_scanning_rx_memory::{
    BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, BluetoothNonScanningRxMemoryBindError,
    BluetoothNonScanningRxMemoryBindFailure, BluetoothNonScanningRxMemoryCpuOwned,
    BluetoothNonScanningRxMemoryIdentity, BluetoothNonScanningRxMemoryStorage,
};
pub use passive_scanning_event_image::{
    BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanPrimaryChannel,
    BluetoothPassiveScanResetConfig, BluetoothPassiveScanSchedulerWindow,
    BluetoothPassiveScanStartSelection,
};
#[cfg(not(target_arch = "riscv32"))]
pub use passive_scanning_memory::BluetoothPassiveScanMemoryGraphModelAddress;
pub use passive_scanning_memory::{
    BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT, BLUETOOTH_PASSIVE_SCAN_RX_PACKET_BYTES,
    BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES, BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY,
    BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT, BluetoothPassiveScanMemoryGraphBindError,
    BluetoothPassiveScanMemoryGraphBindFailure, BluetoothPassiveScanMemoryGraphCommandPublished,
    BluetoothPassiveScanMemoryGraphCompletionObservation,
    BluetoothPassiveScanMemoryGraphCompletionObserved, BluetoothPassiveScanMemoryGraphCpuOwned,
    BluetoothPassiveScanMemoryGraphEventPrepared, BluetoothPassiveScanMemoryGraphPublicationError,
    BluetoothPassiveScanMemoryGraphPublicationMismatch,
    BluetoothPassiveScanMemoryGraphPublicationPrepared, BluetoothPassiveScanMemoryGraphPublished,
    BluetoothPassiveScanMemoryGraphRecycleError, BluetoothPassiveScanMemoryGraphRecycleFailure,
    BluetoothPassiveScanMemoryGraphRecyclePrepared, BluetoothPassiveScanMemoryGraphRecycled,
    BluetoothPassiveScanMemoryGraphRunning, BluetoothPassiveScanMemoryGraphRxExtracted,
    BluetoothPassiveScanMemoryGraphRxExtractionFailure,
    BluetoothPassiveScanMemoryGraphSchedulerAdmissionPrepared,
    BluetoothPassiveScanMemoryGraphStorage, BluetoothPassiveScanSchedulerAllocationConfig,
    BluetoothPassiveScanSchedulerItemCompletionStatus,
};
#[cfg(not(target_arch = "riscv32"))]
pub use peripheral_connection_memory::BluetoothPeripheralConnectionMemoryGraphModelAddress;
pub use peripheral_connection_memory::{
    BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT,
    BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES,
    BluetoothPeripheralConnectionCapturedAnchorAvailability,
    BluetoothPeripheralConnectionCapturedAnchorTime, BluetoothPeripheralConnectionDataChannel,
    BluetoothPeripheralConnectionDefaultTxPowerDbm, BluetoothPeripheralConnectionEventSpan,
    BluetoothPeripheralConnectionIdentity, BluetoothPeripheralConnectionIntervalTicks,
    BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned,
    BluetoothPeripheralConnectionMemoryGraphBindError,
    BluetoothPeripheralConnectionMemoryGraphBindFailure,
    BluetoothPeripheralConnectionMemoryGraphCompletionObservation,
    BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
    BluetoothPeripheralConnectionMemoryGraphCpuOwned,
    BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared,
    BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared,
    BluetoothPeripheralConnectionMemoryGraphIdentity,
    BluetoothPeripheralConnectionMemoryGraphIdentityPrepared,
    BluetoothPeripheralConnectionMemoryGraphPublicationError,
    BluetoothPeripheralConnectionMemoryGraphPublicationMismatch,
    BluetoothPeripheralConnectionMemoryGraphPublicationPrepared,
    BluetoothPeripheralConnectionMemoryGraphReceivePrepared,
    BluetoothPeripheralConnectionMemoryGraphRecurringEventFieldsPrepared,
    BluetoothPeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared,
    BluetoothPeripheralConnectionMemoryGraphRecycleError,
    BluetoothPeripheralConnectionMemoryGraphRecycleFailure,
    BluetoothPeripheralConnectionMemoryGraphRecyclePrepared,
    BluetoothPeripheralConnectionMemoryGraphRecycled,
    BluetoothPeripheralConnectionMemoryGraphRunning,
    BluetoothPeripheralConnectionMemoryGraphRxExtracted,
    BluetoothPeripheralConnectionMemoryGraphRxExtractionFailure,
    BluetoothPeripheralConnectionMemoryGraphRxPublished,
    BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared,
    BluetoothPeripheralConnectionMemoryGraphStorage, BluetoothPeripheralConnectionReceiveWait,
    BluetoothPeripheralConnectionRecurringReceiveWait,
    BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
    BluetoothPeripheralConnectionSchedulerPriority, BluetoothPeripheralConnectionSchedulerWindow,
};
pub use rx_memory_list::BluetoothRxMemoryListClass;
pub use scheduler_context::{BLUETOOTH_SCHEDULER_CONTEXT_BYTES, BluetoothSchedulerContextStorage};
pub use sram_link::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
    BluetoothControllerSramLinkAddress, BluetoothControllerSramLinkAddressError,
};
