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

use core::{num::NonZeroU32, pin::Pin};

use crate::{
    direction_finding_workspace::BluetoothDirectionFindingWorkspaceLink,
    le_rx_packet::{BluetoothLeReceivedBatch, BluetoothLeRxError},
    non_scanning_rx_memory::{
        BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, BluetoothNonScanningRxMemoryCpuOwned,
        BluetoothNonScanningRxMemoryIdentity,
    },
    rx_memory_list::BluetoothRxMemoryListClass,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListSelector, BluetoothRxMemoryListPublished,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerSoftwareListRemovalReady,
};

mod codec;

pub use codec::BluetoothPeripheralConnectionMemoryGraphStorage;
use codec::{
    BluetoothPeripheralConnectionFirstEventCodecInput,
    BluetoothPeripheralConnectionMemoryGraphBinding,
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
        identity: BluetoothPeripheralConnectionIdentity,
    ) -> BluetoothPeripheralConnectionMemoryGraphIdentityPrepared {
        self.storage.as_ref().get_ref().prepare_identity(identity);
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
        self.storage.as_ref().get_ref().identity()
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
        !self.storage.as_ref().get_ref().has_empty_receive_queue() && self.pool.is_initialized()
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
        let graph = self.storage.as_ref().get_ref();
        let input = BluetoothPeripheralConnectionFirstEventCodecInput {
            channel,
            interval,
            window,
            receive_wait,
            default_tx_power,
            priority,
        };
        graph.prepare_reviewed_first_event_fields(&self.binding, self.pool.head(), &input);
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
        self.storage.as_ref().get_ref().clear_receive_pool();
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
        self,
    ) -> BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared {
        self.prepared
            .storage
            .as_ref()
            .get_ref()
            .prepare_scheduler_admission(&self.prepared.binding);
        BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared { prepared: self }
    }

    /// Remove the unpublished workspace link and recover the prior exact state.
    pub fn cancel(self) -> BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared {
        self.prepared
            .storage
            .as_ref()
            .get_ref()
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
        self.prepared.prepared.binding.scheduler_head()
    }

    /// Freeze the complete SRAM graph before selector-two publication.
    pub fn prepare_publication(
        self,
    ) -> BluetoothPeripheralConnectionMemoryGraphPublicationPrepared {
        BluetoothPeripheralConnectionMemoryGraphPublicationPrepared { prepared: self }
    }

    /// Restore the exact private free chain before any MMIO publication.
    pub fn cancel(self) -> BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared {
        self.prepared
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .restore_scheduler_admission(&self.prepared.prepared.binding);
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

    /// Consume one fresh list-zero completion report and inspect the selected item.
    ///
    /// The status word remains private controller SRAM. The in-flight sentinel
    /// retains hardware ownership; any other value advances only to a fenced
    /// completion observation and does not authorize descriptor mutation.
    pub fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothPeripheralConnectionMemoryGraphCompletionObservation {
        if observed.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            return BluetoothPeripheralConnectionMemoryGraphCompletionObservation::ListMismatch {
                running: self,
                observed,
            };
        }
        let Some(status) = self
            .prepared
            .prepared
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .scheduler_completion_status()
        else {
            return BluetoothPeripheralConnectionMemoryGraphCompletionObservation::StillInFlight(
                self,
            );
        };
        BluetoothPeripheralConnectionMemoryGraphCompletionObservation::CompletionObserved(
            BluetoothPeripheralConnectionMemoryGraphCompletionObserved {
                running: self,
                status,
            },
        )
    }
}

/// Opaque interpretation of one non-sentinel connection scheduler status.
///
/// The numeric nonzero value is retained for diagnostics. No Link Layer
/// meaning is assigned until the corresponding controller branch is reviewed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
    Zero,
    NonZero(NonZeroU32),
}

/// One bounded observation of a running connection graph.
#[must_use = "the graph and any unrelated finished-list token remain owned"]
pub enum BluetoothPeripheralConnectionMemoryGraphCompletionObservation {
    ListMismatch {
        running: BluetoothPeripheralConnectionMemoryGraphRunning,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothPeripheralConnectionMemoryGraphRunning),
    CompletionObserved(BluetoothPeripheralConnectionMemoryGraphCompletionObserved),
}

/// Hardware-owned graph after its selected item produced a non-sentinel status.
#[must_use = "the completed connection graph must pass scheduler unlink before CPU access"]
pub struct BluetoothPeripheralConnectionMemoryGraphCompletionObserved {
    running: BluetoothPeripheralConnectionMemoryGraphRunning,
    status: BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
}

