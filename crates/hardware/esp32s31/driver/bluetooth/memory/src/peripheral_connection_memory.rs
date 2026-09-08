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

use core::pin::Pin;

use crate::{
    direction_finding_workspace::DirectionFindingWorkspaceLink,
    le_rx_packet::{LeReceivedBatch, LeRxError},
    non_scanning_rx_memory::{
        BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, NonScanningRxMemoryCpuOwned,
        NonScanningRxMemoryIdentity,
    },
    rx_memory_list::RxMemoryListClass,
};

use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
        BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerSoftwareListRemovalReady,
        RxMemoryListPublished,
    },
    types::{
        BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
        BluetoothMemoryListSelector,
    },
};

mod codec;

pub use codec::PeripheralConnectionMemoryGraphStorage;

use codec::{
    PeripheralConnectionFirstEventCodecInput, PeripheralConnectionMemoryGraphBinding,
    PeripheralConnectionRecurringEventCodecInput,
    PeripheralConnectionSchedulerCompletionObservation,
};

/// Bytes retained by one connection link-state allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES: usize = 0x84;
/// Bytes retained by one connection scheduler-item allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Scheduler items retained by one connection allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT: usize = 2;
/// Bytes retained by the initially empty transmit queue sentinel.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES: usize = 0x18;

/// Air-interface identity consumed by the S31 connection link state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionIdentity {
    access_address: [u8; 4],
    crc_initialization: [u8; 3],
}

impl PeripheralConnectionIdentity {
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
pub struct PeripheralConnectionDataChannel {
    index: u8,
    frequency_image: u8,
}

impl PeripheralConnectionDataChannel {
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
pub struct PeripheralConnectionIntervalTicks(u32);

impl PeripheralConnectionIntervalTicks {
    pub const fn new(ticks: u32) -> Option<Self> {
        if ticks == 0 { None } else { Some(Self(ticks)) }
    }

    const fn ticks(self) -> u32 {
        self.0
    }
}

/// Non-empty Controller event span installed before one connection RUN.
///
/// This is a required link-state input for both the first and recurring event.
/// A completed event publishes its independent captured receive-time
/// observation in the scheduler item; the two values never share storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionEventSpan(u32);

impl PeripheralConnectionEventSpan {
    pub const fn new(ticks: u32) -> Option<Self> {
        if ticks == 0 { None } else { Some(Self(ticks)) }
    }

    const fn ticks(self) -> u32 {
        self.0
    }
}

/// Opaque Controller receive-time observation captured for a connection event.
///
/// This is neither scheduler time nor a normalized packet-start anchor. It can
/// enter only the chip-private epoch and PHY timing projection above this
/// controller-memory boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionCapturedAnchorTime(u32);

impl PeripheralConnectionCapturedAnchorTime {
    const fn from_controller_sram_word(word: u32) -> Self {
        Self(word)
    }

    /// Borrow the wrapping tick observation for chip-private time projection.
    #[doc(hidden)]
    pub const fn wrapping_controller_ticks(self) -> u32 {
        self.0
    }
}

/// Whether a completed connection scheduler item published a receive-time capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionCapturedAnchorAvailability {
    Absent,
    Available(PeripheralConnectionCapturedAnchorTime),
}

/// Non-empty raw Controller window for one connection scheduler item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionSchedulerWindow {
    start: u32,
    end: u32,
}

impl PeripheralConnectionSchedulerWindow {
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
pub struct PeripheralConnectionReceiveWait {
    transmit_window_micros: u32,
    timing_guard_micros: u32,
    total_micros: u16,
}

impl PeripheralConnectionReceiveWait {
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
}

/// Recurring-event receive wait for the reviewed software window-widening path.
///
/// The semantic duration combines the Controller's fixed guard, accumulated
/// anchor uncertainty, twice the current window widening and the final
/// Controller boundary guard. The private SRAM codec alone selects the
/// zero-duration, short or half-resolution long descriptor representation.
/// Automatic window widening is intentionally not represented by this type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionRecurringReceiveWait {
    total_micros: u32,
}

impl PeripheralConnectionRecurringReceiveWait {
    /// Form one receive wait whose physical duration is represented exactly.
    ///
    /// The long form stores two-microsecond units. Odd long durations are
    /// rejected instead of silently reproducing the vendor's truncating shift.
    pub const fn new(total_micros: u32) -> Option<Self> {
        const SHORT_MAX_MICROS: u32 = u16::MAX as u32 - 1;
        const LONG_MAX_MICROS: u32 = u16::MAX as u32 * 2;

        if total_micros > LONG_MAX_MICROS
            || (total_micros > SHORT_MAX_MICROS && total_micros & 1 != 0)
        {
            return None;
        }
        Some(Self { total_micros })
    }

    pub const fn total_micros(self) -> u32 {
        self.total_micros
    }
}

/// Physical default transmit-power request for the first connection profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionDefaultTxPowerDbm(i8);

impl PeripheralConnectionDefaultTxPowerDbm {
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    pub const fn dbm(self) -> i8 {
        self.0
    }
}

/// Source-owned event priority shared by connection state and scheduler item.
///
/// The first event starts at 13. A normally completed recurring event resets
/// to 8. Ordinary conflict escalation is capped at 14; the distinct exhausted
/// retry-budget path may force 15. Those policy transitions remain outside the
/// private descriptor encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionSchedulerPriority(u8);

