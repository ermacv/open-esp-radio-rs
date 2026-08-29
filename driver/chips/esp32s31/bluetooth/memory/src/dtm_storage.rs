//! Static CPU-owned storage for the recovered ESP32-S31 DTM memory graph.
//!
//! The types in this module reserve the complete finite per-event link-graph
//! footprint and reproduce only reviewed CPU-side initialization transforms.
//! The separate `0x28`-byte DTM environment remains LLL state above this
//! boundary. A target-only binding derives the real addresses of one static
//! allocation, rejects storage outside physical internal SRAM and retains the
//! allocation behind one movable CPU owner. It exposes no controller
//! publication or hardware-owned state. Four-byte alignment is the minimum
//! proven by compressed controller links; it is not a cache-coherency claim.

#![forbid(unsafe_code)]

use core::{convert::Infallible, marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use pin_project::pin_project;

use crate::{
    dtm_event_image::{
        BluetoothDtmLinkStateReviewedWords, BluetoothDtmPositionalEventWords,
        BluetoothDtmSchedulerItemReviewedWords,
    },
    sram_link::BluetoothDtmBoundSramLinkAddress,
};

/// Bytes allocated for one DTM link-state object.
pub const BLUETOOTH_DTM_LINK_STATE_BYTES: usize = 0x84;
/// Bytes allocated for one DTM scheduler item.
pub const BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Bytes allocated for the separate DTM scheduler context.
pub const BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES: usize = 0x48;
/// Bytes in every common RX/TX buffer header.
pub const BLUETOOTH_DTM_BUFFER_HEADER_BYTES: usize = 0x18;
/// Bytes preceding the maximum DTM transmitter payload.
pub const BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES: usize = 0x12;
/// Bytes preceding the maximum DTM receiver capacity.
pub const BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES: usize = 0x1e;
/// Maximum packet capacity supplied by the complete DTM allocator.
pub const BLUETOOTH_DTM_MAX_PACKET_CAPACITY: usize = u8::MAX as usize;
/// Logical bytes in the complete DTM TX packet allocation.
pub const BLUETOOTH_DTM_TX_PACKET_BYTES: usize =
    BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;
/// Logical bytes in the complete DTM RX packet allocation.
pub const BLUETOOTH_DTM_RX_PACKET_BYTES: usize =
    BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;

/// First byte in the physical ESP32-S31 internal SRAM window.
pub const BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW: u32 = 0x2f00_0000;
/// First byte after the physical ESP32-S31 internal SRAM window.
///
/// This is deliberately narrower than the controller's 20-bit compressed
/// pointer encoding domain. Linker policy may reserve an additional suffix
/// below this architectural boundary and must place the static allocation in
/// its available `.dma.bss` range.
pub const BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH: u32 = 0x2f08_0000;

const RX_PACKET_LAST_ALIGNED_OFFSET: u32 = 0x11c;
const TX_PACKET_LAST_ALIGNED_OFFSET: u32 = 0x110;
const TX_HEADER_PACKET_TARGET_OFFSET: u32 = 0x10;
const LINK_STATE_RX_HEAD_OFFSET: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD_OFFSET: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL_OFFSET: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL_OFFSET: usize = 0x74 / 4;
const LINK_STATE_RX_SWAP_RESERVE_OFFSET: usize = 0x78 / 4;
const LINK_STATE_ALLOCATION_CONFIG_OFFSET: usize = 0x30 / 4;
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: u32 = 0x0000_1e00;
const SCHEDULER_ITEM_ALLOCATION_PREFIX_OFFSET: usize = 0;
const SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE: u32 = 0x0030_0000;
const SCHEDULER_ITEM_CONTEXT_OFFSET: usize = 1;
const SCHEDULER_ITEM_LINK_STATE_OFFSET: usize = 0x08 / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_OFFSET: usize = 0x1c / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: u32 = 0xffdf_ffff;
const SCHEDULER_ITEM_ALLOCATION_CONFIG_OFFSET: usize = 0x20 / 4;
const SCHEDULER_ITEM_POSITIONAL_24_OFFSET: usize = 0x24 / 4;
const SCHEDULER_ITEM_POSITIONAL_24_IMAGE: u32 = 0x0007_bdef;
const SCHEDULER_ITEM_STATUS_OFFSET: usize = 0x38 / 4;
const SCHEDULER_ITEM_CONTROL_OFFSET: usize = 0x4c / 4;
const SCHEDULER_ITEM_CONTROL_BYTE: usize = 2;
const SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET: usize = 0;
const SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET: usize = 0x50 / 4;
const SCHEDULER_ITEM_COMPLETED_LINK_OFFSET: usize = 0x54 / 4;
const LOW_TWENTY_MASK: u32 = 0x000f_ffff;

/// Source-owned inputs to the DTM scheduler allocation field.
///
/// The current allocator forms scheduler-item `+0x20[11:0]` from three public
/// ESP-IDF controller limits, two fixed additions and one private-options
/// halfword. The private field remains positional because its semantic name is
/// not yet proven. Requiring it here prevents a target from silently inheriting
/// a value from one reviewed vendor build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerAllocationConfig {
    extended_advertising_instances: u16,
    connections: u16,
    private_options_halfword_14: u16,
    periodic_syncs: u8,
}

impl BluetoothDtmSchedulerAllocationConfig {
    /// Capture the exact source-owned inputs used by the S31 DTM allocator.
    pub const fn new(
        extended_advertising_instances: u16,
        connections: u16,
        private_options_halfword_14: u16,
        periodic_syncs: u8,
    ) -> Self {
        Self {
            extended_advertising_instances,
            connections,
            private_options_halfword_14,
            periodic_syncs,
        }
    }

    const fn allocation_image(self) -> u32 {
        ((self.extended_advertising_instances as u32)
            .wrapping_add(1)
            .wrapping_add(self.connections as u32)
            .wrapping_add(self.private_options_halfword_14 as u32)
            .wrapping_add(4)
            .wrapping_add(self.periodic_syncs as u32))
            & 0x0fff
    }
}

/// Why a complete DTM TX packet extent cannot inhabit controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmTxPacketAddressError {
    /// The proposed base is not a reviewed compressed controller-SRAM address.
    InvalidBase(BluetoothControllerSramAddressError),
    /// The base encodes as the zero link used for an unbound header.
    ZeroCompressedBase,
    /// The aligned packet tail crosses the reviewed controller-SRAM window.
    ExtentOutsideControllerSram,
}

/// Validated controller-SRAM geometry for one complete DTM TX packet.
///
/// This value proves only address range and alignment. It does not derive an
/// address from Rust storage, dereference it or grant publication authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxPacketAddress {
    base: BluetoothControllerSramAddress,
    header_packet_target: BluetoothControllerSramAddress,
}

impl BluetoothDtmTxPacketAddress {
    /// Validate the base, header target and final aligned word of the
    /// `0x111`-byte allocation.
    pub const fn new(address: u32) -> Result<Self, BluetoothDtmTxPacketAddressError> {
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(BluetoothDtmTxPacketAddressError::InvalidBase(error)),
        };
        if base.compressed_image() == 0 {
            return Err(BluetoothDtmTxPacketAddressError::ZeroCompressedBase);
        }
        let header_packet_target_address = match address.checked_add(TX_HEADER_PACKET_TARGET_OFFSET)
        {
            Some(address) => address,
            None => return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let last_aligned_address = match address.checked_add(TX_PACKET_LAST_ALIGNED_OFFSET) {
            Some(address) => address,
            None => return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let header_packet_target =
            match BluetoothControllerSramAddress::new(header_packet_target_address) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram);
                }
            };
        if BluetoothControllerSramAddress::new(last_aligned_address).is_err() {
            return Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram);
        }
        Ok(Self {
            base,
            header_packet_target,
        })
    }

    const fn base_compressed_image(self) -> u32 {
        self.base.compressed_image()
    }

    const fn header_packet_target_compressed_image(self) -> u32 {
        self.header_packet_target.compressed_image()
    }
}

/// Complete six-word allocation-time image of one DTM TX buffer header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxBufferHeaderImage {
    words: [u32; 6],
}

impl BluetoothDtmTxBufferHeaderImage {
    /// Build the exact header image used by the full-capacity DTM allocation.
    pub const fn new(packet: BluetoothDtmTxPacketAddress) -> Self {
        Self {
            words: [
                0,
                packet.base_compressed_image(),
                0x80a0_0000 | packet.header_packet_target_compressed_image(),
                0,
                0x0000_07f8,
                0,
            ],
        }
    }

    /// Return all six little-endian positional words without publication.
    pub const fn words(self) -> [u32; 6] {
        self.words
    }
}

/// Why a complete DTM RX packet extent cannot inhabit controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRxPacketAddressError {
    /// The proposed base is not a reviewed compressed controller-SRAM address.
    InvalidBase(BluetoothControllerSramAddressError),
    /// The base encodes as the zero link used by the unbound swap reserve.
    ZeroCompressedBase,
    /// The aligned packet tail crosses the reviewed controller-SRAM window.
    ExtentOutsideControllerSram,
}

/// Validated controller-SRAM geometry for one complete DTM RX packet.
///
/// This value proves only address range and alignment. It does not derive an
/// address from Rust storage, dereference it or grant publication authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxPacketAddress {
    base: BluetoothControllerSramAddress,
}

impl BluetoothDtmRxPacketAddress {
    /// Validate the base and final aligned word of the `0x11d`-byte allocation.
    pub const fn new(address: u32) -> Result<Self, BluetoothDtmRxPacketAddressError> {
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(BluetoothDtmRxPacketAddressError::InvalidBase(error)),
        };
        if base.compressed_image() == 0 {
            return Err(BluetoothDtmRxPacketAddressError::ZeroCompressedBase);
        }
        let last_aligned_address = match address.checked_add(RX_PACKET_LAST_ALIGNED_OFFSET) {
            Some(address) => address,
            None => return Err(BluetoothDtmRxPacketAddressError::ExtentOutsideControllerSram),
        };
        if BluetoothControllerSramAddress::new(last_aligned_address).is_err() {
            return Err(BluetoothDtmRxPacketAddressError::ExtentOutsideControllerSram);
        }
        Ok(Self { base })
    }

    const fn compressed_image(self) -> u32 {
        self.base.compressed_image()
    }
}

