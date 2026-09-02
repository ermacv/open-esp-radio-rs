//! Pinned allocation graph for one ESP32-S31 BLE peripheral connection.
//!
//! This is the stable memory boundary recovered from the current controller
//! artifact.  It owns the two reusable scheduler items, their shared context,
//! the connection link state and the initially empty transmit queue sentinel.
//! A separately owned static non-scanning RX pool can be attached for an exact
//! event and recovered on cancellation. A later affine transition joins the
//! controller-global direction-finding workspace before the graph can approach
//! scheduler publication.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListSelector, BluetoothRxMemoryListPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
};
use pin_project::pin_project;
use vcell::VolatileCell;

use crate::{
    direction_finding_workspace::BluetoothDirectionFindingWorkspaceLink,
    le_tx_power::rounded_tx_power,
    non_scanning_rx_memory::BluetoothNonScanningRxMemoryCpuOwned,
    rx_memory_list::BluetoothRxMemoryListClass,
    scheduler_context::BluetoothSchedulerContextStorage,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        BluetoothControllerSramLinkAddress,
    },
};

/// Bytes retained by one connection link-state allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES: usize = 0x84;
/// Bytes retained by one connection scheduler-item allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Scheduler items retained by one connection allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT: usize = 2;
/// Bytes retained by the initially empty transmit queue sentinel.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES: usize = 0x18;

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

/// Air-interface identity consumed by the S31 connection link state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionIdentity {
    access_address: [u8; 4],
    crc_initialization: [u8; 3],
}

impl BluetoothPeripheralConnectionIdentity {
    /// Construct the exact two fields in over-the-air little-endian order.
    pub const fn new(access_address: [u8; 4], crc_initialization: [u8; 3]) -> Self {
        Self {
            access_address,
            crc_initialization,
        }
    }

    /// Access Address octets in Link Layer wire order.
    pub const fn access_address_wire_bytes(self) -> [u8; 4] {
        self.access_address
    }

    /// CRCInit octets in Link Layer wire order.
    pub const fn crc_initialization_wire_bytes(self) -> [u8; 3] {
        self.crc_initialization
    }

    const fn crc_initialization_word(self) -> [u8; 4] {
        [
            self.crc_initialization[0],
            self.crc_initialization[1],
            self.crc_initialization[2],
            0,
        ]
    }
}

/// One validated LE data channel projected into the S31 frequency table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionDataChannel {
    index: u8,
    frequency_image: u8,
}

impl BluetoothPeripheralConnectionDataChannel {
    /// Bind one of the 37 Link Layer data-channel indices.
    pub const fn new(index: u8) -> Option<Self> {
        if index >= 37 {
            return None;
        }
        let frequency_image = if index <= 10 {
            (index + 1) * 2
        } else {
            (index + 2) * 2
        };
        Some(Self {
            index,
            frequency_image,
        })
    }

    pub const fn index(self) -> u8 {
        self.index
    }

    const fn frequency_image(self) -> u8 {
        self.frequency_image
    }
}

/// Non-empty raw Controller interval between connection events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionIntervalTicks(u32);

impl BluetoothPeripheralConnectionIntervalTicks {
    pub const fn new(ticks: u32) -> Option<Self> {
        if ticks == 0 { None } else { Some(Self(ticks)) }
    }

    const fn ticks(self) -> u32 {
        self.0
    }
}

/// Non-empty raw Controller window for one connection scheduler item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionSchedulerWindow {
    start: u32,
    end: u32,
}

impl BluetoothPeripheralConnectionSchedulerWindow {
    pub const fn new(start: u32, end: u32) -> Option<Self> {
        let duration = end.wrapping_sub(start);
        if duration == 0 || duration > i32::MAX as u32 {
            None
        } else {
            Some(Self { start, end })
        }
    }

    const fn start(self) -> u32 {
        self.start
    }

    const fn end(self) -> u32 {
        self.end
    }
}

/// Bounded first-event receive wait expressed only in physical time.
///
/// The controller-memory codec owns the positional duration/mode encoding.
/// Callers provide the accepted transmit-window width and the symmetric timing
/// uncertainty which surrounds it; they cannot construct a descriptor word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionReceiveWait {
    transmit_window_micros: u32,
    timing_guard_micros: u32,
    total_micros: u16,
}

