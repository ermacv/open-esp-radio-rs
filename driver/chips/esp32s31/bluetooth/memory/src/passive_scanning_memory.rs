//! Fixed controller-SRAM memory graph for legacy passive scanning.
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
    passive_scanning_event_image::{
        BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS, BluetoothPassiveScanLinkStateImage,
        BluetoothPassiveScanResetConfig, BluetoothPassiveScanRxHeadProjection,
    },
    rx_memory_list::BluetoothRxMemoryListClass,
    scheduler_context::BluetoothSchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

/// Number of independently backed receive nodes in the first passive scanner.
pub const BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT: usize = 2;
/// Number of scheduler-item allocations retained by the scanner graph.
pub const BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT: usize = 3;
/// Bytes preceding a received Link Layer payload in one controller allocation.
pub const BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES: usize = 0x1e;
/// Maximum Link Layer payload admitted by the first scanner graph.
pub const BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY: usize = u8::MAX as usize;
/// Complete logical receive-packet allocation size.
pub const BLUETOOTH_PASSIVE_SCAN_RX_PACKET_BYTES: usize =
    BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES + BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY;

const BUFFER_HEADER_BYTES: usize = 0x18;
const BUFFER_HEADER_WORDS: usize = BUFFER_HEADER_BYTES / 4;
const RX_PACKET_WORDS: usize = BLUETOOTH_PASSIVE_SCAN_RX_PACKET_BYTES.div_ceil(4);
const RX_PACKET_LAST_ALIGNED_OFFSET: u32 =
    ((BLUETOOTH_PASSIVE_SCAN_RX_PACKET_BYTES as u32 - 1) / 4) * 4;
const LINK_STATE_RX_HEAD_WORD: usize = 0x68 / 4;
const LINK_STATE_RX_TAIL_WORD: usize = 0x70 / 4;
const LINK_STATE_RX_SWAP_RESERVE_WORD: usize = 0x78 / 4;
const LINK_STATE_SCHEDULER_HEAD_WORD: usize = 0x64 / 4;
const SCHEDULER_ITEM_BYTES: usize = 0x60;
const SCHEDULER_ITEM_WORDS: usize = SCHEDULER_ITEM_BYTES / 4;
const SCHEDULER_ITEM_HARDWARE_NEXT_WORD: usize = 0;
const SCHEDULER_ITEM_CONTEXT_WORD: usize = 1;
const SCHEDULER_ITEM_LINK_STATE_WORD: usize = 0x08 / 4;
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0030_0000;
const SCHEDULER_ITEM_LINK_STATE_PREFIX: u32 = 0x00c0_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothPassiveScanRxPacketAddress(BluetoothControllerSramAddress);

