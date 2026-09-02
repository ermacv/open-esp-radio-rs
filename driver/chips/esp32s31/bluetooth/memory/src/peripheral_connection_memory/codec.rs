//! Private SRAM layout and word codec for one peripheral connection graph.

use core::{marker::PhantomPinned, num::NonZeroU32, pin::Pin};

use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress;
use pin_project::pin_project;
use vcell::VolatileCell;

use super::{
    BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT,
    BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES,
    BluetoothPeripheralConnectionCapturedAnchorTime, BluetoothPeripheralConnectionDataChannel,
    BluetoothPeripheralConnectionDefaultTxPowerDbm, BluetoothPeripheralConnectionEventSpan,
    BluetoothPeripheralConnectionIdentity, BluetoothPeripheralConnectionIntervalTicks,
    BluetoothPeripheralConnectionMemoryGraphBindError,
    BluetoothPeripheralConnectionMemoryGraphIdentity, BluetoothPeripheralConnectionReceiveWait,
    BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
    BluetoothPeripheralConnectionSchedulerPriority, BluetoothPeripheralConnectionSchedulerWindow,
};
use crate::{
    direction_finding_workspace::BluetoothDirectionFindingWorkspaceLink,
    le_tx_power::rounded_tx_power,
    scheduler_context::BluetoothSchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

const LINK_STATE_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES / 4;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES / 4;
const TX_SENTINEL_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES / 4;

const LINK_STATE_SCHEDULER_HEAD: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL: usize = 0x74 / 4;
const LINK_STATE_RX_RESERVE: usize = 0x78 / 4;
const LINK_STATE_TX_PATH: usize = 0;
const LINK_STATE_CRC_INITIALIZATION: usize = 0x2c / 4;
const LINK_STATE_ACCESS_ADDRESS: usize = 0x38 / 4;
const LINK_STATE_ROUNDED_POWER: usize = 1;
const LINK_STATE_RX_PATH: usize = 2;
const LINK_STATE_CONTROL_POLICY: usize = 3;
const LINK_STATE_PACKET_FLAGS: usize = 0x14 / 4;
const LINK_STATE_INTERVAL_TICKS: usize = 0x18 / 4;
const LINK_STATE_PACKET_HISTORY: usize = 0x1c / 4;
const LINK_STATE_PACKET_CONTROL: usize = 0x20 / 4;
const LINK_STATE_PACKET_SEQUENCE: usize = 0x30 / 4;
const LINK_STATE_EVENT_SPAN_OR_CAPTURED_ANCHOR: usize = 0x34 / 4;
const LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION: usize = 0x50 / 4;
const LINK_STATE_DIRECTION_FINDING_POLICY: usize = 0x54 / 4;
const LINK_STATE_EVENT_PRIORITY: usize = 0x60 / 4;
const LINK_STATE_ROUNDED_POWER_MASK: u32 = 0x0f80_0000;
const LINK_STATE_TX_PATH_VALID: u32 = 1 << 31;
const LINK_STATE_TX_QUEUE_READY: u32 = 1 << 28;
const LINK_STATE_SUPPORTED_MAX_TX_OCTETS: u32 = 251;
const LINK_STATE_RX_UNCONSUMED_LIMIT: u32 = 0xff;
const LINK_STATE_CONTROL_POLICY_ACTIVE: u32 = 1 << 31;
const LINK_STATE_BASELINE_CONTROL_POLICY: u32 = 2;
const LINK_STATE_CRC_CONTEXT_READY: u32 = 1 << 31;
const LINK_STATE_PACKET_SEQUENCE_BASELINE: u32 = 0x1e00;
const LINK_STATE_COMMON_RADIO_POLICY_BASELINE: u32 = 3;
const LINK_STATE_DIRECTION_FINDING_RETAINED_POLICY: u32 = 0xbf00_0000;
const LINK_STATE_DIRECTION_FINDING_CONFIGURATION_READY: u32 = 1 << 30;
const LINK_STATE_DIRECTION_FINDING_POLICY_RETAINED: u32 = 0x8007_ffff;
const LINK_STATE_DIRECTION_FINDING_DISABLED_BASELINE: u32 = 0x0018_0000;

const SCHEDULER_ITEM_NEXT: usize = 0;
const SCHEDULER_ITEM_CONTEXT: usize = 1;
const SCHEDULER_ITEM_LINK_STATE: usize = 2;
const SCHEDULER_ITEM_CLASS: usize = 0x4c / 4;
const SCHEDULER_ITEM_CONTEXT_STATE: usize = 1;
const SCHEDULER_ITEM_RATE_AND_POWER: usize = 0x14 / 4;
const SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY: usize = 0x18 / 4;
const SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION: usize = 0x2c / 4;
const SCHEDULER_ITEM_STATUS: usize = 0x38 / 4;
const SCHEDULER_ITEM_START: usize = 0x44 / 4;
const SCHEDULER_ITEM_END: usize = 0x48 / 4;
const SCHEDULER_ITEM_LINK_MASK: u32 = 0x000f_ffff;
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0010_0000;
const SCHEDULER_ITEM_PERIPHERAL_PREFIX: u32 = 0x0020_0000;
const SCHEDULER_ITEM_CONNECTION_CLASS: u32 = 3 << 8;
const SCHEDULER_ITEM_CONTEXT_READY: u32 = 1 << 31;
const SCHEDULER_ITEM_RATE_AND_POWER_MASK: u32 = 0xfff0_0000;
const SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY_MASK: u32 = 0x0000_7fff;
const SCHEDULER_ITEM_RECEIVE_WAIT_SHORT_MODE: u32 = 0x000f_0000;

const TX_SENTINEL_STATE: usize = 0x0c / 4;
const TX_SENTINEL_CLASS: usize = 0x10 / 4;
const TX_SENTINEL_EMPTY_QUEUE: u32 = 0x8000_0000;
const TX_SENTINEL_CONNECTION_CLASS: u32 = 2;

#[repr(C, align(4))]
struct BluetoothPeripheralConnectionLinkStateStorage {
    words: [VolatileCell<u32>; LINK_STATE_WORDS],
}

impl BluetoothPeripheralConnectionLinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; LINK_STATE_WORDS],
        }
    }

    fn initialize_allocation(
        &self,
        scheduler_head: BluetoothControllerSramLinkAddress,
        tx_sentinel: BluetoothControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        self.words[LINK_STATE_SCHEDULER_HEAD].set(scheduler_head.controller_address().address());
        self.words[LINK_STATE_TX_HEAD].set(tx_sentinel.controller_address().address());
        self.words[LINK_STATE_TX_TAIL].set(tx_sentinel.controller_address().address());
    }

    fn has_empty_receive_queue(&self) -> bool {
        self.words[LINK_STATE_RX_HEAD].get() == 0
            && self.words[LINK_STATE_RX_TAIL].get() == 0
            && self.words[LINK_STATE_RX_RESERVE].get() == 0
    }

    fn install_receive_pool(
        &self,
        head: BluetoothControllerSramAddress,
        tail: BluetoothControllerSramAddress,
    ) {
        self.words[LINK_STATE_RX_HEAD].set(head.address());
        self.words[LINK_STATE_RX_TAIL].set(tail.address());
        self.words[LINK_STATE_RX_RESERVE].set(0);
    }

    fn clear_receive_pool(&self) {
        self.words[LINK_STATE_RX_HEAD].set(0);
        self.words[LINK_STATE_RX_TAIL].set(0);
        self.words[LINK_STATE_RX_RESERVE].set(0);
    }

    fn retains_transmit_sentinel(&self, sentinel: BluetoothControllerSramLinkAddress) -> bool {
        let address = sentinel.controller_address().address();
        self.words[LINK_STATE_TX_HEAD].get() == address
            && self.words[LINK_STATE_TX_TAIL].get() == address
    }

    fn retains_scheduler_head(&self, head: BluetoothControllerSramLinkAddress) -> bool {
        self.words[LINK_STATE_SCHEDULER_HEAD].get() == head.controller_address().address()
    }

    fn install_scheduler_head(&self, head: BluetoothControllerSramLinkAddress) {
        self.words[LINK_STATE_SCHEDULER_HEAD].set(head.controller_address().address());
    }

    fn prepare_identity(&self, identity: BluetoothPeripheralConnectionIdentity) {
        self.words[LINK_STATE_CRC_INITIALIZATION]
            .set(u32::from_le_bytes(identity.crc_initialization_word()));
        self.words[LINK_STATE_ACCESS_ADDRESS]
            .set(u32::from_le_bytes(identity.access_address_wire_bytes()));
    }

    fn prepare_event_profile(
        &self,
        receive_head: BluetoothControllerSramAddress,
        transmit_sentinel: BluetoothControllerSramLinkAddress,
        interval: BluetoothPeripheralConnectionIntervalTicks,
        event_span: BluetoothPeripheralConnectionEventSpan,
        default_tx_power: BluetoothPeripheralConnectionDefaultTxPowerDbm,
        priority: BluetoothPeripheralConnectionSchedulerPriority,
    ) {
        self.words[LINK_STATE_TX_PATH].set(
            LINK_STATE_TX_PATH_VALID
                | LINK_STATE_TX_QUEUE_READY
                | (LINK_STATE_SUPPORTED_MAX_TX_OCTETS << 20)
                | transmit_sentinel.compressed_image(),
        );
        self.words[LINK_STATE_RX_PATH]
            .set((LINK_STATE_RX_UNCONSUMED_LIMIT << 20) | receive_head.compressed_image());
        self.words[LINK_STATE_CONTROL_POLICY].set(
            LINK_STATE_CONTROL_POLICY_ACTIVE
                | (LINK_STATE_BASELINE_CONTROL_POLICY << 20)
                | (LINK_STATE_BASELINE_CONTROL_POLICY << 24),
        );
        self.words[LINK_STATE_PACKET_FLAGS].set(0);
        self.words[LINK_STATE_PACKET_HISTORY].set(0);
        self.words[LINK_STATE_PACKET_CONTROL].set(0);
        self.words[LINK_STATE_CRC_INITIALIZATION]
            .set(self.words[LINK_STATE_CRC_INITIALIZATION].get() | LINK_STATE_CRC_CONTEXT_READY);
        self.words[LINK_STATE_PACKET_SEQUENCE].set(LINK_STATE_PACKET_SEQUENCE_BASELINE);
        self.words[LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION]
            .set(LINK_STATE_COMMON_RADIO_POLICY_BASELINE << 24);
        self.words[LINK_STATE_EVENT_PRIORITY].set(u32::from(priority.value()));

        let power = u32::from(rounded_tx_power(default_tx_power.dbm()));
        let current = self.words[LINK_STATE_ROUNDED_POWER].get();
        self.words[LINK_STATE_ROUNDED_POWER]
            .set((current & !LINK_STATE_ROUNDED_POWER_MASK) | (power << 23));
        self.words[LINK_STATE_INTERVAL_TICKS].set(interval.ticks());
        self.words[LINK_STATE_EVENT_SPAN_OR_CAPTURED_ANCHOR].set(event_span.ticks());
    }

    fn captured_anchor_time(&self) -> BluetoothPeripheralConnectionCapturedAnchorTime {
        BluetoothPeripheralConnectionCapturedAnchorTime::from_controller_sram_word(
            self.words[LINK_STATE_EVENT_SPAN_OR_CAPTURED_ANCHOR].get(),
        )
    }

    #[cfg(test)]
    fn model_controller_complete_event(
        &self,
        prepared_span: BluetoothPeripheralConnectionEventSpan,
        captured_anchor: BluetoothPeripheralConnectionCapturedAnchorTime,
    ) -> bool {
        if self.words[LINK_STATE_EVENT_SPAN_OR_CAPTURED_ANCHOR].get() != prepared_span.ticks() {
            return false;
        }
        self.words[LINK_STATE_EVENT_SPAN_OR_CAPTURED_ANCHOR]
            .set(captured_anchor.wrapping_controller_ticks());
        true
    }

    fn install_direction_finding_workspace(
        &self,
        workspace: BluetoothDirectionFindingWorkspaceLink,
    ) {
        let configuration =
            self.words[LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION].get();
        self.words[LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION].set(
            (configuration & LINK_STATE_DIRECTION_FINDING_RETAINED_POLICY)
                | LINK_STATE_DIRECTION_FINDING_CONFIGURATION_READY
                | workspace.compressed_link_state_configuration(),
        );

        let policy = self.words[LINK_STATE_DIRECTION_FINDING_POLICY].get();
        self.words[LINK_STATE_DIRECTION_FINDING_POLICY].set(
            (policy & LINK_STATE_DIRECTION_FINDING_POLICY_RETAINED)
                | LINK_STATE_DIRECTION_FINDING_DISABLED_BASELINE,
        );
    }

    fn remove_direction_finding_workspace(&self) {
        self.words[LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION]
            .set(LINK_STATE_COMMON_RADIO_POLICY_BASELINE << 24);
        self.words[LINK_STATE_DIRECTION_FINDING_POLICY].set(0);
    }

    fn rounded_power(&self) -> u32 {
        (self.words[LINK_STATE_ROUNDED_POWER].get() & LINK_STATE_ROUNDED_POWER_MASK) >> 23
    }

    fn identity(&self) -> BluetoothPeripheralConnectionIdentity {
        let crc_initialization = self.words[LINK_STATE_CRC_INITIALIZATION]
            .get()
            .to_le_bytes();
        BluetoothPeripheralConnectionIdentity::new(
            self.words[LINK_STATE_ACCESS_ADDRESS].get().to_le_bytes(),
            [
                crc_initialization[0],
                crc_initialization[1],
                crc_initialization[2],
            ],
        )
    }
}

