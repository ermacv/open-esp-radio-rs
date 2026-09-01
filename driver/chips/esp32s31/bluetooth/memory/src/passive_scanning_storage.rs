//! Fixed controller-SRAM receive arena for legacy passive scanning.
//!
//! The vendor allocator is deliberately absent. This module owns the reviewed
//! two-node header/packet topology, private SRAM encoding and affine
//! publication boundary needed by the first passive LE 1M scanner.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListSelector, BluetoothRxMemoryListPublished,
};
use pin_project::pin_project;
use vcell::VolatileCell;

use crate::{
    rx_memory_list::BluetoothRxMemoryListClass,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

/// Number of independently backed receive nodes in the first passive scanner.
pub const BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT: usize = 2;
/// Bytes preceding a received Link Layer payload in one controller allocation.
pub const BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES: usize = 0x1e;
/// Maximum Link Layer payload admitted by the first scanner arena.
pub const BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY: usize = u8::MAX as usize;
/// Complete logical receive-packet allocation size.
pub const BLUETOOTH_PASSIVE_SCAN_RX_PACKET_BYTES: usize =
    BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES + BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY;

const BUFFER_HEADER_BYTES: usize = 0x18;
const BUFFER_HEADER_WORDS: usize = BUFFER_HEADER_BYTES / 4;
const RX_PACKET_WORDS: usize = BLUETOOTH_PASSIVE_SCAN_RX_PACKET_BYTES.div_ceil(4);
const RX_PACKET_LAST_ALIGNED_OFFSET: u32 =
    ((BLUETOOTH_PASSIVE_SCAN_RX_PACKET_BYTES as u32 - 1) / 4) * 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothPassiveScanRxPacketAddress(BluetoothControllerSramAddress);

impl BluetoothPassiveScanRxPacketAddress {
    fn new(address: u32) -> Result<Self, BluetoothPassiveScanRxArenaBindError> {
        let address = BluetoothControllerSramAddress::new(address)
            .map_err(BluetoothPassiveScanRxArenaBindError::InvalidAddress)?;
        if address.compressed_image() == 0 {
            return Err(BluetoothPassiveScanRxArenaBindError::ZeroCompressedLink);
        }
        let tail = address
            .address()
            .checked_add(RX_PACKET_LAST_ALIGNED_OFFSET)
            .ok_or(BluetoothPassiveScanRxArenaBindError::ExtentOutsidePhysicalSram)?;
        BluetoothControllerSramAddress::new(tail)
            .map_err(BluetoothPassiveScanRxArenaBindError::InvalidAddress)?;
        Ok(Self(address))
    }

    const fn compressed_image(self) -> u32 {
        self.0.compressed_image()
    }
}

/// Private controller-shared receive-buffer header.
#[repr(C, align(4))]
struct BluetoothPassiveScanRxBufferHeaderStorage {
    words: [VolatileCell<u32>; BUFFER_HEADER_WORDS],
}

impl BluetoothPassiveScanRxBufferHeaderStorage {
    #[cfg(test)]
    const LINK_MASK: u32 = 0x000f_ffff;
    const ROTATION_MARKER: u32 = 1;

    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BUFFER_HEADER_WORDS],
        }
    }

    fn install(
        &self,
        packet: BluetoothPassiveScanRxPacketAddress,
        successor: Option<BluetoothControllerSramLinkAddress>,
        predecessor: Option<BluetoothControllerSramAddress>,
        rotates_into_successor: bool,
    ) {
        let successor = successor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image);
        let predecessor = predecessor.map_or(0, BluetoothControllerSramAddress::address);
        let rotation = if rotates_into_successor {
            Self::ROTATION_MARKER
        } else {
            0
        };
        let image = [
            successor,
            packet.compressed_image(),
            0x8080_0000,
            0,
            rotation,
            predecessor,
        ];
        for (cell, word) in self.words.iter().zip(image) {
            cell.set(word);
        }
    }

    #[cfg(test)]
    fn retains_packet(&self, packet: BluetoothPassiveScanRxPacketAddress) -> bool {
        self.words[1].get() & Self::LINK_MASK == packet.compressed_image()
    }

    #[cfg(test)]
    fn successor(&self) -> Option<u32> {
        let image = self.words[0].get() & Self::LINK_MASK;
        (image != 0).then_some(image)
    }

    #[cfg(test)]
    fn predecessor(&self) -> Option<u32> {
        let address = self.words[5].get();
        (address != 0).then_some(address)
    }

    #[cfg(test)]
    fn rotates_into_successor(&self) -> bool {
        self.words[4].get() & Self::ROTATION_MARKER != 0
    }
}

