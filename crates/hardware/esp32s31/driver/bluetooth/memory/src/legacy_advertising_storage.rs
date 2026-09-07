//! Pinned CPU-owned storage for the first ESP32-S31 legacy advertising role.
//!
//! This module closes allocation and the stable links between the common TX
//! packet/header, advertising link state, scheduler context and first
//! scheduler item. A later CPU-owned transition can synthesize the first event
//! image, but no hardware list can be published from these states.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, num::NonZeroU32, pin::Pin};

use crate::{
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxBufferHeaderStorage, LeTxPacketAddress,
        LeTxPacketPrepareError, LeTxPacketPreparedLength, LeTxPacketStorage,
    },
    legacy_advertising_event_image::{
        LegacyAdvertisingLinkStateWords, LegacyAdvertisingOwnAddress, LegacyAdvertisingPduError,
        LegacyAdvertisingPrimaryChannelPlan, LegacyAdvertisingSchedulerItemWords,
    },
    scheduler_context::SchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        ControllerSramLinkAddress,
    },
};

use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothSchedulerFinishedHardwareListObserved,
        BluetoothSchedulerHardwareListHeadPublished, BluetoothSchedulerHardwareListIndex,
        BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerSoftwareListRemovalReady,
    },
    types::{BluetoothControllerSramAddress, BluetoothControllerSramAddressError},
};

use pin_project::pin_project;

use vcell::VolatileCell;

/// Bytes reserved for the advertising link-state object.
pub const BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES: usize = 0x84;
/// Bytes reserved for one advertising scheduler item.
pub const BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Maximum primary-channel items in one legacy advertising event.
pub const BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY: usize = 3;
/// Maximum payload after the two-byte legacy advertising PDU header.
pub const BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = 37;
/// Complete controller TX allocation for the maximum legacy advertising PDU.
pub const BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

const LINK_STATE_WORDS: usize = BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES / 4;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES / 4;

const LINK_STATE_SCHEDULER_HEAD_OFFSET: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD_OFFSET: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD_OFFSET: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL_OFFSET: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL_OFFSET: usize = 0x74 / 4;
const LINK_STATE_RX_SWAP_RESERVE_OFFSET: usize = 0x78 / 4;
const LINK_STATE_ALLOCATION_CONFIG_OFFSET: usize = 0x30 / 4;
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: u32 = 0x0000_1e00;

const LINK_STATE_WORD_00_OFFSET: usize = 0;
const LINK_STATE_WORD_04_OFFSET: usize = 1;
const LINK_STATE_WORD_08_OFFSET: usize = 2;
const LINK_STATE_WORD_0C_OFFSET: usize = 3;
const LINK_STATE_WORD_14_OFFSET: usize = 0x14 / 4;
const LINK_STATE_WORD_18_OFFSET: usize = 0x18 / 4;
const LINK_STATE_WORD_24_OFFSET: usize = 0x24 / 4;
const LINK_STATE_WORD_2C_OFFSET: usize = 0x2c / 4;
const LINK_STATE_WORD_30_OFFSET: usize = 0x30 / 4;
const LINK_STATE_WORD_34_OFFSET: usize = 0x34 / 4;
const LINK_STATE_WORD_38_OFFSET: usize = 0x38 / 4;
const LINK_STATE_WORD_3C_OFFSET: usize = 0x3c / 4;
const LINK_STATE_WORD_40_OFFSET: usize = 0x40 / 4;
const LINK_STATE_WORD_50_OFFSET: usize = 0x50 / 4;
const LINK_STATE_WORD_60_OFFSET: usize = 0x60 / 4;

const SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET: usize = 0;
const SCHEDULER_ITEM_CONTEXT_OFFSET: usize = 1;
const SCHEDULER_ITEM_LINK_STATE_OFFSET: usize = 0x08 / 4;
const SCHEDULER_ITEM_HARDWARE_NEXT_MASK: u32 = 0x000f_ffff;
const SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE: u32 = 0x0010_0000;
const SCHEDULER_ITEM_LINK_STATE_PREFIX_IMAGE: u32 = 0x0060_0000;
const SCHEDULER_ITEM_WORD_14_OFFSET: usize = 0x14 / 4;
const SCHEDULER_ITEM_WORD_18_OFFSET: usize = 0x18 / 4;
const SCHEDULER_ITEM_WORD_38_OFFSET: usize = 0x38 / 4;
const SCHEDULER_ITEM_WORD_44_OFFSET: usize = 0x44 / 4;
const SCHEDULER_ITEM_WORD_48_OFFSET: usize = 0x48 / 4;
const SCHEDULER_ITEM_WORD_4C_OFFSET: usize = 0x4c / 4;
const SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET: usize = 0x50 / 4;
const SCHEDULER_ITEM_COMPLETED_LINK_OFFSET: usize = 0x54 / 4;

type AdvertisingTxPacketAddress = LeTxPacketAddress<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketLength =
    LeTxPacketPreparedLength<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;

/// Opaque advertising link-state allocation before event-time preparation.
#[repr(C, align(4))]
struct LegacyAdvertisingLinkStateStorage {
    words: [VolatileCell<u32>; LINK_STATE_WORDS],
}