impl BluetoothPeripheralConnectionReceiveWait {
    /// Form the complete first-event receive wait.
    ///
    /// The extra 61 microseconds are a fixed S31 PHY allowance recovered from
    /// the complete connection-event builder. This constructor admits only the
    /// short hardware form used by every valid legacy first transmit window.
    pub const fn new(transmit_window_micros: u32, timing_guard_micros: u32) -> Option<Self> {
        let Some(double_guard) = timing_guard_micros.checked_mul(2) else {
            return None;
        };
        let Some(guarded_window_micros) = transmit_window_micros.checked_add(double_guard) else {
            return None;
        };
        let Some(total_micros) = guarded_window_micros.checked_add(61) else {
            return None;
        };
        if transmit_window_micros == 0 || total_micros > 0xfffe {
            return None;
        }
        Some(Self {
            transmit_window_micros,
            timing_guard_micros,
            total_micros: total_micros as u16,
        })
    }

    pub const fn transmit_window_micros(self) -> u32 {
        self.transmit_window_micros
    }

    pub const fn timing_guard_micros(self) -> u32 {
        self.timing_guard_micros
    }

    pub const fn total_micros(self) -> u32 {
        self.total_micros as u32
    }

    const fn descriptor_image(self) -> u32 {
        SCHEDULER_ITEM_RECEIVE_WAIT_SHORT_MODE | self.total_micros as u32
    }
}

/// Physical default transmit-power request for the first connection profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionDefaultTxPowerDbm(i8);

impl BluetoothPeripheralConnectionDefaultTxPowerDbm {
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    pub const fn dbm(self) -> i8 {
        self.0
    }
}

/// Source-owned first-event priority shared by connection state and scheduler item.
///
/// The retained default Controller options select 13. Conflict handling then
/// increases the value and saturates at 15; the later recurring-event reset to
/// 8 is deliberately outside this first-event value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionSchedulerPriority(u8);

impl BluetoothPeripheralConnectionSchedulerPriority {
    /// Priority selected by the reviewed ESP32-S31 first-event policy.
    pub const FIRST_EVENT: Self = Self(13);

