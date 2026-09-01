//! Fixed controller-SRAM memory graph for legacy passive scanning.
//!
//! The vendor allocator is deliberately absent. This module owns the reviewed
//! two-node header/packet topology, private SRAM encoding and affine
//! publication boundary needed by the first passive LE 1M scanner.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerLatchedTime, BluetoothControllerSramAddress,
    BluetoothControllerSramAddressError, BluetoothMemoryListSelector,
    BluetoothRxMemoryListPublished, BluetoothScanStartPublished,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerSoftwareListRemovalReady,
};
use pin_project::pin_project;
use vcell::VolatileCell;

use crate::{
    passive_scanning_event_image::{
        BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS, BluetoothPassiveScanLinkStateImage,
        BluetoothPassiveScanPrimaryChannel, BluetoothPassiveScanResetConfig,
        BluetoothPassiveScanRxHeadProjection, BluetoothPassiveScanSchedulerItemWords,
        BluetoothPassiveScanSchedulerWindow, BluetoothPassiveScanStartSelection,
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
const SCHEDULER_ITEM_WORD_14: usize = 0x14 / 4;
const SCHEDULER_ITEM_WORD_18: usize = 0x18 / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_WORD: usize = 0x1c / 4;
const SCHEDULER_ITEM_ALLOCATION_CONFIG_WORD: usize = 0x20 / 4;
const SCHEDULER_ITEM_POSITIONAL_24_WORD: usize = 0x24 / 4;
const SCHEDULER_ITEM_EVENT_CLASS_WORD: usize = 0x2c / 4;
const SCHEDULER_ITEM_WORD_38: usize = 0x38 / 4;
const SCHEDULER_ITEM_WORD_44: usize = 0x44 / 4;
const SCHEDULER_ITEM_WORD_48: usize = 0x48 / 4;
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0030_0000;
const SCHEDULER_ITEM_LINK_STATE_PREFIX: u32 = 0x00c0_0000;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: u32 = 0x0fdf_ffff;
const SCHEDULER_ITEM_POSITIONAL_24_IMAGE: u32 = 0x0007_bdef;
const SCHEDULER_ITEM_EVENT_CLASS_IMAGE: u32 = 1;
const SCHEDULER_ITEM_ALLOCATION_CONFIG_MAX: u32 = 0x0fff;
const SCHEDULER_ITEM_LINK_MASK: u32 = 0x000f_ffff;

/// Product-owned limits consumed by the passive-scanner item allocator.
///
/// The private codec adds one and the zero-based item index exactly as the
/// reviewed S31 allocator does. Construction rejects a combination that
/// cannot fit every one of the graph's three scheduler items.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPassiveScanSchedulerAllocationConfig {
    extended_advertising_instances: u16,
    connections: u16,
}

impl BluetoothPassiveScanSchedulerAllocationConfig {
    /// Validate the source-owned Controller capacity limits.
    pub const fn new(extended_advertising_instances: u16, connections: u16) -> Option<Self> {
        let largest_image = (extended_advertising_instances as u32)
            .wrapping_add(1)
            .wrapping_add(connections as u32)
            .wrapping_add((BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT - 1) as u32);
        if largest_image <= SCHEDULER_ITEM_ALLOCATION_CONFIG_MAX {
            Some(Self {
                extended_advertising_instances,
                connections,
            })
        } else {
            None
        }
    }

    const fn item_image(self, index: usize) -> u32 {
        (self.extended_advertising_instances as u32)
            .wrapping_add(1)
            .wrapping_add(self.connections as u32)
            .wrapping_add(index as u32)
    }
}

/// One bounded Link Layer PDU copied from a completed scanner packet.
///
/// The vendor packet prefix, producer sentinel and receive epoch remain
/// private. Callers receive only the on-air advertising-channel PDU and its
/// signed receive-strength sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPassiveScanReceivedPdu {
    bytes: [u8; BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY + 2],
    length: u16,
    rssi_dbm: i8,
}

impl BluetoothPassiveScanReceivedPdu {
    /// Complete two-byte advertising header and declared payload.
    pub const fn as_bytes(&self) -> &[u8] {
        self.bytes.split_at(self.length as usize).0
    }

    /// Number of copied Link Layer PDU octets.
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Whether the copied PDU is empty. A valid hardware result is never empty.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Signed receive-strength byte supplied by the controller packet prefix.
    pub const fn rssi_dbm(&self) -> i8 {
        self.rssi_dbm
    }
}