/// Complete zero-based allocation-time image of the bound DTM RX header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxBufferHeaderImage {
    words: [u32; 6],
}

impl BluetoothDtmRxBufferHeaderImage {
    /// Bind the RX packet while preserving the exact positional high image.
    pub const fn new(packet: BluetoothDtmRxPacketAddress) -> Self {
        Self {
            words: [0, packet.compressed_image(), 0x8080_0000, 0, 0, 0],
        }
    }

    /// Return all six little-endian positional words without publication.
    pub const fn words(self) -> [u32; 6] {
        self.words
    }

    /// Initial image of the separately allocated software swap reserve.
    pub const fn unbound_swap_reserve() -> Self {
        Self {
            words: [0, 0, 0x8080_0000, 0, 0, 0],
        }
    }
}

/// Opaque CPU-owned link-state allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmLinkStateStorage {
    words: [u32; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
}

impl BluetoothDtmLinkStateStorage {
    fn reviewed_words(&self) -> BluetoothDtmLinkStateReviewedWords {
        BluetoothDtmLinkStateReviewedWords {
            word_00: self.words[0],
            word_04: self.words[1],
            word_08: self.words[2],
            word_14: self.words[5],
            word_2c: self.words[11],
            word_34: self.words[13],
            word_38: self.words[14],
            word_50: self.words[20],
        }
    }

    fn write_reviewed_words(&mut self, words: BluetoothDtmLinkStateReviewedWords) {
        self.words[0] = words.word_00;
        self.words[1] = words.word_04;
        self.words[2] = words.word_08;
        self.words[5] = words.word_14;
        self.words[11] = words.word_2c;
        self.words[13] = words.word_34;
        self.words[14] = words.word_38;
        self.words[20] = words.word_50;
    }
}

/// Opaque CPU-owned scheduler-item allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmSchedulerItemStorage {
    words: [u32; BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4],
}

impl BluetoothDtmSchedulerItemStorage {
    fn reviewed_words(&self) -> BluetoothDtmSchedulerItemReviewedWords {
        BluetoothDtmSchedulerItemReviewedWords {
            word_00: self.words[0],
            word_04: self.words[1],
            word_08: self.words[2],
            word_0c: self.words[3],
            word_10: self.words[4],
            word_14: self.words[5],
            word_18: self.words[6],
            word_2c: self.words[11],
            word_44: self.words[17],
            word_48: self.words[18],
            word_4c: self.words[19],
        }
    }

    fn write_reviewed_words(&mut self, words: BluetoothDtmSchedulerItemReviewedWords) {
        self.words[0] = words.word_00;
        self.words[1] = words.word_04;
        self.words[2] = words.word_08;
        self.words[3] = words.word_0c;
        self.words[4] = words.word_10;
        self.words[5] = words.word_14;
        self.words[6] = words.word_18;
        self.words[11] = words.word_2c;
        self.words[17] = words.word_44;
        self.words[18] = words.word_48;
        self.words[19] = words.word_4c;
    }
}

/// Opaque CPU-owned scheduler-context allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmSchedulerContextStorage {
    words: [u32; BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES / 4],
}

/// One zero-based common buffer-header allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmBufferHeaderStorage {
    words: [u32; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
}

impl BluetoothDtmBufferHeaderStorage {
    fn clear_rx_completion_bit(&mut self) {
        self.words[3] &= !0x8000_0000;
    }
}

/// CPU-owned TX packet slot reserved by the complete DTM link graph.
///
/// This is the sole backing-storage type used by both the memory graph and the
/// LLL packet-pattern preparation. It exposes no raw address or publication
/// operation.
#[repr(C, align(4))]
pub struct BluetoothDtmTxPacketStorage {
    bytes: [u8; BLUETOOTH_DTM_TX_PACKET_BYTES],
}

impl BluetoothDtmTxPacketStorage {
    /// Create zeroed, CPU-owned storage without allocating.
    pub const fn new() -> Self {
        Self {
            bytes: [0; BLUETOOTH_DTM_TX_PACKET_BYTES],
        }
    }

    /// Install the four positional DTM prefix bytes and exclusively borrow the
    /// declared payload for an upper-layer pattern generator.
    pub fn begin_prepare(
        &mut self,
        pattern_selector: u8,
        payload_length: u8,
    ) -> BluetoothDtmTxPacketPreparation<'_> {
        self.bytes[0x05] = 2;
        self.bytes[0x06] = 0;
        self.bytes[0x10] = pattern_selector;
        self.bytes[0x11] = payload_length;

        BluetoothDtmTxPacketPreparation {
            payload_length: payload_length as usize,
            storage: self,
        }
    }

    /// Borrow the complete CPU-owned allocation image for review and tests.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Default for BluetoothDtmTxPacketStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Exclusive CPU-owned TX packet preparation in progress.
///
/// The memory layer deliberately treats the selector and payload bytes as
/// positional. Standard DTM pattern semantics remain in the LLL/portable
/// Controller above this type.
pub struct BluetoothDtmTxPacketPreparation<'storage> {
    payload_length: usize,
    storage: &'storage mut BluetoothDtmTxPacketStorage,
}

impl<'storage> BluetoothDtmTxPacketPreparation<'storage> {
    /// Borrow exactly the declared payload bytes for bounded pattern filling.
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.storage.bytes[BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES..][..self.payload_length]
    }

    /// Finish the CPU-only transform without publishing the packet.
    pub fn finish(self) -> BluetoothDtmPreparedTxPacketStorage<'storage> {
        BluetoothDtmPreparedTxPacketStorage {
            payload_length: self.payload_length,
            storage: self.storage,
        }
    }
}

/// Affine CPU-owned view of one positionally prepared TX packet slot.
pub struct BluetoothDtmPreparedTxPacketStorage<'storage> {
    payload_length: usize,
    storage: &'storage mut BluetoothDtmTxPacketStorage,
}

impl<'storage> BluetoothDtmPreparedTxPacketStorage<'storage> {
    /// Borrow the reviewed prefix and declared payload only.
    pub fn prepared_bytes(&self) -> &[u8] {
        &self.storage.bytes[..BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES + self.payload_length]
    }

    /// Borrow only the declared payload bytes.
    pub fn payload(&self) -> &[u8] {
        &self.storage.bytes[BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES..][..self.payload_length]
    }

    /// Return the sole CPU-owned backing slot.
    pub fn release(self) -> &'storage mut BluetoothDtmTxPacketStorage {
        self.storage
    }
}

/// CPU-owned RX packet allocation with only the reviewed DTM defaults exposed.
#[repr(C, align(4))]
pub struct BluetoothDtmRxPacketStorage {
    bytes: [u8; BLUETOOTH_DTM_RX_PACKET_BYTES],
}

impl BluetoothDtmRxPacketStorage {
    /// Create one zero-based slot and apply the reviewed maximum-capacity
    /// allocation and re-arm images.
    pub const fn new() -> Self {
        let mut storage = Self {
            bytes: [0; BLUETOOTH_DTM_RX_PACKET_BYTES],
        };
        storage.bytes[0x05] = 1;
        storage.bytes[0x06] = 1;
        storage.bytes[0x0c] = 0xff;
        storage.bytes[0x0d] = 0xff;
        storage.bytes[0x0e] = 0xff;
        storage.bytes[0x18] = 0xff;
        storage.bytes[0x19] = 0xff;
        storage
    }

    /// Reapply the exact returned-buffer default values before a future append.
    ///
    /// The low three bytes of word `+0x0c` become `0xff`; byte `+0x0f` is
    /// deliberately retained. Halfword `+0x18` becomes `0xffff`.
    fn rearm_reviewed_packet_fields(&mut self) {
        self.bytes[0x0c] = 0xff;
        self.bytes[0x0d] = 0xff;
        self.bytes[0x0e] = 0xff;
        self.bytes[0x18] = 0xff;
        self.bytes[0x19] = 0xff;
    }

    /// Read the positional result word without assigning field semantics.
    pub fn result_word(&self) -> u32 {
        u32::from_le_bytes([
            self.bytes[0x0c],
            self.bytes[0x0d],
            self.bytes[0x0e],
            self.bytes[0x0f],
        ])
    }

    /// Borrow the CPU-owned allocation for reviewed preparation and tests.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Exclusive CPU-owned view of the ordinary RX packet and header allocations.
///
/// The append path mutates both allocations as one transition, so the public
/// memory API does not expose packet re-arm separately from clearing the
/// returned header's completion bit.
pub struct BluetoothDtmRxBufferStorage<'storage> {
    packet: &'storage mut BluetoothDtmRxPacketStorage,
    header: &'storage mut BluetoothDtmBufferHeaderStorage,
}

/// Why the ordinary returned-buffer re-arm must stop without mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRxRearmError {
    /// Header bit `+0x10.0` selects the unresolved swap-reserve branch.
    SwapReserveDecisionRequired,
}

impl BluetoothDtmRxBufferStorage<'_> {
    /// Apply the complete ordinary returned-buffer re-arm before append.
    ///
    /// This covers the packet sentinels and completion bit. The special header
    /// `+0x10.0` swap-reserve branch fails closed before either allocation is
    /// mutated.
    pub fn rearm_reviewed_fields(&mut self) -> Result<(), BluetoothDtmRxRearmError> {
        if self.header.words[4] & 1 != 0 {
            return Err(BluetoothDtmRxRearmError::SwapReserveDecisionRequired);
        }
        self.packet.rearm_reviewed_packet_fields();
        self.header.clear_rx_completion_bit();
        Ok(())
    }

    /// Read the positional packet result word before re-arm.
    pub fn result_word(&self) -> u32 {
        self.packet.result_word()
    }
}

impl Default for BluetoothDtmRxPacketStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete no-heap allocation capacity for one reviewed DTM per-event graph.
///
/// Fields are private because the link relationships and publishable images
/// belong to an address-bound owner. In particular, this value has no method
/// that publishes a raw address or creates a `HardwareOwned` state.
#[pin_project]
#[repr(C)]
pub struct BluetoothDtmMemoryGraphStorage {
    link_state: BluetoothDtmLinkStateStorage,
    scheduler_context: BluetoothDtmSchedulerContextStorage,
    scheduler_item: BluetoothDtmSchedulerItemStorage,
    rx_header: BluetoothDtmBufferHeaderStorage,
    rx_swap_reserve: BluetoothDtmBufferHeaderStorage,
    tx_header: BluetoothDtmBufferHeaderStorage,
    tx_packet: BluetoothDtmTxPacketStorage,
    rx_packet: BluetoothDtmRxPacketStorage,
    #[pin]
    _pin: PhantomPinned,
}

const BLUETOOTH_DTM_MEMORY_GRAPH_BYTES: u32 = 0x3a8;
const LINK_STATE_STORAGE_OFFSET: u32 = 0x000;
const SCHEDULER_CONTEXT_STORAGE_OFFSET: u32 = 0x084;
const SCHEDULER_ITEM_STORAGE_OFFSET: u32 = 0x0cc;
const RX_HEADER_STORAGE_OFFSET: u32 = 0x12c;
const RX_SWAP_RESERVE_STORAGE_OFFSET: u32 = 0x144;
const TX_HEADER_STORAGE_OFFSET: u32 = 0x15c;
const TX_PACKET_STORAGE_OFFSET: u32 = 0x174;
const RX_PACKET_STORAGE_OFFSET: u32 = 0x288;

const _: () = {
    assert!(core::mem::size_of::<BluetoothDtmMemoryGraphStorage>() == 0x3a8);
    assert!(core::mem::align_of::<BluetoothDtmMemoryGraphStorage>() == 4);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, link_state) == 0x000);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_context) == 0x084);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_item) == 0x0cc);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_header) == 0x12c);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_swap_reserve) == 0x144);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, tx_header) == 0x15c);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, tx_packet) == 0x174);
    assert!(core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_packet) == 0x288);
};

/// Why static DTM graph storage cannot become an address-bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphBindError {
    /// A target pointer cannot be represented by the 32-bit S31 address space.
    AddressWidth,
    /// The proposed base is outside the compressed controller-pointer domain.
    InvalidBase(BluetoothControllerSramAddressError),
    /// Some byte of the complete graph is outside physical internal SRAM.
    ExtentOutsidePhysicalSram,
    /// A required graph link would encode as the unbound zero image.
    ZeroCompressedLink,
    /// A reviewed packet extent is inconsistent with the bound graph layout.
    InvalidPacketExtent,
}

/// Failed static binding that retains the exact, unchanged allocation.
///
/// Address validation completes before any allocation-time field is written.
/// The failure is linear and cannot be duplicated:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphBindFailure;
///
/// fn duplicate(failure: BluetoothDtmMemoryGraphBindFailure) {
///     let moved = failure;
///     let _ = failure.error();
///     drop(moved);
/// }
/// ```
pub struct BluetoothDtmMemoryGraphBindFailure {
    storage: &'static mut BluetoothDtmMemoryGraphStorage,
    error: BluetoothDtmMemoryGraphBindError,
}

impl BluetoothDtmMemoryGraphBindFailure {
    fn new(
        storage: &'static mut BluetoothDtmMemoryGraphStorage,
        error: BluetoothDtmMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    /// Return the finite reason without releasing the allocation.
    pub const fn error(&self) -> BluetoothDtmMemoryGraphBindError {
        self.error
    }

    /// Recover the unchanged static allocation and binding error.
    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothDtmMemoryGraphStorage,
        BluetoothDtmMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
///
/// Construction proves the compressed-pointer syntax only. The graph binding
/// still validates the complete extent against physical S31 SRAM. Production
/// RISC-V code has no access to this type and must derive real field addresses
/// from its static allocation.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothDtmMemoryGraphModelAddress {
    /// Validate one synthetic base against the controller encoding domain.
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

/// Non-forgeable address binding retained by one CPU-owned static graph.
///
/// The value proves that every contained allocation lies in physical internal
/// SRAM and matches this crate's exact `repr(C)` layout. It is intentionally
/// not `Clone` or `Copy`; obtaining compressed component addresses does not
/// grant controller publication authority.
pub struct BluetoothDtmMemoryGraphBinding {
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    link_state: BluetoothDtmBoundSramLinkAddress,
    scheduler_context: BluetoothControllerSramAddress,
    scheduler_item: BluetoothDtmBoundSramLinkAddress,
    rx_header: BluetoothDtmBoundSramLinkAddress,
    rx_swap_reserve: BluetoothDtmBoundSramLinkAddress,
    tx_header: BluetoothDtmBoundSramLinkAddress,
    tx_packet: BluetoothDtmTxPacketAddress,
    rx_packet: BluetoothDtmRxPacketAddress,
}

impl BluetoothDtmMemoryGraphBinding {
    fn new(base: u32) -> Result<Self, BluetoothDtmMemoryGraphBindError> {
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothDtmMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || BLUETOOTH_DTM_MEMORY_GRAPH_BYTES
                > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram);
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let bound_link = |offset: u32| {
            BluetoothDtmBoundSramLinkAddress::new(address(offset)?)
                .map_err(|_| BluetoothDtmMemoryGraphBindError::ZeroCompressedLink)
        };

        let link_state = bound_link(LINK_STATE_STORAGE_OFFSET)?;
        let scheduler_context =
            BluetoothControllerSramAddress::new(address(SCHEDULER_CONTEXT_STORAGE_OFFSET)?)
                .map_err(BluetoothDtmMemoryGraphBindError::InvalidBase)?;
        let scheduler_item = bound_link(SCHEDULER_ITEM_STORAGE_OFFSET)?;
        let rx_header = bound_link(RX_HEADER_STORAGE_OFFSET)?;
        let rx_swap_reserve = bound_link(RX_SWAP_RESERVE_STORAGE_OFFSET)?;
        let tx_header = bound_link(TX_HEADER_STORAGE_OFFSET)?;
        let tx_packet = BluetoothDtmTxPacketAddress::new(address(TX_PACKET_STORAGE_OFFSET)?)
            .map_err(|_| BluetoothDtmMemoryGraphBindError::InvalidPacketExtent)?;
        let rx_packet = BluetoothDtmRxPacketAddress::new(address(RX_PACKET_STORAGE_OFFSET)?)
            .map_err(|_| BluetoothDtmMemoryGraphBindError::InvalidPacketExtent)?;

        Ok(Self {
            base: base_address,
            end_exclusive: base + BLUETOOTH_DTM_MEMORY_GRAPH_BYTES,
            link_state,
            scheduler_context,
            scheduler_item,
            rx_header,
            rx_swap_reserve,
            tx_header,
            tx_packet,
            rx_packet,
        })
    }

    /// Return the complete physical SRAM range occupied by this graph.
    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    /// Return the address of the private DTM link-state allocation.
    pub const fn link_state_address(&self) -> BluetoothDtmBoundSramLinkAddress {
        self.link_state
    }

    /// Return the address of the separate CPU-owned scheduler context.
    pub const fn scheduler_context_address(&self) -> BluetoothControllerSramAddress {
        self.scheduler_context
    }

    /// Return the address of the private DTM scheduler item.
    pub const fn scheduler_item_address(&self) -> BluetoothDtmBoundSramLinkAddress {
        self.scheduler_item
    }
}

/// Snapshot supplied to one in-place positional event-word builder.
///
/// Both private links are sampled from this graph's current link-state words,
/// not reconstructed from an earlier binding or another graph. The builder is
/// invoked while the unique graph owner is consumed, so its output cannot be
/// applied later to a different owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmPositionalEventSeed {
    words: BluetoothDtmPositionalEventWords,
    tx_head: BluetoothDtmBoundSramLinkAddress,
    rx_tail: BluetoothDtmBoundSramLinkAddress,
}

impl BluetoothDtmPositionalEventSeed {
    /// Return the current values of exactly the nineteen writable words.
    pub const fn words(self) -> BluetoothDtmPositionalEventWords {
        self.words
    }

    /// Return the current private TX-head link sampled from link-state `+0x6c`.
    pub const fn tx_head(self) -> BluetoothDtmBoundSramLinkAddress {
        self.tx_head
    }

    /// Return the current private RX-tail link sampled from link-state `+0x70`.
    pub const fn rx_tail(self) -> BluetoothDtmBoundSramLinkAddress {
        self.rx_tail
    }
}

/// Why CPU-owned positional event words were not committed to the graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphPrepareError<BuildError = Infallible> {
    /// The upper builder rejected its semantic inputs before any graph write.
    Build(BuildError),
    /// The current private TX-head word contains the unbound zero link.
    CurrentTxHeadUnbound,
    /// The current private RX-tail word contains the unbound zero link.
    CurrentRxTailUnbound,
    /// Link-state `+0x00` does not retain this graph's freshly sampled TX head.
    LinkStateTxHeadMismatch {
        /// Current private-chain image required by this graph.
        expected: u32,
        /// Candidate low-twenty-bit image returned by the builder.
        observed: u32,
    },
    /// Link-state `+0x08` does not retain this graph's freshly sampled RX tail.
    LinkStateRxTailMismatch {
        /// Current private-chain image required by this graph.
        expected: u32,
        /// Candidate low-twenty-bit image returned by the builder.
        observed: u32,
    },
    /// Scheduler-item `+0x08` no longer points to this graph's link-state.
    SchedulerItemLinkStateMismatch {
        /// Statically bound link-state image required by this graph.
        expected: u32,
        /// Candidate low-twenty-bit image returned by the builder.
        observed: u32,
    },
}

