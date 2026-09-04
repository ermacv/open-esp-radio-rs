//! Private descriptor codec for response-capable legacy advertising.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress;
use vcell::VolatileCell;

use crate::{
    BluetoothNonScanningRxMemoryCpuOwned,
    le_tx_packet::{
        BluetoothLeTxBufferHeaderStorage, BluetoothLeTxPacketAddress,
        BluetoothLeTxPacketPreparedInput, BluetoothLeTxPacketStorage,
    },
    legacy_advertising_event_image::{
        BluetoothLegacyAdvertisingLinkStateWords, BluetoothLegacyAdvertisingOwnAddress,
        BluetoothLegacyAdvertisingPrimaryChannel, BluetoothLegacyAdvertisingSchedulerItemWords,
    },
    scheduler_context::BluetoothSchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

use super::{
    AdvertisingTxPacketLength, BluetoothLegacyConnectableAdvertisingMemoryGraphBindError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
    BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
    BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    BluetoothLegacyConnectableAdvertisingSchedulerSpan, LEGACY_ADVERTISING_TX_PACKET_BYTES,
};

const LINK_STATE_BYTES: usize = 0x84;
const SCHEDULER_ITEM_BYTES: usize = 0x60;
const LINK_STATE_WORDS: usize = LINK_STATE_BYTES / 4;
const SCHEDULER_ITEM_WORDS: usize = SCHEDULER_ITEM_BYTES / 4;

const LINK_STATE_SCHEDULER_HEAD: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL: usize = 0x74 / 4;
const LINK_STATE_RX_SWAP_RESERVE: usize = 0x78 / 4;
const LINK_STATE_ALLOCATION_CONFIG: usize = 0x30 / 4;
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: u32 = 0x0000_1e00;
const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;

const LINK_STATE_WORD_00: usize = 0;
const LINK_STATE_WORD_04: usize = 1;
const LINK_STATE_WORD_08: usize = 2;
const LINK_STATE_WORD_0C: usize = 3;
const LINK_STATE_WORD_14: usize = 0x14 / 4;
const LINK_STATE_WORD_18: usize = 0x18 / 4;
const LINK_STATE_WORD_24: usize = 0x24 / 4;
const LINK_STATE_WORD_2C: usize = 0x2c / 4;
const LINK_STATE_WORD_30: usize = 0x30 / 4;
const LINK_STATE_WORD_34: usize = 0x34 / 4;
const LINK_STATE_WORD_38: usize = 0x38 / 4;
const LINK_STATE_WORD_3C: usize = 0x3c / 4;
const LINK_STATE_WORD_40: usize = 0x40 / 4;
const LINK_STATE_WORD_50: usize = 0x50 / 4;
const LINK_STATE_WORD_60: usize = 0x60 / 4;

const SCHEDULER_ITEM_HARDWARE_NEXT: usize = 0;
const SCHEDULER_ITEM_CONTEXT: usize = 1;
const SCHEDULER_ITEM_LINK_STATE: usize = 0x08 / 4;
const SCHEDULER_ITEM_WORD_14: usize = 0x14 / 4;
const SCHEDULER_ITEM_WORD_18: usize = 0x18 / 4;
const SCHEDULER_ITEM_WORD_38: usize = 0x38 / 4;
const SCHEDULER_ITEM_RAW_START: usize = 0x44 / 4;
const SCHEDULER_ITEM_RAW_END: usize = 0x48 / 4;
const SCHEDULER_ITEM_CONTROL: usize = 0x4c / 4;
const SCHEDULER_ITEM_SOFTWARE_NEXT: usize = 0x50 / 4;
const SCHEDULER_ITEM_COMPLETED_LINK: usize = 0x54 / 4;
const SCHEDULER_ITEM_HARDWARE_NEXT_MASK: u32 = 0x000f_ffff;
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0010_0000;
const SCHEDULER_ITEM_LINK_STATE_PREFIX: u32 = 0x0060_0000;

const LE_1M_FIXED_PACKET_MICROS: u32 = 80;
const VENDOR_RESPONSE_CAPABLE_ITEM_TAIL_MICROS: u32 = 4;

type AdvertisingTxPacketAddress = BluetoothLeTxPacketAddress<LEGACY_ADVERTISING_TX_PACKET_BYTES>;

#[repr(C, align(4))]
struct LinkStateStorage {
    words: [VolatileCell<u32>; LINK_STATE_WORDS],
}

impl LinkStateStorage {
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
        adv_ind_header: BluetoothControllerSramLinkAddress,
        scan_response_header: BluetoothControllerSramLinkAddress,
    ) {
        self.clear();
        self.words[LINK_STATE_SCHEDULER_HEAD].set(scheduler_item.controller_address().address());
        self.words[LINK_STATE_TX_HEAD].set(adv_ind_header.controller_address().address());
        self.words[LINK_STATE_TX_TAIL].set(scan_response_header.controller_address().address());
        self.words[LINK_STATE_ALLOCATION_CONFIG].set(LINK_STATE_ALLOCATION_CONFIG_IMAGE);
    }

    fn reviewed_words(&self) -> BluetoothLegacyAdvertisingLinkStateWords {
        BluetoothLegacyAdvertisingLinkStateWords {
            word_00: self.words[LINK_STATE_WORD_00].get(),
            word_04: self.words[LINK_STATE_WORD_04].get(),
            word_08: self.words[LINK_STATE_WORD_08].get(),
            word_0c: self.words[LINK_STATE_WORD_0C].get(),
            word_14: self.words[LINK_STATE_WORD_14].get(),
            word_18: self.words[LINK_STATE_WORD_18].get(),
            word_24: self.words[LINK_STATE_WORD_24].get(),
            crc_init_word_2c: self.words[LINK_STATE_WORD_2C].get(),
            word_30: self.words[LINK_STATE_WORD_30].get(),
            word_34: self.words[LINK_STATE_WORD_34].get(),
            access_address_word_38: self.words[LINK_STATE_WORD_38].get(),
            word_3c: self.words[LINK_STATE_WORD_3C].get(),
            word_40: self.words[LINK_STATE_WORD_40].get(),
            word_50: self.words[LINK_STATE_WORD_50].get(),
            word_60: self.words[LINK_STATE_WORD_60].get(),
        }
    }

    fn write_reviewed_words(&self, words: BluetoothLegacyAdvertisingLinkStateWords) {
        self.words[LINK_STATE_WORD_00].set(words.word_00);
        self.words[LINK_STATE_WORD_04].set(words.word_04);
        self.words[LINK_STATE_WORD_08].set(words.word_08);
        self.words[LINK_STATE_WORD_0C].set(words.word_0c);
        self.words[LINK_STATE_WORD_14].set(words.word_14);
        self.words[LINK_STATE_WORD_18].set(words.word_18);
        self.words[LINK_STATE_WORD_24].set(words.word_24);
        self.words[LINK_STATE_WORD_2C].set(words.crc_init_word_2c);
        self.words[LINK_STATE_WORD_30].set(words.word_30);
        self.words[LINK_STATE_WORD_34].set(words.word_34);
        self.words[LINK_STATE_WORD_38].set(words.access_address_word_38);
        self.words[LINK_STATE_WORD_3C].set(words.word_3c);
        self.words[LINK_STATE_WORD_40].set(words.word_40);
        self.words[LINK_STATE_WORD_50].set(words.word_50);
        self.words[LINK_STATE_WORD_60].set(words.word_60);
    }

    fn scheduler_head(&self) -> u32 {
        self.words[LINK_STATE_SCHEDULER_HEAD].get()
    }

    fn detach_scheduler_item(&self) {
        self.words[LINK_STATE_SCHEDULER_HEAD].set(0);
    }

    fn restore_scheduler_item(&self, scheduler_item: BluetoothControllerSramAddress) {
        self.words[LINK_STATE_SCHEDULER_HEAD].set(scheduler_item.address());
    }

    fn prepare_profile(
        &self,
        binding: &BluetoothLegacyConnectableAdvertisingGraphBinding,
        rx_head: BluetoothControllerSramAddress,
        rx_tail: BluetoothControllerSramAddress,
        own_address: BluetoothLegacyAdvertisingOwnAddress,
        default_tx_power_dbm: i8,
    ) {
        let mut words =
            self.reviewed_words()
                .reset(binding.adv_ind_header, own_address, default_tx_power_dbm);
        words.word_08 = (words.word_08 & !COMPRESSED_LINK_MASK) | rx_head.compressed_image();
        self.write_reviewed_words(words);
        self.words[LINK_STATE_RX_HEAD].set(rx_head.address());
        self.words[LINK_STATE_RX_TAIL].set(rx_tail.address());
        self.words[LINK_STATE_RX_SWAP_RESERVE].set(0);
    }

    fn retains_prepared_graph(
        &self,
        binding: &BluetoothLegacyConnectableAdvertisingGraphBinding,
        rx_head: BluetoothControllerSramAddress,
        rx_tail: BluetoothControllerSramAddress,
    ) -> bool {
        self.words[LINK_STATE_SCHEDULER_HEAD].get()
            == binding.scheduler_item.controller_address().address()
            && self.words[LINK_STATE_WORD_08].get() & COMPRESSED_LINK_MASK
                == rx_head.compressed_image()
            && self.words[LINK_STATE_RX_HEAD].get() == rx_head.address()
            && self.words[LINK_STATE_RX_TAIL].get() == rx_tail.address()
            && self.words[LINK_STATE_TX_HEAD].get()
                == binding.adv_ind_header.controller_address().address()
            && self.words[LINK_STATE_TX_TAIL].get()
                == binding.scan_response_header.controller_address().address()
    }

    #[cfg(test)]
    fn emulate_missing_rx_consumer_link(&self) {
        self.words[LINK_STATE_WORD_08]
            .set(self.words[LINK_STATE_WORD_08].get() & !COMPRESSED_LINK_MASK);
    }
}

