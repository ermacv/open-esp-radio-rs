//! Passive LE 1M scanner instances.
//!
//! One instance holds the scanner link state, the scheduler context and three
//! scheduler items. Hardware receives into the global scanning chain and tags
//! each packet with the number of the receiving item; the instance only names
//! that source. The items form the vendor's private free chain: link-state
//! `+0x64` holds the free head, and each item's hardware next link names the
//! next free item. An event takes the free head, as `r_ble_lll_scan_restart`
//! does, and finishing the event returns it.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::bluetooth::BluetoothControllerLatchedTime;
use vcell::VolatileCell;

use crate::{
    le_rx_chain::{LeRxChain, LeRxSource, LeRxTag},
    passive_scanning_event_image::{
        BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS, PassiveScanLinkStateImage,
        PassiveScanPrimaryChannel, PassiveScanResetConfig, PassiveScanRxHeadProjection,
        PassiveScanSchedulerItemWords, PassiveScanSchedulerWindow, PassiveScanStartSelection,
    },
    rx_memory_list::RxMemoryListClass,
    scheduler_context::SchedulerContextStorage,
    scheduler_item::SchedulerItemHeader,
    scheduler_pool::{
        SchedulerPoolBindError, SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind,
        SchedulerRolePool, SchedulerRoleStorage, sealed,
    },
    sram_link::ControllerSramLinkAddress,
};

/// Scheduler items of one scanner instance.
pub const BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT: usize = 3;

const ITEMS: usize = BLUETOOTH_PASSIVE_SCAN_SCHEDULER_ITEM_COUNT;
const LINK_STATE_RX_CLASS_WORD: usize = 0x20 / 4;
const LINK_STATE_SCHEDULER_HEAD_WORD: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD_WORD: usize = 0x68 / 4;
const LINK_STATE_RX_TAIL_WORD: usize = 0x70 / 4;
const LINK_STATE_RX_SWAP_RESERVE_WORD: usize = 0x78 / 4;
const SCHEDULER_ITEM_BYTES: usize = 0x60;
const SCHEDULER_ITEM_WORDS: usize = SCHEDULER_ITEM_BYTES / 4;
const SCHEDULER_ITEM_CONTEXT_WORD: usize = 1;
const SCHEDULER_ITEM_LINK_STATE_WORD: usize = 0x08 / 4;
const SCHEDULER_ITEM_WORD_14: usize = 0x14 / 4;
const SCHEDULER_ITEM_WORD_18: usize = 0x18 / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_WORD: usize = 0x1c / 4;
const SCHEDULER_ITEM_ALLOCATION_NUMBER_WORD: usize = 0x20 / 4;
const SCHEDULER_ITEM_POSITIONAL_24_WORD: usize = 0x24 / 4;
const SCHEDULER_ITEM_EVENT_CLASS_WORD: usize = 0x2c / 4;
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0030_0000;
const SCHEDULER_ITEM_LINK_STATE_PREFIX: u32 = 0x00c0_0000;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: u32 = 0x0fdf_ffff;
const SCHEDULER_ITEM_POSITIONAL_24_IMAGE: u32 = 0x0007_bdef;
const SCHEDULER_ITEM_EVENT_CLASS_IMAGE: u32 = 1;

/// Scanner link state.
#[repr(C, align(4))]
struct LinkStateStorage {
    words: [VolatileCell<u32>; BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS],
}

