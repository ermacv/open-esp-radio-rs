//! Fixed receive-memory pool shared by ESP32-S31 non-scanning BLE roles.
//!
//! The current controller routes ordinary advertising and connection items to
//! positional RX-list selector two. The vendor allocator and its global
//! bookkeeping are not part of this boundary: the open driver owns the
//! smallest reusable two-node rotation graph directly and transfers its
//! affine owner between response-capable advertising and a connection.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use pin_project::pin_project;

use crate::{
    le_rx_packet::{BluetoothLeRxNodeStorage, BluetoothLeRxPacketAddress},
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

/// Receive nodes retained by the first non-scanning BLE pool.
pub const BLUETOOTH_NON_SCANNING_RX_NODE_COUNT: usize = 2;

#[repr(C)]
#[pin_project]
pub struct BluetoothNonScanningRxMemoryStorage {
    nodes: [BluetoothLeRxNodeStorage; BLUETOOTH_NON_SCANNING_RX_NODE_COUNT],
    #[pin]
    _pin: PhantomPinned,
}

const STORAGE_BYTES: u32 = core::mem::size_of::<BluetoothNonScanningRxMemoryStorage>() as u32;
const NODE_BYTES: u32 = core::mem::size_of::<BluetoothLeRxNodeStorage>() as u32;
const PACKET_OFFSET: u32 = core::mem::offset_of!(BluetoothLeRxNodeStorage, packet) as u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothNonScanningRxNodeBinding {
    header: BluetoothControllerSramLinkAddress,
    packet: BluetoothLeRxPacketAddress,
}

struct BluetoothNonScanningRxMemoryBinding {
    nodes: [BluetoothNonScanningRxNodeBinding; BLUETOOTH_NON_SCANNING_RX_NODE_COUNT],
}

impl BluetoothNonScanningRxMemoryBinding {
    fn new(base: u32) -> Result<Self, BluetoothNonScanningRxMemoryBindError> {
        let end_exclusive = base
            .checked_add(STORAGE_BYTES)
            .ok_or(BluetoothNonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(BluetoothNonScanningRxMemoryBindError::ExtentOutsidePhysicalSram);
        }

        let node = |index: u32| {
            let node_base = base
                .checked_add(
                    index
                        .checked_mul(NODE_BYTES)
                        .ok_or(BluetoothNonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?,
                )
                .ok_or(BluetoothNonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?;
            let header = BluetoothControllerSramLinkAddress::new(node_base)
                .map_err(|_| BluetoothNonScanningRxMemoryBindError::ZeroCompressedLink)?;
            let packet = BluetoothLeRxPacketAddress::new(
                node_base
                    .checked_add(PACKET_OFFSET)
                    .ok_or(BluetoothNonScanningRxMemoryBindError::ExtentOutsidePhysicalSram)?,
            )
            .map_err(BluetoothNonScanningRxMemoryBindError::InvalidAddress)?;
            if packet.compressed_image() == 0 {
                return Err(BluetoothNonScanningRxMemoryBindError::ZeroCompressedLink);
            }
            Ok(BluetoothNonScanningRxNodeBinding { header, packet })
        };

        Ok(Self {
            nodes: [node(0)?, node(1)?],
        })
    }
}

/// Why the shared non-scanning RX pool cannot be bound to controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothNonScanningRxMemoryBindError {
    AddressWidth,
    InvalidAddress(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
}

/// Failed binding that returns the exact unchanged static RX allocation.
pub struct BluetoothNonScanningRxMemoryBindFailure {
    storage: &'static mut BluetoothNonScanningRxMemoryStorage,
    error: BluetoothNonScanningRxMemoryBindError,
}

impl BluetoothNonScanningRxMemoryBindFailure {
    fn new(
        storage: &'static mut BluetoothNonScanningRxMemoryStorage,
        error: BluetoothNonScanningRxMemoryBindError,
    ) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> BluetoothNonScanningRxMemoryBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothNonScanningRxMemoryStorage,
        BluetoothNonScanningRxMemoryBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothNonScanningRxMemoryBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothNonScanningRxMemoryBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothNonScanningRxMemoryModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothNonScanningRxMemoryModelAddress {
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
pub struct BluetoothNonScanningRxMemoryCpuOwned {
    storage: Pin<&'static mut BluetoothNonScanningRxMemoryStorage>,
    binding: BluetoothNonScanningRxMemoryBinding,
}

impl BluetoothNonScanningRxMemoryCpuOwned {
    pub(crate) const fn head(&self) -> BluetoothControllerSramAddress {
        self.binding.nodes[0].header.controller_address()
    }

    pub(crate) const fn tail(&self) -> BluetoothControllerSramAddress {
        self.binding.nodes[1].header.controller_address()
    }

    /// Whether both packet allocations are armed in the bounded rotation graph.
    pub fn is_initialized(&self) -> bool {
        let storage = self.storage.as_ref().get_ref();
        let [first, second] = self.binding.nodes;
        storage.nodes[0].packet.is_armed()
            && storage.nodes[1].packet.is_armed()
            && storage.nodes[0].header.retains_packet(first.packet)
            && storage.nodes[0].header.successor() == Some(second.header.compressed_image())
            && storage.nodes[0].header.predecessor().is_none()
            && storage.nodes[0].header.rotates_into_successor()
            && storage.nodes[1].header.retains_packet(second.packet)
            && storage.nodes[1].header.successor().is_none()
            && storage.nodes[1].header.predecessor()
                == Some(first.header.controller_address().address())
            && !storage.nodes[1].header.rotates_into_successor()
    }

    fn reinitialize(&mut self) {
        let bindings = self.binding.nodes;
        let storage = self.storage.as_mut().project();
        for (node, binding) in storage.nodes.iter().zip(bindings) {
            node.packet.initialize();
            node.header.install(binding.packet, None, None, false);
        }
        storage.nodes[0]
            .header
            .install(bindings[0].packet, Some(bindings[1].header), None, true);
        storage.nodes[1].header.install(
            bindings[1].packet,
            None,
            Some(bindings[0].header.controller_address()),
            false,
        );
    }
}

impl BluetoothNonScanningRxMemoryStorage {
    pub const fn new() -> Self {
        Self {
            nodes: [
                BluetoothLeRxNodeStorage::new(),
                BluetoothLeRxNodeStorage::new(),
            ],
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<BluetoothNonScanningRxMemoryCpuOwned, BluetoothNonScanningRxMemoryBindFailure> {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothNonScanningRxMemoryBindFailure::new(
                    storage,
                    BluetoothNonScanningRxMemoryBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothNonScanningRxMemoryModelAddress,
    ) -> Result<BluetoothNonScanningRxMemoryCpuOwned, BluetoothNonScanningRxMemoryBindFailure> {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<BluetoothNonScanningRxMemoryCpuOwned, BluetoothNonScanningRxMemoryBindFailure> {
        let binding = match BluetoothNonScanningRxMemoryBinding::new(base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(BluetoothNonScanningRxMemoryBindFailure::new(storage, error));
            }
        };
        let mut owner = BluetoothNonScanningRxMemoryCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize();
        Ok(owner)
    }
}

impl Default for BluetoothNonScanningRxMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage};

    #[test]
    fn pinned_pool_forms_one_initialized_two_node_rotation() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothNonScanningRxMemoryStorage::new(),
        ));
        let base = BluetoothNonScanningRxMemoryModelAddress::new(0x2f00_4000)
            .expect("the model base belongs to controller SRAM");
        let owner = BluetoothNonScanningRxMemoryStorage::pin_static_model(storage, base)
            .expect("the complete RX pool fits controller SRAM");

        assert!(owner.is_initialized());
    }
}