/// Private controller-shared receive packet allocation.
#[repr(C, align(4))]
struct BluetoothPassiveScanRxPacketStorage {
    words: [VolatileCell<u32>; RX_PACKET_WORDS],
}

impl BluetoothPassiveScanRxPacketStorage {
    const CAPACITY_WORD: usize = 1;
    const RESULT_WORD: usize = 3;
    const EPOCH_WORD: usize = 6;
    const RESULT_REARM_SENTINEL: u32 = 0x00ff_ffff;
    const EPOCH_REARM_SENTINEL: u32 = 0x0000_ffff;
    const CAPACITY_IMAGE: u32 = 0x0001_0100;

    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; RX_PACKET_WORDS],
        }
    }

    fn initialize(&self) {
        for word in &self.words {
            word.set(0);
        }
        self.words[Self::CAPACITY_WORD].set(Self::CAPACITY_IMAGE);
        self.rearm();
    }

    fn rearm(&self) {
        self.words[Self::RESULT_WORD]
            .set(self.words[Self::RESULT_WORD].get() | Self::RESULT_REARM_SENTINEL);
        self.words[Self::EPOCH_WORD]
            .set(self.words[Self::EPOCH_WORD].get() | Self::EPOCH_REARM_SENTINEL);
    }

    #[cfg(test)]
    fn is_armed(&self) -> bool {
        self.words[Self::RESULT_WORD].get() & Self::RESULT_REARM_SENTINEL
            == Self::RESULT_REARM_SENTINEL
            && self.words[Self::EPOCH_WORD].get() & Self::EPOCH_REARM_SENTINEL
                == Self::EPOCH_REARM_SENTINEL
    }
}

#[repr(C)]
struct BluetoothPassiveScanRxNodeStorage {
    header: BluetoothPassiveScanRxBufferHeaderStorage,
    packet: BluetoothPassiveScanRxPacketStorage,
}

impl BluetoothPassiveScanRxNodeStorage {
    const fn new() -> Self {
        Self {
            header: BluetoothPassiveScanRxBufferHeaderStorage::new(),
            packet: BluetoothPassiveScanRxPacketStorage::new(),
        }
    }
}

/// Complete no-heap allocation for the first passive-scanner RX chain.
///
/// The storage has no address or publication methods until a unique static
/// allocation is pinned and validated against physical S31 SRAM.
#[pin_project]
#[repr(C)]
pub struct BluetoothPassiveScanRxArenaStorage {
    nodes: [BluetoothPassiveScanRxNodeStorage; BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT],
    #[pin]
    _pin: PhantomPinned,
}

const RX_ARENA_BYTES: u32 = core::mem::size_of::<BluetoothPassiveScanRxArenaStorage>() as u32;
const RX_NODE_BYTES: u32 = core::mem::size_of::<BluetoothPassiveScanRxNodeStorage>() as u32;
const RX_PACKET_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPassiveScanRxNodeStorage, packet) as u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothPassiveScanRxNodeBinding {
    header: BluetoothControllerSramLinkAddress,
    packet: BluetoothPassiveScanRxPacketAddress,
}