#[repr(C, align(4))]
struct SchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl SchedulerItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    fn initialize_graph(
        &self,
        context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT].set(SCHEDULER_ITEM_ALLOCATION_PREFIX);
        self.words[SCHEDULER_ITEM_CONTEXT].set(context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE]
            .set(SCHEDULER_ITEM_LINK_STATE_PREFIX | link_state.compressed_image());
    }

    fn is_terminal(&self) -> bool {
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT].get() & SCHEDULER_ITEM_HARDWARE_NEXT_MASK == 0
    }

    fn reviewed_words(&self) -> BluetoothLegacyAdvertisingSchedulerItemWords {
        BluetoothLegacyAdvertisingSchedulerItemWords {
            word_00: self.words[SCHEDULER_ITEM_HARDWARE_NEXT].get(),
            word_04: self.words[SCHEDULER_ITEM_CONTEXT].get(),
            word_14: self.words[SCHEDULER_ITEM_WORD_14].get(),
            word_18: self.words[SCHEDULER_ITEM_WORD_18].get(),
            word_38: self.words[SCHEDULER_ITEM_WORD_38].get(),
            raw_start_word_44: self.words[SCHEDULER_ITEM_RAW_START].get(),
            raw_end_word_48: self.words[SCHEDULER_ITEM_RAW_END].get(),
            word_4c: self.words[SCHEDULER_ITEM_CONTROL].get(),
        }
    }

    fn write_reviewed_words(&self, words: BluetoothLegacyAdvertisingSchedulerItemWords) {
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT].set(words.word_00);
        self.words[SCHEDULER_ITEM_CONTEXT].set(words.word_04);
        self.words[SCHEDULER_ITEM_WORD_14].set(words.word_14);
        self.words[SCHEDULER_ITEM_WORD_18].set(words.word_18);
        self.words[SCHEDULER_ITEM_WORD_38].set(words.word_38);
        self.words[SCHEDULER_ITEM_RAW_START].set(words.raw_start_word_44);
        self.words[SCHEDULER_ITEM_RAW_END].set(words.raw_end_word_48);
        self.words[SCHEDULER_ITEM_CONTROL].set(words.word_4c);
    }

    fn completion_status(
        &self,
    ) -> Option<BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus> {
        match self.words[SCHEDULER_ITEM_WORD_38].get() {
            u32::MAX => None,
            0 => Some(BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero),
            _ => Some(BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero),
        }
    }

    #[cfg(test)]
    fn model_controller_completion(
        &self,
        status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    ) {
        self.words[SCHEDULER_ITEM_WORD_38].set(match status {
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero => 0,
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero => 1,
        });
    }

    fn retains_graph(
        &self,
        context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
    ) -> bool {
        self.words[SCHEDULER_ITEM_HARDWARE_NEXT].get() == SCHEDULER_ITEM_ALLOCATION_PREFIX
            && self.words[SCHEDULER_ITEM_CONTEXT].get() == context.compressed_image()
            && self.words[SCHEDULER_ITEM_LINK_STATE].get()
                == SCHEDULER_ITEM_LINK_STATE_PREFIX | link_state.compressed_image()
    }
}