impl BluetoothPeripheralConnectionMemoryGraphCompletionObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.running.scheduler_item_address()
    }

    pub const fn status(&self) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.status
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
        BluetoothPeripheralConnectionMemoryGraphRecyclePrepared,
        BluetoothPeripheralConnectionMemoryGraphRecycleFailure,
    > {
        let error = if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(BluetoothPeripheralConnectionMemoryGraphRecycleError::HardwareListMismatch)
        } else if removal.completed_head().address() != Some(self.scheduler_item_address()) {
            Some(BluetoothPeripheralConnectionMemoryGraphRecycleError::SchedulerItemMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(BluetoothPeripheralConnectionMemoryGraphRecycleFailure {
                completed: self,
                removal,
                error,
            });
        }
        Ok(BluetoothPeripheralConnectionMemoryGraphRecyclePrepared {
            completed: self,
            removal,
        })
    }
}

/// Why a completed connection graph rejected CPU-recycle authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPeripheralConnectionMemoryGraphRecycleError {
    HardwareListMismatch,
    SchedulerItemMismatch,
}

/// Lossless recycle rejection retaining the hardware-owned graph and proof.
#[must_use = "the completed connection graph and removal proof remain owned"]
pub struct BluetoothPeripheralConnectionMemoryGraphRecycleFailure {
    completed: BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    error: BluetoothPeripheralConnectionMemoryGraphRecycleError,
}

impl BluetoothPeripheralConnectionMemoryGraphRecycleFailure {
    pub const fn error(&self) -> BluetoothPeripheralConnectionMemoryGraphRecycleError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

/// Completed connection graph authorized for bounded RX extraction.
#[must_use = "the connection RX result must be extracted or retained unchanged"]
pub struct BluetoothPeripheralConnectionMemoryGraphRecyclePrepared {
    completed: BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl BluetoothPeripheralConnectionMemoryGraphRecyclePrepared {
    /// Recover both unchanged owners before extraction starts.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }

    /// Validate and copy every contiguous completed PDU without mutating SRAM.
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc extraction failure retains the complete affine graph"
        )
    )]
    pub fn extract_received(
        self,
    ) -> Result<
        BluetoothPeripheralConnectionMemoryGraphRxExtracted,
        BluetoothPeripheralConnectionMemoryGraphRxExtractionFailure,
    > {
        let batch = match self
            .completed
            .running
            .prepared
            .prepared
            .prepared
            .pool
            .extract_completed_rx_batch()
        {
            Ok(batch) => batch,
            Err(error) => {
                return Err(
                    BluetoothPeripheralConnectionMemoryGraphRxExtractionFailure {
                        prepared: self,
                        error,
                    },
                );
            }
        };
        Ok(BluetoothPeripheralConnectionMemoryGraphRxExtracted {
            prepared: self,
            batch,
        })
    }
}

/// Malformed completed RX storage retaining the unchanged recycle owner.
#[must_use = "the unchanged connection graph remains unavailable until fail-stop handling"]
pub struct BluetoothPeripheralConnectionMemoryGraphRxExtractionFailure {
    prepared: BluetoothPeripheralConnectionMemoryGraphRecyclePrepared,
    error: BluetoothLeRxError,
}

impl BluetoothPeripheralConnectionMemoryGraphRxExtractionFailure {
    pub const fn error(&self) -> BluetoothLeRxError {
        self.error
    }

    pub fn into_prepared(self) -> BluetoothPeripheralConnectionMemoryGraphRecyclePrepared {
        self.prepared
    }
}

/// Copied RX batch paired with the sole reclaimable connection graph.
#[must_use = "commit reclamation before reusing the connection allocation"]
pub struct BluetoothPeripheralConnectionMemoryGraphRxExtracted {
    prepared: BluetoothPeripheralConnectionMemoryGraphRecyclePrepared,
    batch: BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
}

impl BluetoothPeripheralConnectionMemoryGraphRxExtracted {
    /// Copy of every completed Link Layer PDU in receive-list order.
    pub const fn batch(&self) -> BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        self.batch
    }

    /// Recover the unchanged recycle proof before reclamation is committed.
    #[doc(hidden)]
    pub fn into_prepared(self) -> BluetoothPeripheralConnectionMemoryGraphRecyclePrepared {
        self.prepared
    }

    /// Restore both private graphs and return ordinary CPU ownership.
    pub fn commit(self) -> BluetoothPeripheralConnectionMemoryGraphRecycled {
        let BluetoothPeripheralConnectionMemoryGraphRecyclePrepared {
            completed,
            removal: _,
        } = self.prepared;
        let BluetoothPeripheralConnectionMemoryGraphCompletionObserved { running, status } =
            completed;
        let BluetoothPeripheralConnectionMemoryGraphRunning {
            prepared,
            _rx_publication: _,
        } = running;
        let BluetoothPeripheralConnectionMemoryGraphSchedulerAdmissionPrepared { prepared } =
            prepared;
        let BluetoothPeripheralConnectionMemoryGraphDirectionFindingPrepared {
            prepared,
            workspace: _,
        } = prepared;
        let BluetoothPeripheralConnectionMemoryGraphEventFieldsPrepared {
            storage,
            binding,
            pool,
            channel: _,
            interval: _,
            window: _,
            receive_wait: _,
            default_tx_power: _,
            priority: _,
        } = prepared;
        let mut graph = BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned {
            storage,
            binding,
            pool,
        };
        graph.restore_after_event();
        BluetoothPeripheralConnectionMemoryGraphRecycled {
            graph,
            batch: self.batch,
            status,
        }
    }
}