impl LegacyAdvertisingLinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; LINK_STATE_WORDS],
        }
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn initialize_graph(
        &self,
        scheduler_item: ControllerSramLinkAddress,
        tx_header: ControllerSramLinkAddress,
    ) {
        self.clear();
        self.words[LINK_STATE_SCHEDULER_HEAD_OFFSET]
            .set(scheduler_item.controller_address().address());
        self.words[LINK_STATE_RX_HEAD_OFFSET].set(0);
        self.words[LINK_STATE_TX_HEAD_OFFSET].set(tx_header.controller_address().address());
        self.words[LINK_STATE_RX_TAIL_OFFSET].set(0);
        self.words[LINK_STATE_TX_TAIL_OFFSET].set(tx_header.controller_address().address());
        self.words[LINK_STATE_RX_SWAP_RESERVE_OFFSET].set(0);
        self.words[LINK_STATE_ALLOCATION_CONFIG_OFFSET].set(LINK_STATE_ALLOCATION_CONFIG_IMAGE);
    }

    fn reviewed_words(&self) -> LegacyAdvertisingLinkStateWords {
        LegacyAdvertisingLinkStateWords {
            word_00: self.words[LINK_STATE_WORD_00_OFFSET].get(),
            word_04: self.words[LINK_STATE_WORD_04_OFFSET].get(),
            word_08: self.words[LINK_STATE_WORD_08_OFFSET].get(),
            word_0c: self.words[LINK_STATE_WORD_0C_OFFSET].get(),
            word_14: self.words[LINK_STATE_WORD_14_OFFSET].get(),
            word_18: self.words[LINK_STATE_WORD_18_OFFSET].get(),
            word_24: self.words[LINK_STATE_WORD_24_OFFSET].get(),
            crc_init_word_2c: self.words[LINK_STATE_WORD_2C_OFFSET].get(),
            word_30: self.words[LINK_STATE_WORD_30_OFFSET].get(),
            word_34: self.words[LINK_STATE_WORD_34_OFFSET].get(),
            access_address_word_38: self.words[LINK_STATE_WORD_38_OFFSET].get(),
            word_3c: self.words[LINK_STATE_WORD_3C_OFFSET].get(),
            word_40: self.words[LINK_STATE_WORD_40_OFFSET].get(),
            word_50: self.words[LINK_STATE_WORD_50_OFFSET].get(),
            word_60: self.words[LINK_STATE_WORD_60_OFFSET].get(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyAdvertisingLinkStateWords) {
        self.words[LINK_STATE_WORD_00_OFFSET].set(words.word_00);
        self.words[LINK_STATE_WORD_04_OFFSET].set(words.word_04);
        self.words[LINK_STATE_WORD_08_OFFSET].set(words.word_08);
        self.words[LINK_STATE_WORD_0C_OFFSET].set(words.word_0c);
        self.words[LINK_STATE_WORD_14_OFFSET].set(words.word_14);
        self.words[LINK_STATE_WORD_18_OFFSET].set(words.word_18);
        self.words[LINK_STATE_WORD_24_OFFSET].set(words.word_24);
        self.words[LINK_STATE_WORD_2C_OFFSET].set(words.crc_init_word_2c);
        self.words[LINK_STATE_WORD_30_OFFSET].set(words.word_30);
        self.words[LINK_STATE_WORD_34_OFFSET].set(words.word_34);
        self.words[LINK_STATE_WORD_38_OFFSET].set(words.access_address_word_38);
        self.words[LINK_STATE_WORD_3C_OFFSET].set(words.word_3c);
        self.words[LINK_STATE_WORD_40_OFFSET].set(words.word_40);
        self.words[LINK_STATE_WORD_50_OFFSET].set(words.word_50);
        self.words[LINK_STATE_WORD_60_OFFSET].set(words.word_60);
    }

    fn reset_restricted_profile(
        &self,
        tx_header: ControllerSramLinkAddress,
        own_address: LegacyAdvertisingOwnAddress,
        default_tx_power_dbm: i8,
    ) {
        self.write_reviewed_words(self.reviewed_words().reset(
            tx_header,
            own_address,
            default_tx_power_dbm,
        ));
    }

    fn scheduler_head(&self) -> u32 {
        self.words[LINK_STATE_SCHEDULER_HEAD_OFFSET].get()
    }

    fn detach_first_scheduler_item(&self) {
        self.words[LINK_STATE_SCHEDULER_HEAD_OFFSET].set(0);
    }

    #[cfg(test)]
    fn retains_graph(
        &self,
        scheduler_item: ControllerSramLinkAddress,
        tx_header: ControllerSramLinkAddress,
    ) -> bool {
        self.words[LINK_STATE_SCHEDULER_HEAD_OFFSET].get()
            == scheduler_item.controller_address().address()
            && self.words[LINK_STATE_RX_HEAD_OFFSET].get() == 0
            && self.words[LINK_STATE_TX_HEAD_OFFSET].get()
                == tx_header.controller_address().address()
            && self.words[LINK_STATE_RX_TAIL_OFFSET].get() == 0
            && self.words[LINK_STATE_TX_TAIL_OFFSET].get()
                == tx_header.controller_address().address()
            && self.words[LINK_STATE_RX_SWAP_RESERVE_OFFSET].get() == 0
            && self.words[LINK_STATE_ALLOCATION_CONFIG_OFFSET].get()
                == LINK_STATE_ALLOCATION_CONFIG_IMAGE
    }
}

/// Opaque first advertising scheduler-item allocation.
#[repr(C, align(4))]
struct LegacyAdvertisingSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl LegacyAdvertisingSchedulerItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn initialize_graph(
        &self,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
    ) {
        self.clear();
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET].set(SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE);
        self.words[SCHEDULER_ITEM_CONTEXT_OFFSET].set(scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE_OFFSET]
            .set(SCHEDULER_ITEM_LINK_STATE_PREFIX_IMAGE | link_state.compressed_image());
    }

    fn is_terminal(&self) -> bool {
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET].get() & SCHEDULER_ITEM_HARDWARE_NEXT_MASK
            == 0
    }

    fn reviewed_words(&self) -> LegacyAdvertisingSchedulerItemWords {
        LegacyAdvertisingSchedulerItemWords {
            word_00: self.words[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET].get(),
            word_04: self.words[SCHEDULER_ITEM_CONTEXT_OFFSET].get(),
            word_14: self.words[SCHEDULER_ITEM_WORD_14_OFFSET].get(),
            word_18: self.words[SCHEDULER_ITEM_WORD_18_OFFSET].get(),
            word_38: self.words[SCHEDULER_ITEM_WORD_38_OFFSET].get(),
            raw_start_word_44: self.words[SCHEDULER_ITEM_WORD_44_OFFSET].get(),
            raw_end_word_48: self.words[SCHEDULER_ITEM_WORD_48_OFFSET].get(),
            word_4c: self.words[SCHEDULER_ITEM_WORD_4C_OFFSET].get(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyAdvertisingSchedulerItemWords) {
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET].set(words.word_00);
        self.words[SCHEDULER_ITEM_CONTEXT_OFFSET].set(words.word_04);
        self.words[SCHEDULER_ITEM_WORD_14_OFFSET].set(words.word_14);
        self.words[SCHEDULER_ITEM_WORD_18_OFFSET].set(words.word_18);
        self.words[SCHEDULER_ITEM_WORD_38_OFFSET].set(words.word_38);
        self.words[SCHEDULER_ITEM_WORD_44_OFFSET].set(words.raw_start_word_44);
        self.words[SCHEDULER_ITEM_WORD_48_OFFSET].set(words.raw_end_word_48);
        self.words[SCHEDULER_ITEM_WORD_4C_OFFSET].set(words.word_4c);
    }

    #[cfg(test)]
    fn retains_graph(
        &self,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
    ) -> bool {
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT_OFFSET].get() & SCHEDULER_ITEM_HARDWARE_NEXT_MASK
            == 0
            && self.words[SCHEDULER_ITEM_CONTEXT_OFFSET].get()
                == scheduler_context.compressed_image()
            && self.words[SCHEDULER_ITEM_LINK_STATE_OFFSET].get()
                == SCHEDULER_ITEM_LINK_STATE_PREFIX_IMAGE | link_state.compressed_image()
    }
}

