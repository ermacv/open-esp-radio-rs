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

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListSelector, BluetoothRxMemoryListPublished,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerSoftwareListRemovalReady,
};
use pin_project::pin_project;

use crate::{
    BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, BluetoothLeReceivedBatch, BluetoothLeRxError,
    BluetoothLegacyAdvertisingPrimaryChannel, BluetoothNonScanningRxMemoryCpuOwned,
    BluetoothNonScanningRxMemoryIdentity,
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, BluetoothLeTxPacketPreparedInput,
        BluetoothLeTxPacketPreparedLength,
    },
    legacy_advertising_event_image::BluetoothLegacyAdvertisingOwnAddress,
    rx_memory_list::BluetoothRxMemoryListClass,
};

use self::codec::{
    BluetoothLegacyConnectableAdvertisingGraphBinding,
    BluetoothLegacyConnectableAdvertisingGraphStorage,
    BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingSnapshot,
    BluetoothLegacyConnectableAdvertisingSoftwareLinkSnapshot,
};

const LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = 37;
const LEGACY_ADVERTISING_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

type AdvertisingTxPacketLength =
    BluetoothLeTxPacketPreparedLength<LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketInput<'a> =
    BluetoothLeTxPacketPreparedInput<'a, LEGACY_ADVERTISING_TX_PACKET_BYTES>;

/// Why an encoded advertising PDU cannot fit this S31 allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingPduFitError {
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
) -> Result<AdvertisingTxPacketInput<'_>, BluetoothLegacyConnectableAdvertisingPduFitError> {
    let payload_bytes = usize::from(payload_bytes);
    if payload_bytes > LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES {
        return Err(
            BluetoothLegacyConnectableAdvertisingPduFitError::PayloadExceedsAllocation {
                payload_bytes,
                capacity: LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES,
            },
        );
    }
    let expected_bytes = 2 + payload_bytes;
    if pdu.len() != expected_bytes {
        return Err(
            BluetoothLegacyConnectableAdvertisingPduFitError::EncodedExtentMismatch {
                expected_bytes,
                actual_bytes: pdu.len(),
            },
        );
    }
    Ok(AdvertisingTxPacketInput::from_validated_encoded_pdu(
        pdu,
        payload_bytes as u8,
    ))
}

/// Allocation-fit projection of one protocol-validated `ADV_IND` PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyConnectableAdvIndPacketInput<'a>(AdvertisingTxPacketInput<'a>);

impl<'a> BluetoothLegacyConnectableAdvIndPacketInput<'a> {
    /// Check only S31 allocation fit; the caller owns protocol validity.
    pub fn try_from_encoded_extent(
        pdu: &'a [u8],
        payload_bytes: u8,
    ) -> Result<Self, BluetoothLegacyConnectableAdvertisingPduFitError> {
        allocation_checked_packet(pdu, payload_bytes).map(Self)
    }
}

/// Allocation-fit projection of one protocol-validated `SCAN_RSP` PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyConnectableScanResponsePacketInput<'a>(AdvertisingTxPacketInput<'a>);

impl<'a> BluetoothLegacyConnectableScanResponsePacketInput<'a> {
    /// Check only S31 allocation fit; the caller owns protocol validity.
    pub fn try_from_encoded_extent(
        pdu: &'a [u8],
        payload_bytes: u8,
    ) -> Result<Self, BluetoothLegacyConnectableAdvertisingPduFitError> {
        allocation_checked_packet(pdu, payload_bytes).map(Self)
    }
}

/// Address behavior already selected by the chip protocol bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingOwnAddress {
    Public,
    Random([u8; 6]),
}

impl BluetoothLegacyConnectableAdvertisingOwnAddress {
    const fn codec(self) -> BluetoothLegacyAdvertisingOwnAddress {
        match self {
            Self::Public => BluetoothLegacyAdvertisingOwnAddress::Public,
            Self::Random(address) => BluetoothLegacyAdvertisingOwnAddress::Random(address),
        }
    }
}

/// Complete allocation-fit input needed to lower one one-channel event.
///
/// This value contains no portable Link Layer owner. Its PDU wrappers check
/// only controller-allocation fit and never parse Bluetooth header semantics;
/// the chip protocol bridge must retain the corresponding validated LL owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyConnectableAdvertisingMemoryInput<'a> {
    adv_ind: BluetoothLegacyConnectableAdvIndPacketInput<'a>,
    scan_response: BluetoothLegacyConnectableScanResponsePacketInput<'a>,
    own_address: BluetoothLegacyConnectableAdvertisingOwnAddress,
    primary_channel: BluetoothLegacyAdvertisingPrimaryChannel,
}

