//! CPU-owned ESP32-S31 memory for one response-capable legacy advertisement.
//!
//! This boundary binds the two transmit PDUs and the reusable non-scanning
//! receive pool into one private graph, prepares the sole scheduler item, and
//! consumes the exact RX-list, scheduler-head, and RUN proof tokens produced by
//! the chip layers. This crate performs no MMIO and exposes no publication
//! operation of its own.

#![forbid(unsafe_code)]

mod codec;

use core::{marker::PhantomPinned, pin::Pin};

use crate::{
    BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, LeReceivedBatch, LeRxError,
    LegacyAdvertisingPrimaryChannel, NonScanningRxMemoryCpuOwned, NonScanningRxMemoryIdentity,
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxPacketPreparedInput, LeTxPacketPreparedLength,
    },
    legacy_advertising_event_image::LegacyAdvertisingOwnAddress,
    rx_memory_list::RxMemoryListClass,
};

use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothSchedulerFinishedHardwareListObserved,
        BluetoothSchedulerHardwareListHeadPublished, BluetoothSchedulerHardwareListIndex,
        BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerSoftwareListRemovalReady,
        RxMemoryListPublished,
    },
    types::{
        BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
        BluetoothMemoryListSelector,
    },
};

use pin_project::pin_project;

use self::codec::{
    LegacyConnectableAdvertisingGraphBinding, LegacyConnectableAdvertisingGraphStorage,
    LegacyConnectableAdvertisingSchedulerBookkeepingSnapshot,
    LegacyConnectableAdvertisingSoftwareLinkSnapshot,
};

const LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = 37;
const LEGACY_ADVERTISING_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

type AdvertisingTxPacketLength = LeTxPacketPreparedLength<LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketInput<'a> = LeTxPacketPreparedInput<'a, LEGACY_ADVERTISING_TX_PACKET_BYTES>;

/// Why an encoded advertising PDU cannot fit this S31 allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingPduFitError {
    /// Address insertion requires the complete six-byte advertiser address.
    AdvertiserAddressMissing { payload_bytes: usize },
    /// The complete encoded extent disagrees with the trusted payload length.
    EncodedExtentMismatch {
        expected_bytes: usize,
        actual_bytes: usize,
    },
    /// The payload exceeds the reviewed legacy-advertising allocation class.
    PayloadExceedsAllocation {
        payload_bytes: usize,
        capacity: usize,
    },
}

fn allocation_checked_packet(
    pdu: &[u8],
    payload_bytes: u8,
) -> Result<AdvertisingTxPacketInput<'_>, LegacyConnectableAdvertisingPduFitError> {
    let payload_bytes = usize::from(payload_bytes);
    if payload_bytes > LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES {
        return Err(
            LegacyConnectableAdvertisingPduFitError::PayloadExceedsAllocation {
                payload_bytes,
                capacity: LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES,
            },
        );
    }
    let expected_bytes = 2 + payload_bytes;
    if pdu.len() != expected_bytes {
        return Err(
            LegacyConnectableAdvertisingPduFitError::EncodedExtentMismatch {
                expected_bytes,
                actual_bytes: pdu.len(),
            },
        );
    }
    if payload_bytes < 6 {
        return Err(
            LegacyConnectableAdvertisingPduFitError::AdvertiserAddressMissing { payload_bytes },
        );
    }
    Ok(AdvertisingTxPacketInput::from_validated_encoded_pdu(
        pdu,
        payload_bytes as u8,
    ))
}

/// Allocation-fit projection of one protocol-validated `ADV_IND` PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvIndPacketInput<'a>(AdvertisingTxPacketInput<'a>);

impl<'a> LegacyConnectableAdvIndPacketInput<'a> {
    /// Check only S31 allocation fit; the caller owns protocol validity.
    pub fn try_from_encoded_extent(
        pdu: &'a [u8],
        payload_bytes: u8,
    ) -> Result<Self, LegacyConnectableAdvertisingPduFitError> {
        allocation_checked_packet(pdu, payload_bytes).map(Self)
    }
}

/// Allocation-fit projection of one protocol-validated `SCAN_RSP` PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableScanResponsePacketInput<'a>(AdvertisingTxPacketInput<'a>);

impl<'a> LegacyConnectableScanResponsePacketInput<'a> {
    /// Check only S31 allocation fit; the caller owns protocol validity.
    pub fn try_from_encoded_extent(
        pdu: &'a [u8],
        payload_bytes: u8,
    ) -> Result<Self, LegacyConnectableAdvertisingPduFitError> {
        allocation_checked_packet(pdu, payload_bytes).map(Self)
    }
}

/// Address behavior already selected by the chip protocol bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingOwnAddress {
    Public,
    Random([u8; 6]),
}

