//! Fixed receive-memory pool shared by ESP32-S31 non-scanning BLE roles.
//!
//! The current controller routes ordinary advertising and connection items to
//! positional RX-list selector two. The vendor allocator and its global
//! bookkeeping are not part of this boundary: the open driver owns the
//! initial completed packetless cursor followed by two writable packet nodes.
//! It transfers its affine owner between response-capable advertising and a
//! connection. Recurring connection events retain the current packet node and
//! rearm the other node as its writable successor.

#![forbid(unsafe_code)]

use core::{cell::Cell, marker::PhantomPinned, pin::Pin};

use crate::{
    le_rx_packet::{
        LeReceivedBatch, LeRxBufferHeaderStorage, LeRxError, LeRxNodeStorage, LeRxPacketAddress,
        extract_completed_rx_batch,
    },
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        ControllerSramLinkAddress,
    },
};

use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};

use pin_project::pin_project;

/// Receive nodes retained by the first non-scanning BLE pool.
pub const BLUETOOTH_NON_SCANNING_RX_NODE_COUNT: usize = 2;

#[derive(Clone, Copy)]
enum ConnectionRxCursor {
    Initial,
    First,
    Second,
}

impl ConnectionRxCursor {
    fn index(self) -> Option<usize> {
        match self {
            Self::Initial => None,
            Self::First => Some(0),
            Self::Second => Some(1),
        }
    }
}

#[repr(C)]
#[pin_project]
pub struct NonScanningRxMemoryStorage {
    predecessor: LeRxBufferHeaderStorage,
    connection_cursor: Cell<ConnectionRxCursor>,
    nodes: [LeRxNodeStorage; BLUETOOTH_NON_SCANNING_RX_NODE_COUNT],
    #[pin]
    _pin: PhantomPinned,
}

const STORAGE_BYTES: u32 = core::mem::size_of::<NonScanningRxMemoryStorage>() as u32;
const NODE_BYTES: u32 = core::mem::size_of::<LeRxNodeStorage>() as u32;
const NODES_OFFSET: u32 = core::mem::offset_of!(NonScanningRxMemoryStorage, nodes) as u32;
const PACKET_OFFSET: u32 = core::mem::offset_of!(LeRxNodeStorage, packet) as u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NonScanningRxNodeBinding {
    header: ControllerSramLinkAddress,
    packet: LeRxPacketAddress,
}

struct NonScanningRxMemoryBinding {
    identity: NonScanningRxMemoryIdentity,
    end_exclusive: u32,
    predecessor: ControllerSramLinkAddress,
    nodes: [NonScanningRxNodeBinding; BLUETOOTH_NON_SCANNING_RX_NODE_COUNT],
}

