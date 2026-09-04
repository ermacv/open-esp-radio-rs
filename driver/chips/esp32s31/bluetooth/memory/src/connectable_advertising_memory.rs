//! CPU-owned ESP32-S31 memory for one response-capable legacy advertisement.
//!
//! This boundary stops before scheduler admission. It binds the two transmit
//! PDUs and the reusable non-scanning receive pool into one private graph, but
//! deliberately exposes no scheduler-item address or publication operation.

#![forbid(unsafe_code)]

mod codec;

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_bluetooth_ll::{
    LeDeviceAddressKind, advertising::PrimaryAdvertisingChannel,
    connectable_advertising::LegacyPreparedConnectableAdvertisingEvent,
};
#[cfg(not(target_arch = "riscv32"))]
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
};

const LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = 37;
const LEGACY_ADVERTISING_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

type AdvertisingTxPacketLength =
    BluetoothLeTxPacketPreparedLength<LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketInput<'a> =
    BluetoothLeTxPacketPreparedInput<'a, LEGACY_ADVERTISING_TX_PACKET_BYTES>;

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
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc rejection returns the portable event and both exact affine memory owners"
        )
    )]
    pub fn prepare_response_capable_event<'a>(
        mut self,
        event: LegacyPreparedConnectableAdvertisingEvent<'a>,
        pool: BluetoothNonScanningRxMemoryCpuOwned,
        default_tx_power_dbm: i8,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared<'a>,
        BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure<'a>,
    > {
        if event.channels().channel_count() != 1 {
            return Err(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure::new(
                self,
                pool,
                event,
                BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::RequiresOnePrimaryChannel,
            ));
        }
        if !pool.is_initialized() {
            return Err(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure::new(
                self,
                pool,
                event,
                BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolNotReady,
            ));
        }
        if !self.binding.is_disjoint_from_receive_pool(&pool) {
            return Err(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure::new(
                self,
                pool,
                event,
                BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolOverlapsGraph,
            ));
        }

        let primary_channel = event
            .channels()
            .channel(0)
            .map(s31_primary_channel)
            .expect("a prepared one-channel event contains its primary channel");
        let advertiser = event.advertisement().advertiser();
        let own_address = match advertiser.kind() {
            LeDeviceAddressKind::Public => BluetoothLegacyAdvertisingOwnAddress::Public,
            LeDeviceAddressKind::Random => {
                BluetoothLegacyAdvertisingOwnAddress::Random(advertiser.wire_bytes())
            }
        };
        let adv_ind = event.adv_ind_pdu();
        let scan_response = event.scan_response_pdu();
        let adv_ind_packet = AdvertisingTxPacketInput::from_validated_encoded_pdu(
            adv_ind.as_bytes(),
            adv_ind.payload_length(),
        );
        let scan_response_packet = AdvertisingTxPacketInput::from_validated_encoded_pdu(
            scan_response.as_bytes(),
            scan_response.payload_length(),
        );
        let (adv_ind_length, scan_response_length) = self
            .storage
            .as_mut()
            .project()
            .graph
            .prepare_pdus(adv_ind_packet, scan_response_packet);
        let scheduler_span = codec::response_capable_scheduler_span(adv_ind.payload_length());
        self.storage.as_ref().get_ref().graph.prepare_profile(
            &self.binding,
            pool.head(),
            pool.tail(),
            own_address,
            default_tx_power_dbm,
        );

        Ok(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared {
            storage: self.storage,
            binding: self.binding,
            pool,
            event,
            adv_ind_length,
            scan_response_length,
            primary_channel,
            scheduler_span,
        })
    }
}

const fn s31_primary_channel(
    channel: PrimaryAdvertisingChannel,
) -> BluetoothLegacyAdvertisingPrimaryChannel {
    match channel {
        PrimaryAdvertisingChannel::Channel37 => BluetoothLegacyAdvertisingPrimaryChannel::Channel37,
        PrimaryAdvertisingChannel::Channel38 => BluetoothLegacyAdvertisingPrimaryChannel::Channel38,
        PrimaryAdvertisingChannel::Channel39 => BluetoothLegacyAdvertisingPrimaryChannel::Channel39,
    }
}

/// Prepared response-capable graph with no publication authority.
#[must_use = "the prepared graph and receive pool must be retained or cancelled"]
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared<'a> {
    storage: Pin<&'static mut BluetoothLegacyConnectableAdvertisingMemoryGraphStorage>,
    binding: BluetoothLegacyConnectableAdvertisingGraphBinding,
    pool: BluetoothNonScanningRxMemoryCpuOwned,
    event: LegacyPreparedConnectableAdvertisingEvent<'a>,
    adv_ind_length: AdvertisingTxPacketLength,
    scan_response_length: AdvertisingTxPacketLength,
    primary_channel: BluetoothLegacyAdvertisingPrimaryChannel,
    scheduler_span: BluetoothLegacyConnectableAdvertisingSchedulerSpan,
}

impl<'a> BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared<'a> {
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
        LegacyPreparedConnectableAdvertisingEvent<'a>,
    ) {
        let mut owner = BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.reinitialize_graph();
        (owner, self.pool, self.event)
    }
}