/// Why a static passive-scanner allocation cannot become CPU-owned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanRxArenaBindError {
    /// A target pointer cannot be represented by the S31 32-bit address space.
    AddressWidth,
    /// One component is outside the compressed controller-address domain.
    InvalidAddress(BluetoothControllerSramAddressError),
    /// Some byte of the complete arena is outside physical internal SRAM.
    ExtentOutsidePhysicalSram,
    /// A required graph component would encode as the unbound zero link.
    ZeroCompressedLink,
}

struct BluetoothPassiveScanRxArenaBinding {
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    nodes: [BluetoothPassiveScanRxNodeBinding; BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT],
}

impl BluetoothPassiveScanRxArenaBinding {
    fn new(base: u32) -> Result<Self, BluetoothPassiveScanRxArenaBindError> {
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothPassiveScanRxArenaBindError::InvalidAddress)?;
        let end_exclusive = base
            .checked_add(RX_ARENA_BYTES)
            .ok_or(BluetoothPassiveScanRxArenaBindError::ExtentOutsidePhysicalSram)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(BluetoothPassiveScanRxArenaBindError::ExtentOutsidePhysicalSram);
        }

        let node = |index: u32| {
            let node_base = base
                .checked_add(index * RX_NODE_BYTES)
                .ok_or(BluetoothPassiveScanRxArenaBindError::ExtentOutsidePhysicalSram)?;
            let header = BluetoothControllerSramLinkAddress::new(node_base)
                .map_err(|_| BluetoothPassiveScanRxArenaBindError::ZeroCompressedLink)?;
            let packet = BluetoothPassiveScanRxPacketAddress::new(
                node_base
                    .checked_add(RX_PACKET_OFFSET)
                    .ok_or(BluetoothPassiveScanRxArenaBindError::ExtentOutsidePhysicalSram)?,
            )?;
            Ok(BluetoothPassiveScanRxNodeBinding { header, packet })
        };

        Ok(Self {
            base: base_address,
            end_exclusive,
            nodes: [node(0)?, node(1)?],
        })
    }

    const fn head(&self) -> BluetoothControllerSramAddress {
        self.nodes[0].header.controller_address()
    }

    const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }
}

/// Failed static binding retaining the exact allocation unchanged.
#[must_use = "failed binding still owns the scanner RX arena"]
pub struct BluetoothPassiveScanRxArenaBindFailure {
    storage: &'static mut BluetoothPassiveScanRxArenaStorage,
    error: BluetoothPassiveScanRxArenaBindError,
}

impl BluetoothPassiveScanRxArenaBindFailure {
    /// Return the finite binding failure reason.
    pub const fn error(&self) -> BluetoothPassiveScanRxArenaBindError {
        self.error
    }