/// Up to two completed packets owned by one restricted scanner event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPassiveScanReceivedBatch {
    packets: [Option<BluetoothPassiveScanReceivedPdu>; BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT],
    len: u8,
}

impl BluetoothPassiveScanReceivedBatch {
    const fn empty() -> Self {
        Self {
            packets: [None; BLUETOOTH_PASSIVE_SCAN_RX_NODE_COUNT],
            len: 0,
        }
    }

    fn push(&mut self, packet: BluetoothPassiveScanReceivedPdu) {
        self.packets[self.len as usize] = Some(packet);
        self.len += 1;
    }

    /// Number of completed packets copied from this event.
    pub const fn len(self) -> usize {
        self.len as usize
    }

    /// Whether this event completed without a received packet.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Borrow one packet in hardware list order.
    pub const fn packet(&self, index: usize) -> Option<&BluetoothPassiveScanReceivedPdu> {
        if index < self.len as usize {
            self.packets[index].as_ref()
        } else {
            None
        }
    }
}

/// Malformed hardware result rejected before scanner graph reclamation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanRxError {
    /// A completed header still points at an untouched producer sentinel.
    ProducerSentinelRetained,
    /// A completed header still points at an untouched receive-epoch sentinel.
    EpochSentinelRetained,
    /// A later node completed after an earlier incomplete node in the chain.
    CompletionChainGap,
}

