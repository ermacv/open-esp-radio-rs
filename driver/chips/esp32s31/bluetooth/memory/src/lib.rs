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
mod dtm_event_image;
mod dtm_rx_result;
mod dtm_storage;
mod rx_memory_list;
mod sram_link;

#[cfg(not(target_arch = "riscv32"))]
pub use ble_phy_engine::BluetoothBlePhyEngineModelAddress;
pub use ble_phy_engine::{
    BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES, BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES,
    BluetoothBlePhyEngineBindError, BluetoothBlePhyEngineBindFailure, BluetoothBlePhyEngineBinding,
    BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineStorage,
};

pub use dtm_event_image::{
    BluetoothDtmLinkStateReviewedWords, BluetoothDtmPositionalEventWords,
    BluetoothDtmReceiverEventPhase, BluetoothDtmRole, BluetoothDtmSchedulerItemEventType,
    BluetoothDtmSchedulerItemReviewedWords,
};
pub use dtm_rx_result::{BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError};
#[cfg(not(target_arch = "riscv32"))]
pub use dtm_storage::BluetoothDtmMemoryGraphModelAddress;
pub use dtm_storage::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
    BLUETOOTH_DTM_BUFFER_HEADER_BYTES, BLUETOOTH_DTM_LINK_STATE_BYTES,
    BLUETOOTH_DTM_MAX_PACKET_CAPACITY, BLUETOOTH_DTM_RX_PACKET_BYTES,
    BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES, BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES,
    BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, BLUETOOTH_DTM_TX_PACKET_BYTES,
    BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES, BluetoothDtmBufferHeaderStorage,
    BluetoothDtmLinkStateStorage, BluetoothDtmMemoryGraphBindError,
    BluetoothDtmMemoryGraphBindFailure, BluetoothDtmMemoryGraphBinding,
    BluetoothDtmMemoryGraphCompletionObservation, BluetoothDtmMemoryGraphCompletionObserved,
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphEmptyListLinkPrepared,
    BluetoothDtmMemoryGraphHeadPublished, BluetoothDtmMemoryGraphPositionalEventPrepared,
    BluetoothDtmMemoryGraphPrepareError, BluetoothDtmMemoryGraphPrepareFailure,
    BluetoothDtmMemoryGraphReclaimed, BluetoothDtmMemoryGraphRecycleCleaned,
    BluetoothDtmMemoryGraphRecycleError, BluetoothDtmMemoryGraphRecycleFailure,
    BluetoothDtmMemoryGraphRecyclePrepared, BluetoothDtmMemoryGraphRecycled,
    BluetoothDtmMemoryGraphRunning, BluetoothDtmMemoryGraphRxSuccessObserved,
    BluetoothDtmMemoryGraphRxSuccessRecycleError, BluetoothDtmMemoryGraphRxSuccessRecycleFailure,
    BluetoothDtmMemoryGraphRxSuccessRecyclePrepared,
    BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared, BluetoothDtmMemoryGraphStorage,
    BluetoothDtmMemoryGraphTxPacketPrepared, BluetoothDtmPositionalEventSeed,
    BluetoothDtmPreparedTxPacketStorage, BluetoothDtmRxBufferHeaderImage,
    BluetoothDtmRxPacketAddress, BluetoothDtmRxPacketAddressError, BluetoothDtmRxPacketStorage,
    BluetoothDtmSchedulerAllocationConfig, BluetoothDtmSchedulerContextStorage,
    BluetoothDtmSchedulerItemCompletionStatus, BluetoothDtmSchedulerItemStorage,
    BluetoothDtmTxBufferHeaderImage, BluetoothDtmTxPacketAddress, BluetoothDtmTxPacketAddressError,
    BluetoothDtmTxPacketPreparation, BluetoothDtmTxPacketStorage,
};
pub use rx_memory_list::BluetoothRxMemoryListClass;
pub use sram_link::{BluetoothDtmBoundSramLinkAddress, BluetoothDtmBoundSramLinkAddressError};
