//! Static CPU-owned storage for the recovered ESP32-S31 DTM memory graph.
//!
//! The types in this module reserve the complete finite per-event link-graph
//! footprint and reproduce only reviewed CPU-side initialization transforms.
//! The separate `0x28`-byte DTM environment remains LLL state above this
//! boundary. A target-only binding derives the real addresses of one static
//! allocation, rejects storage outside physical internal SRAM and retains the
//! allocation behind one movable owner. A matching affine PAC head-publication
//! token consumes the last CPU rollback state into a controller-visible graph.
//! Four-byte alignment is the minimum proven by compressed controller links;
//! it is not a cache-coherency claim.

#![forbid(unsafe_code)]

use core::{convert::Infallible, marker::PhantomPinned, num::NonZeroU32, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerSoftwareListRemovalReady,
};
use pin_project::pin_project;
use vcell::VolatileCell;

use crate::{
    dtm_event_image::{
        BluetoothDtmLinkStateProfileWord, BluetoothDtmLinkStateReviewedWords,
        BluetoothDtmPositionalEventWords, BluetoothDtmRxHeaderTailProjection,
        BluetoothDtmSchedulerHardwareChainWord, BluetoothDtmSchedulerItemReviewedWords,
        BluetoothDtmTxHeaderHeadProjection, BluetoothLeAccessAddress, BluetoothLeCrcInit,
    },
    dtm_rx_result::{BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError},
    le_tx_packet::{
        BLUETOOTH_LE_BUFFER_HEADER_BYTES, BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES,
        BluetoothLeTxBufferHeaderStorage, BluetoothLeTxPacketAddress,
        BluetoothLeTxPacketPreparedLength, BluetoothLeTxPacketStorage,
    },
    sram_link::BluetoothControllerSramLinkAddress,
};

/// Bytes allocated for one DTM link-state object.
pub const BLUETOOTH_DTM_LINK_STATE_BYTES: usize = 0x84;
/// Bytes allocated for one DTM scheduler item.
pub const BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Bytes allocated for the separate DTM scheduler context.
pub const BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES: usize = 0x48;
/// Bytes preceding the maximum DTM receiver capacity.
pub const BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES: usize = 0x1e;
/// Maximum packet capacity supplied by the complete DTM allocator.
pub const BLUETOOTH_DTM_MAX_PACKET_CAPACITY: usize = u8::MAX as usize;
/// Logical bytes in the complete DTM TX packet allocation.
pub const BLUETOOTH_DTM_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;
/// Logical bytes in the complete DTM RX packet allocation.
pub const BLUETOOTH_DTM_RX_PACKET_BYTES: usize =
    BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;

type BluetoothDtmTxPacketAddress = BluetoothLeTxPacketAddress<BLUETOOTH_DTM_TX_PACKET_BYTES>;

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
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4;
const RX_PACKET_WORDS: usize = BLUETOOTH_DTM_RX_PACKET_BYTES.div_ceil(4);
const PRIVATE_SCHEDULER_ALLOCATION_ADDITION: u32 = 5;

/// Product-owned limits contributing to the DTM scheduler allocation field.
///
/// The current allocator forms scheduler-item `+0x20[11:0]` from three public
/// ESP-IDF controller limits and target-private additions. The private
/// halfword recovered at options offset `+0x14` is fixed by this S31
/// implementation; it is not an application policy or part of this API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerAllocationConfig {
    extended_advertising_instances: u16,
    connections: u16,
    periodic_syncs: u8,
}

impl BluetoothDtmSchedulerAllocationConfig {
    /// Capture the exact source-owned inputs used by the S31 DTM allocator.
    pub const fn new(
        extended_advertising_instances: u16,
        connections: u16,
        periodic_syncs: u8,
    ) -> Self {
        Self {
            extended_advertising_instances,
            connections,
            periodic_syncs,
        }
    }

    const fn allocation_image(self) -> u32 {
        ((self.extended_advertising_instances as u32)
            .wrapping_add(1)
            .wrapping_add(self.connections as u32)
            .wrapping_add(PRIVATE_SCHEDULER_ALLOCATION_ADDITION)
            .wrapping_add(4)
            .wrapping_add(self.periodic_syncs as u32))
            & 0x0fff
    }
}

/// Why a complete DTM RX packet extent cannot inhabit controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothDtmRxPacketAddressError {
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
struct BluetoothDtmRxPacketAddress {
    base: BluetoothControllerSramAddress,
}