impl LegacyConnectableAdvertisingOwnAddress {
    const fn codec(self) -> LegacyAdvertisingOwnAddress {
        match self {
            Self::Public => LegacyAdvertisingOwnAddress::Public,
            Self::Random(address) => LegacyAdvertisingOwnAddress::Random(address),
        }
    }
}

/// Complete allocation-fit input needed to lower one one-channel event.
///
/// This value contains no portable Link Layer owner. Its PDU wrappers check
/// only controller-allocation fit and never parse Bluetooth header semantics;
/// the chip protocol bridge must retain the corresponding validated LL owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvertisingMemoryInput<'a> {
    adv_ind: LegacyConnectableAdvIndPacketInput<'a>,
    scan_response: LegacyConnectableScanResponsePacketInput<'a>,
    own_address: LegacyConnectableAdvertisingOwnAddress,
    primary_channel: LegacyAdvertisingPrimaryChannel,
}

impl<'a> LegacyConnectableAdvertisingMemoryInput<'a> {
    pub const fn new(
        adv_ind: LegacyConnectableAdvIndPacketInput<'a>,
        scan_response: LegacyConnectableScanResponsePacketInput<'a>,
        own_address: LegacyConnectableAdvertisingOwnAddress,
        primary_channel: LegacyAdvertisingPrimaryChannel,
    ) -> Self {
        Self {
            adv_ind,
            scan_response,
            own_address,
            primary_channel,
        }
    }
}

/// Vendor-derived duration after the nominal advertising anchor.
///
/// The duration contains the ADV_IND LE 1M airtime and the opaque four-
/// microsecond response-capable item tail. It excludes the scheduler
/// preparation lead, so it is not the complete `END - START` reservation and
/// does not claim that the RF response window has ended. Controller ownership
/// must remain exclusive until a separately observed terminal completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvertisingPostAnchorDuration(u32);

impl LegacyConnectableAdvertisingPostAnchorDuration {
    /// Post-anchor duration in microseconds before controller-epoch projection.
    pub const fn as_micros(self) -> u32 {
        self.0
    }
}

/// Stable pinned allocation for one response-capable advertising graph.
#[repr(C)]
#[pin_project]
pub struct LegacyConnectableAdvertisingMemoryGraphStorage {
    graph: LegacyConnectableAdvertisingGraphStorage,
    #[pin]
    _pin: PhantomPinned,
}

/// Opaque identity of one exact pinned connectable-advertising allocation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LegacyConnectableAdvertisingMemoryGraphIdentity(usize);