#[repr(C, align(4))]
struct BluetoothPeripheralConnectionSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl BluetoothPeripheralConnectionSchedulerItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    fn initialize_allocation(
        &self,
        successor: Option<BluetoothControllerSramLinkAddress>,
        scheduler_context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        let successor = successor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image);
        self.words[SCHEDULER_ITEM_NEXT].set(SCHEDULER_ITEM_ALLOCATION_PREFIX | successor);
        self.words[SCHEDULER_ITEM_CONTEXT].set(scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE]
            .set(SCHEDULER_ITEM_PERIPHERAL_PREFIX | link_state.compressed_image());
        self.words[SCHEDULER_ITEM_CLASS].set(SCHEDULER_ITEM_CONNECTION_CLASS);
    }

    fn retains_allocation(
        &self,
        successor: Option<BluetoothControllerSramLinkAddress>,
        scheduler_context: BluetoothControllerSramLinkAddress,
        link_state: BluetoothControllerSramLinkAddress,
    ) -> bool {
        let successor = successor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image);
        self.words[SCHEDULER_ITEM_NEXT].get() & SCHEDULER_ITEM_LINK_MASK == successor
            && self.words[SCHEDULER_ITEM_CONTEXT].get() & SCHEDULER_ITEM_LINK_MASK
                == scheduler_context.compressed_image()
            && self.words[SCHEDULER_ITEM_LINK_STATE].get() & SCHEDULER_ITEM_LINK_MASK
                == link_state.compressed_image()
    }

    fn detach_hardware_predecessor(&self) {
        self.words[SCHEDULER_ITEM_NEXT]
            .set(self.words[SCHEDULER_ITEM_NEXT].get() & !SCHEDULER_ITEM_LINK_MASK);
    }

    fn restore_hardware_predecessor(&self, predecessor: BluetoothControllerSramLinkAddress) {
        self.words[SCHEDULER_ITEM_NEXT]
            .set(SCHEDULER_ITEM_ALLOCATION_PREFIX | predecessor.compressed_image());
    }

    fn mark_in_flight(&self) {
        self.words[SCHEDULER_ITEM_STATUS].set(u32::MAX);
    }

    fn restore_cpu_owned_status(&self) {
        self.words[SCHEDULER_ITEM_STATUS].set(0);
    }

    fn completion_status(
        &self,
    ) -> Option<BluetoothPeripheralConnectionSchedulerItemCompletionStatus> {
        match self.words[SCHEDULER_ITEM_STATUS].get() {
            u32::MAX => None,
            0 => Some(BluetoothPeripheralConnectionSchedulerItemCompletionStatus::Zero),
            status => Some(
                BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero(
                    NonZeroU32::new(status).expect("a nonzero branch retains a nonzero value"),
                ),
            ),
        }
    }

    fn prepare_reviewed_first_event_fields(
        &self,
        rounded_power: u32,
        channel: BluetoothPeripheralConnectionDataChannel,
        window: BluetoothPeripheralConnectionSchedulerWindow,
        receive_wait: BluetoothPeripheralConnectionReceiveWait,
        priority: BluetoothPeripheralConnectionSchedulerPriority,
    ) {
        self.words[SCHEDULER_ITEM_CONTEXT_STATE]
            .set(self.words[SCHEDULER_ITEM_CONTEXT_STATE].get() | SCHEDULER_ITEM_CONTEXT_READY);
        self.words[SCHEDULER_ITEM_RATE_AND_POWER].set(
            (self.words[SCHEDULER_ITEM_RATE_AND_POWER].get() & !SCHEDULER_ITEM_RATE_AND_POWER_MASK)
                | (rounded_power << 20),
        );
        let priority = u32::from(priority.value());
        self.words[SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY].set(
            (self.words[SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY].get()
                & !SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY_MASK)
                | (u32::from(channel.frequency_image()) << 8)
                | priority
                | (priority << 4),
        );
        self.words[SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION]
            .set(SCHEDULER_ITEM_RECEIVE_WAIT_SHORT_MODE | receive_wait.total_micros());
        self.words[SCHEDULER_ITEM_STATUS].set(0);
        self.words[SCHEDULER_ITEM_START].set(window.start());
        self.words[SCHEDULER_ITEM_END].set(window.end());
        self.words[SCHEDULER_ITEM_CLASS].set(self.words[SCHEDULER_ITEM_CLASS].get() & 0xffff_ff00);
    }

    #[cfg(test)]
    fn model_controller_status(&self, status: u32) {
        self.words[SCHEDULER_ITEM_STATUS].set(status);
    }
}