impl BluetoothPassiveScanRxPacketAddress {
    fn new(address: u32) -> Result<Self, BluetoothPassiveScanMemoryGraphBindError> {
        let address = BluetoothControllerSramAddress::new(address)
            .map_err(BluetoothPassiveScanMemoryGraphBindError::InvalidAddress)?;
        if address.compressed_image() == 0 {
            return Err(BluetoothPassiveScanMemoryGraphBindError::ZeroCompressedLink);
        }
        let tail = address
            .address()
            .checked_add(RX_PACKET_LAST_ALIGNED_OFFSET)
            .ok_or(BluetoothPassiveScanMemoryGraphBindError::ExtentOutsidePhysicalSram)?;
        BluetoothControllerSramAddress::new(tail)
            .map_err(BluetoothPassiveScanMemoryGraphBindError::InvalidAddress)?;
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

/// Private controller-shared scanner link state.
#[repr(C, align(4))]
struct BluetoothPassiveScanLinkStateStorage {
    words: [VolatileCell<u32>; BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS],
}

impl BluetoothPassiveScanLinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS],
        }
    }

    fn install(&self, image: BluetoothPassiveScanLinkStateImage) {
        for (cell, word) in self.words.iter().zip(image.words()) {
            cell.set(word);
        }
    }

    fn install_receive_graph(
        &self,
        head: BluetoothControllerSramAddress,
        tail: BluetoothControllerSramAddress,
    ) {
        self.words[LINK_STATE_RX_HEAD_WORD].set(head.address());
        self.words[LINK_STATE_RX_TAIL_WORD].set(tail.address());
        self.words[LINK_STATE_RX_SWAP_RESERVE_WORD].set(0);
    }

    fn install_scheduler_head(&self, head: BluetoothControllerSramAddress) {
        self.words[LINK_STATE_SCHEDULER_HEAD_WORD].set(head.address());
    }

    #[cfg(test)]
    fn image(&self) -> BluetoothPassiveScanLinkStateImage {
        BluetoothPassiveScanLinkStateImage::from_words(core::array::from_fn(|index| {
            self.words[index].get()
        }))
    }

    #[cfg(test)]
    fn receive_graph(
        &self,
    ) -> (
        BluetoothControllerSramAddress,
        BluetoothControllerSramAddress,
        Option<BluetoothControllerSramAddress>,
    ) {
        let address = |word: usize| {
            BluetoothControllerSramAddress::new(self.words[word].get())
                .expect("the installed scanner graph retains validated addresses")
        };
        let reserve = self.words[LINK_STATE_RX_SWAP_RESERVE_WORD].get();
        (
            address(LINK_STATE_RX_HEAD_WORD),
            address(LINK_STATE_RX_TAIL_WORD),
            (reserve != 0).then(|| address(LINK_STATE_RX_SWAP_RESERVE_WORD)),
        )
    }

    #[cfg(test)]
    fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        BluetoothControllerSramAddress::new(self.words[LINK_STATE_SCHEDULER_HEAD_WORD].get())
            .expect("the scanner link state retains a validated scheduler head")
    }
}

/// Private hardware-shared scheduler item for one scanner event.
#[repr(C, align(4))]
struct BluetoothPassiveScanSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl BluetoothPassiveScanSchedulerItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    fn initialize_graph(
        &self,
        predecessor: Option<BluetoothControllerSramLinkAddress>,
        scheduler_context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_WORD].set(
            SCHEDULER_ITEM_ALLOCATION_PREFIX
                | predecessor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image),
        );
        self.words[SCHEDULER_ITEM_CONTEXT_WORD].set(scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE_WORD]
            .set(SCHEDULER_ITEM_LINK_STATE_PREFIX | link_state.compressed_image());
    }

    #[cfg(test)]
    fn retains_graph(
        &self,
        predecessor: Option<BluetoothControllerSramLinkAddress>,
        scheduler_context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
    ) -> bool {
        let predecessor =
            predecessor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image);
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_WORD].get() & 0x000f_ffff == predecessor
            && self.words[SCHEDULER_ITEM_CONTEXT_WORD].get() == scheduler_context.compressed_image()
            && self.words[SCHEDULER_ITEM_LINK_STATE_WORD].get() & 0x000f_ffff
                == link_state.compressed_image()
    }
}

/// Complete no-heap allocation for the first passive-scanner memory graph.
///
/// The storage has no address or publication methods until a unique static
/// allocation is pinned and validated against physical S31 SRAM.
#[pin_project]
#[repr(C)]
pub struct BluetoothPassiveScanMemoryGraphStorage {
    link_state: BluetoothPassiveScanLinkStateStorage,
    scheduler_context: BluetoothSchedulerContextStorage,
    scheduler_items:
        [BluetoothPassiveScanSchedulerItemStorage; BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT],
    nodes: [BluetoothPassiveScanRxNodeStorage; BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT],
    #[pin]
    _pin: PhantomPinned,
}

const MEMORY_GRAPH_BYTES: u32 =
    core::mem::size_of::<BluetoothPassiveScanMemoryGraphStorage>() as u32;