/// Stable storage for one restricted legacy advertising event graph.
#[pin_project]
#[repr(C)]
pub struct LegacyAdvertisingMemoryGraphStorage {
    link_state: LegacyAdvertisingLinkStateStorage,
    scheduler_context: SchedulerContextStorage,
    scheduler_items: [LegacyAdvertisingSchedulerItemStorage;
        BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
    tx_header: LeTxBufferHeaderStorage,
    tx_packet: LeTxPacketStorage<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>,
    #[pin]
    _pin: PhantomPinned,
}

const GRAPH_BYTES: u32 = core::mem::size_of::<LegacyAdvertisingMemoryGraphStorage>() as u32;
const LINK_STATE_OFFSET: u32 =
    core::mem::offset_of!(LegacyAdvertisingMemoryGraphStorage, link_state) as u32;
const SCHEDULER_ITEMS_OFFSET: u32 =
    core::mem::offset_of!(LegacyAdvertisingMemoryGraphStorage, scheduler_items) as u32;
const SCHEDULER_CONTEXT_OFFSET: u32 =
    core::mem::offset_of!(LegacyAdvertisingMemoryGraphStorage, scheduler_context) as u32;
const TX_HEADER_OFFSET: u32 =
    core::mem::offset_of!(LegacyAdvertisingMemoryGraphStorage, tx_header) as u32;
const TX_PACKET_OFFSET: u32 =
    core::mem::offset_of!(LegacyAdvertisingMemoryGraphStorage, tx_packet) as u32;

/// Why static advertising storage cannot become an address-bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingMemoryGraphBindError {
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
    InvalidPacketExtent,
}

/// Failed binding that returns the exact unchanged static allocation.
pub struct LegacyAdvertisingMemoryGraphBindFailure {
    storage: &'static mut LegacyAdvertisingMemoryGraphStorage,
    error: LegacyAdvertisingMemoryGraphBindError,
}