#[repr(C, align(4))]
struct BluetoothPeripheralConnectionTxSentinelStorage {
    words: [VolatileCell<u32>; TX_SENTINEL_WORDS],
}

impl BluetoothPeripheralConnectionTxSentinelStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; TX_SENTINEL_WORDS],
        }
    }

    fn initialize_empty(&self) {
        for word in &self.words {
            word.set(0);
        }
        self.words[TX_SENTINEL_STATE].set(TX_SENTINEL_EMPTY_QUEUE);
        self.words[TX_SENTINEL_CLASS].set(TX_SENTINEL_CONNECTION_CLASS);
    }

    fn is_empty_queue_sentinel(&self) -> bool {
        self.words[TX_SENTINEL_STATE].get() == TX_SENTINEL_EMPTY_QUEUE
            && self.words[TX_SENTINEL_CLASS].get() == TX_SENTINEL_CONNECTION_CLASS
    }
}

/// Static storage for the allocation-time graph of one peripheral connection.
#[pin_project]
#[repr(C)]
pub struct BluetoothPeripheralConnectionMemoryGraphStorage {
    link_state: BluetoothPeripheralConnectionLinkStateStorage,
    scheduler_context: BluetoothSchedulerContextStorage,
    scheduler_items: [BluetoothPeripheralConnectionSchedulerItemStorage;
        BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
    tx_sentinel: BluetoothPeripheralConnectionTxSentinelStorage,
    #[pin]
    _pin: PhantomPinned,
}

const GRAPH_BYTES: u32 =
    core::mem::size_of::<BluetoothPeripheralConnectionMemoryGraphStorage>() as u32;
const LINK_STATE_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPeripheralConnectionMemoryGraphStorage, link_state) as u32;
const SCHEDULER_CONTEXT_OFFSET: u32 = core::mem::offset_of!(
    BluetoothPeripheralConnectionMemoryGraphStorage,
    scheduler_context
) as u32;
const SCHEDULER_ITEMS_OFFSET: u32 = core::mem::offset_of!(
    BluetoothPeripheralConnectionMemoryGraphStorage,
    scheduler_items
) as u32;
const TX_SENTINEL_OFFSET: u32 =
    core::mem::offset_of!(BluetoothPeripheralConnectionMemoryGraphStorage, tx_sentinel) as u32;

