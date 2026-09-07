//! Private SRAM layout and word codec for one DTM memory graph.

use core::{marker::PhantomPinned, num::NonZeroU32, pin::Pin};

use crate::{
    dtm_event_image::{
        DtmLinkStateProfileWord, DtmLinkStateReviewedWords, DtmPositionalEventWords,
        DtmRxHeaderTailProjection, DtmSchedulerHardwareChainWord, DtmSchedulerItemReviewedWords,
        DtmTxHeaderHeadProjection,
    },
    dtm_rx_result::{DtmRxResultProjection, DtmRxResultProjectionError},
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_packet::{
        BLUETOOTH_LE_BUFFER_HEADER_BYTES, BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES,
        LeTxBufferHeaderStorage, LeTxPacketAddress, LeTxPacketPreparedLength, LeTxPacketStorage,
    },
    scheduler_context::SchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        ControllerSramLinkAddress,
    },
};

use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};

use pin_project::pin_project;

use super::{
    DtmMemoryGraphBindError, DtmMemoryGraphIdentity, DtmMemoryGraphPrepareError,
    DtmMemoryGraphRecyclePrepared, DtmMemoryGraphRxSuccessRecycleError, DtmPositionalEventSeed,
    DtmSchedulerAllocationConfig, DtmSchedulerItemCompletionStatus, DtmTxPacketPrepareError,
};

use vcell::VolatileCell;

/// Bytes allocated for one DTM link-state object.
pub const BLUETOOTH_DTM_LINK_STATE_BYTES: usize = 0x84;
/// Bytes allocated for one DTM scheduler item.
pub const BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES: usize = 0x60;
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

type DtmTxPacketAddress = LeTxPacketAddress<BLUETOOTH_DTM_TX_PACKET_BYTES>;

const RX_PACKET_LAST_ALIGNED_OFFSET: u32 = 0x11c;
pub(super) const LINK_STATE_RX_TAIL_OFFSET: usize = 0x70 / 4;
const LINK_STATE_RX_HEAD_OFFSET: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD_OFFSET: usize = 0x6c / 4;
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
pub(super) const SCHEDULER_ITEM_STATUS_OFFSET: usize = 0x38 / 4;
const SCHEDULER_ITEM_CONTROL_OFFSET: usize = 0x4c / 4;
const SCHEDULER_ITEM_CONTROL_BYTE: usize = 2;
const SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET: usize = 0;
const SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET: usize = 0x50 / 4;
const SCHEDULER_ITEM_COMPLETED_LINK_OFFSET: usize = 0x54 / 4;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4;
const RX_PACKET_WORDS: usize = BLUETOOTH_DTM_RX_PACKET_BYTES.div_ceil(4);
const PRIVATE_SCHEDULER_ALLOCATION_ADDITION: u32 = 5;

impl DtmSchedulerAllocationConfig {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtmRxPacketAddressError {
    InvalidBase(BluetoothControllerSramAddressError),
    ZeroCompressedBase,
    ExtentOutsideControllerSram,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DtmRxPacketAddress {
    base: BluetoothControllerSramAddress,
}

impl DtmRxPacketAddress {
    const fn new(address: u32) -> Result<Self, DtmRxPacketAddressError> {
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(DtmRxPacketAddressError::InvalidBase(error)),
        };
        if base.compressed_image() == 0 {
            return Err(DtmRxPacketAddressError::ZeroCompressedBase);
        }
        let last_aligned_address = match address.checked_add(RX_PACKET_LAST_ALIGNED_OFFSET) {
            Some(address) => address,
            None => return Err(DtmRxPacketAddressError::ExtentOutsideControllerSram),
        };
        if BluetoothControllerSramAddress::new(last_aligned_address).is_err() {
            return Err(DtmRxPacketAddressError::ExtentOutsideControllerSram);
        }
        Ok(Self { base })
    }

    const fn compressed_image(self) -> u32 {
        self.base.compressed_image()
    }
}

/// Opaque CPU-owned link-state allocation.
#[repr(C, align(4))]
pub(super) struct DtmLinkStateStorage {
    words: [VolatileCell<u32>; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
}

impl DtmLinkStateStorage {
    const CRC_INIT_WORD: usize = 0x2c / 4;
    const ACCESS_ADDRESS_WORD: usize = 0x38 / 4;

    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
        }
    }

    pub(super) fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    pub(super) fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn access_address(&self) -> LeAccessAddress {
        LeAccessAddress::from_controller_image(self.read_word(Self::ACCESS_ADDRESS_WORD))
    }