impl PeripheralConnectionSchedulerPriority {
    /// Priority selected by the reviewed ESP32-S31 first-event policy.
    pub const FIRST_EVENT: Self = Self(13);

    /// Baseline restored before an ordinary recurring event.
    pub const RECURRING_BASELINE: Self = Self(8);

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Why peripheral-connection storage cannot become a bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionMemoryGraphBindError {
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
}

/// Failed binding that returns the exact unchanged static allocation.
pub struct PeripheralConnectionMemoryGraphBindFailure {
    storage: &'static mut PeripheralConnectionMemoryGraphStorage,
    error: PeripheralConnectionMemoryGraphBindError,
}

impl PeripheralConnectionMemoryGraphBindFailure {
    fn new(
        storage: &'static mut PeripheralConnectionMemoryGraphStorage,
        error: PeripheralConnectionMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> PeripheralConnectionMemoryGraphBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut PeripheralConnectionMemoryGraphStorage,
        PeripheralConnectionMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for PeripheralConnectionMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PeripheralConnectionMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl PeripheralConnectionMemoryGraphModelAddress {
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

/// Opaque identity of one exact statically pinned connection graph.
///
/// This is only an equality witness. It exposes neither its storage pointer
/// nor any controller-SRAM address and grants no memory or publication access.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PeripheralConnectionMemoryGraphIdentity(usize);

impl PeripheralConnectionMemoryGraphIdentity {
    fn for_storage(storage: &PeripheralConnectionMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for PeripheralConnectionMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PeripheralConnectionMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

/// Unique CPU owner of one allocation-time peripheral-connection graph.
#[must_use = "the bound peripheral-connection graph must be retained"]
pub struct PeripheralConnectionMemoryGraphCpuOwned {
    storage: Pin<&'static mut PeripheralConnectionMemoryGraphStorage>,
    binding: PeripheralConnectionMemoryGraphBinding,
}

impl PeripheralConnectionMemoryGraphCpuOwned {
    /// Equality witness for the exact pinned storage object.
    pub const fn identity(&self) -> PeripheralConnectionMemoryGraphIdentity {
        self.binding.identity()
    }

    /// The recovered allocation starts without any receive buffer owner.
    pub fn has_empty_receive_queue(&self) -> bool {
        self.storage.as_ref().get_ref().has_empty_receive_queue()
    }

    /// The recovered allocation starts with one shared head/tail TX sentinel.
    pub fn has_empty_transmit_queue(&self) -> bool {
        self.storage
            .as_ref()
            .get_ref()
            .has_empty_transmit_queue(&self.binding)
    }

    fn reinitialize_graph(&mut self) {
        self.storage.as_mut().initialize_graph(&self.binding);
    }

    /// Both reusable scheduler items still form the recovered private pool.
    pub fn has_recovered_scheduler_pool(&self) -> bool {
        self.storage
            .as_ref()
            .get_ref()
            .has_recovered_scheduler_pool(&self.binding)
    }

    /// Install only the reviewed connection identity fields.
    ///
    /// This state cannot publish a scheduler item. A later event builder must
    /// consume it after closing the anchor, duration and packet sequence
    /// semantics.
    pub fn prepare_identity(
        self,
        identity: PeripheralConnectionIdentity,
    ) -> PeripheralConnectionMemoryGraphIdentityPrepared {
        self.storage.as_ref().get_ref().prepare_identity(identity);
        PeripheralConnectionMemoryGraphIdentityPrepared {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// CPU-owned graph with Access Address and CRCInit installed, but no event.
#[must_use = "the identity-prepared connection graph must be retained or cancelled"]
pub struct PeripheralConnectionMemoryGraphIdentityPrepared {
    storage: Pin<&'static mut PeripheralConnectionMemoryGraphStorage>,
    binding: PeripheralConnectionMemoryGraphBinding,
}

impl PeripheralConnectionMemoryGraphIdentityPrepared {
    /// Read the two installed semantic values without exposing SRAM words.
    pub fn identity(&self) -> PeripheralConnectionIdentity {
        self.storage.as_ref().get_ref().identity()
    }

    /// Attach the shared non-scanning RX pool to this connection link state.
    ///
    /// The pool remains separately owned and can later transfer from
    /// response-capable advertising without exposing either SRAM endpoint.
    pub fn attach_receive_pool(
        self,
        pool: NonScanningRxMemoryCpuOwned,
    ) -> PeripheralConnectionMemoryGraphReceivePrepared {
        self.storage
            .as_ref()
            .get_ref()
            .install_receive_pool(pool.current_cursor(), pool.tail());
        PeripheralConnectionMemoryGraphReceivePrepared {
            storage: self.storage,
            binding: self.binding,
            pool,
        }
    }

    /// Discard the unsubmitted identity and recover the pristine allocation.
    pub fn cancel(self) -> PeripheralConnectionMemoryGraphCpuOwned {
        let mut owner = PeripheralConnectionMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        owner
    }
}

/// Identity-prepared connection graph owning its initialized selector-two RX pool.
#[must_use = "the receive-prepared connection graph must be retained or cancelled"]
pub struct PeripheralConnectionMemoryGraphReceivePrepared {
    storage: Pin<&'static mut PeripheralConnectionMemoryGraphStorage>,
    binding: PeripheralConnectionMemoryGraphBinding,
    pool: NonScanningRxMemoryCpuOwned,
}

impl PeripheralConnectionMemoryGraphReceivePrepared {
    /// Whether the complete bounded receive topology is ready for later publication.
    pub fn receive_pool_is_initialized(&self) -> bool {
        !self.storage.as_ref().get_ref().has_empty_receive_queue() && self.pool.is_initialized()
    }

    /// Install only the complete first-event fields whose transforms are reviewed.
    ///
    /// This is not a publishable descriptor: direction-finding workspace and
    /// scheduler admission remain outside this state.
    /// `raw_sequence_lead` is the accepted scheduler policy's duration in
    /// controller ticks. It shifts the hardware start while preserving the
    /// complete admitted window duration.
    #[expect(
        clippy::too_many_arguments,
        reason = "the complete typed first-event fields cross this ownership boundary together"
    )]
    pub fn prepare_reviewed_first_event_fields(
        self,
        channel: PeripheralConnectionDataChannel,
        interval: PeripheralConnectionIntervalTicks,
        event_span: PeripheralConnectionEventSpan,
        window: PeripheralConnectionSchedulerWindow,
        receive_wait: PeripheralConnectionReceiveWait,
        default_tx_power: PeripheralConnectionDefaultTxPowerDbm,
        priority: PeripheralConnectionSchedulerPriority,
        raw_sequence_lead: u32,
    ) -> PeripheralConnectionMemoryGraphEventFieldsPrepared {
        let graph = self.storage.as_ref().get_ref();
        let input = PeripheralConnectionFirstEventCodecInput {
            channel,
            interval,
            event_span,
            window,
            receive_wait,
            default_tx_power,
            priority,
            raw_sequence_lead,
        };
        graph.prepare_reviewed_first_event_fields(
            &self.binding,
            self.pool.current_cursor(),
            &input,
        );
        PeripheralConnectionMemoryGraphEventFieldsPrepared {
            storage: self.storage,
            binding: self.binding,
            pool: self.pool,
            channel,
            interval,
            event_span,
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
        PeripheralConnectionMemoryGraphIdentityPrepared,
        NonScanningRxMemoryCpuOwned,
    ) {
        self.storage.as_ref().get_ref().clear_receive_pool();
        (
            PeripheralConnectionMemoryGraphIdentityPrepared {
                storage: self.storage,
                binding: self.binding,
            },
            self.pool,
        )
    }
}

/// RX-attached graph carrying the reviewed subset of one first-event image.
#[must_use = "the partial connection event image must be retained or cancelled"]
pub struct PeripheralConnectionMemoryGraphEventFieldsPrepared {
    storage: Pin<&'static mut PeripheralConnectionMemoryGraphStorage>,
    binding: PeripheralConnectionMemoryGraphBinding,
    pool: NonScanningRxMemoryCpuOwned,
    channel: PeripheralConnectionDataChannel,
    interval: PeripheralConnectionIntervalTicks,
    event_span: PeripheralConnectionEventSpan,
    window: PeripheralConnectionSchedulerWindow,
    receive_wait: PeripheralConnectionReceiveWait,
    default_tx_power: PeripheralConnectionDefaultTxPowerDbm,
    priority: PeripheralConnectionSchedulerPriority,
}

impl PeripheralConnectionMemoryGraphEventFieldsPrepared {
    pub const fn channel(&self) -> PeripheralConnectionDataChannel {
        self.channel
    }

    pub const fn interval(&self) -> PeripheralConnectionIntervalTicks {
        self.interval
    }

    pub const fn event_span(&self) -> PeripheralConnectionEventSpan {
        self.event_span
    }

    pub const fn window(&self) -> PeripheralConnectionSchedulerWindow {
        self.window
    }

    pub const fn receive_wait(&self) -> PeripheralConnectionReceiveWait {
        self.receive_wait
    }

    pub const fn default_tx_power(&self) -> PeripheralConnectionDefaultTxPowerDbm {
        self.default_tx_power
    }

    pub const fn priority(&self) -> PeripheralConnectionSchedulerPriority {
        self.priority
    }

    /// Join the controller-global disabled-CTE workspace to this exact event.
    ///
    /// The opaque link carries no storage or publication authority. Its
    /// positional encoding and the adjacent baseline policy remain confined
    /// to this private controller-memory codec.
    pub fn install_direction_finding_workspace(
        self,
        workspace: DirectionFindingWorkspaceLink,
    ) -> PeripheralConnectionMemoryGraphDirectionFindingPrepared {
        self.storage
            .as_ref()
            .get_ref()
            .install_direction_finding_workspace(workspace);
        PeripheralConnectionMemoryGraphDirectionFindingPrepared {
            prepared: self,
            workspace,
        }
    }

    /// Return to the RX-attached CPU frontier without publishing hardware state.
    pub fn cancel(self) -> PeripheralConnectionMemoryGraphReceivePrepared {
        PeripheralConnectionMemoryGraphReceivePrepared {
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
pub struct PeripheralConnectionMemoryGraphDirectionFindingPrepared {
    prepared: PeripheralConnectionMemoryGraphEventFieldsPrepared,
    workspace: DirectionFindingWorkspaceLink,
}

impl PeripheralConnectionMemoryGraphDirectionFindingPrepared {
    pub const fn channel(&self) -> PeripheralConnectionDataChannel {
        self.prepared.channel()
    }

    pub const fn interval(&self) -> PeripheralConnectionIntervalTicks {
        self.prepared.interval()
    }

    pub const fn event_span(&self) -> PeripheralConnectionEventSpan {
        self.prepared.event_span()
    }

    pub const fn window(&self) -> PeripheralConnectionSchedulerWindow {
        self.prepared.window()
    }

    pub const fn receive_wait(&self) -> PeripheralConnectionReceiveWait {
        self.prepared.receive_wait()
    }

    pub const fn default_tx_power(&self) -> PeripheralConnectionDefaultTxPowerDbm {
        self.prepared.default_tx_power()
    }

    pub const fn priority(&self) -> PeripheralConnectionSchedulerPriority {
        self.prepared.priority()
    }

    /// Opaque identity of the controller-global workspace joined to this event.
    pub const fn direction_finding_workspace(&self) -> DirectionFindingWorkspaceLink {
        self.workspace
    }

    /// Detach the selected event item from the connection-private free chain.
    ///
    /// This reproduces only the reviewed allocation ownership transition: the
    /// private head advances to its predecessor, the selected item becomes a
    /// detached in-flight candidate and no MMIO is performed.
    pub fn prepare_scheduler_admission(
        self,
    ) -> PeripheralConnectionMemoryGraphSchedulerAdmissionPrepared {
        let prepared = PeripheralConnectionMemoryGraphPreparedEvent::First(self);
        let prepared = PeripheralConnectionMemoryGraphSchedulerAdmissionCore::new(prepared);
        PeripheralConnectionMemoryGraphSchedulerAdmissionPrepared { prepared }
    }

    /// Remove the unpublished workspace link and recover the prior exact state.
    pub fn cancel(self) -> PeripheralConnectionMemoryGraphEventFieldsPrepared {
        self.prepared
            .storage
            .as_ref()
            .get_ref()
            .remove_direction_finding_workspace();
        self.prepared
    }
}

/// One fully prepared connection event before common scheduler publication.
///
/// First and recurring events retain different cancellation frontiers, while
/// the detached item, RX publication and RUN ownership protocol is identical.
enum PeripheralConnectionMemoryGraphPreparedEvent {
    First(PeripheralConnectionMemoryGraphDirectionFindingPrepared),
    Recurring(PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared),
}

impl PeripheralConnectionMemoryGraphPreparedEvent {
    fn storage(&self) -> &PeripheralConnectionMemoryGraphStorage {
        match self {
            Self::First(first) => first.prepared.storage.as_ref().get_ref(),
            Self::Recurring(recurring) => recurring.active.storage.as_ref().get_ref(),
        }
    }

    const fn binding(&self) -> &PeripheralConnectionMemoryGraphBinding {
        match self {
            Self::First(first) => &first.prepared.binding,
            Self::Recurring(recurring) => &recurring.active.binding,
        }
    }

    fn receive_head(&self) -> BluetoothControllerSramAddress {
        match self {
            Self::First(first) => first.prepared.pool.current_cursor(),
            Self::Recurring(recurring) => recurring.active.pool.connection_current_cursor(),
        }
    }

    const fn receive_pool(&self) -> &NonScanningRxMemoryCpuOwned {
        match self {
            Self::First(first) => &first.prepared.pool,
            Self::Recurring(recurring) => &recurring.active.pool,
        }
    }

    fn into_active(self) -> PeripheralConnectionMemoryGraphActiveCpuOwned {
        match self {
            Self::First(first) => {
                let PeripheralConnectionMemoryGraphDirectionFindingPrepared {
                    prepared,
                    workspace,
                } = first;
                let PeripheralConnectionMemoryGraphEventFieldsPrepared {
                    storage,
                    binding,
                    pool,
                    channel: _,
                    interval: _,
                    event_span: _,
                    window: _,
                    receive_wait: _,
                    default_tx_power: _,
                    priority: _,
                } = prepared;
                PeripheralConnectionMemoryGraphActiveCpuOwned {
                    storage,
                    binding,
                    pool,
                    workspace,
                }
            }
            Self::Recurring(recurring) => recurring.active,
        }
    }
}

/// Shared detached-event owner used by both first and recurring lifecycles.
struct PeripheralConnectionMemoryGraphSchedulerAdmissionCore {
    prepared: PeripheralConnectionMemoryGraphPreparedEvent,
}

impl PeripheralConnectionMemoryGraphSchedulerAdmissionCore {
    fn new(prepared: PeripheralConnectionMemoryGraphPreparedEvent) -> Self {
        prepared
            .storage()
            .prepare_scheduler_admission(prepared.binding());
        Self { prepared }
    }

    const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding().scheduler_head()
    }

    fn receive_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.receive_head()
    }

    fn storage(&self) -> &PeripheralConnectionMemoryGraphStorage {
        self.prepared.storage()
    }

    const fn receive_pool(&self) -> &NonScanningRxMemoryCpuOwned {
        self.prepared.receive_pool()
    }

    fn cancel(self) -> PeripheralConnectionMemoryGraphPreparedEvent {
        self.prepared
            .storage()
            .restore_scheduler_admission(self.prepared.binding());
        self.prepared
    }

    fn into_active(self) -> PeripheralConnectionMemoryGraphActiveCpuOwned {
        self.prepared.into_active()
    }
}

/// DF-linked event whose selected item is detached from the private free list.
#[must_use = "the detached connection item must be published or restored"]
pub struct PeripheralConnectionMemoryGraphSchedulerAdmissionPrepared {
    prepared: PeripheralConnectionMemoryGraphSchedulerAdmissionCore,
}

impl PeripheralConnectionMemoryGraphSchedulerAdmissionPrepared {
    /// Exact selected item that may enter the common scheduler list.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.scheduler_head()
    }

    /// Freeze the complete SRAM graph before selector-two publication.
    pub fn prepare_publication(self) -> PeripheralConnectionMemoryGraphPublicationPrepared {
        PeripheralConnectionMemoryGraphPublicationPrepared {
            prepared: self.prepared,
        }
    }

    /// Restore the exact private free chain before any MMIO publication.
    pub fn cancel(self) -> PeripheralConnectionMemoryGraphDirectionFindingPrepared {
        let PeripheralConnectionMemoryGraphPreparedEvent::First(prepared) = self.prepared.cancel()
        else {
            unreachable!("the first-event admission wrapper retains a first event")
        };
        prepared
    }
}

/// Recurring event whose selected item is detached for common-list admission.
#[must_use = "the detached recurring connection item must be published or restored"]
pub struct PeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared {
    prepared: PeripheralConnectionMemoryGraphSchedulerAdmissionCore,
}

impl PeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared {
    /// Exact selected item that may enter the common scheduler list.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.scheduler_head()
    }

    /// Converge on the common frozen publication lifecycle.
    pub fn prepare_publication(self) -> PeripheralConnectionMemoryGraphPublicationPrepared {
        PeripheralConnectionMemoryGraphPublicationPrepared {
            prepared: self.prepared,
        }
    }

    /// Restore the private chain and recover the recurring preparation owner.
    pub fn cancel(self) -> PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared {
        let PeripheralConnectionMemoryGraphPreparedEvent::Recurring(prepared) =
            self.prepared.cancel()
        else {
            unreachable!("the recurring admission wrapper retains a recurring event")
        };
        prepared
    }
}

/// Complete connection graph ready for selector-two RX-list publication.
#[must_use = "the prepared connection graph must be published or retained"]
pub struct PeripheralConnectionMemoryGraphPublicationPrepared {
    prepared: PeripheralConnectionMemoryGraphSchedulerAdmissionCore,
}

impl PeripheralConnectionMemoryGraphPublicationPrepared {
    /// Memory-layer mapping for an ordinary non-scanning connection item.
    #[doc(hidden)]
    pub const fn selector(&self) -> BluetoothMemoryListSelector {
        RxMemoryListClass::NonScanning.selector()
    }

    /// Validated receive header retained by this affine graph.
    #[doc(hidden)]
    pub fn receive_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.receive_head()
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
        publication: RxMemoryListPublished,
    ) -> Result<
        PeripheralConnectionMemoryGraphRxPublished,
        PeripheralConnectionMemoryGraphPublicationMismatch,
    > {
        let error = if publication.selector() != self.selector() {
            Some(PeripheralConnectionMemoryGraphPublicationError::SelectorMismatch)
        } else if publication.head() != self.receive_head() {
            Some(PeripheralConnectionMemoryGraphPublicationError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(PeripheralConnectionMemoryGraphPublicationMismatch {
                prepared: self,
                publication,
                error,
            });
        }
        Ok(PeripheralConnectionMemoryGraphRxPublished {
            prepared: self.prepared,
            rx_publication: publication,
        })
    }
}

/// Why a receive-list publication does not name this connection graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionMemoryGraphPublicationError {
    /// The publication belongs to another positional memory list.
    SelectorMismatch,
    /// The publication names another pinned receive pool.
    HeadMismatch,
}

/// Failed selector-two publication join retaining both affine owners.
#[must_use = "a mismatched publication still owns the graph and HAL token"]
pub struct PeripheralConnectionMemoryGraphPublicationMismatch {
    prepared: PeripheralConnectionMemoryGraphPublicationPrepared,
    publication: RxMemoryListPublished,
    error: PeripheralConnectionMemoryGraphPublicationError,
}

impl PeripheralConnectionMemoryGraphPublicationMismatch {
    /// Finite reason why the two affine owners did not match.
    pub const fn error(&self) -> PeripheralConnectionMemoryGraphPublicationError {
        self.error
    }

    /// Recover both unchanged owners.
    pub fn into_parts(
        self,
    ) -> (
        PeripheralConnectionMemoryGraphPublicationPrepared,
        RxMemoryListPublished,
    ) {
        (self.prepared, self.publication)
    }
}

impl core::fmt::Debug for PeripheralConnectionMemoryGraphPublicationMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PeripheralConnectionMemoryGraphPublicationMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Connection graph whose selector-two RX list is hardware-visible.
#[must_use = "the RX-published connection graph must enter the common scheduler"]
pub struct PeripheralConnectionMemoryGraphRxPublished {
    prepared: PeripheralConnectionMemoryGraphSchedulerAdmissionCore,
    rx_publication: RxMemoryListPublished,
}

impl PeripheralConnectionMemoryGraphRxPublished {
    /// Exact detached scheduler item paired with this RX publication.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.scheduler_head()
    }

    /// Borrow the retained selector-two publication proof.
    #[doc(hidden)]
    pub const fn rx_publication(&self) -> &RxMemoryListPublished {
        &self.rx_publication
    }

    /// Join the exact common RUN proof and retain hardware ownership.
    pub fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> PeripheralConnectionMemoryGraphRunning {
        assert_eq!(
            run.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "connection events use the primary scheduler list"
        );
        assert_eq!(
            run.head().address(),
            Some(self.scheduler_head()),
            "the RUN proof must retain this connection item"
        );
        PeripheralConnectionMemoryGraphRunning {
            prepared: self.prepared,
            _rx_publication: self.rx_publication,
        }
    }
}

/// Hardware-owned connection graph admitted through the common RUN transaction.
#[must_use = "the running connection graph must advance through fenced completion"]
pub struct PeripheralConnectionMemoryGraphRunning {
    prepared: PeripheralConnectionMemoryGraphSchedulerAdmissionCore,
    _rx_publication: RxMemoryListPublished,
}

impl PeripheralConnectionMemoryGraphRunning {
    /// Exact selected scheduler item retained by the hardware-owned graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.prepared.scheduler_head()
    }

    /// Consume one fresh list-zero completion report and inspect the selected item.
    ///
    /// The status word remains private controller SRAM. The in-flight sentinel
    /// retains hardware ownership; any other value advances only to a fenced
    /// completion observation and does not authorize descriptor mutation.
    pub fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> PeripheralConnectionMemoryGraphCompletionObservation {
        if observed.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            return PeripheralConnectionMemoryGraphCompletionObservation::ListMismatch {
                running: self,
                observed,
            };
        }
        let Some(completion) = self.prepared.storage().scheduler_completion_observation() else {
            return PeripheralConnectionMemoryGraphCompletionObservation::StillInFlight(self);
        };
        PeripheralConnectionMemoryGraphCompletionObservation::CompletionObserved(
            PeripheralConnectionMemoryGraphCompletionObserved {
                running: self,
                completion,
            },
        )
    }
}

/// Opaque category of one non-sentinel connection scheduler status.
///
/// The raw controller word remains private to the memory codec. Zero versus
/// nonzero is diagnostic only and does not classify Link Layer completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionSchedulerItemCompletionStatus {
    Zero,
    NonZero,
}

/// One bounded observation of a running connection graph.
#[must_use = "the graph and any unrelated finished-list token remain owned"]
pub enum PeripheralConnectionMemoryGraphCompletionObservation {
    ListMismatch {
        running: PeripheralConnectionMemoryGraphRunning,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(PeripheralConnectionMemoryGraphRunning),
    CompletionObserved(PeripheralConnectionMemoryGraphCompletionObserved),
}

/// Hardware-owned graph after its selected item produced a non-sentinel status.
#[must_use = "the completed connection graph must pass scheduler unlink before CPU access"]
pub struct PeripheralConnectionMemoryGraphCompletionObserved {
    running: PeripheralConnectionMemoryGraphRunning,
    completion: PeripheralConnectionSchedulerCompletionObservation,
}

impl PeripheralConnectionMemoryGraphCompletionObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.running.scheduler_item_address()
    }