impl BluetoothDtmRxPacketAddress {
    /// Validate the base and final aligned word of the `0x11d`-byte allocation.
    const fn new(address: u32) -> Result<Self, BluetoothDtmRxPacketAddressError> {
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

/// Opaque CPU-owned link-state allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmLinkStateStorage {
    words: [VolatileCell<u32>; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
}

impl BluetoothDtmLinkStateStorage {
    const CRC_INIT_WORD: usize = 0x2c / 4;
    const ACCESS_ADDRESS_WORD: usize = 0x38 / 4;

    fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn access_address(&self) -> BluetoothLeAccessAddress {
        BluetoothLeAccessAddress::from_controller_image(self.read_word(Self::ACCESS_ADDRESS_WORD))
    }

    fn write_access_address(&self, access_address: BluetoothLeAccessAddress) {
        self.write_word(Self::ACCESS_ADDRESS_WORD, access_address.controller_image());
    }

    fn crc_init(&self) -> BluetoothLeCrcInit {
        BluetoothLeCrcInit::from_controller_word(self.read_word(Self::CRC_INIT_WORD))
    }

    fn write_crc_init(&self, crc_init: BluetoothLeCrcInit) {
        let word = self.read_word(Self::CRC_INIT_WORD);
        self.write_word(Self::CRC_INIT_WORD, crc_init.apply_to_controller_word(word));
    }

    #[cfg(test)]
    fn snapshot(&self) -> [u32; BLUETOOTH_DTM_LINK_STATE_BYTES / 4] {
        core::array::from_fn(|index| self.read_word(index))
    }

    fn reviewed_words(&self) -> BluetoothDtmLinkStateReviewedWords {
        BluetoothDtmLinkStateReviewedWords {
            word_00: self.read_word(0),
            word_04: self.read_word(1),
            word_08: self.read_word(2),
            profile_word_14: BluetoothDtmLinkStateProfileWord::from_storage(self.read_word(5)),
            crc_init: self.crc_init(),
            word_34: self.read_word(13),
            access_address: self.access_address(),
            word_50: self.read_word(20),
        }
    }

    fn write_reviewed_words(&self, words: BluetoothDtmLinkStateReviewedWords) {
        self.write_word(0, words.word_00);
        self.write_word(1, words.word_04);
        self.write_word(2, words.word_08);
        self.write_word(5, words.profile_word_14.into_storage());
        self.write_crc_init(words.crc_init());
        self.write_word(13, words.word_34);
        self.write_access_address(words.access_address());
        self.write_word(20, words.word_50);
    }
}

/// Opaque hardware-shared scheduler-item allocation.
///
/// Every word is volatile because the controller may change descriptor state
/// after head publication. CPU-owned typestates still serialize all writes;
/// the volatile cells prevent ordinary Rust loads from being substituted for
/// the explicit post-fence completion observation.
#[repr(C, align(4))]
pub struct BluetoothDtmSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl BluetoothDtmSchedulerItemStorage {
    fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn hardware_chain_word(&self) -> BluetoothDtmSchedulerHardwareChainWord {
        BluetoothDtmSchedulerHardwareChainWord::from_storage(
            self.read_word(SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET),
        )
    }

    fn write_hardware_chain_word(&self, word: BluetoothDtmSchedulerHardwareChainWord) {
        self.write_word(SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET, word.into_storage());
    }

    /// Terminate the hardware-consumed scheduler chain and return the exact
    /// prior field-containing word for pre-publication rollback.
    fn terminate_hardware_chain(&self) -> BluetoothDtmSchedulerHardwareChainWord {
        let previous = self.hardware_chain_word();
        self.write_hardware_chain_word(previous.terminate());
        previous
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> [u32; SCHEDULER_ITEM_WORDS] {
        core::array::from_fn(|index| self.read_word(index))
    }

    fn reviewed_words(&self) -> BluetoothDtmSchedulerItemReviewedWords {
        BluetoothDtmSchedulerItemReviewedWords {
            word_00: self.read_word(0),
            word_04: self.read_word(1),
            word_08: self.read_word(2),
            word_0c: self.read_word(3),
            word_10: self.read_word(4),
            word_14: self.read_word(5),
            word_18: self.read_word(6),
            word_2c: self.read_word(11),
            word_44: self.read_word(17),
            word_48: self.read_word(18),
            word_4c: self.read_word(19),
        }
    }

    fn write_reviewed_words(&mut self, words: BluetoothDtmSchedulerItemReviewedWords) {
        self.write_word(0, words.word_00);
        self.write_word(1, words.word_04);
        self.write_word(2, words.word_08);
        self.write_word(3, words.word_0c);
        self.write_word(4, words.word_10);
        self.write_word(5, words.word_14);
        self.write_word(6, words.word_18);
        self.write_word(11, words.word_2c);
        self.write_word(17, words.word_44);
        self.write_word(18, words.word_48);
        self.write_word(19, words.word_4c);
    }
}

/// Opaque CPU-owned scheduler-context allocation.
#[repr(C, align(4))]
pub struct BluetoothDtmSchedulerContextStorage {
    words: [u32; BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES / 4],
}

/// One zero-based DTM RX buffer-header allocation.
#[repr(C, align(4))]
struct BluetoothDtmRxBufferHeaderStorage {
    words: [VolatileCell<u32>; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
}

impl BluetoothDtmRxBufferHeaderStorage {
    const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;
    const RX_COMPLETION_OBSERVED_MASK: u32 = 0x8000_0000;
    const RX_SOFTWARE_TERMINAL_MASK: u32 = 1;

    fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn install(&self, words: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4]) {
        for (cell, word) in self.words.iter().zip(words) {
            cell.set(word);
        }
    }

    fn snapshot_words(&self) -> [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4] {
        core::array::from_fn(|index| self.read_word(index))
    }

    fn initialize_bound_rx(&self, packet: BluetoothDtmRxPacketAddress) {
        self.install([0, packet.compressed_image(), 0x8080_0000, 0, 0, 0]);
    }

    fn initialize_rx_swap_reserve(&self) {
        self.install([0, 0, 0x8080_0000, 0, 0, 0]);
    }

    fn rx_successor_link(&self) -> Option<BluetoothDtmRxHeaderSuccessorLink> {
        BluetoothDtmRxHeaderSuccessorLink::from_image(
            self.read_word(0) & Self::COMPRESSED_LINK_MASK,
        )
    }

    fn set_rx_successor_link(&self, link: Option<BluetoothDtmRxHeaderSuccessorLink>) {
        let current = self.read_word(0);
        let image = link.map_or(0, BluetoothDtmRxHeaderSuccessorLink::image);
        self.write_word(0, (current & !Self::COMPRESSED_LINK_MASK) | image);
    }

    fn rx_packet_link(&self) -> Option<BluetoothDtmRxPacketLink> {
        BluetoothDtmRxPacketLink::from_image(self.read_word(1) & Self::COMPRESSED_LINK_MASK)
    }

    fn clear_rx_packet_link(&self) {
        let current = self.read_word(1);
        self.write_word(1, current & !Self::COMPRESSED_LINK_MASK);
    }

    /// Observe the controller-written completion state only after the exact
    /// finished-list report fence retained by the recycle transaction.
    fn observe_rx_completion_after_fence(
        &self,
        _fenced: &BluetoothDtmMemoryGraphRecyclePrepared,
    ) -> BluetoothDtmRxHeaderCompletionObservation {
        BluetoothDtmRxHeaderCompletionObservation(
            self.read_word(3) & Self::RX_COMPLETION_OBSERVED_MASK != 0,
        )
    }

    fn clear_rx_completion_observation(&self) {
        let current = self.read_word(3);
        self.write_word(3, current & !Self::RX_COMPLETION_OBSERVED_MASK);
    }

    #[cfg(test)]
    fn model_controller_completion_observed(&self) {
        let current = self.read_word(3);
        self.write_word(3, current | Self::RX_COMPLETION_OBSERVED_MASK);
    }

    fn set_rx_software_terminal(&self, terminal: bool) {
        let current = self.read_word(4);
        let value = if terminal {
            current | Self::RX_SOFTWARE_TERMINAL_MASK
        } else {
            current & !Self::RX_SOFTWARE_TERMINAL_MASK
        };
        self.write_word(4, value);
    }

    fn rx_backlink(&self) -> Option<BluetoothDtmRxHeaderBacklink> {
        BluetoothDtmRxHeaderBacklink::from_address(self.read_word(5))
    }

    fn set_rx_backlink(&self, backlink: Option<BluetoothDtmRxHeaderBacklink>) {
        self.write_word(5, backlink.map_or(0, BluetoothDtmRxHeaderBacklink::address));
    }

    fn copy_complete_rx_image_from(&self, source: &Self) {
        self.install(source.snapshot_words());
    }

    #[cfg(test)]
    fn model_retarget_rx_packet(&self, packet: BluetoothDtmRxPacketAddress) {
        let current = self.read_word(1);
        self.write_word(
            1,
            (current & !Self::COMPRESSED_LINK_MASK) | packet.compressed_image(),
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothDtmRxHeaderSuccessorLink(u32);

impl BluetoothDtmRxHeaderSuccessorLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn from_bound(address: BluetoothControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn image(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothDtmRxPacketLink(u32);

impl BluetoothDtmRxPacketLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn from_bound(packet: BluetoothDtmRxPacketAddress) -> Self {
        Self(packet.compressed_image())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothDtmRxHeaderBacklink(u32);

impl BluetoothDtmRxHeaderBacklink {
    fn from_address(address: u32) -> Option<Self> {
        (address != 0).then_some(Self(address))
    }

    const fn bound(address: BluetoothControllerSramAddress) -> Self {
        Self(address.address())
    }

    const fn address(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothDtmRxHeaderCompletionObservation(bool);

impl BluetoothDtmRxHeaderCompletionObservation {
    const fn is_completed(self) -> bool {
        self.0
    }
}

/// Why a lower-layer selector cannot form a standard LE Test PDU header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmTxPacketPrepareError {
    /// The LE Test PDU Type field admits exactly the eight standard DTM types.
    UnsupportedPayloadType,
}

/// One standard LE Test PDU header without a Constant Tone Extension.
///
/// The current linked producer writes the payload Type and leaves the CP field
/// clear. Encoding remains private to the packet codec; this value does not
/// claim that a particular hardware block consumes the packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothLeTestPduHeader(u8);

impl BluetoothLeTestPduHeader {
    const fn without_cte(payload_type: u8) -> Result<Self, BluetoothDtmTxPacketPrepareError> {
        if payload_type <= 7 {
            Ok(Self(payload_type))
        } else {
            Err(BluetoothDtmTxPacketPrepareError::UnsupportedPayloadType)
        }
    }

    const fn controller_image(self) -> u8 {
        self.0
    }
}

/// CPU-owned RX packet allocation with only the reviewed DTM defaults exposed.
#[repr(C, align(4))]
struct BluetoothDtmRxPacketStorage {
    words: [VolatileCell<u32>; RX_PACKET_WORDS],
}

impl BluetoothDtmRxPacketStorage {
    const CAPACITY_WORD: usize = 1;
    const RESULT_WORD: usize = 3;
    const AUXILIARY_REARM_WORD: usize = 6;
    const RESULT_REARM_SENTINEL: u32 = 0x00ff_ffff;
    const AUXILIARY_REARM_SENTINEL: u32 = 0x0000_ffff;

    /// Create one zero-based slot and apply the reviewed maximum-capacity
    /// allocation and re-arm images.
    const fn new() -> Self {
        let mut storage = Self {
            words: [const { VolatileCell::new(0) }; RX_PACKET_WORDS],
        };
        storage.words[Self::CAPACITY_WORD] = VolatileCell::new(0x0001_0100);
        storage.words[Self::RESULT_WORD] = VolatileCell::new(Self::RESULT_REARM_SENTINEL);
        storage.words[Self::AUXILIARY_REARM_WORD] =
            VolatileCell::new(Self::AUXILIARY_REARM_SENTINEL);
        storage
    }

    fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> [u8; BLUETOOTH_DTM_RX_PACKET_BYTES] {
        core::array::from_fn(|index| {
            let shift = (index % 4) * 8;
            (self.read_word(index / 4) >> shift) as u8
        })
    }

    /// Reapply the exact returned-buffer default values before a future append.
    ///
    /// The low three bytes of word `+0x0c` become `0xff`; byte `+0x0f` is
    /// deliberately retained. Halfword `+0x18` becomes `0xffff`.
    fn rearm_reviewed_packet_fields(&self) {
        self.write_word(
            Self::RESULT_WORD,
            self.read_word(Self::RESULT_WORD) | Self::RESULT_REARM_SENTINEL,
        );
        self.write_word(
            Self::AUXILIARY_REARM_WORD,
            self.read_word(Self::AUXILIARY_REARM_WORD) | Self::AUXILIARY_REARM_SENTINEL,
        );
    }

    fn initialize_reviewed_allocation_fields(&self) {
        self.write_word(Self::CAPACITY_WORD, 0x0001_0100);
        self.rearm_reviewed_packet_fields();
    }

    /// Observe controller-written packet fields only through the retained
    /// post-report owner. `BluetoothDtmMemoryGraphRecyclePrepared` contains the
    /// exact running graph after its finished-list report fence and the later
    /// software-list removal proof, so no CPU-owned or merely published graph
    /// can construct this observation.
    fn observe_after_fenced_completion(
        &self,
        _fenced: &BluetoothDtmMemoryGraphRecyclePrepared,
    ) -> Result<
        BluetoothDtmRxPacketCompletionObservation,
        BluetoothDtmRxPacketCompletionObservationError,
    > {
        let result_word = self.read_word(Self::RESULT_WORD);
        if result_word & Self::RESULT_REARM_SENTINEL == Self::RESULT_REARM_SENTINEL {
            return Err(BluetoothDtmRxPacketCompletionObservationError::ResultNotProduced);
        }
        if self.read_word(Self::AUXILIARY_REARM_WORD) & Self::AUXILIARY_REARM_SENTINEL
            == Self::AUXILIARY_REARM_SENTINEL
        {
            return Err(BluetoothDtmRxPacketCompletionObservationError::AuxiliaryNotProduced);
        }
        Ok(BluetoothDtmRxPacketCompletionObservation {
            projection: BluetoothDtmRxResultProjection::from_word(result_word),
        })
    }

    #[cfg(test)]
    fn model_controller_completion(&self, result_word: u32, auxiliary: u16) {
        self.write_word(Self::RESULT_WORD, result_word);
        self.write_word(Self::AUXILIARY_REARM_WORD, u32::from(auxiliary));
    }
}

#[derive(Clone, Copy)]
struct BluetoothDtmRxPacketCompletionObservation {
    projection: Result<BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError>,
}

impl BluetoothDtmRxPacketCompletionObservation {
    const fn projection(
        self,
    ) -> Result<BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError> {
        self.projection
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothDtmRxPacketCompletionObservationError {
    ResultNotProduced,
    AuxiliaryNotProduced,
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
/// that publishes a raw address or creates a head-published state.
#[pin_project]
#[repr(C)]
pub struct BluetoothDtmMemoryGraphStorage {
    link_state: BluetoothDtmLinkStateStorage,
    scheduler_context: BluetoothDtmSchedulerContextStorage,
    scheduler_item: BluetoothDtmSchedulerItemStorage,
    rx_header: BluetoothDtmRxBufferHeaderStorage,
    rx_swap_reserve: BluetoothDtmRxBufferHeaderStorage,
    tx_header: BluetoothLeTxBufferHeaderStorage,
    tx_packet: BluetoothLeTxPacketStorage<BLUETOOTH_DTM_TX_PACKET_BYTES>,
    rx_packet: BluetoothDtmRxPacketStorage,
    #[pin]
    _pin: PhantomPinned,
}

const BLUETOOTH_DTM_MEMORY_GRAPH_BYTES: u32 =
    core::mem::size_of::<BluetoothDtmMemoryGraphStorage>() as u32;
const LINK_STATE_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, link_state) as u32;
const SCHEDULER_CONTEXT_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_context) as u32;
const SCHEDULER_ITEM_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, scheduler_item) as u32;
const RX_HEADER_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_header) as u32;
const RX_SWAP_RESERVE_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_swap_reserve) as u32;
const TX_HEADER_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, tx_header) as u32;
const TX_PACKET_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, tx_packet) as u32;
const RX_PACKET_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothDtmMemoryGraphStorage, rx_packet) as u32;

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

/// Opaque identity of one exact statically pinned DTM graph storage object.
///
/// This value is an equality witness only: it exposes neither the storage
/// pointer nor controller-SRAM addresses and grants no access or publication
/// authority. The memory layer mints it while consuming the unique static
/// storage borrow, and every graph typestate retains it inside its binding.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct BluetoothDtmMemoryGraphIdentity(usize);

impl BluetoothDtmMemoryGraphIdentity {
    fn for_storage(storage: &BluetoothDtmMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

/// Non-forgeable address binding retained by one CPU-owned static graph.
///
/// The value proves that every contained allocation lies in physical internal
/// SRAM and matches this crate's exact `repr(C)` layout. It is intentionally
/// not `Clone` or `Copy`; obtaining compressed component addresses does not
/// grant controller publication authority.
pub struct BluetoothDtmMemoryGraphBinding {
    #[cfg(not(target_arch = "riscv32"))]
    identity: BluetoothDtmMemoryGraphIdentity,
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    allocation_config: BluetoothDtmSchedulerAllocationConfig,
    link_state: BluetoothControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramAddress,
    scheduler_item: BluetoothControllerSramLinkAddress,
    rx_header: BluetoothControllerSramLinkAddress,
    rx_swap_reserve: BluetoothControllerSramLinkAddress,
    tx_header: BluetoothControllerSramLinkAddress,
    tx_packet: BluetoothDtmTxPacketAddress,
    rx_packet: BluetoothDtmRxPacketAddress,
}

impl BluetoothDtmMemoryGraphBinding {
    fn new(
        identity: BluetoothDtmMemoryGraphIdentity,
        base: u32,
        allocation_config: BluetoothDtmSchedulerAllocationConfig,
    ) -> Result<Self, BluetoothDtmMemoryGraphBindError> {
        #[cfg(target_arch = "riscv32")]
        let _ = identity;
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
            BluetoothControllerSramLinkAddress::new(address(offset)?)
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
            #[cfg(not(target_arch = "riscv32"))]
            identity,
            base: base_address,
            end_exclusive: base + BLUETOOTH_DTM_MEMORY_GRAPH_BYTES,
            allocation_config,
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

    /// Return the opaque identity of this exact pinned storage object.
    pub const fn identity(&self) -> BluetoothDtmMemoryGraphIdentity {
        #[cfg(target_arch = "riscv32")]
        {
            BluetoothDtmMemoryGraphIdentity(self.base.address() as usize)
        }
        #[cfg(not(target_arch = "riscv32"))]
        {
            self.identity
        }
    }

    /// Return the complete physical SRAM range occupied by this graph.
    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    /// Return the semantic allocator inputs permanently bound to this graph.
    ///
    /// Reinitialization uses this retained value, so upper composition can
    /// report the effective policy without keeping a second, cross-wireable
    /// copy beside the graph.
    pub const fn allocation_config(&self) -> BluetoothDtmSchedulerAllocationConfig {
        self.allocation_config
    }

    /// Return the address of the private DTM link-state allocation.
    pub const fn link_state_address(&self) -> BluetoothControllerSramLinkAddress {
        self.link_state
    }

    /// Return the address of the separate CPU-owned scheduler context.
    pub const fn scheduler_context_address(&self) -> BluetoothControllerSramAddress {
        self.scheduler_context
    }

    /// Return the address of the private DTM scheduler item.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramLinkAddress {
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
    tx_header_head: BluetoothDtmTxHeaderHeadProjection,
    rx_header_tail: BluetoothDtmRxHeaderTailProjection,
}

impl BluetoothDtmPositionalEventSeed {
    /// Return the current values of exactly the nineteen writable words.
    pub const fn words(self) -> BluetoothDtmPositionalEventWords {
        self.words
    }

    /// Return the current graph-bound TX-header head projection.
    pub const fn tx_header_head_projection(self) -> BluetoothDtmTxHeaderHeadProjection {
        self.tx_header_head
    }

    /// Return the current graph-bound RX-header tail projection.
    pub const fn rx_header_tail_projection(self) -> BluetoothDtmRxHeaderTailProjection {
        self.rx_header_tail
    }
}

/// Why CPU-owned positional event words were not committed to the graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphPrepareError<BuildError = Infallible> {
    /// The upper builder rejected its semantic inputs before any graph write.
    Build(BuildError),
    /// The current private TX-head word contains the unbound zero link.
    CurrentTxHeadUnbound,
    /// The current private TX head does not name this graph's bound TX header.
    CurrentTxHeadIdentityMismatch,
    /// The selected TX header no longer names this graph's packet allocation.
    CurrentTxHeaderPacketBaseMismatch,
    /// The selected TX header no longer names this graph's LE Test PDU.
    CurrentTxHeaderPduTargetMismatch,
    /// The selected TX header lost the reviewed full-capacity allocation profile.
    CurrentTxHeaderAllocationExtentMismatch,
    /// The current private RX-tail word contains the unbound zero link.
    CurrentRxTailUnbound,
    /// The current private RX tail names neither of this graph's two RX headers.
    CurrentRxTailIdentityMismatch,
    /// The selected RX-tail header no longer names this graph's packet allocation.
    CurrentRxTailPacketMismatch,
    /// Link-state `+0x00` does not retain this graph's freshly sampled TX head.
    LinkStateTxHeadMismatch {
        /// Current private-chain projection required by this graph.
        expected: BluetoothDtmTxHeaderHeadProjection,
        /// Candidate projection returned by the builder.
        observed: BluetoothDtmTxHeaderHeadProjection,
    },
    /// Link-state `+0x08` does not retain this graph's freshly sampled RX tail.
    LinkStateRxTailMismatch {
        /// Current private-chain projection required by this graph.
        expected: BluetoothDtmRxHeaderTailProjection,
        /// Candidate projection returned by the builder.
        observed: BluetoothDtmRxHeaderTailProjection,
    },
    /// Scheduler-item `+0x08` no longer points to this graph's link-state.
    SchedulerItemLinkStateMismatch,
}

/// Failed positional preparation retaining the exact CPU-owned graph.
///
/// Current header-to-packet bindings, builder execution and all three
/// candidate descriptor anchors are checked before the first backing-storage
/// write. `into_parts` therefore returns the byte-unchanged owner for retry or
/// explicit shutdown.
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
/// This state proves that the selected TX/RX headers retain this graph's
/// packet allocations, that the TX header retains its PDU target and reviewed
/// allocation extent, and that the candidate retained the three descriptor
/// links. It does not by itself prove fresh TX packet contents and has no list
/// insertion, fence, publication or hardware-ownership authority.
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
    /// hardware-consumed descriptor, release fence and scheduler-head
    /// publication are still separate prerequisites.
    pub fn prepare_scheduler_bookkeeping(
        mut self,
    ) -> BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
        let storage = self.storage.as_mut().project().scheduler_item;
        let previous_control = storage.read_word(SCHEDULER_ITEM_CONTROL_OFFSET);
        let previous_status = storage.read_word(SCHEDULER_ITEM_STATUS_OFFSET);
        let previous_completed_link = storage.read_word(SCHEDULER_ITEM_COMPLETED_LINK_OFFSET);

        let mut control = previous_control.to_le_bytes();
        control[SCHEDULER_ITEM_CONTROL_BYTE] = 0;
        storage.write_word(SCHEDULER_ITEM_CONTROL_OFFSET, u32::from_le_bytes(control));
        storage.write_word(SCHEDULER_ITEM_STATUS_OFFSET, u32::MAX);
        storage.write_word(SCHEDULER_ITEM_COMPLETED_LINK_OFFSET, 0);

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
        let previous_hardware_chain = storage.terminate_hardware_chain();
        let previous_software_next = storage.read_word(SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET);
        storage.write_word(SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET, 0);

        BluetoothDtmMemoryGraphEmptyListLinkPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
            previous_control: self.previous_control,
            previous_status: self.previous_status,
            previous_completed_link: self.previous_completed_link,
            previous_hardware_chain,
            previous_software_next,
        }
    }

    /// Cancel before publication and recover the positional event owner.
    pub fn cancel(mut self) -> BluetoothDtmMemoryGraphPositionalEventPrepared {
        let storage = self.storage.as_mut().project().scheduler_item;
        storage.write_word(SCHEDULER_ITEM_CONTROL_OFFSET, self.previous_control);
        storage.write_word(SCHEDULER_ITEM_STATUS_OFFSET, self.previous_status);
        storage.write_word(
            SCHEDULER_ITEM_COMPLETED_LINK_OFFSET,
            self.previous_completed_link,
        );

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
    previous_hardware_chain: BluetoothDtmSchedulerHardwareChainWord,
    previous_software_next: u32,
}

impl BluetoothDtmMemoryGraphEmptyListLinkPrepared {
    /// Return the exact scheduler-item identity retained by this graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Consume CPU rollback ownership after the exact list head is published.
    ///
    /// This transition accepts only the affine PAC publication proof for DTM's
    /// fixed list zero and verifies that its head names this pinned graph. The
    /// resulting owner deliberately discards every rollback image: hardware
    /// may already retain and mutate the graph, so cancellation and CPU-side
    /// preparation must no longer be expressible.
    #[doc(hidden)]
    pub fn into_head_published(
        self,
        publication: &BluetoothSchedulerHardwareListHeadPublished,
    ) -> BluetoothDtmMemoryGraphHeadPublished {
        assert_eq!(
            publication.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "a DTM graph can only consume its fixed hardware-list publication"
        );
        assert_eq!(
            publication.head().address(),
            Some(self.scheduler_item_address()),
            "the published scheduler head must name the retained DTM graph"
        );

        BluetoothDtmMemoryGraphHeadPublished {
            storage: self.storage,
            binding: self.binding,
        }
    }

    /// Cancel before visibility or publication and recover bookkeeping state.
    pub fn cancel(mut self) -> BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
        let scheduler_item = self.storage.as_mut().project().scheduler_item;
        scheduler_item.write_hardware_chain_word(self.previous_hardware_chain);
        scheduler_item.write_word(
            SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET,
            self.previous_software_next,
        );

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

/// Pinned DTM graph visible to and exclusively retained for controller use.
///
/// The matching affine PAC publication has completed both visibility fences
/// and installed this graph as hardware-list zero's head. This type exposes
/// identity only: it grants neither CPU mutation nor completion visibility,
/// and it has no cancellation or conversion back into a prepared owner.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphHeadPublished;
/// use open_esp_radio_esp32s31_hal::BluetoothSchedulerFinishedHardwareListObserved;
///
/// fn observe_before_run(
///     graph: BluetoothDtmMemoryGraphHeadPublished,
///     observed: BluetoothSchedulerFinishedHardwareListObserved,
/// ) {
///     graph.observe_completion(observed);
/// }
/// ```
#[must_use = "the published graph must either reach RUN or remain fail-stop owned"]
pub struct BluetoothDtmMemoryGraphHeadPublished {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

impl BluetoothDtmMemoryGraphHeadPublished {
    /// Return the exact scheduler-item identity visible through the list head.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Consume the exact RUN proof and admit completion observation.
    ///
    /// Head publication alone only makes the graph controller-visible. This
    /// transition requires the complete interrupt-preparation and RUN suffix
    /// for the same list and head before hardware-written status can be read.
    #[doc(hidden)]
    pub fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothDtmMemoryGraphRunning {
        assert_eq!(
            run.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "a DTM graph can only run on its fixed hardware list"
        );
        assert_eq!(
            run.head().address(),
            Some(self.scheduler_item_address()),
            "the RUN proof must retain the published DTM graph"
        );

        BluetoothDtmMemoryGraphRunning {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// Pinned DTM graph admitted through the complete scheduler RUN transaction.
///
/// This is the first memory state that may consume a fenced finished-list
/// observation. It still exposes no CPU mutation or cancellation path.
#[must_use = "the running graph must advance through proven completion"]
pub struct BluetoothDtmMemoryGraphRunning {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

impl BluetoothDtmMemoryGraphRunning {
    /// Return the exact scheduler-item identity retained during execution.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Observe the sole DTM item's status after one affine finished-list event.
    ///
    /// The current source-owned scheduler epoch assigns DTM exclusively to
    /// list zero and admits exactly one item. A token for another list returns
    /// both owners unchanged and performs no status read. A matching token is
    /// consumed by exactly one volatile load after the PAC transfer's trailing
    /// device fence. The in-flight sentinel retains hardware ownership; every
    /// other value advances only to completion-observed, not CPU-owned.
    #[doc(hidden)]
    pub fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothDtmMemoryGraphCompletionObservation {
        if observed.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            return BluetoothDtmMemoryGraphCompletionObservation::ListMismatch {
                owner: self,
                observed,
            };
        }

        let status = self
            .storage
            .as_ref()
            .get_ref()
            .scheduler_item
            .read_word(SCHEDULER_ITEM_STATUS_OFFSET);
        if status == u32::MAX {
            BluetoothDtmMemoryGraphCompletionObservation::StillInFlight(self)
        } else {
            let status = match NonZeroU32::new(status) {
                None => BluetoothDtmSchedulerItemCompletionStatus::Zero,
                Some(status) => BluetoothDtmSchedulerItemCompletionStatus::NonZero(status),
            };
            BluetoothDtmMemoryGraphCompletionObservation::CompletionObserved(
                BluetoothDtmMemoryGraphCompletionObserved {
                    owner: self,
                    status,
                },
            )
        }
    }
}

/// Reviewed DTM interpretation of one non-sentinel scheduler-item status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerItemCompletionStatus {
    /// The role-specific accounting path accepts positional status zero.
    Zero,
    /// Hardware reported a positional nonzero status.
    NonZero(NonZeroU32),
}

/// Hardware-owned graph after a non-sentinel status was observed.
///
/// The descriptor has not yet been unlinked from the hardware list or removed
/// from the software completion queue. This state therefore exposes status and
/// identity only and cannot reclaim, mutate or republish the graph.
#[must_use = "completion observation must advance through unlink and recycle ownership"]
pub struct BluetoothDtmMemoryGraphCompletionObserved {
    owner: BluetoothDtmMemoryGraphRunning,
    status: BluetoothDtmSchedulerItemCompletionStatus,
}

impl BluetoothDtmMemoryGraphCompletionObserved {
    /// Exact item whose status was observed after the fenced transfer.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.owner.scheduler_item_address()
    }

    /// Semantic non-sentinel status retained by this completion observation.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.status
    }

    #[doc(hidden)]
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphRunning,
        BluetoothDtmSchedulerItemCompletionStatus,
    ) {
        (self.owner, self.status)
    }

    /// Prepare reclamation after exact hardware-head retirement and the
    /// post-unlink software-list removal gate without mutating SRAM.
    ///
    /// The two affine lower tokens are consumed together and the retained RUN
    /// head must name this graph on list zero. Success binds them into one
    /// affine prepared transaction; its later infallible commit performs the
    /// reviewed SRAM cleanup and returns the retained status with CPU ownership.
    #[doc(hidden)]
    pub fn prepare_recycle_after_software_list_removal(
        self,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<BluetoothDtmMemoryGraphRecyclePrepared, BluetoothDtmMemoryGraphRecycleFailure> {
        let error = if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(BluetoothDtmMemoryGraphRecycleError::HardwareListMismatch)
        } else if removal.completed_head().address() != Some(self.scheduler_item_address()) {
            Some(BluetoothDtmMemoryGraphRecycleError::SchedulerItemMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(BluetoothDtmMemoryGraphRecycleFailure {
                error,
                completed: self,
                removal,
            });
        }

        Ok(BluetoothDtmMemoryGraphRecyclePrepared {
            completed: self,
            removal,
        })
    }
}

/// Validated but not yet mutated completed-graph recycle transaction.
///
/// This affine owner binds the exact completed graph, retired head and
/// post-unlink removal proof. It can be rolled back losslessly until
/// [`Self::commit`] performs the reviewed SRAM cleanup.
#[must_use = "the prepared recycle must be committed or returned to its owners"]
pub struct BluetoothDtmMemoryGraphRecyclePrepared {
    completed: BluetoothDtmMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl BluetoothDtmMemoryGraphRecyclePrepared {
    /// Recover every unchanged owner before the first SRAM mutation.
    #[doc(hidden)]
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }

    /// Perform the infallible reviewed recycle suffix.
    ///
    /// Vendor order removes the completed-queue link first and then clears the
    /// compressed hardware-next link while preserving its non-link prefix.
    #[doc(hidden)]
    pub fn commit(self) -> BluetoothDtmMemoryGraphRecycleCleaned {
        let Self {
            completed,
            removal: _,
        } = self;
        let BluetoothDtmMemoryGraphCompletionObserved { owner, status } = completed;
        let BluetoothDtmMemoryGraphRunning {
            mut storage,
            binding,
        } = owner;
        let scheduler_item = storage.as_mut().project().scheduler_item;
        scheduler_item.write_word(SCHEDULER_ITEM_COMPLETED_LINK_OFFSET, 0);
        let _ = scheduler_item.terminate_hardware_chain();

        BluetoothDtmMemoryGraphRecycleCleaned {
            storage,
            binding,
            status,
        }
    }

    /// Validate the exact two-header RX-success topology without mutating it.
    ///
    /// The finished-list fence and removal proof are already retained by this
    /// transaction. This operation additionally proves either that no packet
    /// was returned or that one completed tail can execute the bounded
    /// two-slot rotation. Every malformed topology is returned unchanged.
    pub fn prepare_receiver_success(
        self,
    ) -> Result<
        BluetoothDtmMemoryGraphRxSuccessRecyclePrepared,
        BluetoothDtmMemoryGraphRxSuccessRecycleFailure,
    > {
        let plan = match BluetoothDtmRxRotationPlan::validate(&self) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(BluetoothDtmMemoryGraphRxSuccessRecycleFailure {
                    error,
                    recycle: self,
                });
            }
        };
        Ok(BluetoothDtmMemoryGraphRxSuccessRecyclePrepared {
            recycle: self,
            plan,
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BluetoothDtmRxHeaderSlot {
    Primary,
    Swap,
}

impl BluetoothDtmRxHeaderSlot {
    const fn other(self) -> Self {
        match self {
            Self::Primary => Self::Swap,
            Self::Swap => Self::Primary,
        }
    }
}

#[derive(Clone, Copy)]
enum BluetoothDtmRxRotationPlan {
    NoReturnedPacket {
        predecessor: Option<BluetoothDtmRxHeaderSlot>,
    },
    Rotate {
        returned: BluetoothDtmRxHeaderSlot,
        copy_target: BluetoothDtmRxHeaderSlot,
        steady: bool,
        packet: BluetoothDtmRxPacketCompletionObservation,
    },
}

impl BluetoothDtmRxRotationPlan {
    fn validate(
        recycle: &BluetoothDtmMemoryGraphRecyclePrepared,
    ) -> Result<Self, BluetoothDtmMemoryGraphRxSuccessRecycleError> {
        if recycle.completed.status() != BluetoothDtmSchedulerItemCompletionStatus::Zero {
            return Err(BluetoothDtmMemoryGraphRxSuccessRecycleError::CompletionStatusMismatch);
        }
        let owner = &recycle.completed.owner;
        let storage = owner.storage.as_ref().get_ref();
        let binding = &owner.binding;
        let primary_address = binding.rx_header.controller_address().address();
        let swap_address = binding.rx_swap_reserve.controller_address().address();
        let slot = |address| {
            if address == primary_address {
                Some(BluetoothDtmRxHeaderSlot::Primary)
            } else if address == swap_address {
                Some(BluetoothDtmRxHeaderSlot::Swap)
            } else {
                None
            }
        };
        let head = slot(storage.link_state.read_word(LINK_STATE_RX_HEAD_OFFSET))
            .ok_or(BluetoothDtmMemoryGraphRxSuccessRecycleError::RxHeadIdentityMismatch)?;
        let tail = slot(storage.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET))
            .ok_or(BluetoothDtmMemoryGraphRxSuccessRecycleError::RxTailIdentityMismatch)?;
        let reserve_address = storage
            .link_state
            .read_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET);
        let header = |slot| match slot {
            BluetoothDtmRxHeaderSlot::Primary => &storage.rx_header,
            BluetoothDtmRxHeaderSlot::Swap => &storage.rx_swap_reserve,
        };
        let successor = |slot| match slot {
            BluetoothDtmRxHeaderSlot::Primary => {
                BluetoothDtmRxHeaderSuccessorLink::from_bound(binding.rx_header)
            }
            BluetoothDtmRxHeaderSlot::Swap => {
                BluetoothDtmRxHeaderSuccessorLink::from_bound(binding.rx_swap_reserve)
            }
        };
        let backlink = |slot| match slot {
            BluetoothDtmRxHeaderSlot::Primary => {
                BluetoothDtmRxHeaderBacklink::bound(binding.rx_header.controller_address())
            }
            BluetoothDtmRxHeaderSlot::Swap => {
                BluetoothDtmRxHeaderBacklink::bound(binding.rx_swap_reserve.controller_address())
            }
        };

        let (returned, copy_target, steady) = if head == tail {
            let copy_target = head.other();
            let expected_reserve = match copy_target {
                BluetoothDtmRxHeaderSlot::Primary => primary_address,
                BluetoothDtmRxHeaderSlot::Swap => swap_address,
            };
            if reserve_address != expected_reserve {
                return Err(BluetoothDtmMemoryGraphRxSuccessRecycleError::RxSwapIdentityMismatch);
            }
            let reserve = header(copy_target);
            if reserve.rx_successor_link().is_some() || reserve.rx_packet_link().is_some() {
                return Err(BluetoothDtmMemoryGraphRxSuccessRecycleError::ReserveNotDetached);
            }
            if header(tail).rx_backlink().is_some() {
                return Err(
                    BluetoothDtmMemoryGraphRxSuccessRecycleError::InitialBacklinkUnexpected,
                );
            }
            (tail, copy_target, false)
        } else {
            if reserve_address != 0 {
                return Err(BluetoothDtmMemoryGraphRxSuccessRecycleError::SwapReserveUnexpected);
            }
            let predecessor = header(head);
            if predecessor.rx_packet_link().is_some() {
                return Err(
                    BluetoothDtmMemoryGraphRxSuccessRecycleError::PredecessorPacketStillBound,
                );
            }
            if predecessor.rx_successor_link() != Some(successor(tail)) {
                return Err(
                    BluetoothDtmMemoryGraphRxSuccessRecycleError::PredecessorSuccessorMismatch,
                );
            }
            if !predecessor
                .observe_rx_completion_after_fence(recycle)
                .is_completed()
            {
                return Err(BluetoothDtmMemoryGraphRxSuccessRecycleError::PredecessorNotCompleted);
            }
            if header(tail).rx_backlink() != Some(backlink(head)) {
                return Err(
                    BluetoothDtmMemoryGraphRxSuccessRecycleError::SuccessorBacklinkMismatch,
                );
            }
            (tail, head, true)
        };

        let returned_header = header(returned);
        if returned_header.rx_successor_link().is_some() {
            return Err(BluetoothDtmMemoryGraphRxSuccessRecycleError::ReturnedHasSuccessor);
        }
        if returned_header.rx_packet_link()
            != Some(BluetoothDtmRxPacketLink::from_bound(binding.rx_packet))
        {
            return Err(BluetoothDtmMemoryGraphRxSuccessRecycleError::ReturnedPacketMismatch);
        }
        if !returned_header
            .observe_rx_completion_after_fence(recycle)
            .is_completed()
        {
            return Ok(Self::NoReturnedPacket {
                predecessor: steady.then_some(copy_target),
            });
        }

        let packet = storage
            .rx_packet
            .observe_after_fenced_completion(recycle)
            .map_err(|error| match error {
                BluetoothDtmRxPacketCompletionObservationError::ResultNotProduced => {
                    BluetoothDtmMemoryGraphRxSuccessRecycleError::ReturnedResultNotProduced
                }
                BluetoothDtmRxPacketCompletionObservationError::AuxiliaryNotProduced => {
                    BluetoothDtmMemoryGraphRxSuccessRecycleError::ReturnedAuxiliaryNotProduced
                }
            })?;
        Ok(Self::Rotate {
            returned,
            copy_target,
            steady,
            packet,
        })
    }
}

/// Validated RX-success recycle retaining the exact two-slot rotation plan.
#[must_use = "the RX-success recycle must be committed or returned unchanged"]
pub struct BluetoothDtmMemoryGraphRxSuccessRecyclePrepared {
    recycle: BluetoothDtmMemoryGraphRecyclePrepared,
    plan: BluetoothDtmRxRotationPlan,
}

impl BluetoothDtmMemoryGraphRxSuccessRecyclePrepared {
    /// Recover the byte-unchanged generic recycle transaction.
    pub fn into_recycle_prepared(self) -> BluetoothDtmMemoryGraphRecyclePrepared {
        self.recycle
    }

    /// Commit generic scheduler cleanup and consume the validated returned-list
    /// read into one affine observation.
    ///
    /// This clears only the already-retired scheduler links. The returned owner
    /// alone can perform the RX append/re-arm suffix after the upper session
    /// consumes the observation. No fallible operation may follow this edge.
    pub fn observe(self) -> BluetoothDtmMemoryGraphRxSuccessObserved {
        let Self { recycle, plan } = self;
        let memory = recycle.commit();
        match plan {
            BluetoothDtmRxRotationPlan::NoReturnedPacket { predecessor } => {
                BluetoothDtmMemoryGraphRxSuccessObserved {
                    kind: BluetoothDtmMemoryGraphRxSuccessObservedKind::NoReturnedPacket(
                        BluetoothDtmMemoryGraphRxNoReturnedPacketObserved {
                            memory,
                            predecessor,
                        },
                    ),
                }
            }
            BluetoothDtmRxRotationPlan::Rotate {
                returned,
                copy_target,
                steady,
                packet,
            } => BluetoothDtmMemoryGraphRxSuccessObserved {
                kind: BluetoothDtmMemoryGraphRxSuccessObservedKind::ReturnedPacket(
                    BluetoothDtmMemoryGraphRxReturnedPacketObserved {
                        memory,
                        returned,
                        copy_target,
                        steady,
                        projection: packet.projection(),
                    },
                ),
            },
        }
    }
}

/// Affine result of the bounded RX returned-list observation.
#[must_use = "the observed RX-success graph must be accounted and re-armed"]
pub struct BluetoothDtmMemoryGraphRxSuccessObserved {
    kind: BluetoothDtmMemoryGraphRxSuccessObservedKind,
}

enum BluetoothDtmMemoryGraphRxSuccessObservedKind {
    NoReturnedPacket(BluetoothDtmMemoryGraphRxNoReturnedPacketObserved),
    ReturnedPacket(BluetoothDtmMemoryGraphRxReturnedPacketObserved),
}

impl BluetoothDtmMemoryGraphRxSuccessObserved {
    /// Consume the semantic observation before the infallible RX re-arm.
    ///
    /// The callback result is retained while this method consumes the sole
    /// graph token. There is no public path that can re-arm either variant
    /// without first executing the callback.
    pub fn consume_then_commit<ResultValue>(
        self,
        consume: impl FnOnce(
            Option<Result<BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError>>,
        ) -> ResultValue,
    ) -> (BluetoothDtmMemoryGraphRecycleCleaned, ResultValue) {
        match self.kind {
            BluetoothDtmMemoryGraphRxSuccessObservedKind::NoReturnedPacket(observed) => {
                let result = consume(None);
                (observed.commit_rearm(), result)
            }
            BluetoothDtmMemoryGraphRxSuccessObservedKind::ReturnedPacket(observed) => {
                let result = consume(Some(observed.projection()));
                (observed.commit_rotation(), result)
            }
        }
    }
}

/// RX-success graph proving that no packet was returned by this event.
#[must_use = "the no-packet RX graph must execute its infallible re-arm suffix"]
struct BluetoothDtmMemoryGraphRxNoReturnedPacketObserved {
    memory: BluetoothDtmMemoryGraphRecycleCleaned,
    predecessor: Option<BluetoothDtmRxHeaderSlot>,
}

impl BluetoothDtmMemoryGraphRxNoReturnedPacketObserved {
    fn commit_rearm(mut self) -> BluetoothDtmMemoryGraphRecycleCleaned {
        if let Some(predecessor) = self.predecessor {
            let storage = self.memory.storage.as_mut().project();
            let header = match predecessor {
                BluetoothDtmRxHeaderSlot::Primary => storage.rx_header,
                BluetoothDtmRxHeaderSlot::Swap => storage.rx_swap_reserve,
            };
            header.set_rx_software_terminal(true);
        }
        self.memory
    }
}

/// RX-success graph binding one result projection to its exact rotation owner.
#[must_use = "the returned packet must be accounted before its rotation owner is consumed"]
struct BluetoothDtmMemoryGraphRxReturnedPacketObserved {
    memory: BluetoothDtmMemoryGraphRecycleCleaned,
    returned: BluetoothDtmRxHeaderSlot,
    copy_target: BluetoothDtmRxHeaderSlot,
    steady: bool,
    projection: Result<BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError>,
}

impl BluetoothDtmMemoryGraphRxReturnedPacketObserved {
    const fn projection(
        &self,
    ) -> Result<BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError> {
        self.projection
    }

    fn commit_rotation(mut self) -> BluetoothDtmMemoryGraphRecycleCleaned {
        let memory = &mut self.memory;
        let storage = memory.storage.as_mut().project();
        let primary = &*storage.rx_header;
        let swap = &*storage.rx_swap_reserve;
        let returned_header = match self.returned {
            BluetoothDtmRxHeaderSlot::Primary => primary,
            BluetoothDtmRxHeaderSlot::Swap => swap,
        };
        let copy_header = match self.copy_target {
            BluetoothDtmRxHeaderSlot::Primary => primary,
            BluetoothDtmRxHeaderSlot::Swap => swap,
        };
        let returned_address = match self.returned {
            BluetoothDtmRxHeaderSlot::Primary => {
                memory.binding.rx_header.controller_address().address()
            }
            BluetoothDtmRxHeaderSlot::Swap => memory
                .binding
                .rx_swap_reserve
                .controller_address()
                .address(),
        };
        let copy_address = match self.copy_target {
            BluetoothDtmRxHeaderSlot::Primary => {
                memory.binding.rx_header.controller_address().address()
            }
            BluetoothDtmRxHeaderSlot::Swap => memory
                .binding
                .rx_swap_reserve
                .controller_address()
                .address(),
        };
        let copy_link = match self.copy_target {
            BluetoothDtmRxHeaderSlot::Primary => {
                BluetoothDtmRxHeaderSuccessorLink::from_bound(memory.binding.rx_header)
            }
            BluetoothDtmRxHeaderSlot::Swap => {
                BluetoothDtmRxHeaderSuccessorLink::from_bound(memory.binding.rx_swap_reserve)
            }
        };
        let returned_backlink = match self.returned {
            BluetoothDtmRxHeaderSlot::Primary => {
                BluetoothDtmRxHeaderBacklink::bound(memory.binding.rx_header.controller_address())
            }
            BluetoothDtmRxHeaderSlot::Swap => BluetoothDtmRxHeaderBacklink::bound(
                memory.binding.rx_swap_reserve.controller_address(),
            ),
        };

        if self.steady {
            storage
                .link_state
                .write_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET, copy_address);
            storage
                .link_state
                .write_word(LINK_STATE_RX_HEAD_OFFSET, returned_address);
            returned_header.set_rx_backlink(None);
        }
        returned_header.set_rx_software_terminal(false);
        copy_header.copy_complete_rx_image_from(returned_header);
        returned_header.clear_rx_packet_link();
        storage
            .link_state
            .write_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET, 0);
        copy_header.set_rx_backlink(None);
        storage.rx_packet.rearm_reviewed_packet_fields();
        copy_header.clear_rx_completion_observation();
        copy_header.set_rx_successor_link(None);
        copy_header.set_rx_backlink(None);
        returned_header.set_rx_successor_link(Some(copy_link));
        storage
            .link_state
            .write_word(LINK_STATE_RX_TAIL_OFFSET, copy_address);
        returned_header.set_rx_software_terminal(true);
        copy_header.set_rx_backlink(Some(returned_backlink));
        self.memory
    }
}

/// Why an RX-success graph cannot enter the two-slot recycle suffix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphRxSuccessRecycleError {
    /// The specialized path received a non-success scheduler status.
    CompletionStatusMismatch,
    /// The private RX head names neither bound header slot.
    RxHeadIdentityMismatch,
    /// The private RX tail names neither bound header slot.
    RxTailIdentityMismatch,
    /// The detached reserve does not name the other bound header slot.
    RxSwapIdentityMismatch,
    /// The initial detached reserve still carries a list or packet link.
    ReserveNotDetached,
    /// The initial packet-bearing header unexpectedly carries a backlink.
    InitialBacklinkUnexpected,
    /// A steady two-header chain unexpectedly retains a detached reserve.
    SwapReserveUnexpected,
    /// The steady predecessor still owns the packet.
    PredecessorPacketStillBound,
    /// The steady predecessor does not lead to the packet-bearing tail.
    PredecessorSuccessorMismatch,
    /// The steady predecessor was not returned by the prior event.
    PredecessorNotCompleted,
    /// The steady tail backlink does not name its exact predecessor.
    SuccessorBacklinkMismatch,
    /// The packet-bearing returned tail unexpectedly has a successor.
    ReturnedHasSuccessor,
    /// The returned tail does not retain this graph's sole RX packet.
    ReturnedPacketMismatch,
    /// Hardware left the packet result in its re-arm sentinel state.
    ReturnedResultNotProduced,
    /// Hardware left the positional auxiliary halfword in its re-arm sentinel state.
    ReturnedAuxiliaryNotProduced,
}

/// Lossless RX-success preflight rejection.
#[must_use = "RX-success rejection retains the unchanged recycle transaction"]
pub struct BluetoothDtmMemoryGraphRxSuccessRecycleFailure {
    error: BluetoothDtmMemoryGraphRxSuccessRecycleError,
    recycle: BluetoothDtmMemoryGraphRecyclePrepared,
}

impl BluetoothDtmMemoryGraphRxSuccessRecycleFailure {
    /// Return the semantic reason preflight rejected the unchanged graph.
    pub const fn error(&self) -> BluetoothDtmMemoryGraphRxSuccessRecycleError {
        self.error
    }

    /// Recover the byte-unchanged generic recycle transaction.
    pub fn into_recycle_prepared(self) -> BluetoothDtmMemoryGraphRecyclePrepared {
        self.recycle
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphRxSuccessRecycleFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphRxSuccessRecycleFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Graph whose reviewed hardware-release and SRAM recycle suffix is complete.
///
/// At this lower memory boundary the consumed empty-head/removal proof already
/// establishes that hardware can no longer reach the graph. The production
/// Controller deliberately keeps this intermediate opaque while it releases
/// its separate software timeline and source-list bookkeeping.
#[must_use = "the cleaned graph must return to its memory owner"]
pub struct BluetoothDtmMemoryGraphRecycleCleaned {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    status: BluetoothDtmSchedulerItemCompletionStatus,
}

impl BluetoothDtmMemoryGraphRecycleCleaned {
    /// Recover lower-layer CPU memory ownership after hardware release.
    ///
    /// This does not claim that any upper scheduler timeline or source-list
    /// bookkeeping has been released.
    #[doc(hidden)]
    pub fn into_cpu_owned(self) -> BluetoothDtmMemoryGraphRecycled {
        BluetoothDtmMemoryGraphRecycled {
            owner: BluetoothDtmMemoryGraphCpuOwned {
                storage: self.storage,
                binding: self.binding,
            },
            status: self.status,
        }
    }
}

/// Why the completed memory graph rejected recycle authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphRecycleError {
    /// The empty-head proof belongs to a hardware list other than DTM list zero.
    HardwareListMismatch,
    /// The retained RUN head names a different scheduler item.
    SchedulerItemMismatch,
}

/// Lossless rejection of one completed-graph recycle attempt.
#[must_use = "recycle rejection retains the graph and both affine authorizations"]
pub struct BluetoothDtmMemoryGraphRecycleFailure {
    error: BluetoothDtmMemoryGraphRecycleError,
    completed: BluetoothDtmMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl BluetoothDtmMemoryGraphRecycleFailure {
    /// Exact identity mismatch that prevented CPU ownership return.
    pub const fn error(&self) -> BluetoothDtmMemoryGraphRecycleError {
        self.error
    }

    /// Recover the unchanged completion and both affine authorizations.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

/// CPU-owned graph after the reviewed scheduler-item recycle suffix.
#[must_use = "the recycled graph and completion status must reach the role owner"]
pub struct BluetoothDtmMemoryGraphRecycled {
    owner: BluetoothDtmMemoryGraphCpuOwned,
    status: BluetoothDtmSchedulerItemCompletionStatus,
}

impl BluetoothDtmMemoryGraphRecycled {
    /// Recover the ordinary CPU graph and its retained completion status.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmSchedulerItemCompletionStatus,
    ) {
        (self.owner, self.status)
    }
}

/// Result of matching one fenced finished-list observation to the DTM graph.
#[must_use = "the retained hardware graph and any unmatched list token must be handled"]
pub enum BluetoothDtmMemoryGraphCompletionObservation {
    /// The token belongs to another list; neither owner was consumed.
    ListMismatch {
        /// Unchanged running DTM owner.
        owner: BluetoothDtmMemoryGraphRunning,
        /// Unchanged affine observation for its actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// List zero was observed but hardware still reports the in-flight sentinel.
    StillInFlight(BluetoothDtmMemoryGraphRunning),
    /// A non-sentinel status was observed after the transfer fence.
    CompletionObserved(BluetoothDtmMemoryGraphCompletionObserved),
}

/// Unique CPU owner of one statically located and allocation-initialized graph.
///
/// This state is still unreachable by hardware. It contains no `arm`,
/// `publish`, raw-pointer or head-published transition; the future LLL must
/// first prove the remaining descriptor contract and visibility fences.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::{
///     BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
/// };
///
/// let storage = Box::leak(Box::new(BluetoothDtmMemoryGraphStorage::new()));
/// let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100).unwrap();
/// let config = open_esp_radio_esp32s31_bluetooth_memory::
///     BluetoothDtmSchedulerAllocationConfig::new(1, 1, 1);
/// let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, config).unwrap();
/// let moved = owner;
/// let _binding = owner.binding();
/// drop(moved);
/// ```
pub struct BluetoothDtmMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

/// Ended CPU-owned DTM allocation awaiting either reuse or permanent retention.
///
/// This is not a vendor allocator or `free` operation. The caller's pinned
/// static storage remains owned by this affine value, stays at the same bound
/// address and is not dropped, unpinned or exposed for mutation. Construction
/// is available only from [`BluetoothDtmMemoryGraphCpuOwned`], so a graph still
/// in a prepared, head-published, running, completion-observed or recycle-intermediate
/// state cannot enter this edge.
///
/// An upper Test End owner must first establish its scheduler, callback and
/// source-list quiescence before it exposes the CPU-owned graph consumed here.
/// This lower memory typestate does not fabricate that upper proof.
#[must_use = "the reclaimed static graph must be retained or reinitialized"]
pub struct BluetoothDtmMemoryGraphReclaimed {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

impl BluetoothDtmMemoryGraphReclaimed {
    /// Borrow the immutable graph binding retained across allocation epochs.
    pub const fn binding(&self) -> &BluetoothDtmMemoryGraphBinding {
        &self.binding
    }

    /// Start a fresh CPU-owned allocation epoch in the same pinned storage.
    ///
    /// Reinitialization consumes the reclaimed token exactly once and installs
    /// the complete reviewed allocation defaults from the configuration bound
    /// to this graph's first epoch. No caller can cross-wire a configuration
    /// from another graph. This performs no MMIO, fence, hardware publication,
    /// heap allocation or vendor `free`/`alloc` call.
    pub fn reinitialize(self) -> BluetoothDtmMemoryGraphCpuOwned {
        let config = self.binding.allocation_config;
        let mut owner = BluetoothDtmMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.initialize_reviewed_allocation(config);
        owner
    }
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
    _pdu_header: BluetoothLeTestPduHeader,
    packet_length: BluetoothLeTxPacketPreparedLength<BLUETOOTH_DTM_TX_PACKET_BYTES>,
}

impl BluetoothDtmMemoryGraphTxPacketPrepared {
    /// Return the declared packet payload length.
    pub const fn payload_length(&self) -> u8 {
        self.packet_length.payload_bytes()
    }

    /// Borrow the complete reviewed prefix and declared payload.
    pub fn prepared_packet_bytes(&self) -> &[u8] {
        let storage = self.storage.as_ref().get_ref();
        storage.tx_packet.prepared_allocation(self.packet_length)
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

/// Failed TX packet preparation retaining the byte-unchanged graph owner.
///
/// The LE Test PDU header is validated before any packet byte is written.
pub struct BluetoothDtmMemoryGraphTxPacketPrepareFailure {
    owner: BluetoothDtmMemoryGraphCpuOwned,
    error: BluetoothDtmTxPacketPrepareError,
}

impl BluetoothDtmMemoryGraphTxPacketPrepareFailure {
    /// Return the finite packet-header validation failure.
    pub const fn error(&self) -> BluetoothDtmTxPacketPrepareError {
        self.error
    }

    /// Recover the byte-unchanged owner and exact validation failure.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmTxPacketPrepareError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphTxPacketPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphTxPacketPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BluetoothDtmMemoryGraphCpuOwned {
    /// Borrow the location proof without granting publication authority.
    pub const fn binding(&self) -> &BluetoothDtmMemoryGraphBinding {
        &self.binding
    }

    /// End this CPU-owned graph epoch without releasing its static allocation.
    ///
    /// The transition is intentionally mutation-free. It consumes the only
    /// ordinary CPU owner and removes every event-preparation operation until
    /// [`BluetoothDtmMemoryGraphReclaimed::reinitialize`] starts a fresh epoch.
    /// No equivalent operation exists on any hardware or completion typestate.
    pub fn into_reclaimed(self) -> BluetoothDtmMemoryGraphReclaimed {
        BluetoothDtmMemoryGraphReclaimed {
            storage: self.storage,
            binding: self.binding,
        }
    }

    /// Consume this graph and install one complete CPU-owned TX packet image.
    ///
    /// The LE Test PDU Type is validated before the first mutation; rejection
    /// returns this exact byte-unchanged owner for retry or shutdown.
    ///
    /// The fixed payload array makes the copy total for every accepted `u8`
    /// length; no callback can claim readiness without supplying all declared
    /// bytes. Bytes after the declared payload remain unchanged.
    pub fn prepare_tx_packet(
        mut self,
        payload_type: u8,
        payload_length: u8,
        payload: &[u8; BLUETOOTH_DTM_MAX_PACKET_CAPACITY],
    ) -> Result<
        BluetoothDtmMemoryGraphTxPacketPrepared,
        BluetoothDtmMemoryGraphTxPacketPrepareFailure,
    > {
        let pdu_header = match BluetoothLeTestPduHeader::without_cte(payload_type) {
            Ok(pdu_header) => pdu_header,
            Err(error) => {
                return Err(BluetoothDtmMemoryGraphTxPacketPrepareFailure { owner: self, error });
            }
        };
        let packet = self.storage.as_mut().project().tx_packet;
        let packet_length = packet
            .prepare_pdu(
                pdu_header.controller_image(),
                &payload[..payload_length as usize],
            )
            .unwrap_or_else(|_| {
                unreachable!("the full DTM allocation accepts every eight-bit payload length")
            });

        Ok(BluetoothDtmMemoryGraphTxPacketPrepared {
            storage: self.storage,
            binding: self.binding,
            _pdu_header: pdu_header,
            packet_length,
        })
    }

    /// Validate the complete private packet chain, then build and commit one
    /// role-neutral positional event-word image.
    ///
    /// The consuming closure receives current words and current private links
    /// from this exact owner. Header-binding, builder and descriptor-anchor
    /// failures return the byte-unchanged owner. Once validation succeeds,
    /// committing all nineteen offsets is infallible and remains entirely
    /// CPU-owned.
    pub fn try_prepare_positional_event<BuildError>(
        mut self,
        build: impl FnOnce(
            BluetoothDtmPositionalEventSeed,
        ) -> Result<BluetoothDtmPositionalEventWords, BuildError>,
    ) -> Result<
        BluetoothDtmMemoryGraphPositionalEventPrepared,
        BluetoothDtmMemoryGraphPrepareFailure<BuildError>,
    > {
        let (previous, tx_head_address, rx_tail_address) = {
            let storage = self.storage.as_ref().get_ref();
            (
                BluetoothDtmPositionalEventWords::new(
                    storage.link_state.reviewed_words(),
                    storage.scheduler_item.reviewed_words(),
                ),
                storage.link_state.read_word(LINK_STATE_TX_HEAD_OFFSET),
                storage.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET),
            )
        };
        let tx_head = match BluetoothControllerSramLinkAddress::new(tx_head_address) {
            Ok(tx_head) => tx_head,
            Err(_) => {
                return Err(BluetoothDtmMemoryGraphPrepareFailure {
                    owner: self,
                    error: BluetoothDtmMemoryGraphPrepareError::CurrentTxHeadUnbound,
                });
            }
        };
        if tx_head != self.binding.tx_header {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::CurrentTxHeadIdentityMismatch,
            });
        }
        let storage = self.storage.as_ref().get_ref();
        if storage.tx_header.packet_base_link() != Some(self.binding.tx_packet.base_link()) {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::CurrentTxHeaderPacketBaseMismatch,
            });
        }
        if storage.tx_header.pdu_target_link() != Some(self.binding.tx_packet.pdu_target_link()) {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::CurrentTxHeaderPduTargetMismatch,
            });
        }
        if !storage
            .tx_header
            .retains_allocation_extent(self.binding.tx_packet)
        {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::CurrentTxHeaderAllocationExtentMismatch,
            });
        }
        let rx_tail = match BluetoothControllerSramLinkAddress::new(rx_tail_address) {
            Ok(rx_tail) => rx_tail,
            Err(_) => {
                return Err(BluetoothDtmMemoryGraphPrepareFailure {
                    owner: self,
                    error: BluetoothDtmMemoryGraphPrepareError::CurrentRxTailUnbound,
                });
            }
        };
        if rx_tail != self.binding.rx_header && rx_tail != self.binding.rx_swap_reserve {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::CurrentRxTailIdentityMismatch,
            });
        }
        let selected_rx_tail = if rx_tail == self.binding.rx_header {
            &storage.rx_header
        } else {
            &storage.rx_swap_reserve
        };
        if selected_rx_tail.rx_packet_link()
            != Some(BluetoothDtmRxPacketLink::from_bound(self.binding.rx_packet))
        {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::CurrentRxTailPacketMismatch,
            });
        }
        let tx_header_head = BluetoothDtmTxHeaderHeadProjection::from_bound(tx_head);
        let rx_header_tail = BluetoothDtmRxHeaderTailProjection::from_bound(rx_tail);
        let seed = BluetoothDtmPositionalEventSeed {
            words: previous,
            tx_header_head,
            rx_header_tail,
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

        let observed_tx_head = candidate.tx_header_head_projection();
        if observed_tx_head != tx_header_head {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::LinkStateTxHeadMismatch {
                    expected: tx_header_head,
                    observed: observed_tx_head,
                },
            });
        }
        let observed_rx_tail = candidate.rx_header_tail_projection();
        if observed_rx_tail != rx_header_tail {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::LinkStateRxTailMismatch {
                    expected: rx_header_tail,
                    observed: observed_rx_tail,
                },
            });
        }
        if !candidate.scheduler_item_retains_link_state(self.binding.link_state) {
            return Err(BluetoothDtmMemoryGraphPrepareFailure {
                owner: self,
                error: BluetoothDtmMemoryGraphPrepareError::SchedulerItemLinkStateMismatch,
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
        let link_state = self.binding.link_state.compressed_image();
        let scheduler_context = self.binding.scheduler_context.compressed_image();
        let rx_header_address = self.binding.rx_header.controller_address().address();
        let rx_swap_address = self.binding.rx_swap_reserve.controller_address().address();
        let tx_header_address = self.binding.tx_header.controller_address().address();

        let storage = self.storage.as_mut().project();
        storage.link_state.clear();
        storage.scheduler_context.words.fill(0);
        storage.scheduler_item.clear();
        storage
            .rx_header
            .initialize_bound_rx(self.binding.rx_packet);
        storage.rx_swap_reserve.initialize_rx_swap_reserve();
        storage
            .tx_header
            .initialize_bound_tx(self.binding.tx_packet);
        storage.tx_packet.clear();
        storage.rx_packet.clear();
        storage.rx_packet.initialize_reviewed_allocation_fields();

        storage
            .link_state
            .write_word(LINK_STATE_RX_HEAD_OFFSET, rx_header_address);
        storage
            .link_state
            .write_word(LINK_STATE_TX_HEAD_OFFSET, tx_header_address);
        storage
            .link_state
            .write_word(LINK_STATE_RX_TAIL_OFFSET, rx_header_address);
        storage
            .link_state
            .write_word(LINK_STATE_TX_TAIL_OFFSET, tx_header_address);
        storage
            .link_state
            .write_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET, rx_swap_address);
        storage.link_state.write_word(
            LINK_STATE_ALLOCATION_CONFIG_OFFSET,
            LINK_STATE_ALLOCATION_CONFIG_IMAGE,
        );
        storage.scheduler_item.write_word(
            SCHEDULER_ITEM_ALLOCATION_PREFIX_OFFSET,
            SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE,
        );
        storage
            .scheduler_item
            .write_word(SCHEDULER_ITEM_CONTEXT_OFFSET, scheduler_context);
        storage
            .scheduler_item
            .write_word(SCHEDULER_ITEM_LINK_STATE_OFFSET, link_state);
        storage.scheduler_item.write_word(
            SCHEDULER_ITEM_ALLOCATION_FLAGS_OFFSET,
            SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE,
        );
        storage.scheduler_item.write_word(
            SCHEDULER_ITEM_ALLOCATION_CONFIG_OFFSET,
            config.allocation_image(),
        );
        storage.scheduler_item.write_word(
            SCHEDULER_ITEM_POSITIONAL_24_OFFSET,
            SCHEDULER_ITEM_POSITIONAL_24_IMAGE,
        );
    }
}