impl LegacyConnectableAdvertisingMemoryGraphIdentity {
    fn for_storage(storage: &LegacyConnectableAdvertisingMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvertisingMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl LegacyConnectableAdvertisingMemoryGraphModelAddress {
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

/// Why static connectable-advertising storage cannot be bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingMemoryGraphBindError {
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
    InvalidPacketExtent,
}

/// Failed binding retaining the exact unchanged static allocation.
pub struct LegacyConnectableAdvertisingMemoryGraphBindFailure {
    storage: &'static mut LegacyConnectableAdvertisingMemoryGraphStorage,
    error: LegacyConnectableAdvertisingMemoryGraphBindError,
}

impl LegacyConnectableAdvertisingMemoryGraphBindFailure {
    pub const fn error(&self) -> LegacyConnectableAdvertisingMemoryGraphBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut LegacyConnectableAdvertisingMemoryGraphStorage,
        LegacyConnectableAdvertisingMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Unique CPU owner before response-capable event preparation.
#[must_use = "the connectable-advertising graph must be retained"]
pub struct LegacyConnectableAdvertisingMemoryGraphCpuOwned {
    storage: Pin<&'static mut LegacyConnectableAdvertisingMemoryGraphStorage>,
    binding: LegacyConnectableAdvertisingGraphBinding,
}

impl LegacyConnectableAdvertisingMemoryGraphCpuOwned {
    pub const fn identity(&self) -> LegacyConnectableAdvertisingMemoryGraphIdentity {
        self.binding.identity()
    }

    fn reinitialize_graph(&mut self) {
        self.storage
            .as_mut()
            .project()
            .graph
            .initialize_graph(&self.binding);
    }

    /// Bind both response PDUs and one reusable RX pool without publication.
    pub fn prepare_response_capable_event(
        mut self,
        input: LegacyConnectableAdvertisingMemoryInput<'_>,
        pool: NonScanningRxMemoryCpuOwned,
        default_tx_power_dbm: i8,
    ) -> Result<
        LegacyConnectableAdvertisingMemoryGraphPrepared,
        LegacyConnectableAdvertisingMemoryGraphPrepareFailure,
    > {
        if !pool.is_initialized() {
            return Err(LegacyConnectableAdvertisingMemoryGraphPrepareFailure::new(
                self,
                pool,
                LegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolNotReady,
            ));
        }
        if !self.binding.is_disjoint_from_receive_pool(&pool) {
            return Err(LegacyConnectableAdvertisingMemoryGraphPrepareFailure::new(
                self,
                pool,
                LegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolOverlapsGraph,
            ));
        }

        let (adv_ind_length, scan_response_length) = self
            .storage
            .as_mut()
            .project()
            .graph
            .prepare_pdus(input.adv_ind.0, input.scan_response.0);
        let post_anchor_duration =
            codec::response_capable_post_anchor_duration(input.adv_ind.0.payload_bytes());
        self.storage.as_ref().get_ref().graph.prepare_profile(
            &self.binding,
            pool.current_cursor(),
            pool.tail(),
            input.own_address.codec(),
            default_tx_power_dbm,
        );

        Ok(LegacyConnectableAdvertisingMemoryGraphPrepared {
            storage: self.storage,
            binding: self.binding,
            pool,
            adv_ind_length,
            scan_response_length,
            primary_channel: input.primary_channel,
            post_anchor_duration,
        })
    }
}

/// Prepared response-capable graph with no publication authority.
#[must_use = "the prepared graph and receive pool must be retained or cancelled"]
pub struct LegacyConnectableAdvertisingMemoryGraphPrepared {
    storage: Pin<&'static mut LegacyConnectableAdvertisingMemoryGraphStorage>,
    binding: LegacyConnectableAdvertisingGraphBinding,
    pool: NonScanningRxMemoryCpuOwned,
    adv_ind_length: AdvertisingTxPacketLength,
    scan_response_length: AdvertisingTxPacketLength,
    primary_channel: LegacyAdvertisingPrimaryChannel,
    post_anchor_duration: LegacyConnectableAdvertisingPostAnchorDuration,
}

impl LegacyConnectableAdvertisingMemoryGraphPrepared {
    pub const fn identity(&self) -> LegacyConnectableAdvertisingMemoryGraphIdentity {
        self.binding.identity()
    }

    pub const fn receive_identity(&self) -> NonScanningRxMemoryIdentity {
        self.pool.identity()
    }

    pub const fn primary_channel(&self) -> LegacyAdvertisingPrimaryChannel {
        self.primary_channel
    }

    pub const fn post_anchor_duration(&self) -> LegacyConnectableAdvertisingPostAnchorDuration {
        self.post_anchor_duration
    }

    pub fn adv_ind_pdu(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .graph
            .adv_ind_pdu(self.adv_ind_length)
    }

    pub fn scan_response_pdu(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .graph
            .scan_response_pdu(self.scan_response_length)
    }

    /// Lower the sole selected channel into one CPU-owned scheduler item.
    ///
    /// `raw_start` and `raw_end` must come from the chip scheduler's accepted
    /// controller-epoch window. This crate stores them but does not interpret
    /// controller time or reserve a common timeline slot. `raw_sequence_lead`
    /// is the same accepted reservation policy used for sequence authorization.
    pub fn prepare_event_fields(
        self,
        raw_start: u32,
        raw_end: u32,
        raw_sequence_lead: u32,
    ) -> Result<
        LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
        LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure,
    > {
        if let Err(error) = self.storage.as_ref().get_ref().graph.prepare_event_fields(
            &self.binding,
            self.primary_channel,
            raw_start,
            raw_end,
            raw_sequence_lead,
        ) {
            return Err(
                LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure {
                    owner: self,
                    error,
                },
            );
        }
        Ok(LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared { prepared: self })
    }

    /// Whether all private TX and RX links still form the prepared topology.
    pub fn is_ready_for_scheduler_lowering(&self) -> bool {
        self.pool.is_initialized()
            && self
                .storage
                .as_ref()
                .get_ref()
                .graph
                .retains_prepared_graph(&self.binding, self.pool.current_cursor(), self.pool.tail())
    }

    /// Remove every unpublished role image and recover both exact owners.
    pub fn cancel(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        NonScanningRxMemoryCpuOwned,
    ) {
        let mut owner = LegacyConnectableAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        (owner, self.pool)
    }
}

/// Why the sole connectable-advertising scheduler item was not writable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError {
    /// The response graph no longer retains its private scheduler-item head.
    SchedulerHeadMismatch,
    /// The sole scheduler item already points at another hardware item.
    NonTerminalSchedulerItem,
}

/// Failed event-field lowering retaining the exact prepared graph and RX pool.
#[must_use = "the unchanged prepared graph and receive pool remain owned"]
pub struct LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure {
    owner: LegacyConnectableAdvertisingMemoryGraphPrepared,
    error: LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
}

impl LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure {
    pub const fn error(&self) -> LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphPrepared,
        LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Complete one-item event fields before common scheduler bookkeeping.
#[must_use = "the event fields must advance, be cancelled, or remain retained"]
pub struct LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared {
    prepared: LegacyConnectableAdvertisingMemoryGraphPrepared,
}

impl LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared {
    /// Exact CPU-owned item eligible for later common-list admission.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding.scheduler_item_address()
    }

    /// Install the common status sentinel and completed-list baseline.
    pub fn prepare_scheduler_bookkeeping(
        self,
    ) -> LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
        let previous = self
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .prepare_scheduler_bookkeeping();
        LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
            event: self,
            previous,
        }
    }

