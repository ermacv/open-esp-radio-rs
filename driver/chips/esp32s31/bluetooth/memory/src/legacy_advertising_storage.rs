//! Pinned CPU-owned storage for the first ESP32-S31 legacy advertising role.
//!
//! This module closes allocation and the stable links between the common TX
//! packet/header, advertising link state, scheduler context and first
//! scheduler item. A later CPU-owned transition can synthesize the first event
//! image, but no hardware list can be published from these states.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use pin_project::pin_project;
use vcell::VolatileCell;

use crate::{
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, BluetoothLeTxBufferHeaderStorage,
        BluetoothLeTxPacketAddress, BluetoothLeTxPacketPrepareError,
        BluetoothLeTxPacketPreparedLength, BluetoothLeTxPacketStorage,
    },
    legacy_advertising_event_image::{
        BluetoothLegacyAdvertisingLinkStateWords, BluetoothLegacyAdvertisingOwnAddress,
        BluetoothLegacyAdvertisingPduError, BluetoothLegacyAdvertisingPrimaryChannel,
        BluetoothLegacyAdvertisingSchedulerItemWords,
    },
    scheduler_context::BluetoothSchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

/// Bytes reserved for the advertising link-state object.
pub const BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES: usize = 0x84;
/// Bytes reserved for one advertising scheduler item.
pub const BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES: usize = 0x60;
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

type AdvertisingTxPacketAddress =
    BluetoothLeTxPacketAddress<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketLength =
    BluetoothLeTxPacketPreparedLength<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;

/// Opaque advertising link-state allocation before event-time preparation.
#[repr(C, align(4))]
struct BluetoothLegacyAdvertisingLinkStateStorage {
    words: [VolatileCell<u32>; LINK_STATE_WORDS],
}

impl BluetoothLegacyAdvertisingLinkStateStorage {
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
        scheduler_item: BluetoothControllerSramLinkAddress,
        tx_header: BluetoothControllerSramLinkAddress,
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