/// CPU owner of a live connection graph between recurring radio events.
///
/// Unlike the cold allocation owner, this state preserves the link-state
/// words updated by hardware. It restores only the detached scheduler item
/// and receive rotation which the completed event exclusively owned.
#[must_use = "the active connection allocation must reach recurrence or teardown"]
pub struct BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned {
    storage: Pin<&'static mut BluetoothPeripheralConnectionMemoryGraphStorage>,
    binding: BluetoothPeripheralConnectionMemoryGraphBinding,
    pool: BluetoothNonScanningRxMemoryCpuOwned,
}

impl BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned {
    pub const fn identity(&self) -> BluetoothPeripheralConnectionMemoryGraphIdentity {
        self.binding.identity()
    }

    pub const fn receive_identity(&self) -> BluetoothNonScanningRxMemoryIdentity {
        self.pool.identity()
    }

    /// Whether the event-local scheduler item and RX pool are reusable.
    pub fn event_resources_are_recycled(&self) -> bool {
        self.storage
            .as_ref()
            .get_ref()
            .event_resources_are_recycled(&self.binding)
            && self.pool.is_initialized()
    }

    fn restore_after_event(&mut self) {
        self.storage
            .as_ref()
            .get_ref()
            .restore_scheduler_admission(&self.binding);
        self.pool.reinitialize_after_event();
    }
}

/// Reusable CPU-owned connection graphs plus copied event results.
#[must_use = "the allocation and received batch must return to the connection owner"]
pub struct BluetoothPeripheralConnectionMemoryGraphRecycled {
    graph: BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned,
    batch: BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
}

impl BluetoothPeripheralConnectionMemoryGraphRecycled {
    pub fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned,
        BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
    ) {
        (self.graph, self.batch, self.status)
    }
}

impl BluetoothPeripheralConnectionMemoryGraphStorage {
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

#[cfg(test)]
mod tests {
    use core::num::NonZeroU32;

    use open_esp_radio_esp32s31_hal::{
        BluetoothRxMemoryListPublished, BluetoothSchedulerFinishedListObservation,
        BluetoothSchedulerFinishedListPop, BluetoothSchedulerHardwareListHead,
        BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothSchedulerHardwareListIndex,
        BluetoothSchedulerSoftwareListRemovalReady,
    };

    use super::{
        BluetoothPeripheralConnectionDataChannel, BluetoothPeripheralConnectionDefaultTxPowerDbm,
        BluetoothPeripheralConnectionIdentity, BluetoothPeripheralConnectionIntervalTicks,
        BluetoothPeripheralConnectionMemoryGraphBindError,
        BluetoothPeripheralConnectionMemoryGraphCompletionObservation,
        BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphRecycleError,
        BluetoothPeripheralConnectionMemoryGraphRunning,
        BluetoothPeripheralConnectionMemoryGraphStorage, BluetoothPeripheralConnectionReceiveWait,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
        BluetoothPeripheralConnectionSchedulerPriority,
        BluetoothPeripheralConnectionSchedulerWindow,
    };
    use crate::{
        BluetoothDirectionFindingWorkspaceModelAddress, BluetoothDirectionFindingWorkspaceStorage,
        BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage,
        BluetoothRxMemoryListClass,
    };