    /// Restore the prepared response graph before any common-list admission.
    pub fn cancel(self) -> LegacyConnectableAdvertisingMemoryGraphPrepared {
        self.prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .restore_event_fields(&self.prepared.binding);
        self.prepared
    }
}

/// One-item event with common scheduler bookkeeping but no list ownership.
#[must_use = "the scheduler-prepared graph must advance or be cancelled"]
pub struct LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
    event: LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
    previous: LegacyConnectableAdvertisingSchedulerBookkeepingSnapshot,
}

impl LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_item_address()
    }

    /// Clear the software-list successor before empty-list head publication.
    pub fn prepare_empty_list_link(
        self,
    ) -> LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
        let previous_software_link = self
            .event
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .prepare_empty_list_link();
        LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
            bookkeeping: self,
            previous_software_link,
        }
    }

    /// Restore the exact scheduler fields observed before bookkeeping.
    pub fn cancel(self) -> LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared {
        self.event
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .restore_scheduler_bookkeeping(self.previous);
        self.event
    }
}

/// One-item graph with a null software successor and full rollback authority.
#[must_use = "the empty-list candidate must be published or cancelled"]
pub struct LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
    bookkeeping: LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    previous_software_link: LegacyConnectableAdvertisingSoftwareLinkSnapshot,
}

impl LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.bookkeeping.scheduler_item_address()
    }

    /// Freeze the complete CPU-owned graph before RX-list publication.
    pub fn prepare_publication(self) -> LegacyConnectableAdvertisingMemoryGraphPublicationPrepared {
        LegacyConnectableAdvertisingMemoryGraphPublicationPrepared { prepared: self }
    }

    /// Restore the exact software-list successor without disturbing bookkeeping.
    pub fn cancel(self) -> LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
        self.bookkeeping
            .event
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .restore_empty_list_link(self.previous_software_link);
        self.bookkeeping
    }
}

/// Complete response-capable graph ready for selector-two RX-list publication.
#[must_use = "the prepared graph must be published, cancelled, or retained"]
pub struct LegacyConnectableAdvertisingMemoryGraphPublicationPrepared {
    prepared: LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared,
}

impl LegacyConnectableAdvertisingMemoryGraphPublicationPrepared {
    /// Stable identity retained across the publication boundary.
    pub const fn identity(&self) -> LegacyConnectableAdvertisingMemoryGraphIdentity {
        self.prepared.bookkeeping.event.prepared.identity()
    }

    /// Stable receive-pool identity retained across the publication boundary.
    pub const fn receive_identity(&self) -> NonScanningRxMemoryIdentity {
        self.prepared.bookkeeping.event.prepared.receive_identity()
    }

    /// Memory-layer mapping for an ordinary non-scanning advertising item.
    #[doc(hidden)]
    pub const fn selector(&self) -> BluetoothMemoryListSelector {
        RxMemoryListClass::NonScanning.selector()
    }

    /// Validated receive header retained by this affine graph.
    #[doc(hidden)]
    pub const fn receive_head(&self) -> BluetoothControllerSramAddress {
        self.prepared
            .bookkeeping
            .event
            .prepared
            .pool
            .current_cursor()
    }

    /// Exact event item retained for the later scheduler-head publication.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.scheduler_item_address()
    }

    /// Consume the matching RX-list publication and surrender CPU rollback.
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
        LegacyConnectableAdvertisingMemoryGraphRxPublished,
        LegacyConnectableAdvertisingMemoryGraphPublicationMismatch,
    > {
        let error = if publication.selector() != self.selector() {
            Some(LegacyConnectableAdvertisingMemoryGraphPublicationError::SelectorMismatch)
        } else if publication.head() != self.receive_head() {
            Some(LegacyConnectableAdvertisingMemoryGraphPublicationError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(LegacyConnectableAdvertisingMemoryGraphPublicationMismatch {
                prepared: self,
                publication,
                error,
            });
        }

        let LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
            bookkeeping,
            previous_software_link: _,
        } = self.prepared;
        let LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
            event,
            previous: _,
        } = bookkeeping;
        let LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared { prepared } = event;
        Ok(LegacyConnectableAdvertisingMemoryGraphRxPublished {
            prepared,
            rx_publication: publication,
        })
    }

    /// Recover the exact empty-list candidate before any hardware publication.
    pub fn cancel(self) -> LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
        self.prepared
    }
}

/// Why an RX-list publication does not name this response-capable graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingMemoryGraphPublicationError {
    /// The publication belongs to another positional RX memory list.
    SelectorMismatch,
    /// The publication names another pinned receive pool.
    HeadMismatch,
}