/// Semantic non-sentinel status written to the scanner scheduler item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanSchedulerItemCompletionStatus {
    Zero,
    NonZero(core::num::NonZeroU32),
}

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
    const COMPLETION_GATE: u32 = 1 << 31;

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

    fn completion_observed(&self) -> bool {
        self.words[3].get() & Self::COMPLETION_GATE != 0
    }

    #[cfg(test)]
    fn emulate_hardware_completion(&self) {
        self.words[3].set(self.words[3].get() | Self::COMPLETION_GATE);
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

    fn received_pdu(&self) -> Result<BluetoothPassiveScanReceivedPdu, BluetoothPassiveScanRxError> {
        let result = self.words[Self::RESULT_WORD].get();
        if result & Self::RESULT_REARM_SENTINEL == Self::RESULT_REARM_SENTINEL {
            return Err(BluetoothPassiveScanRxError::ProducerSentinelRetained);
        }
        let epoch = self.words[Self::EPOCH_WORD].get();
        if epoch & Self::EPOCH_REARM_SENTINEL == Self::EPOCH_REARM_SENTINEL {
            return Err(BluetoothPassiveScanRxError::EpochSentinelRetained);
        }

        let payload_length = self.read_byte(BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES - 1);
        let length = usize::from(payload_length) + 2;
        let mut bytes = [0; BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY + 2];
        let mut index = 0;
        while index < length {
            bytes[index] =
                self.read_byte(BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES - 2 + index);
            index += 1;
        }
        Ok(BluetoothPassiveScanReceivedPdu {
            bytes,
            length: length as u16,
            rssi_dbm: self.read_byte(BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES - 15) as i8,
        })
    }

    fn read_byte(&self, offset: usize) -> u8 {
        let word = self.words[offset / 4].get();
        ((word >> ((offset % 4) * 8)) & 0xff) as u8
    }

    #[cfg(test)]
    fn emulate_hardware_receive(&self, pdu: &[u8], rssi_dbm: i8) {
        assert!((2..=BLUETOOTH_PASSIVE_SCAN_RX_PAYLOAD_CAPACITY + 2).contains(&pdu.len()));
        assert_eq!(usize::from(pdu[1]) + 2, pdu.len());
        self.words[Self::RESULT_WORD].set(0);
        self.words[Self::EPOCH_WORD].set(0);
        for (offset, byte) in pdu.iter().copied().enumerate() {
            self.write_byte(
                BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES - 2 + offset,
                byte,
            );
        }
        self.write_byte(
            BLUETOOTH_PASSIVE_SCAN_RX_PACKET_PREFIX_BYTES - 15,
            rssi_dbm as u8,
        );
    }

    #[cfg(test)]
    fn write_byte(&self, offset: usize, value: u8) {
        let shift = (offset % 4) * 8;
        let word = self.words[offset / 4].get();
        self.words[offset / 4].set((word & !(0xff << shift)) | (u32::from(value) << shift));
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

    fn image(&self) -> BluetoothPassiveScanLinkStateImage {
        BluetoothPassiveScanLinkStateImage::from_words(core::array::from_fn(|index| {
            self.words[index].get()
        }))
    }

    fn update_controller_time(&self, controller_time: BluetoothControllerLatchedTime) {
        self.install(self.image().with_controller_time(controller_time));
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
        index: usize,
        allocation: BluetoothPassiveScanSchedulerAllocationConfig,
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
        self.words[SCHEDULER_ITEM_ALLOCATION_FLAGS_WORD].set(SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE);
        self.words[SCHEDULER_ITEM_ALLOCATION_CONFIG_WORD].set(allocation.item_image(index));
        self.words[SCHEDULER_ITEM_POSITIONAL_24_WORD].set(SCHEDULER_ITEM_POSITIONAL_24_IMAGE);
        self.words[SCHEDULER_ITEM_EVENT_CLASS_WORD].set(SCHEDULER_ITEM_EVENT_CLASS_IMAGE);
    }

    fn reviewed_words(&self) -> BluetoothPassiveScanSchedulerItemWords {
        BluetoothPassiveScanSchedulerItemWords {
            word_00: self.words[SCHEDULER_ITEM_HARDWARE_NEXT_WORD].get(),
            word_04: self.words[SCHEDULER_ITEM_CONTEXT_WORD].get(),
            word_14: self.words[SCHEDULER_ITEM_WORD_14].get(),
            word_18: self.words[SCHEDULER_ITEM_WORD_18].get(),
            word_38: self.words[SCHEDULER_ITEM_WORD_38].get(),
            raw_start_word_44: self.words[SCHEDULER_ITEM_WORD_44].get(),
            raw_end_word_48: self.words[SCHEDULER_ITEM_WORD_48].get(),
        }
    }

    fn detach_hardware_predecessor(&self) {
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_WORD]
            .set(self.words[SCHEDULER_ITEM_HARDWARE_NEXT_WORD].get() & !SCHEDULER_ITEM_LINK_MASK);
    }

    fn mark_in_flight(&self) {
        self.words[SCHEDULER_ITEM_WORD_38].set(u32::MAX);
    }

    fn restore_cpu_owned_status(&self) {
        self.words[SCHEDULER_ITEM_WORD_38].set(0);
    }

    fn completion_status(&self) -> Option<BluetoothPassiveScanSchedulerItemCompletionStatus> {
        let status = self.words[SCHEDULER_ITEM_WORD_38].get();
        if status == u32::MAX {
            None
        } else if status == 0 {
            Some(BluetoothPassiveScanSchedulerItemCompletionStatus::Zero)
        } else {
            Some(BluetoothPassiveScanSchedulerItemCompletionStatus::NonZero(
                core::num::NonZeroU32::new(status)
                    .expect("a nonzero scheduler status constructs a nonzero value"),
            ))
        }
    }

    fn restore_hardware_predecessor(&self, predecessor: BluetoothControllerSramLinkAddress) {
        let image = self.words[SCHEDULER_ITEM_HARDWARE_NEXT_WORD].get();
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_WORD]
            .set((image & !SCHEDULER_ITEM_LINK_MASK) | predecessor.compressed_image());
    }

    fn write_reviewed_words(&self, words: BluetoothPassiveScanSchedulerItemWords) {
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_WORD].set(words.word_00);
        self.words[SCHEDULER_ITEM_CONTEXT_WORD].set(words.word_04);
        self.words[SCHEDULER_ITEM_WORD_14].set(words.word_14);
        self.words[SCHEDULER_ITEM_WORD_18].set(words.word_18);
        self.words[SCHEDULER_ITEM_WORD_38].set(words.word_38);
        self.words[SCHEDULER_ITEM_WORD_44].set(words.raw_start_word_44);
        self.words[SCHEDULER_ITEM_WORD_48].set(words.raw_end_word_48);
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

    /// Lower the first accepted passive LE 1M window into the current item.
    pub fn prepare_first_event(
        mut self,
        channel: BluetoothPassiveScanPrimaryChannel,
        window: BluetoothPassiveScanSchedulerWindow,
        start_selection: BluetoothPassiveScanStartSelection,
        controller_time: BluetoothControllerLatchedTime,
    ) -> BluetoothPassiveScanMemoryGraphEventPrepared {
        let item_index = BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT - 1;
        let graph = self.storage.as_mut().project();
        graph.link_state.update_controller_time(controller_time);
        let words = graph.scheduler_items[item_index]
            .reviewed_words()
            .prepare_first_event(graph.link_state.image(), channel, window, start_selection);
        graph.scheduler_items[item_index].write_reviewed_words(words);
        BluetoothPassiveScanMemoryGraphEventPrepared {
            storage: self.storage,
            binding: self.binding,
            channel,
            window,
        }
    }
}