#[derive(Clone, Copy)]
pub(super) struct BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingSnapshot {
    control: u32,
    status: u32,
    completed_link: u32,
}

#[derive(Clone, Copy)]
pub(super) struct BluetoothLegacyConnectableAdvertisingSoftwareLinkSnapshot(u32);

#[repr(C)]
pub(super) struct BluetoothLegacyConnectableAdvertisingGraphStorage {
    link_state: LinkStateStorage,
    scheduler_context: BluetoothSchedulerContextStorage,
    scheduler_item: SchedulerItemStorage,
    adv_ind_header: BluetoothLeTxBufferHeaderStorage,
    scan_response_header: BluetoothLeTxBufferHeaderStorage,
    adv_ind_packet: BluetoothLeTxPacketStorage<LEGACY_ADVERTISING_TX_PACKET_BYTES>,
    scan_response_packet: BluetoothLeTxPacketStorage<LEGACY_ADVERTISING_TX_PACKET_BYTES>,
}

impl BluetoothLegacyConnectableAdvertisingGraphStorage {
    pub(super) const fn new() -> Self {
        Self {
            link_state: LinkStateStorage::new(),
            scheduler_context: BluetoothSchedulerContextStorage::new(),
            scheduler_item: SchedulerItemStorage::new(),
            adv_ind_header: BluetoothLeTxBufferHeaderStorage::new(),
            scan_response_header: BluetoothLeTxBufferHeaderStorage::new(),
            adv_ind_packet: BluetoothLeTxPacketStorage::new(),
            scan_response_packet: BluetoothLeTxPacketStorage::new(),
        }
    }