    pub const fn value(self) -> u8 {
        self.0
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
        self.words[SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION].set(receive_wait.descriptor_image());
        self.words[SCHEDULER_ITEM_STATUS].set(0);
        self.words[SCHEDULER_ITEM_START].set(window.start());
        self.words[SCHEDULER_ITEM_END].set(window.end());
        self.words[SCHEDULER_ITEM_CLASS].set(self.words[SCHEDULER_ITEM_CLASS].get() & 0xffff_ff00);
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

/// Why peripheral-connection storage cannot become a bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPeripheralConnectionMemoryGraphBindError {
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
}

/// Failed binding that returns the exact unchanged static allocation.
pub struct BluetoothPeripheralConnectionMemoryGraphBindFailure {
    storage: &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
    error: BluetoothPeripheralConnectionMemoryGraphBindError,
}

impl BluetoothPeripheralConnectionMemoryGraphBindFailure {
    fn new(
        storage: &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
        error: BluetoothPeripheralConnectionMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> BluetoothPeripheralConnectionMemoryGraphBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothPeripheralConnectionMemoryGraphStorage,
        BluetoothPeripheralConnectionMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothPeripheralConnectionMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothPeripheralConnectionMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothPeripheralConnectionMemoryGraphModelAddress {
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

/// Immutable geometry retained privately by the connection memory codec.
struct BluetoothPeripheralConnectionMemoryGraphBinding {
    identity: BluetoothPeripheralConnectionMemoryGraphIdentity,
    link_state: BluetoothControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramLinkAddress,
    scheduler_items:
        [BluetoothControllerSramLinkAddress; BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT],
    tx_sentinel: BluetoothControllerSramLinkAddress,
}

/// Opaque identity of one exact statically pinned connection graph.
///
/// This is only an equality witness. It exposes neither its storage pointer
/// nor any controller-SRAM address and grants no memory or publication access.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct BluetoothPeripheralConnectionMemoryGraphIdentity(usize);

impl BluetoothPeripheralConnectionMemoryGraphIdentity {
    fn for_storage(storage: &BluetoothPeripheralConnectionMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for BluetoothPeripheralConnectionMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothPeripheralConnectionMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

impl BluetoothPeripheralConnectionMemoryGraphBinding {
    fn new(
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

    const fn identity(&self) -> BluetoothPeripheralConnectionMemoryGraphIdentity {
        self.identity
    }
}

/// Unique CPU owner of one allocation-time peripheral-connection graph.
#[must_use = "the bound peripheral-connection graph must be retained"]
pub struct BluetoothPeripheralConnectionMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothPeripheralConnectionMemoryGraphStorage>,
    binding: BluetoothPeripheralConnectionMemoryGraphBinding,
}

impl BluetoothPeripheralConnectionMemoryGraphCpuOwned {
    /// Equality witness for the exact pinned storage object.
    pub const fn identity(&self) -> BluetoothPeripheralConnectionMemoryGraphIdentity {
        self.binding.identity()
    }

    /// The recovered allocation starts without any receive buffer owner.
    pub fn has_empty_receive_queue(&self) -> bool {
        self.storage
            .as_ref()
            .get_ref()
            .link_state
            .has_empty_receive_queue()
    }

    /// The recovered allocation starts with one shared head/tail TX sentinel.
    pub fn has_empty_transmit_queue(&self) -> bool {
        let graph = self.storage.as_ref().get_ref();
        graph
            .link_state
            .retains_transmit_sentinel(self.binding.tx_sentinel)
            && graph.tx_sentinel.is_empty_queue_sentinel()
    }

    fn reinitialize_graph(&mut self) {
        let graph = self.storage.as_mut().project();
        graph.scheduler_context.clear();
        graph.scheduler_items[0].initialize_allocation(
            None,
            self.binding.scheduler_context,
            self.binding.link_state,
        );
        graph.scheduler_items[1].initialize_allocation(
            Some(self.binding.scheduler_items[0]),
            self.binding.scheduler_context,
            self.binding.link_state,
        );
        graph
            .link_state
            .initialize_allocation(self.binding.scheduler_items[1], self.binding.tx_sentinel);
        graph.tx_sentinel.initialize_empty();
    }

    /// Both reusable scheduler items still form the recovered private pool.
    pub fn has_recovered_scheduler_pool(&self) -> bool {
        let graph = self.storage.as_ref().get_ref();
        graph
            .link_state
            .retains_scheduler_head(self.binding.scheduler_items[1])
            && graph.scheduler_items[0].retains_allocation(
                None,
                self.binding.scheduler_context,
                self.binding.link_state,
            )
            && graph.scheduler_items[1].retains_allocation(
                Some(self.binding.scheduler_items[0]),
                self.binding.scheduler_context,
                self.binding.link_state,
            )
    }

    /// Install only the reviewed connection identity fields.
    ///
    /// This state cannot publish a scheduler item. A later event builder must
    /// consume it after closing the anchor, duration and packet sequence
    /// semantics.
    pub fn prepare_identity(
        self,
        identity: BluetoothPeripheralConnectionIdentity,
    ) -> BluetoothPeripheralConnectionMemoryGraphIdentityPrepared {
        self.storage
            .as_ref()
            .get_ref()
            .link_state
            .prepare_identity(identity);
        BluetoothPeripheralConnectionMemoryGraphIdentityPrepared {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// CPU-owned graph with Access Address and CRCInit installed, but no event.
#[must_use = "the identity-prepared connection graph must be retained or cancelled"]
pub struct BluetoothPeripheralConnectionMemoryGraphIdentityPrepared {
    storage: Pin<&'static mut BluetoothPeripheralConnectionMemoryGraphStorage>,
    binding: BluetoothPeripheralConnectionMemoryGraphBinding,
}

impl BluetoothPeripheralConnectionMemoryGraphIdentityPrepared {
    /// Read the two installed semantic values without exposing SRAM words.
    pub fn identity(&self) -> BluetoothPeripheralConnectionIdentity {
        self.storage.as_ref().get_ref().link_state.identity()
    }

    /// Attach the shared non-scanning RX pool to this connection link state.
    ///
    /// The pool remains separately owned and can later transfer from
    /// response-capable advertising without exposing either SRAM endpoint.
    pub fn attach_receive_pool(
        self,
        pool: BluetoothNonScanningRxMemoryCpuOwned,
    ) -> BluetoothPeripheralConnectionMemoryGraphReceivePrepared {
        self.storage
            .as_ref()
            .get_ref()
            .link_state
            .install_receive_pool(pool.head(), pool.tail());
        BluetoothPeripheralConnectionMemoryGraphReceivePrepared {
            storage: self.storage,
            binding: self.binding,
            pool,
        }
    }

    /// Discard the unsubmitted identity and recover the pristine allocation.
    pub fn cancel(self) -> BluetoothPeripheralConnectionMemoryGraphCpuOwned {
        let mut owner = BluetoothPeripheralConnectionMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        owner
    }
}

/// Identity-prepared connection graph owning its initialized selector-two RX pool.
#[must_use = "the receive-prepared connection graph must be retained or cancelled"]
pub struct BluetoothPeripheralConnectionMemoryGraphReceivePrepared {
    storage: Pin<&'static mut BluetoothPeripheralConnectionMemoryGraphStorage>,
    binding: BluetoothPeripheralConnectionMemoryGraphBinding,
    pool: BluetoothNonScanningRxMemoryCpuOwned,
}

impl BluetoothPeripheralConnectionMemoryGraphReceivePrepared {
    /// Whether the complete bounded receive topology is ready for later publication.
    pub fn receive_pool_is_initialized(&self) -> bool {
        !self
            .storage
            .as_ref()
            .get_ref()
            .link_state
            .has_empty_receive_queue()
            && self.pool.is_initialized()
    }

    /// Install only the complete first-event fields whose transforms are reviewed.
    ///
    /// This is not a publishable descriptor: direction-finding workspace and
    /// scheduler admission remain outside this state.
    pub fn prepare_reviewed_first_event_fields(
        self,
        channel: BluetoothPeripheralConnectionDataChannel,
        interval: BluetoothPeripheralConnectionIntervalTicks,
        window: BluetoothPeripheralConnectionSchedulerWindow,
        receive_wait: BluetoothPeripheralConnectionReceiveWait,
        default_tx_power: BluetoothPeripheralConnectionDefaultTxPowerDbm,
        priority: BluetoothPeripheralConnectionSchedulerPriority,
    ) -> BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        let graph = self.storage.as_ref().get_ref();
        graph.link_state.prepare_event_profile(
            self.pool.head(),
            self.binding.tx_sentinel,
            interval,
            default_tx_power,
            priority,
        );
        graph.scheduler_items[selected_index].prepare_reviewed_first_event_fields(
            graph.link_state.rounded_power(),
            channel,
            window,
            receive_wait,
            priority,
        );
        BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared {
            storage: self.storage,
            binding: self.binding,
            pool: self.pool,
            channel,
            interval,
            window,
            receive_wait,
            default_tx_power,
            priority,
        }
    }

    /// Remove the unpublished RX links and recover both exact CPU owners.
    pub fn cancel(
        self,
    ) -> (
        BluetoothPeripheralConnectionMemoryGraphIdentityPrepared,
        BluetoothNonScanningRxMemoryCpuOwned,
    ) {
        self.storage
            .as_ref()
            .get_ref()
            .link_state
            .clear_receive_pool();
        (
            BluetoothPeripheralConnectionMemoryGraphIdentityPrepared {
                storage: self.storage,
                binding: self.binding,
            },
            self.pool,
        )
    }
}

/// RX-attached graph carrying the reviewed subset of one first-event image.
#[must_use = "the partial connection event image must be retained or cancelled"]
pub struct BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared {
    storage: Pin<&'static mut BluetoothPeripheralConnectionMemoryGraphStorage>,
    binding: BluetoothPeripheralConnectionMemoryGraphBinding,
    pool: BluetoothNonScanningRxMemoryCpuOwned,
    channel: BluetoothPeripheralConnectionDataChannel,
    interval: BluetoothPeripheralConnectionIntervalTicks,
    window: BluetoothPeripheralConnectionSchedulerWindow,
    receive_wait: BluetoothPeripheralConnectionReceiveWait,
    default_tx_power: BluetoothPeripheralConnectionDefaultTxPowerDbm,
    priority: BluetoothPeripheralConnectionSchedulerPriority,
}

impl BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared {
    pub const fn channel(&self) -> BluetoothPeripheralConnectionDataChannel {
        self.channel
    }

    pub const fn interval(&self) -> BluetoothPeripheralConnectionIntervalTicks {
        self.interval
    }

    pub const fn window(&self) -> BluetoothPeripheralConnectionSchedulerWindow {
        self.window
    }

    pub const fn receive_wait(&self) -> BluetoothPeripheralConnectionReceiveWait {
        self.receive_wait
    }

    pub const fn default_tx_power(&self) -> BluetoothPeripheralConnectionDefaultTxPowerDbm {
        self.default_tx_power
    }

    pub const fn priority(&self) -> BluetoothPeripheralConnectionSchedulerPriority {
        self.priority
    }

    /// Join the controller-global disabled-CTE workspace to this exact event.
    ///
    /// The opaque link carries no storage or publication authority. Its
    /// positional encoding and the adjacent baseline policy remain confined
    /// to this private controller-memory codec.
    pub fn install_direction_finding_workspace(
        self,
        workspace: BluetoothDirectionFindingWorkspaceLink,
    ) -> BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared {
        self.storage
            .as_ref()
            .get_ref()
            .link_state
            .install_direction_finding_workspace(workspace);
        BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared {
            prepared: self,
            workspace,
        }
    }

    /// Return to the RX-attached CPU frontier without publishing hardware state.
    pub fn cancel(self) -> BluetoothPeripheralConnectionMemoryGraphReceivePrepared {
        BluetoothPeripheralConnectionMemoryGraphReceivePrepared {
            storage: self.storage,
            binding: self.binding,
            pool: self.pool,
        }
    }
}

/// Complete reviewed first-event fields joined to the global DF workspace.
///
/// This remains CPU-owned and cannot publish a scheduler head or execute RUN.
#[must_use = "the direction-finding-prepared graph must advance or be cancelled"]
pub struct BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared {
    prepared: BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared,
    workspace: BluetoothDirectionFindingWorkspaceLink,
}

impl BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared {
    pub const fn channel(&self) -> BluetoothPeripheralConnectionDataChannel {
        self.prepared.channel()
    }

    pub const fn interval(&self) -> BluetoothPeripheralConnectionIntervalTicks {
        self.prepared.interval()
    }

    pub const fn window(&self) -> BluetoothPeripheralConnectionSchedulerWindow {
        self.prepared.window()
    }

    pub const fn receive_wait(&self) -> BluetoothPeripheralConnectionReceiveWait {
        self.prepared.receive_wait()
    }

    pub const fn default_tx_power(&self) -> BluetoothPeripheralConnectionDefaultTxPowerDbm {
        self.prepared.default_tx_power()
    }

    pub const fn priority(&self) -> BluetoothPeripheralConnectionSchedulerPriority {
        self.prepared.priority()
    }

    /// Opaque identity of the controller-global workspace joined to this event.
    pub const fn direction_finding_workspace(&self) -> BluetoothDirectionFindingWorkspaceLink {
        self.workspace
    }

    /// Detach the selected event item from the connection-private free chain.
    ///
    /// This reproduces only the reviewed allocation ownership transition: the
    /// private head advances to its predecessor, the selected item becomes a
    /// detached in-flight candidate and no MMIO is performed.
    pub fn prepare_scheduler_admission(
        mut self,
    ) -> BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        let predecessor = self.prepared.binding.scheduler_items[selected_index - 1];
        let graph = self.prepared.storage.as_mut().project();
        graph.scheduler_items[selected_index].detach_hardware_predecessor();
        graph.scheduler_items[selected_index].mark_in_flight();
        graph.link_state.install_scheduler_head(predecessor);
        BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared { prepared: self }
    }

    /// Remove the unpublished workspace link and recover the prior exact state.
    pub fn cancel(self) -> BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared {
        self.prepared
            .storage
            .as_ref()
            .get_ref()
            .link_state
            .remove_direction_finding_workspace();
        self.prepared
    }
}

/// DF-linked event whose selected item is detached from the private free list.
#[must_use = "the detached connection item must be published or restored"]
pub struct BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared {
    prepared: BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared,
}

impl BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared {
    /// Exact selected item that may enter the common scheduler list.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        self.prepared.prepared.binding.scheduler_items[selected_index].controller_address()
    }

    /// Freeze the complete SRAM graph before selector-two publication.
    pub fn prepare_publication(
        self,
    ) -> BluetoothPeripheralConnectionMemoryGraphPublicationPrepared {
        BluetoothPeripheralConnectionMemoryGraphPublicationPrepared { prepared: self }
    }

    /// Restore the exact private free chain before any MMIO publication.
    pub fn cancel(mut self) -> BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared {
        let selected_index = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT - 1;
        let selected = self.prepared.prepared.binding.scheduler_items[selected_index];
        let predecessor = self.prepared.prepared.binding.scheduler_items[selected_index - 1];
        let graph = self.prepared.prepared.storage.as_mut().project();
        graph.scheduler_items[selected_index].restore_hardware_predecessor(predecessor);
        graph.scheduler_items[selected_index].restore_cpu_owned_status();
        graph.link_state.install_scheduler_head(selected);
        self.prepared
    }
}

/// Complete connection graph ready for selector-two RX-list publication.
#[must_use = "the prepared connection graph must be published or retained"]
pub struct BluetoothPeripheralConnectionMemoryGraphPublicationPrepared {
    prepared: BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared,
}

impl BluetoothPeripheralConnectionMemoryGraphPublicationPrepared {
    /// Memory-layer mapping for an ordinary non-scanning connection item.
    #[doc(hidden)]
    pub const fn selector(&self) -> BluetoothMemoryListSelector {
        BluetoothRxMemoryListClass::NonScanning.selector()
    }

    /// Validated first receive header retained by this affine graph.
    #[doc(hidden)]
    pub const fn receive_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.prepared.prepared.pool.head()
    }

    /// Exact detached event item retained by this affine graph.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.scheduler_head()
    }

    /// Consume a matching selector-two HAL publication into hardware ownership.
    #[doc(hidden)]
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc mismatch returns both exact affine owners"
        )
    )]
    pub fn into_rx_published(
        self,
        publication: BluetoothRxMemoryListPublished,
    ) -> Result<
        BluetoothPeripheralConnectionMemoryGraphRxPublished,
        BluetoothPeripheralConnectionMemoryGraphPublicationMismatch,
    > {
        let error = if publication.selector() != self.selector() {
            Some(BluetoothPeripheralConnectionMemoryGraphPublicationError::SelectorMismatch)
        } else if publication.head() != self.receive_head() {
            Some(BluetoothPeripheralConnectionMemoryGraphPublicationError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(
                BluetoothPeripheralConnectionMemoryGraphPublicationMismatch {
                    prepared: self,
                    publication,
                    error,
                },
            );
        }
        Ok(BluetoothPeripheralConnectionMemoryGraphRxPublished {
            prepared: self.prepared,
            rx_publication: publication,
        })
    }
}

/// Why a receive-list publication does not name this connection graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPeripheralConnectionMemoryGraphPublicationError {
    /// The publication belongs to another positional memory list.
    SelectorMismatch,
    /// The publication names another pinned receive pool.
    HeadMismatch,
}

/// Failed selector-two publication join retaining both affine owners.
#[must_use = "a mismatched publication still owns the graph and HAL token"]
pub struct BluetoothPeripheralConnectionMemoryGraphPublicationMismatch {
    prepared: BluetoothPeripheralConnectionMemoryGraphPublicationPrepared,
    publication: BluetoothRxMemoryListPublished,
    error: BluetoothPeripheralConnectionMemoryGraphPublicationError,
}

impl BluetoothPeripheralConnectionMemoryGraphPublicationMismatch {
    /// Finite reason why the two affine owners did not match.
    pub const fn error(&self) -> BluetoothPeripheralConnectionMemoryGraphPublicationError {
        self.error
    }