/// CPU-owned scanner graph carrying one complete first-event image.
#[must_use = "the prepared scanner event must be admitted or retained"]
pub struct BluetoothPassiveScanMemoryGraphEventPrepared {
    storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
    channel: BluetoothPassiveScanPrimaryChannel,
    window: BluetoothPassiveScanSchedulerWindow,
}

impl BluetoothPassiveScanMemoryGraphEventPrepared {
    /// Primary channel retained by this exact event.
    pub const fn channel(&self) -> BluetoothPassiveScanPrimaryChannel {
        self.channel
    }

    /// Scheduler window retained by this exact event.
    pub const fn window(&self) -> BluetoothPassiveScanSchedulerWindow {
        self.window
    }

    /// Detach the prepared item from the private free chain before admission.
    ///
    /// This transition remains CPU-only and is cancellable. It advances the
    /// private free head to the retained predecessor while clearing the
    /// selected item's hardware-next link.
    pub fn prepare_scheduler_admission(
        mut self,
    ) -> BluetoothPassiveScanMemoryGraphSchedulerAdmissionPrepared {
        let selected_index = BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT - 1;
        let predecessor = self.binding.scheduler_items[selected_index - 1];
        let graph = self.storage.as_mut().project();
        graph.scheduler_items[selected_index].detach_hardware_predecessor();
        graph.scheduler_items[selected_index].mark_in_flight();
        graph
            .link_state
            .install_scheduler_head(predecessor.controller_address());
        BluetoothPassiveScanMemoryGraphSchedulerAdmissionPrepared {
            storage: self.storage,
            binding: self.binding,
            channel: self.channel,
            window: self.window,
        }
    }
}

/// CPU-owned scanner graph whose selected item is detached from its free chain.
#[must_use = "the detached scanner item must be published or restored"]
pub struct BluetoothPassiveScanMemoryGraphSchedulerAdmissionPrepared {
    storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
    channel: BluetoothPassiveScanPrimaryChannel,
    window: BluetoothPassiveScanSchedulerWindow,
}

impl BluetoothPassiveScanMemoryGraphSchedulerAdmissionPrepared {
    /// Exact selected item that may consume one common scheduler list.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_head()
    }

    /// Restore the exact private chain before any MMIO publication.
    pub fn cancel(mut self) -> BluetoothPassiveScanMemoryGraphEventPrepared {
        let selected_index = BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT - 1;
        let selected = self.binding.scheduler_items[selected_index];
        let predecessor = self.binding.scheduler_items[selected_index - 1];
        let graph = self.storage.as_mut().project();
        graph.scheduler_items[selected_index].restore_hardware_predecessor(predecessor);
        graph.scheduler_items[selected_index].restore_cpu_owned_status();
        graph
            .link_state
            .install_scheduler_head(selected.controller_address());
        BluetoothPassiveScanMemoryGraphEventPrepared {
            storage: self.storage,
            binding: self.binding,
            channel: self.channel,
            window: self.window,
        }
    }

    /// Freeze CPU initialization before an upper controller owner performs the
    /// ordered RX-list publication.
    pub fn prepare_publication(self) -> BluetoothPassiveScanMemoryGraphPublicationPrepared {
        BluetoothPassiveScanMemoryGraphPublicationPrepared {
            storage: self.storage,
            binding: self.binding,
            channel: self.channel,
            window: self.window,
        }
    }
}