/// Failed RX-list proof join retaining the CPU graph and HAL publication.
#[must_use = "a mismatched publication still owns the graph and HAL token"]
pub struct LegacyConnectableAdvertisingMemoryGraphPublicationMismatch {
    prepared: LegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
    publication: RxMemoryListPublished,
    error: LegacyConnectableAdvertisingMemoryGraphPublicationError,
}

impl LegacyConnectableAdvertisingMemoryGraphPublicationMismatch {
    pub const fn error(&self) -> LegacyConnectableAdvertisingMemoryGraphPublicationError {
        self.error
    }

    /// Recover both exact affine owners without changing either identity.
    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
        RxMemoryListPublished,
    ) {
        (self.prepared, self.publication)
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphPublicationMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphPublicationMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Graph whose exact non-scanning RX list is hardware-visible.
#[must_use = "the RX-published graph must enter the primary scheduler list"]
pub struct LegacyConnectableAdvertisingMemoryGraphRxPublished {
    prepared: LegacyConnectableAdvertisingMemoryGraphPrepared,
    rx_publication: RxMemoryListPublished,
}

impl LegacyConnectableAdvertisingMemoryGraphRxPublished {
    /// Exact scheduler item paired with this receive-list publication.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding.scheduler_item_address()
    }

    /// Borrow the retained non-scanning RX-list publication proof.
    #[doc(hidden)]
    pub const fn rx_publication(&self) -> &RxMemoryListPublished {
        &self.rx_publication
    }

    /// Join the exact list-zero scheduler-head proof.
    #[doc(hidden)]
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "a mismatch retains the complete hardware-owned graph"
        )
    )]
    pub fn into_head_published(
        self,
        publication: &BluetoothSchedulerHardwareListHeadPublished,
    ) -> Result<
        LegacyConnectableAdvertisingMemoryGraphHeadPublished,
        LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch,
    > {
        let error = if publication.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(LegacyConnectableAdvertisingMemoryGraphSchedulerProofError::ListMismatch)
        } else if publication.head().address() != Some(self.scheduler_head()) {
            Some(LegacyConnectableAdvertisingMemoryGraphSchedulerProofError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(
                LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch {
                    published: self,
                    error,
                },
            );
        }
        Ok(LegacyConnectableAdvertisingMemoryGraphHeadPublished {
            prepared: self.prepared,
            rx_publication: self.rx_publication,
        })
    }
}

/// Why a scheduler HEAD or RUN proof cannot join this exact graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingMemoryGraphSchedulerProofError {
    /// The proof belongs to another hardware scheduler list.
    ListMismatch,
    /// The proof retains another scheduler-item head.
    HeadMismatch,
}

/// Failed scheduler-head proof join retaining the RX-published graph.
#[must_use = "a mismatched scheduler-head proof still leaves the graph hardware-owned"]
pub struct LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch {
    published: LegacyConnectableAdvertisingMemoryGraphRxPublished,
    error: LegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
}

impl LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch {
    pub const fn error(&self) -> LegacyConnectableAdvertisingMemoryGraphSchedulerProofError {
        self.error
    }

    /// Recover the unchanged hardware-owned graph and finite mismatch reason.
    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphRxPublished,
        LegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
    ) {
        (self.published, self.error)
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Response-capable graph visible through both RX and scheduler list heads.
#[must_use = "the head-published graph must reach RUN or remain fail-stop owned"]
pub struct LegacyConnectableAdvertisingMemoryGraphHeadPublished {
    prepared: LegacyConnectableAdvertisingMemoryGraphPrepared,
    rx_publication: RxMemoryListPublished,
}

impl LegacyConnectableAdvertisingMemoryGraphHeadPublished {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding.scheduler_item_address()
    }

    /// Borrow the exact RX publication retained by the graph.
    #[doc(hidden)]
    pub const fn rx_publication(&self) -> &RxMemoryListPublished {
        &self.rx_publication
    }

    /// Consume the complete matching scheduler RUN proof.
    #[doc(hidden)]
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "a mismatch retains the complete head-published graph"
        )
    )]
    pub fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> Result<
        LegacyConnectableAdvertisingMemoryGraphRunning,
        LegacyConnectableAdvertisingMemoryGraphRunMismatch,
    > {
        let error = if run.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(LegacyConnectableAdvertisingMemoryGraphSchedulerProofError::ListMismatch)
        } else if run.head().address() != Some(self.scheduler_item_address()) {
            Some(LegacyConnectableAdvertisingMemoryGraphSchedulerProofError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(LegacyConnectableAdvertisingMemoryGraphRunMismatch {
                published: self,
                error,
            });
        }
        Ok(LegacyConnectableAdvertisingMemoryGraphRunning {
            prepared: self.prepared,
            _rx_publication: self.rx_publication,
        })
    }
}