    /// Recover both unchanged owners.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionMemoryGraphPublicationPrepared,
        BluetoothRxMemoryListPublished,
    ) {
        (self.prepared, self.publication)
    }
}

impl core::fmt::Debug for BluetoothPeripheralConnectionMemoryGraphPublicationMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothPeripheralConnectionMemoryGraphPublicationMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Connection graph whose selector-two RX list is hardware-visible.
#[must_use = "the RX-published connection graph must enter the common scheduler"]
pub struct BluetoothPeripheralConnectionMemoryGraphRxPublished {
    prepared: BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared,
    rx_publication: BluetoothRxMemoryListPublished,
}

impl BluetoothPeripheralConnectionMemoryGraphRxPublished {
    /// Exact detached scheduler item paired with this RX publication.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.scheduler_head()
    }

    /// Borrow the retained selector-two publication proof.
    #[doc(hidden)]
    pub const fn rx_publication(&self) -> &BluetoothRxMemoryListPublished {
        &self.rx_publication
    }

    /// Join the exact common RUN proof and retain hardware ownership.
    pub fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothPeripheralConnectionMemoryGraphRunning {
        assert_eq!(
            run.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "the first connection event uses the primary scheduler list"
        );
        assert_eq!(
            run.head().address(),
            Some(self.scheduler_head()),
            "the RUN proof must retain this connection item"
        );
        BluetoothPeripheralConnectionMemoryGraphRunning {
            prepared: self.prepared,
            _rx_publication: self.rx_publication,
        }
    }
}