    /// Recover the unchanged allocation and its failure reason.
    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothPassiveScanRxArenaStorage,
        BluetoothPassiveScanRxArenaBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothPassiveScanRxArenaBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothPassiveScanRxArenaBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic physical-SRAM base for native ownership tests.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPassiveScanRxArenaModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothPassiveScanRxArenaModelAddress {
    /// Validate one deterministic controller-SRAM model address.
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

/// CPU-owned, initialized scanner arena not visible to hardware.
#[must_use = "the initialized scanner arena owns its static allocation"]
pub struct BluetoothPassiveScanRxArenaCpuOwned {
    storage: Pin<&'static mut BluetoothPassiveScanRxArenaStorage>,
    binding: BluetoothPassiveScanRxArenaBinding,
}

impl BluetoothPassiveScanRxArenaCpuOwned {
    /// Return the complete physical SRAM range occupied by the arena.
    pub const fn range(&self) -> (u32, u32) {
        self.binding.range()
    }

    /// Freeze CPU initialization before an upper controller owner performs the
    /// ordered MMIO publication.
    pub fn prepare_publication(self) -> BluetoothPassiveScanRxArenaPublicationPrepared {
        BluetoothPassiveScanRxArenaPublicationPrepared {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// Initialized pinned arena ready for selector-one list publication.
#[must_use = "the prepared scanner arena must be published or retained"]
pub struct BluetoothPassiveScanRxArenaPublicationPrepared {
    storage: Pin<&'static mut BluetoothPassiveScanRxArenaStorage>,
    binding: BluetoothPassiveScanRxArenaBinding,
}

impl BluetoothPassiveScanRxArenaPublicationPrepared {
    /// Return the memory-layer mapping for the passive-scanner RX list.
    #[doc(hidden)]
    pub const fn selector(&self) -> BluetoothMemoryListSelector {
        BluetoothRxMemoryListClass::Scanning.selector()
    }

    /// Return the validated first header for the HAL publication operation.
    #[doc(hidden)]
    pub const fn head(&self) -> BluetoothControllerSramAddress {
        self.binding.head()
    }

    /// Consume a matching affine HAL publication into hardware ownership.
    #[doc(hidden)]
    pub fn into_published(
        self,
        publication: BluetoothRxMemoryListPublished,
    ) -> Result<BluetoothPassiveScanRxArenaPublished, BluetoothPassiveScanRxArenaPublicationMismatch>
    {
        let error = if publication.selector() != self.selector() {
            Some(BluetoothPassiveScanRxArenaPublicationError::SelectorMismatch)
        } else if publication.head() != self.head() {
            Some(BluetoothPassiveScanRxArenaPublicationError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(BluetoothPassiveScanRxArenaPublicationMismatch {
                prepared: self,
                publication,
                error,
            });
        }
        Ok(BluetoothPassiveScanRxArenaPublished {
            _storage: self.storage,
            binding: self.binding,
            publication,
        })
    }
}

/// Why a HAL receive-list publication does not name this scanner arena.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanRxArenaPublicationError {
    /// The publication belongs to another positional memory list.
    SelectorMismatch,
    /// The publication names another pinned arena head.
    HeadMismatch,
}

/// Failed publication join retaining both affine owners.
#[must_use = "a mismatched publication still owns both the arena and HAL token"]
pub struct BluetoothPassiveScanRxArenaPublicationMismatch {
    prepared: BluetoothPassiveScanRxArenaPublicationPrepared,
    publication: BluetoothRxMemoryListPublished,
    error: BluetoothPassiveScanRxArenaPublicationError,
}

impl BluetoothPassiveScanRxArenaPublicationMismatch {
    /// Return the finite mismatch reason.
    pub const fn error(&self) -> BluetoothPassiveScanRxArenaPublicationError {
        self.error
    }

    /// Recover both unchanged affine owners.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothPassiveScanRxArenaPublicationPrepared,
        BluetoothRxMemoryListPublished,
    ) {
        (self.prepared, self.publication)
    }
}

impl core::fmt::Debug for BluetoothPassiveScanRxArenaPublicationMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothPassiveScanRxArenaPublicationMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Pinned scanner arena visible to and exclusively retained by hardware.
///
/// No completion or CPU-reclaim method exists yet. Those transitions require
/// the next controller interrupt/fence proof and cannot be inferred from list
/// publication alone.
#[must_use = "the published scanner arena remains hardware-owned"]
pub struct BluetoothPassiveScanRxArenaPublished {
    _storage: Pin<&'static mut BluetoothPassiveScanRxArenaStorage>,
    binding: BluetoothPassiveScanRxArenaBinding,
    publication: BluetoothRxMemoryListPublished,
}

impl BluetoothPassiveScanRxArenaPublished {
    /// Return the exact retained arena head without exposing SRAM contents.
    pub const fn head(&self) -> BluetoothControllerSramAddress {
        self.binding.head()
    }

    /// Borrow the matching HAL publication proof.
    #[doc(hidden)]
    pub const fn publication(&self) -> &BluetoothRxMemoryListPublished {
        &self.publication
    }
}

impl BluetoothPassiveScanRxArenaStorage {
    /// Reserve a zero-based scanner arena.
    pub const fn new() -> Self {
        Self {
            nodes: [
                BluetoothPassiveScanRxNodeStorage::new(),
                BluetoothPassiveScanRxNodeStorage::new(),
            ],
            _pin: PhantomPinned,
        }
    }

    /// Bind the real address of one unique static S31 allocation.
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<BluetoothPassiveScanRxArenaCpuOwned, BluetoothPassiveScanRxArenaBindFailure> {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothPassiveScanRxArenaBindFailure {
                    storage,
                    error: BluetoothPassiveScanRxArenaBindError::AddressWidth,
                });
            }
        };
        Self::pin_static_inner(storage, base)
    }

    /// Bind one deterministic physical-SRAM address to a native ownership
    /// model without deriving an address from the host allocation.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothPassiveScanRxArenaModelAddress,
    ) -> Result<BluetoothPassiveScanRxArenaCpuOwned, BluetoothPassiveScanRxArenaBindFailure> {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<BluetoothPassiveScanRxArenaCpuOwned, BluetoothPassiveScanRxArenaBindFailure> {
        let binding = match BluetoothPassiveScanRxArenaBinding::new(base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(BluetoothPassiveScanRxArenaBindFailure { storage, error });
            }
        };
        let mut owner = BluetoothPassiveScanRxArenaCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.initialize();
        Ok(owner)
    }
}

