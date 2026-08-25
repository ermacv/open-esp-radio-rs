//! ESP32-S31 Bluetooth controller-memory boundary.
//!
//! The restricted PAC below this crate owns positional MMIO transactions and
//! compressed-address encoding. This crate owns controller-SRAM layouts and
//! role-to-list routing that are consumed above the register boundary. It is
//! intentionally sparse: static graph location and allocation-time links are
//! bound, while hardware publication, device fences and affine reclamation
//! still require independent proof.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

mod dtm_rx_result;
mod dtm_storage;
mod rx_memory_list;
mod sram_link;

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
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphStorage,
    BluetoothDtmPreparedTxPacketStorage, BluetoothDtmRxBufferHeaderImage,
    BluetoothDtmRxBufferStorage, BluetoothDtmRxPacketAddress, BluetoothDtmRxPacketAddressError,
    BluetoothDtmRxPacketStorage, BluetoothDtmRxRearmError, BluetoothDtmSchedulerContextStorage,
    BluetoothDtmSchedulerItemStorage, BluetoothDtmTxBufferHeaderImage, BluetoothDtmTxPacketAddress,
    BluetoothDtmTxPacketAddressError, BluetoothDtmTxPacketPreparation, BluetoothDtmTxPacketStorage,
};
pub use rx_memory_list::BluetoothRxMemoryListClass;
pub use sram_link::{BluetoothDtmBoundSramLinkAddress, BluetoothDtmBoundSramLinkAddressError};