/// Initialized pinned graph ready for selector-one list publication.
#[must_use = "the prepared scanner graph must be published or retained"]
pub struct BluetoothPassiveScanMemoryGraphPublicationPrepared {
    storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
    channel: BluetoothPassiveScanPrimaryChannel,
    window: BluetoothPassiveScanSchedulerWindow,
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
            channel: self.channel,
            window: self.window,
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
#[must_use = "the published scanner graph remains hardware-owned"]
pub struct BluetoothPassiveScanMemoryGraphPublished {
    _storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
    publication: BluetoothRxMemoryListPublished,
    channel: BluetoothPassiveScanPrimaryChannel,
    window: BluetoothPassiveScanSchedulerWindow,
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

    /// Primary channel retained by the hardware-owned event.
    pub const fn channel(&self) -> BluetoothPassiveScanPrimaryChannel {
        self.channel
    }

    /// Scheduler window retained by the hardware-owned event.
    pub const fn window(&self) -> BluetoothPassiveScanSchedulerWindow {
        self.window
    }

    /// Join the matching restricted scan command while retaining the graph.
    pub fn into_scan_command_published(
        self,
        command: BluetoothScanStartPublished,
    ) -> BluetoothPassiveScanMemoryGraphCommandPublished {
        BluetoothPassiveScanMemoryGraphCommandPublished {
            _storage: self._storage,
            binding: self.binding,
            rx_publication: self.publication,
            _command: command,
            channel: self.channel,
            window: self.window,
        }
    }
}

/// Scanner graph whose RX list and standard-backoff command are hardware-visible.
///
/// The scheduler item remains outside a hardware list until the common
/// scheduler epoch consumes this state.
#[must_use = "the command-published scanner graph must enter the common scheduler"]
pub struct BluetoothPassiveScanMemoryGraphCommandPublished {
    _storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
    rx_publication: BluetoothRxMemoryListPublished,
    _command: BluetoothScanStartPublished,
    channel: BluetoothPassiveScanPrimaryChannel,
    window: BluetoothPassiveScanSchedulerWindow,
}

impl BluetoothPassiveScanMemoryGraphCommandPublished {
    /// Exact detached scheduler item prepared by this graph.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_head()
    }

    /// Borrow the retained selector-one RX-list publication.
    #[doc(hidden)]
    pub const fn rx_publication(&self) -> &BluetoothRxMemoryListPublished {
        &self.rx_publication
    }

    /// Primary channel retained by the hardware-visible event.
    pub const fn channel(&self) -> BluetoothPassiveScanPrimaryChannel {
        self.channel
    }

    /// Scheduler window retained by the hardware-visible event.
    pub const fn window(&self) -> BluetoothPassiveScanSchedulerWindow {
        self.window
    }

    /// Join the exact common RUN proof and retain hardware ownership.
    pub fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothPassiveScanMemoryGraphRunning {
        assert_eq!(
            run.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "the restricted scanner uses the primary scheduler list"
        );
        assert_eq!(
            run.head().address(),
            Some(self.scheduler_head()),
            "the RUN proof must retain this scanner item"
        );
        BluetoothPassiveScanMemoryGraphRunning {
            storage: self._storage,
            binding: self.binding,
            _rx_publication: self.rx_publication,
            _command: self._command,
            channel: self.channel,
            window: self.window,
        }
    }
}

/// Hardware-owned scanner graph admitted through the common RUN transaction.
#[must_use = "the running scanner graph must advance through fenced completion"]
pub struct BluetoothPassiveScanMemoryGraphRunning {
    storage: Pin<&'static mut BluetoothPassiveScanMemoryGraphStorage>,
    binding: BluetoothPassiveScanMemoryGraphBinding,
    _rx_publication: BluetoothRxMemoryListPublished,
    _command: BluetoothScanStartPublished,
    channel: BluetoothPassiveScanPrimaryChannel,
    window: BluetoothPassiveScanSchedulerWindow,
}

impl BluetoothPassiveScanMemoryGraphRunning {
    /// Exact scanner item retained by the hardware-owned graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_head()
    }

    /// Primary advertising channel retained by this event.
    pub const fn channel(&self) -> BluetoothPassiveScanPrimaryChannel {
        self.channel
    }

    /// Exact scheduler window retained by this event.
    pub const fn window(&self) -> BluetoothPassiveScanSchedulerWindow {
        self.window
    }

    /// Consume one fresh finished-list observation and inspect the item status.
    pub fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothPassiveScanMemoryGraphCompletionObservation {
        if observed.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            return BluetoothPassiveScanMemoryGraphCompletionObservation::ListMismatch {
                running: self,
                observed,
            };
        }
        let selected_index = BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT - 1;
        let Some(status) =
            self.storage.as_ref().get_ref().scheduler_items[selected_index].completion_status()
        else {
            return BluetoothPassiveScanMemoryGraphCompletionObservation::StillInFlight(self);
        };
        BluetoothPassiveScanMemoryGraphCompletionObservation::CompletionObserved(
            BluetoothPassiveScanMemoryGraphCompletionObserved {
                running: self,
                status,
            },
        )
    }
}