impl LinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS],
        }
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn install(&self, image: PassiveScanLinkStateImage) {
        for (cell, word) in self.words.iter().zip(image.words()) {
            cell.set(word);
        }
    }

    fn image(&self) -> PassiveScanLinkStateImage {
        PassiveScanLinkStateImage::from_words(core::array::from_fn(|index| self.words[index].get()))
    }

    fn free_head(&self) -> u32 {
        self.words[LINK_STATE_SCHEDULER_HEAD_WORD].get()
    }

    fn set_free_head(&self, head: Option<ControllerSramLinkAddress>) {
        self.words[LINK_STATE_SCHEDULER_HEAD_WORD]
            .set(head.map_or(0, |head| head.controller_address().address()));
    }

    /// Select the scanning class and snapshot the global chain's software
    /// endpoints, as the vendor global RX-link update does.
    fn join_receive_chain(&self, (head, tail, spare): (u32, u32, u32)) {
        let class = &self.words[LINK_STATE_RX_CLASS_WORD];
        class.set(RxMemoryListClass::Scanning.select_in_link_state(class.get()));
        self.words[LINK_STATE_RX_HEAD_WORD].set(head);
        self.words[LINK_STATE_RX_TAIL_WORD].set(tail);
        self.words[LINK_STATE_RX_SWAP_RESERVE_WORD].set(spare);
    }
}

/// One scanner scheduler item.
#[repr(C, align(4))]
struct ItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl ItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    fn header(&self) -> SchedulerItemHeader<'_, [VolatileCell<u32>; SCHEDULER_ITEM_WORDS]> {
        SchedulerItemHeader::new(&self.words)
    }

    /// The common and scanner allocators' image, with the item linked to its
    /// free-chain successor.
    fn initialize(
        &self,
        number: u16,
        next_free: Option<ControllerSramLinkAddress>,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        let header = self.header();
        header.set_hardware_next_word(SCHEDULER_ITEM_ALLOCATION_PREFIX);
        header.link_hardware_next(next_free);
        self.words[SCHEDULER_ITEM_CONTEXT_WORD].set(scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE_WORD]
            .set(SCHEDULER_ITEM_LINK_STATE_PREFIX | link_state.compressed_image());
        self.words[SCHEDULER_ITEM_ALLOCATION_FLAGS_WORD].set(SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE);
        self.words[SCHEDULER_ITEM_ALLOCATION_NUMBER_WORD].set(u32::from(number));
        self.words[SCHEDULER_ITEM_POSITIONAL_24_WORD].set(SCHEDULER_ITEM_POSITIONAL_24_IMAGE);
        self.words[SCHEDULER_ITEM_EVENT_CLASS_WORD].set(SCHEDULER_ITEM_EVENT_CLASS_IMAGE);
    }

    fn reviewed_words(&self) -> PassiveScanSchedulerItemWords {
        let header = self.header();
        PassiveScanSchedulerItemWords {
            word_00: header.hardware_next_word(),
            word_04: self.words[SCHEDULER_ITEM_CONTEXT_WORD].get(),
            word_14: self.words[SCHEDULER_ITEM_WORD_14].get(),
            word_18: self.words[SCHEDULER_ITEM_WORD_18].get(),
            word_38: header.status(),
            raw_start_word_44: header.raw_start(),
            raw_end_word_48: header.raw_end(),
        }
    }

    fn write_reviewed_words(&self, words: PassiveScanSchedulerItemWords) {
        let header = self.header();
        header.set_hardware_next_word(words.word_00);
        self.words[SCHEDULER_ITEM_CONTEXT_WORD].set(words.word_04);
        self.words[SCHEDULER_ITEM_WORD_14].set(words.word_14);
        self.words[SCHEDULER_ITEM_WORD_18].set(words.word_18);
        header.set_status(words.word_38);
        header.set_raw_start(words.raw_start_word_44);
        header.set_raw_end(words.raw_end_word_48);
    }
}

/// Controller-SRAM graph of one scanner instance.
#[repr(C)]
pub struct PassiveScanStorage {
    link_state: LinkStateStorage,
    scheduler_context: SchedulerContextStorage,
    items: [ItemStorage; ITEMS],
}

/// Addresses and item numbers of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct PassiveScanBinding {
    link_state: ControllerSramLinkAddress,
    scheduler_context: ControllerSramLinkAddress,
    items: [ControllerSramLinkAddress; ITEMS],
    first_number: u16,
}

impl PassiveScanBinding {
    fn item_at(&self, address: u32) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.controller_address().address() == address)
    }
}