const RX_NODE_BYTES: u32 = core::mem::size_of::<BluetoothPassiveScanRxNodeStorage>() as u32;
const RX_NODES_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPassiveScanMemoryGraphStorage, nodes) as u32;
const SCHEDULER_CONTEXT_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPassiveScanMemoryGraphStorage, scheduler_context) as u32;
const SCHEDULER_ITEMS_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPassiveScanMemoryGraphStorage, scheduler_items) as u32;
const RX_PACKET_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPassiveScanRxNodeStorage, packet) as u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothPassiveScanRxNodeBinding {
    header: BluetoothControllerSramLinkAddress,
    packet: BluetoothPassiveScanRxPacketAddress,
}

/// Why a static passive-scanner allocation cannot become CPU-owned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanMemoryGraphBindError {
    /// A target pointer cannot be represented by the S31 32-bit address space.
    AddressWidth,
    /// One component is outside the compressed controller-address domain.
    InvalidAddress(BluetoothControllerSramAddressError),
    /// Some byte of the complete graph is outside physical internal SRAM.
    ExtentOutsidePhysicalSram,
    /// A required graph component would encode as the unbound zero link.
    ZeroCompressedLink,
}

struct BluetoothPassiveScanMemoryGraphBinding {
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    link_state: BluetoothControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramLinkAddress,
    scheduler_items:
        [BluetoothControllerSramLinkAddress; BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT],
    nodes: [BluetoothPassiveScanRxNodeBinding; BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT],
}

impl BluetoothPassiveScanMemoryGraphBinding {
    fn new(base: u32) -> Result<Self, BluetoothPassiveScanMemoryGraphBindError> {
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothPassiveScanMemoryGraphBindError::InvalidAddress)?;
        let end_exclusive = base
            .checked_add(MEMORY_GRAPH_BYTES)
            .ok_or(BluetoothPassiveScanMemoryGraphBindError::ExtentOutsidePhysicalSram)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(BluetoothPassiveScanMemoryGraphBindError::ExtentOutsidePhysicalSram);
        }
        let link_state = BluetoothControllerSramLinkAddress::new(base)
            .map_err(|_| BluetoothPassiveScanMemoryGraphBindError::ZeroCompressedLink)?;
        let bound_link = |offset: u32| {
            let address = base
                .checked_add(offset)
                .ok_or(BluetoothPassiveScanMemoryGraphBindError::ExtentOutsidePhysicalSram)?;
            BluetoothControllerSramLinkAddress::new(address)
                .map_err(|_| BluetoothPassiveScanMemoryGraphBindError::ZeroCompressedLink)
        };
        let scheduler_context = bound_link(SCHEDULER_CONTEXT_OFFSET)?;
        let scheduler_items = [
            bound_link(SCHEDULER_ITEMS_OFFSET)?,
            bound_link(SCHEDULER_ITEMS_OFFSET + SCHEDULER_ITEM_BYTES as u32)?,
            bound_link(SCHEDULER_ITEMS_OFFSET + 2 * SCHEDULER_ITEM_BYTES as u32)?,
        ];

        let node = |index: u32| {
            let node_base = base
                .checked_add(RX_NODES_OFFSET)
                .and_then(|address| address.checked_add(index * RX_NODE_BYTES))
                .ok_or(BluetoothPassiveScanMemoryGraphBindError::ExtentOutsidePhysicalSram)?;
            let header = BluetoothControllerSramLinkAddress::new(node_base)
                .map_err(|_| BluetoothPassiveScanMemoryGraphBindError::ZeroCompressedLink)?;
            let packet = BluetoothPassiveScanRxPacketAddress::new(
                node_base
                    .checked_add(RX_PACKET_OFFSET)
                    .ok_or(BluetoothPassiveScanMemoryGraphBindError::ExtentOutsidePhysicalSram)?,
            )?;
            Ok(BluetoothPassiveScanRxNodeBinding { header, packet })
        };

        Ok(Self {
            base: base_address,
            end_exclusive,
            link_state,
            scheduler_context,
            scheduler_items,
            nodes: [node(0)?, node(1)?],
        })
    }

    const fn head(&self) -> BluetoothControllerSramAddress {
        self.nodes[0].header.controller_address()
    }

    const fn link_state(&self) -> BluetoothControllerSramAddress {
        self.link_state.controller_address()
    }

    const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.scheduler_items[BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT - 1].controller_address()
    }

    const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }
}