impl LegacyAdvertisingMemoryGraphBindFailure {
    fn new(
        storage: &'static mut LegacyAdvertisingMemoryGraphStorage,
        error: LegacyAdvertisingMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> LegacyAdvertisingMemoryGraphBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut LegacyAdvertisingMemoryGraphStorage,
        LegacyAdvertisingMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for LegacyAdvertisingMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyAdvertisingMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl LegacyAdvertisingMemoryGraphModelAddress {
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

/// Opaque identity of one exact pinned advertising graph allocation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LegacyAdvertisingMemoryGraphIdentity(usize);

impl LegacyAdvertisingMemoryGraphIdentity {
    fn for_storage(storage: &LegacyAdvertisingMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for LegacyAdvertisingMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyAdvertisingMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

/// Immutable geometry retained by every advertising graph typestate.
pub struct LegacyAdvertisingMemoryGraphBinding {
    #[cfg(not(target_arch = "riscv32"))]
    identity: LegacyAdvertisingMemoryGraphIdentity,
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    link_state: ControllerSramLinkAddress,
    scheduler_context: ControllerSramLinkAddress,
    scheduler_items:
        [ControllerSramLinkAddress; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
    tx_header: ControllerSramLinkAddress,
    tx_packet: AdvertisingTxPacketAddress,
}

impl LegacyAdvertisingMemoryGraphBinding {
    fn new(
        identity: LegacyAdvertisingMemoryGraphIdentity,
        base: u32,
    ) -> Result<Self, LegacyAdvertisingMemoryGraphBindError> {
        #[cfg(target_arch = "riscv32")]
        let _ = identity;
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(LegacyAdvertisingMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || GRAPH_BYTES > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(LegacyAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram);
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(LegacyAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let link = |offset: u32| {
            ControllerSramLinkAddress::new(address(offset)?)
                .map_err(|_| LegacyAdvertisingMemoryGraphBindError::ZeroCompressedLink)
        };

        let link_state = link(LINK_STATE_OFFSET)?;
        let scheduler_context = link(SCHEDULER_CONTEXT_OFFSET)?;
        let scheduler_item_offset = |index: usize| {
            let item_offset = u32::try_from(index)
                .ok()
                .and_then(|index| {
                    index.checked_mul(BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES as u32)
                })
                .and_then(|offset| SCHEDULER_ITEMS_OFFSET.checked_add(offset))
                .ok_or(LegacyAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram)?;
            link(item_offset)
        };
        let scheduler_items = [
            scheduler_item_offset(0)?,
            scheduler_item_offset(1)?,
            scheduler_item_offset(2)?,
        ];
        let tx_header = link(TX_HEADER_OFFSET)?;
        let tx_packet = AdvertisingTxPacketAddress::new(address(TX_PACKET_OFFSET)?)
            .map_err(|_| LegacyAdvertisingMemoryGraphBindError::InvalidPacketExtent)?;

        Ok(Self {
            #[cfg(not(target_arch = "riscv32"))]
            identity,
            base: base_address,
            end_exclusive: base + GRAPH_BYTES,
            link_state,
            scheduler_context,
            scheduler_items,
            tx_header,
            tx_packet,
        })
    }

    pub const fn identity(&self) -> LegacyAdvertisingMemoryGraphIdentity {
        #[cfg(target_arch = "riscv32")]
        {
            LegacyAdvertisingMemoryGraphIdentity(self.base.address() as usize)
        }
        #[cfg(not(target_arch = "riscv32"))]
        {
            self.identity
        }
    }

    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    pub const fn link_state_address(&self) -> ControllerSramLinkAddress {
        self.link_state
    }

    pub const fn scheduler_item_address(&self) -> ControllerSramLinkAddress {
        self.scheduler_items[0]
    }

    pub const fn scheduler_context_address(&self) -> BluetoothControllerSramAddress {
        self.scheduler_context.controller_address()
    }

    pub const fn tx_header_address(&self) -> ControllerSramLinkAddress {
        self.tx_header
    }
}

/// Unique CPU owner of one bound advertising graph before descriptor reset.
#[must_use = "the bound advertising graph must be retained"]
pub struct LegacyAdvertisingMemoryGraphCpuOwned {
    storage: Pin<&'static mut LegacyAdvertisingMemoryGraphStorage>,
    binding: LegacyAdvertisingMemoryGraphBinding,
}

impl LegacyAdvertisingMemoryGraphCpuOwned {
    pub const fn binding(&self) -> &LegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    #[cfg(test)]
    fn retains_reviewed_graph(&self) -> bool {
        let storage = self.storage.as_ref().get_ref();
        storage
            .link_state
            .retains_graph(self.binding.scheduler_items[0], self.binding.tx_header)
            && storage.scheduler_items.iter().all(|item| {
                item.retains_graph(self.binding.scheduler_context, self.binding.link_state)
            })
    }

    fn reinitialize_graph(&mut self) {
        let scheduler_item = self.binding.scheduler_items[0];
        let scheduler_context = self.binding.scheduler_context;
        let link_state = self.binding.link_state;
        let tx_header = self.binding.tx_header;
        let tx_packet = self.binding.tx_packet;
        let graph = self.storage.as_mut().project();
        graph.scheduler_context.clear();
        graph.link_state.initialize_graph(scheduler_item, tx_header);
        for item in graph.scheduler_items.iter() {
            item.initialize_graph(scheduler_context, link_state);
        }
        graph.tx_header.initialize_bound_tx(tx_packet);
        graph.tx_packet.clear();
    }

    /// Install one complete legacy advertising PDU without publishing the graph.
    pub fn prepare_packet(
        mut self,
        pdu: &[u8],
    ) -> Result<
        LegacyAdvertisingMemoryGraphPacketPrepared,
        LegacyAdvertisingMemoryGraphPacketPrepareFailure,
    > {
        let packet = self.storage.as_mut().project().tx_packet;
        let packet_length = match packet.prepare_encoded_pdu(pdu) {
            Ok(length) => length,
            Err(error) => {
                return Err(LegacyAdvertisingMemoryGraphPacketPrepareFailure {
                    owner: self,
                    error,
                });
            }
        };
        Ok(LegacyAdvertisingMemoryGraphPacketPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length,
        })
    }
}

/// Bound advertising graph carrying one complete CPU-owned TX packet.
#[must_use = "the prepared advertising graph must be retained or cancelled"]
pub struct LegacyAdvertisingMemoryGraphPacketPrepared {
    storage: Pin<&'static mut LegacyAdvertisingMemoryGraphStorage>,
    binding: LegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
}

impl LegacyAdvertisingMemoryGraphPacketPrepared {
    pub const fn binding(&self) -> &LegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    pub fn pdu(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .tx_packet
            .prepared_pdu(self.packet_length)
    }

    pub fn cancel(self) -> LegacyAdvertisingMemoryGraphCpuOwned {
        let mut owner = LegacyAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        owner
    }

    /// Apply the complete reviewed restricted advertising reset without
    /// creating scheduler timing or publication authority.
    pub fn reset_link_state(
        mut self,
        default_tx_power_dbm: i8,
    ) -> Result<
        LegacyAdvertisingMemoryGraphLinkStateReset,
        LegacyAdvertisingMemoryGraphLinkStateResetFailure,
    > {
        let own_address = match LegacyAdvertisingOwnAddress::from_pdu(self.pdu()) {
            Ok(own_address) => own_address,
            Err(error) => {
                return Err(LegacyAdvertisingMemoryGraphLinkStateResetFailure {
                    owner: self,
                    error,
                });
            }
        };
        self.storage
            .as_mut()
            .project()
            .link_state
            .reset_restricted_profile(self.binding.tx_header, own_address, default_tx_power_dbm);
        Ok(LegacyAdvertisingMemoryGraphLinkStateReset {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
        })
    }
}

/// Advertising graph after descriptor reset but before event-time scheduling.
#[must_use = "the reset graph must be advanced, cancelled, or retained"]
pub struct LegacyAdvertisingMemoryGraphLinkStateReset {
    storage: Pin<&'static mut LegacyAdvertisingMemoryGraphStorage>,
    binding: LegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
}

impl LegacyAdvertisingMemoryGraphLinkStateReset {
    pub const fn binding(&self) -> &LegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    pub fn pdu(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .tx_packet
            .prepared_pdu(self.packet_length)
    }

    /// Number of Link Layer payload bytes retained by this advertising event.
    pub const fn payload_length(&self) -> u8 {
        self.packet_length.payload_bytes()
    }

    /// Lower one accepted event into a linked primary-channel item chain.
    pub fn prepare_event(
        mut self,
        channels: LegacyAdvertisingPrimaryChannelPlan,
        raw_start: u32,
        raw_item_duration: u32,
    ) -> Result<
        LegacyAdvertisingMemoryGraphEventPrepared,
        LegacyAdvertisingMemoryGraphEventPrepareFailure,
    > {
        if self.storage.as_ref().get_ref().link_state.scheduler_head()
            != self.binding.scheduler_items[0]
                .controller_address()
                .address()
        {
            return Err(LegacyAdvertisingMemoryGraphEventPrepareFailure {
                owner: self,
                error: LegacyAdvertisingMemoryGraphEventPrepareError::SchedulerHeadMismatch,
            });
        }
        if !self
            .storage
            .as_ref()
            .get_ref()
            .scheduler_items
            .iter()
            .all(LegacyAdvertisingSchedulerItemStorage::is_terminal)
        {
            return Err(LegacyAdvertisingMemoryGraphEventPrepareFailure {
                owner: self,
                error: LegacyAdvertisingMemoryGraphEventPrepareError::NonTerminalSchedulerItem,
            });
        }

        let graph = self.storage.as_mut().project();
        let item_count = channels.channel_count();
        for index in 0..item_count {
            let item_start = raw_start.wrapping_add(raw_item_duration.wrapping_mul(index as u32));
            let item_end = item_start.wrapping_add(raw_item_duration);
            let successor = if index + 1 < item_count {
                Some(self.binding.scheduler_items[index + 1])
            } else {
                None
            };
            let words = graph.scheduler_items[index]
                .reviewed_words()
                .prepare_event_item(
                    graph.link_state.reviewed_words(),
                    channels
                        .channel(index)
                        .expect("a validated channel plan contains every active position"),
                    successor,
                    item_start,
                    item_end,
                );
            graph.scheduler_items[index].write_reviewed_words(words);
        }
        graph.link_state.detach_first_scheduler_item();

        Ok(LegacyAdvertisingMemoryGraphEventPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
            item_count: item_count as u8,
        })
    }

    /// Roll back every reset word and return an ordinary reusable graph.
    pub fn cancel(self) -> LegacyAdvertisingMemoryGraphCpuOwned {
        let mut owner = LegacyAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        owner
    }
}

/// Advertising graph carrying one complete first scheduler event image.
#[must_use = "the prepared event must enter admission, be cancelled, or retained"]
pub struct LegacyAdvertisingMemoryGraphEventPrepared {
    storage: Pin<&'static mut LegacyAdvertisingMemoryGraphStorage>,
    binding: LegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
    item_count: u8,
}

impl LegacyAdvertisingMemoryGraphEventPrepared {
    pub const fn binding(&self) -> &LegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    pub fn pdu(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .tx_packet
            .prepared_pdu(self.packet_length)
    }

    /// Exact CPU-owned scheduler-item identity selected by this graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_items[0].controller_address()
    }

    /// Number of hardware items linked into this event.
    pub const fn scheduler_item_count(&self) -> usize {
        self.item_count as usize
    }

    /// Install the common pre-publication scheduler bookkeeping.
    pub fn prepare_scheduler_bookkeeping(
        mut self,
    ) -> LegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
        let items = self.storage.as_mut().project().scheduler_items;
        let mut previous_control = [0; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY];
        let mut previous_status = [0; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY];
        let mut previous_completed_link = [0; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY];
        for (index, item) in items.iter().enumerate().take(self.item_count as usize) {
            previous_control[index] = item.words[SCHEDULER_ITEM_WORD_4C_OFFSET].get();
            previous_status[index] = item.words[SCHEDULER_ITEM_WORD_38_OFFSET].get();
            previous_completed_link[index] = item.words[SCHEDULER_ITEM_COMPLETED_LINK_OFFSET].get();
            item.words[SCHEDULER_ITEM_WORD_4C_OFFSET].set(previous_control[index] & !0xff);
            item.words[SCHEDULER_ITEM_WORD_38_OFFSET].set(u32::MAX);
            item.words[SCHEDULER_ITEM_COMPLETED_LINK_OFFSET].set(0);
        }

        LegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
            item_count: self.item_count,
            previous_control,
            previous_status,
            previous_completed_link,
        }
    }

    /// Roll back the private event image and recover a reusable graph.
    pub fn cancel(self) -> LegacyAdvertisingMemoryGraphCpuOwned {
        let mut owner = LegacyAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        owner
    }
}

/// Advertising graph with common scheduler bookkeeping but no list ownership.
#[must_use = "the scheduler-prepared graph must remain owned or be cancelled"]
pub struct LegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
    storage: Pin<&'static mut LegacyAdvertisingMemoryGraphStorage>,
    binding: LegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
    item_count: u8,
    previous_control: [u32; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
    previous_status: [u32; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
    previous_completed_link: [u32; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
}

impl LegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_items[0].controller_address()
    }

    /// Clear software-list links while retaining the private hardware chain.
    pub fn prepare_empty_list_link(mut self) -> LegacyAdvertisingMemoryGraphEmptyListLinkPrepared {
        let items = self.storage.as_mut().project().scheduler_items;
        let mut previous_software_next = [0; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY];
        for index in 0..self.item_count as usize {
            let item = &items[index];
            previous_software_next[index] = item.words[SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET].get();
            item.words[SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET].set(0);
        }

        LegacyAdvertisingMemoryGraphEmptyListLinkPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
            item_count: self.item_count,
            previous_control: self.previous_control,
            previous_status: self.previous_status,
            previous_completed_link: self.previous_completed_link,
            previous_software_next,
        }
    }

    pub fn cancel(mut self) -> LegacyAdvertisingMemoryGraphEventPrepared {
        let items = self.storage.as_mut().project().scheduler_items;
        for (index, item) in items.iter().enumerate().take(self.item_count as usize) {
            item.words[SCHEDULER_ITEM_WORD_4C_OFFSET].set(self.previous_control[index]);
            item.words[SCHEDULER_ITEM_WORD_38_OFFSET].set(self.previous_status[index]);
            item.words[SCHEDULER_ITEM_COMPLETED_LINK_OFFSET]
                .set(self.previous_completed_link[index]);
        }
        LegacyAdvertisingMemoryGraphEventPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
            item_count: self.item_count,
        }
    }
}

/// Advertising graph whose active scheduler items have null software links.
#[must_use = "the empty-list candidate must remain owned or be cancelled"]
pub struct LegacyAdvertisingMemoryGraphEmptyListLinkPrepared {
    storage: Pin<&'static mut LegacyAdvertisingMemoryGraphStorage>,
    binding: LegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
    item_count: u8,
    previous_control: [u32; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
    previous_status: [u32; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
    previous_completed_link: [u32; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
    previous_software_next: [u32; BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
}

impl LegacyAdvertisingMemoryGraphEmptyListLinkPrepared {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_items[0].controller_address()
    }

    /// Consume rollback authority after the exact list-zero head publication.
    pub fn into_head_published(
        self,
        publication: &BluetoothSchedulerHardwareListHeadPublished,
    ) -> LegacyAdvertisingMemoryGraphHeadPublished {
        assert_eq!(
            publication.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "legacy advertising uses the reviewed primary scheduler list"
        );
        assert_eq!(
            publication.head().address(),
            Some(self.scheduler_item_address()),
            "the published scheduler head must name the retained advertising graph"
        );
        LegacyAdvertisingMemoryGraphHeadPublished {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
            item_count: self.item_count,
        }
    }

    pub fn cancel(mut self) -> LegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
        let items = self.storage.as_mut().project().scheduler_items;
        for (index, item) in items.iter().enumerate().take(self.item_count as usize) {
            item.words[SCHEDULER_ITEM_SOFTWARE_NEXT_OFFSET].set(self.previous_software_next[index]);
        }
        LegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
            item_count: self.item_count,
            previous_control: self.previous_control,
            previous_status: self.previous_status,
            previous_completed_link: self.previous_completed_link,
        }
    }
}

/// Hardware-visible advertising graph after exact list-head publication.
#[must_use = "the head-published graph must reach RUN or remain fail-stop owned"]
pub struct LegacyAdvertisingMemoryGraphHeadPublished {
    storage: Pin<&'static mut LegacyAdvertisingMemoryGraphStorage>,
    binding: LegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
    item_count: u8,
}

impl LegacyAdvertisingMemoryGraphHeadPublished {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_items[0].controller_address()
    }