/// Preparation state of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum PassiveScanState {
    Empty,
    Reset { event: Option<u8> },
}

impl sealed::Sealed for PassiveScanStorage {}

impl SchedulerRoleStorage for PassiveScanStorage {
    const KIND: SchedulerRoleKind = SchedulerRoleKind::PassiveScanning;
    const ITEMS: usize = ITEMS;
    const NUMBERS: usize = ITEMS;
    const NEW: Self = Self {
        link_state: LinkStateStorage::new(),
        scheduler_context: SchedulerContextStorage::new(),
        items: [const { ItemStorage::new() }; ITEMS],
    };
    type Binding = PassiveScanBinding;
    type State = PassiveScanState;
    const INITIAL_STATE: PassiveScanState = PassiveScanState::Empty;

    fn bind(base: u32, first_number: u16) -> Result<PassiveScanBinding, SchedulerPoolBindError> {
        let link = |offset: usize| {
            ControllerSramLinkAddress::new(base + offset as u32)
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)
        };
        let items = core::mem::offset_of!(Self, items);
        let item = core::mem::size_of::<ItemStorage>();
        Ok(PassiveScanBinding {
            link_state: link(core::mem::offset_of!(Self, link_state))?,
            scheduler_context: link(core::mem::offset_of!(Self, scheduler_context))?,
            items: [link(items)?, link(items + item)?, link(items + 2 * item)?],
            first_number,
        })
    }

    fn item_words(&self, item: usize) -> &[VolatileCell<u32>] {
        &self.items[item].words
    }

    fn item_link(binding: &PassiveScanBinding, item: usize) -> ControllerSramLinkAddress {
        binding.items[item]
    }

    fn reinitialize(&mut self, binding: &PassiveScanBinding) -> Self::State {
        self.link_state.clear();
        self.scheduler_context.clear();
        for (index, item) in self.items.iter().enumerate() {
            item.initialize(
                binding.first_number + index as u16,
                index.checked_sub(1).map(|previous| binding.items[previous]),
                binding.scheduler_context,
                binding.link_state,
            );
        }
        self.link_state
            .set_free_head(Some(binding.items[ITEMS - 1]));
        PassiveScanState::Empty
    }

    fn admits(state: &PassiveScanState, item: usize) -> bool {
        matches!(state, PassiveScanState::Reset { event: Some(event) } if usize::from(*event) == item)
    }
}

/// Pool of scanner instances.
pub type PassiveScanPool<const N: usize> = SchedulerRolePool<PassiveScanStorage, N>;

/// Why a scanner operation was refused. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PassiveScanError {
    Pool(SchedulerPoolError),
    /// The operation does not follow the instance's preparation state.
    State,
    /// The chain receives another class.
    ForeignReceiveClass,
    /// The free chain holds no item.
    NoFreeItem,
    /// The free head names no item of this instance.
    ForeignFreeHead,
}

/// Item and raw window of one prepared scan window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PassiveScanEvent {
    item: usize,
    window: PassiveScanSchedulerWindow,
}

impl PassiveScanEvent {
    /// Item that carries the window.
    pub const fn item(&self) -> usize {
        self.item
    }

    /// Raw controller window of the item.
    pub const fn raw_window(&self) -> (u32, u32) {
        (self.window.start(), self.window.end())
    }
}