/// Failed scheduler RUN proof join retaining the head-published graph.
#[must_use = "a mismatched RUN proof still leaves the graph hardware-owned"]
pub struct LegacyConnectableAdvertisingMemoryGraphRunMismatch {
    published: LegacyConnectableAdvertisingMemoryGraphHeadPublished,
    error: LegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
}

impl LegacyConnectableAdvertisingMemoryGraphRunMismatch {
    pub const fn error(&self) -> LegacyConnectableAdvertisingMemoryGraphSchedulerProofError {
        self.error
    }

    /// Recover the unchanged graph and finite mismatch reason.
    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphHeadPublished,
        LegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
    ) {
        (self.published, self.error)
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphRunMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphRunMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Hardware-owned response-capable graph admitted through the scheduler RUN transaction.
#[must_use = "the running graph awaits a separately reviewed completion lifecycle"]
pub struct LegacyConnectableAdvertisingMemoryGraphRunning {
    prepared: LegacyConnectableAdvertisingMemoryGraphPrepared,
    _rx_publication: RxMemoryListPublished,
}

impl LegacyConnectableAdvertisingMemoryGraphRunning {
    /// Stable graph identity retained while hardware owns the event.
    pub const fn identity(&self) -> LegacyConnectableAdvertisingMemoryGraphIdentity {
        self.prepared.identity()
    }

    /// Stable receive-pool identity retained while hardware owns the event.
    pub const fn receive_identity(&self) -> NonScanningRxMemoryIdentity {
        self.prepared.receive_identity()
    }

    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding.scheduler_item_address()
    }

    /// Consume one fenced finished-list observation and inspect the sole item.
    pub fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> LegacyConnectableAdvertisingMemoryGraphCompletionObservation {
        if observed.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            return LegacyConnectableAdvertisingMemoryGraphCompletionObservation::ListMismatch {
                running: self,
                observed,
            };
        }
        let Some(status) = self
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .completion_status()
        else {
            return LegacyConnectableAdvertisingMemoryGraphCompletionObservation::StillInFlight(
                self,
            );
        };
        LegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
            LegacyConnectableAdvertisingMemoryGraphCompletionObserved {
                running: self,
                status,
            },
        )
    }

    #[cfg(test)]
    fn model_controller_completion(
        &self,
        status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    ) {
        self.prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .model_controller_completion(status);
    }

    #[cfg(test)]
    fn model_controller_receive(&self, index: usize, pdu: &[u8], rssi_dbm: i8, captured_time: u32) {
        self.prepared
            .pool
            .model_controller_receive(index, pdu, rssi_dbm, captured_time);
    }

    #[cfg(test)]
    fn model_controller_discard(&self, index: usize) {
        self.prepared.pool.model_controller_discard(index);
    }
}

/// Opaque category of one non-sentinel connectable-advertising item status.
///
/// Zero versus nonzero remains diagnostic and does not classify an RX PDU or
/// authorize portable `CONNECT_IND` admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
    Zero,
    /// Exact opaque hardware value, retained only for diagnosis.
    NonZero(u32),
}

/// One bounded observation of the hardware-owned response-capable graph.
#[must_use = "the graph and any unrelated finished-list proof remain owned"]
pub enum LegacyConnectableAdvertisingMemoryGraphCompletionObservation {
    ListMismatch {
        running: LegacyConnectableAdvertisingMemoryGraphRunning,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(LegacyConnectableAdvertisingMemoryGraphRunning),
    CompletionObserved(LegacyConnectableAdvertisingMemoryGraphCompletionObserved),
}

/// Hardware-owned graph after its sole item produced a non-sentinel status.
#[must_use = "the completed graph must pass exact scheduler removal before CPU access"]
pub struct LegacyConnectableAdvertisingMemoryGraphCompletionObserved {
    running: LegacyConnectableAdvertisingMemoryGraphRunning,
    status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
}

impl LegacyConnectableAdvertisingMemoryGraphCompletionObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.running.scheduler_item_address()
    }

    pub const fn status(&self) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.status
    }

    /// Observe RX progress only with the matching post-unlink removal proof.
    /// These independent volatile observations do not authorize PDU dispatch.
    pub fn observe_receive_nodes_after_removal(
        &self,
        removal: &BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Option<[crate::LeRxNodeObservation; BLUETOOTH_NON_SCANNING_RX_NODE_COUNT]> {
        if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO
            || removal.completed_head().address() != Some(self.scheduler_item_address())
        {
            return None;
        }
        Some(self.running.prepared.pool.observe_nodes())
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
        LegacyConnectableAdvertisingMemoryGraphRecyclePrepared,
        LegacyConnectableAdvertisingMemoryGraphRecycleFailure,
    > {
        let error = if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(LegacyConnectableAdvertisingMemoryGraphRecycleError::HardwareListMismatch)
        } else if removal.completed_head().address() != Some(self.scheduler_item_address()) {
            Some(LegacyConnectableAdvertisingMemoryGraphRecycleError::SchedulerItemMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(LegacyConnectableAdvertisingMemoryGraphRecycleFailure {
                completed: self,
                removal,
                error,
            });
        }
        Ok(LegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
            completed: self,
            removal,
        })
    }
}