    pub fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> LegacyAdvertisingMemoryGraphRunning {
        assert_eq!(
            run.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "legacy advertising uses the reviewed primary scheduler list"
        );
        assert_eq!(
            run.head().address(),
            Some(self.scheduler_item_address()),
            "the RUN proof must retain the published advertising graph"
        );
        LegacyAdvertisingMemoryGraphRunning {
            storage: self.storage,
            binding: self.binding,
            _packet_length: self.packet_length,
            item_count: self.item_count,
        }
    }
}

/// Hardware-owned advertising graph admitted through the exact RUN suffix.
#[must_use = "the running graph must advance through fenced completion"]
pub struct LegacyAdvertisingMemoryGraphRunning {
    storage: Pin<&'static mut LegacyAdvertisingMemoryGraphStorage>,
    binding: LegacyAdvertisingMemoryGraphBinding,
    _packet_length: AdvertisingTxPacketLength,
    item_count: u8,
}

impl LegacyAdvertisingMemoryGraphRunning {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_items[0].controller_address()
    }

    pub fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> LegacyAdvertisingMemoryGraphCompletionObservation {
        if observed.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            return LegacyAdvertisingMemoryGraphCompletionObservation::ListMismatch {
                owner: self,
                observed,
            };
        }
        let mut statuses = [LegacyAdvertisingSchedulerItemCompletionStatus::Zero;
            BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY];
        for (index, status) in statuses
            .iter_mut()
            .enumerate()
            .take(self.item_count as usize)
        {
            let raw = self.storage.as_ref().get_ref().scheduler_items[index].words
                [SCHEDULER_ITEM_WORD_38_OFFSET]
                .get();
            if raw == u32::MAX {
                return LegacyAdvertisingMemoryGraphCompletionObservation::StillInFlight(self);
            }
            *status = match NonZeroU32::new(raw) {
                None => LegacyAdvertisingSchedulerItemCompletionStatus::Zero,
                Some(status) => LegacyAdvertisingSchedulerItemCompletionStatus::NonZero(status),
            };
        }
        let item_count = self.item_count;
        LegacyAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
            LegacyAdvertisingMemoryGraphCompletionObserved {
                owner: self,
                statuses: LegacyAdvertisingEventCompletionStatuses {
                    statuses,
                    len: item_count,
                },
            },
        )
    }
}