    fn storage() -> &'static mut BluetoothPeripheralConnectionMemoryGraphStorage {
        std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ))
    }

    fn completed_graph(
        graph_base: u32,
        status: u32,
    ) -> BluetoothPeripheralConnectionMemoryGraphCompletionObserved {
        let owner = BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(
            storage(),
            BluetoothPeripheralConnectionMemoryGraphModelAddress::new(graph_base)
                .expect("the model graph address is controller-encodable"),
        )
        .expect("the connection graph fits controller SRAM");
        let receive_storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothNonScanningRxMemoryStorage::new(),
        ));
        let receive_pool = BluetoothNonScanningRxMemoryStorage::pin_static_model(
            receive_storage,
            BluetoothNonScanningRxMemoryModelAddress::new(graph_base + 0x1000)
                .expect("the model RX address is controller-encodable"),
        )
        .expect("the receive graph fits controller SRAM");
        let workspace_storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothDirectionFindingWorkspaceStorage::new(),
        ));
        let workspace = BluetoothDirectionFindingWorkspaceStorage::pin_static_model(
            workspace_storage,
            BluetoothDirectionFindingWorkspaceModelAddress::new(graph_base + 0x2000)
                .expect("the model workspace address is controller-encodable"),
        )
        .expect("the direction-finding workspace fits controller SRAM");
        let prepared = owner
            .prepare_identity(BluetoothPeripheralConnectionIdentity::new(
                [0xd4, 0xc3, 0xb2, 0xa1],
                [0x33, 0x22, 0x11],
            ))
            .attach_receive_pool(receive_pool)
            .prepare_reviewed_first_event_fields(
                BluetoothPeripheralConnectionDataChannel::new(0)
                    .expect("data channel zero is valid"),
                BluetoothPeripheralConnectionIntervalTicks::new(24_000)
                    .expect("the connection interval is nonzero"),
                BluetoothPeripheralConnectionSchedulerWindow::new(100, 200)
                    .expect("the scheduler window is nonempty"),
                BluetoothPeripheralConnectionReceiveWait::new(1_250, 16)
                    .expect("the first receive wait fits its short form"),
                BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
                BluetoothPeripheralConnectionSchedulerPriority::FIRST_EVENT,
            )
            .install_direction_finding_workspace(workspace.binding().link())
            .prepare_scheduler_admission();
        let scheduler_item_address = prepared.scheduler_head();
        prepared
            .prepared
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .model_controller_status(status);
        let running = BluetoothPeripheralConnectionMemoryGraphRunning {
            prepared,
            _rx_publication: BluetoothRxMemoryListPublished::from_parts_for_validation(
                BluetoothRxMemoryListClass::NonScanning.selector(),
                scheduler_item_address,
            ),
        };
        let observation =
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0])
                .expect("list zero is representable");
        let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest()
        else {
            panic!("the semantic observation contains list zero")
        };
        match running.observe_completion(observed) {
            BluetoothPeripheralConnectionMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the non-sentinel status completes the model event"),
        }
    }

    fn removal_ready(
        index: BluetoothSchedulerHardwareListIndex,
        address: open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
    ) -> BluetoothSchedulerSoftwareListRemovalReady {
        let head = BluetoothSchedulerHardwareListHead::from_address(address)
            .expect("the connection item forms a nonempty scheduler head");
        let empty = BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(
            index, head,
        );
        BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(empty)
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

    #[test]
    fn scheduler_status_separates_in_flight_from_opaque_completion() {
        let storage = BluetoothPeripheralConnectionMemoryGraphStorage::new();

        storage.model_controller_status(u32::MAX);
        assert_eq!(storage.scheduler_completion_status(), None);

        storage.model_controller_status(0);
        assert_eq!(
            storage.scheduler_completion_status(),
            Some(BluetoothPeripheralConnectionSchedulerItemCompletionStatus::Zero)
        );

        let opaque = NonZeroU32::new(7).expect("the fixture status is nonzero");
        storage.model_controller_status(opaque.get());
        assert_eq!(
            storage.scheduler_completion_status(),
            Some(BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero(opaque))
        );
    }

    #[test]
    fn recycle_rejects_a_foreign_item_without_mutating_the_connection() {
        let completed = completed_graph(0x2f00_5000, 7);
        let address = completed.scheduler_item_address();
        let foreign =
            open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(address.address() + 4)
                .expect("the adjacent model address remains controller-encodable");
        let failure = match completed.prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            foreign,
        )) {
            Ok(_) => panic!("a removal proof for another item must be rejected"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothPeripheralConnectionMemoryGraphRecycleError::SchedulerItemMismatch
        );
        let (completed, _) = failure.into_parts();
        assert_eq!(completed.scheduler_item_address(), address);
        assert_eq!(
            completed.status(),
            BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero(
                NonZeroU32::new(7).expect("seven is nonzero")
            )
        );
    }

    #[test]
    fn recycle_restores_event_resources_without_resetting_the_active_owner() {
        let completed = completed_graph(0x2f00_9000, 0);
        let address = completed.scheduler_item_address();
        let prepared = completed
            .prepare_recycle_after_software_list_removal(removal_ready(
                BluetoothSchedulerHardwareListIndex::ZERO,
                address,
            ))
            .unwrap_or_else(|_| panic!("the exact removal proof authorizes reclamation"));
        let extracted = prepared
            .extract_received()
            .unwrap_or_else(|_| panic!("an event without a received packet is valid"));
        assert!(extracted.batch().is_empty());

        let (active, batch, status) = extracted.commit().into_parts();

        assert!(active.event_resources_are_recycled());
        assert!(batch.is_empty());
        assert_eq!(
            status,
            BluetoothPeripheralConnectionSchedulerItemCompletionStatus::Zero
        );
    }
}