    pub const fn status(&self) -> PeripheralConnectionSchedulerItemCompletionStatus {
        self.completion.status()
    }

    /// Bind the exact post-unlink removal proof before reading or resetting SRAM.
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc mismatch returns both exact affine owners"
        )
    )]
    pub fn prepare_recycle_after_software_list_removal(
        self,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        PeripheralConnectionMemoryGraphRecyclePrepared,
        PeripheralConnectionMemoryGraphRecycleFailure,
    > {
        let error = if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(PeripheralConnectionMemoryGraphRecycleError::HardwareListMismatch)
        } else if removal.completed_head().address() != Some(self.scheduler_item_address()) {
            Some(PeripheralConnectionMemoryGraphRecycleError::SchedulerItemMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(PeripheralConnectionMemoryGraphRecycleFailure {
                completed: self,
                removal,
                error,
            });
        }
        let capture = self
            .running
            .prepared
            .storage()
            .captured_anchor_availability(self.completion);
        Ok(PeripheralConnectionMemoryGraphRecyclePrepared {
            completed: self,
            removal,
            capture,
        })
    }
}

/// Why a completed connection graph rejected CPU-recycle authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionMemoryGraphRecycleError {
    HardwareListMismatch,
    SchedulerItemMismatch,
}

/// Lossless recycle rejection retaining the hardware-owned graph and proof.
#[must_use = "the completed connection graph and removal proof remain owned"]
pub struct PeripheralConnectionMemoryGraphRecycleFailure {
    completed: PeripheralConnectionMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    error: PeripheralConnectionMemoryGraphRecycleError,
}