    fn write_access_address(&self, access_address: LeAccessAddress) {
        self.write_word(Self::ACCESS_ADDRESS_WORD, access_address.controller_image());
    }

    fn crc_init(&self) -> LeCrcInit {
        LeCrcInit::from_controller_word(self.read_word(Self::CRC_INIT_WORD))
    }

    fn write_crc_init(&self, crc_init: LeCrcInit) {
        let word = self.read_word(Self::CRC_INIT_WORD);
        self.write_word(Self::CRC_INIT_WORD, crc_init.apply_to_controller_word(word));
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> [u32; BLUETOOTH_DTM_LINK_STATE_BYTES / 4] {
        core::array::from_fn(|index| self.read_word(index))
    }

    pub(super) fn reviewed_words(&self) -> DtmLinkStateReviewedWords {
        DtmLinkStateReviewedWords {
            word_00: self.read_word(0),
            word_04: self.read_word(1),
            word_08: self.read_word(2),
            profile_word_14: DtmLinkStateProfileWord::from_storage(self.read_word(5)),
            crc_init: self.crc_init(),
            word_34: self.read_word(13),
            access_address: self.access_address(),
            word_50: self.read_word(20),
        }
    }

    pub(super) fn write_reviewed_words(&self, words: DtmLinkStateReviewedWords) {
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
#[repr(C, align(4))]
pub(super) struct DtmSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl DtmSchedulerItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    pub(super) fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    pub(super) fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn hardware_chain_word(&self) -> DtmSchedulerHardwareChainWord {
        DtmSchedulerHardwareChainWord::from_storage(
            self.read_word(SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET),
        )
    }

    fn write_hardware_chain_word(&self, word: DtmSchedulerHardwareChainWord) {
        self.write_word(SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET, word.into_storage());
    }

    fn terminate_hardware_chain(&self) -> DtmSchedulerHardwareChainWord {
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
    pub(super) fn snapshot(&self) -> [u32; SCHEDULER_ITEM_WORDS] {
        core::array::from_fn(|index| self.read_word(index))
    }

    fn reviewed_words(&self) -> DtmSchedulerItemReviewedWords {
        DtmSchedulerItemReviewedWords {
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

    fn write_reviewed_words(&self, words: DtmSchedulerItemReviewedWords) {
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

#[repr(C, align(4))]
pub(super) struct DtmRxBufferHeaderStorage {
    words: [VolatileCell<u32>; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
}

impl DtmRxBufferHeaderStorage {
    const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;
    const RX_COMPLETION_OBSERVED_MASK: u32 = 0x8000_0000;
    const RX_SOFTWARE_TERMINAL_MASK: u32 = 1;

    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
        }
    }

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

    #[cfg(test)]
    pub(super) fn snapshot_words(&self) -> [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4] {
        core::array::from_fn(|index| self.read_word(index))
    }

    fn initialize_bound_rx(&self, packet: DtmRxPacketAddress) {
        self.install([0, packet.compressed_image(), 0x8080_0000, 0, 0, 0]);
    }

    fn initialize_rx_swap_reserve(&self) {
        self.install([0, 0, 0x8080_0000, 0, 0, 0]);
    }

    fn rx_successor_link(&self) -> Option<DtmRxHeaderSuccessorLink> {
        DtmRxHeaderSuccessorLink::from_image(self.read_word(0) & Self::COMPRESSED_LINK_MASK)
    }

    fn set_rx_successor_link(&self, link: Option<DtmRxHeaderSuccessorLink>) {
        let current = self.read_word(0);
        let image = link.map_or(0, DtmRxHeaderSuccessorLink::image);
        self.write_word(0, (current & !Self::COMPRESSED_LINK_MASK) | image);
    }

    fn rx_packet_link(&self) -> Option<DtmRxPacketLink> {
        DtmRxPacketLink::from_image(self.read_word(1) & Self::COMPRESSED_LINK_MASK)
    }

    fn clear_rx_packet_link(&self) {
        let current = self.read_word(1);
        self.write_word(1, current & !Self::COMPRESSED_LINK_MASK);
    }

    fn observe_rx_completion_after_fence(&self, _fenced: &DtmMemoryGraphRecyclePrepared) -> bool {
        self.read_word(3) & Self::RX_COMPLETION_OBSERVED_MASK != 0
    }

    fn clear_rx_completion_observation(&self) {
        let current = self.read_word(3);
        self.write_word(3, current & !Self::RX_COMPLETION_OBSERVED_MASK);
    }

    #[cfg(test)]
    pub(super) fn model_controller_completion_observed(&self) {
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

    fn rx_backlink(&self) -> Option<DtmRxHeaderBacklink> {
        DtmRxHeaderBacklink::from_address(self.read_word(5))
    }

    fn set_rx_backlink(&self, backlink: Option<DtmRxHeaderBacklink>) {
        self.write_word(5, backlink.map_or(0, DtmRxHeaderBacklink::address));
    }

    fn copy_complete_rx_image_from(&self, source: &Self) {
        self.install(core::array::from_fn(|index| source.read_word(index)));
    }

    #[cfg(test)]
    pub(super) fn model_retarget_rx_packet(&self, packet: DtmRxPacketAddress) {
        let current = self.read_word(1);
        self.write_word(
            1,
            (current & !Self::COMPRESSED_LINK_MASK) | packet.compressed_image(),
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxHeaderSuccessorLink(u32);

impl DtmRxHeaderSuccessorLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn from_bound(address: ControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn image(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxPacketLink(u32);

impl DtmRxPacketLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn from_bound(packet: DtmRxPacketAddress) -> Self {
        Self(packet.compressed_image())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxHeaderBacklink(u32);

impl DtmRxHeaderBacklink {
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
pub(super) struct LeTestPduHeader(u8);

impl LeTestPduHeader {
    pub(super) const fn without_cte(payload_type: u8) -> Result<Self, DtmTxPacketPrepareError> {
        if payload_type <= 7 {
            Ok(Self(payload_type))
        } else {
            Err(DtmTxPacketPrepareError::UnsupportedPayloadType)
        }
    }

    const fn controller_image(self) -> u8 {
        self.0
    }
}

#[repr(C, align(4))]
pub(super) struct DtmRxPacketStorage {
    words: [VolatileCell<u32>; RX_PACKET_WORDS],
}

impl DtmRxPacketStorage {
    const CAPACITY_WORD: usize = 1;
    const RESULT_WORD: usize = 3;
    const AUXILIARY_REARM_WORD: usize = 6;
    const RESULT_REARM_SENTINEL: u32 = 0x00ff_ffff;
    const AUXILIARY_REARM_SENTINEL: u32 = 0x0000_ffff;

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
    pub(super) fn snapshot(&self) -> [u8; BLUETOOTH_DTM_RX_PACKET_BYTES] {
        core::array::from_fn(|index| {
            let shift = (index % 4) * 8;
            (self.read_word(index / 4) >> shift) as u8
        })
    }

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

    fn observe_after_fenced_completion(
        &self,
        _fenced: &DtmMemoryGraphRecyclePrepared,
    ) -> Result<
        Result<DtmRxResultProjection, DtmRxResultProjectionError>,
        DtmRxPacketCompletionObservationError,
    > {
        let result_word = self.read_word(Self::RESULT_WORD);
        if result_word & Self::RESULT_REARM_SENTINEL == Self::RESULT_REARM_SENTINEL {
            return Err(DtmRxPacketCompletionObservationError::ResultNotProduced);
        }
        if self.read_word(Self::AUXILIARY_REARM_WORD) & Self::AUXILIARY_REARM_SENTINEL
            == Self::AUXILIARY_REARM_SENTINEL
        {
            return Err(DtmRxPacketCompletionObservationError::AuxiliaryNotProduced);
        }
        Ok(DtmRxResultProjection::from_word(result_word))
    }

    #[cfg(test)]
    pub(super) fn model_controller_completion(&self, result_word: u32, auxiliary: u16) {
        self.write_word(Self::RESULT_WORD, result_word);
        self.write_word(Self::AUXILIARY_REARM_WORD, u32::from(auxiliary));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtmRxPacketCompletionObservationError {
    ResultNotProduced,
    AuxiliaryNotProduced,
}

#[pin_project]
#[repr(C)]
pub struct DtmMemoryGraphStorage {
    #[cfg_attr(test, allow(dead_code))]
    pub(super) link_state: DtmLinkStateStorage,
    pub(super) scheduler_context: SchedulerContextStorage,
    pub(super) scheduler_item: DtmSchedulerItemStorage,
    pub(super) rx_header: DtmRxBufferHeaderStorage,
    pub(super) rx_swap_reserve: DtmRxBufferHeaderStorage,
    pub(super) tx_header: LeTxBufferHeaderStorage,
    pub(super) tx_packet: LeTxPacketStorage<BLUETOOTH_DTM_TX_PACKET_BYTES>,
    pub(super) rx_packet: DtmRxPacketStorage,
    #[pin]
    _pin: PhantomPinned,
}

const BLUETOOTH_DTM_MEMORY_GRAPH_BYTES: u32 = core::mem::size_of::<DtmMemoryGraphStorage>() as u32;
const LINK_STATE_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmMemoryGraphStorage, link_state) as u32;
const SCHEDULER_CONTEXT_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmMemoryGraphStorage, scheduler_context) as u32;
const SCHEDULER_ITEM_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmMemoryGraphStorage, scheduler_item) as u32;
const RX_HEADER_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmMemoryGraphStorage, rx_header) as u32;
const RX_SWAP_RESERVE_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmMemoryGraphStorage, rx_swap_reserve) as u32;
const TX_HEADER_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmMemoryGraphStorage, tx_header) as u32;
const TX_PACKET_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmMemoryGraphStorage, tx_packet) as u32;
const RX_PACKET_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmMemoryGraphStorage, rx_packet) as u32;

/// Non-forgeable address binding retained by one CPU-owned static graph.
pub(super) struct DtmMemoryGraphBinding {
    #[cfg(not(target_arch = "riscv32"))]
    identity: DtmMemoryGraphIdentity,
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    allocation_config: DtmSchedulerAllocationConfig,
    link_state: ControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramAddress,
    scheduler_item: ControllerSramLinkAddress,
    pub(super) rx_header: ControllerSramLinkAddress,
    pub(super) rx_swap_reserve: ControllerSramLinkAddress,
    tx_header: ControllerSramLinkAddress,
    pub(super) tx_packet: DtmTxPacketAddress,
    pub(super) rx_packet: DtmRxPacketAddress,
}

impl DtmMemoryGraphBinding {
    pub(super) fn new(
        identity: DtmMemoryGraphIdentity,
        base: u32,
        allocation_config: DtmSchedulerAllocationConfig,
    ) -> Result<Self, DtmMemoryGraphBindError> {
        #[cfg(target_arch = "riscv32")]
        let _ = identity;
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(DtmMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || BLUETOOTH_DTM_MEMORY_GRAPH_BYTES
                > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(DtmMemoryGraphBindError::ExtentOutsidePhysicalSram);
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(DtmMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let bound_link = |offset: u32| {
            ControllerSramLinkAddress::new(address(offset)?)
                .map_err(|_| DtmMemoryGraphBindError::ZeroCompressedLink)
        };

        let link_state = bound_link(LINK_STATE_STORAGE_OFFSET)?;
        let scheduler_context =
            BluetoothControllerSramAddress::new(address(SCHEDULER_CONTEXT_STORAGE_OFFSET)?)
                .map_err(DtmMemoryGraphBindError::InvalidBase)?;
        let scheduler_item = bound_link(SCHEDULER_ITEM_STORAGE_OFFSET)?;
        let rx_header = bound_link(RX_HEADER_STORAGE_OFFSET)?;
        let rx_swap_reserve = bound_link(RX_SWAP_RESERVE_STORAGE_OFFSET)?;
        let tx_header = bound_link(TX_HEADER_STORAGE_OFFSET)?;
        let tx_packet = DtmTxPacketAddress::new(address(TX_PACKET_STORAGE_OFFSET)?)
            .map_err(|_| DtmMemoryGraphBindError::InvalidPacketExtent)?;
        let rx_packet = DtmRxPacketAddress::new(address(RX_PACKET_STORAGE_OFFSET)?)
            .map_err(|_| DtmMemoryGraphBindError::InvalidPacketExtent)?;

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

    pub const fn identity(&self) -> DtmMemoryGraphIdentity {
        #[cfg(target_arch = "riscv32")]
        {
            DtmMemoryGraphIdentity(self.base.address() as usize)
        }
        #[cfg(not(target_arch = "riscv32"))]
        {
            self.identity
        }
    }

    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    pub const fn allocation_config(&self) -> DtmSchedulerAllocationConfig {
        self.allocation_config
    }

    pub const fn scheduler_item_address(&self) -> ControllerSramLinkAddress {
        self.scheduler_item
    }
}

pub(super) struct DtmSchedulerBookkeepingRollback {
    previous_control: u32,
    previous_status: u32,
    previous_completed_link: u32,
}

pub(super) struct DtmEmptyListRollback {
    bookkeeping: DtmSchedulerBookkeepingRollback,
    previous_hardware_chain: DtmSchedulerHardwareChainWord,
    previous_software_next: u32,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum DtmRxHeaderSlot {
    Primary,
    Swap,
}

impl DtmRxHeaderSlot {
    const fn other(self) -> Self {
        match self {
            Self::Primary => Self::Swap,
            Self::Swap => Self::Primary,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DtmRxRotationPlan {
    NoReturnedPacket {
        predecessor: Option<DtmRxHeaderSlot>,
    },
    Rotate {
        returned: DtmRxHeaderSlot,
        copy_target: DtmRxHeaderSlot,
        steady: bool,
        projection: Result<DtmRxResultProjection, DtmRxResultProjectionError>,
    },
}

impl DtmRxRotationPlan {
    pub(super) const fn projection(
        self,
    ) -> Option<Result<DtmRxResultProjection, DtmRxResultProjectionError>> {
        match self {
            Self::NoReturnedPacket { .. } => None,
            Self::Rotate { projection, .. } => Some(projection),
        }
    }
}

impl DtmMemoryGraphStorage {
    pub const fn new() -> Self {
        Self {
            link_state: DtmLinkStateStorage::new(),
            scheduler_context: SchedulerContextStorage::new(),
            scheduler_item: DtmSchedulerItemStorage::new(),
            rx_header: DtmRxBufferHeaderStorage::new(),
            rx_swap_reserve: DtmRxBufferHeaderStorage::new(),
            tx_header: LeTxBufferHeaderStorage::new(),
            tx_packet: LeTxPacketStorage::new(),
            rx_packet: DtmRxPacketStorage::new(),
            _pin: PhantomPinned,
        }
    }

    pub(super) fn initialize_reviewed_allocation(
        self: Pin<&mut Self>,
        binding: &DtmMemoryGraphBinding,
        config: DtmSchedulerAllocationConfig,
    ) {
        let link_state = binding.link_state.compressed_image();
        let scheduler_context = binding.scheduler_context.compressed_image();
        let rx_header_address = binding.rx_header.controller_address().address();
        let rx_swap_address = binding.rx_swap_reserve.controller_address().address();
        let tx_header_address = binding.tx_header.controller_address().address();

        let storage = self.project();
        storage.link_state.clear();
        storage.scheduler_context.clear();
        storage.scheduler_item.clear();
        storage.rx_header.initialize_bound_rx(binding.rx_packet);
        storage.rx_swap_reserve.initialize_rx_swap_reserve();
        storage.tx_header.initialize_bound_tx(binding.tx_packet);
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

    pub(super) fn reviewed_event_words(&self) -> DtmPositionalEventWords {
        DtmPositionalEventWords::new(
            self.link_state.reviewed_words(),
            self.scheduler_item.reviewed_words(),
        )
    }

    pub(super) fn restore_positional_words(
        self: Pin<&mut Self>,
        previous: DtmPositionalEventWords,
    ) {
        let storage = self.project();
        storage
            .link_state
            .write_reviewed_words(previous.link_state());
        storage
            .scheduler_item
            .write_reviewed_words(previous.scheduler_item());
    }

    pub(super) fn prepare_scheduler_bookkeeping(
        self: Pin<&mut Self>,
    ) -> DtmSchedulerBookkeepingRollback {
        let scheduler_item = self.project().scheduler_item;
        let previous_control = scheduler_item.read_word(SCHEDULER_ITEM_CONTROL_OFFSET);
        let previous_status = scheduler_item.read_word(SCHEDULER_ITEM_STATUS_OFFSET);
        let previous_completed_link =
            scheduler_item.read_word(SCHEDULER_ITEM_COMPLETED_LINK_OFFSET);
        let mut control = previous_control.to_le_bytes();
        control[SCHEDULER_ITEM_CONTROL_BYTE] = 0;
        scheduler_item.write_word(SCHEDULER_ITEM_CONTROL_OFFSET, u32::from_le_bytes(control));
        scheduler_item.write_word(SCHEDULER_ITEM_STATUS_OFFSET, u32::MAX);
        scheduler_item.write_word(SCHEDULER_ITEM_COMPLETED_LINK_OFFSET, 0);
        DtmSchedulerBookkeepingRollback {
            previous_control,
            previous_status,
            previous_completed_link,
        }
    }

    pub(super) fn restore_scheduler_bookkeeping(
        self: Pin<&mut Self>,
        rollback: DtmSchedulerBookkeepingRollback,
    ) {
        let scheduler_item = self.project().scheduler_item;
        scheduler_item.write_word(SCHEDULER_ITEM_CONTROL_OFFSET, rollback.previous_control);
        scheduler_item.write_word(SCHEDULER_ITEM_STATUS_OFFSET, rollback.previous_status);
        scheduler_item.write_word(
            SCHEDULER_ITEM_COMPLETED_LINK_OFFSET,
            rollback.previous_completed_link,
        );
    }

    pub(super) fn prepare_empty_list_link(
        self: Pin<&mut Self>,
        bookkeeping: DtmSchedulerBookkeepingRollback,
    ) -> DtmEmptyListRollback {
        let scheduler_item = self.project().scheduler_item;
        let previous_hardware_chain = scheduler_item.terminate_hardware_chain();
        let previous_software_next = scheduler_item.read_word(SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET);
        scheduler_item.write_word(SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET, 0);
        DtmEmptyListRollback {
            bookkeeping,
            previous_hardware_chain,
            previous_software_next,
        }
    }

    pub(super) fn restore_empty_list_link(
        self: Pin<&mut Self>,
        rollback: DtmEmptyListRollback,
    ) -> DtmSchedulerBookkeepingRollback {
        let scheduler_item = self.project().scheduler_item;
        scheduler_item.write_hardware_chain_word(rollback.previous_hardware_chain);
        scheduler_item.write_word(
            SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET,
            rollback.previous_software_next,
        );
        rollback.bookkeeping
    }

    pub(super) fn observe_completion_status(&self) -> Option<DtmSchedulerItemCompletionStatus> {
        let status = self.scheduler_item.read_word(SCHEDULER_ITEM_STATUS_OFFSET);
        if status == u32::MAX {
            None
        } else {
            Some(match NonZeroU32::new(status) {
                None => DtmSchedulerItemCompletionStatus::Zero,
                Some(status) => DtmSchedulerItemCompletionStatus::NonZero(status),
            })
        }
    }

    pub(super) fn commit_scheduler_recycle(self: Pin<&mut Self>) {
        let scheduler_item = self.project().scheduler_item;
        scheduler_item.write_word(SCHEDULER_ITEM_COMPLETED_LINK_OFFSET, 0);
        let _ = scheduler_item.terminate_hardware_chain();
    }

    pub(super) fn prepared_tx_packet_bytes(
        &self,
        packet_length: LeTxPacketPreparedLength<BLUETOOTH_DTM_TX_PACKET_BYTES>,
    ) -> &[u8] {
        self.tx_packet.prepared_allocation(packet_length)
    }

    pub(super) fn prepare_tx_packet(
        self: Pin<&mut Self>,
        pdu_header: LeTestPduHeader,
        payload: &[u8],
    ) -> LeTxPacketPreparedLength<BLUETOOTH_DTM_TX_PACKET_BYTES> {
        self.project()
            .tx_packet
            .prepare_pdu(pdu_header.controller_image(), payload)
            .unwrap_or_else(|_| {
                unreachable!("the full DTM allocation accepts every eight-bit payload length")
            })
    }

    pub(super) fn positional_event_seed<BuildError>(
        &self,
        binding: &DtmMemoryGraphBinding,
    ) -> Result<DtmPositionalEventSeed, DtmMemoryGraphPrepareError<BuildError>> {
        let previous = self.reviewed_event_words();
        let tx_head_address = self.link_state.read_word(LINK_STATE_TX_HEAD_OFFSET);
        let rx_tail_address = self.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET);
        let tx_head = ControllerSramLinkAddress::new(tx_head_address)
            .map_err(|_| DtmMemoryGraphPrepareError::CurrentTxHeadUnbound)?;
        if tx_head != binding.tx_header {
            return Err(DtmMemoryGraphPrepareError::CurrentTxHeadIdentityMismatch);
        }
        if self.tx_header.packet_base_link() != Some(binding.tx_packet.base_link()) {
            return Err(DtmMemoryGraphPrepareError::CurrentTxHeaderPacketBaseMismatch);
        }
        if self.tx_header.pdu_target_link() != Some(binding.tx_packet.pdu_target_link()) {
            return Err(DtmMemoryGraphPrepareError::CurrentTxHeaderPduTargetMismatch);
        }
        if !self.tx_header.retains_allocation_extent(binding.tx_packet) {
            return Err(DtmMemoryGraphPrepareError::CurrentTxHeaderAllocationExtentMismatch);
        }
        let rx_tail = ControllerSramLinkAddress::new(rx_tail_address)
            .map_err(|_| DtmMemoryGraphPrepareError::CurrentRxTailUnbound)?;
        if rx_tail != binding.rx_header && rx_tail != binding.rx_swap_reserve {
            return Err(DtmMemoryGraphPrepareError::CurrentRxTailIdentityMismatch);
        }
        let selected_rx_tail = if rx_tail == binding.rx_header {
            &self.rx_header
        } else {
            &self.rx_swap_reserve
        };
        if selected_rx_tail.rx_packet_link() != Some(DtmRxPacketLink::from_bound(binding.rx_packet))
        {
            return Err(DtmMemoryGraphPrepareError::CurrentRxTailPacketMismatch);
        }
        Ok(DtmPositionalEventSeed {
            words: previous,
            tx_header_head: DtmTxHeaderHeadProjection::from_bound(tx_head),
            rx_header_tail: DtmRxHeaderTailProjection::from_bound(rx_tail),
        })
    }

    pub(super) fn validate_and_commit_positional_event<BuildError>(
        self: Pin<&mut Self>,
        binding: &DtmMemoryGraphBinding,
        seed: DtmPositionalEventSeed,
        candidate: DtmPositionalEventWords,
    ) -> Result<(), DtmMemoryGraphPrepareError<BuildError>> {
        let expected_tx_head = seed.tx_header_head_projection();
        let observed_tx_head = candidate.tx_header_head_projection();
        if observed_tx_head != expected_tx_head {
            return Err(DtmMemoryGraphPrepareError::LinkStateTxHeadMismatch {
                expected: expected_tx_head,
                observed: observed_tx_head,
            });
        }
        let expected_rx_tail = seed.rx_header_tail_projection();
        let observed_rx_tail = candidate.rx_header_tail_projection();
        if observed_rx_tail != expected_rx_tail {
            return Err(DtmMemoryGraphPrepareError::LinkStateRxTailMismatch {
                expected: expected_rx_tail,
                observed: observed_rx_tail,
            });
        }
        if !candidate.scheduler_item_retains_link_state(binding.link_state) {
            return Err(DtmMemoryGraphPrepareError::SchedulerItemLinkStateMismatch);
        }
        let storage = self.project();
        storage
            .link_state
            .write_reviewed_words(candidate.link_state());
        storage
            .scheduler_item
            .write_reviewed_words(candidate.scheduler_item());
        Ok(())
    }

    pub(super) fn validate_rx_rotation(
        &self,
        binding: &DtmMemoryGraphBinding,
        recycle: &DtmMemoryGraphRecyclePrepared,
        status: DtmSchedulerItemCompletionStatus,
    ) -> Result<DtmRxRotationPlan, DtmMemoryGraphRxSuccessRecycleError> {
        if status != DtmSchedulerItemCompletionStatus::Zero {
            return Err(DtmMemoryGraphRxSuccessRecycleError::CompletionStatusMismatch);
        }
        let primary_address = binding.rx_header.controller_address().address();
        let swap_address = binding.rx_swap_reserve.controller_address().address();
        let slot = |address| {
            if address == primary_address {
                Some(DtmRxHeaderSlot::Primary)
            } else if address == swap_address {
                Some(DtmRxHeaderSlot::Swap)
            } else {
                None
            }
        };
        let head = slot(self.link_state.read_word(LINK_STATE_RX_HEAD_OFFSET))
            .ok_or(DtmMemoryGraphRxSuccessRecycleError::RxHeadIdentityMismatch)?;
        let tail = slot(self.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET))
            .ok_or(DtmMemoryGraphRxSuccessRecycleError::RxTailIdentityMismatch)?;
        let reserve_address = self.link_state.read_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET);
        let header = |slot| match slot {
            DtmRxHeaderSlot::Primary => &self.rx_header,
            DtmRxHeaderSlot::Swap => &self.rx_swap_reserve,
        };
        let successor = |slot| match slot {
            DtmRxHeaderSlot::Primary => DtmRxHeaderSuccessorLink::from_bound(binding.rx_header),
            DtmRxHeaderSlot::Swap => DtmRxHeaderSuccessorLink::from_bound(binding.rx_swap_reserve),
        };
        let backlink = |slot| match slot {
            DtmRxHeaderSlot::Primary => {
                DtmRxHeaderBacklink::bound(binding.rx_header.controller_address())
            }
            DtmRxHeaderSlot::Swap => {
                DtmRxHeaderBacklink::bound(binding.rx_swap_reserve.controller_address())
            }
        };

        let (returned, copy_target, steady) = if head == tail {
            let copy_target = head.other();
            let expected_reserve = match copy_target {
                DtmRxHeaderSlot::Primary => primary_address,
                DtmRxHeaderSlot::Swap => swap_address,
            };
            if reserve_address != expected_reserve {
                return Err(DtmMemoryGraphRxSuccessRecycleError::RxSwapIdentityMismatch);
            }
            let reserve = header(copy_target);
            if reserve.rx_successor_link().is_some() || reserve.rx_packet_link().is_some() {
                return Err(DtmMemoryGraphRxSuccessRecycleError::ReserveNotDetached);
            }
            if header(tail).rx_backlink().is_some() {
                return Err(DtmMemoryGraphRxSuccessRecycleError::InitialBacklinkUnexpected);
            }
            (tail, copy_target, false)
        } else {
            if reserve_address != 0 {
                return Err(DtmMemoryGraphRxSuccessRecycleError::SwapReserveUnexpected);
            }
            let predecessor = header(head);
            if predecessor.rx_packet_link().is_some() {
                return Err(DtmMemoryGraphRxSuccessRecycleError::PredecessorPacketStillBound);
            }
            if predecessor.rx_successor_link() != Some(successor(tail)) {
                return Err(DtmMemoryGraphRxSuccessRecycleError::PredecessorSuccessorMismatch);
            }
            if !predecessor.observe_rx_completion_after_fence(recycle) {
                return Err(DtmMemoryGraphRxSuccessRecycleError::PredecessorNotCompleted);
            }
            if header(tail).rx_backlink() != Some(backlink(head)) {
                return Err(DtmMemoryGraphRxSuccessRecycleError::SuccessorBacklinkMismatch);
            }
            (tail, head, true)
        };

        let returned_header = header(returned);
        if returned_header.rx_successor_link().is_some() {
            return Err(DtmMemoryGraphRxSuccessRecycleError::ReturnedHasSuccessor);
        }
        if returned_header.rx_packet_link() != Some(DtmRxPacketLink::from_bound(binding.rx_packet))
        {
            return Err(DtmMemoryGraphRxSuccessRecycleError::ReturnedPacketMismatch);
        }
        if !returned_header.observe_rx_completion_after_fence(recycle) {
            return Ok(DtmRxRotationPlan::NoReturnedPacket {
                predecessor: steady.then_some(copy_target),
            });
        }

        let projection = self
            .rx_packet
            .observe_after_fenced_completion(recycle)
            .map_err(|error| match error {
                DtmRxPacketCompletionObservationError::ResultNotProduced => {
                    DtmMemoryGraphRxSuccessRecycleError::ReturnedResultNotProduced
                }
                DtmRxPacketCompletionObservationError::AuxiliaryNotProduced => {
                    DtmMemoryGraphRxSuccessRecycleError::ReturnedAuxiliaryNotProduced
                }
            })?;
        Ok(DtmRxRotationPlan::Rotate {
            returned,
            copy_target,
            steady,
            projection,
        })
    }

    pub(super) fn commit_rx_rotation(
        self: Pin<&mut Self>,
        binding: &DtmMemoryGraphBinding,
        plan: DtmRxRotationPlan,
    ) {
        let storage = self.project();
        match plan {
            DtmRxRotationPlan::NoReturnedPacket { predecessor } => {
                if let Some(predecessor) = predecessor {
                    match predecessor {
                        DtmRxHeaderSlot::Primary => storage.rx_header,
                        DtmRxHeaderSlot::Swap => storage.rx_swap_reserve,
                    }
                    .set_rx_software_terminal(true);
                }
            }
            DtmRxRotationPlan::Rotate {
                returned,
                copy_target,
                steady,
                projection: _,
            } => {
                let primary = &*storage.rx_header;
                let swap = &*storage.rx_swap_reserve;
                let returned_header = match returned {
                    DtmRxHeaderSlot::Primary => primary,
                    DtmRxHeaderSlot::Swap => swap,
                };
                let copy_header = match copy_target {
                    DtmRxHeaderSlot::Primary => primary,
                    DtmRxHeaderSlot::Swap => swap,
                };
                let address = |slot| match slot {
                    DtmRxHeaderSlot::Primary => binding.rx_header.controller_address().address(),
                    DtmRxHeaderSlot::Swap => binding.rx_swap_reserve.controller_address().address(),
                };
                let copy_link = match copy_target {
                    DtmRxHeaderSlot::Primary => {
                        DtmRxHeaderSuccessorLink::from_bound(binding.rx_header)
                    }
                    DtmRxHeaderSlot::Swap => {
                        DtmRxHeaderSuccessorLink::from_bound(binding.rx_swap_reserve)
                    }
                };
                let returned_backlink = match returned {
                    DtmRxHeaderSlot::Primary => {
                        DtmRxHeaderBacklink::bound(binding.rx_header.controller_address())
                    }
                    DtmRxHeaderSlot::Swap => {
                        DtmRxHeaderBacklink::bound(binding.rx_swap_reserve.controller_address())
                    }
                };

                if steady {
                    storage
                        .link_state
                        .write_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET, address(copy_target));
                    storage
                        .link_state
                        .write_word(LINK_STATE_RX_HEAD_OFFSET, address(returned));
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
                    .write_word(LINK_STATE_RX_TAIL_OFFSET, address(copy_target));
                returned_header.set_rx_software_terminal(true);
                copy_header.set_rx_backlink(Some(returned_backlink));
            }
        }
    }
}

impl Default for DtmMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}
