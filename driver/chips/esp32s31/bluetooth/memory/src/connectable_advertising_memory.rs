//! CPU-owned ESP32-S31 memory for one response-capable legacy advertisement.
//!
//! This boundary binds the two transmit PDUs and the reusable non-scanning
//! receive pool into one private graph, then prepares the sole scheduler item
//! up to an empty-list candidate. It exposes that item's typed identity to the
//! chip scheduler but owns no list-head/RX publication, MMIO, or RUN operation.

#![forbid(unsafe_code)]

mod codec;

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress;
use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddressError;
use pin_project::pin_project;

use crate::{
    BluetoothLegacyAdvertisingPrimaryChannel, BluetoothNonScanningRxMemoryCpuOwned,
    BluetoothNonScanningRxMemoryIdentity,
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, BluetoothLeTxPacketPreparedInput,
        BluetoothLeTxPacketPreparedLength,
    },
    legacy_advertising_event_image::BluetoothLegacyAdvertisingOwnAddress,
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

/// Complete reviewed scheduler span for one response-capable LE 1M item.
///
/// The private codec owns the constituent packet and controller-tail policy;
/// this type exposes only their combined physical duration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyConnectableAdvertisingSchedulerSpan(u32);

impl BluetoothLegacyConnectableAdvertisingSchedulerSpan {
    /// Combined duration in microseconds before controller-epoch projection.
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
        let scheduler_span =
            codec::response_capable_scheduler_span(input.adv_ind.0.payload_bytes());
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
            scheduler_span,
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
    scheduler_span: BluetoothLegacyConnectableAdvertisingSchedulerSpan,
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

    pub const fn scheduler_span(&self) -> BluetoothLegacyConnectableAdvertisingSchedulerSpan {
        self.scheduler_span
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