impl<'a> BluetoothLegacyConnectableAdvertisingMemoryInput<'a> {
    pub const fn new(
        adv_ind: BluetoothLegacyConnectableAdvIndPacketInput<'a>,
        scan_response: BluetoothLegacyConnectableScanResponsePacketInput<'a>,
        own_address: BluetoothLegacyConnectableAdvertisingOwnAddress,
        primary_channel: BluetoothLegacyAdvertisingPrimaryChannel,
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
pub struct BluetoothLegacyConnectableAdvertisingPostAnchorDuration(u32);

impl BluetoothLegacyConnectableAdvertisingPostAnchorDuration {
    /// Post-anchor duration in microseconds before controller-epoch projection.
    pub const fn as_micros(self) -> u32 {
        self.0
    }
}

/// Stable pinned allocation for one response-capable advertising graph.
#[repr(C)]
#[pin_project]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphStorage {
    graph: BluetoothLegacyConnectableAdvertisingGraphStorage,
    #[pin]
    _pin: PhantomPinned,
}

/// Opaque identity of one exact pinned connectable-advertising allocation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity(usize);

impl BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity {
    fn for_storage(storage: &BluetoothLegacyConnectableAdvertisingMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress(
    BluetoothControllerSramAddress,
);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress {
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
pub enum BluetoothLegacyConnectableAdvertisingMemoryGraphBindError {
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
    InvalidPacketExtent,
}

/// Failed binding retaining the exact unchanged static allocation.
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure {
    storage: &'static mut BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphBindError,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure {
    pub const fn error(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphBindError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
        BluetoothLegacyConnectableAdvertisingMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Unique CPU owner before response-capable event preparation.
#[must_use = "the connectable-advertising graph must be retained"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothLegacyConnectableAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyConnectableAdvertisingGraphBinding,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned {
    pub const fn identity(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity {
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
        input: BluetoothLegacyConnectableAdvertisingMemoryInput<'_>,
        pool: BluetoothNonScanningRxMemoryCpuOwned,
        default_tx_power_dbm: i8,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
        BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure,
    > {
        if !pool.is_initialized() {
            return Err(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure::new(
                self,
                pool,
                BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolNotReady,
            ));
        }
        if !self.binding.is_disjoint_from_receive_pool(&pool) {
            return Err(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure::new(
                self,
                pool,
                BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolOverlapsGraph,
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
            pool.head(),
            pool.tail(),
            input.own_address.codec(),
            default_tx_power_dbm,
        );

        Ok(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared {
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
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared {
    storage: Pin<&'static mut BluetoothLegacyConnectableAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyConnectableAdvertisingGraphBinding,
    pool: BluetoothNonScanningRxMemoryCpuOwned,
    adv_ind_length: AdvertisingTxPacketLength,
    scan_response_length: AdvertisingTxPacketLength,
    primary_channel: BluetoothLegacyAdvertisingPrimaryChannel,
    post_anchor_duration: BluetoothLegacyConnectableAdvertisingPostAnchorDuration,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared {
    pub const fn identity(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity {
        self.binding.identity()
    }

    pub const fn receive_identity(&self) -> BluetoothNonScanningRxMemoryIdentity {
        self.pool.identity()
    }

    pub const fn primary_channel(&self) -> BluetoothLegacyAdvertisingPrimaryChannel {
        self.primary_channel
    }

    pub const fn post_anchor_duration(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingPostAnchorDuration {
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
    /// controller time or reserve a common timeline slot.
    pub fn prepare_event_fields(
        self,
        raw_start: u32,
        raw_end: u32,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
        BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure,
    > {
        if let Err(error) = self.storage.as_ref().get_ref().graph.prepare_event_fields(
            &self.binding,
            self.primary_channel,
            raw_start,
            raw_end,
        ) {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure {
                    owner: self,
                    error,
                },
            );
        }
        Ok(BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared { prepared: self })
    }

    /// Whether all private TX and RX links still form the prepared topology.
    pub fn is_ready_for_scheduler_lowering(&self) -> bool {
        self.pool.is_initialized()
            && self
                .storage
                .as_ref()
                .get_ref()
                .graph
                .retains_prepared_graph(&self.binding, self.pool.head(), self.pool.tail())
    }

    /// Remove every unpublished role image and recover both exact owners.
    pub fn cancel(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        BluetoothNonScanningRxMemoryCpuOwned,
    ) {
        let mut owner = BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        (owner, self.pool)
    }
}

/// Why the sole connectable-advertising scheduler item was not writable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError {
    /// The response graph no longer retains its private scheduler-item head.
    SchedulerHeadMismatch,
    /// The sole scheduler item already points at another hardware item.
    NonTerminalSchedulerItem,
}

/// Failed event-field lowering retaining the exact prepared graph and RX pool.
#[must_use = "the unchanged prepared graph and receive pool remain owned"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure {
    owner: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure {
    pub const fn error(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
        BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug
    for BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct(
                "BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareFailure",
            )
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Complete one-item event fields before common scheduler bookkeeping.
#[must_use = "the event fields must advance, be cancelled, or remain retained"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared {
    prepared: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared {
    /// Exact CPU-owned item eligible for later common-list admission.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding.scheduler_item_address()
    }

    /// Install the common status sentinel and completed-list baseline.
    pub fn prepare_scheduler_bookkeeping(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
        let previous = self
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .prepare_scheduler_bookkeeping();
        BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
            event: self,
            previous,
        }
    }

    /// Restore the prepared response graph before any common-list admission.
    pub fn cancel(self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared {
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
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
    event: BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
    previous: BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingSnapshot,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_item_address()
    }

    /// Clear the software-list successor before empty-list head publication.
    pub fn prepare_empty_list_link(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
        let previous_software_link = self
            .event
            .prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .prepare_empty_list_link();
        BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
            bookkeeping: self,
            previous_software_link,
        }
    }

    /// Restore the exact scheduler fields observed before bookkeeping.
    pub fn cancel(self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared {
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
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
    bookkeeping: BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    previous_software_link: BluetoothLegacyConnectableAdvertisingSoftwareLinkSnapshot,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.bookkeeping.scheduler_item_address()
    }

    /// Freeze the complete CPU-owned graph before RX-list publication.
    pub fn prepare_publication(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared {
        BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared { prepared: self }
    }

    /// Restore the exact software-list successor without disturbing bookkeeping.
    pub fn cancel(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
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
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared {
    prepared: BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared {
    /// Stable identity retained across the publication boundary.
    pub const fn identity(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity {
        self.prepared.bookkeeping.event.prepared.identity()
    }

    /// Stable receive-pool identity retained across the publication boundary.
    pub const fn receive_identity(&self) -> BluetoothNonScanningRxMemoryIdentity {
        self.prepared.bookkeeping.event.prepared.receive_identity()
    }

    /// Memory-layer mapping for an ordinary non-scanning advertising item.
    #[doc(hidden)]
    pub const fn selector(&self) -> BluetoothMemoryListSelector {
        BluetoothRxMemoryListClass::NonScanning.selector()
    }

    /// Validated receive header retained by this affine graph.
    #[doc(hidden)]
    pub const fn receive_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.bookkeeping.event.prepared.pool.head()
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
        publication: BluetoothRxMemoryListPublished,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingMemoryGraphRxPublished,
        BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationMismatch,
    > {
        let error = if publication.selector() != self.selector() {
            Some(BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError::SelectorMismatch)
        } else if publication.head() != self.receive_head() {
            Some(BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationMismatch {
                    prepared: self,
                    publication,
                    error,
                },
            );
        }

        let BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
            bookkeeping,
            previous_software_link: _,
        } = self.prepared;
        let BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared {
            event,
            previous: _,
        } = bookkeeping;
        let BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared { prepared } =
            event;
        Ok(
            BluetoothLegacyConnectableAdvertisingMemoryGraphRxPublished {
                prepared,
                rx_publication: publication,
            },
        )
    }

    /// Recover the exact empty-list candidate before any hardware publication.
    pub fn cancel(self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared {
        self.prepared
    }
}

/// Why an RX-list publication does not name this response-capable graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError {
    /// The publication belongs to another positional RX memory list.
    SelectorMismatch,
    /// The publication names another pinned receive pool.
    HeadMismatch,
}

/// Failed RX-list proof join retaining the CPU graph and HAL publication.
#[must_use = "a mismatched publication still owns the graph and HAL token"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationMismatch {
    prepared: BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
    publication: BluetoothRxMemoryListPublished,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationMismatch {
    pub const fn error(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError {
        self.error
    }

    /// Recover both exact affine owners without changing either identity.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
        BluetoothRxMemoryListPublished,
    ) {
        (self.prepared, self.publication)
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Graph whose exact non-scanning RX list is hardware-visible.
#[must_use = "the RX-published graph must enter the primary scheduler list"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRxPublished {
    prepared: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
    rx_publication: BluetoothRxMemoryListPublished,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRxPublished {
    /// Exact scheduler item paired with this receive-list publication.
    #[doc(hidden)]
    pub const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding.scheduler_item_address()
    }

    /// Borrow the retained non-scanning RX-list publication proof.
    #[doc(hidden)]
    pub const fn rx_publication(&self) -> &BluetoothRxMemoryListPublished {
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
        BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublished,
        BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch,
    > {
        let error = if publication.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError::ListMismatch)
        } else if publication.head().address() != Some(self.scheduler_head()) {
            Some(BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch {
                    published: self,
                    error,
                },
            );
        }
        Ok(
            BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublished {
                prepared: self.prepared,
                rx_publication: self.rx_publication,
            },
        )
    }
}

/// Why a scheduler HEAD or RUN proof cannot join this exact graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError {
    /// The proof belongs to another hardware scheduler list.
    ListMismatch,
    /// The proof retains another scheduler-item head.
    HeadMismatch,
}

/// Failed scheduler-head proof join retaining the RX-published graph.
#[must_use = "a mismatched scheduler-head proof still leaves the graph hardware-owned"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch {
    published: BluetoothLegacyConnectableAdvertisingMemoryGraphRxPublished,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch {
    pub const fn error(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError {
        self.error
    }

    /// Recover the unchanged hardware-owned graph and finite mismatch reason.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphRxPublished,
        BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
    ) {
        (self.published, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublicationMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Response-capable graph visible through both RX and scheduler list heads.
#[must_use = "the head-published graph must reach RUN or remain fail-stop owned"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublished {
    prepared: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
    rx_publication: BluetoothRxMemoryListPublished,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublished {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding.scheduler_item_address()
    }

    /// Borrow the exact RX publication retained by the graph.
    #[doc(hidden)]
    pub const fn rx_publication(&self) -> &BluetoothRxMemoryListPublished {
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
        BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
        BluetoothLegacyConnectableAdvertisingMemoryGraphRunMismatch,
    > {
        let error = if run.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError::ListMismatch)
        } else if run.head().address() != Some(self.scheduler_item_address()) {
            Some(BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError::HeadMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphRunMismatch {
                    published: self,
                    error,
                },
            );
        }
        Ok(BluetoothLegacyConnectableAdvertisingMemoryGraphRunning {
            prepared: self.prepared,
            _rx_publication: self.rx_publication,
        })
    }
}

/// Failed scheduler RUN proof join retaining the head-published graph.
#[must_use = "a mismatched RUN proof still leaves the graph hardware-owned"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRunMismatch {
    published: BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublished,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRunMismatch {
    pub const fn error(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError {
        self.error
    }

    /// Recover the unchanged graph and finite mismatch reason.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphHeadPublished,
        BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
    ) {
        (self.published, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphRunMismatch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphRunMismatch")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Hardware-owned response-capable graph admitted through the scheduler RUN transaction.
#[must_use = "the running graph awaits a separately reviewed completion lifecycle"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRunning {
    prepared: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
    _rx_publication: BluetoothRxMemoryListPublished,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRunning {
    /// Stable graph identity retained while hardware owns the event.
    pub const fn identity(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity {
        self.prepared.identity()
    }

    /// Stable receive-pool identity retained while hardware owns the event.
    pub const fn receive_identity(&self) -> BluetoothNonScanningRxMemoryIdentity {
        self.prepared.receive_identity()
    }

    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.prepared.binding.scheduler_item_address()
    }

    /// Consume one fenced finished-list observation and inspect the sole item.
    pub fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation {
        if observed.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            return BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::ListMismatch {
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
            return BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::StillInFlight(self);
        };
        BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved {
                running: self,
                status,
            },
        )
    }

    #[cfg(test)]
    fn model_controller_completion(
        &self,
        status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
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
pub enum BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
    Zero,
    NonZero,
}

/// One bounded observation of the hardware-owned response-capable graph.
#[must_use = "the graph and any unrelated finished-list proof remain owned"]
pub enum BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation {
    ListMismatch {
        running: BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothLegacyConnectableAdvertisingMemoryGraphRunning),
    CompletionObserved(BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved),
}

/// Hardware-owned graph after its sole item produced a non-sentinel status.
#[must_use = "the completed graph must pass exact scheduler removal before CPU access"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved {
    running: BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
    status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.running.scheduler_item_address()
    }

    pub const fn status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
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
        BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared,
        BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleFailure,
    > {
        let error = if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError::HardwareListMismatch)
        } else if removal.completed_head().address() != Some(self.scheduler_item_address()) {
            Some(
                BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError::SchedulerItemMismatch,
            )
        } else {
            None
        };
        if let Some(error) = error {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleFailure {
                    completed: self,
                    removal,
                    error,
                },
            );
        }
        Ok(
            BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
                completed: self,
                removal,
            },
        )
    }
}

/// Why completed-graph recycle authorization did not match this event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError {
    HardwareListMismatch,
    SchedulerItemMismatch,
}

/// Lossless recycle rejection retaining the completed graph and removal proof.
#[must_use = "the completed graph and removal proof remain hardware-owned"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleFailure {
    completed: BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleFailure {
    pub const fn error(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Completed graph authorized for bounded, copy-only RX extraction.
#[must_use = "the receive result must be extracted or returned unchanged"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
    completed: BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
    /// Recover both unchanged owners before RX extraction starts.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved,
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
        BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtracted,
        BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtractionFailure,
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
                return Err(
                    BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtractionFailure {
                        prepared: self,
                        error,
                    },
                );
            }
        };
        Ok(
            BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtracted {
                prepared: self,
                batch,
            },
        )
    }
}

/// Malformed completed RX storage retaining the unchanged recycle owner.
#[must_use = "the unchanged graph remains unavailable until fail-stop handling"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtractionFailure {
    prepared: BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared,
    error: BluetoothLeRxError,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtractionFailure {
    pub const fn error(&self) -> BluetoothLeRxError {
        self.error
    }

    pub fn into_prepared(self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
        self.prepared
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtractionFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtractionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Copied RX batch paired with the sole reclaimable response-capable graph.
#[must_use = "commit reclamation before reusing either pinned allocation"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtracted {
    prepared: BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared,
    batch: BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRxExtracted {
    pub const fn batch(&self) -> BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        self.batch
    }

    /// Recover the unchanged recycle transaction before reclamation commits.
    #[doc(hidden)]
    pub fn into_prepared(self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
        self.prepared
    }

    /// Rearm both RX nodes, restore the graph, and return ordinary CPU owners.
    pub fn commit(self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled {
        let BluetoothLegacyConnectableAdvertisingMemoryGraphRecyclePrepared {
            completed,
            removal: _,
        } = self.prepared;
        let BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved { running, status } =
            completed;
        let BluetoothLegacyConnectableAdvertisingMemoryGraphRunning {
            prepared,
            _rx_publication: _,
        } = running;
        let BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared {
            storage,
            binding,
            mut pool,
            adv_ind_length: _,
            scan_response_length: _,
            primary_channel: _,
            post_anchor_duration: _,
        } = prepared;
        let mut owner =
            BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned { storage, binding };
        owner.reinitialize_graph();
        pool.reinitialize_after_event();
        BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled {
            owner,
            pool,
            batch: self.batch,
            status,
        }
    }
}

/// Reclaimed graph, rearmed RX pool, copied batch, and opaque scheduler status.
#[must_use = "return both memory owners and the event result to the role layer"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled {
    owner: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    pool: BluetoothNonScanningRxMemoryCpuOwned,
    batch: BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled {
    /// Stable identity of the reclaimed advertising graph.
    pub const fn identity(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity {
        self.owner.identity()
    }

    /// Stable identity of the rearmed non-scanning receive pool.
    pub const fn receive_identity(&self) -> BluetoothNonScanningRxMemoryIdentity {
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
        BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared,
        BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked,
    > {
        let discarded = self.batch.discarded_count();
        if discarded != 0 {
            return Err(
                BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked {
                    recycled: self,
                    discarded,
                },
            );
        }
        Ok(BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared { recycled: self })
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        BluetoothNonScanningRxMemoryCpuOwned,
        BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
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
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared {
    recycled: BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchPrepared {
    pub const fn batch(&self) -> BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        self.recycled.batch
    }

    pub const fn status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.recycled.status
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        BluetoothNonScanningRxMemoryCpuOwned,
        BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    ) {
        self.recycled.into_parts()
    }
}

/// Indeterminate receive result retaining the complete reclaimed memory owner.
#[must_use = "a missing PDU cannot be classified as no connection"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked {
    recycled: BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled,
    discarded: usize,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked {
    /// Number of completed observations for which no PDU was dispatchable.
    pub const fn discarded_count(&self) -> usize {
        self.discarded
    }

    /// Recover the unchanged reclaimed graph for sealed fail-stop ownership.
    pub fn into_recycled(self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled {
        self.recycled
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked")
            .field("discarded", &self.discarded)
            .finish_non_exhaustive()
    }
}

/// Why the complete response-capable graph could not be prepared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError {
    ReceivePoolNotReady,
    ReceivePoolOverlapsGraph,
}

/// Failed preparation retaining both affine memory owners.
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure {
    owner: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    pool: BluetoothNonScanningRxMemoryCpuOwned,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure {
    fn new(
        owner: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        pool: BluetoothNonScanningRxMemoryCpuOwned,
        error: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    ) -> Self {
        Self { owner, pool, error }
    }

    pub const fn error(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        BluetoothNonScanningRxMemoryCpuOwned,
        BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    ) {
        (self.owner, self.pool, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BluetoothLegacyConnectableAdvertisingMemoryGraphStorage {
    pub const fn new() -> Self {
        Self {
            graph: BluetoothLegacyConnectableAdvertisingGraphStorage::new(),
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure,
    > {
        let base =
            match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
                Ok(base) => base,
                Err(_) => {
                    return Err(BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure {
                    storage,
                    error: BluetoothLegacyConnectableAdvertisingMemoryGraphBindError::AddressWidth,
                });
                }
            };
        Self::pin_static_inner(storage, base)
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure,
    > {
        Self::pin_static_inner(storage, base.address())
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure,
    > {
        let identity =
            BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity::for_storage(storage);
        let binding = match BluetoothLegacyConnectableAdvertisingGraphBinding::new(identity, base) {
            Ok(binding) => binding,
            Err(error) => {
                return Err(
                    BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure { storage, error },
                );
            }
        };
        let mut owner = BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.reinitialize_graph();
        Ok(owner)
    }
}

impl Default for BluetoothLegacyConnectableAdvertisingMemoryGraphStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage};
    use open_esp_radio_esp32s31_hal::{
        BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
        BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
    };

    const ADVERTISER: [u8; 6] = [1, 2, 3, 4, 5, 6];
    const ADV_IND_PDU: [u8; 11] = [0x60, 9, 1, 2, 3, 4, 5, 6, 2, 1, 6];
    const SCAN_RESPONSE_PDU: [u8; 8] = [0x44, 6, 1, 2, 3, 4, 5, 6];

    fn owner(base: u32) -> BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::new(),
        ));
        let base = BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress::new(base)
            .expect("the model graph base belongs to controller SRAM");
        BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
            .expect("the response-capable graph fits controller SRAM")
    }

    fn pool(base: u32) -> BluetoothNonScanningRxMemoryCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothNonScanningRxMemoryStorage::new(),
        ));
        let base = BluetoothNonScanningRxMemoryModelAddress::new(base)
            .expect("the model RX base belongs to controller SRAM");
        BluetoothNonScanningRxMemoryStorage::pin_static_model(storage, base)
            .expect("the RX pool fits controller SRAM")
    }

    fn input(
        primary_channel: BluetoothLegacyAdvertisingPrimaryChannel,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryInput<'static> {
        BluetoothLegacyConnectableAdvertisingMemoryInput::new(
            BluetoothLegacyConnectableAdvIndPacketInput::try_from_encoded_extent(&ADV_IND_PDU, 9)
                .expect("the portable ADV_IND fits the S31 packet allocation"),
            BluetoothLegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
                &SCAN_RESPONSE_PDU,
                6,
            )
            .expect("the portable SCAN_RSP fits the S31 packet allocation"),
            BluetoothLegacyConnectableAdvertisingOwnAddress::Random(ADVERTISER),
            primary_channel,
        )
    }

    fn running(
        graph_base: u32,
        pool_base: u32,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
        BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
        BluetoothNonScanningRxMemoryIdentity,
    ) {
        let owner = owner(graph_base);
        let graph_identity = owner.identity();
        let pool = pool(pool_base);
        let pool_identity = pool.identity();
        let receive_head = pool.head();
        let published = owner
            .prepare_response_capable_event(
                input(BluetoothLegacyAdvertisingPrimaryChannel::Channel37),
                pool,
                0,
            )
            .expect("the disjoint response graph is supported")
            .prepare_event_fields(6_000, 6_200)
            .expect("the pristine one-item graph accepts event fields")
            .prepare_scheduler_bookkeeping()
            .prepare_empty_list_link()
            .prepare_publication()
            .into_rx_published(BluetoothRxMemoryListPublished::from_parts_for_validation(
                BluetoothRxMemoryListClass::NonScanning.selector(),
                receive_head,
            ))
            .unwrap_or_else(|_| panic!("the exact RX publication must join this graph"));
        let BluetoothLegacyConnectableAdvertisingMemoryGraphRxPublished {
            prepared,
            rx_publication,
        } = published;
        (
            BluetoothLegacyConnectableAdvertisingMemoryGraphRunning {
                prepared,
                _rx_publication: rx_publication,
            },
            graph_identity,
            pool_identity,
        )
    }

    fn finished_list(index: u8) -> BluetoothSchedulerFinishedHardwareListObserved {
        let observation =
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[index])
                .expect("the selected hardware list is representable");
        let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest()
        else {
            panic!("the semantic observation contains the selected list")
        };
        observed
    }

    fn removal_ready(
        index: BluetoothSchedulerHardwareListIndex,
        address: BluetoothControllerSramAddress,
    ) -> BluetoothSchedulerSoftwareListRemovalReady {
        let head = BluetoothSchedulerHardwareListHead::from_address(address)
            .expect("the scheduler item forms a nonempty head");
        let empty = BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(
            index, head,
        );
        BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(empty)
    }

    #[test]
    fn response_graph_owns_both_pdus_and_receive_pool_until_cancelled() {
        let owner = owner(0x2f00_0100);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_4000);
        let pool_identity = pool.identity();
        let prepared = owner
            .prepare_response_capable_event(
                input(BluetoothLegacyAdvertisingPrimaryChannel::Channel37),
                pool,
                0,
            )
            .expect("the disjoint response graph is supported");

        assert_eq!(prepared.identity(), graph_identity);
        assert_eq!(prepared.receive_identity(), pool_identity);
        assert_eq!(prepared.adv_ind_pdu(), &ADV_IND_PDU);
        assert_eq!(prepared.scan_response_pdu(), &SCAN_RESPONSE_PDU);
        assert_eq!(prepared.post_anchor_duration().as_micros(), 156);
        assert_eq!(
            prepared.primary_channel(),
            BluetoothLegacyAdvertisingPrimaryChannel::Channel37
        );
        assert!(prepared.is_ready_for_scheduler_lowering());

        let (owner, pool) = prepared.cancel();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
    }

    #[test]
    fn event_fields_and_common_list_preparation_cancel_in_reverse() {
        let owner = owner(0x2f00_1000);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_5000);
        let pool_identity = pool.identity();
        let prepared = owner
            .prepare_response_capable_event(
                input(BluetoothLegacyAdvertisingPrimaryChannel::Channel38),
                pool,
                -4,
            )
            .expect("the disjoint response graph is supported");

        let event = prepared
            .prepare_event_fields(1_200, 1_400)
            .expect("the pristine one-item graph accepts event fields");
        let scheduler_item = event.scheduler_item_address();
        let bookkeeping = event.prepare_scheduler_bookkeeping();
        assert_eq!(bookkeeping.scheduler_item_address(), scheduler_item);
        let empty = bookkeeping.prepare_empty_list_link();
        assert_eq!(empty.scheduler_item_address(), scheduler_item);
        let publication = empty.prepare_publication();
        assert_eq!(publication.identity(), graph_identity);
        assert_eq!(publication.receive_identity(), pool_identity);
        assert_eq!(publication.scheduler_head(), scheduler_item);

        let empty = publication.cancel();
        let bookkeeping = empty.cancel();
        let event = bookkeeping.cancel();
        let prepared = event.cancel();
        assert!(prepared.is_ready_for_scheduler_lowering());
        assert_eq!(prepared.identity(), graph_identity);
        assert_eq!(prepared.receive_identity(), pool_identity);
        assert_eq!(prepared.adv_ind_pdu(), &ADV_IND_PDU);
        assert_eq!(prepared.scan_response_pdu(), &SCAN_RESPONSE_PDU);

        let (owner, pool) = prepared.cancel();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
    }

    #[test]
    fn matching_rx_publication_surrenders_cpu_rollback_and_retains_identities() {
        let owner = owner(0x2f00_2400);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_6400);
        let pool_identity = pool.identity();
        let receive_head = pool.head();
        let publication = owner
            .prepare_response_capable_event(
                input(BluetoothLegacyAdvertisingPrimaryChannel::Channel37),
                pool,
                -2,
            )
            .expect("the disjoint response graph is supported")
            .prepare_event_fields(3_000, 3_200)
            .expect("the pristine one-item graph accepts event fields")
            .prepare_scheduler_bookkeeping()
            .prepare_empty_list_link()
            .prepare_publication();
        let scheduler_head = publication.scheduler_head();
        assert_eq!(publication.identity(), graph_identity);
        assert_eq!(publication.receive_identity(), pool_identity);

        let published = publication
            .into_rx_published(BluetoothRxMemoryListPublished::from_parts_for_validation(
                BluetoothRxMemoryListClass::NonScanning.selector(),
                receive_head,
            ))
            .unwrap_or_else(|_| panic!("the exact RX publication must join this graph"));
        assert_eq!(published.scheduler_head(), scheduler_head);
        assert_eq!(
            published.rx_publication().selector(),
            BluetoothRxMemoryListClass::NonScanning.selector()
        );
        assert_eq!(published.rx_publication().head(), receive_head);
    }

    #[test]
    fn completion_requires_list_zero_and_a_non_sentinel_item_status() {
        let (running, _, _) = running(0x2f00_3000, 0x2f00_7000);
        let scheduler_item = running.scheduler_item_address();
        let running = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::StillInFlight(
                running,
            ) => running,
            _ => panic!("the in-flight sentinel must retain hardware ownership"),
        };
        running.model_controller_completion(
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
        );

        let running = match running.observe_completion(finished_list(1)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::ListMismatch {
                running,
                observed,
            } => {
                assert_ne!(observed.index(), BluetoothSchedulerHardwareListIndex::ZERO);
                running
            }
            _ => panic!("an unrelated finished list must retain both owners"),
        };
        assert_eq!(running.scheduler_item_address(), scheduler_item);

        let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the matching finished list may consume a non-sentinel status"),
        };
        assert_eq!(completed.scheduler_item_address(), scheduler_item);
        assert_eq!(
            completed.status(),
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
        );
    }

    #[test]
    fn matching_removal_extracts_then_rearms_both_cpu_owned_graphs() {
        let (running, graph_identity, pool_identity) = running(0x2f00_3400, 0x2f00_7400);
        let scheduler_item = running.scheduler_item_address();
        running.model_controller_completion(
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero,
        );
        let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled non-sentinel status completes the item"),
        };
        let extracted = completed
            .prepare_recycle_after_software_list_removal(removal_ready(
                BluetoothSchedulerHardwareListIndex::ZERO,
                scheduler_item,
            ))
            .unwrap_or_else(|_| panic!("the exact removal proof must authorize RX extraction"))
            .extract_received()
            .unwrap_or_else(|_| panic!("an event without a received PDU is a valid empty batch"));
        assert!(extracted.batch().is_empty());

        let (owner, pool, batch, status) = extracted.commit().into_parts();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
        assert!(batch.is_empty());
        assert_eq!(
            status,
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero
        );
    }

    #[test]
    fn copied_receive_batch_is_admitted_to_role_dispatch_after_reclamation() {
        let (running, graph_identity, pool_identity) = running(0x2f00_3500, 0x2f00_7500);
        let scheduler_item = running.scheduler_item_address();
        let received = [0x03, 6, 1, 2, 3, 4, 5, 6];
        running.model_controller_receive(0, &received, -31, 12_345);
        running.model_controller_completion(
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
        );
        let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled event must complete"),
        };
        let recycled = completed
            .prepare_recycle_after_software_list_removal(removal_ready(
                BluetoothSchedulerHardwareListIndex::ZERO,
                scheduler_item,
            ))
            .unwrap_or_else(|_| panic!("the exact removal proof must authorize reclamation"))
            .extract_received()
            .unwrap_or_else(|_| panic!("the completed receive node must be extractable"))
            .commit();

        let dispatch = recycled
            .prepare_rx_dispatch()
            .unwrap_or_else(|_| panic!("every completed observation retains its PDU"));
        assert_eq!(dispatch.batch().len(), 1);
        assert_eq!(
            dispatch.batch().packet(0).map(|packet| packet.as_bytes()),
            Some(received.as_slice())
        );
        let (owner, pool, _, status) = dispatch.into_parts();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
        assert_eq!(
            status,
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
        );
    }

    #[test]
    fn discarded_receive_observation_cannot_become_no_connection() {
        let (running, graph_identity, pool_identity) = running(0x2f00_3600, 0x2f00_7600);
        let scheduler_item = running.scheduler_item_address();
        running.model_controller_discard(0);
        running.model_controller_completion(
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero,
        );
        let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled event must complete"),
        };
        let recycled = completed
            .prepare_recycle_after_software_list_removal(removal_ready(
                BluetoothSchedulerHardwareListIndex::ZERO,
                scheduler_item,
            ))
            .unwrap_or_else(|_| panic!("the exact removal proof must authorize reclamation"))
            .extract_received()
            .unwrap_or_else(|_| panic!("a hardware discard is a bounded receive observation"))
            .commit();

        let blocked = match recycled.prepare_rx_dispatch() {
            Ok(_) => panic!("missing PDU bytes must not reach role dispatch"),
            Err(blocked) => blocked,
        };
        assert_eq!(blocked.discarded_count(), 1);
        let recycled = blocked.into_recycled();
        assert_eq!(recycled.identity(), graph_identity);
        assert_eq!(recycled.receive_identity(), pool_identity);
    }

    #[test]
    fn removal_list_mismatch_retains_the_completed_graph_and_proof() {
        let (running, _, _) = running(0x2f00_3800, 0x2f00_7800);
        let scheduler_item = running.scheduler_item_address();
        running.model_controller_completion(
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
        );
        let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled non-sentinel status completes the item"),
        };
        let another_list = BluetoothSchedulerHardwareListIndex::new(1)
            .expect("the second scheduler list is representable");

        let failure = match completed.prepare_recycle_after_software_list_removal(removal_ready(
            another_list,
            scheduler_item,
        )) {
            Ok(_) => panic!("another scheduler list must not authorize reclamation"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError::HardwareListMismatch
        );
        let (completed, removal) = failure.into_parts();
        assert_eq!(completed.scheduler_item_address(), scheduler_item);
        assert_eq!(removal.index(), another_list);
    }

    #[test]
    fn removal_head_mismatch_retains_the_completed_graph_and_proof() {
        let (running, _, _) = running(0x2f00_3c00, 0x2f00_7c00);
        let scheduler_item = running.scheduler_item_address();
        running.model_controller_completion(
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
        );
        let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled non-sentinel status completes the item"),
        };
        let foreign = owner(0x2f00_8000);
        let foreign_item = foreign.binding.scheduler_item_address();

        let failure = match completed.prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            foreign_item,
        )) {
            Ok(_) => panic!("another scheduler item must not authorize reclamation"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError::SchedulerItemMismatch
        );
        let (completed, removal) = failure.into_parts();
        assert_eq!(completed.scheduler_item_address(), scheduler_item);
        assert_eq!(removal.completed_head().address(), Some(foreign_item));
        assert_ne!(completed.scheduler_item_address(), foreign_item);
    }

    #[test]
    fn selector_mismatch_retains_publication_and_all_cpu_rollback_authority() {
        let owner = owner(0x2f00_2800);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_6800);
        let pool_identity = pool.identity();
        let receive_head = pool.head();
        let publication = owner
            .prepare_response_capable_event(
                input(BluetoothLegacyAdvertisingPrimaryChannel::Channel38),
                pool,
                1,
            )
            .expect("the disjoint response graph is supported")
            .prepare_event_fields(4_000, 4_200)
            .expect("the pristine one-item graph accepts event fields")
            .prepare_scheduler_bookkeeping()
            .prepare_empty_list_link()
            .prepare_publication();
        let mismatched = BluetoothRxMemoryListPublished::from_parts_for_validation(
            BluetoothRxMemoryListClass::Scanning.selector(),
            receive_head,
        );

        let mismatch = match publication.into_rx_published(mismatched) {
            Ok(_) => panic!("a scanner-list publication must not join this graph"),
            Err(mismatch) => mismatch,
        };
        assert_eq!(
            mismatch.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError::SelectorMismatch
        );
        let (publication, mismatched) = mismatch.into_parts();
        assert_eq!(publication.identity(), graph_identity);
        assert_eq!(publication.receive_identity(), pool_identity);
        assert_eq!(mismatched.head(), receive_head);
        assert_eq!(
            mismatched.selector(),
            BluetoothRxMemoryListClass::Scanning.selector()
        );

        let prepared = publication.cancel().cancel().cancel().cancel();
        let (owner, pool) = prepared.cancel();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
    }

    #[test]
    fn receive_head_mismatch_retains_both_affine_owners() {
        let owner = owner(0x2f00_2c00);
        let graph_identity = owner.identity();
        let receive_pool = pool(0x2f00_6c00);
        let pool_identity = receive_pool.identity();
        let other_pool = pool(0x2f00_7400);
        let other_head = other_pool.head();
        let publication = owner
            .prepare_response_capable_event(
                input(BluetoothLegacyAdvertisingPrimaryChannel::Channel39),
                receive_pool,
                2,
            )
            .expect("the disjoint response graph is supported")
            .prepare_event_fields(5_000, 5_200)
            .expect("the pristine one-item graph accepts event fields")
            .prepare_scheduler_bookkeeping()
            .prepare_empty_list_link()
            .prepare_publication();
        let mismatched = BluetoothRxMemoryListPublished::from_parts_for_validation(
            BluetoothRxMemoryListClass::NonScanning.selector(),
            other_head,
        );

        let mismatch = match publication.into_rx_published(mismatched) {
            Ok(_) => panic!("another receive pool must not join this graph"),
            Err(mismatch) => mismatch,
        };
        assert_eq!(
            mismatch.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError::HeadMismatch
        );
        let (publication, mismatched) = mismatch.into_parts();
        assert_eq!(publication.identity(), graph_identity);
        assert_eq!(publication.receive_identity(), pool_identity);
        assert_eq!(mismatched.head(), other_head);

        let prepared = publication.cancel().cancel().cancel().cancel();
        let (owner, pool) = prepared.cancel();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
        assert!(other_pool.is_initialized());
    }

    #[test]
    fn event_field_rejection_retains_both_affine_owners() {
        let owner = owner(0x2f00_2000);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_6000);
        let pool_identity = pool.identity();
        let prepared = owner
            .prepare_response_capable_event(
                input(BluetoothLegacyAdvertisingPrimaryChannel::Channel39),
                pool,
                0,
            )
            .expect("the disjoint response graph is supported");
        prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .emulate_missing_scheduler_head();

        let failure = match prepared.prepare_event_fields(2_000, 2_200) {
            Ok(_) => panic!("a graph without its private scheduler head must fail closed"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError::SchedulerHeadMismatch
        );
        let (prepared, error) = failure.into_parts();
        assert_eq!(
            error,
            BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError::SchedulerHeadMismatch
        );
        assert_eq!(prepared.identity(), graph_identity);
        assert_eq!(prepared.receive_identity(), pool_identity);

        let (owner, pool) = prepared.cancel();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
    }

    #[test]
    fn packet_fit_is_proved_without_interpreting_protocol_headers() {
        let oversized = [0; 40];
        assert_eq!(
            BluetoothLegacyConnectableAdvIndPacketInput::try_from_encoded_extent(&oversized, 38),
            Err(
                BluetoothLegacyConnectableAdvertisingPduFitError::PayloadExceedsAllocation {
                    payload_bytes: 38,
                    capacity: 37,
                }
            )
        );
        assert_eq!(
            BluetoothLegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
                &[0xaa, 0xbb, 0xcc],
                2,
            ),
            Err(
                BluetoothLegacyConnectableAdvertisingPduFitError::EncodedExtentMismatch {
                    expected_bytes: 4,
                    actual_bytes: 3,
                }
            )
        );
    }

    #[test]
    fn missing_rx_consumer_link_blocks_lowering_and_cancel_recovers_both_owners() {
        let owner = owner(0x2f00_1800);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_5800);
        let pool_identity = pool.identity();
        let prepared = owner
            .prepare_response_capable_event(
                input(BluetoothLegacyAdvertisingPrimaryChannel::Channel39),
                pool,
                0,
            )
            .expect("the complete response topology is initially ready");
        prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .emulate_missing_rx_consumer_link();

        assert!(!prepared.is_ready_for_scheduler_lowering());
        let (owner, pool) = prepared.cancel();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
    }

    #[test]
    fn overlapping_rx_pool_is_rejected_without_losing_either_owner() {
        let owner = owner(0x2f00_6000);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_6000);
        let pool_identity = pool.identity();
        let failure = match owner.prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel37),
            pool,
            0,
        ) {
            Ok(_) => panic!("overlapping controller-memory extents must fail closed"),
            Err(failure) => failure,
        };

        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolOverlapsGraph
        );
        let (owner, pool, _) = failure.into_parts();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
    }
}
