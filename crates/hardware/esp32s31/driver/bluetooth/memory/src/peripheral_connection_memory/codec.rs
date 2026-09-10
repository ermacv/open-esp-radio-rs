//! Private SRAM layout and word codec for one peripheral connection graph.

use crate::le_tx_packet::{
    LeTxBufferHeaderStorage, LeTxPacketAddress, LeTxPacketPrepareError, LeTxPacketStorage,
};
use core::{cell::Cell, marker::PhantomPinned, pin::Pin};

use crate::{
    direction_finding_workspace::DirectionFindingWorkspaceLink,
    le_tx_power::rounded_tx_power,
    scheduler_context::SchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        ControllerSramLinkAddress,
    },
};

use oer_esp32s31_hal::types::BluetoothControllerSramAddress;

use pin_project::pin_project;

use super::{
    BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT,
    PeripheralConnectionCapturedAnchorAvailability, PeripheralConnectionCapturedAnchorTime,
    PeripheralConnectionDataChannel, PeripheralConnectionDefaultTxPowerDbm,
    PeripheralConnectionEventSpan, PeripheralConnectionIdentity,
    PeripheralConnectionMemoryGraphBindError, PeripheralConnectionMemoryGraphIdentity,
    PeripheralConnectionReceiveTime, PeripheralConnectionReceiveWait,
    PeripheralConnectionRecurringReceiveWait, PeripheralConnectionSchedulerItemCompletionStatus,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
};

use vcell::VolatileCell;

const LINK_STATE_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES / 4;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES / 4;
const CONTROL_TX_PACKET_BYTES: usize = crate::BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + 27;

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
const LINK_STATE_RECEIVE_TIME: usize = 0x18 / 4;
const LINK_STATE_PACKET_HISTORY: usize = 0x1c / 4;
const LINK_STATE_PACKET_CONTROL: usize = 0x20 / 4;
const LINK_STATE_PACKET_SEQUENCE: usize = 0x30 / 4;
const LINK_STATE_EVENT_SPAN: usize = 0x34 / 4;
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
const SCHEDULER_ITEM_SEQUENCE_START: usize = 0x0c / 4;
const SCHEDULER_ITEM_SEQUENCE_DURATION: usize = 0x10 / 4;
const SCHEDULER_ITEM_RATE_AND_POWER: usize = 0x14 / 4;
const SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY: usize = 0x18 / 4;
const SCHEDULER_ITEM_RADIO_REQUEST_PRIORITIES: usize = 0x24 / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS: usize = 0x1c / 4;
const SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION: usize = 0x2c / 4;
const SCHEDULER_ITEM_CAPTURED_ANCHOR: usize = 0x34 / 4;
const SCHEDULER_ITEM_STATUS: usize = 0x38 / 4;
const SCHEDULER_ITEM_START: usize = 0x44 / 4;
const SCHEDULER_ITEM_END: usize = 0x48 / 4;
const SCHEDULER_ITEM_LINK_MASK: u32 = 0x000f_ffff;
// Common allocation sets both bits; the connection role retains them.
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0030_0000;
// Standalone module default after common allocation and connection-role masks.
const SCHEDULER_ITEM_PERIPHERAL_ALLOCATION_FLAGS: u32 = 0xe7df_7fff;
// Product policy for the dedicated radio, not the vendor coexistence default.
const STANDALONE_RADIO_REQUEST_PRIORITY: u32 = 15;
const SCHEDULER_ITEM_PERIPHERAL_PREFIX: u32 = 0x0020_0000;
const SCHEDULER_ITEM_CONNECTION_CLASS: u32 = 3 << 8;
const SCHEDULER_ITEM_CONTEXT_READY: u32 = 1 << 31;
const SCHEDULER_ITEM_RATE_AND_POWER_MASK: u32 = 0xfff0_0000;
const SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY_MASK: u32 = 0x0000_7fff;
const SCHEDULER_ITEM_RECEIVE_WAIT_SHORT_MODE: u32 = 0x000f_0000;
const SCHEDULER_ITEM_RECEIVE_WAIT_LONG_MODE: u32 = 0x001f_0000;
const SCHEDULER_ITEM_RECEIVE_WAIT_ZERO_IMAGE: u32 = 1;
const SCHEDULER_ITEM_CAPTURE_AVAILABLE: u32 = 1 << 11;