impl BluetoothPassiveScanRxArenaCpuOwned {
    fn initialize(&mut self) {
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

impl Default for BluetoothPassiveScanRxArenaStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BluetoothPassiveScanRxArenaBindError,
        BluetoothPassiveScanRxArenaModelAddress, BluetoothPassiveScanRxArenaStorage,
    };

    fn model_arena(base: u32) -> super::BluetoothPassiveScanRxArenaCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPassiveScanRxArenaStorage::new(),
        ));
        let address = BluetoothPassiveScanRxArenaModelAddress::new(base)
            .expect("the model address is controller-encodable");
        BluetoothPassiveScanRxArenaStorage::pin_static_model(storage, address)
            .expect("the arena fits physical controller SRAM")
    }

    #[test]
    fn initialized_arena_is_a_two_node_scanning_chain() {
        let owner = model_arena(0x2f00_0100);
        let bindings = owner.binding.nodes;
        let storage = owner.storage.as_ref().get_ref();

        assert!(storage.nodes[0].header.retains_packet(bindings[0].packet));
        assert!(storage.nodes[1].header.retains_packet(bindings[1].packet));
        assert_eq!(
            storage.nodes[0].header.successor(),
            Some(bindings[1].header.compressed_image())
        );
        assert_eq!(storage.nodes[1].header.successor(), None);
        assert_eq!(storage.nodes[0].header.predecessor(), None);
        assert_eq!(
            storage.nodes[1].header.predecessor(),
            Some(bindings[0].header.controller_address().address())
        );
        assert!(storage.nodes[0].header.rotates_into_successor());
        assert!(!storage.nodes[1].header.rotates_into_successor());
        assert!(storage.nodes.iter().all(|node| node.packet.is_armed()));

        let prepared = owner.prepare_publication();
        assert_eq!(prepared.head(), bindings[0].header.controller_address());
        assert_eq!(
            prepared.selector(),
            crate::BluetoothRxMemoryListClass::Scanning.selector()
        );
    }

    #[test]
    fn failed_extent_binding_retains_the_exact_static_allocation() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPassiveScanRxArenaStorage::new(),
        ));
        let identity = core::ptr::addr_of!(*storage).addr();
        let address = BluetoothPassiveScanRxArenaModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 4,
        )
        .expect("the aligned address remains controller-encodable");

        let failure = match BluetoothPassiveScanRxArenaStorage::pin_static_model(storage, address) {
            Ok(_) => panic!("the complete arena must not cross physical SRAM"),
            Err(failure) => failure,
        };

        assert_eq!(
            failure.error(),
            BluetoothPassiveScanRxArenaBindError::ExtentOutsidePhysicalSram
        );
        let (storage, _) = failure.into_parts();
        assert_eq!(core::ptr::addr_of!(*storage).addr(), identity);
    }
}