/// Hardware-owned connection graph admitted through the common RUN transaction.
#[must_use = "the running connection graph must advance through fenced completion"]
pub struct BluetoothPeripheralConnectionMemoryGraphRunning {
    prepared: BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared,
    _rx_publication: BluetoothRxMemoryListPublished,
}

impl BluetoothPeripheralConnectionMemoryGraphRunning {
    /// Exact selected scheduler item retained by the hardware-owned graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.prepared.scheduler_head()
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

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<
        BluetoothPeripheralConnectionMemoryGraphCpuOwned,
        BluetoothPeripheralConnectionMemoryGraphBindFailure,
    > {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothPeripheralConnectionMemoryGraphBindFailure::new(
                    storage,
                    BluetoothPeripheralConnectionMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothPeripheralConnectionMemoryGraphModelAddress,
    ) -> Result<
        BluetoothPeripheralConnectionMemoryGraphCpuOwned,
        BluetoothPeripheralConnectionMemoryGraphBindFailure,
    > {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<
        BluetoothPeripheralConnectionMemoryGraphCpuOwned,
        BluetoothPeripheralConnectionMemoryGraphBindFailure,
    > {
        let identity = BluetoothPeripheralConnectionMemoryGraphIdentity::for_storage(storage);
        let binding = match BluetoothPeripheralConnectionMemoryGraphBinding::new(identity, base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(BluetoothPeripheralConnectionMemoryGraphBindFailure::new(
                    storage, error,
                ));
            }
        };
        let mut owner = BluetoothPeripheralConnectionMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize_graph();
        Ok(owner)
    }
}

impl Default for BluetoothPeripheralConnectionMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothPeripheralConnectionIdentity, BluetoothPeripheralConnectionMemoryGraphBindError,
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphStorage,
    };

    fn storage() -> &'static mut BluetoothPeripheralConnectionMemoryGraphStorage {
        std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ))
    }

    #[test]
    fn binding_builds_the_recovered_allocation_topology() {
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("the model base uses controller SRAM syntax");
        let owner =
            BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(storage(), base)
                .expect("the complete graph fits physical controller SRAM");

        assert!(owner.has_recovered_scheduler_pool());
        assert!(owner.has_empty_receive_queue());
        assert!(owner.has_empty_transmit_queue());
    }

    #[test]
    fn identity_preparation_is_affine_and_cancellable() {
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_1000)
            .expect("the model base uses controller SRAM syntax");
        let owner =
            BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(storage(), base)
                .expect("the complete graph fits physical controller SRAM");
        let identity = BluetoothPeripheralConnectionIdentity::new(
            [0xd4, 0xc3, 0xb2, 0xa1],
            [0x33, 0x22, 0x11],
        );

        let prepared = owner.prepare_identity(identity);
        assert_eq!(prepared.identity(), identity);

        let owner = prepared.cancel();
        assert!(owner.has_recovered_scheduler_pool());
        assert!(owner.has_empty_receive_queue());
        assert!(owner.has_empty_transmit_queue());
    }

    #[test]
    fn out_of_window_binding_returns_the_same_storage() {
        let storage = storage();
        let identity = core::ptr::addr_of!(*storage);
        let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f07_fff0)
            .expect("the final aligned controller SRAM address is syntactically valid");
        let failure = match BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(
            storage, base,
        ) {
            Ok(_) => panic!("the complete graph crosses the physical SRAM boundary"),
            Err(failure) => failure,
        };

        assert_eq!(
            failure.error(),
            BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram
        );
        let (storage, _) = failure.into_parts();
        assert_eq!(core::ptr::addr_of!(*storage), identity);
    }
}