    pub(super) fn initialize_graph(
        &mut self,
        binding: &BluetoothLegacyConnectableAdvertisingGraphBinding,
    ) {
        self.scheduler_context.clear();
        self.scheduler_item
            .initialize_graph(binding.scheduler_context, binding.link_state);
        self.adv_ind_header.initialize_bound_tx_with_successor(
            binding.adv_ind_packet,
            Some(binding.scan_response_header),
        );
        self.scan_response_header
            .initialize_bound_tx(binding.scan_response_packet);
        self.link_state.initialize_graph(
            binding.scheduler_item,
            binding.adv_ind_header,
            binding.scan_response_header,
        );
        self.adv_ind_packet.clear();
        self.scan_response_packet.clear();
    }

    pub(super) fn prepare_pdus(
        &mut self,
        adv_ind: BluetoothLeTxPacketPreparedInput<'_, LEGACY_ADVERTISING_TX_PACKET_BYTES>,
        scan_response: BluetoothLeTxPacketPreparedInput<'_, LEGACY_ADVERTISING_TX_PACKET_BYTES>,
    ) -> (AdvertisingTxPacketLength, AdvertisingTxPacketLength) {
        let adv_ind_length = self.adv_ind_packet.prepare_validated_encoded_pdu(adv_ind);
        let scan_response_length = self
            .scan_response_packet
            .prepare_validated_encoded_pdu(scan_response);
        (adv_ind_length, scan_response_length)
    }

    pub(super) fn prepare_profile(
        &self,
        binding: &BluetoothLegacyConnectableAdvertisingGraphBinding,
        rx_head: BluetoothControllerSramAddress,
        rx_tail: BluetoothControllerSramAddress,
        own_address: BluetoothLegacyAdvertisingOwnAddress,
        default_tx_power_dbm: i8,
    ) {
        self.link_state.prepare_profile(
            binding,
            rx_head,
            rx_tail,
            own_address,
            default_tx_power_dbm,
        );
    }