impl PeripheralConnectionMemoryGraphRecycleFailure {
    pub const fn error(&self) -> PeripheralConnectionMemoryGraphRecycleError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        PeripheralConnectionMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

/// Completed connection graph authorized for bounded RX extraction.
#[must_use = "the connection RX result must be extracted or retained unchanged"]
pub struct PeripheralConnectionMemoryGraphRecyclePrepared {
    completed: PeripheralConnectionMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    capture: PeripheralConnectionCapturedAnchorAvailability,
}

impl PeripheralConnectionMemoryGraphRecyclePrepared {
    /// Whether this completed item published a hardware receive-time capture.
    pub const fn captured_anchor_availability(
        &self,
    ) -> PeripheralConnectionCapturedAnchorAvailability {
        self.capture
    }

    /// Recover both unchanged owners before extraction starts.
    pub fn into_parts(
        self,
    ) -> (
        PeripheralConnectionMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }

    /// Validate and copy every contiguous completed PDU without mutating SRAM.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc extraction failure retains the complete affine graph"
    )]
    pub fn extract_received(
        self,
    ) -> Result<
        PeripheralConnectionMemoryGraphRxExtracted,
        PeripheralConnectionMemoryGraphRxExtractionFailure,
    > {
        let batch = match self
            .completed
            .running
            .prepared
            .receive_pool()
            .extract_completed_connection_rx_batch()
        {
            Ok(batch) => batch,
            Err(error) => {
                return Err(PeripheralConnectionMemoryGraphRxExtractionFailure {
                    prepared: self,
                    error,
                });
            }
        };
        Ok(PeripheralConnectionMemoryGraphRxExtracted {
            prepared: self,
            batch,
        })
    }
}