/// Why completed-graph recycle authorization did not match this event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingMemoryGraphRecycleError {
    HardwareListMismatch,
    SchedulerItemMismatch,
}

/// Lossless recycle rejection retaining the completed graph and removal proof.
#[must_use = "the completed graph and removal proof remain hardware-owned"]
pub struct LegacyConnectableAdvertisingMemoryGraphRecycleFailure {
    completed: LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    error: LegacyConnectableAdvertisingMemoryGraphRecycleError,
}

impl LegacyConnectableAdvertisingMemoryGraphRecycleFailure {
    pub const fn error(&self) -> LegacyConnectableAdvertisingMemoryGraphRecycleError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphRecycleFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphRecycleFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Completed graph authorized for bounded, copy-only RX extraction.
#[must_use = "the receive result must be extracted or returned unchanged"]
pub struct LegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
    completed: LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl LegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
    /// Recover both unchanged owners before RX extraction starts.
    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }

    /// Copy every contiguous completed PDU without mutating either graph.
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
        LegacyConnectableAdvertisingMemoryGraphRxExtracted,
        LegacyConnectableAdvertisingMemoryGraphRxExtractionFailure,
    > {
        let batch = match self
            .completed
            .running
            .prepared
            .pool
            .extract_completed_rx_batch()
        {
            Ok(batch) => batch,
            Err(error) => {
                return Err(LegacyConnectableAdvertisingMemoryGraphRxExtractionFailure {
                    prepared: self,
                    error,
                });
            }
        };
        Ok(LegacyConnectableAdvertisingMemoryGraphRxExtracted {
            prepared: self,
            batch,
        })
    }
}

/// Malformed completed RX storage retaining the unchanged recycle owner.
#[must_use = "the unchanged graph remains unavailable until fail-stop handling"]
pub struct LegacyConnectableAdvertisingMemoryGraphRxExtractionFailure {
    prepared: LegacyConnectableAdvertisingMemoryGraphRecyclePrepared,
    error: LeRxError,
}

impl LegacyConnectableAdvertisingMemoryGraphRxExtractionFailure {
    pub const fn error(&self) -> LeRxError {
        self.error
    }

    pub fn into_prepared(self) -> LegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
        self.prepared
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphRxExtractionFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphRxExtractionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Copied RX batch paired with the sole reclaimable response-capable graph.
#[must_use = "commit reclamation before reusing either pinned allocation"]
pub struct LegacyConnectableAdvertisingMemoryGraphRxExtracted {
    prepared: LegacyConnectableAdvertisingMemoryGraphRecyclePrepared,
    batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
}

impl LegacyConnectableAdvertisingMemoryGraphRxExtracted {
    pub const fn batch(&self) -> LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        self.batch
    }

    /// Recover the unchanged recycle transaction before reclamation commits.
    #[doc(hidden)]
    pub fn into_prepared(self) -> LegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
        self.prepared
    }

    /// Rearm both RX nodes, restore the graph, and return ordinary CPU owners.
    pub fn commit(self) -> LegacyConnectableAdvertisingMemoryGraphRecycled {
        let LegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
            completed,
            removal: _,
        } = self.prepared;
        let LegacyConnectableAdvertisingMemoryGraphCompletionObserved { running, status } =
            completed;
        let LegacyConnectableAdvertisingMemoryGraphRunning {
            prepared,
            _rx_publication: _,
        } = running;
        let LegacyConnectableAdvertisingMemoryGraphPrepared {
            storage,
            binding,
            mut pool,
            adv_ind_length: _,
            scan_response_length: _,
            primary_channel: _,
            post_anchor_duration: _,
        } = prepared;
        let mut owner = LegacyConnectableAdvertisingMemoryGraphCpuOwned { storage, binding };
        owner.reinitialize_graph();
        pool.reinitialize_after_event();
        LegacyConnectableAdvertisingMemoryGraphRecycled {
            owner,
            pool,
            batch: self.batch,
            status,
        }
    }
}

/// Reclaimed graph, rearmed RX pool, copied batch, and opaque scheduler status.
#[must_use = "return both memory owners and the event result to the role layer"]
pub struct LegacyConnectableAdvertisingMemoryGraphRecycled {
    owner: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
    pool: NonScanningRxMemoryCpuOwned,
    batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
}

impl LegacyConnectableAdvertisingMemoryGraphRecycled {
    /// Stable identity of the reclaimed advertising graph.
    pub const fn identity(&self) -> LegacyConnectableAdvertisingMemoryGraphIdentity {
        self.owner.identity()
    }