/// Failed static binding retaining the exact allocation unchanged.
#[must_use = "failed binding still owns the scanner memory graph"]
pub struct BluetoothPassiveScanMemoryGraphBindFailure {
    storage: &'static mut BluetoothPassiveScanMemoryGraphStorage,
    error: BluetoothPassiveScanMemoryGraphBindError,
}

impl BluetoothPassiveScanMemoryGraphBindFailure {
    /// Return the finite binding failure reason.
    pub const fn error(&self) -> BluetoothPassiveScanMemoryGraphBindError {
        self.error
    }

    /// Recover the unchanged allocation and its failure reason.
    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothPassiveScanMemoryGraphStorage,
        BluetoothPassiveScanMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothPassiveScanMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothPassiveScanMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic physical-SRAM base for native ownership tests.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPassiveScanMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothPassiveScanMemoryGraphModelAddress {
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

/// CPU-owned, initialized scanner graph not visible to hardware.
#[must_use = "the initialized scanner graph owns its static allocation"]
pub struct BluetoothPassiveScanMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
}

impl BluetoothPassiveScanMemoryGraphCpuOwned {
    /// Return the complete physical SRAM range occupied by the graph.
    pub const fn range(&self) -> (u32, u32) {
        self.binding.range()
    }

    /// Freeze CPU initialization before an upper controller owner performs the
    /// ordered MMIO publication.
    pub fn prepare_publication(self) -> BluetoothPassiveScanMemoryGraphPublicationPrepared {
        BluetoothPassiveScanMemoryGraphPublicationPrepared {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// Initialized pinned graph ready for selector-one list publication.
#[must_use = "the prepared scanner graph must be published or retained"]
pub struct BluetoothPassiveScanMemoryGraphPublicationPrepared {
    storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
}

impl BluetoothPassiveScanMemoryGraphPublicationPrepared {
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

    /// Return the private scanner link-state address for the matching
    /// scheduler-item codec. This grants no dereference or publication access.
    #[doc(hidden)]
    pub const fn link_state(&self) -> BluetoothControllerSramAddress {
        self.binding.link_state()
    }

    /// Return the validated first event item retained by the scanner graph.
    /// This grants no scheduler-list publication authority.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_head()
    }

    /// Consume a matching affine HAL publication into hardware ownership.
    #[doc(hidden)]
    pub fn into_published(
        self,
        publication: BluetoothRxMemoryListPublished,
    ) -> Result<
        BluetoothPassiveScanMemoryGraphPublished,
        BluetoothPassiveScanMemoryGraphPublicationMismatch,
    > {
        let error = if publication.selector() != self.selector() {
            Some(BluetoothPassiveScanMemoryGraphPublicationError::SelectorMismatch)
        } else if publication.head() != self.head() {
            Some(BluetoothPassiveScanMemoryGraphPublicationError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(BluetoothPassiveScanMemoryGraphPublicationMismatch {
                prepared: self,
                publication,
                error,
            });
        }
        Ok(BluetoothPassiveScanMemoryGraphPublished {
            _storage: self.storage,
            binding: self.binding,
            publication,
        })
    }
}

/// Why a HAL receive-list publication does not name this scanner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanMemoryGraphPublicationError {
    /// The publication belongs to another positional memory list.
    SelectorMismatch,
    /// The publication names another pinned arena head.
    HeadMismatch,
}

/// Failed publication join retaining both affine owners.
#[must_use = "a mismatched publication still owns both the graph and HAL token"]
pub struct BluetoothPassiveScanMemoryGraphPublicationMismatch {
    prepared: BluetoothPassiveScanMemoryGraphPublicationPrepared,
    publication: BluetoothRxMemoryListPublished,
    error: BluetoothPassiveScanMemoryGraphPublicationError,
}

impl BluetoothPassiveScanMemoryGraphPublicationMismatch {
    /// Return the finite mismatch reason.
    pub const fn error(&self) -> BluetoothPassiveScanMemoryGraphPublicationError {
        self.error
    }

    /// Recover both unchanged affine owners.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothPassiveScanMemoryGraphPublicationPrepared,
        BluetoothRxMemoryListPublished,
    ) {
        (self.prepared, self.publication)
    }
}

impl core::fmt::Debug for BluetoothPassiveScanMemoryGraphPublicationMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothPassiveScanMemoryGraphPublicationMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Pinned scanner graph visible to and exclusively retained by hardware.
///
/// No completion or CPU-reclaim method exists yet. Those transitions require
/// the next controller interrupt/fence proof and cannot be inferred from list
/// publication alone.
#[must_use = "the published scanner graph remains hardware-owned"]
pub struct BluetoothPassiveScanMemoryGraphPublished {
    _storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
    publication: BluetoothRxMemoryListPublished,
}

impl BluetoothPassiveScanMemoryGraphPublished {
    /// Return the exact retained receive-list head without exposing SRAM contents.
    pub const fn head(&self) -> BluetoothControllerSramAddress {
        self.binding.head()
    }