/// Semantic non-sentinel advertising scheduler status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingSchedulerItemCompletionStatus {
    Zero,
    NonZero(NonZeroU32),
}

/// Diagnostic completion values for every item in one hardware event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingEventCompletionStatuses {
    statuses: [LegacyAdvertisingSchedulerItemCompletionStatus;
        BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
    len: u8,
}

impl LegacyAdvertisingEventCompletionStatuses {
    pub const fn item_count(self) -> usize {
        self.len as usize
    }

    pub const fn status(
        self,
        position: usize,
    ) -> Option<LegacyAdvertisingSchedulerItemCompletionStatus> {
        if position < self.item_count() {
            Some(self.statuses[position])
        } else {
            None
        }
    }
}

/// One bounded completion observation of a running advertising graph.
#[must_use = "the observation and graph owners must remain paired"]
pub enum LegacyAdvertisingMemoryGraphCompletionObservation {
    ListMismatch {
        owner: LegacyAdvertisingMemoryGraphRunning,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(LegacyAdvertisingMemoryGraphRunning),
    CompletionObserved(LegacyAdvertisingMemoryGraphCompletionObserved),
}

/// Hardware-owned graph after a non-sentinel status observation.
#[must_use = "the completed graph must advance through unlink and recycle"]
pub struct LegacyAdvertisingMemoryGraphCompletionObserved {
    owner: LegacyAdvertisingMemoryGraphRunning,
    statuses: LegacyAdvertisingEventCompletionStatuses,
}

impl LegacyAdvertisingMemoryGraphCompletionObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.owner.scheduler_item_address()
    }

    pub const fn statuses(&self) -> LegacyAdvertisingEventCompletionStatuses {
        self.statuses
    }

    /// Bind the exact post-unlink removal proof before any SRAM cleanup.
    pub fn prepare_recycle_after_software_list_removal(
        self,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        LegacyAdvertisingMemoryGraphRecyclePrepared,
        LegacyAdvertisingMemoryGraphRecycleFailure,
    > {
        let error = if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(LegacyAdvertisingMemoryGraphRecycleError::HardwareListMismatch)
        } else if removal.completed_head().address() != Some(self.scheduler_item_address()) {
            Some(LegacyAdvertisingMemoryGraphRecycleError::SchedulerItemMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(LegacyAdvertisingMemoryGraphRecycleFailure {
                error,
                completed: self,
                removal,
            });
        }
        Ok(LegacyAdvertisingMemoryGraphRecyclePrepared {
            completed: self,
            removal,
        })
    }
}