/// One bounded observation of a running scanner graph.
#[must_use = "the graph and any unrelated finished-list token remain owned"]
pub enum BluetoothPassiveScanMemoryGraphCompletionObservation {
    ListMismatch {
        running: BluetoothPassiveScanMemoryGraphRunning,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothPassiveScanMemoryGraphRunning),
    CompletionObserved(BluetoothPassiveScanMemoryGraphCompletionObserved),
}

/// Scanner graph after a non-sentinel scheduler status was observed.
#[must_use = "the completed scanner graph must pass scheduler unlink before CPU access"]
pub struct BluetoothPassiveScanMemoryGraphCompletionObserved {
    running: BluetoothPassiveScanMemoryGraphRunning,
    status: BluetoothPassiveScanSchedulerItemCompletionStatus,
}

impl BluetoothPassiveScanMemoryGraphCompletionObserved {
    /// Exact item whose scheduler status left the in-flight sentinel.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.running.scheduler_item_address()
    }

    /// Semantic scheduler completion status retained for diagnostics.
    pub const fn status(&self) -> BluetoothPassiveScanSchedulerItemCompletionStatus {
        self.status
    }

    /// Bind the exact software-list removal proof before reading RX SRAM.
    pub fn prepare_recycle_after_software_list_removal(
        self,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        BluetoothPassiveScanMemoryGraphRecyclePrepared,
        BluetoothPassiveScanMemoryGraphRecycleFailure,
    > {
        let error = if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(BluetoothPassiveScanMemoryGraphRecycleError::HardwareListMismatch)
        } else if removal.completed_head().address() != Some(self.scheduler_item_address()) {
            Some(BluetoothPassiveScanMemoryGraphRecycleError::SchedulerItemMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(BluetoothPassiveScanMemoryGraphRecycleFailure {
                completed: self,
                removal,
                error,
            });
        }
        Ok(BluetoothPassiveScanMemoryGraphRecyclePrepared {
            completed: self,
            _removal: removal,
        })
    }
}

/// Why a completed scanner graph rejected CPU-recycle authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanMemoryGraphRecycleError {
    HardwareListMismatch,
    SchedulerItemMismatch,
}

/// Lossless recycle rejection retaining both affine owners.
#[must_use = "the completed scanner graph and removal proof remain owned"]
pub struct BluetoothPassiveScanMemoryGraphRecycleFailure {
    completed: BluetoothPassiveScanMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    error: BluetoothPassiveScanMemoryGraphRecycleError,
}

impl BluetoothPassiveScanMemoryGraphRecycleFailure {
    pub const fn error(&self) -> BluetoothPassiveScanMemoryGraphRecycleError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothPassiveScanMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

/// Completed graph authorized for bounded RX extraction and reclamation.
#[must_use = "the scanner graph must be extracted or retained unchanged"]
pub struct BluetoothPassiveScanMemoryGraphRecyclePrepared {
    completed: BluetoothPassiveScanMemoryGraphCompletionObserved,
    _removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl BluetoothPassiveScanMemoryGraphRecyclePrepared {
    /// Validate and copy every contiguous completed node without mutating SRAM.
    pub fn extract_received(
        self,
    ) -> Result<
        BluetoothPassiveScanMemoryGraphRxExtracted,
        BluetoothPassiveScanMemoryGraphRxExtractionFailure,
    > {
        let batch = match self
            .completed
            .running
            .storage
            .as_ref()
            .get_ref()
            .extract_received_batch()
        {
            Ok(batch) => batch,
            Err(error) => {
                return Err(BluetoothPassiveScanMemoryGraphRxExtractionFailure {
                    prepared: self,
                    error,
                });
            }
        };
        Ok(BluetoothPassiveScanMemoryGraphRxExtracted {
            prepared: self,
            batch,
        })
    }
}

/// Malformed completed RX storage retaining the unchanged recycle owner.
#[must_use = "the unchanged graph remains unavailable until fail-stop handling"]
pub struct BluetoothPassiveScanMemoryGraphRxExtractionFailure {
    prepared: BluetoothPassiveScanMemoryGraphRecyclePrepared,
    error: BluetoothPassiveScanRxError,
}

impl BluetoothPassiveScanMemoryGraphRxExtractionFailure {
    pub const fn error(&self) -> BluetoothPassiveScanRxError {
        self.error
    }