    fn reviewed_words(&self) -> BluetoothLegacyAdvertisingLinkStateWords {
        BluetoothLegacyAdvertisingLinkStateWords {
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

    fn write_reviewed_words(&self, words: BluetoothLegacyAdvertisingLinkStateWords) {
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
        tx_header: BluetoothControllerSramLinkAddress,
        own_address: BluetoothLegacyAdvertisingOwnAddress,
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
        scheduler_item: BluetoothControllerSramLinkAddress,
        tx_header: BluetoothControllerSramLinkAddress,
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
struct BluetoothLegacyAdvertisingSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl BluetoothLegacyAdvertisingSchedulerItemStorage {
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
        scheduler_context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
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

    fn reviewed_words(&self) -> BluetoothLegacyAdvertisingSchedulerItemWords {
        BluetoothLegacyAdvertisingSchedulerItemWords {
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

    fn write_reviewed_words(&self, words: BluetoothLegacyAdvertisingSchedulerItemWords) {
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
        scheduler_context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
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
pub struct BluetoothLegacyAdvertisingMemoryGraphStorage {
    link_state: BluetoothLegacyAdvertisingLinkStateStorage,
    scheduler_context: BluetoothSchedulerContextStorage,
    scheduler_item: BluetoothLegacyAdvertisingSchedulerItemStorage,
    tx_header: BluetoothLeTxBufferHeaderStorage,
    tx_packet: BluetoothLeTxPacketStorage<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>,
    #[pin]
    _pin: PhantomPinned,
}

const GRAPH_BYTES: u32 =
    core::mem::size_of::<BluetoothLegacyAdvertisingMemoryGraphStorage>() as u32;
const LINK_STATE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothLegacyAdvertisingMemoryGraphStorage, link_state) as u32;
const SCHEDULER_ITEM_OFFSET: u32 =
    core::mem::offset_of!(BluetoothLegacyAdvertisingMemoryGraphStorage, scheduler_item) as u32;
const SCHEDULER_CONTEXT_OFFSET: u32 = core::mem::offset_of!(
    BluetoothLegacyAdvertisingMemoryGraphStorage,
    scheduler_context
) as u32;
const TX_HEADER_OFFSET: u32 =
    core::mem::offset_of!(BluetoothLegacyAdvertisingMemoryGraphStorage, tx_header) as u32;
const TX_PACKET_OFFSET: u32 =
    core::mem::offset_of!(BluetoothLegacyAdvertisingMemoryGraphStorage, tx_packet) as u32;

/// Why static advertising storage cannot become an address-bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingMemoryGraphBindError {
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
    InvalidPacketExtent,
}

/// Failed binding that returns the exact unchanged static allocation.
pub struct BluetoothLegacyAdvertisingMemoryGraphBindFailure {
    storage: &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
    error: BluetoothLegacyAdvertisingMemoryGraphBindError,
}

impl BluetoothLegacyAdvertisingMemoryGraphBindFailure {
    fn new(
        storage: &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
        error: BluetoothLegacyAdvertisingMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> BluetoothLegacyAdvertisingMemoryGraphBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
        BluetoothLegacyAdvertisingMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyAdvertisingMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothLegacyAdvertisingMemoryGraphModelAddress {
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
pub struct BluetoothLegacyAdvertisingMemoryGraphIdentity(usize);

impl BluetoothLegacyAdvertisingMemoryGraphIdentity {
    fn for_storage(storage: &BluetoothLegacyAdvertisingMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

/// Immutable geometry retained by every advertising graph typestate.
pub struct BluetoothLegacyAdvertisingMemoryGraphBinding {
    #[cfg(not(target_arch = "riscv32"))]
    identity: BluetoothLegacyAdvertisingMemoryGraphIdentity,
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    link_state: BluetoothControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramLinkAddress,
    scheduler_item: BluetoothControllerSramLinkAddress,
    tx_header: BluetoothControllerSramLinkAddress,
    tx_packet: AdvertisingTxPacketAddress,
}

impl BluetoothLegacyAdvertisingMemoryGraphBinding {
    fn new(
        identity: BluetoothLegacyAdvertisingMemoryGraphIdentity,
        base: u32,
    ) -> Result<Self, BluetoothLegacyAdvertisingMemoryGraphBindError> {
        #[cfg(target_arch = "riscv32")]
        let _ = identity;
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothLegacyAdvertisingMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || GRAPH_BYTES > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(BluetoothLegacyAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram);
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(BluetoothLegacyAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let link = |offset: u32| {
            BluetoothControllerSramLinkAddress::new(address(offset)?)
                .map_err(|_| BluetoothLegacyAdvertisingMemoryGraphBindError::ZeroCompressedLink)
        };

        let link_state = link(LINK_STATE_OFFSET)?;
        let scheduler_context = link(SCHEDULER_CONTEXT_OFFSET)?;
        let scheduler_item = link(SCHEDULER_ITEM_OFFSET)?;
        let tx_header = link(TX_HEADER_OFFSET)?;
        let tx_packet = AdvertisingTxPacketAddress::new(address(TX_PACKET_OFFSET)?)
            .map_err(|_| BluetoothLegacyAdvertisingMemoryGraphBindError::InvalidPacketExtent)?;

        Ok(Self {
            #[cfg(not(target_arch = "riscv32"))]
            identity,
            base: base_address,
            end_exclusive: base + GRAPH_BYTES,
            link_state,
            scheduler_context,
            scheduler_item,
            tx_header,
            tx_packet,
        })
    }

    pub const fn identity(&self) -> BluetoothLegacyAdvertisingMemoryGraphIdentity {
        #[cfg(target_arch = "riscv32")]
        {
            BluetoothLegacyAdvertisingMemoryGraphIdentity(self.base.address() as usize)
        }
        #[cfg(not(target_arch = "riscv32"))]
        {
            self.identity
        }
    }

    pub const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    pub const fn link_state_address(&self) -> BluetoothControllerSramLinkAddress {
        self.link_state
    }

    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramLinkAddress {
        self.scheduler_item
    }

    pub const fn scheduler_context_address(&self) -> BluetoothControllerSramAddress {
        self.scheduler_context.controller_address()
    }

    pub const fn tx_header_address(&self) -> BluetoothControllerSramLinkAddress {
        self.tx_header
    }
}

/// Unique CPU owner of one bound advertising graph before descriptor reset.
#[must_use = "the bound advertising graph must be retained"]
pub struct BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothLegacyAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyAdvertisingMemoryGraphBinding,
}

impl BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
    pub const fn binding(&self) -> &BluetoothLegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    #[cfg(test)]
    fn retains_reviewed_graph(&self) -> bool {
        let storage = self.storage.as_ref().get_ref();
        storage
            .link_state
            .retains_graph(self.binding.scheduler_item, self.binding.tx_header)
            && storage
                .scheduler_item
                .retains_graph(self.binding.scheduler_context, self.binding.link_state)
    }

    fn reinitialize_graph(&mut self) {
        let scheduler_item = self.binding.scheduler_item;
        let scheduler_context = self.binding.scheduler_context;
        let link_state = self.binding.link_state;
        let tx_header = self.binding.tx_header;
        let tx_packet = self.binding.tx_packet;
        let graph = self.storage.as_mut().project();
        graph.scheduler_context.clear();
        graph.link_state.initialize_graph(scheduler_item, tx_header);
        graph
            .scheduler_item
            .initialize_graph(scheduler_context, link_state);
        graph.tx_header.initialize_bound_tx(tx_packet);
        graph.tx_packet.clear();
    }

    /// Install one complete legacy advertising PDU without publishing the graph.
    pub fn prepare_packet(
        mut self,
        pdu: &[u8],
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
        BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure,
    > {
        let packet = self.storage.as_mut().project().tx_packet;
        let packet_length = match packet.prepare_encoded_pdu(pdu) {
            Ok(length) => length,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure {
                    owner: self,
                    error,
                });
            }
        };
        Ok(BluetoothLegacyAdvertisingMemoryGraphPacketPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length,
        })
    }
}

/// Bound advertising graph carrying one complete CPU-owned TX packet.
#[must_use = "the prepared advertising graph must be retained or cancelled"]
pub struct BluetoothLegacyAdvertisingMemoryGraphPacketPrepared {
    storage: Pin<&'static mut BluetoothLegacyAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
}

impl BluetoothLegacyAdvertisingMemoryGraphPacketPrepared {
    pub const fn binding(&self) -> &BluetoothLegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    pub fn pdu(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .tx_packet
            .prepared_pdu(self.packet_length)
    }

    pub fn cancel(self) -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        let mut owner = BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
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
        BluetoothLegacyAdvertisingMemoryGraphLinkStateReset,
        BluetoothLegacyAdvertisingMemoryGraphLinkStateResetFailure,
    > {
        let own_address = match BluetoothLegacyAdvertisingOwnAddress::from_pdu(self.pdu()) {
            Ok(own_address) => own_address,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingMemoryGraphLinkStateResetFailure {
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
        Ok(BluetoothLegacyAdvertisingMemoryGraphLinkStateReset {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
        })
    }
}

/// Advertising graph after descriptor reset but before event-time scheduling.
#[must_use = "the reset graph must be advanced, cancelled, or retained"]
pub struct BluetoothLegacyAdvertisingMemoryGraphLinkStateReset {
    storage: Pin<&'static mut BluetoothLegacyAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
}

impl BluetoothLegacyAdvertisingMemoryGraphLinkStateReset {
    pub const fn binding(&self) -> &BluetoothLegacyAdvertisingMemoryGraphBinding {
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

    /// Lower one accepted raw-time window into the private first-event image.
    pub fn prepare_first_event(
        mut self,
        channel: BluetoothLegacyAdvertisingPrimaryChannel,
        raw_start: u32,
        raw_end: u32,
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepared,
        BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareFailure,
    > {
        if self.storage.as_ref().get_ref().link_state.scheduler_head()
            != self.binding.scheduler_item.controller_address().address()
        {
            return Err(
                BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareFailure {
                    owner: self,
                    error: BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError::
                        SchedulerHeadMismatch,
                },
            );
        }
        if !self.storage.as_ref().get_ref().scheduler_item.is_terminal() {
            return Err(
                BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareFailure {
                    owner: self,
                    error: BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError::
                        NonTerminalSchedulerItem,
                },
            );
        }

        let graph = self.storage.as_mut().project();
        let words = graph.scheduler_item.reviewed_words().prepare_first_event(
            graph.link_state.reviewed_words(),
            channel,
            raw_start,
            raw_end,
        );
        graph.link_state.detach_first_scheduler_item();
        graph.scheduler_item.write_reviewed_words(words);

        Ok(BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepared {
            storage: self.storage,
            binding: self.binding,
            packet_length: self.packet_length,
        })
    }

    /// Roll back every reset word and return an ordinary reusable graph.
    pub fn cancel(self) -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        let mut owner = BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        owner
    }
}

/// Advertising graph carrying one complete first scheduler event image.
#[must_use = "the prepared event must enter admission, be cancelled, or retained"]
pub struct BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepared {
    storage: Pin<&'static mut BluetoothLegacyAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyAdvertisingMemoryGraphBinding,
    packet_length: AdvertisingTxPacketLength,
}

impl BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepared {
    pub const fn binding(&self) -> &BluetoothLegacyAdvertisingMemoryGraphBinding {
        &self.binding
    }

    pub fn pdu(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .tx_packet
            .prepared_pdu(self.packet_length)
    }

    /// Roll back the private event image and recover a reusable graph.
    pub fn cancel(self) -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        let mut owner = BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        owner
    }
}

/// Why the reset graph could not become a first-event graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError {
    /// The link state did not retain the bound first scheduler item.
    SchedulerHeadMismatch,
    /// The bounded single-item allocation unexpectedly contained a successor.
    NonTerminalSchedulerItem,
}

/// Failed first-event preparation retaining the exact reset graph.
pub struct BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareFailure {
    owner: BluetoothLegacyAdvertisingMemoryGraphLinkStateReset,
    error: BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError,
}

impl BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareFailure {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingMemoryGraphLinkStateReset,
        BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed reset retaining the exact packet-prepared graph.
pub struct BluetoothLegacyAdvertisingMemoryGraphLinkStateResetFailure {
    owner: BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
    error: BluetoothLegacyAdvertisingPduError,
}

impl BluetoothLegacyAdvertisingMemoryGraphLinkStateResetFailure {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingPduError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
        BluetoothLegacyAdvertisingPduError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingMemoryGraphLinkStateResetFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingMemoryGraphLinkStateResetFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed packet preparation retaining the exact byte-unchanged graph owner.
pub struct BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure {
    owner: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    error: BluetoothLeTxPacketPrepareError,
}

impl BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure {
    pub const fn error(&self) -> BluetoothLeTxPacketPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLeTxPacketPrepareError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingMemoryGraphPacketPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BluetoothLegacyAdvertisingMemoryGraphStorage {
    pub const fn new() -> Self {
        Self {
            link_state: BluetoothLegacyAdvertisingLinkStateStorage::new(),
            scheduler_context: BluetoothSchedulerContextStorage::new(),
            scheduler_item: BluetoothLegacyAdvertisingSchedulerItemStorage::new(),
            tx_header: BluetoothLeTxBufferHeaderStorage::new(),
            tx_packet: BluetoothLeTxPacketStorage::new(),
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphBindFailure,
    > {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothLegacyAdvertisingMemoryGraphBindFailure::new(
                    storage,
                    BluetoothLegacyAdvertisingMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothLegacyAdvertisingMemoryGraphModelAddress,
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphBindFailure,
    > {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphBindFailure,
    > {
        let identity = BluetoothLegacyAdvertisingMemoryGraphIdentity::for_storage(storage);
        let binding = match BluetoothLegacyAdvertisingMemoryGraphBinding::new(identity, base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingMemoryGraphBindFailure::new(
                    storage, error,
                ));
            }
        };
        let mut owner = BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize_graph();
        Ok(owner)
    }
}

impl Default for BluetoothLegacyAdvertisingMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothLegacyAdvertisingMemoryGraphModelAddress,
        BluetoothLegacyAdvertisingMemoryGraphStorage,
    };

    fn owner() -> super::BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
        ));
        let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("the model base uses controller SRAM syntax");
        BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
            .expect("the complete graph fits physical controller SRAM")
    }

    #[test]
    fn bound_graph_prepares_and_cancels_one_complete_advertising_pdu() {
        let owner = owner();
        assert!(owner.retains_reviewed_graph());
        let identity = owner.binding().identity();
        let range = owner.binding().range();
        let prepared = owner
            .prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6])
            .expect("the complete legacy advertising PDU fits");

        assert_eq!(prepared.pdu(), &[0x02, 6, 1, 2, 3, 4, 5, 6]);
        assert_eq!(prepared.binding().identity(), identity);

        let owner = prepared.cancel();
        assert!(owner.retains_reviewed_graph());
        assert_eq!(owner.binding().identity(), identity);
        assert_eq!(owner.binding().range(), range);
    }

    #[test]
    fn malformed_packet_returns_the_same_graph_for_retry() {
        let owner = owner();
        let identity = owner.binding().identity();
        let failure = match owner.prepare_packet(&[0x02, 7, 1, 2, 3]) {
            Ok(_) => panic!("a mismatched encoded length must fail closed"),
            Err(failure) => failure,
        };
        let (owner, _) = failure.into_parts();
        assert_eq!(owner.binding().identity(), identity);
        assert!(owner.prepare_packet(&[0x02, 3, 1, 2, 3]).is_ok());
    }

    #[test]
    fn reset_rejects_a_non_advertising_packet_without_losing_the_prepared_graph() {
        let prepared = owner()
            .prepare_packet(&[0x00, 6, 1, 2, 3, 4, 5, 6])
            .expect("the common packet allocation validates only the LE length");
        let failure = match prepared.reset_link_state(0) {
            Ok(_) => panic!("a non-advertising PDU must not select the advertising reset"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            super::BluetoothLegacyAdvertisingPduError::UnsupportedPduType
        );
        let (prepared, _) = failure.into_parts();
        assert_eq!(prepared.pdu(), &[0x00, 6, 1, 2, 3, 4, 5, 6]);
        assert!(prepared.cancel().retains_reviewed_graph());
    }

    #[test]
    fn random_address_packet_reaches_the_same_cancellable_reset_state() {
        let reset = owner()
            .prepare_packet(&[0x42, 6, 1, 2, 3, 4, 5, 0xc6])
            .expect("the encoded random-address PDU fits")
            .reset_link_state(7)
            .expect("TxAdd selects the reviewed static-random reset branch");
        assert_eq!(reset.pdu(), &[0x42, 6, 1, 2, 3, 4, 5, 0xc6]);
        assert!(reset.cancel().retains_reviewed_graph());
    }

    #[test]
    fn first_event_preparation_is_affine_and_cancellation_restores_the_graph() {
        let prepared = owner()
            .prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6])
            .expect("the encoded advertising PDU fits")
            .reset_link_state(0)
            .expect("the PDU selects the restricted reset")
            .prepare_first_event(
                crate::BluetoothLegacyAdvertisingPrimaryChannel::Channel37,
                1_000,
                1_128,
            )
            .expect("the bound single-item graph is intact");

        assert_eq!(prepared.pdu(), &[0x02, 6, 1, 2, 3, 4, 5, 6]);
        assert!(prepared.cancel().retains_reviewed_graph());
    }
}