/// Completed advertising graph validated for the CPU-owned recycle suffix.
#[must_use = "the recycle transaction must be committed or returned unchanged"]
pub struct LegacyAdvertisingMemoryGraphRecyclePrepared {
    completed: LegacyAdvertisingMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl LegacyAdvertisingMemoryGraphRecyclePrepared {
    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertisingMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }

    /// Return the released graph to its pristine CPU-owned allocation state.
    pub fn commit(self) -> LegacyAdvertisingMemoryGraphRecycled {
        let LegacyAdvertisingMemoryGraphCompletionObserved { owner, statuses } = self.completed;
        let LegacyAdvertisingMemoryGraphRunning {
            storage,
            binding,
            _packet_length: _,
            item_count: _,
        } = owner;
        let mut owner = LegacyAdvertisingMemoryGraphCpuOwned { storage, binding };
        owner.reinitialize_graph();
        LegacyAdvertisingMemoryGraphRecycled { owner, statuses }
    }
}

/// Why a completed advertising graph rejected recycle authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingMemoryGraphRecycleError {
    HardwareListMismatch,
    SchedulerItemMismatch,
}

/// Lossless lower recycle rejection retaining both affine owners.
#[must_use = "the completed graph and removal proof remain owned"]
pub struct LegacyAdvertisingMemoryGraphRecycleFailure {
    error: LegacyAdvertisingMemoryGraphRecycleError,
    completed: LegacyAdvertisingMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl LegacyAdvertisingMemoryGraphRecycleFailure {
    pub const fn error(&self) -> LegacyAdvertisingMemoryGraphRecycleError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertisingMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

/// CPU-owned advertising graph plus the unresolved hardware completion status.
#[must_use = "the memory owner and completion status must reach the role owner"]
pub struct LegacyAdvertisingMemoryGraphRecycled {
    owner: LegacyAdvertisingMemoryGraphCpuOwned,
    statuses: LegacyAdvertisingEventCompletionStatuses,
}

impl LegacyAdvertisingMemoryGraphRecycled {
    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertisingMemoryGraphCpuOwned,
        LegacyAdvertisingEventCompletionStatuses,
    ) {
        (self.owner, self.statuses)
    }
}

/// Why the reset graph could not become a first-event graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingMemoryGraphEventPrepareError {
    /// The link state did not retain the bound first scheduler item.
    SchedulerHeadMismatch,
    /// The bounded single-item allocation unexpectedly contained a successor.
    NonTerminalSchedulerItem,
}