/// Failed positional preparation retaining the exact CPU-owned graph.
///
/// Current anchors, builder execution and all three candidate anchors are
/// checked before the first backing-storage write. `into_parts` therefore
/// returns the byte-unchanged owner for retry or explicit shutdown.
pub struct BluetoothDtmMemoryGraphPrepareFailure<BuildError = Infallible> {
    owner: BluetoothDtmMemoryGraphCpuOwned,
    error: BluetoothDtmMemoryGraphPrepareError<BuildError>,
}

impl<BuildError> BluetoothDtmMemoryGraphPrepareFailure<BuildError> {
    /// Borrow the finite failure reason without releasing the owner.
    pub const fn error(&self) -> &BluetoothDtmMemoryGraphPrepareError<BuildError> {
        &self.error
    }

    /// Recover the unchanged owner and the exact builder or anchor error.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmMemoryGraphPrepareError<BuildError>,
    ) {
        (self.owner, self.error)
    }
}

impl<BuildError: core::fmt::Debug> core::fmt::Debug
    for BluetoothDtmMemoryGraphPrepareFailure<BuildError>
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// CPU-owned graph after exactly nineteen positional event words were stored.
///
/// The public input images remain forgeable, so this state proves only that
/// the candidate retained this graph's three current links and that no omitted
/// offset was written. It has no packet-ready, list insertion, fence,
/// publication or hardware-ownership authority.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphPositionalEventPrepared;
///
/// fn cannot_mutate_packet(prepared: &mut BluetoothDtmMemoryGraphPositionalEventPrepared) {
///     let _packet = prepared.tx_packet_mut();
/// }
/// ```
pub struct BluetoothDtmMemoryGraphPositionalEventPrepared {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    previous: BluetoothDtmPositionalEventWords,
}

impl BluetoothDtmMemoryGraphPositionalEventPrepared {
    /// Return the typed identity of this graph's prepared scheduler item.
    ///
    /// The returned address does not publish the item or change graph
    /// ownership. The upper scheduler must retain this consumed graph while a
    /// request carrying the identity is admitted.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Read back exactly the positional subset while it remains CPU-owned.
    pub fn words(&self) -> BluetoothDtmPositionalEventWords {
        let storage = self.storage.as_ref().get_ref();
        BluetoothDtmPositionalEventWords::new(
            storage.link_state.reviewed_words(),
            storage.scheduler_item.reviewed_words(),
        )
    }

    /// Prepare the scheduler-owned bookkeeping fields that precede insertion.
    ///
    /// This consumes the event-word owner, clears the reviewed control byte
    /// and software completed-item link, and installs the in-flight status
    /// sentinel. The returned graph remains CPU-owned: the complete
    /// hardware-consumed descriptor, release fence, private packet-engine
    /// latch and scheduler-head publication are still separate prerequisites.
    pub fn prepare_scheduler_bookkeeping(
        mut self,
    ) -> BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
        let storage = self.storage.as_mut().project().scheduler_item;
        let previous_control = storage.words[SCHEDULER_ITEM_CONTROL_OFFSET];
        let previous_status = storage.words[SCHEDULER_ITEM_STATUS_OFFSET];
        let previous_completed_link = storage.words[SCHEDULER_ITEM_COMPLETED_LINK_OFFSET];

        let mut control = previous_control.to_le_bytes();
        control[SCHEDULER_ITEM_CONTROL_BYTE] = 0;
        storage.words[SCHEDULER_ITEM_CONTROL_OFFSET] = u32::from_le_bytes(control);
        storage.words[SCHEDULER_ITEM_STATUS_OFFSET] = u32::MAX;
        storage.words[SCHEDULER_ITEM_COMPLETED_LINK_OFFSET] = 0;

        BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
            previous_control,
            previous_status,
            previous_completed_link,
        }
    }

    /// Cancel before publication and restore all nineteen prior words.
    ///
    /// This restoration is complete because this state never exposed mutable
    /// storage and the preparing transition wrote no other graph offset.
    pub fn cancel(mut self) -> BluetoothDtmMemoryGraphCpuOwned {
        let previous = self.previous;
        let storage = self.storage.as_mut().project();
        storage
            .link_state
            .write_reviewed_words(previous.link_state());
        storage
            .scheduler_item
            .write_reviewed_words(previous.scheduler_item());

        BluetoothDtmMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// CPU-owned graph with the common scheduler bookkeeping prefix installed.
///
/// This state is deliberately not called published or hardware-owned. It has
/// no operation that exposes packet storage or permits the controller to
/// consume the graph before the remaining descriptor and visibility contract
/// is proven.
#[must_use = "the scheduler-prepared graph must remain owned or be cancelled"]
pub struct BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    previous: BluetoothDtmPositionalEventWords,
    previous_control: u32,
    previous_status: u32,
    previous_completed_link: u32,
}

impl BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
    /// Return the typed identity of this graph's scheduler item.
    ///
    /// This value still carries no publication or CPU-access authority.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Prepare this item as the sole member of a proven-empty hardware list.
    ///
    /// SOURCE: complete current `r_sym_bt_YRnBzKlWCjsIbotqvNyS` and
    /// instruction-corresponding same-chip named
    /// `r_btdm_sched_merge_list_remove_overlap`. On the empty-list edge the
    /// selected item is the submitted item and its compressed hardware-next
    /// link is cleared before any hardware-head publication.
    ///
    /// This memory-only transition does not prove that a scheduler list is
    /// empty and performs no fence or MMIO. The Controller must consume it
    /// together with its exclusive empty-list epoch before publication.
    #[doc(hidden)]
    pub fn prepare_empty_list_link(mut self) -> BluetoothDtmMemoryGraphEmptyListLinkPrepared {
        let storage = self.storage.as_mut().project().scheduler_item;
        let previous_hardware_next = storage.words[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET];
        let previous_software_next = storage.words[SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET];
        storage.words[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET] =
            previous_hardware_next & !LOW_TWENTY_MASK;
        storage.words[SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET] = 0;

        BluetoothDtmMemoryGraphEmptyListLinkPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
            previous_control: self.previous_control,
            previous_status: self.previous_status,
            previous_completed_link: self.previous_completed_link,
            previous_hardware_next,
            previous_software_next,
        }
    }

    /// Cancel before publication and recover the positional event owner.
    pub fn cancel(mut self) -> BluetoothDtmMemoryGraphPositionalEventPrepared {
        let storage = self.storage.as_mut().project().scheduler_item;
        storage.words[SCHEDULER_ITEM_CONTROL_OFFSET] = self.previous_control;
        storage.words[SCHEDULER_ITEM_STATUS_OFFSET] = self.previous_status;
        storage.words[SCHEDULER_ITEM_COMPLETED_LINK_OFFSET] = self.previous_completed_link;

        BluetoothDtmMemoryGraphPositionalEventPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
        }
    }
}

/// CPU-owned graph whose scheduler item has a null hardware-next link.
///
/// This is only descriptor preparation for the empty-list merge case. It does
/// not carry list ownership, visibility, publication or hardware-consumption
/// authority; those belong to the upper Controller lifecycle.
#[must_use = "the empty-list candidate must remain owned or be cancelled"]
pub struct BluetoothDtmMemoryGraphEmptyListLinkPrepared {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    previous: BluetoothDtmPositionalEventWords,
    previous_control: u32,
    previous_status: u32,
    previous_completed_link: u32,
    previous_hardware_next: u32,
    previous_software_next: u32,
}

impl BluetoothDtmMemoryGraphEmptyListLinkPrepared {
    /// Return the exact scheduler-item identity retained by this graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Cancel before visibility or publication and recover bookkeeping state.
    pub fn cancel(mut self) -> BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
        self.storage.as_mut().project().scheduler_item.words[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET] =
            self.previous_hardware_next;
        self.storage.as_mut().project().scheduler_item.words[SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET] =
            self.previous_software_next;

        BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
            previous_control: self.previous_control,
            previous_status: self.previous_status,
            previous_completed_link: self.previous_completed_link,
        }
    }
}

/// Unique CPU owner of one statically located and allocation-initialized graph.
///
/// This state is still unreachable by hardware. It contains no `arm`,
/// `publish`, raw-pointer or `HardwareOwned` transition; the future LLL must
/// first prove the private packet-engine latch and visibility fences.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::{
///     BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
/// };
///
/// let storage = Box::leak(Box::new(BluetoothDtmMemoryGraphStorage::new()));
/// let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100).unwrap();
/// let config = open_esp_radio_esp32s31_bluetooth_memory::
///     BluetoothDtmSchedulerAllocationConfig::new(1, 1, 5, 1);
/// let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, config).unwrap();
/// let moved = owner;
/// let _binding = owner.binding();
/// drop(moved);
/// ```
pub struct BluetoothDtmMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

/// CPU-owned graph whose sole TX packet slot has a complete DTM image.
///
/// Construction consumes the graph owner and copies every declared payload
/// byte. The state remains unreachable by hardware and grants no scheduler or
/// publication authority.
#[must_use = "the prepared TX graph must remain owned or be explicitly discarded"]
pub struct BluetoothDtmMemoryGraphTxPacketPrepared {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    pattern_selector: u8,
    payload_length: u8,
}

impl BluetoothDtmMemoryGraphTxPacketPrepared {
    /// Return the positional DTM pattern selector written into the packet.
    pub const fn pattern_selector(&self) -> u8 {
        self.pattern_selector
    }

    /// Return the declared packet payload length.
    pub const fn payload_length(&self) -> u8 {
        self.payload_length
    }

    /// Borrow the complete reviewed prefix and declared payload.
    pub fn prepared_packet_bytes(&self) -> &[u8] {
        let storage = self.storage.as_ref().get_ref();
        &storage.tx_packet.bytes
            [..BLUETOOTH_DTM_TX_PACKET_PREFIX_BYTES + self.payload_length as usize]
    }