    /// Stable identity of the rearmed non-scanning receive pool.
    pub const fn receive_identity(&self) -> NonScanningRxMemoryIdentity {
        self.pool.identity()
    }

    /// Admit the copied batch to role-specific Link Layer dispatch.
    ///
    /// A completed observation rejected by the private controller-result gate
    /// has no PDU bytes. It might have been a connection request, so the memory
    /// boundary must not silently reinterpret it as an event with no connection.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete reclaimed affine graph"
    )]
    pub fn prepare_rx_dispatch(
        self,
    ) -> Result<
        LegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared,
        LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked,
    > {
        let discarded = self.batch.discarded_count();
        if discarded != 0 {
            return Err(LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked {
                recycled: self,
                discarded,
            });
        }
        Ok(LegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared { recycled: self })
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        NonScanningRxMemoryCpuOwned,
        LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    ) {
        (self.owner, self.pool, self.batch, self.status)
    }
}

/// Reclaimed memory whose every completed receive observation has PDU bytes.
///
/// This type does not parse Bluetooth wire semantics. It only proves that the
/// role layer can inspect every completed observation before choosing an event
/// outcome.
#[must_use = "dispatch every copied PDU before returning the two memory owners"]
pub struct LegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared {
    recycled: LegacyConnectableAdvertisingMemoryGraphRecycled,
}

impl LegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared {
    pub const fn batch(&self) -> LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        self.recycled.batch
    }

    pub const fn status(&self) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.recycled.status
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        NonScanningRxMemoryCpuOwned,
        LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    ) {
        self.recycled.into_parts()
    }
}

/// Indeterminate receive result retaining the complete reclaimed memory owner.
#[must_use = "a missing PDU cannot be classified as no connection"]
pub struct LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked {
    recycled: LegacyConnectableAdvertisingMemoryGraphRecycled,
    discarded: usize,
}

impl LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked {
    /// Number of completed observations for which no PDU was dispatchable.
    pub const fn discarded_count(&self) -> usize {
        self.discarded
    }

    /// Recover the unchanged reclaimed graph for sealed fail-stop ownership.
    pub fn into_recycled(self) -> LegacyConnectableAdvertisingMemoryGraphRecycled {
        self.recycled
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked")
            .field("discarded", &self.discarded)
            .finish_non_exhaustive()
    }
}

/// Why the complete response-capable graph could not be prepared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingMemoryGraphPrepareError {
    ReceivePoolNotReady,
    ReceivePoolOverlapsGraph,
}

/// Failed preparation retaining both affine memory owners.
pub struct LegacyConnectableAdvertisingMemoryGraphPrepareFailure {
    owner: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
    pool: NonScanningRxMemoryCpuOwned,
    error: LegacyConnectableAdvertisingMemoryGraphPrepareError,
}

impl LegacyConnectableAdvertisingMemoryGraphPrepareFailure {
    fn new(
        owner: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        pool: NonScanningRxMemoryCpuOwned,
        error: LegacyConnectableAdvertisingMemoryGraphPrepareError,
    ) -> Self {
        Self { owner, pool, error }
    }

    pub const fn error(&self) -> LegacyConnectableAdvertisingMemoryGraphPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        NonScanningRxMemoryCpuOwned,
        LegacyConnectableAdvertisingMemoryGraphPrepareError,
    ) {
        (self.owner, self.pool, self.error)
    }
}

impl core::fmt::Debug for LegacyConnectableAdvertisingMemoryGraphPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyConnectableAdvertisingMemoryGraphPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl LegacyConnectableAdvertisingMemoryGraphStorage {
    pub const fn new() -> Self {
        Self {
            graph: LegacyConnectableAdvertisingGraphStorage::new(),
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<
        LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        LegacyConnectableAdvertisingMemoryGraphBindFailure,
    > {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(LegacyConnectableAdvertisingMemoryGraphBindFailure {
                    storage,
                    error: LegacyConnectableAdvertisingMemoryGraphBindError::AddressWidth,
                });
            }
        };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: LegacyConnectableAdvertisingMemoryGraphModelAddress,
    ) -> Result<
        LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        LegacyConnectableAdvertisingMemoryGraphBindFailure,
    > {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<
        LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        LegacyConnectableAdvertisingMemoryGraphBindFailure,
    > {
        let identity = LegacyConnectableAdvertisingMemoryGraphIdentity::for_storage(storage);
        let binding = match LegacyConnectableAdvertisingGraphBinding::new(identity, base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(LegacyConnectableAdvertisingMemoryGraphBindFailure { storage, error });
            }
        };
        let mut owner = LegacyConnectableAdvertisingMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize_graph();
        Ok(owner)
    }
}

impl Default for LegacyConnectableAdvertisingMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