impl NonScanningRxMemoryBinding {
    fn new(
        identity: NonScanningRxMemoryIdentity,
        base: u32,
    ) -> Result<Self, NonScanningRxMemoryBindError> {
        let end_exclusive = base
            .checked_add(STORAGE_BYTES)
            .ok_or(NonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(NonScanningRxMemoryBindError::ExtentOutsidePhysicalSram);
        }

        let node = |index: u32| {
            let node_base = base
                .checked_add(NODES_OFFSET)
                .ok_or(NonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?
                .checked_add(
                    index
                        .checked_mul(NODE_BYTES)
                        .ok_or(NonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?,
                )
                .ok_or(NonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?;
            let header = ControllerSramLinkAddress::new(node_base)
                .map_err(|_| NonScanningRxMemoryBindError::ZeroCompressedLink)?;
            let packet = LeRxPacketAddress::new(
                node_base
                    .checked_add(PACKET_OFFSET)
                    .ok_or(NonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?,
            )
            .map_err(NonScanningRxMemoryBindError::InvalidAddress)?;
            if packet.compressed_image() == 0 {
                return Err(NonScanningRxMemoryBindError::ZeroCompressedLink);
            }
            Ok(NonScanningRxNodeBinding { header, packet })
        };

        Ok(Self {
            identity,
            end_exclusive,
            predecessor: ControllerSramLinkAddress::new(base)
                .map_err(|_| NonScanningRxMemoryBindError::ZeroCompressedLink)?,
            nodes: [node(0)?, node(1)?],
        })
    }
}

/// Opaque identity of one exact statically pinned non-scanning RX pool.
///
/// This value supports equality only and exposes no address or dereference
/// operation to the controller layer.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct NonScanningRxMemoryIdentity(usize);

impl NonScanningRxMemoryIdentity {
    fn for_storage(storage: &NonScanningRxMemoryStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for NonScanningRxMemoryIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("NonScanningRxMemoryIdentity")
            .finish_non_exhaustive()
    }
}

/// Why the shared non-scanning RX pool cannot be bound to controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NonScanningRxMemoryBindError {
    AddressWidth,
    InvalidAddress(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
}

/// Failed binding that returns the exact unchanged static RX allocation.
pub struct NonScanningRxMemoryBindFailure {
    storage: &'static mut NonScanningRxMemoryStorage,
    error: NonScanningRxMemoryBindError,
}

impl NonScanningRxMemoryBindFailure {
    fn new(
        storage: &'static mut NonScanningRxMemoryStorage,
        error: NonScanningRxMemoryBindError,
    ) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> NonScanningRxMemoryBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut NonScanningRxMemoryStorage,
        NonScanningRxMemoryBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for NonScanningRxMemoryBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("NonScanningRxMemoryBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonScanningRxMemoryModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl NonScanningRxMemoryModelAddress {
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        match BluetoothControllerSramAddress::new(address) {
            Ok(address) => Ok(Self(address)),
            Err(error) => Err(error),
        }
    }

    const fn address(self) -> u32 {
        self.0.address()
    }
}

/// Unique CPU owner of the initialized selector-two RX rotation graph.
#[must_use = "the non-scanning receive pool must be retained or transferred"]
pub struct NonScanningRxMemoryCpuOwned {
    storage: Pin<&'static mut NonScanningRxMemoryStorage>,
    binding: NonScanningRxMemoryBinding,
}

impl NonScanningRxMemoryCpuOwned {
    /// Equality witness for the exact pinned storage object.
    pub const fn identity(&self) -> NonScanningRxMemoryIdentity {
        self.binding.identity
    }

    pub(crate) const fn current_cursor(&self) -> BluetoothControllerSramAddress {
        self.binding.predecessor.controller_address()
    }

    pub(crate) const fn tail(&self) -> BluetoothControllerSramAddress {
        self.binding.nodes[1].header.controller_address()
    }

    pub(crate) const fn controller_range(&self) -> (u32, u32) {
        (
            self.binding.predecessor.controller_address().address(),
            self.binding.end_exclusive,
        )
    }

    /// Whether the initial cursor precedes both armed packet allocations.
    pub fn is_initialized(&self) -> bool {
        let storage = self.storage.as_ref().get_ref();
        let [first, second] = self.binding.nodes;
        storage.predecessor.is_packetless()
            && storage.predecessor.completion_observed()
            && storage.predecessor.successor() == Some(first.header.compressed_image())
            && storage.nodes[0].packet.is_armed()
            && storage.nodes[1].packet.is_armed()
            && storage.nodes[0].header.retains_packet(first.packet)
            && storage.nodes[0].header.successor() == Some(second.header.compressed_image())
            && storage.nodes[0].header.predecessor() == Some(self.current_cursor().address())
            && storage.nodes[0].header.rotates_into_successor()
            && storage.nodes[1].header.retains_packet(second.packet)
            && storage.nodes[1].header.successor().is_none()
            && storage.nodes[1].header.predecessor()
                == Some(first.header.controller_address().address())
            && !storage.nodes[1].header.rotates_into_successor()
    }

    /// Validate and copy every contiguous completed node without mutating SRAM.
    ///
    /// Connection reclamation keeps this exact affine pool owner on failure.
    /// A successful caller must finish consuming the copied batch before
    /// explicitly rearming the pool with [`Self::reinitialize_after_event`].
    pub(crate) fn extract_completed_rx_batch(
        &self,
    ) -> Result<LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>, LeRxError> {
        extract_completed_rx_batch(&self.storage.as_ref().get_ref().nodes)
    }

    pub(crate) fn extract_completed_connection_rx_batch(
        &self,
    ) -> Result<LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>, LeRxError> {
        let storage = self.storage.as_ref().get_ref();
        match storage.connection_cursor.get().index() {
            None => crate::le_rx_packet::extract_completed_connection_rx_batch(&storage.nodes),
            Some(current) => crate::le_rx_packet::extract_completed_rx_nodes(
                core::iter::once(&storage.nodes[1 - current]),
                true,
            ),
        }
    }

    pub(crate) fn connection_current_cursor(&self) -> BluetoothControllerSramAddress {
        match self
            .storage
            .as_ref()
            .get_ref()
            .connection_cursor
            .get()
            .index()
        {
            None => self.current_cursor(),
            Some(current) => self.binding.nodes[current].header.controller_address(),
        }
    }

    pub(crate) fn connection_tail(&self) -> BluetoothControllerSramAddress {
        match self
            .storage
            .as_ref()
            .get_ref()
            .connection_cursor
            .get()
            .index()
        {
            None => self.tail(),
            Some(current) => self.binding.nodes[1 - current].header.controller_address(),
        }
    }

    pub(crate) fn connection_resources_ready(&self) -> bool {
        let storage = self.storage.as_ref().get_ref();
        let Some(current) = storage.connection_cursor.get().index() else {
            return self.is_initialized();
        };
        let next = 1 - current;
        storage.nodes[current].header.completion_observed()
            && storage.nodes[current].header.successor()
                == Some(self.binding.nodes[next].header.compressed_image())
            && !storage.nodes[next].header.completion_observed()
            && storage.nodes[next]
                .header
                .retains_packet(self.binding.nodes[next].packet)
            && storage.nodes[next].header.successor().is_none()
            && storage.nodes[next].packet.is_armed()
    }

    /// Retain the last completed descriptor and its packet until hardware has
    /// advanced to its successor. Recurring events have one writable RX slot.
    pub(crate) fn rotate_after_connection_event(&mut self) {
        let storage = self.storage.as_ref().get_ref();
        let cursor = match storage.connection_cursor.get().index() {
            None => {
                if storage.nodes[1].header.completion_observed() {
                    ConnectionRxCursor::Second
                } else if storage.nodes[0].header.completion_observed() {
                    ConnectionRxCursor::First
                } else {
                    return;
                }
            }
            Some(current) => {
                let advanced = storage.nodes[1 - current].header.completion_observed();
                match (current, advanced) {
                    (0, false) | (1, true) => ConnectionRxCursor::First,
                    _ => ConnectionRxCursor::Second,
                }
            }
        };
        let current = cursor.index().expect("a connection cursor is retained");
        let next = 1 - current;
        // Only the non-current allocation is reusable. Rewinding the private
        // hardware cursor or rearming its packet breaks the next exchange.
        storage.nodes[next].packet.initialize();
        storage.nodes[next].header.install(
            self.binding.nodes[next].packet,
            None,
            Some(self.binding.nodes[current].header.controller_address()),
            false,
        );
        storage.nodes[current]
            .header
            .append_successor(self.binding.nodes[next].header);
        storage.connection_cursor.set(cursor);
    }

    pub(crate) fn observe_nodes(
        &self,
    ) -> [crate::LeRxNodeObservation; BLUETOOTH_NON_SCANNING_RX_NODE_COUNT] {
        let storage = self.storage.as_ref().get_ref();
        core::array::from_fn(|index| {
            let node = &storage.nodes[index];
            node.packet
                .observe(&node.header, self.binding.nodes[index].packet)
        })
    }

    /// Rearm both packet allocations after the completed event was copied.
    pub(crate) fn reinitialize_after_event(&mut self) {
        self.reinitialize();
    }

    #[cfg(test)]
    pub(crate) fn model_controller_receive_after_current(
        &self,
        current: BluetoothControllerSramAddress,
        pdu: &[u8],
    ) -> BluetoothControllerSramAddress {
        let storage = self.storage.as_ref().get_ref();
        let header = if current == self.binding.predecessor.controller_address() {
            &storage.predecessor
        } else {
            let index = self
                .binding
                .nodes
                .iter()
                .position(|node| node.header.controller_address() == current)
                .expect("the current cursor belongs to the retained pool");
            &storage.nodes[index].header
        };
        let next = header.successor().expect("a writable RX successor remains");
        let index = self
            .binding
            .nodes
            .iter()
            .position(|node| node.header.compressed_image() == next)
            .expect("the successor is a retained packet-bearing node");
        assert!(
            storage.nodes[index]
                .header
                .retains_packet(self.binding.nodes[index].packet)
        );
        self.model_controller_receive(index, pdu, -42, 1_000);
        self.binding.nodes[index].header.controller_address()
    }

    #[cfg(test)]
    pub(crate) fn model_controller_receive(
        &self,
        index: usize,
        pdu: &[u8],
        rssi_dbm: i8,
        captured_time: u32,
    ) {
        let node = &self.storage.as_ref().get_ref().nodes[index];
        node.packet
            .emulate_hardware_receive(pdu, rssi_dbm, captured_time);
        node.header.emulate_hardware_completion();
    }

    #[cfg(test)]
    pub(crate) fn model_controller_discard(&self, index: usize) {
        let node = &self.storage.as_ref().get_ref().nodes[index];
        node.packet.emulate_hardware_discard();
        node.header.emulate_hardware_completion();
    }

    fn reinitialize(&mut self) {
        let bindings = self.binding.nodes;
        let storage = self.storage.as_mut().project();
        storage.connection_cursor.set(ConnectionRxCursor::Initial);
        storage
            .predecessor
            .install_completed_predecessor(bindings[0].header);
        for (node, binding) in storage.nodes.iter().zip(bindings) {
            node.packet.initialize();
            node.header.install(binding.packet, None, None, false);
        }
        storage.nodes[0].header.install(
            bindings[0].packet,
            Some(bindings[1].header),
            Some(self.binding.predecessor.controller_address()),
            true,
        );
        storage.nodes[1].header.install(
            bindings[1].packet,
            None,
            Some(bindings[0].header.controller_address()),
            false,
        );
    }
}

impl NonScanningRxMemoryStorage {
    pub const fn new() -> Self {
        Self {
            predecessor: LeRxBufferHeaderStorage::new(),
            connection_cursor: Cell::new(ConnectionRxCursor::Initial),
            nodes: [LeRxNodeStorage::new(), LeRxNodeStorage::new()],
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<NonScanningRxMemoryCpuOwned, NonScanningRxMemoryBindFailure> {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(NonScanningRxMemoryBindFailure::new(
                    storage,
                    NonScanningRxMemoryBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: NonScanningRxMemoryModelAddress,
    ) -> Result<NonScanningRxMemoryCpuOwned, NonScanningRxMemoryBindFailure> {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<NonScanningRxMemoryCpuOwned, NonScanningRxMemoryBindFailure> {
        let identity = NonScanningRxMemoryIdentity::for_storage(storage);
        let binding = match NonScanningRxMemoryBinding::new(identity, base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(NonScanningRxMemoryBindFailure::new(storage, error));
            }
        };
        let mut owner = NonScanningRxMemoryCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize();
        Ok(owner)
    }
}

impl Default for NonScanningRxMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