    /// Build and commit one positional event image while consuming readiness.
    ///
    /// A successful upper TX transition can retain this construction path in
    /// its own typestate. On failure the ordinary CPU owner is returned and
    /// the caller must explicitly prepare the packet again before retrying TX.
    pub fn try_prepare_positional_event<BuildError>(
        self,
        build: impl FnOnce(
            BluetoothDtmPositionalEventSeed,
        ) -> Result<BluetoothDtmPositionalEventWords, BuildError>,
    ) -> Result<
        BluetoothDtmMemoryGraphPositionalEventPrepared,
        BluetoothDtmMemoryGraphPrepareFailure<BuildError>,
    > {
        self.discard_packet_readiness()
            .try_prepare_positional_event(build)
    }

    /// Discard only the readiness proof and recover the CPU-owned graph.
    ///
    /// Packet bytes are retained, but another TX event must prepare them again
    /// to obtain a fresh proof for its exact pattern and length.
    pub fn discard_packet_readiness(self) -> BluetoothDtmMemoryGraphCpuOwned {
        BluetoothDtmMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

impl BluetoothDtmMemoryGraphCpuOwned {
    /// Borrow the location proof without granting publication authority.
    pub const fn binding(&self) -> &BluetoothDtmMemoryGraphBinding {
        &self.binding
    }

    /// Consume this graph and install one complete CPU-owned TX packet image.
    ///
    /// The fixed payload array makes the copy total for every accepted `u8`
    /// length; no callback can claim readiness without supplying all declared
    /// bytes. Bytes after the declared payload remain unchanged.
    pub fn prepare_tx_packet(
        mut self,
        pattern_selector: u8,
        payload_length: u8,
        payload: &[u8; BLUETOOTH_DTM_MAX_PACKET_CAPACITY],
    ) -> BluetoothDtmMemoryGraphTxPacketPrepared {
        let packet = self.storage.as_mut().project().tx_packet;
        let mut preparation = packet.begin_prepare(pattern_selector, payload_length);
        preparation
            .payload_mut()
            .copy_from_slice(&payload[..payload_length as usize]);
        let _prepared = preparation.finish();

        BluetoothDtmMemoryGraphTxPacketPrepared {
            storage: self.storage,
            binding: self.binding,
            pattern_selector,
            payload_length,
        }
    }

    /// Borrow the ordinary RX packet/header allocations as one CPU transition.
    ///
    /// This does not claim that hardware has completed or released a header.
    pub fn rx_buffer_mut(&mut self) -> BluetoothDtmRxBufferStorage<'_> {
        let storage = self.storage.as_mut().project();
        BluetoothDtmRxBufferStorage {
            packet: storage.rx_packet,
            header: storage.rx_header,
        }
    }

    /// Build and commit one role-neutral positional event-word image.
    ///
    /// The consuming closure receives current words and current private links
    /// from this exact owner. Builder failure and every anchor mismatch return
    /// the byte-unchanged owner. Once validation succeeds, committing all
    /// nineteen offsets is infallible and remains entirely CPU-owned.
    pub fn try_prepare_positional_event<BuildError>(
        mut self,
        build: impl FnOnce(
            BluetoothDtmPositionalEventSeed,
        ) -> Result<BluetoothDtmPositionalEventWords, BuildError>,
    ) -> Result<
        BluetoothDtmMemoryGraphPositionalEventPrepared,
        BluetoothDtmMemoryGraphPrepareFailure<BuildError>,
    > {
        let (previous, tx_head_image, rx_tail_image) = {
            let storage = self.storage.as_ref().get_ref();
            (
                BluetoothDtmPositionalEventWords::new(
                    storage.link_state.reviewed_words(),
                    storage.scheduler_item.reviewed_words(),
                ),
                storage.link_state.words[LINK_STATE_TX_HEAD_OFFSET] & LOW_TWENTY_MASK,
                storage.link_state.words[LINK_STATE_RX_TAIL_OFFSET] & LOW_TWENTY_MASK,
            )
        };
        let Some(tx_head) =
            BluetoothDtmBoundSramLinkAddress::from_nonzero_compressed_image(tx_head_image)
        else {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::CurrentTxHeadUnbound,
            });
        };
        let Some(rx_tail) =
            BluetoothDtmBoundSramLinkAddress::from_nonzero_compressed_image(rx_tail_image)
        else {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::CurrentRxTailUnbound,
            });
        };
        let seed = BluetoothDtmPositionalEventSeed {
            words: previous,
            tx_head,
            rx_tail,
        };
        let candidate = match build(seed) {
            Ok(candidate) => candidate,
            Err(error) => {
                return Err(BluetoothDtmMemoryGraphPrepareFailure {
                    owner: self,
                    error: BluetoothDtmMemoryGraphPrepareError::Build(error),
                });
            }
        };

        let observed_tx_head = candidate.link_state().word_00 & LOW_TWENTY_MASK;
        if observed_tx_head != tx_head_image {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::LinkStateTxHeadMismatch {
                    expected: tx_head_image,
                    observed: observed_tx_head,
                },
            });
        }
        let observed_rx_tail = candidate.link_state().word_08 & LOW_TWENTY_MASK;
        if observed_rx_tail != rx_tail_image {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::LinkStateRxTailMismatch {
                    expected: rx_tail_image,
                    observed: observed_rx_tail,
                },
            });
        }
        let expected_link_state = self.binding.link_state.compressed_image();
        let observed_link_state = candidate.scheduler_item().word_08 & LOW_TWENTY_MASK;
        if observed_link_state != expected_link_state {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::SchedulerItemLinkStateMismatch {
                    expected: expected_link_state,
                    observed: observed_link_state,
                },
            });
        }

        let storage = self.storage.as_mut().project();
        storage
            .link_state
            .write_reviewed_words(candidate.link_state());
        storage
            .scheduler_item
            .write_reviewed_words(candidate.scheduler_item());

        Ok(BluetoothDtmMemoryGraphPositionalEventPrepared {
            storage: self.storage,
            binding: self.binding,
            previous,
        })
    }

    fn initialize_reviewed_allocation(&mut self, config: BluetoothDtmSchedulerAllocationConfig) {
        let rx_header = BluetoothDtmRxBufferHeaderImage::new(self.binding.rx_packet).words();
        let rx_swap_reserve = BluetoothDtmRxBufferHeaderImage::unbound_swap_reserve().words();
        let tx_header = BluetoothDtmTxBufferHeaderImage::new(self.binding.tx_packet).words();
        let link_state = self.binding.link_state.compressed_image();
        let scheduler_context = self.binding.scheduler_context.compressed_image();
        let rx_header_link = self.binding.rx_header.compressed_image();
        let rx_swap_link = self.binding.rx_swap_reserve.compressed_image();
        let tx_header_link = self.binding.tx_header.compressed_image();

        let storage = self.storage.as_mut().project();
        storage.link_state.words.fill(0);
        storage.scheduler_context.words.fill(0);
        storage.scheduler_item.words.fill(0);
        storage.rx_header.words = rx_header;
        storage.rx_swap_reserve.words = rx_swap_reserve;
        storage.tx_header.words = tx_header;
        storage.tx_packet.bytes.fill(0);
        *storage.rx_packet = BluetoothDtmRxPacketStorage::new();

        storage.link_state.words[LINK_STATE_RX_HEAD_OFFSET] = rx_header_link;
        storage.link_state.words[LINK_STATE_TX_HEAD_OFFSET] = tx_header_link;
        storage.link_state.words[LINK_STATE_RX_TAIL_OFFSET] = rx_header_link;
        storage.link_state.words[LINK_STATE_TX_TAIL_OFFSET] = tx_header_link;
        storage.link_state.words[LINK_STATE_RX_SWAP_RESERVE_OFFSET] = rx_swap_link;
        storage.link_state.words[LINK_STATE_ALLOCATION_CONFIG_OFFSET] =
            LINK_STATE_ALLOCATION_CONFIG_IMAGE;
        storage.scheduler_item.words[SCHEDULER_ITEM_ALLOCATION_PREFIX_OFFSET] =
            SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE;
        storage.scheduler_item.words[SCHEDULER_ITEM_CONTEXT_OFFSET] = scheduler_context;
        storage.scheduler_item.words[SCHEDULER_ITEM_LINK_STATE_OFFSET] = link_state;
        storage.scheduler_item.words[SCHEDULER_ITEM_ALLOCATION_FLAGS_OFFSET] =
            SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE;
        storage.scheduler_item.words[SCHEDULER_ITEM_ALLOCATION_CONFIG_OFFSET] =
            config.allocation_image();
        storage.scheduler_item.words[SCHEDULER_ITEM_POSITIONAL_24_OFFSET] =
            SCHEDULER_ITEM_POSITIONAL_24_IMAGE;
    }
}