/// Malformed completed RX storage retaining the unchanged recycle owner.
#[must_use = "the unchanged connection graph remains unavailable until fail-stop handling"]
pub struct PeripheralConnectionMemoryGraphRxExtractionFailure {
    prepared: PeripheralConnectionMemoryGraphRecyclePrepared,
    error: LeRxError,
}

impl PeripheralConnectionMemoryGraphRxExtractionFailure {
    pub const fn error(&self) -> LeRxError {
        self.error
    }

    pub fn into_prepared(self) -> PeripheralConnectionMemoryGraphRecyclePrepared {
        self.prepared
    }
}

/// Copied RX batch paired with the sole reclaimable connection graph.
#[must_use = "commit reclamation before reusing the connection allocation"]
pub struct PeripheralConnectionMemoryGraphRxExtracted {
    prepared: PeripheralConnectionMemoryGraphRecyclePrepared,
    batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
}

impl PeripheralConnectionMemoryGraphRxExtracted {
    /// Copy of every completed Link Layer PDU in receive-list order.
    pub const fn batch(&self) -> LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        self.batch
    }

    /// Recover the unchanged recycle proof before reclamation is committed.
    #[doc(hidden)]
    pub fn into_prepared(self) -> PeripheralConnectionMemoryGraphRecyclePrepared {
        self.prepared
    }

    /// Restore both private graphs and return ordinary CPU ownership.
    pub fn commit(self) -> PeripheralConnectionMemoryGraphRecycled {
        let PeripheralConnectionMemoryGraphRecyclePrepared {
            completed,
            removal: _,
            capture,
        } = self.prepared;
        let PeripheralConnectionMemoryGraphCompletionObserved {
            running,
            completion,
        } = completed;
        let PeripheralConnectionMemoryGraphRunning {
            prepared,
            _rx_publication: _,
        } = running;
        let mut graph = prepared.into_active();
        graph.restore_after_event();
        PeripheralConnectionMemoryGraphRecycled {
            graph,
            batch: self.batch,
            status: completion.status(),
            capture,
        }
    }
}