    /// Borrow the matching HAL publication proof.
    #[doc(hidden)]
    pub const fn publication(&self) -> &BluetoothRxMemoryListPublished {
        &self.publication
    }
}

impl BluetoothPassiveScanMemoryGraphStorage {
    /// Reserve a zero-based scanner memory graph.
    pub const fn new() -> Self {
        Self {
            link_state: BluetoothPassiveScanLinkStateStorage::new(),
            scheduler_context: BluetoothSchedulerContextStorage::new(),
            scheduler_items: [const { BluetoothPassiveScanSchedulerItemStorage::new() };
                BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT],
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
        config: BluetoothPassiveScanResetConfig,
    ) -> Result<BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanMemoryGraphBindFailure>
    {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothPassiveScanMemoryGraphBindFailure {
                    storage,
                    error: BluetoothPassiveScanMemoryGraphBindError::AddressWidth,
                });
            }
        };
        Self::pin_static_inner(storage, base, config)
    }

    /// Bind one deterministic physical-SRAM address to a native ownership
    /// model without deriving an address from the host allocation.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothPassiveScanMemoryGraphModelAddress,
        config: BluetoothPassiveScanResetConfig,
    ) -> Result<BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanMemoryGraphBindFailure>
    {
        Self::pin_static_inner(storage, base.address(), config)
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
        config: BluetoothPassiveScanResetConfig,
    ) -> Result<BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanMemoryGraphBindFailure>
    {
        let binding = match BluetoothPassiveScanMemoryGraphBinding::new(base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(BluetoothPassiveScanMemoryGraphBindFailure { storage, error });
            }
        };
        let mut owner = BluetoothPassiveScanMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.initialize(config);
        Ok(owner)
    }
}

impl BluetoothPassiveScanMemoryGraphCpuOwned {
    fn initialize(&mut self, config: BluetoothPassiveScanResetConfig) {
        let bindings = self.binding.nodes;
        let scheduler_items = self.binding.scheduler_items;
        let link_state = BluetoothPassiveScanLinkStateImage::restricted_passive_le_1m(
            BluetoothPassiveScanRxHeadProjection::from_bound(bindings[0].header),
            config,
        );
        let storage = self.storage.as_mut().project();
        storage.link_state.install(link_state);
        storage.scheduler_context.clear();
        for (index, item) in storage.scheduler_items.iter().enumerate() {
            let predecessor = index.checked_sub(1).map(|index| scheduler_items[index]);
            item.initialize_graph(
                predecessor,
                self.binding.scheduler_context,
                self.binding.link_state,
            );
        }
        storage.link_state.install_scheduler_head(
            scheduler_items[BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT - 1].controller_address(),
        );
        storage.link_state.install_receive_graph(
            bindings[0].header.controller_address(),
            bindings[1].header.controller_address(),
        );
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

impl Default for BluetoothPassiveScanMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;

    use crate::{
        BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanResetConfig,
        le_phy_packet::{BluetoothLeAccessAddress, BluetoothLeCrcInit},
        passive_scanning_event_image::BluetoothPassiveScanRxHeadProjection,
    };

    use super::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BluetoothPassiveScanMemoryGraphBindError,
        BluetoothPassiveScanMemoryGraphModelAddress, BluetoothPassiveScanMemoryGraphStorage,
    };