impl BluetoothDtmMemoryGraphStorage {
    /// Reserve the graph and install the reviewed RX allocation defaults.
    pub const fn new() -> Self {
        Self {
            link_state: BluetoothDtmLinkStateStorage {
                words: [0; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
            },
            scheduler_context: BluetoothDtmSchedulerContextStorage {
                words: [0; BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES / 4],
            },
            scheduler_item: BluetoothDtmSchedulerItemStorage {
                words: [0; BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4],
            },
            rx_header: BluetoothDtmBufferHeaderStorage {
                words: [0; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
            },
            rx_swap_reserve: BluetoothDtmBufferHeaderStorage {
                words: [0; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
            },
            tx_header: BluetoothDtmBufferHeaderStorage {
                words: [0; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
            },
            tx_packet: BluetoothDtmTxPacketStorage::new(),
            rx_packet: BluetoothDtmRxPacketStorage::new(),
            _pin: PhantomPinned,
        }
    }

    /// Bind the real addresses of one unique static S31 allocation.
    ///
    /// All address and extent checks finish before the first mutation. The
    /// returned CPU owner installs only reviewed allocation-time headers and
    /// private-chain anchors; the graph remains unreachable by hardware.
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
        config: BluetoothDtmSchedulerAllocationConfig,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothDtmMemoryGraphBindFailure::new(
                    storage,
                    BluetoothDtmMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base, config)
    }

    /// Bind a deterministic physical-SRAM address to a native ownership model.
    ///
    /// The model never derives an address from its host pointer. Passing a raw
    /// integer is rejected by the type system:
    ///
    /// ```compile_fail
    /// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphStorage;
    ///
    /// let storage = Box::leak(Box::new(BluetoothDtmMemoryGraphStorage::new()));
    /// let config = open_esp_radio_esp32s31_bluetooth_memory::
    ///     BluetoothDtmSchedulerAllocationConfig::new(1, 1, 5, 1);
    /// let _ = BluetoothDtmMemoryGraphStorage::pin_static_model(
    ///     storage,
    ///     0x2f00_0100,
    ///     config,
    /// );
    /// ```
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothDtmMemoryGraphModelAddress,
        config: BluetoothDtmSchedulerAllocationConfig,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        Self::pin_static_inner(storage, base.address(), config)
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
        config: BluetoothDtmSchedulerAllocationConfig,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        let binding = match BluetoothDtmMemoryGraphBinding::new(base) {
            Ok(binding) => binding,
            Err(error) => return Err(BluetoothDtmMemoryGraphBindFailure::new(storage, error)),
        };
        let mut owner = BluetoothDtmMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.initialize_reviewed_allocation(config);
        Ok(owner)
    }

    /// Borrow the sole TX packet backing slot for CPU-only LLL preparation.
    pub fn tx_packet_mut(&mut self) -> &mut BluetoothDtmTxPacketStorage {
        &mut self.tx_packet
    }

    /// Borrow the ordinary RX packet/header allocations as one re-arm unit.
    ///
    /// A future completed-header owner must quarantine the special swap bit
    /// before choosing this ordinary transition.
    pub fn rx_buffer_mut(&mut self) -> BluetoothDtmRxBufferStorage<'_> {
        BluetoothDtmRxBufferStorage {
            packet: &mut self.rx_packet,
            header: &mut self.rx_header,
        }
    }
}

impl Default for BluetoothDtmMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use core::{
        convert::Infallible,
        fmt::Debug,
        mem::{align_of, offset_of, size_of},
    };

    use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddressError;