#[repr(C, align(4))]
struct PeripheralConnectionLinkStateStorage {
    words: [VolatileCell<u32>; LINK_STATE_WORDS],
}

impl PeripheralConnectionLinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; LINK_STATE_WORDS],
        }
    }

    fn initialize_allocation(
        &self,
        scheduler_head: ControllerSramLinkAddress,
        tx_sentinel: ControllerSramLinkAddress,
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

    fn retains_transmit_sentinel(&self, sentinel: ControllerSramLinkAddress) -> bool {
        let address = sentinel.controller_address().address();
        self.words[LINK_STATE_TX_HEAD].get() == address
            && self.words[LINK_STATE_TX_TAIL].get() == address
    }

    fn retains_scheduler_head(&self, head: ControllerSramLinkAddress) -> bool {
        self.words[LINK_STATE_SCHEDULER_HEAD].get() == head.controller_address().address()
    }

    fn install_scheduler_head(&self, head: ControllerSramLinkAddress) {
        self.words[LINK_STATE_SCHEDULER_HEAD].set(head.controller_address().address());
    }

    fn prepare_identity(&self, identity: PeripheralConnectionIdentity) {
        self.words[LINK_STATE_CRC_INITIALIZATION]
            .set(u32::from_le_bytes(identity.crc_initialization_word()));
        self.words[LINK_STATE_ACCESS_ADDRESS]
            .set(u32::from_le_bytes(identity.access_address_wire_bytes()));
    }

    fn prepare_event_profile(
        &self,
        receive_head: BluetoothControllerSramAddress,
        transmit_sentinel: ControllerSramLinkAddress,
        receive_time: PeripheralConnectionReceiveTime,
        event_span: PeripheralConnectionEventSpan,
        default_tx_power: PeripheralConnectionDefaultTxPowerDbm,
        priority: PeripheralConnectionSchedulerPriority,
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
        self.words[LINK_STATE_RECEIVE_TIME].set(receive_time.wrapping_controller_ticks());
        self.words[LINK_STATE_EVENT_SPAN].set(event_span.ticks());
    }

    fn receive_time(&self) -> PeripheralConnectionReceiveTime {
        PeripheralConnectionReceiveTime::from_controller_ticks(
            self.words[LINK_STATE_RECEIVE_TIME].get(),
        )
    }

    fn prepare_recurring_event_profile(
        &self,
        event_span: PeripheralConnectionEventSpan,
        priority: PeripheralConnectionSchedulerPriority,
    ) {
        self.words[LINK_STATE_EVENT_PRIORITY].set(u32::from(priority.value()));
        self.words[LINK_STATE_EVENT_SPAN].set(event_span.ticks());
    }

    #[cfg(test)]
    fn retains_event_span(&self, prepared_span: PeripheralConnectionEventSpan) -> bool {
        self.words[LINK_STATE_EVENT_SPAN].get() == prepared_span.ticks()
    }

    fn install_direction_finding_workspace(&self, workspace: DirectionFindingWorkspaceLink) {
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

    fn identity(&self) -> PeripheralConnectionIdentity {
        let crc_initialization = self.words[LINK_STATE_CRC_INITIALIZATION]
            .get()
            .to_le_bytes();
        PeripheralConnectionIdentity::new(
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
struct PeripheralConnectionSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

#[derive(Clone, Copy)]
pub(super) struct PeripheralConnectionSchedulerCompletionObservation {
    status: PeripheralConnectionSchedulerItemCompletionStatus,
    capture_available: bool,
}

impl PeripheralConnectionSchedulerCompletionObservation {
    pub(super) const fn status(self) -> PeripheralConnectionSchedulerItemCompletionStatus {
        self.status
    }
}

impl PeripheralConnectionSchedulerItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    fn initialize_allocation(
        &self,
        successor: Option<ControllerSramLinkAddress>,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        let successor = successor.map_or(0, ControllerSramLinkAddress::compressed_image);
        self.words[SCHEDULER_ITEM_NEXT].set(SCHEDULER_ITEM_ALLOCATION_PREFIX | successor);
        self.words[SCHEDULER_ITEM_ALLOCATION_FLAGS].set(SCHEDULER_ITEM_PERIPHERAL_ALLOCATION_FLAGS);
        self.words[SCHEDULER_ITEM_RADIO_REQUEST_PRIORITIES]
            .set(STANDALONE_RADIO_REQUEST_PRIORITY | (STANDALONE_RADIO_REQUEST_PRIORITY << 5));
        self.words[SCHEDULER_ITEM_CONTEXT].set(scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE]
            .set(SCHEDULER_ITEM_PERIPHERAL_PREFIX | link_state.compressed_image());
        self.words[SCHEDULER_ITEM_CLASS].set(SCHEDULER_ITEM_CONNECTION_CLASS);
    }

    fn retains_allocation(
        &self,
        successor: Option<ControllerSramLinkAddress>,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
    ) -> bool {
        let successor = successor.map_or(0, ControllerSramLinkAddress::compressed_image);
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

    fn restore_hardware_predecessor(&self, predecessor: ControllerSramLinkAddress) {
        self.words[SCHEDULER_ITEM_NEXT]
            .set(SCHEDULER_ITEM_ALLOCATION_PREFIX | predecessor.compressed_image());
    }

    // Current r_btdm_sched_calc_seq_time: the sequencer starts after the
    // admitted lead and retains the complete software-window duration.
    fn prepare_sequence_timing(
        &self,
        window: PeripheralConnectionSchedulerWindow,
        raw_sequence_lead: u32,
    ) {
        self.words[SCHEDULER_ITEM_SEQUENCE_START]
            .set(window.start().wrapping_add(raw_sequence_lead));
        self.words[SCHEDULER_ITEM_SEQUENCE_DURATION].set(window.end().wrapping_sub(window.start()));
    }

    fn mark_in_flight(&self) {
        self.words[SCHEDULER_ITEM_STATUS].set(u32::MAX);
    }

    fn restore_cpu_owned_status(&self) {
        self.words[SCHEDULER_ITEM_STATUS].set(0);
    }

    fn completion_observation(&self) -> Option<PeripheralConnectionSchedulerCompletionObservation> {
        match self.words[SCHEDULER_ITEM_STATUS].get() {
            u32::MAX => None,
            status => Some(PeripheralConnectionSchedulerCompletionObservation {
                status: if status == 0 {
                    PeripheralConnectionSchedulerItemCompletionStatus::Zero
                } else {
                    PeripheralConnectionSchedulerItemCompletionStatus::NonZero
                },
                capture_available: status & SCHEDULER_ITEM_CAPTURE_AVAILABLE != 0,
            }),
        }
    }

    fn captured_anchor_availability(
        &self,
        completion: PeripheralConnectionSchedulerCompletionObservation,
    ) -> PeripheralConnectionCapturedAnchorAvailability {
        if completion.capture_available {
            PeripheralConnectionCapturedAnchorAvailability::Available(
                PeripheralConnectionCapturedAnchorTime::from_controller_sram_word(
                    self.words[SCHEDULER_ITEM_CAPTURED_ANCHOR].get(),
                ),
            )
        } else {
            PeripheralConnectionCapturedAnchorAvailability::Absent
        }
    }

    fn prepare_reviewed_first_event_fields(
        &self,
        rounded_power: u32,
        channel: PeripheralConnectionDataChannel,
        window: PeripheralConnectionSchedulerWindow,
        receive_wait: PeripheralConnectionReceiveWait,
        priority: PeripheralConnectionSchedulerPriority,
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

    fn prepare_reviewed_recurring_event_fields(
        &self,
        channel: PeripheralConnectionDataChannel,
        window: PeripheralConnectionSchedulerWindow,
        receive_wait: PeripheralConnectionRecurringReceiveWait,
        priority: PeripheralConnectionSchedulerPriority,
    ) {
        self.words[SCHEDULER_ITEM_CONTEXT_STATE]
            .set(self.words[SCHEDULER_ITEM_CONTEXT_STATE].get() | SCHEDULER_ITEM_CONTEXT_READY);
        let priority = u32::from(priority.value());
        self.words[SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY].set(
            (self.words[SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY].get()
                & !SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY_MASK)
                | (u32::from(channel.frequency_image()) << 8)
                | priority
                | (priority << 4),
        );
        let total_micros = receive_wait.total_micros();
        let receive_wait_image = if total_micros == 0 {
            SCHEDULER_ITEM_RECEIVE_WAIT_ZERO_IMAGE
        } else if total_micros < u32::from(u16::MAX) {
            SCHEDULER_ITEM_RECEIVE_WAIT_SHORT_MODE | total_micros
        } else {
            SCHEDULER_ITEM_RECEIVE_WAIT_LONG_MODE | (total_micros >> 1)
        };
        self.words[SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION].set(receive_wait_image);
        self.words[SCHEDULER_ITEM_STATUS].set(0);
        self.words[SCHEDULER_ITEM_START].set(window.start());
        self.words[SCHEDULER_ITEM_END].set(window.end());
        self.words[SCHEDULER_ITEM_CLASS].set(self.words[SCHEDULER_ITEM_CLASS].get() & 0xffff_ff00);
    }

    #[cfg(test)]
    fn model_controller_completion(
        &self,
        status: PeripheralConnectionSchedulerItemCompletionStatus,
        capture: PeripheralConnectionCapturedAnchorAvailability,
    ) -> bool {
        let status = match (status, capture) {
            (
                PeripheralConnectionSchedulerItemCompletionStatus::Zero,
                PeripheralConnectionCapturedAnchorAvailability::Absent,
            ) => 0,
            (
                PeripheralConnectionSchedulerItemCompletionStatus::NonZero,
                PeripheralConnectionCapturedAnchorAvailability::Absent,
            ) => 1,
            (
                PeripheralConnectionSchedulerItemCompletionStatus::NonZero,
                PeripheralConnectionCapturedAnchorAvailability::Available(captured),
            ) => {
                self.words[SCHEDULER_ITEM_CAPTURED_ANCHOR]
                    .set(captured.wrapping_controller_ticks());
                SCHEDULER_ITEM_CAPTURE_AVAILABLE
            }
            (
                PeripheralConnectionSchedulerItemCompletionStatus::Zero,
                PeripheralConnectionCapturedAnchorAvailability::Available(_),
            ) => return false,
        };
        self.words[SCHEDULER_ITEM_STATUS].set(status);
        true
    }
}

/// Static storage for the allocation-time graph of one peripheral connection.
#[pin_project]
#[repr(C)]
pub struct PeripheralConnectionMemoryGraphStorage {
    link_state: PeripheralConnectionLinkStateStorage,
    scheduler_context: SchedulerContextStorage,
    scheduler_items: [PeripheralConnectionSchedulerItemStorage;
        BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
    tx_sentinel: LeTxBufferHeaderStorage,
    tx_successor: LeTxBufferHeaderStorage,
    tx_packet: LeTxPacketStorage<CONTROL_TX_PACKET_BYTES>,
    tx_current_second: Cell<bool>,
    tx_pending: Cell<bool>,
    #[pin]
    _pin: PhantomPinned,
}

const GRAPH_BYTES: u32 = core::mem::size_of::<PeripheralConnectionMemoryGraphStorage>() as u32;
const LINK_STATE_OFFSET: u32 =
    core::mem::offset_of!(PeripheralConnectionMemoryGraphStorage, link_state) as u32;
const SCHEDULER_CONTEXT_OFFSET: u32 =
    core::mem::offset_of!(PeripheralConnectionMemoryGraphStorage, scheduler_context) as u32;
const SCHEDULER_ITEMS_OFFSET: u32 =
    core::mem::offset_of!(PeripheralConnectionMemoryGraphStorage, scheduler_items) as u32;
const TX_SENTINEL_OFFSET: u32 =
    core::mem::offset_of!(PeripheralConnectionMemoryGraphStorage, tx_sentinel) as u32;

/// Immutable controller-SRAM geometry for one exact pinned graph.
pub(super) struct PeripheralConnectionMemoryGraphBinding {
    identity: PeripheralConnectionMemoryGraphIdentity,
    link_state: ControllerSramLinkAddress,
    scheduler_context: ControllerSramLinkAddress,
    scheduler_items:
        [ControllerSramLinkAddress; BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
    tx_sentinel: ControllerSramLinkAddress,
}

pub(super) struct PeripheralConnectionFirstEventCodecInput {
    pub(super) channel: PeripheralConnectionDataChannel,
    pub(super) receive_time: PeripheralConnectionReceiveTime,
    pub(super) event_span: PeripheralConnectionEventSpan,
    pub(super) window: PeripheralConnectionSchedulerWindow,
    pub(super) receive_wait: PeripheralConnectionReceiveWait,
    pub(super) default_tx_power: PeripheralConnectionDefaultTxPowerDbm,
    pub(super) priority: PeripheralConnectionSchedulerPriority,
    pub(super) raw_sequence_lead: u32,
}

pub(super) struct PeripheralConnectionRecurringEventCodecInput {
    pub(super) channel: PeripheralConnectionDataChannel,
    pub(super) event_span: PeripheralConnectionEventSpan,
    pub(super) window: PeripheralConnectionSchedulerWindow,
    pub(super) receive_wait: PeripheralConnectionRecurringReceiveWait,
    pub(super) priority: PeripheralConnectionSchedulerPriority,
    pub(super) raw_sequence_lead: u32,
}

impl PeripheralConnectionMemoryGraphBinding {
    pub(super) fn new(
        identity: PeripheralConnectionMemoryGraphIdentity,
        base: u32,
    ) -> Result<Self, PeripheralConnectionMemoryGraphBindError> {
        BluetoothControllerSramAddress::new(base)
            .map_err(PeripheralConnectionMemoryGraphBindError::InvalidBase)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || GRAPH_BYTES > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(PeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram);
        }

        let address = |offset: u32| {
            base.checked_add(offset)
                .ok_or(PeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram)
        };
        let link = |offset: u32| {
            ControllerSramLinkAddress::new(address(offset)?)
                .map_err(|_| PeripheralConnectionMemoryGraphBindError::ZeroCompressedLink)
        };
        let scheduler_item = |index: usize| {
            let index = u32::try_from(index)
                .map_err(|_| PeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram)?;
            let offset = index
                .checked_mul(BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES as u32)
                .and_then(|offset| SCHEDULER_ITEMS_OFFSET.checked_add(offset))
                .ok_or(PeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram)?;
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

    fn tx_successor(&self) -> ControllerSramLinkAddress {
        ControllerSramLinkAddress::new(
            self.link_state.controller_address().address() - LINK_STATE_OFFSET
                + core::mem::offset_of!(PeripheralConnectionMemoryGraphStorage, tx_successor)
                    as u32,
        )
        .expect("the whole pinned graph extent was validated")
    }

    fn tx_packet(&self) -> LeTxPacketAddress<CONTROL_TX_PACKET_BYTES> {
        LeTxPacketAddress::new(
            self.link_state.controller_address().address() - LINK_STATE_OFFSET
                + core::mem::offset_of!(PeripheralConnectionMemoryGraphStorage, tx_packet) as u32,
        )
        .expect("the whole pinned graph extent was validated")
    }

    pub(super) const fn identity(&self) -> PeripheralConnectionMemoryGraphIdentity {
        self.identity
    }

    pub(super) const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .controller_address()
    }
}

impl PeripheralConnectionMemoryGraphStorage {
    #[cfg(test)]
    pub(super) fn model_controller_valid_receive(&self, time: PeripheralConnectionReceiveTime) {
        self.link_state.words[LINK_STATE_RECEIVE_TIME].set(time.wrapping_controller_ticks());
    }

    pub(super) fn receive_time(&self) -> PeripheralConnectionReceiveTime {
        self.link_state.receive_time()
    }

    pub const fn new() -> Self {
        Self {
            link_state: PeripheralConnectionLinkStateStorage::new(),
            scheduler_context: SchedulerContextStorage::new(),
            scheduler_items: [const { PeripheralConnectionSchedulerItemStorage::new() };
                BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
            tx_sentinel: LeTxBufferHeaderStorage::new(),
            tx_successor: LeTxBufferHeaderStorage::new(),
            tx_packet: LeTxPacketStorage::new(),
            tx_current_second: Cell::new(false),
            tx_pending: Cell::new(false),
            _pin: PhantomPinned,
        }
    }

    pub(super) fn initialize_graph(
        self: Pin<&mut Self>,
        binding: &PeripheralConnectionMemoryGraphBinding,
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
        graph.tx_sentinel.initialize_empty_cursor();
        graph.tx_successor.initialize_empty_cursor();
        graph.tx_packet.clear();
        graph.tx_current_second.set(false);
        graph.tx_pending.set(false);
    }

    pub(super) fn has_empty_receive_queue(&self) -> bool {
        self.link_state.has_empty_receive_queue()
    }

    /// Model the sequencer's deadline using only its encoded hardware inputs.
    #[cfg(test)]
    pub(super) fn model_controller_sequence_elapsed(&self, now: u32) -> bool {
        let item = &self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1];
        let start = item.words[SCHEDULER_ITEM_SEQUENCE_START].get();
        let duration = item.words[SCHEDULER_ITEM_SEQUENCE_DURATION].get();
        let elapsed = now.wrapping_sub(start) as i32;
        elapsed >= 0 && elapsed as u32 >= duration
    }

    /// Model the two peripheral arbitration requests consumed by the controller.
    #[cfg(test)]
    pub(super) fn model_controller_can_request_radio(&self) -> bool {
        let item = &self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1];
        let priorities = item.words[SCHEDULER_ITEM_RADIO_REQUEST_PRIORITIES].get();
        (0..2).all(|phase| (priorities >> (phase * 5)) & 31 != 0)
    }

    pub(super) fn has_empty_transmit_queue(
        &self,
        binding: &PeripheralConnectionMemoryGraphBinding,
    ) -> bool {
        self.link_state
            .retains_transmit_sentinel(binding.tx_sentinel)
            && self.tx_sentinel.is_empty_cursor()
    }

    /// Called only by the reclaimed live graph, never while a RUN owns SRAM.
    pub(super) fn reclaim_control_tx(
        &self,
        binding: &PeripheralConnectionMemoryGraphBinding,
    ) -> bool {
        if !self.tx_pending.get() {
            return false;
        }
        let (header, address) = if self.tx_current_second.get() {
            (&self.tx_sentinel, binding.tx_sentinel)
        } else {
            (&self.tx_successor, binding.tx_successor())
        };
        if !header.transmission_completed() {
            return false;
        }
        // The completed node remains the hardware current cursor. Only its
        // packet allocation is released, matching get_txed_buffer's tail case.
        header.release_completed_packet();
        self.link_state.words[LINK_STATE_TX_HEAD].set(address.controller_address().address());
        self.tx_current_second.set(!self.tx_current_second.get());
        self.tx_pending.set(false);
        true
    }

    pub(super) fn enqueue_control_tx(
        self: Pin<&mut Self>,
        binding: &PeripheralConnectionMemoryGraphBinding,
        payload: &[u8],
    ) -> Result<bool, LeTxPacketPrepareError> {
        if self.tx_pending.get() {
            return Ok(false);
        }
        let graph = self.project();
        graph.tx_packet.prepare_pdu(3, payload)?;
        let (current, next, next_address) = if graph.tx_current_second.get() {
            (graph.tx_successor, graph.tx_sentinel, binding.tx_sentinel)
        } else {
            (
                graph.tx_sentinel,
                graph.tx_successor,
                binding.tx_successor(),
            )
        };
        next.initialize_bound_tx(binding.tx_packet());
        next.mark_complete_control_packet();
        current.link_successor(next_address);
        graph.link_state.words[LINK_STATE_TX_TAIL].set(next_address.controller_address().address());
        graph.tx_pending.set(true);
        Ok(true)
    }

    #[cfg(test)]
    pub(super) fn model_transmit_control(
        &self,
        binding: &PeripheralConnectionMemoryGraphBinding,
        acknowledged: bool,
    ) -> Option<std::vec::Vec<u8>> {
        // Walk the actual published current/successor links. The peer ACK,
        // rather than an event completion, authorizes descriptor completion.
        let current = self.link_state.words[LINK_STATE_TX_PATH].get() & SCHEDULER_ITEM_LINK_MASK;
        let headers = [
            (&self.tx_sentinel, binding.tx_sentinel),
            (&self.tx_successor, binding.tx_successor()),
        ];
        let (cursor, _) = headers
            .iter()
            .find(|(_, address)| address.compressed_image() == current)?;
        let next = cursor.snapshot()[0] & SCHEDULER_ITEM_LINK_MASK;
        let (header, address) = headers
            .iter()
            .find(|(_, address)| address.compressed_image() == next)?;
        if header.transmission_completed()
            || header.packet_base_link() != Some(binding.tx_packet().base_link())
        {
            return None;
        }
        let pdu = self.tx_packet.model_pdu().to_vec();
        if acknowledged {
            header.model_complete_transmission();
            let path = &self.link_state.words[LINK_STATE_TX_PATH];
            path.set((path.get() & !SCHEDULER_ITEM_LINK_MASK) | address.compressed_image());
        }
        Some(pdu)
    }

    #[cfg(test)]
    pub(super) fn model_receive_current(&self) -> BluetoothControllerSramAddress {
        let compressed = self.link_state.words[LINK_STATE_RX_PATH].get() & SCHEDULER_ITEM_LINK_MASK;
        BluetoothControllerSramAddress::new(
            BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW | (compressed << 2),
        )
        .unwrap()
    }

    #[cfg(test)]
    pub(super) fn model_advance_receive_current(&self, current: BluetoothControllerSramAddress) {
        let path = &self.link_state.words[LINK_STATE_RX_PATH];
        path.set((path.get() & !SCHEDULER_ITEM_LINK_MASK) | current.compressed_image());
    }

    pub(super) fn has_recovered_scheduler_pool(
        &self,
        binding: &PeripheralConnectionMemoryGraphBinding,
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

    pub(super) fn prepare_identity(&self, identity: PeripheralConnectionIdentity) {
        self.link_state.prepare_identity(identity);
    }

    pub(super) fn identity(&self) -> PeripheralConnectionIdentity {
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
        binding: &PeripheralConnectionMemoryGraphBinding,
        receive_head: BluetoothControllerSramAddress,
        input: &PeripheralConnectionFirstEventCodecInput,
    ) {
        self.link_state.prepare_event_profile(
            receive_head,
            binding.tx_sentinel,
            input.receive_time,
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
        let item = &self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1];
        item.prepare_sequence_timing(input.window, input.raw_sequence_lead);
    }

    pub(super) fn prepare_reviewed_recurring_event_fields(
        &self,
        input: &PeripheralConnectionRecurringEventCodecInput,
    ) {
        self.link_state
            .prepare_recurring_event_profile(input.event_span, input.priority);
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .prepare_reviewed_recurring_event_fields(
                input.channel,
                input.window,
                input.receive_wait,
                input.priority,
            );
        let item = &self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1];
        item.prepare_sequence_timing(input.window, input.raw_sequence_lead);
    }

    pub(super) fn install_direction_finding_workspace(
        &self,
        workspace: DirectionFindingWorkspaceLink,
    ) {
        self.link_state
            .install_direction_finding_workspace(workspace);
    }

    pub(super) fn remove_direction_finding_workspace(&self) {
        self.link_state.remove_direction_finding_workspace();
    }

    pub(super) fn prepare_scheduler_admission(
        &self,
        binding: &PeripheralConnectionMemoryGraphBinding,
    ) {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        self.scheduler_items[selected_index].detach_hardware_predecessor();
        self.scheduler_items[selected_index].mark_in_flight();
        self.link_state
            .install_scheduler_head(binding.scheduler_items[selected_index - 1]);
    }

    pub(super) fn restore_scheduler_admission(
        &self,
        binding: &PeripheralConnectionMemoryGraphBinding,
    ) {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        self.scheduler_items[selected_index]
            .restore_hardware_predecessor(binding.scheduler_items[selected_index - 1]);
        self.scheduler_items[selected_index].restore_cpu_owned_status();
        self.link_state
            .install_scheduler_head(binding.scheduler_items[selected_index]);
    }

    pub(super) fn scheduler_completion_observation(
        &self,
    ) -> Option<PeripheralConnectionSchedulerCompletionObservation> {
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .completion_observation()
    }

    pub(super) fn captured_anchor_availability(
        &self,
        completion: PeripheralConnectionSchedulerCompletionObservation,
    ) -> PeripheralConnectionCapturedAnchorAvailability {
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .captured_anchor_availability(completion)
    }

    #[cfg(test)]
    pub(super) fn model_controller_complete_event(
        &self,
        prepared_span: PeripheralConnectionEventSpan,
        status: PeripheralConnectionSchedulerItemCompletionStatus,
        capture: PeripheralConnectionCapturedAnchorAvailability,
    ) -> bool {
        if !self.link_state.retains_event_span(prepared_span) {
            return false;
        }
        self.scheduler_items[BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1]
            .model_controller_completion(status, capture)
    }

    pub(super) fn event_resources_are_recycled(
        &self,
        binding: &PeripheralConnectionMemoryGraphBinding,
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
}

impl Default for PeripheralConnectionMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}