    fn reset_config() -> BluetoothPassiveScanResetConfig {
        BluetoothPassiveScanResetConfig::le_1m_public_accept_all(
            BluetoothPassiveScanDefaultTxPowerDbm::new(0),
            BluetoothControllerLatchedTime::from_bits(0x1234_5678),
        )
    }

    fn model_graph(base: u32) -> super::BluetoothPassiveScanMemoryGraphCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPassiveScanMemoryGraphStorage::new(),
        ));
        let address = BluetoothPassiveScanMemoryGraphModelAddress::new(base)
            .expect("the model address is controller-encodable");
        BluetoothPassiveScanMemoryGraphStorage::pin_static_model(storage, address, reset_config())
            .expect("the graph fits physical controller SRAM")
    }

    #[test]
    fn initialized_graph_contains_the_scanner_link_state_and_receive_chain() {
        let owner = model_graph(0x2f00_0100);
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
        let link_state = storage.link_state.image();
        assert!(
            link_state.retains_rx_head(BluetoothPassiveScanRxHeadProjection::from_bound(
                bindings[0].header
            ))
        );
        assert_eq!(link_state.crc_init(), BluetoothLeCrcInit::LE_PRESET);
        assert_eq!(
            link_state.access_address(),
            BluetoothLeAccessAddress::PRIMARY_ADVERTISING
        );
        assert_eq!(
            link_state.controller_time(),
            reset_config().controller_time().bits()
        );
        assert_eq!(
            storage.link_state.receive_graph(),
            (
                bindings[0].header.controller_address(),
                bindings[1].header.controller_address(),
                None,
            )
        );
        assert_eq!(
            storage.link_state.scheduler_head(),
            owner.binding.scheduler_head()
        );
        assert!(
            storage
                .scheduler_items
                .iter()
                .enumerate()
                .all(|(index, item)| item.retains_graph(
                    index
                        .checked_sub(1)
                        .map(|index| owner.binding.scheduler_items[index]),
                    owner.binding.scheduler_context,
                    owner.binding.link_state,
                ))
        );

        let link_state_address = owner.binding.link_state();
        let scheduler_head = owner.binding.scheduler_head();
        let prepared = owner.prepare_publication();
        assert_eq!(prepared.head(), bindings[0].header.controller_address());
        assert_eq!(prepared.link_state(), link_state_address);
        assert_eq!(prepared.scheduler_head(), scheduler_head);
        assert_eq!(
            prepared.selector(),
            crate::BluetoothRxMemoryListClass::Scanning.selector()
        );
    }

    #[test]
    fn failed_extent_binding_retains_the_exact_static_allocation() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPassiveScanMemoryGraphStorage::new(),
        ));
        let identity = core::ptr::addr_of!(*storage).addr();
        let address = BluetoothPassiveScanMemoryGraphModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 4,
        )
        .expect("the aligned address remains controller-encodable");

        let failure = match BluetoothPassiveScanMemoryGraphStorage::pin_static_model(
            storage,
            address,
            reset_config(),
        ) {
            Ok(_) => panic!("the complete graph must not cross physical SRAM"),
            Err(failure) => failure,
        };

        assert_eq!(
            failure.error(),
            BluetoothPassiveScanMemoryGraphBindError::ExtentOutsidePhysicalSram
        );
        let (storage, _) = failure.into_parts();
        assert_eq!(core::ptr::addr_of!(*storage).addr(), identity);
    }
}