    use super::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_DTM_BUFFER_HEADER_BYTES,
        BLUETOOTH_DTM_LINK_STATE_BYTES, BLUETOOTH_DTM_MAX_PACKET_CAPACITY,
        BLUETOOTH_DTM_RX_PACKET_BYTES, BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES,
        BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, BLUETOOTH_DTM_TX_PACKET_BYTES,
        BluetoothDtmBufferHeaderStorage, BluetoothDtmLinkStateReviewedWords,
        BluetoothDtmLinkStateStorage, BluetoothDtmMemoryGraphBindError,
        BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphModelAddress,
        BluetoothDtmMemoryGraphPrepareError, BluetoothDtmMemoryGraphStorage,
        BluetoothDtmPositionalEventSeed, BluetoothDtmPositionalEventWords,
        BluetoothDtmRxBufferHeaderImage, BluetoothDtmRxBufferStorage, BluetoothDtmRxPacketAddress,
        BluetoothDtmRxPacketAddressError, BluetoothDtmRxPacketStorage, BluetoothDtmRxRearmError,
        BluetoothDtmSchedulerAllocationConfig, BluetoothDtmSchedulerContextStorage,
        BluetoothDtmSchedulerItemReviewedWords, BluetoothDtmSchedulerItemStorage,
        BluetoothDtmTxBufferHeaderImage, BluetoothDtmTxPacketAddress,
        BluetoothDtmTxPacketAddressError, BluetoothDtmTxPacketStorage, LINK_STATE_RX_TAIL_OFFSET,
        LINK_STATE_TX_HEAD_OFFSET, LOW_TWENTY_MASK, SCHEDULER_ITEM_ALLOCATION_CONFIG_OFFSET,
        SCHEDULER_ITEM_ALLOCATION_FLAGS_OFFSET, SCHEDULER_ITEM_ALLOCATION_PREFIX_OFFSET,
        SCHEDULER_ITEM_COMPLETED_LINK_OFFSET, SCHEDULER_ITEM_CONTEXT_OFFSET,
        SCHEDULER_ITEM_CONTROL_BYTE, SCHEDULER_ITEM_CONTROL_OFFSET,
        SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET, SCHEDULER_ITEM_POSITIONAL_24_OFFSET,
        SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET, SCHEDULER_ITEM_STATUS_OFFSET,
    };

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct GraphSnapshot {
        link_state: [u32; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
        scheduler_context: [u32; BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES / 4],
        scheduler_item: [u32; BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4],
        rx_header: [u32; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
        rx_swap_reserve: [u32; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
        tx_header: [u32; BLUETOOTH_DTM_BUFFER_HEADER_BYTES / 4],
        tx_packet: [u8; BLUETOOTH_DTM_TX_PACKET_BYTES],
        rx_packet: [u8; BLUETOOTH_DTM_RX_PACKET_BYTES],
    }

    fn snapshot(storage: &BluetoothDtmMemoryGraphStorage) -> GraphSnapshot {
        GraphSnapshot {
            link_state: storage.link_state.words,
            scheduler_context: storage.scheduler_context.words,
            scheduler_item: storage.scheduler_item.words,
            rx_header: storage.rx_header.words,
            rx_swap_reserve: storage.rx_swap_reserve.words,
            tx_header: storage.tx_header.words,
            tx_packet: storage.tx_packet.bytes,
            rx_packet: storage.rx_packet.bytes,
        }
    }

    fn model_owner(base: u32) -> BluetoothDtmMemoryGraphCpuOwned {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(base)
            .expect("test base has valid compressed-pointer syntax");
        BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, allocation_config())
            .expect("test graph fits physical controller SRAM")
    }

    const fn allocation_config() -> BluetoothDtmSchedulerAllocationConfig {
        BluetoothDtmSchedulerAllocationConfig::new(2, 3, 5, 4)
    }

    fn candidate_words(seed: BluetoothDtmPositionalEventSeed) -> BluetoothDtmPositionalEventWords {
        let current = seed.words();
        let tx_head = seed.tx_head().compressed_image();
        let rx_tail = seed.rx_tail().compressed_image();
        let link_state = current.scheduler_item().word_08 & LOW_TWENTY_MASK;

        BluetoothDtmPositionalEventWords::new(
            BluetoothDtmLinkStateReviewedWords {
                word_00: 0xabc0_0000 | tx_head,
                word_04: 0x1111_1111,
                word_08: 0xdef0_0000 | rx_tail,
                word_14: 0x2222_2222,
                word_2c: 0x3333_3333,
                word_34: 0x4444_4444,
                word_38: 0x5555_5555,
                word_50: 0x6666_6666,
            },
            BluetoothDtmSchedulerItemReviewedWords {
                word_00: 0x7777_7777,
                word_04: 0x8888_8888,
                word_08: 0x9990_0000 | link_state,
                word_0c: 0x1212_1212,
                word_10: 0x3434_3434,
                word_14: 0xaaaa_aaaa,
                word_18: 0xbbbb_bbbb,
                word_2c: 0xcccc_cccc,
                word_44: 0xdddd_dddd,
                word_48: 0xeeee_eeee,
                word_4c: 0xffff_ffff,
            },
        )
    }

    fn assert_prepare_failure_unchanged<BuildError: Debug + Eq + PartialEq>(
        owner: BluetoothDtmMemoryGraphCpuOwned,
        build: impl FnOnce(
            BluetoothDtmPositionalEventSeed,
        ) -> Result<BluetoothDtmPositionalEventWords, BuildError>,
        expected: BluetoothDtmMemoryGraphPrepareError<BuildError>,
    ) -> BluetoothDtmMemoryGraphCpuOwned {
        let before = snapshot(owner.storage.as_ref().get_ref());
        let failure = match owner.try_prepare_positional_event(build) {
            Ok(_) => panic!("invalid positional event words must be rejected"),
            Err(failure) => failure,
        };
        assert_eq!(failure.error(), &expected);
        let (owner, error) = failure.into_parts();
        assert_eq!(error, expected);
        assert_eq!(snapshot(owner.storage.as_ref().get_ref()), before);
        owner
    }

    #[test]
    fn static_link_graph_has_every_reviewed_graph_allocation_footprint() {
        assert_eq!(size_of::<BluetoothDtmLinkStateStorage>(), 0x84);
        assert_eq!(size_of::<BluetoothDtmSchedulerItemStorage>(), 0x60);
        assert_eq!(size_of::<BluetoothDtmSchedulerContextStorage>(), 0x48);
        assert_eq!(size_of::<BluetoothDtmBufferHeaderStorage>(), 0x18);
        assert_eq!(size_of::<BluetoothDtmTxPacketStorage>(), 0x114);
        assert_eq!(size_of::<BluetoothDtmRxPacketStorage>(), 0x120);
        assert_eq!(size_of::<BluetoothDtmMemoryGraphStorage>(), 0x3a8);

        assert_eq!(BLUETOOTH_DTM_LINK_STATE_BYTES, 0x84);
        assert_eq!(BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, 0x60);
        assert_eq!(BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES, 0x48);
        assert_eq!(BLUETOOTH_DTM_BUFFER_HEADER_BYTES, 0x18);
        assert_eq!(BLUETOOTH_DTM_TX_PACKET_BYTES, 0x111);
        assert_eq!(BLUETOOTH_DTM_RX_PACKET_BYTES, 0x11d);

        assert_eq!(align_of::<BluetoothDtmMemoryGraphStorage>(), 4);
        assert_eq!(align_of::<BluetoothDtmRxPacketStorage>(), 4);

        assert_eq!(
            offset_of!(BluetoothDtmMemoryGraphStorage, link_state),
            0x000
        );
        assert_eq!(
            offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_context),
            0x084
        );
        assert_eq!(
            offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_item),
            0x0cc
        );
        assert_eq!(offset_of!(BluetoothDtmMemoryGraphStorage, rx_header), 0x12c);
        assert_eq!(
            offset_of!(BluetoothDtmMemoryGraphStorage, rx_swap_reserve),
            0x144
        );
        assert_eq!(offset_of!(BluetoothDtmMemoryGraphStorage, tx_header), 0x15c);
        assert_eq!(offset_of!(BluetoothDtmMemoryGraphStorage, tx_packet), 0x174);
        assert_eq!(offset_of!(BluetoothDtmMemoryGraphStorage, rx_packet), 0x288);
    }

    #[test]
    fn model_binding_initializes_every_reviewed_header_and_private_anchor() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("model base has valid compressed-pointer syntax");
        let owner =
            BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, allocation_config())
                .expect("complete model graph fits physical controller SRAM");
        assert_eq!(owner.binding().range(), (0x2f00_0100, 0x2f00_04a8));
        assert_eq!(
            owner.binding().link_state_address().compressed_image(),
            0x040
        );
        assert_eq!(
            owner.binding().scheduler_context_address().address(),
            0x2f00_0184
        );
        assert_eq!(
            owner.binding().scheduler_item_address().compressed_image(),
            0x073
        );

        let graph = owner.storage.as_ref().get_ref();
        assert_eq!(graph.rx_header.words, [0, 0x0e2, 0x8080_0000, 0, 0, 0]);
        assert_eq!(graph.rx_swap_reserve.words, [0, 0, 0x8080_0000, 0, 0, 0]);
        assert_eq!(
            graph.tx_header.words,
            [0, 0x09d, 0x80a0_00a1, 0, 0x0000_07f8, 0]
        );
        assert_eq!(
            &graph.link_state.words[0x68 / 4..=0x78 / 4],
            &[0x08b, 0x097, 0x08b, 0x097, 0x091]
        );
        assert_eq!(graph.scheduler_item.words[0x08 / 4], 0x040);
        assert_eq!(graph.link_state.words[0x30 / 4], 0x0000_1e00);
        assert_eq!(
            graph.scheduler_item.words[SCHEDULER_ITEM_ALLOCATION_PREFIX_OFFSET],
            0x0030_0000
        );
        assert_eq!(
            graph.scheduler_item.words[SCHEDULER_ITEM_CONTEXT_OFFSET],
            0x061
        );
        assert_eq!(
            graph.scheduler_item.words[SCHEDULER_ITEM_ALLOCATION_FLAGS_OFFSET],
            0xffdf_ffff
        );
        assert_eq!(
            graph.scheduler_item.words[SCHEDULER_ITEM_ALLOCATION_CONFIG_OFFSET],
            19
        );
        assert_eq!(
            graph.scheduler_item.words[SCHEDULER_ITEM_POSITIONAL_24_OFFSET],
            0x0007_bdef
        );
        assert_eq!(graph.rx_packet.bytes[0x05], 1);
        assert_eq!(graph.rx_packet.bytes[0x06], 1);
        assert_eq!(graph.rx_packet.result_word(), 0x00ff_ffff);
        assert_eq!(&graph.rx_packet.bytes[0x18..0x1a], &[0xff, 0xff]);
    }

    #[test]
    fn positional_commit_writes_only_the_nineteen_reviewed_offsets() {
        let owner = model_owner(0x2f00_0100);
        let before = snapshot(owner.storage.as_ref().get_ref());
        let mut expected = None;
        let prepared = owner
            .try_prepare_positional_event(|seed| {
                let candidate = candidate_words(seed);
                expected = Some(candidate);
                Ok::<_, Infallible>(candidate)
            })
            .expect("fresh graph links accept their matching positional image");
        let expected = expected.expect("the consuming builder ran exactly once");
        assert_eq!(prepared.words(), expected);

        let after = snapshot(prepared.storage.as_ref().get_ref());
        let link_state_offsets = [0, 1, 2, 5, 11, 13, 14, 20];
        let scheduler_item_offsets = [0, 1, 2, 3, 4, 5, 6, 11, 17, 18, 19];
        for index in 0..before.link_state.len() {
            if !link_state_offsets.contains(&index) {
                assert_eq!(after.link_state[index], before.link_state[index]);
            }
        }
        for index in 0..before.scheduler_item.len() {
            if !scheduler_item_offsets.contains(&index) {
                assert_eq!(after.scheduler_item[index], before.scheduler_item[index]);
            }
        }
        assert_eq!(after.scheduler_context, before.scheduler_context);
        assert_eq!(after.rx_header, before.rx_header);
        assert_eq!(after.rx_swap_reserve, before.rx_swap_reserve);
        assert_eq!(after.tx_header, before.tx_header);
        assert_eq!(after.tx_packet, before.tx_packet);
        assert_eq!(after.rx_packet, before.rx_packet);
        assert_eq!(after.link_state[0x6c / 4], before.link_state[0x6c / 4]);
        assert_eq!(after.link_state[0x70 / 4], before.link_state[0x70 / 4]);
    }

    #[test]
    fn cancel_restores_the_complete_logical_graph_image() {
        let mut payload = [0; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
        payload[..3].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        let owner = model_owner(0x2f00_0500)
            .prepare_tx_packet(7, 3, &payload)
            .discard_packet_readiness();
        let before = snapshot(owner.storage.as_ref().get_ref());

        let prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("matching anchors prepare a CPU-owned image");
        assert_ne!(snapshot(prepared.storage.as_ref().get_ref()), before);
        let owner = prepared.cancel();

        assert_eq!(snapshot(owner.storage.as_ref().get_ref()), before);
    }

    #[test]
    fn scheduler_bookkeeping_cancel_restores_the_prepared_event() {
        let mut owner = model_owner(0x2f00_0900);
        let storage = owner.storage.as_mut().project().scheduler_item;
        storage.words[SCHEDULER_ITEM_STATUS_OFFSET] = 7;
        storage.words[SCHEDULER_ITEM_COMPLETED_LINK_OFFSET] = 0x1234;

        let prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("matching anchors prepare a CPU-owned image");
        let before = snapshot(prepared.storage.as_ref().get_ref());
        let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
        let graph = scheduler_prepared.storage.as_ref().get_ref();
        let control = graph.scheduler_item.words[SCHEDULER_ITEM_CONTROL_OFFSET].to_le_bytes();

        assert_eq!(control[SCHEDULER_ITEM_CONTROL_BYTE], 0);
        assert_eq!(
            graph.scheduler_item.words[SCHEDULER_ITEM_STATUS_OFFSET],
            u32::MAX
        );
        assert_eq!(
            graph.scheduler_item.words[SCHEDULER_ITEM_COMPLETED_LINK_OFFSET],
            0
        );

        let prepared = scheduler_prepared.cancel();
        assert_eq!(snapshot(prepared.storage.as_ref().get_ref()), before);
    }

    #[test]
    fn empty_list_link_preparation_is_the_only_additional_memory_change() {
        let owner = model_owner(0x2f00_0d00);
        let prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("matching anchors prepare a CPU-owned image")
            .prepare_scheduler_bookkeeping();
        let before = snapshot(prepared.storage.as_ref().get_ref());

        let merged = prepared.prepare_empty_list_link();
        let after = snapshot(merged.storage.as_ref().get_ref());
        let mut expected = before.clone();
        expected.scheduler_item[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET] &= !LOW_TWENTY_MASK;
        expected.scheduler_item[SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET] = 0;
        assert_eq!(after, expected);

        let prepared = merged.cancel();
        assert_eq!(snapshot(prepared.storage.as_ref().get_ref()), before);
    }

    #[test]
    fn consuming_seed_uses_current_private_links_instead_of_initial_binding_links() {
        let mut owner = model_owner(0x2f00_1500);
        let storage = owner.storage.as_mut().project();
        storage.link_state.words[LINK_STATE_TX_HEAD_OFFSET] = 0x701;
        storage.link_state.words[LINK_STATE_RX_TAIL_OFFSET] = 0x702;
        let before = snapshot(owner.storage.as_ref().get_ref());

        let prepared = owner
            .try_prepare_positional_event(|seed| {
                assert_eq!(seed.tx_head().compressed_image(), 0x701);
                assert_eq!(seed.rx_tail().compressed_image(), 0x702);
                Ok::<_, Infallible>(candidate_words(seed))
            })
            .expect("freshly sampled nonzero links bind the same transaction");
        let words = prepared.words();
        assert_eq!(words.link_state().word_00 & LOW_TWENTY_MASK, 0x701);
        assert_eq!(words.link_state().word_08 & LOW_TWENTY_MASK, 0x702);
        let after = snapshot(prepared.storage.as_ref().get_ref());
        assert_eq!(after.link_state[LINK_STATE_TX_HEAD_OFFSET], 0x701);
        assert_eq!(after.link_state[LINK_STATE_RX_TAIL_OFFSET], 0x702);

        let owner = prepared.cancel();
        assert_eq!(snapshot(owner.storage.as_ref().get_ref()), before);
    }

    #[test]
    fn every_builder_and_anchor_failure_returns_the_byte_unchanged_owner() {
        let owner = model_owner(0x2f00_0900);
        let owner = assert_prepare_failure_unchanged(
            owner,
            |_| Err::<BluetoothDtmPositionalEventWords, _>("builder rejected inputs"),
            BluetoothDtmMemoryGraphPrepareError::Build("builder rejected inputs"),
        );

        let owner = assert_prepare_failure_unchanged(
            owner,
            |seed| {
                let candidate = candidate_words(seed);
                let mut link_state = candidate.link_state();
                link_state.word_00 ^= 1;
                Ok::<_, Infallible>(BluetoothDtmPositionalEventWords::new(
                    link_state,
                    candidate.scheduler_item(),
                ))
            },
            BluetoothDtmMemoryGraphPrepareError::LinkStateTxHeadMismatch {
                expected: 0x297,
                observed: 0x296,
            },
        );

        let owner = assert_prepare_failure_unchanged(
            owner,
            |seed| {
                let candidate = candidate_words(seed);
                let mut link_state = candidate.link_state();
                link_state.word_08 ^= 1;
                Ok::<_, Infallible>(BluetoothDtmPositionalEventWords::new(
                    link_state,
                    candidate.scheduler_item(),
                ))
            },
            BluetoothDtmMemoryGraphPrepareError::LinkStateRxTailMismatch {
                expected: 0x28b,
                observed: 0x28a,
            },
        );

        let _owner = assert_prepare_failure_unchanged(
            owner,
            |seed| {
                let candidate = candidate_words(seed);
                let mut scheduler_item = candidate.scheduler_item();
                scheduler_item.word_08 ^= 1;
                Ok::<_, Infallible>(BluetoothDtmPositionalEventWords::new(
                    candidate.link_state(),
                    scheduler_item,
                ))
            },
            BluetoothDtmMemoryGraphPrepareError::SchedulerItemLinkStateMismatch {
                expected: 0x240,
                observed: 0x241,
            },
        );

        let mut owner = model_owner(0x2f00_0d00);
        owner.storage.as_mut().project().link_state.words[LINK_STATE_TX_HEAD_OFFSET] = 0;
        let _owner = assert_prepare_failure_unchanged(
            owner,
            |_| -> Result<BluetoothDtmPositionalEventWords, Infallible> {
                panic!("an unbound current TX head must preempt the builder")
            },
            BluetoothDtmMemoryGraphPrepareError::CurrentTxHeadUnbound,
        );

        let mut owner = model_owner(0x2f00_1100);
        owner.storage.as_mut().project().link_state.words[LINK_STATE_RX_TAIL_OFFSET] = 0;
        let _owner = assert_prepare_failure_unchanged(
            owner,
            |_| -> Result<BluetoothDtmPositionalEventWords, Infallible> {
                panic!("an unbound current RX tail must preempt the builder")
            },
            BluetoothDtmMemoryGraphPrepareError::CurrentRxTailUnbound,
        );
    }

    #[test]
    fn failed_binding_returns_the_unchanged_storage_for_retry() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let mut preparation = storage.tx_packet_mut().begin_prepare(7, 3);
        preparation
            .payload_mut()
            .copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        preparation.finish().release();
        let original_address = core::ptr::addr_of!(*storage).addr();
        let crossing = BluetoothDtmMemoryGraphModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 0x3a8 + 4,
        )
        .expect("crossing base still has valid compressed-pointer syntax");

        let failure = match BluetoothDtmMemoryGraphStorage::pin_static_model(
            storage,
            crossing,
            allocation_config(),
        ) {
            Ok(_) => panic!("a graph crossing physical SRAM must be rejected"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram
        );
        let (storage, error) = failure.into_parts();
        assert_eq!(
            error,
            BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram
        );
        assert_eq!(core::ptr::addr_of!(*storage).addr(), original_address);
        assert_eq!(
            &storage.tx_packet.bytes[0x10..0x15],
            &[7, 3, 0xaa, 0xbb, 0xcc]
        );
        assert_eq!(storage.rx_header.words, [0; 6]);

        let valid = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("retry base has valid compressed-pointer syntax");
        let owner =
            BluetoothDtmMemoryGraphStorage::pin_static_model(storage, valid, allocation_config())
                .expect("returned allocation can be bound exactly once");
        assert_eq!(owner.binding().range().0, 0x2f00_0100);
        assert_eq!(owner.storage.as_ref().get_ref().tx_packet.bytes, [0; 0x111]);
    }

    #[test]
    fn binding_rejects_zero_link_and_accepts_the_exact_physical_tail() {
        assert_eq!(
            BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0101),
            Err(BluetoothControllerSramAddressError::Unaligned)
        );

        let zero_storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let zero = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0000)
            .expect("physical base has valid encoding syntax");
        let zero_failure = match BluetoothDtmMemoryGraphStorage::pin_static_model(
            zero_storage,
            zero,
            allocation_config(),
        ) {
            Ok(_) => panic!("link state must not collapse onto the unbound zero image"),
            Err(failure) => failure,
        };
        assert_eq!(
            zero_failure.error(),
            BluetoothDtmMemoryGraphBindError::ZeroCompressedLink
        );

        let tail_storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let tail = BluetoothDtmMemoryGraphModelAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 0x3a8,
        )
        .expect("tail base has valid compressed-pointer syntax");
        let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(
            tail_storage,
            tail,
            allocation_config(),
        )
        .expect("graph whose exclusive end equals SRAM high is valid");
        assert_eq!(
            owner.binding().range(),
            (
                BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 0x3a8,
                BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH,
            )
        );
    }

    #[test]
    fn tx_packet_extent_and_bound_header_match_the_complete_allocator() {
        let packet = BluetoothDtmTxPacketAddress::new(0x2f00_0100)
            .expect("complete TX packet extent fits controller SRAM");
        assert_eq!(
            BluetoothDtmTxBufferHeaderImage::new(packet).words(),
            [0, 0x0000_0040, 0x80a0_0044, 0, 0x0000_07f8, 0]
        );

        assert!(BluetoothDtmTxPacketAddress::new(0x2f3f_feec).is_ok());
        assert_eq!(
            BluetoothDtmTxPacketAddress::new(0x2f3f_fef0),
            Err(BluetoothDtmTxPacketAddressError::ExtentOutsideControllerSram)
        );
        assert_eq!(
            BluetoothDtmTxPacketAddress::new(0x2f00_0000),
            Err(BluetoothDtmTxPacketAddressError::ZeroCompressedBase)
        );
        assert_eq!(
            BluetoothDtmTxPacketAddress::new(0x2f00_0001),
            Err(BluetoothDtmTxPacketAddressError::InvalidBase(
                BluetoothControllerSramAddressError::Unaligned
            ))
        );
    }

    #[test]
    fn rx_packet_extent_and_bound_header_match_the_complete_allocator() {
        let packet = BluetoothDtmRxPacketAddress::new(0x2f00_0100)
            .expect("complete RX packet extent fits controller SRAM");
        assert_eq!(
            BluetoothDtmRxBufferHeaderImage::new(packet).words(),
            [0, 0x0000_0040, 0x8080_0000, 0, 0, 0]
        );
        assert_eq!(
            BluetoothDtmRxBufferHeaderImage::unbound_swap_reserve().words(),
            [0, 0, 0x8080_0000, 0, 0, 0]
        );

        assert!(BluetoothDtmRxPacketAddress::new(0x2f3f_fee0).is_ok());
        assert_eq!(
            BluetoothDtmRxPacketAddress::new(0x2f00_0000),
            Err(BluetoothDtmRxPacketAddressError::ZeroCompressedBase)
        );
        assert_eq!(
            BluetoothDtmRxPacketAddress::new(0x2f3f_fee4),
            Err(BluetoothDtmRxPacketAddressError::ExtentOutsideControllerSram)
        );
        assert_eq!(
            BluetoothDtmRxPacketAddress::new(0x2f00_0001),
            Err(BluetoothDtmRxPacketAddressError::InvalidBase(
                BluetoothControllerSramAddressError::Unaligned
            ))
        );
    }

    #[test]
    fn rx_rearm_updates_packet_and_header_as_one_transition() {
        let mut packet = BluetoothDtmRxPacketStorage::new();
        let mut header = BluetoothDtmBufferHeaderStorage {
            words: [0, 0, 0x8080_0000, 0x8123_4567, 0, 0],
        };
        assert_eq!(packet.bytes()[0x05], 1);
        assert_eq!(packet.bytes()[0x06], 1);
        assert_eq!(packet.result_word(), 0x00ff_ffff);
        assert_eq!(&packet.bytes()[0x18..0x1a], &[0xff, 0xff]);

        packet.bytes[0x0c] = 0;
        packet.bytes[0x0d] = 0;
        packet.bytes[0x0e] = 0;
        packet.bytes[0x0f] = 0xa5;
        packet.bytes[0x18] = 0;
        packet.bytes[0x19] = 0;
        BluetoothDtmRxBufferStorage {
            packet: &mut packet,
            header: &mut header,
        }
        .rearm_reviewed_fields()
        .expect("ordinary header does not request the swap reserve");

        assert_eq!(packet.result_word(), 0xa5ff_ffff);
        assert_eq!(&packet.bytes()[0x18..0x1a], &[0xff, 0xff]);
        assert_eq!(header.words[3], 0x0123_4567);
    }

    #[test]
    fn rx_rearm_quarantines_swap_reserve_bit_without_mutation() {
        let mut packet = BluetoothDtmRxPacketStorage::new();
        packet.bytes[0x0c] = 0;
        packet.bytes[0x0d] = 0;
        packet.bytes[0x0e] = 0;
        packet.bytes[0x0f] = 0xa5;
        packet.bytes[0x18] = 0;
        packet.bytes[0x19] = 0;
        let before_packet = packet.bytes;
        let mut header = BluetoothDtmBufferHeaderStorage {
            words: [0, 0x40, 0x8080_0000, 0x8123_4567, 1, 0],
        };
        let before_header = header.words;

        assert_eq!(
            BluetoothDtmRxBufferStorage {
                packet: &mut packet,
                header: &mut header,
            }
            .rearm_reviewed_fields(),
            Err(BluetoothDtmRxRearmError::SwapReserveDecisionRequired)
        );
        assert_eq!(packet.bytes, before_packet);
        assert_eq!(header.words, before_header);
    }

    #[test]
    fn graph_tx_slot_is_the_same_backing_used_for_packet_preparation() {
        let mut graph = BluetoothDtmMemoryGraphStorage::new();
        let mut preparation = graph.tx_packet_mut().begin_prepare(7, 3);
        preparation
            .payload_mut()
            .copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        let prepared = preparation.finish();

        assert_eq!(prepared.prepared_bytes()[0x05], 2);
        assert_eq!(prepared.prepared_bytes()[0x06], 0);
        assert_eq!(prepared.prepared_bytes()[0x10], 7);
        assert_eq!(prepared.prepared_bytes()[0x11], 3);
        assert_eq!(prepared.payload(), [0xaa, 0xbb, 0xcc]);
    }
}