impl BluetoothDtmMemoryGraphStorage {
    /// Reserve the graph and install the reviewed RX allocation defaults.
    pub const fn new() -> Self {
        Self {
            link_state: BluetoothDtmLinkStateStorage {
                words: [const { VolatileCell::new(0) }; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
            },
            scheduler_context: BluetoothDtmSchedulerContextStorage {
                words: [0; BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES / 4],
            },
            scheduler_item: BluetoothDtmSchedulerItemStorage {
                words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
            },
            rx_header: BluetoothDtmRxBufferHeaderStorage {
                words: [const { VolatileCell::new(0) }; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
            },
            rx_swap_reserve: BluetoothDtmRxBufferHeaderStorage {
                words: [const { VolatileCell::new(0) }; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
            },
            tx_header: BluetoothLeTxBufferHeaderStorage::new(),
            tx_packet: BluetoothLeTxPacketStorage::new(),
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
    ///     BluetoothDtmSchedulerAllocationConfig::new(1, 1, 1);
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
        let identity = BluetoothDtmMemoryGraphIdentity::for_storage(storage);
        let binding = match BluetoothDtmMemoryGraphBinding::new(identity, base, config) {
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
}

impl Default for BluetoothDtmMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use core::{cell::Cell, convert::Infallible, fmt::Debug};

    use crate::dtm_event_image::{BluetoothDtmRole, BluetoothLeAccessAddress, BluetoothLeCrcInit};
    use open_esp_radio_esp32s31_hal::{
        BluetoothControllerSramAddress, BluetoothSchedulerFinishedListObservation,
        BluetoothSchedulerFinishedListPop, BluetoothSchedulerHardwareListHead,
        BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothSchedulerHardwareListIndex,
        BluetoothSchedulerSoftwareListRemovalReady,
    };

    use super::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_DTM_LINK_STATE_BYTES,
        BLUETOOTH_DTM_MAX_PACKET_CAPACITY, BLUETOOTH_DTM_RX_PACKET_BYTES,
        BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES, BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES,
        BLUETOOTH_DTM_TX_PACKET_BYTES, BLUETOOTH_LE_BUFFER_HEADER_BYTES,
        BluetoothDtmMemoryGraphCompletionObservation, BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphPrepareError,
        BluetoothDtmMemoryGraphRecycleCleaned, BluetoothDtmMemoryGraphRecycleError,
        BluetoothDtmMemoryGraphRecyclePrepared, BluetoothDtmMemoryGraphRunning,
        BluetoothDtmMemoryGraphRxSuccessObserved, BluetoothDtmMemoryGraphRxSuccessRecycleError,
        BluetoothDtmMemoryGraphStorage, BluetoothDtmPositionalEventSeed,
        BluetoothDtmPositionalEventWords, BluetoothDtmRxResultProjection,
        BluetoothDtmRxResultProjectionError, BluetoothDtmSchedulerAllocationConfig,
        BluetoothDtmSchedulerItemCompletionStatus, BluetoothDtmTxPacketPrepareError,
        LINK_STATE_RX_TAIL_OFFSET, SCHEDULER_ITEM_STATUS_OFFSET,
    };

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct GraphSnapshot {
        link_state: [u32; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
        scheduler_context: [u32; BLUETOOTH_DTM_SCHEDULER_CONTEXT_BYTES / 4],
        scheduler_item: [u32; BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4],
        rx_header: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
        rx_swap_reserve: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
        tx_header: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
        tx_packet: [u8; BLUETOOTH_DTM_TX_PACKET_BYTES],
        rx_packet: [u8; BLUETOOTH_DTM_RX_PACKET_BYTES],
    }

    fn snapshot(storage: &BluetoothDtmMemoryGraphStorage) -> GraphSnapshot {
        GraphSnapshot {
            link_state: storage.link_state.snapshot(),
            scheduler_context: storage.scheduler_context.words,
            scheduler_item: storage.scheduler_item.snapshot(),
            rx_header: storage.rx_header.snapshot_words(),
            rx_swap_reserve: storage.rx_swap_reserve.snapshot_words(),
            tx_header: storage.tx_header.snapshot(),
            tx_packet: storage.tx_packet.snapshot(),
            rx_packet: storage.rx_packet.snapshot(),
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

    fn running_owner_with_status(base: u32, status: u32) -> BluetoothDtmMemoryGraphRunning {
        running_owner_from_cpu(model_owner(base), status)
    }

    fn running_owner_from_cpu(
        owner: BluetoothDtmMemoryGraphCpuOwned,
        status: u32,
    ) -> BluetoothDtmMemoryGraphRunning {
        let prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("matching anchors prepare a CPU-owned image")
            .prepare_scheduler_bookkeeping()
            .prepare_empty_list_link();
        let owner = BluetoothDtmMemoryGraphRunning {
            storage: prepared.storage,
            binding: prepared.binding,
        };
        owner
            .storage
            .as_ref()
            .get_ref()
            .scheduler_item
            .write_word(SCHEDULER_ITEM_STATUS_OFFSET, status);
        owner
    }

    fn script_current_rx_tail_returned(
        owner: &BluetoothDtmMemoryGraphRunning,
        result_word: u32,
        auxiliary: u16,
    ) {
        let storage = owner.storage.as_ref().get_ref();
        let tail = storage.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET);
        let header = if tail == owner.binding.rx_header.controller_address().address() {
            &storage.rx_header
        } else if tail == owner.binding.rx_swap_reserve.controller_address().address() {
            &storage.rx_swap_reserve
        } else {
            panic!("the semantic fixture requires one bound RX tail");
        };
        header.model_controller_completion_observed();
        storage
            .rx_packet
            .model_controller_completion(result_word, auxiliary);
    }

    fn commit_rx_observed(
        observed: BluetoothDtmMemoryGraphRxSuccessObserved,
    ) -> (
        BluetoothDtmMemoryGraphRecycleCleaned,
        Option<Result<BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError>>,
    ) {
        observed.consume_then_commit(core::convert::identity)
    }

    fn rx_success_recycle_prepared(
        owner: BluetoothDtmMemoryGraphRunning,
    ) -> BluetoothDtmMemoryGraphRecyclePrepared {
        let completed = match owner.observe_completion(observed_list(0)) {
            BluetoothDtmMemoryGraphCompletionObservation::CompletionObserved(completed) => {
                completed
            }
            _ => panic!("the scripted RX scheduler item must be completed"),
        };
        let address = completed.scheduler_item_address();
        match completed.prepare_recycle_after_software_list_removal(removal_ready_for(
            BluetoothSchedulerHardwareListIndex::ZERO,
            address,
        )) {
            Ok(prepared) => prepared,
            Err(_) => panic!("the exact removal proof must prepare RX recycle"),
        }
    }

    fn observed_list(
        index: u8,
    ) -> open_esp_radio_esp32s31_hal::BluetoothSchedulerFinishedHardwareListObserved {
        let observation =
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[index])
                .expect("test list belongs to the scheduler domain");
        match observation.pop_lowest() {
            BluetoothSchedulerFinishedListPop::List {
                observed,
                remaining,
            } => {
                assert!(remaining.is_empty());
                observed
            }
            BluetoothSchedulerFinishedListPop::Complete => {
                unreachable!("one scripted list cannot be empty")
            }
        }
    }

    fn removal_ready_for(
        index: BluetoothSchedulerHardwareListIndex,
        address: BluetoothControllerSramAddress,
    ) -> BluetoothSchedulerSoftwareListRemovalReady {
        let head = BluetoothSchedulerHardwareListHead::from_address(address)
            .expect("test identity is a nonempty controller head");
        let head = BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(
            index, head,
        );
        BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(head)
    }

    const fn allocation_config() -> BluetoothDtmSchedulerAllocationConfig {
        BluetoothDtmSchedulerAllocationConfig::new(2, 3, 4)
    }

    fn candidate_words(seed: BluetoothDtmPositionalEventSeed) -> BluetoothDtmPositionalEventWords {
        let current = seed.words();
        let link_state = current.link_state().apply_reset(
            Some(seed.tx_header_head_projection()),
            Some(seed.rx_header_tail_projection()),
            0,
            0,
            BluetoothDtmRole::Transmitter,
        );

        BluetoothDtmPositionalEventWords::new(link_state, current.scheduler_item())
    }

    #[test]
    fn dtm_access_address_initialization_round_trips_semantically() {
        let storage = BluetoothDtmMemoryGraphStorage::new();
        let reset = storage.link_state.reviewed_words().apply_reset(
            None,
            None,
            0,
            0,
            BluetoothDtmRole::Transmitter,
        );

        assert_eq!(
            reset.access_address(),
            BluetoothLeAccessAddress::DIRECT_TEST_MODE
        );
        storage.link_state.write_reviewed_words(reset);
        assert_eq!(
            storage.link_state.reviewed_words().access_address(),
            BluetoothLeAccessAddress::DIRECT_TEST_MODE
        );
    }

    #[test]
    fn dtm_crc_initialization_round_trips_semantically() {
        let storage = BluetoothDtmMemoryGraphStorage::new();
        let reset = storage.link_state.reviewed_words().apply_reset(
            None,
            None,
            0,
            0,
            BluetoothDtmRole::Transmitter,
        );

        assert_eq!(reset.crc_init(), BluetoothLeCrcInit::DIRECT_TEST_MODE);
        storage.link_state.write_reviewed_words(reset);
        assert_eq!(
            storage.link_state.reviewed_words().crc_init(),
            BluetoothLeCrcInit::DIRECT_TEST_MODE
        );
    }

    #[test]
    fn dtm_reset_selects_and_retains_the_reviewed_link_state_profile() {
        let storage = BluetoothDtmMemoryGraphStorage::new();
        let reset = storage.link_state.reviewed_words().apply_reset(
            None,
            None,
            0,
            0,
            BluetoothDtmRole::Transmitter,
        );

        assert!(reset.profile_word_14.direct_test_mode_is_selected());
        storage.link_state.write_reviewed_words(reset);
        assert!(
            storage
                .link_state
                .reviewed_words()
                .profile_word_14
                .direct_test_mode_is_selected()
        );
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

    fn assert_rejected_before_builder(
        owner: BluetoothDtmMemoryGraphCpuOwned,
        expected: BluetoothDtmMemoryGraphPrepareError,
    ) -> BluetoothDtmMemoryGraphCpuOwned {
        let builder_called = Cell::new(false);
        let owner = assert_prepare_failure_unchanged(
            owner,
            |seed| {
                builder_called.set(true);
                Ok::<_, Infallible>(candidate_words(seed))
            },
            expected,
        );
        assert!(!builder_called.get());
        owner
    }

    #[test]
    fn positional_preparation_rejects_a_foreign_tx_packet_base_before_builder() {
        let owner = model_owner(0x2f00_0100);
        let foreign = model_owner(0x2f00_2000);
        owner
            .storage
            .as_ref()
            .get_ref()
            .tx_header
            .model_retarget_packet_base(foreign.binding.tx_packet);

        let _owner = assert_rejected_before_builder(
            owner,
            BluetoothDtmMemoryGraphPrepareError::CurrentTxHeaderPacketBaseMismatch,
        );
    }

    #[test]
    fn positional_preparation_rejects_a_foreign_tx_pdu_before_builder() {
        let owner = model_owner(0x2f00_0100);
        let foreign = model_owner(0x2f00_2000);
        owner
            .storage
            .as_ref()
            .get_ref()
            .tx_header
            .model_retarget_pdu(foreign.binding.tx_packet);

        let _owner = assert_rejected_before_builder(
            owner,
            BluetoothDtmMemoryGraphPrepareError::CurrentTxHeaderPduTargetMismatch,
        );
    }

    #[test]
    fn positional_preparation_rejects_a_lost_tx_allocation_extent_before_builder() {
        let owner = model_owner(0x2f00_0100);
        owner
            .storage
            .as_ref()
            .get_ref()
            .tx_header
            .model_drop_allocation_extent();

        let _owner = assert_rejected_before_builder(
            owner,
            BluetoothDtmMemoryGraphPrepareError::CurrentTxHeaderAllocationExtentMismatch,
        );
    }

    #[test]
    fn positional_preparation_rejects_a_foreign_rx_packet_before_builder() {
        let owner = model_owner(0x2f00_0100);
        let foreign = model_owner(0x2f00_2000);
        owner
            .storage
            .as_ref()
            .get_ref()
            .rx_header
            .model_retarget_rx_packet(foreign.binding.rx_packet);

        let _owner = assert_rejected_before_builder(
            owner,
            BluetoothDtmMemoryGraphPrepareError::CurrentRxTailPacketMismatch,
        );
    }

    #[test]
    fn cancel_restores_the_complete_logical_graph_image() {
        let mut payload = [0; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
        payload[..3].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        let owner = model_owner(0x2f00_0500)
            .prepare_tx_packet(7, 3, &payload)
            .expect("standard LE Test PDU Type prepares")
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
        let prepared = model_owner(0x2f00_0900)
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("matching anchors prepare a CPU-owned image");
        let before = snapshot(prepared.storage.as_ref().get_ref());
        let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
        let prepared = scheduler_prepared.cancel();
        assert_eq!(snapshot(prepared.storage.as_ref().get_ref()), before);
    }

    #[test]
    fn completion_observation_preserves_owners_and_classifies_status() {
        let owner = running_owner_with_status(0x2f00_1900, 0);
        let owner = match owner.observe_completion(observed_list(3)) {
            BluetoothDtmMemoryGraphCompletionObservation::ListMismatch { owner, .. } => owner,
            _ => panic!("another list cannot inspect the DTM item"),
        };
        let completed = match owner.observe_completion(observed_list(0)) {
            BluetoothDtmMemoryGraphCompletionObservation::CompletionObserved(completed) => {
                completed
            }
            _ => panic!("zero status must produce a completion observation"),
        };
        assert_eq!(
            completed.status(),
            BluetoothDtmSchedulerItemCompletionStatus::Zero
        );

        let owner = running_owner_with_status(0x2f00_1d00, u32::MAX);
        assert!(matches!(
            owner.observe_completion(observed_list(0)),
            BluetoothDtmMemoryGraphCompletionObservation::StillInFlight(_)
        ));

        let owner = running_owner_with_status(0x2f00_2100, 7);
        let completed = match owner.observe_completion(observed_list(0)) {
            BluetoothDtmMemoryGraphCompletionObservation::CompletionObserved(completed) => {
                completed
            }
            _ => panic!("non-sentinel status must produce a completion observation"),
        };
        assert_eq!(
            completed.status(),
            BluetoothDtmSchedulerItemCompletionStatus::NonZero(
                core::num::NonZeroU32::new(7).expect("seven is nonzero")
            )
        );
    }

    #[test]
    fn recycle_is_lossless_before_commit_and_returns_a_reusable_cpu_graph() {
        let owner = running_owner_with_status(0x2f00_2500, 7);
        let completed = match owner.observe_completion(observed_list(0)) {
            BluetoothDtmMemoryGraphCompletionObservation::CompletionObserved(completed) => {
                completed
            }
            _ => panic!("non-sentinel status must produce a completion observation"),
        };
        let wrong_address =
            BluetoothControllerSramAddress::new(completed.scheduler_item_address().address() + 4)
                .expect("adjacent aligned model identity stays in controller SRAM");
        let failure = match completed.prepare_recycle_after_software_list_removal(
            removal_ready_for(BluetoothSchedulerHardwareListIndex::ZERO, wrong_address),
        ) {
            Ok(_) => panic!("a removal proof for another item must reject before mutation"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmMemoryGraphRecycleError::SchedulerItemMismatch
        );
        let (completed, _wrong_removal) = failure.into_parts();
        assert_eq!(
            completed.status(),
            BluetoothDtmSchedulerItemCompletionStatus::NonZero(
                core::num::NonZeroU32::new(7).expect("seven is nonzero")
            )
        );

        let address = completed.scheduler_item_address();
        let prepared = match completed.prepare_recycle_after_software_list_removal(
            removal_ready_for(BluetoothSchedulerHardwareListIndex::ZERO, address),
        ) {
            Ok(prepared) => prepared,
            Err(_) => panic!("the bound removal proof must authorize the exact completed graph"),
        };
        let cleaned = prepared.commit();
        let (owner, status) = cleaned.into_cpu_owned().into_parts();
        assert_eq!(
            status,
            BluetoothDtmSchedulerItemCompletionStatus::NonZero(
                core::num::NonZeroU32::new(7).expect("seven is nonzero")
            )
        );
        let _prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("the recycled CPU graph can prepare a later event");
    }

    #[test]
    fn rx_success_without_a_returned_packet_recycles_for_a_later_event() {
        let owner = running_owner_with_status(0x2f00_2900, 0);
        let prepared = rx_success_recycle_prepared(owner)
            .prepare_receiver_success()
            .expect("an incomplete initial tail is a valid empty RX result");
        let (cleaned, projection) = commit_rx_observed(prepared.observe());

        assert_eq!(projection, None);
        let (owner, status) = cleaned.into_cpu_owned().into_parts();
        assert_eq!(status, BluetoothDtmSchedulerItemCompletionStatus::Zero);
        let _prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("an empty RX completion leaves a reusable graph");
    }

    #[test]
    fn rx_success_rotates_both_headers_across_recurring_events() {
        let first = running_owner_with_status(0x2f00_2d00, 0);
        script_current_rx_tail_returned(&first, 0xa500_0000, 0);
        let (cleaned, first_projection) = commit_rx_observed(
            rx_success_recycle_prepared(first)
                .prepare_receiver_success()
                .expect("the first completed tail has a valid swap plan")
                .observe(),
        );
        assert_eq!(
            first_projection,
            Some(BluetoothDtmRxResultProjection::from_word(0xa500_0000))
        );

        let (owner, _) = cleaned.into_cpu_owned().into_parts();
        let second = running_owner_from_cpu(owner, 0);
        script_current_rx_tail_returned(&second, 0x3100_0001, 7);
        let (cleaned, second_projection) = commit_rx_observed(
            rx_success_recycle_prepared(second)
                .prepare_receiver_success()
                .expect("the alternate completed tail rotates back to the first slot")
                .observe(),
        );
        assert_eq!(
            second_projection,
            Some(BluetoothDtmRxResultProjection::from_word(0x3100_0001))
        );

        let (owner, _) = cleaned.into_cpu_owned().into_parts();
        let _prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("two rotations retain a valid recurring graph");
    }

    #[test]
    fn steady_empty_event_preserves_the_chain_for_the_next_return() {
        let first = running_owner_with_status(0x2f00_3100, 0);
        script_current_rx_tail_returned(&first, 0, 0);
        let (cleaned, _) = commit_rx_observed(
            rx_success_recycle_prepared(first)
                .prepare_receiver_success()
                .expect("the first return is valid")
                .observe(),
        );

        let (owner, _) = cleaned.into_cpu_owned().into_parts();
        let empty = running_owner_from_cpu(owner, 0);
        let (cleaned, projection) = commit_rx_observed(
            rx_success_recycle_prepared(empty)
                .prepare_receiver_success()
                .expect("an incomplete steady tail is a valid empty event")
                .observe(),
        );
        assert_eq!(projection, None);

        let (owner, _) = cleaned.into_cpu_owned().into_parts();
        let returned = running_owner_from_cpu(owner, 0);
        script_current_rx_tail_returned(&returned, 0x4200_0000, 3);
        let (cleaned, projection) = commit_rx_observed(
            rx_success_recycle_prepared(returned)
                .prepare_receiver_success()
                .expect("the tail after an empty recurrence remains returnable")
                .observe(),
        );
        assert!(projection.is_some());
        let _owner = cleaned.into_cpu_owned();
    }

    #[test]
    fn rx_success_sentinel_rejection_is_lossless() {
        let owner = running_owner_with_status(0x2f00_3500, 0);
        script_current_rx_tail_returned(&owner, 0x00ff_ffff, 0);
        let prepared = rx_success_recycle_prepared(owner);
        let before = snapshot(prepared.completed.owner.storage.as_ref().get_ref());
        let failure = match prepared.prepare_receiver_success() {
            Ok(_) => panic!("a returned packet cannot retain the result sentinel"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmMemoryGraphRxSuccessRecycleError::ReturnedResultNotProduced
        );
        let prepared = failure.into_recycle_prepared();
        assert_eq!(
            snapshot(prepared.completed.owner.storage.as_ref().get_ref()),
            before
        );
    }

    #[test]
    fn rx_success_auxiliary_rearm_rejection_is_lossless() {
        let owner = running_owner_with_status(0x2f00_3900, 0);
        script_current_rx_tail_returned(&owner, 0, u16::MAX);
        let prepared = rx_success_recycle_prepared(owner);
        let before = snapshot(prepared.completed.owner.storage.as_ref().get_ref());
        let failure = match prepared.prepare_receiver_success() {
            Ok(_) => panic!("a returned packet cannot retain the auxiliary re-arm state"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmMemoryGraphRxSuccessRecycleError::ReturnedAuxiliaryNotProduced
        );
        let prepared = failure.into_recycle_prepared();
        assert_eq!(
            snapshot(prepared.completed.owner.storage.as_ref().get_ref()),
            before
        );
    }

    #[test]
    fn builder_failure_returns_the_byte_unchanged_reusable_owner() {
        let owner = model_owner(0x2f00_0900);
        let owner = assert_prepare_failure_unchanged(
            owner,
            |_| Err::<BluetoothDtmPositionalEventWords, _>("builder rejected inputs"),
            BluetoothDtmMemoryGraphPrepareError::Build("builder rejected inputs"),
        );
        let _prepared = owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
            .expect("the returned owner remains reusable");
    }

    #[test]
    fn reclaimed_cpu_graph_can_start_another_affine_epoch() {
        let reclaimed = model_owner(0x2f00_4100).into_reclaimed();
        let owner = reclaimed.reinitialize();

        let reclaimed = owner.into_reclaimed();
        let _owner = reclaimed.reinitialize();
    }

    #[test]
    fn failed_binding_returns_the_same_storage_for_retry() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
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
        let (storage, _) = failure.into_parts();
        assert_eq!(core::ptr::addr_of!(*storage).addr(), original_address);

        let valid = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("retry base has valid compressed-pointer syntax");
        let _owner =
            BluetoothDtmMemoryGraphStorage::pin_static_model(storage, valid, allocation_config())
                .expect("returned allocation can be bound exactly once");
    }

    #[test]
    fn every_standard_le_test_pdu_type_reaches_packet_readiness() {
        let payload = [0; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
        let mut owner = model_owner(0x2f00_2100);

        for payload_type in 0..=7 {
            let prepared = owner
                .prepare_tx_packet(payload_type, 0, &payload)
                .expect("all standard LE Test PDU Types prepare");
            owner = prepared.discard_packet_readiness();
        }
    }

    #[test]
    fn unsupported_le_test_pdu_type_returns_the_unchanged_owner() {
        let payload = [0; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
        let owner = model_owner(0x2f00_2500);
        let before = snapshot(owner.storage.as_ref().get_ref());

        let failure = match owner.prepare_tx_packet(8, 3, &payload) {
            Ok(_) => panic!("an unsupported LE Test PDU Type cannot claim readiness"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmTxPacketPrepareError::UnsupportedPayloadType
        );
        let (owner, error) = failure.into_parts();
        assert_eq!(
            error,
            BluetoothDtmTxPacketPrepareError::UnsupportedPayloadType
        );
        assert_eq!(snapshot(owner.storage.as_ref().get_ref()), before);
    }
}