/// CPU owner of a live connection graph between recurring radio events.
///
/// Unlike the cold allocation owner, this state preserves the link-state
/// words updated by hardware. It restores only the detached scheduler item
/// and receive rotation which the completed event exclusively owned.
#[must_use = "the active connection allocation must reach recurrence or teardown"]
pub struct PeripheralConnectionMemoryGraphActiveCpuOwned {
    storage: Pin<&'static mut PeripheralConnectionMemoryGraphStorage>,
    binding: PeripheralConnectionMemoryGraphBinding,
    pool: NonScanningRxMemoryCpuOwned,
    workspace: DirectionFindingWorkspaceLink,
}

impl PeripheralConnectionMemoryGraphActiveCpuOwned {
    pub const fn identity(&self) -> PeripheralConnectionMemoryGraphIdentity {
        self.binding.identity()
    }

    pub const fn receive_identity(&self) -> NonScanningRxMemoryIdentity {
        self.pool.identity()
    }

    /// Persistent radio identity retained while event-local fields are rebuilt.
    pub fn connection_identity(&self) -> PeripheralConnectionIdentity {
        self.storage.as_ref().get_ref().identity()
    }

    /// Opaque global workspace link retained across recurring events.
    pub const fn direction_finding_workspace(&self) -> DirectionFindingWorkspaceLink {
        self.workspace
    }