    pub fn into_prepared(self) -> BluetoothPassiveScanMemoryGraphRecyclePrepared {
        self.prepared
    }
}

/// Validated RX batch paired with the sole reclaimable scanner graph.
#[must_use = "commit reclamation before reusing the scanner graph"]
pub struct BluetoothPassiveScanMemoryGraphRxExtracted {
    prepared: BluetoothPassiveScanMemoryGraphRecyclePrepared,
    batch: BluetoothPassiveScanReceivedBatch,
}

impl BluetoothPassiveScanMemoryGraphRxExtracted {
    /// Copy of every completed Link Layer PDU in receive-list order.
    pub const fn batch(&self) -> BluetoothPassiveScanReceivedBatch {
        self.batch
    }

    /// Restore the private lists and return ordinary CPU ownership.
    pub fn commit(self) -> BluetoothPassiveScanMemoryGraphRecycled {
        let BluetoothPassiveScanMemoryGraphRecyclePrepared {
            completed,
            _removal: _,
        } = self.prepared;
        let BluetoothPassiveScanMemoryGraphCompletionObserved { running, status } = completed;
        let BluetoothPassiveScanMemoryGraphRunning {
            storage,
            binding,
            _rx_publication: _,
            _command: _,
            channel: _,
            window: _,
        } = running;
        let mut owner = BluetoothPassiveScanMemoryGraphCpuOwned { storage, binding };
        owner.restore_after_event();
        BluetoothPassiveScanMemoryGraphRecycled {
            owner,
            batch: self.batch,
            status,
        }
    }
}

/// Reusable CPU-owned scanner graph plus copied event results.
#[must_use = "the graph and received batch must return to the scanner role owner"]
pub struct BluetoothPassiveScanMemoryGraphRecycled {
    owner: BluetoothPassiveScanMemoryGraphCpuOwned,
    batch: BluetoothPassiveScanReceivedBatch,
    status: BluetoothPassiveScanSchedulerItemCompletionStatus,
}

impl BluetoothPassiveScanMemoryGraphRecycled {
    pub fn into_parts(
        self,
    ) -> (
        BluetoothPassiveScanMemoryGraphCpuOwned,
        BluetoothPassiveScanReceivedBatch,
        BluetoothPassiveScanSchedulerItemCompletionStatus,
    ) {
        (self.owner, self.batch, self.status)
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

    fn extract_received_batch(
        &self,
    ) -> Result<BluetoothPassiveScanReceivedBatch, BluetoothPassiveScanRxError> {
        let mut batch = BluetoothPassiveScanReceivedBatch::empty();
        let mut incomplete_observed = false;
        for node in &self.nodes {
            if !node.header.completion_observed() {
                incomplete_observed = true;
                continue;
            }
            if incomplete_observed {
                return Err(BluetoothPassiveScanRxError::CompletionChainGap);
            }
            batch.push(node.packet.received_pdu()?);
        }
        Ok(batch)
    }

    /// Bind the real address of one unique static S31 allocation.
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
        config: BluetoothPassiveScanResetConfig,
        scheduler_allocation: BluetoothPassiveScanSchedulerAllocationConfig,
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
        Self::pin_static_inner(storage, base, config, scheduler_allocation)
    }