impl<const N: usize> PassiveScanPool<N> {
    /// Apply the restricted passive LE 1M reset and join the scanning chain.
    pub fn reset<const PACKETS: usize>(
        &mut self,
        instance: &SchedulerRoleInstance,
        chain: &LeRxChain<PACKETS>,
        config: PassiveScanResetConfig,
    ) -> Result<(), PassiveScanError> {
        if chain.class() != RxMemoryListClass::Scanning {
            return Err(PassiveScanError::ForeignReceiveClass);
        }
        let cpu = self.cpu(instance).map_err(PassiveScanError::Pool)?;
        if !matches!(cpu.state, PassiveScanState::Empty) {
            return Err(PassiveScanError::State);
        }
        let free_head = cpu.graph.link_state.free_head();
        cpu.graph
            .link_state
            .install(PassiveScanLinkStateImage::restricted_passive_le_1m(
                PassiveScanRxHeadProjection::from_bound(chain.head_link()),
                config,
            ));
        cpu.graph.link_state.words[LINK_STATE_SCHEDULER_HEAD_WORD].set(free_head);
        cpu.graph.link_state.join_receive_chain(chain.snapshot());
        *cpu.state = PassiveScanState::Reset { event: None };
        Ok(())
    }

    /// Lower one passive window into the free head item.
    pub fn prepare_event(
        &mut self,
        instance: &SchedulerRoleInstance,
        channel: PassiveScanPrimaryChannel,
        window: PassiveScanSchedulerWindow,
        start_selection: PassiveScanStartSelection,
        controller_time: BluetoothControllerLatchedTime,
    ) -> Result<PassiveScanEvent, PassiveScanError> {
        let cpu = self.cpu(instance).map_err(PassiveScanError::Pool)?;
        if !matches!(cpu.state, PassiveScanState::Reset { event: None }) {
            return Err(PassiveScanError::State);
        }
        let head = cpu.graph.link_state.free_head();
        if head == 0 {
            return Err(PassiveScanError::NoFreeItem);
        }
        let index = cpu
            .binding
            .item_at(head)
            .ok_or(PassiveScanError::ForeignFreeHead)?;
        let item = &cpu.graph.items[index];
        let next_free = item.header().hardware_next_image();
        let next_free = cpu
            .binding
            .items
            .iter()
            .copied()
            .find(|link| next_free != 0 && link.compressed_image() == next_free);

        let link_state = &cpu.graph.link_state;
        link_state.install(link_state.image().with_controller_time(controller_time));
        item.write_reviewed_words(item.reviewed_words().prepare_first_event(
            link_state.image(),
            channel,
            window,
            start_selection,
        ));
        // Detach the item from the free chain before the executor links it.
        item.header().link_hardware_next(None);
        link_state.set_free_head(next_free);
        *cpu.state = PassiveScanState::Reset {
            event: Some(index as u8),
        };
        Ok(PassiveScanEvent {
            item: index,
            window,
        })
    }

    /// Return the item of a finished or abandoned window to the free chain.
    pub fn finish_event(
        &mut self,
        instance: &SchedulerRoleInstance,
    ) -> Result<(), PassiveScanError> {
        let cpu = self.cpu(instance).map_err(PassiveScanError::Pool)?;
        let PassiveScanState::Reset { event: Some(index) } = *cpu.state else {
            return Err(PassiveScanError::State);
        };
        let index = usize::from(index);
        let head = cpu.graph.link_state.free_head();
        let next_free = cpu
            .binding
            .item_at(head)
            .map(|head| cpu.binding.items[head]);
        let item = &cpu.graph.items[index];
        item.header().link_hardware_next(next_free);
        item.header().set_status(0);
        cpu.graph
            .link_state
            .set_free_head(Some(cpu.binding.items[index]));
        *cpu.state = PassiveScanState::Reset { event: None };
        Ok(())
    }

    /// Packets that `item` of the instance receives.
    pub fn receive_source(
        &self,
        instance: &SchedulerRoleInstance,
        item: usize,
    ) -> Result<LeRxSource, PassiveScanError> {
        let (_, binding, _) = self.shared(instance).map_err(PassiveScanError::Pool)?;
        if item >= ITEMS {
            return Err(PassiveScanError::Pool(SchedulerPoolError::NoSuchItem));
        }
        let tag = LeRxTag::new(binding.first_number + item as u16)
            .expect("the allocation numbers fit twelve bits");
        Ok(LeRxSource::new(tag, RxMemoryListClass::Scanning, false))
    }
}

#[cfg(test)]
mod tests;