/// Why the complete response-capable graph could not be prepared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError {
    RequiresOnePrimaryChannel,
    ReceivePoolNotReady,
    ReceivePoolOverlapsGraph,
}

/// Failed preparation retaining both affine memory owners.
pub struct BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure<'a> {
    owner: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    pool: BluetoothNonScanningRxMemoryCpuOwned,
    event: LegacyPreparedConnectableAdvertisingEvent<'a>,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
}

impl<'a> BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure<'a> {
    fn new(
        owner: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        pool: BluetoothNonScanningRxMemoryCpuOwned,
        event: LegacyPreparedConnectableAdvertisingEvent<'a>,
        error: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    ) -> Self {
        Self {
            owner,
            pool,
            event,
            error,
        }
    }

    pub const fn error(&self) -> BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        BluetoothNonScanningRxMemoryCpuOwned,
        LegacyPreparedConnectableAdvertisingEvent<'a>,
        BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    ) {
        (self.owner, self.pool, self.event, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareFailure<'_> {
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
    use open_esp_radio_bluetooth_ll::{
        LeDeviceAddress,
        advertising::{AdvertisingInterval, LegacyAdvertisingData, PrimaryAdvertisingChannelMap},
        connectable_advertising::{
            LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
            LegacyConnectableAdvertisingSet, LegacyPreparedConnectableAdvertisingEvent,
            LegacyScanResponseData,
        },
    };

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

    fn event(
        channels: PrimaryAdvertisingChannelMap,
    ) -> LegacyPreparedConnectableAdvertisingEvent<'static> {
        LegacyConnectableAdvertisingSet::new(
            LegacyConnectableAdvertisement::new(
                LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], LeDeviceAddressKind::Random),
                LegacyAdvertisingData::new(&[2, 1, 6]).expect("advertising data fits"),
                LeChannelSelectionAlgorithmTwoSupport::Supported,
            ),
            LegacyScanResponseData::new(&[]).expect("an empty scan response is valid"),
            channels,
            AdvertisingInterval::new(32).expect("the minimum interval is valid"),
        )
        .begin_event()
        .prepare()
    }

    fn one_channel() -> PrimaryAdvertisingChannelMap {
        PrimaryAdvertisingChannelMap::new(true, false, false)
            .expect("channel 37 forms a non-empty plan")
    }

    #[test]
    fn response_graph_owns_both_pdus_and_receive_pool_until_cancelled() {
        let owner = owner(0x2f00_0100);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_4000);
        let pool_identity = pool.identity();
        let event = event(one_channel());
        let adv_ind = event.adv_ind_pdu();
        let scan_response = event.scan_response_pdu();
        let prepared = owner
            .prepare_response_capable_event(event, pool, 0)
            .expect("the disjoint one-channel response graph is supported");

        assert_eq!(prepared.identity(), graph_identity);
        assert_eq!(prepared.receive_identity(), pool_identity);
        assert_eq!(prepared.adv_ind_pdu(), adv_ind.as_bytes());
        assert_eq!(prepared.scan_response_pdu(), scan_response.as_bytes());
        assert_eq!(
            prepared.primary_channel(),
            BluetoothLegacyAdvertisingPrimaryChannel::Channel37
        );
        assert!(prepared.is_ready_for_scheduler_lowering());

        let (owner, pool, event) = prepared.cancel();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
        assert_eq!(event.channels(), one_channel());
    }

    #[test]
    fn unsupported_channel_plan_returns_both_owners_for_retry() {
        let owner = owner(0x2f00_0800);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_4800);
        let pool_identity = pool.identity();
        let channels = PrimaryAdvertisingChannelMap::new(true, true, false)
            .expect("two channels are a valid generic advertising plan");
        let failure = match owner.prepare_response_capable_event(event(channels), pool, 0) {
            Ok(_) => panic!("response-capable multi-channel buffering is not yet proven"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::RequiresOnePrimaryChannel
        );
        let (owner, pool, unsupported_event, _) = failure.into_parts();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        let _unsupported_set = unsupported_event.cancel().into_set();

        assert!(
            owner
                .prepare_response_capable_event(event(one_channel()), pool, 0)
                .is_ok()
        );
    }

    #[test]
    fn missing_rx_consumer_link_blocks_lowering_and_cancel_recovers_both_owners() {
        let owner = owner(0x2f00_1800);
        let graph_identity = owner.identity();
        let pool = pool(0x2f00_5800);
        let pool_identity = pool.identity();
        let prepared = owner
            .prepare_response_capable_event(event(one_channel()), pool, 0)
            .expect("the complete response topology is initially ready");
        prepared
            .storage
            .as_ref()
            .get_ref()
            .graph
            .emulate_missing_rx_consumer_link();

        assert!(!prepared.is_ready_for_scheduler_lowering());
        let (owner, pool, _) = prepared.cancel();
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
        let failure = match owner.prepare_response_capable_event(event(one_channel()), pool, 0) {
            Ok(_) => panic!("overlapping controller-memory extents must fail closed"),
            Err(failure) => failure,
        };

        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolOverlapsGraph
        );
        let (owner, pool, _, _) = failure.into_parts();
        assert_eq!(owner.identity(), graph_identity);
        assert_eq!(pool.identity(), pool_identity);
        assert!(pool.is_initialized());
    }
}