    /// Bind one deterministic physical-SRAM address to a native ownership
    /// model without deriving an address from the host allocation.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothPassiveScanMemoryGraphModelAddress,
        config: BluetoothPassiveScanResetConfig,
        scheduler_allocation: BluetoothPassiveScanSchedulerAllocationConfig,
    ) -> Result<BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanMemoryGraphBindFailure>
    {
        Self::pin_static_inner(storage, base.address(), config, scheduler_allocation)
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
        config: BluetoothPassiveScanResetConfig,
        scheduler_allocation: BluetoothPassiveScanSchedulerAllocationConfig,
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
        owner.initialize(config, scheduler_allocation);
        Ok(owner)
    }
}

impl BluetoothPassiveScanMemoryGraphCpuOwned {
    fn initialize(
        &mut self,
        config: BluetoothPassiveScanResetConfig,
        scheduler_allocation: BluetoothPassiveScanSchedulerAllocationConfig,
    ) {
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
                index,
                scheduler_allocation,
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

    fn restore_after_event(&mut self) {
        let bindings = self.binding.nodes;
        let scheduler_items = self.binding.scheduler_items;
        let selected_index = BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT - 1;
        let storage = self.storage.as_mut().project();
        storage.scheduler_items[selected_index]
            .restore_hardware_predecessor(scheduler_items[selected_index - 1]);
        storage.scheduler_items[selected_index].restore_cpu_owned_status();
        storage
            .link_state
            .install_scheduler_head(scheduler_items[selected_index].controller_address());
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
        BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanPrimaryChannel,
        BluetoothPassiveScanResetConfig, BluetoothPassiveScanSchedulerWindow,
        BluetoothPassiveScanStartSelection,
        le_phy_packet::{BluetoothLeAccessAddress, BluetoothLeCrcInit},
        passive_scanning_event_image::BluetoothPassiveScanRxHeadProjection,
    };

    use super::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BluetoothPassiveScanMemoryGraphBindError,
        BluetoothPassiveScanMemoryGraphModelAddress, BluetoothPassiveScanMemoryGraphStorage,
        BluetoothPassiveScanSchedulerAllocationConfig,
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
        BluetoothPassiveScanMemoryGraphStorage::pin_static_model(
            storage,
            address,
            reset_config(),
            BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
                .expect("the restricted product limits fit every scanner item"),
        )
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
        let window = BluetoothPassiveScanSchedulerWindow::from_controller_ticks(500, 1_500)
            .expect("the first scan window is non-empty");
        let event = owner.prepare_first_event(
            BluetoothPassiveScanPrimaryChannel::Channel37,
            window,
            BluetoothPassiveScanStartSelection::Requested,
            BluetoothControllerLatchedTime::from_bits(0x2345_6789),
        );
        assert_eq!(
            event.channel(),
            BluetoothPassiveScanPrimaryChannel::Channel37
        );
        assert_eq!(event.window(), window);
        assert_eq!(
            event
                .storage
                .as_ref()
                .get_ref()
                .link_state
                .image()
                .controller_time(),
            0x2345_6789
        );
        let event = event.prepare_scheduler_admission().cancel();
        let prepared = event.prepare_scheduler_admission().prepare_publication();
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
            BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
                .expect("the restricted product limits fit every scanner item"),
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

    #[test]
    fn completed_receive_nodes_copy_only_bounded_pdu_and_signed_rssi() {
        let owner = model_graph(0x2f00_1000);
        let storage = owner.storage.as_ref().get_ref();
        let pdu = [0x02, 6, 1, 2, 3, 4, 5, 6];
        storage.nodes[0].packet.emulate_hardware_receive(&pdu, -47);
        storage.nodes[0].header.emulate_hardware_completion();

        let batch = storage
            .extract_received_batch()
            .expect("one completed prefix node is a valid receive batch");
        assert_eq!(batch.len(), 1);
        let packet = batch.packet(0).expect("the completed node is retained");
        assert_eq!(packet.as_bytes(), &pdu);
        assert_eq!(packet.rssi_dbm(), -47);
        assert!(batch.packet(1).is_none());
    }

    #[test]
    fn completed_receive_chain_rejects_a_gap_before_packet_access() {
        let owner = model_graph(0x2f00_2000);
        let storage = owner.storage.as_ref().get_ref();
        storage.nodes[1]
            .packet
            .emulate_hardware_receive(&[0x02, 6, 1, 2, 3, 4, 5, 6], -20);
        storage.nodes[1].header.emulate_hardware_completion();

        assert_eq!(
            storage.extract_received_batch(),
            Err(super::BluetoothPassiveScanRxError::CompletionChainGap)
        );
    }

    #[test]
    fn scheduler_limits_must_fit_every_retained_item() {
        assert!(BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0).is_some());
        assert!(BluetoothPassiveScanSchedulerAllocationConfig::new(u16::MAX, u16::MAX).is_none());
    }
}