    /// Whether the event-local scheduler item and RX pool are reusable.
    pub fn event_resources_are_recycled(&self) -> bool {
        self.storage
            .as_ref()
            .get_ref()
            .event_resources_are_recycled(&self.binding)
            && self.pool.connection_resources_ready()
    }

    /// Reclaim a completed control payload while retaining the hardware cursor.
    /// A scheduler completion by itself does not complete a queued transmission.
    pub fn reclaim_control_transmission(&mut self) -> bool {
        self.storage
            .as_ref()
            .get_ref()
            .reclaim_control_tx(&self.binding)
    }

    /// Queue a control payload under exclusive CPU ownership. `false` leaves
    /// both the queued packet and its retransmission state unchanged.
    pub fn enqueue_control_transmission(
        &mut self,
        payload: &[u8],
    ) -> Result<bool, crate::LeTxPacketPrepareError> {
        self.storage
            .as_mut()
            .enqueue_control_tx(&self.binding, payload)
    }

    /// Prepare the reviewed dynamic fields for one software-widened recurrence.
    ///
    /// Connection identity, packet history, sequence state, RX ownership and
    /// the installed direction-finding workspace remain untouched. This step
    /// performs no publication and can be cancelled back to this exact owner.
    /// `raw_sequence_lead` must come from the reservation's timing policy after
    /// sequence authorization, just as for the first event.
    pub fn prepare_reviewed_recurring_event_fields(
        self,
        channel: PeripheralConnectionDataChannel,
        event_span: PeripheralConnectionEventSpan,
        window: PeripheralConnectionSchedulerWindow,
        receive_wait: PeripheralConnectionRecurringReceiveWait,
        priority: PeripheralConnectionSchedulerPriority,
        raw_sequence_lead: u32,
    ) -> PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared {
        let input = PeripheralConnectionRecurringEventCodecInput {
            channel,
            event_span,
            window,
            receive_wait,
            priority,
            raw_sequence_lead,
        };
        self.storage
            .as_ref()
            .get_ref()
            .prepare_reviewed_recurring_event_fields(&input);
        PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared {
            active: self,
            channel,
            event_span,
            window,
            receive_wait,
            priority,
        }
    }