/// Failed first-event preparation retaining the exact reset graph.
pub struct LegacyAdvertisingMemoryGraphEventPrepareFailure {
    owner: LegacyAdvertisingMemoryGraphLinkStateReset,
    error: LegacyAdvertisingMemoryGraphEventPrepareError,
}

impl LegacyAdvertisingMemoryGraphEventPrepareFailure {
    pub const fn error(&self) -> LegacyAdvertisingMemoryGraphEventPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertisingMemoryGraphLinkStateReset,
        LegacyAdvertisingMemoryGraphEventPrepareError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for LegacyAdvertisingMemoryGraphEventPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyAdvertisingMemoryGraphEventPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed reset retaining the exact packet-prepared graph.
pub struct LegacyAdvertisingMemoryGraphLinkStateResetFailure {
    owner: LegacyAdvertisingMemoryGraphPacketPrepared,
    error: LegacyAdvertisingPduError,
}

impl LegacyAdvertisingMemoryGraphLinkStateResetFailure {
    pub const fn error(&self) -> LegacyAdvertisingPduError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertisingMemoryGraphPacketPrepared,
        LegacyAdvertisingPduError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for LegacyAdvertisingMemoryGraphLinkStateResetFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyAdvertisingMemoryGraphLinkStateResetFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed packet preparation retaining the exact byte-unchanged graph owner.
pub struct LegacyAdvertisingMemoryGraphPacketPrepareFailure {
    owner: LegacyAdvertisingMemoryGraphCpuOwned,
    error: LeTxPacketPrepareError,
}

impl LegacyAdvertisingMemoryGraphPacketPrepareFailure {
    pub const fn error(&self) -> LeTxPacketPrepareError {
        self.error
    }

    pub fn into_parts(self) -> (LegacyAdvertisingMemoryGraphCpuOwned, LeTxPacketPrepareError) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for LegacyAdvertisingMemoryGraphPacketPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyAdvertisingMemoryGraphPacketPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl LegacyAdvertisingMemoryGraphStorage {
    pub const fn new() -> Self {
        Self {
            link_state: LegacyAdvertisingLinkStateStorage::new(),
            scheduler_context: SchedulerContextStorage::new(),
            scheduler_items: [const { LegacyAdvertisingSchedulerItemStorage::new() };
                BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY],
            tx_header: LeTxBufferHeaderStorage::new(),
            tx_packet: LeTxPacketStorage::new(),
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<LegacyAdvertisingMemoryGraphCpuOwned, LegacyAdvertisingMemoryGraphBindFailure> {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(LegacyAdvertisingMemoryGraphBindFailure::new(
                    storage,
                    LegacyAdvertisingMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: LegacyAdvertisingMemoryGraphModelAddress,
    ) -> Result<LegacyAdvertisingMemoryGraphCpuOwned, LegacyAdvertisingMemoryGraphBindFailure> {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<LegacyAdvertisingMemoryGraphCpuOwned, LegacyAdvertisingMemoryGraphBindFailure> {
        let identity = LegacyAdvertisingMemoryGraphIdentity::for_storage(storage);
        let binding = match LegacyAdvertisingMemoryGraphBinding::new(identity, base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(LegacyAdvertisingMemoryGraphBindFailure::new(storage, error));
            }
        };
        let mut owner = LegacyAdvertisingMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize_graph();
        Ok(owner)
    }
}

impl Default for LegacyAdvertisingMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