/// Immutable controller-SRAM geometry for one exact pinned graph.
pub(super) struct BluetoothPeripheralConnectionMemoryGraphBinding {
    identity: BluetoothPeripheralConnectionMemoryGraphIdentity,
    link_state: BluetoothControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramLinkAddress,
    scheduler_items:
        [BluetoothControllerSramLinkAddress; BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
    tx_sentinel: BluetoothControllerSramLinkAddress,
}

pub(super) struct BluetoothPeripheralConnectionFirstEventCodecInput {
    pub(super) channel: BluetoothPeripheralConnectionDataChannel,
    pub(super) interval: BluetoothPeripheralConnectionIntervalTicks,
    pub(super) event_span: BluetoothPeripheralConnectionEventSpan,
    pub(super) window: BluetoothPeripheralConnectionSchedulerWindow,
    pub(super) receive_wait: BluetoothPeripheralConnectionReceiveWait,
    pub(super) default_tx_power: BluetoothPeripheralConnectionDefaultTxPowerDbm,
    pub(super) priority: BluetoothPeripheralConnectionSchedulerPriority,
}

impl BluetoothPeripheralConnectionMemoryGraphBinding {
    pub(super) fn new(
        identity: BluetoothPeripheralConnectionMemoryGraphIdentity,
        base: u32,
    ) -> Result<Self, BluetoothPeripheralConnectionMemoryGraphBindError> {
        BluetoothControllerSramAddress::new(base)
            .map_err(BluetoothPeripheralConnectionMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || GRAPH_BYTES > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(
                BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram,
            );
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let link = |offset: u32| {
            BluetoothControllerSramLinkAddress::new(address(offset)?)
                .map_err(|_| BluetoothPeripheralConnectionMemoryGraphBindError::ZeroCompressedLink)
        };
        let scheduler_item = |index: usize| {
            let index = u32::try_from(index).map_err(|_| {
                BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram
            })?;
            let offset = index
                .checked_mul(BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES as u32)
                .and_then(|offset| SCHEDULER_ITEMS_OFFSET.checked_add(offset))
                .ok_or(
                    BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram,
                )?;
            link(offset)
        };

        Ok(Self {
            identity,
            link_state: link(LINK_STATE_OFFSET)?,
            scheduler_context: link(SCHEDULER_CONTEXT_OFFSET)?,
            scheduler_items: [scheduler_item(0)?, scheduler_item(1)?],
            tx_sentinel: link(TX_SENTINEL_OFFSET)?,
        })
    }

    pub(super) const fn identity(&self) -> BluetoothPeripheralConnectionMemoryGraphIdentity {
        self.identity
    }

    pub(super) const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .controller_address()
    }
}

impl BluetoothPeripheralConnectionMemoryGraphStorage {
    pub const fn new() -> Self {
        Self {
            link_state: BluetoothPeripheralConnectionLinkStateStorage::new(),
            scheduler_context: BluetoothSchedulerContextStorage::new(),
            scheduler_items: [const { BluetoothPeripheralConnectionSchedulerItemStorage::new() };
                BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
            tx_sentinel: BluetoothPeripheralConnectionTxSentinelStorage::new(),
            _pin: PhantomPinned,
        }
    }

    pub(super) fn initialize_graph(
        self: Pin<&mut Self>,
        binding: &BluetoothPeripheralConnectionMemoryGraphBinding,
    ) {
        let graph = self.project();
        graph.scheduler_context.clear();
        graph.scheduler_items[0].initialize_allocation(
            None,
            binding.scheduler_context,
            binding.link_state,
        );
        graph.scheduler_items[1].initialize_allocation(
            Some(binding.scheduler_items[0]),
            binding.scheduler_context,
            binding.link_state,
        );
        graph
            .link_state
            .initialize_allocation(binding.scheduler_items[1], binding.tx_sentinel);
        graph.tx_sentinel.initialize_empty();
    }

    pub(super) fn has_empty_receive_queue(&self) -> bool {
        self.link_state.has_empty_receive_queue()
    }

    pub(super) fn has_empty_transmit_queue(
        &self,
        binding: &BluetoothPeripheralConnectionMemoryGraphBinding,
    ) -> bool {
        self.link_state
            .retains_transmit_sentinel(binding.tx_sentinel)
            && self.tx_sentinel.is_empty_queue_sentinel()
    }

    pub(super) fn has_recovered_scheduler_pool(
        &self,
        binding: &BluetoothPeripheralConnectionMemoryGraphBinding,
    ) -> bool {
        self.link_state
            .retains_scheduler_head(binding.scheduler_items[1])
            && self.scheduler_items[0].retains_allocation(
                None,
                binding.scheduler_context,
                binding.link_state,
            )
            && self.scheduler_items[1].retains_allocation(
                Some(binding.scheduler_items[0]),
                binding.scheduler_context,
                binding.link_state,
            )
    }

    pub(super) fn prepare_identity(&self, identity: BluetoothPeripheralConnectionIdentity) {
        self.link_state.prepare_identity(identity);
    }

    pub(super) fn identity(&self) -> BluetoothPeripheralConnectionIdentity {
        self.link_state.identity()
    }

    pub(super) fn install_receive_pool(
        &self,
        head: BluetoothControllerSramAddress,
        tail: BluetoothControllerSramAddress,
    ) {
        self.link_state.install_receive_pool(head, tail);
    }

    pub(super) fn clear_receive_pool(&self) {
        self.link_state.clear_receive_pool();
    }

    pub(super) fn prepare_reviewed_first_event_fields(
        &self,
        binding: &BluetoothPeripheralConnectionMemoryGraphBinding,
        receive_head: BluetoothControllerSramAddress,
        input: &BluetoothPeripheralConnectionFirstEventCodecInput,
    ) {
        self.link_state.prepare_event_profile(
            receive_head,
            binding.tx_sentinel,
            input.interval,
            input.event_span,
            input.default_tx_power,
            input.priority,
        );
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .prepare_reviewed_first_event_fields(
                self.link_state.rounded_power(),
                input.channel,
                input.window,
                input.receive_wait,
                input.priority,
            );
    }

    pub(super) fn install_direction_finding_workspace(
        &self,
        workspace: BluetoothDirectionFindingWorkspaceLink,
    ) {
        self.link_state
            .install_direction_finding_workspace(workspace);
    }

    pub(super) fn remove_direction_finding_workspace(&self) {
        self.link_state.remove_direction_finding_workspace();
    }

    pub(super) fn prepare_scheduler_admission(
        &self,
        binding: &BluetoothPeripheralConnectionMemoryGraphBinding,
    ) {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        self.scheduler_items[selected_index].detach_hardware_predecessor();
        self.scheduler_items[selected_index].mark_in_flight();
        self.link_state
            .install_scheduler_head(binding.scheduler_items[selected_index - 1]);
    }

    pub(super) fn restore_scheduler_admission(
        &self,
        binding: &BluetoothPeripheralConnectionMemoryGraphBinding,
    ) {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        self.scheduler_items[selected_index]
            .restore_hardware_predecessor(binding.scheduler_items[selected_index - 1]);
        self.scheduler_items[selected_index].restore_cpu_owned_status();
        self.link_state
            .install_scheduler_head(binding.scheduler_items[selected_index]);
    }

    pub(super) fn scheduler_completion_status(
        &self,
    ) -> Option<BluetoothPeripheralConnectionSchedulerItemCompletionStatus> {
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .completion_status()
    }

    pub(super) fn captured_anchor_time(&self) -> BluetoothPeripheralConnectionCapturedAnchorTime {
        self.link_state.captured_anchor_time()
    }

    #[cfg(test)]
    pub(super) fn model_controller_complete_event(
        &self,
        prepared_span: BluetoothPeripheralConnectionEventSpan,
        captured_anchor: BluetoothPeripheralConnectionCapturedAnchorTime,
        status: u32,
    ) -> bool {
        if !self
            .link_state
            .model_controller_complete_event(prepared_span, captured_anchor)
        {
            return false;
        }
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .model_controller_status(status);
        true
    }

    pub(super) fn event_resources_are_recycled(
        &self,
        binding: &BluetoothPeripheralConnectionMemoryGraphBinding,
    ) -> bool {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        self.link_state
            .retains_scheduler_head(binding.scheduler_items[selected_index])
            && self.scheduler_items[selected_index].retains_allocation(
                Some(binding.scheduler_items[selected_index - 1]),
                binding.scheduler_context,
                binding.link_state,
            )
    }

    #[cfg(test)]
    pub(super) fn model_controller_status(&self, status: u32) {
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .model_controller_status(status);
    }
}

impl Default for BluetoothPeripheralConnectionMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}