    fn restore_after_event(&mut self) {
        self.storage
            .as_ref()
            .get_ref()
            .restore_scheduler_admission(&self.binding);
        self.pool.rotate_after_connection_event();
        self.storage.as_ref().get_ref().install_receive_pool(
            self.pool.connection_current_cursor(),
            self.pool.connection_tail(),
        );
    }
}

/// CPU-owned live graph carrying one unpublished recurring-event image.
///
/// The supported profile is software window widening only. No automatic-WW
/// selector or raw receive-wait image can cross this boundary.
#[must_use = "the recurring event fields must be cancelled or advanced before publication"]
pub struct PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared {
    active: PeripheralConnectionMemoryGraphActiveCpuOwned,
    channel: PeripheralConnectionDataChannel,
    event_span: PeripheralConnectionEventSpan,
    window: PeripheralConnectionSchedulerWindow,
    receive_wait: PeripheralConnectionRecurringReceiveWait,
    priority: PeripheralConnectionSchedulerPriority,
}

impl PeripheralConnectionMemoryGraphRecurringEventFieldsPrepared {
    pub const fn channel(&self) -> PeripheralConnectionDataChannel {
        self.channel
    }

    pub const fn event_span(&self) -> PeripheralConnectionEventSpan {
        self.event_span
    }

    pub const fn window(&self) -> PeripheralConnectionSchedulerWindow {
        self.window
    }

    pub const fn receive_wait(&self) -> PeripheralConnectionRecurringReceiveWait {
        self.receive_wait
    }

    pub const fn priority(&self) -> PeripheralConnectionSchedulerPriority {
        self.priority
    }

    /// Detach the selected recurring item for the common admission lifecycle.
    pub fn prepare_scheduler_admission(
        self,
    ) -> PeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared {
        let prepared = PeripheralConnectionMemoryGraphPreparedEvent::Recurring(self);
        let prepared = PeripheralConnectionMemoryGraphSchedulerAdmissionCore::new(prepared);
        PeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared { prepared }
    }

    /// Roll back before publication without replacing any persistent owner.
    pub fn cancel(self) -> PeripheralConnectionMemoryGraphActiveCpuOwned {
        self.active
    }
}

/// Reusable CPU-owned connection graphs plus copied event results.
#[must_use = "the allocation and received batch must return to the connection owner"]
pub struct PeripheralConnectionMemoryGraphRecycled {
    graph: PeripheralConnectionMemoryGraphActiveCpuOwned,
    batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: PeripheralConnectionSchedulerItemCompletionStatus,
    capture: PeripheralConnectionCapturedAnchorAvailability,
}

impl PeripheralConnectionMemoryGraphRecycled {
    pub const fn captured_anchor_availability(
        &self,
    ) -> PeripheralConnectionCapturedAnchorAvailability {
        self.capture
    }

    pub fn into_parts(
        self,
    ) -> (
        PeripheralConnectionMemoryGraphActiveCpuOwned,
        LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        PeripheralConnectionSchedulerItemCompletionStatus,
        PeripheralConnectionCapturedAnchorAvailability,
    ) {
        (self.graph, self.batch, self.status, self.capture)
    }
}

impl PeripheralConnectionMemoryGraphStorage {
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<PeripheralConnectionMemoryGraphCpuOwned, PeripheralConnectionMemoryGraphBindFailure>
    {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(PeripheralConnectionMemoryGraphBindFailure::new(
                    storage,
                    PeripheralConnectionMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: PeripheralConnectionMemoryGraphModelAddress,
    ) -> Result<PeripheralConnectionMemoryGraphCpuOwned, PeripheralConnectionMemoryGraphBindFailure>
    {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<PeripheralConnectionMemoryGraphCpuOwned, PeripheralConnectionMemoryGraphBindFailure>
    {
        let identity = PeripheralConnectionMemoryGraphIdentity::for_storage(storage);
        let binding = match PeripheralConnectionMemoryGraphBinding::new(identity, base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(PeripheralConnectionMemoryGraphBindFailure::new(
                    storage, error,
                ));
            }
        };
        let mut owner = PeripheralConnectionMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize_graph();
        Ok(owner)
    }
}

#[cfg(test)]
mod tests;