    pub(super) fn prepare_event_fields(
        &self,
        binding: &BluetoothLegacyConnectableAdvertisingGraphBinding,
        primary_channel: BluetoothLegacyAdvertisingPrimaryChannel,
        raw_start: u32,
        raw_end: u32,
    ) -> Result<(), BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError> {
        if self.link_state.scheduler_head() != binding.scheduler_item.controller_address().address()
        {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError::SchedulerHeadMismatch,
            );
        }
        if !self.scheduler_item.is_terminal() {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError::NonTerminalSchedulerItem,
            );
        }

        let words = self.scheduler_item.reviewed_words().prepare_event_item(
            self.link_state.reviewed_words(),
            primary_channel,
            None,
            raw_start,
            raw_end,
        );
        self.scheduler_item.write_reviewed_words(words);
        self.link_state.detach_scheduler_item();
        Ok(())
    }

    pub(super) fn restore_event_fields(
        &self,
        binding: &BluetoothLegacyConnectableAdvertisingGraphBinding,
    ) {
        self.scheduler_item
            .initialize_graph(binding.scheduler_context, binding.link_state);
        self.link_state
            .restore_scheduler_item(binding.scheduler_item.controller_address());
    }

    pub(super) fn prepare_scheduler_bookkeeping(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingSnapshot {
        let snapshot = BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingSnapshot {
            control: self.scheduler_item.words[SCHEDULER_ITEM_CONTROL].get(),
            status: self.scheduler_item.words[SCHEDULER_ITEM_WORD_38].get(),
            completed_link: self.scheduler_item.words[SCHEDULER_ITEM_COMPLETED_LINK].get(),
        };
        self.scheduler_item.words[SCHEDULER_ITEM_CONTROL].set(snapshot.control & !0xff);
        self.scheduler_item.words[SCHEDULER_ITEM_WORD_38].set(u32::MAX);
        self.scheduler_item.words[SCHEDULER_ITEM_COMPLETED_LINK].set(0);
        snapshot
    }

    pub(super) fn restore_scheduler_bookkeeping(
        &self,
        snapshot: BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingSnapshot,
    ) {
        self.scheduler_item.words[SCHEDULER_ITEM_CONTROL].set(snapshot.control);
        self.scheduler_item.words[SCHEDULER_ITEM_WORD_38].set(snapshot.status);
        self.scheduler_item.words[SCHEDULER_ITEM_COMPLETED_LINK].set(snapshot.completed_link);
    }

    pub(super) fn prepare_empty_list_link(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSoftwareLinkSnapshot {
        let snapshot = BluetoothLegacyConnectableAdvertisingSoftwareLinkSnapshot(
            self.scheduler_item.words[SCHEDULER_ITEM_SOFTWARE_NEXT].get(),
        );
        self.scheduler_item.words[SCHEDULER_ITEM_SOFTWARE_NEXT].set(0);
        snapshot
    }

    pub(super) fn restore_empty_list_link(
        &self,
        snapshot: BluetoothLegacyConnectableAdvertisingSoftwareLinkSnapshot,
    ) {
        self.scheduler_item.words[SCHEDULER_ITEM_SOFTWARE_NEXT].set(snapshot.0);
    }

    pub(super) fn completion_status(
        &self,
    ) -> Option<BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus> {
        self.scheduler_item.completion_status()
    }

    #[cfg(test)]
    pub(super) fn model_controller_completion(
        &self,
        status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    ) {
        self.scheduler_item.model_controller_completion(status);
    }

    pub(super) fn adv_ind_pdu(&self, length: AdvertisingTxPacketLength) -> &[u8] {
        self.adv_ind_packet.prepared_pdu(length)
    }

    pub(super) fn scan_response_pdu(&self, length: AdvertisingTxPacketLength) -> &[u8] {
        self.scan_response_packet.prepared_pdu(length)
    }

    pub(super) fn retains_prepared_graph(
        &self,
        binding: &BluetoothLegacyConnectableAdvertisingGraphBinding,
        rx_head: BluetoothControllerSramAddress,
        rx_tail: BluetoothControllerSramAddress,
    ) -> bool {
        self.link_state
            .retains_prepared_graph(binding, rx_head, rx_tail)
            && self
                .scheduler_item
                .retains_graph(binding.scheduler_context, binding.link_state)
            && self.adv_ind_header.retains_bound_tx_with_successor(
                binding.adv_ind_packet,
                Some(binding.scan_response_header),
            )
            && self
                .scan_response_header
                .retains_bound_tx_with_successor(binding.scan_response_packet, None)
    }

    #[cfg(test)]
    pub(super) fn emulate_missing_rx_consumer_link(&self) {
        self.link_state.emulate_missing_rx_consumer_link();
    }

    #[cfg(test)]
    pub(super) fn emulate_missing_scheduler_head(&self) {
        self.link_state.detach_scheduler_item();
    }
}

pub(super) struct BluetoothLegacyConnectableAdvertisingGraphBinding {
    identity: BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
    base: BluetoothControllerSramAddress,
    end_exclusive: u32,
    link_state: BluetoothControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramLinkAddress,
    scheduler_item: BluetoothControllerSramLinkAddress,
    adv_ind_header: BluetoothControllerSramLinkAddress,
    scan_response_header: BluetoothControllerSramLinkAddress,
    adv_ind_packet: AdvertisingTxPacketAddress,
    scan_response_packet: AdvertisingTxPacketAddress,
}

impl BluetoothLegacyConnectableAdvertisingGraphBinding {
    pub(super) fn new(
        identity: BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
        base: u32,
    ) -> Result<Self, BluetoothLegacyConnectableAdvertisingMemoryGraphBindError> {
        let base_address = BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothLegacyConnectableAdvertisingMemoryGraphBindError::InvalidBase)?;
        let graph_bytes =
            core::mem::size_of::<BluetoothLegacyConnectableAdvertisingMemoryGraphStorage>() as u32;
        let end_exclusive = base.checked_add(graph_bytes).ok_or(
            BluetoothLegacyConnectableAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram,
        )?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram,
            );
        }

        let graph_offset = core::mem::offset_of!(
            BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
            graph
        ) as u32;
        let address = |offset: usize| {
            base.checked_add(graph_offset)
                .and_then(|address| address.checked_add(offset as u32))
                .ok_or(
                    BluetoothLegacyConnectableAdvertisingMemoryGraphBindError::ExtentOutsidePhysicalSram,
                )
        };
        let link = |offset: usize| {
            BluetoothControllerSramLinkAddress::new(address(offset)?).map_err(|_| {
                BluetoothLegacyConnectableAdvertisingMemoryGraphBindError::ZeroCompressedLink
            })
        };

        let link_state = link(core::mem::offset_of!(
            BluetoothLegacyConnectableAdvertisingGraphStorage,
            link_state
        ))?;
        let scheduler_context = link(core::mem::offset_of!(
            BluetoothLegacyConnectableAdvertisingGraphStorage,
            scheduler_context
        ))?;
        let scheduler_item = link(core::mem::offset_of!(
            BluetoothLegacyConnectableAdvertisingGraphStorage,
            scheduler_item
        ))?;
        let adv_ind_header = link(core::mem::offset_of!(
            BluetoothLegacyConnectableAdvertisingGraphStorage,
            adv_ind_header
        ))?;
        let scan_response_header = link(core::mem::offset_of!(
            BluetoothLegacyConnectableAdvertisingGraphStorage,
            scan_response_header
        ))?;
        let packet = |offset: usize| {
            AdvertisingTxPacketAddress::new(address(offset)?).map_err(|_| {
                BluetoothLegacyConnectableAdvertisingMemoryGraphBindError::InvalidPacketExtent
            })
        };
        let adv_ind_packet = packet(core::mem::offset_of!(
            BluetoothLegacyConnectableAdvertisingGraphStorage,
            adv_ind_packet
        ))?;
        let scan_response_packet = packet(core::mem::offset_of!(
            BluetoothLegacyConnectableAdvertisingGraphStorage,
            scan_response_packet
        ))?;

        Ok(Self {
            identity,
            base: base_address,
            end_exclusive,
            link_state,
            scheduler_context,
            scheduler_item,
            adv_ind_header,
            scan_response_header,
            adv_ind_packet,
            scan_response_packet,
        })
    }

    pub(super) const fn identity(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity {
        self.identity
    }

    pub(super) const fn range(&self) -> (u32, u32) {
        (self.base.address(), self.end_exclusive)
    }

    pub(super) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.scheduler_item.controller_address()
    }

    pub(super) const fn is_disjoint_from_receive_pool(
        &self,
        pool: &BluetoothNonScanningRxMemoryCpuOwned,
    ) -> bool {
        let graph = self.range();
        let receive = pool.controller_range();
        graph.1 <= receive.0 || receive.1 <= graph.0
    }
}

pub(super) const fn response_capable_scheduler_span(
    payload_length: u8,
) -> BluetoothLegacyConnectableAdvertisingSchedulerSpan {
    BluetoothLegacyConnectableAdvertisingSchedulerSpan(
        (payload_length as u32)
            .wrapping_mul(8)
            .wrapping_add(LE_1M_FIXED_PACKET_MICROS)
            .wrapping_add(VENDOR_RESPONSE_CAPABLE_ITEM_TAIL_MICROS),
    )
}
