//! Permanently located RX/TX slots for bounded, copy-minimal network ownership.

#[cfg(feature = "tx-egress-scheduling")]
use core::num::{NonZeroU8, NonZeroU32};
use core::{
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    task::{Context, Poll},
};

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
use crate::tx_performance::TxShadowGrantObservation;
#[cfg(feature = "tx-phase-telemetry")]
use crate::tx_performance::{TX_PERFORMANCE, TxPerformanceSample};
use embassy_net_driver::{
    Capabilities, Checksum, ChecksumCapabilities, Driver, HardwareAddress, LinkState,
};
#[cfg(feature = "tx-egress-scheduling")]
use embassy_net_driver::{
    EgressAdmission, EgressBurstGrant as DriverEgressBurstGrant, EgressDemandUpdate,
    EgressGrantCompletion, EgressGrantMode, EgressKey, EgressRoute, EgressSchedule,
};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError},
    signal::Signal,
    waitqueue::GenericAtomicWaker,
};
use open_esp_radio_dma::{
    DmaIndexReturn, ExternalRxHandoffPool, ExternalRxNetworkLease, PinnedDmaTxNetworkLease,
    PinnedDmaTxPool, PinnedDmaTxRadioLease, ReturningStableDmaBacking, RxHandoffPool,
    RxNetworkLease, RxRadioLease, TaggedStableDmaBacking,
};

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
use crate::EgressGrantKey;
#[cfg(feature = "tx-phase-telemetry")]
use crate::EgressShadowGrant;
#[cfg(feature = "tx-egress-scheduling")]
use crate::egress_control::egress_control_enabled;
#[cfg(feature = "tx-egress-scheduling")]
use crate::{
    AssociatedEgressIdentity, DefaultEgressNetworkScheduler, EgressBurstGrant, EgressGrantProgress,
};
use crate::{
    ETHERNET_HEADER_LEN, EgressPeerResolver, FrameLengthError, RxEnqueueError, SharedLinkState,
};

#[cfg(feature = "tx-egress-scheduling")]
const PINNED_EGRESS_GRANT_PIPELINE_DEPTH: usize =
    crate::egress_control::DEFAULT_EGRESS_GRANT_DEPTH;

/// Opaque identity of one logical network endpoint sharing a physical radio.
///
/// The network adapter preserves this value but never assigns Wi-Fi meaning
/// to it. The radio composition owns the mapping to STA, AP, or another VIF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceId(u8);

impl NetworkInterfaceId {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Immutable network classification retained with one physical TX owner.
///
/// The opaque egress key contains the scheduling epoch and the driver's
/// generation-bound peer identity. It remains CPU-only metadata: neither the
/// Ethernet frame nor the DMA allocation is enlarged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinnedTxMetadata {
    interface: NetworkInterfaceId,
    #[cfg(feature = "tx-egress-scheduling")]
    egress_key: Option<EgressKey>,
}

impl PinnedTxMetadata {
    const fn unclassified(interface: NetworkInterfaceId) -> Self {
        Self {
            interface,
            #[cfg(feature = "tx-egress-scheduling")]
            egress_key: None,
        }
    }

    #[cfg(feature = "tx-egress-scheduling")]
    const fn classified(interface: NetworkInterfaceId, egress_key: EgressKey) -> Self {
        Self {
            interface,
            egress_key: Some(egress_key),
        }
    }

    pub const fn interface(self) -> NetworkInterfaceId {
        self.interface
    }

    #[cfg(feature = "tx-egress-scheduling")]
    pub const fn egress_key(self) -> Option<EgressKey> {
        self.egress_key
    }

    /// Decode the generic associated-peer identity retained at final SRAM
    /// admission.
    ///
    /// The result still carries a generic traffic class. It is not transmit
    /// authority and does not map that class to a Wi-Fi TID; the radio role
    /// must perform both operations against its current state.
    #[cfg(feature = "tx-egress-scheduling")]
    pub fn associated_peer_identity(self) -> Option<AssociatedEgressIdentity> {
        self.egress_key.and_then(AssociatedEgressIdentity::decode)
    }
}

/// Compact identity of one affine TX owner and its immutable sidecar slot.
///
/// The handle is intentionally only two bytes. Radio state machines may keep
/// many packet owners across aggregate and retry phases without copying the
/// complete generic egress key into every enum variant. The pool slot cannot
/// be reused while its packet owner is live, so the referenced metadata has
/// exactly the same lifetime as the owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinnedTxOwnerTag {
    interface: NetworkInterfaceId,
    pool_index: u8,
}

impl PinnedTxOwnerTag {
    pub const fn new(interface: NetworkInterfaceId, pool_index: u8) -> Self {
        Self {
            interface,
            pool_index,
        }
    }

    pub const fn interface(self) -> NetworkInterfaceId {
        self.interface
    }

    pub const fn pool_index(self) -> u8 {
        self.pool_index
    }
}

/// Metadata sidecar paired by index with one physical packet pool.
///
/// The affine pool state remains the ownership protocol: Core1 writes before
/// publishing the one-byte index and Core0 reads after receiving it. Relaxed
/// atomics make the sidecar data-race-safe without adding a lock or another
/// fence. The final format-word release and the reader's acquire form an
/// explicit sidecar publication edge, independent of the queue implementation.
/// Keeping the four opaque words out of both packet owners and queues avoids
/// multiplying them by every aggregate state and per-VIF channel slot.
struct PinnedTxMetadataSlot {
    #[cfg(feature = "tx-egress-scheduling")]
    words: [AtomicU32; 4],
}

impl PinnedTxMetadataSlot {
    const fn new() -> Self {
        Self {
            #[cfg(feature = "tx-egress-scheduling")]
            words: [const { AtomicU32::new(0) }; 4],
        }
    }

    fn publish(&self, metadata: PinnedTxMetadata) {
        #[cfg(feature = "tx-egress-scheduling")]
        match metadata.egress_key() {
            Some(key) => {
                let words = key.words();
                debug_assert_ne!(words[0], 0, "classified driver key has a format word");
                for (destination, word) in self.words[1..].iter().zip(words[1..].iter()) {
                    destination.store(*word, Ordering::Relaxed);
                }
                self.words[0].store(words[0], Ordering::Release);
            }
            None => self.words[0].store(0, Ordering::Release),
        }
        #[cfg(not(feature = "tx-egress-scheduling"))]
        let _ = metadata;
    }

    fn read(&self, interface: NetworkInterfaceId) -> PinnedTxMetadata {
        #[cfg(feature = "tx-egress-scheduling")]
        {
            let first = self.words[0].load(Ordering::Acquire);
            if first == 0 {
                return PinnedTxMetadata::unclassified(interface);
            }
            PinnedTxMetadata::classified(
                interface,
                EgressKey::from_words([
                    first,
                    self.words[1].load(Ordering::Relaxed),
                    self.words[2].load(Ordering::Relaxed),
                    self.words[3].load(Ordering::Relaxed),
                ]),
            )
        }
        #[cfg(not(feature = "tx-egress-scheduling"))]
        PinnedTxMetadata::unclassified(interface)
    }
}

/// How stack-resolved link routes map to the physical egress scheduler.
///
/// This describes queue geometry only. It neither authorizes a radio peer nor
/// replaces the radio owner's association-generation and airtime checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EgressQueueTopology {
    /// Every link destination reaches one physical radio peer.
    ///
    /// Infrastructure STA is the canonical example: all Ethernet traffic is
    /// transmitted to the associated BSSID even when the destination is a
    /// different host behind that access point.
    SingleRadioPeer,
    /// Each Ethernet destination owns an independent scheduling domain.
    ///
    /// This is the fail-closed fallback for a multi-peer endpoint without a
    /// published association identity. The radio owner still validates that
    /// the destination belongs to a live peer generation.
    PerLinkDestination,
    /// Unicast routes resolve through a generation-bound radio peer table.
    ///
    /// SoftAP uses this topology. The lookup controls queue grouping only;
    /// final radio admission validates the same generation independently.
    AssociatedPeer,
}

/// Immutable network endpoint identity and egress queue topology.
#[derive(Clone, Copy)]
pub struct NetworkEndpointConfig<'registry> {
    interface: NetworkInterfaceId,
    hardware_address: [u8; 6],
    egress_topology: EgressQueueTopology,
    #[cfg_attr(not(feature = "tx-egress-scheduling"), allow(dead_code))]
    peer_resolver: Option<&'registry dyn EgressPeerResolver>,
    #[cfg(feature = "tx-phase-telemetry")]
    shadow_grant: Option<&'registry EgressShadowGrant>,
}

impl<'registry> NetworkEndpointConfig<'registry> {
    /// Configure an endpoint whose routes all reach one physical radio peer.
    pub const fn single_radio_peer(
        interface: NetworkInterfaceId,
        hardware_address: [u8; 6],
    ) -> Self {
        Self {
            interface,
            hardware_address,
            egress_topology: EgressQueueTopology::SingleRadioPeer,
            peer_resolver: None,
            #[cfg(feature = "tx-phase-telemetry")]
            shadow_grant: None,
        }
    }

    /// Configure an endpoint whose Ethernet destinations are independent
    /// scheduling domains.
    pub const fn per_link_destination(
        interface: NetworkInterfaceId,
        hardware_address: [u8; 6],
    ) -> Self {
        Self {
            interface,
            hardware_address,
            egress_topology: EgressQueueTopology::PerLinkDestination,
            peer_resolver: None,
            #[cfg(feature = "tx-phase-telemetry")]
            shadow_grant: None,
        }
    }

    /// Configure a multi-peer endpoint from a radio-owned peer snapshot.
    pub const fn associated_peers(
        interface: NetworkInterfaceId,
        hardware_address: [u8; 6],
        peer_resolver: &'registry dyn EgressPeerResolver,
    ) -> Self {
        NetworkEndpointConfig {
            interface,
            hardware_address,
            egress_topology: EgressQueueTopology::AssociatedPeer,
            peer_resolver: Some(peer_resolver),
            #[cfg(feature = "tx-phase-telemetry")]
            shadow_grant: None,
        }
    }

    /// Attach a diagnostic Core0 grant publication to this endpoint.
    ///
    /// Shadow observations never change admission and are compiled out of
    /// ordinary production images.
    #[cfg(feature = "tx-phase-telemetry")]
    pub const fn with_shadow_grant(mut self, grant: &'registry EgressShadowGrant) -> Self {
        self.shadow_grant = Some(grant);
        self
    }

    pub const fn interface(self) -> NetworkInterfaceId {
        self.interface
    }

    pub const fn hardware_address(self) -> [u8; 6] {
        self.hardware_address
    }

    pub const fn egress_topology(self) -> EgressQueueTopology {
        self.egress_topology
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn classify(self, epoch: u32, route: EgressRoute) -> EgressKey {
        use crate::egress_key::{
            ASSOCIATED_PEER, KEY_FORMAT, PER_LINK_DESTINATION, SINGLE_RADIO_PEER,
        };

        let header =
            KEY_FORMAT | (u32::from(self.interface.value()) << 8) | u32::from(route.traffic_class);
        match (self.egress_topology, route.destination) {
            (EgressQueueTopology::SingleRadioPeer, HardwareAddress::Ethernet(_)) => {
                EgressKey::from_words([header | SINGLE_RADIO_PEER, epoch, 0, 0])
            }
            (EgressQueueTopology::PerLinkDestination, HardwareAddress::Ethernet(address)) => {
                EgressKey::from_words([
                    header | PER_LINK_DESTINATION,
                    epoch,
                    u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
                    u32::from(u16::from_le_bytes([address[4], address[5]])),
                ])
            }
            (EgressQueueTopology::AssociatedPeer, HardwareAddress::Ethernet(address)) => self
                .peer_resolver
                .and_then(|resolver| resolver.resolve(address))
                .map_or_else(
                    || {
                        EgressKey::from_words([
                            header | PER_LINK_DESTINATION,
                            epoch,
                            u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
                            u32::from(u16::from_le_bytes([address[4], address[5]])),
                        ])
                    },
                    |peer| {
                        EgressKey::from_words([
                            header | ASSOCIATED_PEER,
                            epoch,
                            peer.generation().get(),
                            u32::from(peer.slot().get()),
                        ])
                    },
                ),
            _ => EgressKey::from_route(route),
        }
    }

    /// Revalidate an opaque stack-retained key immediately before physical
    /// SRAM admission.
    ///
    /// Route classification and final token allocation are separated by
    /// arbitrary stack scheduling. Link lifecycle and AP reassociation may
    /// advance in between, so a successful earlier lookup cannot authorize a
    /// later DMA credit. This check remains policy-free: a future airtime
    /// scheduler may additionally defer a valid key.
    #[cfg(feature = "tx-egress-scheduling")]
    fn key_is_current(self, key: EgressKey, epoch: u32) -> bool {
        use crate::egress_key::{
            ASSOCIATED_PEER, KEY_FORMAT, KEY_FORMAT_MASK, PER_LINK_DESTINATION, SINGLE_RADIO_PEER,
            TOPOLOGY_MASK,
        };

        let [header, key_epoch, generation, slot] = key.words();
        if header & KEY_FORMAT_MASK != KEY_FORMAT
            || ((header >> 8) & 0xff) != u32::from(self.interface.value())
            || key_epoch != epoch
        {
            return false;
        }

        match (self.egress_topology, header & TOPOLOGY_MASK) {
            (EgressQueueTopology::SingleRadioPeer, SINGLE_RADIO_PEER) => {
                generation == 0 && slot == 0
            }
            (EgressQueueTopology::PerLinkDestination, PER_LINK_DESTINATION) => {
                slot <= u32::from(u16::MAX)
            }
            (EgressQueueTopology::AssociatedPeer, PER_LINK_DESTINATION) => {
                // Unknown unicast and group destinations retain their full
                // link identity. A peer-directory revision advances `epoch`,
                // so an old fallback key cannot survive association.
                slot <= u32::from(u16::MAX)
            }
            (EgressQueueTopology::AssociatedPeer, ASSOCIATED_PEER) => {
                let Some(slot) = u8::try_from(slot).ok().and_then(NonZeroU8::new) else {
                    return false;
                };
                let Some(generation) = NonZeroU32::new(generation) else {
                    return false;
                };
                self.peer_resolver
                    .is_some_and(|resolver| resolver.is_current(slot, generation))
            }
            _ => false,
        }
    }

    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    fn grant_key(self, key: EgressKey) -> Option<EgressGrantKey> {
        let identity = AssociatedEgressIdentity::decode(key)?;
        if self.egress_topology != EgressQueueTopology::AssociatedPeer
            || identity.interface() != self.interface.value()
            || identity.traffic_class() != 0
        {
            return None;
        }
        Some(EgressGrantKey::new(
            identity.interface(),
            identity.peer_slot(),
            identity.peer_generation(),
            // The AP currently negotiates only best-effort TID 0. The generic
            // route traffic class is not silently treated as a WMM mapping.
            0,
        ))
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn peer_revision(self) -> u32 {
        self.peer_resolver.map_or(0, EgressPeerResolver::revision)
    }
}

/// Point-in-time ownership geometry of the direct pinned TX pool.
///
/// This is diagnostic evidence, not a scheduling input. Counts may change
/// immediately after the snapshot because Core0 and Core1 own disjoint
/// transitions concurrently. A complete accounting sample nevertheless lets
/// the AP distinguish a genuinely empty producer frontier from credits held
/// by another VIF or by the network driver's synchronous token boundary.
#[cfg(feature = "tx-phase-telemetry")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PinnedTxOwnershipSnapshot {
    /// All immediately available physical credits, including `control_free`.
    pub free: usize,
    /// Informational subset of `free` reserved for bounded control traffic.
    pub control_free: usize,
    pub ready_for_interface: usize,
    pub ready_for_other_interfaces: usize,
    pub ingress_reserved: usize,
    pub application_reserved: usize,
    pub control_reserved: usize,
    pub tokens_in_flight: usize,
}

#[cfg(feature = "tx-phase-telemetry")]
impl PinnedTxOwnershipSnapshot {
    pub fn radio_owned(self, capacity: usize) -> usize {
        capacity.saturating_sub(
            self.free
                .saturating_add(self.ready_for_interface)
                .saturating_add(self.ready_for_other_interfaces)
                .saturating_add(self.ingress_reserved)
                .saturating_add(self.application_reserved)
                .saturating_add(self.control_reserved)
                .saturating_add(self.tokens_in_flight),
        )
    }
}

const PINNED_TX_CREDIT_WAKER_SLOTS: usize = 8;
#[cfg(feature = "tx-egress-scheduling")]
const UNINITIALIZED_CONTROL_TX_INDEX: usize = usize::MAX;
#[cfg(feature = "tx-egress-scheduling")]
const INITIALIZING_CONTROL_TX_INDEX: usize = usize::MAX - 1;

/// One HT A-MPDU worth of value-only ownership records.
///
/// Packet bytes remain in the two pinned pools. Only source/destination
/// indices cross cores, avoiding a self-referential static queue of leases.
#[cfg(feature = "tx-core1-materializer-probe")]
pub const PINNED_TX_MATERIALIZATION_BATCH_CAPACITY: usize = 32;

#[cfg(feature = "tx-core1-materializer-probe")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TxMaterializationPair {
    source: u8,
    destination: u8,
}

#[cfg(feature = "tx-core1-materializer-probe")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TxMaterializationRequest {
    interface: NetworkInterfaceId,
    count: u8,
    pairs: [TxMaterializationPair; PINNED_TX_MATERIALIZATION_BATCH_CAPACITY],
}

#[cfg(feature = "tx-core1-materializer-probe")]
impl TxMaterializationRequest {
    const fn empty(interface: NetworkInterfaceId) -> Self {
        Self {
            interface,
            count: 0,
            pairs: [TxMaterializationPair {
                source: 0,
                destination: 0,
            }; PINNED_TX_MATERIALIZATION_BATCH_CAPACITY],
        }
    }
}

#[cfg(feature = "tx-core1-materializer-probe")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TxMaterializationCompletion {
    interface: NetworkInterfaceId,
    count: u8,
    destinations: [u8; PINNED_TX_MATERIALIZATION_BATCH_CAPACITY],
    cancelled: bool,
}

#[cfg(feature = "tx-core1-materializer-probe")]
impl TxMaterializationCompletion {
    const fn empty(interface: NetworkInterfaceId) -> Self {
        Self {
            interface,
            count: 0,
            destinations: [0; PINNED_TX_MATERIALIZATION_BATCH_CAPACITY],
            cancelled: false,
        }
    }
}

#[cfg(feature = "tx-core1-materializer-probe")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxCore1MaterializerSnapshot {
    pub submitted_batches: u32,
    pub completed_batches: u32,
    pub materialized_frames: u32,
    pub no_credit: u32,
    pub cancelled_batches: u32,
}

#[cfg(feature = "tx-core1-materializer-probe")]
impl TxCore1MaterializerSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            submitted_batches: self
                .submitted_batches
                .wrapping_sub(earlier.submitted_batches),
            completed_batches: self
                .completed_batches
                .wrapping_sub(earlier.completed_batches),
            materialized_frames: self
                .materialized_frames
                .wrapping_sub(earlier.materialized_frames),
            no_credit: self.no_credit.wrapping_sub(earlier.no_credit),
            cancelled_batches: self
                .cancelled_batches
                .wrapping_sub(earlier.cancelled_batches),
        }
    }
}

#[cfg(feature = "tx-core1-materializer-probe")]
pub struct TxCore1MaterializerCounters {
    submitted_batches: AtomicU32,
    completed_batches: AtomicU32,
    materialized_frames: AtomicU32,
    no_credit: AtomicU32,
    cancelled_batches: AtomicU32,
}

#[cfg(feature = "tx-core1-materializer-probe")]
impl TxCore1MaterializerCounters {
    pub const fn new() -> Self {
        Self {
            submitted_batches: AtomicU32::new(0),
            completed_batches: AtomicU32::new(0),
            materialized_frames: AtomicU32::new(0),
            no_credit: AtomicU32::new(0),
            cancelled_batches: AtomicU32::new(0),
        }
    }

    pub fn snapshot(&self) -> TxCore1MaterializerSnapshot {
        TxCore1MaterializerSnapshot {
            submitted_batches: self.submitted_batches.load(Ordering::Relaxed),
            completed_batches: self.completed_batches.load(Ordering::Relaxed),
            materialized_frames: self.materialized_frames.load(Ordering::Relaxed),
            no_credit: self.no_credit.load(Ordering::Relaxed),
            cancelled_batches: self.cancelled_batches.load(Ordering::Relaxed),
        }
    }
}

#[cfg(feature = "tx-core1-materializer-probe")]
impl Default for TxCore1MaterializerCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "tx-core1-materializer-probe")]
pub static TX_CORE1_MATERIALIZER_COUNTERS: TxCore1MaterializerCounters =
    TxCore1MaterializerCounters::new();

#[cfg(feature = "tx-staging-copy-probe")]
static TX_STAGING_COPY_PROBE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "tx-core1-materializer-probe")]
static TX_CORE1_MATERIALIZER_PROBE: AtomicBool = AtomicBool::new(false);

/// Select the same-image PSRAM-to-DMA TX copy discriminator.
///
/// The network task materializes the frame in the endpoint's persistent PSRAM
/// scratch storage and copies it once into the existing final DMA slot. The
/// DMA slot remains the sole published owner. This measures the lower-bound
/// materialization cost; it does not add queues or change packet scheduling.
#[cfg(feature = "tx-staging-copy-probe")]
pub fn configure_tx_staging_copy_probe(enabled: bool) {
    TX_STAGING_COPY_PROBE.store(enabled, Ordering::Release);
}

/// Select Core1 execution for already scheduled staged TX batches.
#[cfg(feature = "tx-core1-materializer-probe")]
pub fn configure_tx_core1_materializer_probe(enabled: bool) {
    TX_CORE1_MATERIALIZER_PROBE.store(enabled, Ordering::Release);
}

#[cfg(feature = "tx-core1-materializer-probe")]
#[inline]
fn tx_core1_materializer_enabled() -> bool {
    TX_CORE1_MATERIALIZER_PROBE.load(Ordering::Acquire)
}

#[cfg(feature = "tx-staging-copy-probe")]
#[inline]
fn tx_staging_copy_enabled() -> bool {
    TX_STAGING_COPY_PROBE.load(Ordering::Acquire)
}

/// Per-endpoint notification for one shared physical TX credit pool.
///
/// `embassy_sync::Channel` intentionally stores one receiver waker. Multiple
/// logical network devices may consume this physical free queue, so polling
/// the channel directly would make their distinct task wakers replace and
/// wake each other forever while the queue is empty. The queue remains the
/// sole owner of free indices; this table wakes one active waiter on each real
/// credit-return edge.
struct PinnedTxCreditWakers<M: RawMutex> {
    slots: [GenericAtomicWaker<M>; PINNED_TX_CREDIT_WAKER_SLOTS],
}

impl<M: RawMutex> PinnedTxCreditWakers<M> {
    const fn new() -> Self {
        Self {
            slots: [const { GenericAtomicWaker::new(M::INIT) }; PINNED_TX_CREDIT_WAKER_SLOTS],
        }
    }

    fn register(&self, interface: NetworkInterfaceId, cx: &mut Context<'_>) {
        self.slots[usize::from(interface.value())].register(cx.waker());
    }

    #[cfg(feature = "tx-core1-materializer-probe")]
    fn wake(&self, interface: NetworkInterfaceId) {
        self.slots[usize::from(interface.value())].wake();
    }

    fn wake_all(&self) {
        for slot in &self.slots {
            slot.wake();
        }
    }

    fn wake_mask(&self, waiting: u32) {
        for (index, slot) in self.slots.iter().enumerate() {
            if waiting & (1_u32 << index) != 0 {
                slot.wake();
            }
        }
    }

    fn wake_waiter_after(
        &self,
        returned_by: NetworkInterfaceId,
        active: &AtomicU32,
        waiting: &AtomicU32,
    ) {
        let candidates = active.load(Ordering::Acquire) & waiting.load(Ordering::Acquire);
        let start = (usize::from(returned_by.value()) + 1) % PINNED_TX_CREDIT_WAKER_SLOTS;
        for offset in 0..PINNED_TX_CREDIT_WAKER_SLOTS {
            let index = (start + offset) % PINNED_TX_CREDIT_WAKER_SLOTS;
            if candidates & (1_u32 << index) != 0 {
                self.slots[index].wake();
                return;
            }
        }
    }

    fn validate(interface: NetworkInterfaceId) {
        assert!(
            usize::from(interface.value()) < PINNED_TX_CREDIT_WAKER_SLOTS,
            "network interface exceeds the physical TX credit notification table"
        );
    }
}

/// One publication frontier shared by copied and externally-backed RX slots.
///
/// The physical pools remain separate, but `embassy-net` observes this typed
/// stream in exact publication order. Its capacity covers the complete S31
/// production geometry (64 owned plus 32 shared slots) without relying on a
/// priority rule between two independent queues.
const ORDERED_RX_READY_CAPACITY: usize = 96;
const ORDERED_RX_SHARED_BIT: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OrderedRxSource {
    Owned(u8),
    Shared(u8),
}

/// Compact typed encoding for the common ready frontier. Production pool
/// indices are below 128, leaving the high bit as an unambiguous source tag
/// while keeping each channel record one byte wide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OrderedRxReady(u8);

const _: [(); 1] = [(); core::mem::size_of::<OrderedRxReady>()];

impl OrderedRxReady {
    fn owned(index: u8) -> Self {
        assert!(
            index < ORDERED_RX_SHARED_BIT,
            "ordered owned RX index must fit in seven bits"
        );
        Self(index)
    }

    fn shared(index: u8) -> Self {
        assert!(
            index < ORDERED_RX_SHARED_BIT,
            "ordered shared RX index must fit in seven bits"
        );
        Self(index | ORDERED_RX_SHARED_BIT)
    }

    const fn source(self) -> OrderedRxSource {
        if self.0 & ORDERED_RX_SHARED_BIT == 0 {
            OrderedRxSource::Owned(self.0)
        } else {
            OrderedRxSource::Shared(self.0 & !ORDERED_RX_SHARED_BIT)
        }
    }
}

/// Ready-index channel for an RX pool owned by a lower staging layer.
///
/// The bytes remain in that external [`RxHandoffPool`]. This resource stores
/// only the ownership publication edge consumed by the network device.
pub struct SharedPinnedRxQueue<M: RawMutex, const SLOT_COUNT: usize> {
    ready: Channel<M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>,
    split: AtomicBool,
}

impl<M: RawMutex, const SLOT_COUNT: usize> SharedPinnedRxQueue<M, SLOT_COUNT> {
    pub const fn new() -> Self {
        Self {
            ready: Channel::new(),
            split: AtomicBool::new(false),
        }
    }

    pub fn split<'resources, const FRAME_CAPACITY: usize>(
        &'resources self,
        pool: &'resources RxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>,
        on_release: fn(),
    ) -> (
        SharedPinnedRxPublisher<'resources, M, SLOT_COUNT>,
        SharedPinnedRxConsumer<'resources, M, FRAME_CAPACITY, SLOT_COUNT>,
    ) {
        assert!(SLOT_COUNT > 0, "shared pinned RX pool must not be empty");
        assert!(
            SLOT_COUNT <= usize::from(ORDERED_RX_SHARED_BIT),
            "shared pinned RX index must fit in seven bits"
        );
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "shared pinned RX queue may only be split once"
        );
        (
            SharedPinnedRxPublisher {
                ready: self.ready.sender(),
            },
            SharedPinnedRxConsumer {
                ready: self.ready.receiver(),
                ready_sender: self.ready.sender(),
                pool: SharedPinnedRxPool::Copied(pool),
                on_release,
            },
        )
    }

    /// Split a queue whose indices retain original descriptor-backed buffers.
    pub fn split_external<'resources, const FRAME_CAPACITY: usize>(
        &'resources self,
        pool: &'resources ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>,
        on_release: fn(),
    ) -> (
        SharedPinnedRxPublisher<'resources, M, SLOT_COUNT>,
        SharedPinnedRxConsumer<'resources, M, FRAME_CAPACITY, SLOT_COUNT>,
    ) {
        assert!(SLOT_COUNT > 0, "shared external RX pool must not be empty");
        assert!(
            SLOT_COUNT <= usize::from(ORDERED_RX_SHARED_BIT),
            "shared external RX index must fit in seven bits"
        );
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "shared external RX queue may only be split once"
        );
        (
            SharedPinnedRxPublisher {
                ready: self.ready.sender(),
            },
            SharedPinnedRxConsumer {
                ready: self.ready.receiver(),
                ready_sender: self.ready.sender(),
                pool: SharedPinnedRxPool::External(pool),
                on_release,
            },
        )
    }

    /// Recreate the cheap producer endpoint after the unique consumer has
    /// been installed. Sequential radio epochs may each own one such handle.
    pub fn publisher(&self) -> SharedPinnedRxPublisher<'_, M, SLOT_COUNT> {
        assert!(
            self.split.load(Ordering::Acquire),
            "shared pinned RX queue must be split before publication"
        );
        SharedPinnedRxPublisher {
            ready: self.ready.sender(),
        }
    }
}

impl<M: RawMutex, const SLOT_COUNT: usize> Default for SharedPinnedRxQueue<M, SLOT_COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

/// Protocol-side capability to publish one already formatted external slot.
pub struct SharedPinnedRxPublisher<'resources, M: RawMutex, const SLOT_COUNT: usize> {
    ready: Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>,
}

impl<M: RawMutex, const SLOT_COUNT: usize> Clone for SharedPinnedRxPublisher<'_, M, SLOT_COUNT> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const SLOT_COUNT: usize> Copy for SharedPinnedRxPublisher<'_, M, SLOT_COUNT> {}

impl<M: RawMutex, const SLOT_COUNT: usize> SharedPinnedRxPublisher<'_, M, SLOT_COUNT> {
    #[inline(always)]
    pub fn publish(&self, index: u8) {
        if let Err(TrySendError::Full(_)) = self.ready.try_send(OrderedRxReady::shared(index)) {
            unreachable!("ordered RX frontier covers every owned and shared slot");
        }
    }

    pub fn queue_len(&self) -> usize {
        self.ready.len()
    }
}

pub struct SharedPinnedRxConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const SLOT_COUNT: usize,
> {
    ready: Receiver<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>,
    ready_sender: Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>,
    pool: SharedPinnedRxPool<'resources, FRAME_CAPACITY, SLOT_COUNT>,
    on_release: fn(),
}

enum SharedPinnedRxPool<'resources, const FRAME_CAPACITY: usize, const SLOT_COUNT: usize> {
    Copied(&'resources RxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>),
    External(&'resources ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>),
}

impl<'resources, const FRAME_CAPACITY: usize, const SLOT_COUNT: usize>
    SharedPinnedRxPool<'resources, FRAME_CAPACITY, SLOT_COUNT>
{
    fn claim_network(&self, index: u8) -> SharedPoolNetworkLease<'resources, FRAME_CAPACITY> {
        match self {
            Self::Copied(pool) => SharedPoolNetworkLease::Copied(pool.claim_network(index)),
            Self::External(pool) => SharedPoolNetworkLease::External(pool.claim_network(index)),
        }
    }
}

/// Static resources for copy-minimal RX and copy-free TX ownership boundaries.
///
/// RX is copied once from the protocol adapter directly into its final slot;
/// only a slot index crosses the queue. [`PinnedTxPool`] owns the separate
/// DMA-visible TX slots. `embassy-net` sees only each TX slot's middle Ethernet
/// region; the radio lease sees the complete allocation and remains its unique
/// owner until dropped. The TX pool must be pinned before [`Self::split`].
///
/// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
/// ieee80211_alloc_tx_buf` cache-TX/type-nine path and complete
/// `libpp.a[esf_buf.o]::{esf_buf_setup,esf_buf_alloc}`.
pub struct PinnedTxResources<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    free_tx: Channel<M, u8, TX_QUEUE_DEPTH>,
    tx_metadata: [PinnedTxMetadataSlot; TX_QUEUE_DEPTH],
    /// Per-VIF FIFO publication frontiers sharing one finite physical credit
    /// pool. Cross-VIF order is chosen by the physical consumer; within each
    /// VIF, publication order is immutable.
    ready_tx: [Channel<M, u8, TX_QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    /// CPU-owned packet tier used only by the one-copy architecture probe.
    /// This resource follows ordinary data placement and is never exposed as
    /// [`StableDmaBacking`].
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_pool: RxHandoffPool<FRAME_CAPACITY, TX_QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_metadata: [PinnedTxMetadataSlot; TX_QUEUE_DEPTH],
    #[cfg(feature = "tx-staging-copy-probe")]
    free_staged: Channel<M, u8, TX_QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    ready_staged: [Channel<M, u8, TX_QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_requests:
        [Channel<M, TxMaterializationRequest, 1>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_completions:
        [Channel<M, TxMaterializationCompletion, 1>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_in_flight: AtomicU32,
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_cancel: AtomicU32,
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_wakers: PinnedTxCreditWakers<M>,
    next_interface: AtomicU32,
    tx_published: Signal<M, ()>,
    tx_credit_wakers: PinnedTxCreditWakers<M>,
    tx_credit_waiters: AtomicU32,
    /// One lazily partitioned physical credit reserved for uncatalogued
    /// control providers after the egress control plane is attached.
    control_tx_index: AtomicUsize,
    control_tx_available: AtomicBool,
    control_tx_waiters: AtomicU32,
    #[cfg(feature = "tx-egress-scheduling")]
    control_tx_next_interface: AtomicU32,
    #[cfg(feature = "tx-staging-copy-probe")]
    tx_staged_interfaces: AtomicU32,
    split: AtomicBool,
    /// Radio-owned link activity for each logical endpoint. A permanent
    /// network device may exist while its role is stopped, so credit sharing
    /// must follow the active owner graph rather than the static device count.
    tx_active: AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_ingress_reserved: AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_application_reserved: AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_control_reserved: AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_tokens_in_flight: AtomicU32,
}

/// Static storage owned by one permanent logical network endpoint.
///
/// STA and AP must each have their own instance. Only RX ownership, link state
/// and the immutable Ethernet identity live here; physical TX storage belongs
/// to [`PinnedTxResources`] and is shared explicitly.
pub struct PinnedEndpointResources<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const RX_QUEUE_DEPTH: usize,
> {
    free_rx: Channel<M, u8, RX_QUEUE_DEPTH>,
    ready_rx: Channel<M, u8, RX_QUEUE_DEPTH>,
    rx_pool: RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    link: SharedLinkState<M>,
    split: AtomicBool,
}

/// Permanently located storage for the TX allocations exposed to radio DMA.
///
/// This is separate from [`PinnedTxResources`] so a platform linker can place
/// only the DMA-visible bytes in internal SRAM while keeping RX queues and
/// Embassy synchronization state in ordinary memory.
pub type PinnedTxPool<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
> PinnedTxResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            free_tx: Channel::new(),
            tx_metadata: [const { PinnedTxMetadataSlot::new() }; TX_QUEUE_DEPTH],
            ready_tx: [const { Channel::new() }; PINNED_TX_CREDIT_WAKER_SLOTS],
            #[cfg(feature = "tx-staging-copy-probe")]
            staged_pool: RxHandoffPool::new(),
            #[cfg(feature = "tx-staging-copy-probe")]
            staged_metadata: [const { PinnedTxMetadataSlot::new() }; TX_QUEUE_DEPTH],
            #[cfg(feature = "tx-staging-copy-probe")]
            free_staged: Channel::new(),
            #[cfg(feature = "tx-staging-copy-probe")]
            ready_staged: [const { Channel::new() }; PINNED_TX_CREDIT_WAKER_SLOTS],
            #[cfg(feature = "tx-core1-materializer-probe")]
            materialization_requests: [const { Channel::new() }; PINNED_TX_CREDIT_WAKER_SLOTS],
            #[cfg(feature = "tx-core1-materializer-probe")]
            materialization_completions: [const { Channel::new() }; PINNED_TX_CREDIT_WAKER_SLOTS],
            #[cfg(feature = "tx-core1-materializer-probe")]
            materialization_in_flight: AtomicU32::new(0),
            #[cfg(feature = "tx-core1-materializer-probe")]
            materialization_cancel: AtomicU32::new(0),
            #[cfg(feature = "tx-core1-materializer-probe")]
            materialization_wakers: PinnedTxCreditWakers::new(),
            next_interface: AtomicU32::new(0),
            tx_published: Signal::new(),
            tx_credit_wakers: PinnedTxCreditWakers::new(),
            tx_credit_waiters: AtomicU32::new(0),
            control_tx_index: AtomicUsize::new(usize::MAX),
            control_tx_available: AtomicBool::new(false),
            control_tx_waiters: AtomicU32::new(0),
            #[cfg(feature = "tx-egress-scheduling")]
            control_tx_next_interface: AtomicU32::new(0),
            #[cfg(feature = "tx-staging-copy-probe")]
            tx_staged_interfaces: AtomicU32::new(0),
            split: AtomicBool::new(false),
            tx_active: AtomicU32::new(0),
            #[cfg(feature = "tx-phase-telemetry")]
            tx_ingress_reserved: AtomicU32::new(0),
            #[cfg(feature = "tx-phase-telemetry")]
            tx_application_reserved: AtomicU32::new(0),
            #[cfg(feature = "tx-phase-telemetry")]
            tx_control_reserved: AtomicU32::new(0),
            #[cfg(feature = "tx-phase-telemetry")]
            tx_tokens_in_flight: AtomicU32::new(0),
        }
    }

    pub fn split<'resources>(
        &'resources mut self,
        pool: Pin<&'resources mut PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>,
    ) -> (
        PinnedTxProvider<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) {
        assert!(TX_QUEUE_DEPTH > 0, "pinned TX pool must not be empty");
        assert!(
            TX_QUEUE_DEPTH <= usize::from(u8::MAX) + 1,
            "pinned TX pool index must fit in u8"
        );
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "pinned resources may only be split once"
        );
        for index in 0..TX_QUEUE_DEPTH {
            self.free_tx
                .try_send(index as u8)
                .expect("an empty free queue accepts every pool index");
            #[cfg(feature = "tx-staging-copy-probe")]
            {
                self.free_staged
                    .try_send(index as u8)
                    .expect("an empty staged free queue accepts every pool index");
            }
        }
        let pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> =
            Pin::into_ref(pool).get_ref();
        let resources: &Self = self;

        (
            PinnedTxProvider {
                free_tx: resources.free_tx.receiver(),
                free_tx_return: resources.free_tx.sender(),
                tx_metadata: &resources.tx_metadata,
                ready_tx: &resources.ready_tx,
                #[cfg(feature = "tx-staging-copy-probe")]
                free_staged: resources.free_staged.receiver(),
                #[cfg(feature = "tx-staging-copy-probe")]
                free_staged_return: resources.free_staged.sender(),
                #[cfg(feature = "tx-staging-copy-probe")]
                ready_staged: &resources.ready_staged,
                #[cfg(feature = "tx-staging-copy-probe")]
                staged_pool: &resources.staged_pool,
                #[cfg(feature = "tx-staging-copy-probe")]
                staged_metadata: &resources.staged_metadata,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_requests: &resources.materialization_requests,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_completions: &resources.materialization_completions,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_in_flight: &resources.materialization_in_flight,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_cancel: &resources.materialization_cancel,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_wakers: &resources.materialization_wakers,
                tx_published: &resources.tx_published,
                tx_credit_wakers: &resources.tx_credit_wakers,
                tx_credit_waiters: &resources.tx_credit_waiters,
                control_tx_index: &resources.control_tx_index,
                control_tx_available: &resources.control_tx_available,
                control_tx_waiters: &resources.control_tx_waiters,
                #[cfg(feature = "tx-egress-scheduling")]
                control_tx_next_interface: &resources.control_tx_next_interface,
                #[cfg(feature = "tx-staging-copy-probe")]
                tx_staged_interfaces: &resources.tx_staged_interfaces,
                tx_active: &resources.tx_active,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_ingress_reserved: &resources.tx_ingress_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_application_reserved: &resources.tx_application_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_control_reserved: &resources.tx_control_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_tokens_in_flight: &resources.tx_tokens_in_flight,
                tx_pool: pool,
            },
            PinnedTxConsumer {
                free_tx: resources.free_tx.sender(),
                tx_metadata: &resources.tx_metadata,
                #[cfg(feature = "tx-staging-copy-probe")]
                free_tx_claim: resources.free_tx.receiver(),
                ready_tx: &resources.ready_tx,
                #[cfg(feature = "tx-staging-copy-probe")]
                free_staged: resources.free_staged.sender(),
                #[cfg(feature = "tx-staging-copy-probe")]
                ready_staged: &resources.ready_staged,
                #[cfg(feature = "tx-staging-copy-probe")]
                staged_pool: &resources.staged_pool,
                #[cfg(feature = "tx-staging-copy-probe")]
                staged_metadata: &resources.staged_metadata,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_requests: &resources.materialization_requests,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_completions: &resources.materialization_completions,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_in_flight: &resources.materialization_in_flight,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_cancel: &resources.materialization_cancel,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_wakers: &resources.materialization_wakers,
                next_interface: &resources.next_interface,
                tx_published: &resources.tx_published,
                tx_credit_wakers: &resources.tx_credit_wakers,
                tx_credit_waiters: &resources.tx_credit_waiters,
                control_tx_index: &resources.control_tx_index,
                control_tx_available: &resources.control_tx_available,
                control_tx_waiters: &resources.control_tx_waiters,
                #[cfg(feature = "tx-staging-copy-probe")]
                tx_staged_interfaces: &resources.tx_staged_interfaces,
                tx_active: &resources.tx_active,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_ingress_reserved: &resources.tx_ingress_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_application_reserved: &resources.tx_application_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_control_reserved: &resources.tx_control_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_tokens_in_flight: &resources.tx_tokens_in_flight,
                tx_pool: pool,
            },
        )
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
> Default for PinnedTxResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize>
    PinnedEndpointResources<M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            free_rx: Channel::new(),
            ready_rx: Channel::new(),
            rx_pool: RxHandoffPool::new(),
            link: SharedLinkState::new(),
            split: AtomicBool::new(false),
        }
    }

    pub fn split<
        'resources,
        const HEADROOM: usize,
        const TRAILER: usize,
        const TX_QUEUE_DEPTH: usize,
    >(
        &'resources mut self,
        tx: PinnedTxProvider<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        endpoint: NetworkEndpointConfig<'resources>,
    ) -> (
        SplitPinnedDevice<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    ) {
        assert!(RX_QUEUE_DEPTH > 0, "pinned RX pool must not be empty");
        assert!(
            RX_QUEUE_DEPTH <= usize::from(u8::MAX) + 1,
            "pinned RX pool index must fit in u8"
        );
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "pinned endpoint resources may only be split once"
        );
        let interface = endpoint.interface();
        PinnedTxCreditWakers::<M>::validate(interface);
        for index in 0..RX_QUEUE_DEPTH {
            self.free_rx
                .try_send(index as u8)
                .expect("an empty free RX queue accepts every pool index");
        }
        let resources: &Self = self;
        (
            SplitPinnedDevice {
                ready_rx: resources.ready_rx.receiver(),
                free_rx: resources.free_rx.sender(),
                rx_pool: &resources.rx_pool,
                free_tx: tx.free_tx,
                free_tx_return: tx.free_tx_return,
                tx_metadata: tx.tx_metadata,
                ready_tx: tx.ready_tx,
                #[cfg(feature = "tx-staging-copy-probe")]
                free_staged: tx.free_staged,
                #[cfg(feature = "tx-staging-copy-probe")]
                free_staged_return: tx.free_staged_return,
                #[cfg(feature = "tx-staging-copy-probe")]
                ready_staged: tx.ready_staged,
                #[cfg(feature = "tx-staging-copy-probe")]
                staged_pool: tx.staged_pool,
                #[cfg(feature = "tx-staging-copy-probe")]
                staged_metadata: tx.staged_metadata,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_requests: tx.materialization_requests,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_completions: tx.materialization_completions,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_in_flight: tx.materialization_in_flight,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_cancel: tx.materialization_cancel,
                #[cfg(feature = "tx-core1-materializer-probe")]
                materialization_wakers: tx.materialization_wakers,
                interface,
                tx_published: tx.tx_published,
                tx_credit_wakers: tx.tx_credit_wakers,
                tx_credit_waiters: tx.tx_credit_waiters,
                control_tx_index: tx.control_tx_index,
                control_tx_available: tx.control_tx_available,
                control_tx_waiters: tx.control_tx_waiters,
                #[cfg(feature = "tx-egress-scheduling")]
                control_tx_next_interface: tx.control_tx_next_interface,
                #[cfg(feature = "tx-staging-copy-probe")]
                tx_staged_interfaces: tx.tx_staged_interfaces,
                tx_active: tx.tx_active,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_ingress_reserved: tx.tx_ingress_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_application_reserved: tx.tx_application_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_control_reserved: tx.tx_control_reserved,
                #[cfg(feature = "tx-phase-telemetry")]
                tx_tokens_in_flight: tx.tx_tokens_in_flight,
                tx_pool: tx.tx_pool,
                link: &resources.link,
                endpoint,
                ingress_tx: None,
                application_tx: None,
                control_tx: None,
                reserve_ingress_tx: false,
                waiting_for_tx_credit: false,
                waiting_for_control_tx: false,
                #[cfg(feature = "tx-staging-copy-probe")]
                staged_tx_selected: false,
                #[cfg(feature = "tx-egress-scheduling")]
                keyed_egress: None,
                #[cfg(feature = "tx-egress-scheduling")]
                keyed_run_length: 0,
                #[cfg(feature = "tx-egress-scheduling")]
                observed_link_epoch: 0,
                #[cfg(feature = "tx-egress-scheduling")]
                observed_peer_revision: 0,
                #[cfg(feature = "tx-egress-scheduling")]
                scheduling_epoch: 0,
                #[cfg(feature = "tx-egress-scheduling")]
                egress_control: None,
                #[cfg(feature = "tx-egress-scheduling")]
                egress_demand_active: false,
                #[cfg(feature = "tx-egress-scheduling")]
                egress_demand_flush_pending: false,
                #[cfg(feature = "tx-egress-scheduling")]
                egress_grant: None,
                #[cfg(feature = "tx-egress-scheduling")]
                egress_standby_grant: None,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_serial: 0,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_key: None,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_remaining: 0,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_checks: 0,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_matches: 0,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_no_window: 0,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_key_mismatch: 0,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_credit_exhausted: 0,
                #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
                shadow_grant_unclassified: 0,
                checksum: ChecksumCapabilities::default(),
                tx_reservation: (),
            },
            SplitPinnedRxRunner {
                free_rx: resources.free_rx.receiver(),
                free_rx_return: resources.free_rx.sender(),
                ready_rx: resources.ready_rx.sender(),
                ordered_rx: None,
                rx_pool: &resources.rx_pool,
                link: &resources.link,
                tx_active: tx.tx_active,
                tx_interface: interface,
                tx_credit_wakers: tx.tx_credit_wakers,
            },
        )
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize> Default
    for PinnedEndpointResources<M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Copyable authority for one logical endpoint to claim unique credits from
/// the shared physical TX pool and publish tagged ready entries.
pub struct PinnedTxProvider<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free_tx: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    free_tx_return: Sender<'resources, M, u8, QUEUE_DEPTH>,
    tx_metadata: &'resources [PinnedTxMetadataSlot; QUEUE_DEPTH],
    ready_tx: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-staging-copy-probe")]
    free_staged: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    free_staged_return: Sender<'resources, M, u8, QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    ready_staged: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_pool: &'resources RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_metadata: &'resources [PinnedTxMetadataSlot; QUEUE_DEPTH],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_requests:
        &'resources [Channel<M, TxMaterializationRequest, 1>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_completions:
        &'resources [Channel<M, TxMaterializationCompletion, 1>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_in_flight: &'resources AtomicU32,
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_cancel: &'resources AtomicU32,
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_published: &'resources Signal<M, ()>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    control_tx_index: &'resources AtomicUsize,
    control_tx_available: &'resources AtomicBool,
    control_tx_waiters: &'resources AtomicU32,
    #[cfg(feature = "tx-egress-scheduling")]
    control_tx_next_interface: &'resources AtomicU32,
    #[cfg(feature = "tx-staging-copy-probe")]
    tx_staged_interfaces: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_ingress_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_application_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_control_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_tokens_in_flight: &'resources AtomicU32,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for PinnedTxProvider<'_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for PinnedTxProvider<'_, M, F, H, T, Q>
{
}

pub struct SplitPinnedDevice<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    ready_rx: Receiver<'resources, M, u8, RX_QUEUE_DEPTH>,
    free_rx: Sender<'resources, M, u8, RX_QUEUE_DEPTH>,
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    free_tx: Receiver<'resources, M, u8, TX_QUEUE_DEPTH>,
    free_tx_return: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    tx_metadata: &'resources [PinnedTxMetadataSlot; TX_QUEUE_DEPTH],
    ready_tx: &'resources [Channel<M, u8, TX_QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-staging-copy-probe")]
    free_staged: Receiver<'resources, M, u8, TX_QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    free_staged_return: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    ready_staged: &'resources [Channel<M, u8, TX_QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_pool: &'resources RxHandoffPool<FRAME_CAPACITY, TX_QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_metadata: &'resources [PinnedTxMetadataSlot; TX_QUEUE_DEPTH],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_requests:
        &'resources [Channel<M, TxMaterializationRequest, 1>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_completions:
        &'resources [Channel<M, TxMaterializationCompletion, 1>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_in_flight: &'resources AtomicU32,
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_cancel: &'resources AtomicU32,
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_wakers: &'resources PinnedTxCreditWakers<M>,
    interface: NetworkInterfaceId,
    tx_published: &'resources Signal<M, ()>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    control_tx_index: &'resources AtomicUsize,
    control_tx_available: &'resources AtomicBool,
    control_tx_waiters: &'resources AtomicU32,
    #[cfg(feature = "tx-egress-scheduling")]
    control_tx_next_interface: &'resources AtomicU32,
    #[cfg(feature = "tx-staging-copy-probe")]
    tx_staged_interfaces: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_ingress_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_application_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_control_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_tokens_in_flight: &'resources AtomicU32,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
    endpoint: NetworkEndpointConfig<'resources>,
    /// One credit unavailable to ordinary egress and therefore available to
    /// satisfy the `Driver::receive` RX+TX-token contract under saturated TX.
    ingress_tx: Option<u8>,
    application_tx: Option<u8>,
    /// The sole global control credit is claimed only while an uncatalogued
    /// provider synchronously constructs its final frame.
    control_tx: Option<u8>,
    reserve_ingress_tx: bool,
    waiting_for_tx_credit: bool,
    waiting_for_control_tx: bool,
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_tx_selected: bool,
    #[cfg(feature = "tx-egress-scheduling")]
    keyed_egress: Option<EgressKey>,
    #[cfg(feature = "tx-egress-scheduling")]
    keyed_run_length: u8,
    #[cfg(feature = "tx-egress-scheduling")]
    observed_link_epoch: u32,
    #[cfg(feature = "tx-egress-scheduling")]
    observed_peer_revision: u32,
    #[cfg(feature = "tx-egress-scheduling")]
    scheduling_epoch: u32,
    #[cfg(feature = "tx-egress-scheduling")]
    egress_control: Option<&'resources mut DefaultEgressNetworkScheduler<'resources, M>>,
    #[cfg(feature = "tx-egress-scheduling")]
    egress_demand_active: bool,
    #[cfg(feature = "tx-egress-scheduling")]
    egress_demand_flush_pending: bool,
    #[cfg(feature = "tx-egress-scheduling")]
    egress_grant: Option<PinnedEgressGrantState>,
    #[cfg(feature = "tx-egress-scheduling")]
    egress_standby_grant: Option<PinnedEgressGrantState>,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_serial: u32,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_key: Option<EgressGrantKey>,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_remaining: u8,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_checks: u32,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_matches: u32,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_no_window: u32,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_key_mismatch: u32,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_credit_exhausted: u32,
    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    shadow_grant_unclassified: u32,
    checksum: ChecksumCapabilities,
    tx_reservation: (),
}

/// Core1-local owner of one radio-issued authoritative quantum.
///
/// The state remains next to the permanent device rather than in an async
/// stack frame. Xarxa owns the exact spent-credit count and publishes one
/// terminal completion. Airtime was already reserved by Core0 before the
/// grant crossed cores, so no materialization-start record is required.
#[cfg(feature = "tx-egress-scheduling")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PinnedEgressGrantState {
    grant: EgressBurstGrant,
    used_frames: u8,
    completion: Option<EgressGrantCompletion>,
}

#[cfg(feature = "tx-egress-scheduling")]
impl PinnedEgressGrantState {
    const fn new(grant: EgressBurstGrant) -> Self {
        Self {
            grant,
            used_frames: 0,
            completion: None,
        }
    }

    fn authorizes(&self, key: EgressKey) -> bool {
        self.completion.is_none()
            && self.grant.demand().key() == key
            && self.used_frames < self.grant.frame_credits().get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PinnedTxAdmissionClass {
    Ordinary,
    #[cfg(feature = "tx-egress-scheduling")]
    Control,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    SplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    /// Attach the sole stack-side endpoint of the radio egress control plane.
    ///
    /// The port is non-`Copy`; moving it into the permanent network device
    /// proves that only this Core1 driver instance can publish the endpoint's
    /// lifecycle demand.
    #[cfg(feature = "tx-egress-scheduling")]
    pub fn with_egress_control(
        mut self,
        control: &'resources mut DefaultEgressNetworkScheduler<'resources, M>,
    ) -> Self {
        assert!(
            self.egress_control.is_none(),
            "network egress control may only be attached once"
        );
        self.initialize_control_tx_reserve();
        // The diagnostic selector is immutable after the device owner graph is
        // built. Snapshotting it here avoids an Acquire load and RISC-V fence
        // in every packet admission while preserving same-ELF enabled/disabled
        // comparison.
        self.egress_demand_active = egress_control_enabled();
        self.egress_demand_flush_pending =
            self.egress_demand_active && control.egress_demand_flush_pending();
        self.egress_control = Some(control);
        self
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn initialize_control_tx_reserve(&mut self) {
        match self.control_tx_index.compare_exchange(
            UNINITIALIZED_CONTROL_TX_INDEX,
            INITIALIZING_CONTROL_TX_INDEX,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                let index = self
                    .free_tx
                    .try_receive()
                    .expect("egress control requires one global physical TX reserve");
                self.control_tx_index
                    .store(usize::from(index), Ordering::Release);
                assert!(
                    !self.control_tx_available.swap(true, Ordering::AcqRel),
                    "new control TX reserve must start unavailable"
                );
            }
            Err(existing) => assert_ne!(
                existing, INITIALIZING_CONTROL_TX_INDEX,
                "control TX reserve owner graph cannot be initialized concurrently"
            ),
        }
    }

    /// Retry a retained terminal grant record. A full transport leaves the
    /// exact record in the permanent Core1 owner and the radio capacity edge
    /// wakes this device again.
    #[cfg(feature = "tx-egress-scheduling")]
    fn flush_egress_grant_progress(&mut self) {
        let Some(control) = self.egress_control.as_deref_mut() else {
            return;
        };

        for _ in 0..PINNED_EGRESS_GRANT_PIPELINE_DEPTH {
            let Some(completion) = self
                .egress_grant
                .as_ref()
                .and_then(|state| state.completion)
            else {
                break;
            };
            let progress = EgressGrantProgress::Finished {
                serial: completion.serial(),
                used_frames: completion.used_frames(),
                remaining: completion.remaining(),
            };
            if control.try_publish_grant_progress(progress).is_ok() {
                self.egress_grant = self.egress_standby_grant.take();
            } else {
                break;
            }
        }
    }

    /// Keep one TX credit unavailable to ordinary application egress so an
    /// incoming frame can always receive the paired `TxToken` required by the
    /// embassy-net driver contract. Resource profiles enabling this must add
    /// one credit per permanent endpoint beyond their advertised application
    /// capacity.
    pub fn with_ingress_tx_reserve(mut self) -> Self {
        assert!(
            TX_QUEUE_DEPTH > 1,
            "ingress TX reserve needs an application credit"
        );
        self.reserve_ingress_tx = true;
        let index = self
            .try_take_free_tx()
            .expect("an ingress-enabled endpoint needs one dedicated TX credit");
        self.ingress_tx = Some(index);
        #[cfg(feature = "tx-phase-telemetry")]
        self.tx_ingress_reserved.fetch_add(1, Ordering::Relaxed);
        self
    }

    /// Override the checksum work advertised to the network stack.
    ///
    /// Selecting a mode which skips RX validation is sound only when a lower
    /// layer has already validated the corresponding packet checksum.
    pub fn with_checksum_capabilities(mut self, checksum: ChecksumCapabilities) -> Self {
        self.checksum = checksum;
        self
    }

    /// Select the CPU packet tier for this logical endpoint from the startup
    /// diagnostic policy. Selection is immutable once the device starts.
    #[cfg(feature = "tx-staging-copy-probe")]
    pub fn with_tx_staging_copy_selected(mut self, selected: bool) -> Self {
        self.staged_tx_selected = selected;
        if self.staged_tx_selected {
            self.tx_staged_interfaces
                .fetch_or(1_u32 << self.interface.value(), Ordering::Release);
        } else {
            self.tx_staged_interfaces
                .fetch_and(!(1_u32 << self.interface.value()), Ordering::Release);
        }
        self
    }

    /// Select the CPU packet tier for this logical endpoint from the startup
    /// diagnostic policy. Selection is immutable once the device starts.
    #[cfg(feature = "tx-staging-copy-probe")]
    pub fn with_tx_staging_copy_probe_selection(self) -> Self {
        self.with_tx_staging_copy_selected(tx_staging_copy_enabled())
    }

    fn try_take_free_tx(&mut self) -> Option<u8> {
        #[cfg(feature = "tx-staging-copy-probe")]
        let index = if self.staged_tx_selected {
            self.free_staged.try_receive().ok()?
        } else {
            self.free_tx.try_receive().ok()?
        };
        #[cfg(not(feature = "tx-staging-copy-probe"))]
        let index = self.free_tx.try_receive().ok()?;
        if self.waiting_for_tx_credit {
            self.tx_credit_waiters
                .fetch_and(!(1_u32 << self.interface.value()), Ordering::AcqRel);
            self.waiting_for_tx_credit = false;
        }
        Some(index)
    }

    fn poll_free_tx(&mut self, cx: &mut Context<'_>) -> Poll<u8> {
        if let Some(index) = self.try_take_free_tx() {
            return Poll::Ready(index);
        }
        // Register outside Channel's single receiver-waker slot, then repeat
        // the ownership probe so a credit returned across the registration
        // edge cannot be lost.
        self.tx_credit_wakers.register(self.interface, cx);
        if !self.waiting_for_tx_credit {
            self.tx_credit_waiters
                .fetch_or(1_u32 << self.interface.value(), Ordering::AcqRel);
            self.waiting_for_tx_credit = true;
        }
        match self.try_take_free_tx() {
            Some(index) => Poll::Ready(index),
            None => Poll::Pending,
        }
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn control_tx_index(&self) -> Option<u8> {
        let index = self.control_tx_index.load(Ordering::Acquire);
        (index < INITIALIZING_CONTROL_TX_INDEX)
            .then(|| u8::try_from(index).expect("pinned TX indices fit in u8"))
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn try_take_control_tx(&mut self) -> Option<u8> {
        let index = self.control_tx_index()?;
        let interface = usize::from(self.interface.value());
        let own_bit = 1_u32 << interface;
        let waiting = self.control_tx_waiters.load(Ordering::Acquire)
            & self.tx_active.load(Ordering::Acquire);
        let start = self.control_tx_next_interface.load(Ordering::Relaxed) as usize
            % PINNED_TX_CREDIT_WAKER_SLOTS;
        let selected = (0..PINNED_TX_CREDIT_WAKER_SLOTS)
            .map(|offset| (start + offset) % PINNED_TX_CREDIT_WAKER_SLOTS)
            .find(|candidate| waiting & (1_u32 << candidate) != 0)?;
        if selected != interface
            || self
                .control_tx_available
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return None;
        }
        self.control_tx_waiters
            .fetch_and(!own_bit, Ordering::AcqRel);
        self.control_tx_next_interface.store(
            ((interface + 1) % PINNED_TX_CREDIT_WAKER_SLOTS) as u32,
            Ordering::Relaxed,
        );
        self.waiting_for_control_tx = false;
        Some(index)
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn poll_reserve_control_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.control_tx.is_some() {
            return true;
        }
        self.tx_credit_wakers.register(self.interface, cx);
        if !self.waiting_for_control_tx {
            self.control_tx_waiters
                .fetch_or(1_u32 << self.interface.value(), Ordering::AcqRel);
            self.waiting_for_control_tx = true;
        }
        if let Some(index) = self.try_take_control_tx() {
            self.control_tx = Some(index);
            #[cfg(feature = "tx-phase-telemetry")]
            self.tx_control_reserved.fetch_add(1, Ordering::Relaxed);
        }
        self.control_tx.is_some()
    }

    fn poll_reserve_ingress_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.ingress_tx.is_none()
            && let Poll::Ready(index) = self.poll_free_tx(cx)
        {
            self.ingress_tx = Some(index);
            #[cfg(feature = "tx-phase-telemetry")]
            self.tx_ingress_reserved.fetch_add(1, Ordering::Relaxed);
        }
        self.ingress_tx.is_some()
    }

    fn poll_reserve_application_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.application_tx.is_some() {
            return true;
        }
        // Re-establish this endpoint's ingress credit before admitting more
        // application egress. Multiple permanent endpoints share the physical
        // pool, so protecting only the final global credit can starve one of
        // them indefinitely.
        if self.reserve_ingress_tx && !self.poll_reserve_ingress_tx(cx) {
            return false;
        }
        if let Poll::Ready(index) = self.poll_free_tx(cx) {
            self.application_tx = Some(index);
            #[cfg(feature = "tx-phase-telemetry")]
            self.tx_application_reserved.fetch_add(1, Ordering::Relaxed);
        }
        self.application_tx.is_some()
    }

    fn take_tx_token<'device>(
        &'device mut self,
        index: u8,
        metadata: PinnedTxMetadata,
        class: PinnedTxAdmissionClass,
    ) -> PinnedTransmitToken<
        'device,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    > {
        #[cfg(feature = "tx-staging-copy-probe")]
        let lease = if self.staged_tx_selected && class == PinnedTxAdmissionClass::Ordinary {
            PinnedTransmitLease::Staged(self.staged_pool.claim_radio(index))
        } else {
            PinnedTransmitLease::Direct(self.tx_pool.claim_network(index))
        };
        #[cfg(not(feature = "tx-staging-copy-probe"))]
        let _ = class;
        #[cfg(not(feature = "tx-staging-copy-probe"))]
        let lease = PinnedTransmitLease::Direct(self.tx_pool.claim_network(index));
        #[cfg(feature = "tx-phase-telemetry")]
        self.tx_tokens_in_flight.fetch_add(1, Ordering::Relaxed);
        PinnedTransmitToken {
            tx_return: PinnedTxReturn {
                free_tx: self.free_tx_return,
                interface: self.interface,
                tx_credit_wakers: self.tx_credit_wakers,
                tx_credit_waiters: self.tx_credit_waiters,
                control_tx_index: self.control_tx_index,
                control_tx_available: self.control_tx_available,
                control_tx_waiters: self.control_tx_waiters,
                tx_active: self.tx_active,
            },
            tx_metadata: self.tx_metadata,
            ready_tx: self.ready_tx,
            #[cfg(feature = "tx-staging-copy-probe")]
            free_staged: self.free_staged_return,
            #[cfg(feature = "tx-staging-copy-probe")]
            ready_staged: self.ready_staged,
            #[cfg(feature = "tx-staging-copy-probe")]
            staged_pool: self.staged_pool,
            #[cfg(feature = "tx-staging-copy-probe")]
            staged_metadata: self.staged_metadata,
            metadata,
            tx_published: self.tx_published,
            tx_credit_wakers: self.tx_credit_wakers,
            tx_credit_waiters: self.tx_credit_waiters,
            tx_active: self.tx_active,
            #[cfg(feature = "tx-phase-telemetry")]
            tx_tokens_in_flight: self.tx_tokens_in_flight,
            tx_pool: self.tx_pool,
            lease: Some(lease),
            _reservation: &mut self.tx_reservation,
        }
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn refresh_scheduling_epoch(&mut self) -> u32 {
        let link_epoch = self.link.egress_epoch();
        let peer_revision = self.endpoint.peer_revision();
        if link_epoch != self.observed_link_epoch || peer_revision != self.observed_peer_revision {
            self.observed_link_epoch = link_epoch;
            self.observed_peer_revision = peer_revision;
            self.scheduling_epoch = self
                .scheduling_epoch
                .checked_add(1)
                .expect("network scheduling epoch is not reusable");
            self.keyed_egress = None;
            self.keyed_run_length = 0;
        }
        self.scheduling_epoch
    }

    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    fn record_shadow_grant(&mut self, observation: TxShadowGrantObservation) {
        self.shadow_grant_checks = self.shadow_grant_checks.wrapping_add(1);
        let category = match observation {
            TxShadowGrantObservation::Matched => &mut self.shadow_grant_matches,
            TxShadowGrantObservation::NoWindow => &mut self.shadow_grant_no_window,
            TxShadowGrantObservation::KeyMismatch => &mut self.shadow_grant_key_mismatch,
            TxShadowGrantObservation::CreditExhausted => &mut self.shadow_grant_credit_exhausted,
            TxShadowGrantObservation::Unclassified => &mut self.shadow_grant_unclassified,
        };
        *category = category.wrapping_add(1);
        TX_PERFORMANCE.publish_shadow_grant(observation, self.shadow_grant_checks, *category);
    }

    #[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
    fn observe_successful_shadow_grant(&mut self, egress: EgressKey) {
        let Some(grant) = self.endpoint.shadow_grant else {
            return;
        };
        let Some(requested) = self.endpoint.grant_key(egress) else {
            self.record_shadow_grant(TxShadowGrantObservation::Unclassified);
            return;
        };
        let Some(start_serial) = grant.serial() else {
            self.record_shadow_grant(TxShadowGrantObservation::NoWindow);
            return;
        };
        if start_serial != self.shadow_grant_serial {
            match grant.snapshot() {
                Some(snapshot) => {
                    self.shadow_grant_serial = snapshot.serial();
                    self.shadow_grant_key = Some(snapshot.key());
                    self.shadow_grant_remaining = snapshot.frame_credits().get();
                }
                None => {
                    self.shadow_grant_serial = start_serial;
                    self.shadow_grant_key = None;
                    self.shadow_grant_remaining = 0;
                }
            }
        }
        if grant.serial() != Some(start_serial) || self.shadow_grant_serial != start_serial {
            self.record_shadow_grant(TxShadowGrantObservation::NoWindow);
            return;
        }
        if self.shadow_grant_key.is_none() {
            self.record_shadow_grant(TxShadowGrantObservation::NoWindow);
        } else if self.shadow_grant_key != Some(requested) {
            self.record_shadow_grant(TxShadowGrantObservation::KeyMismatch);
        } else if self.shadow_grant_remaining == 0 {
            self.record_shadow_grant(TxShadowGrantObservation::CreditExhausted);
        } else {
            self.shadow_grant_remaining -= 1;
            self.record_shadow_grant(TxShadowGrantObservation::Matched);
        }
    }

    /// Execute at most one scheduler-selected copy batch in the network
    /// task's driver poll. The radio submitted only value-level indices after
    /// transferring each staged source to READY ownership, so this method is
    /// the sole Core1 claimant of every source and reserved destination.
    #[cfg(feature = "tx-core1-materializer-probe")]
    fn service_core1_materialization(&mut self, cx: &mut Context<'_>) {
        if !self.staged_tx_selected || !tx_core1_materializer_enabled() {
            return;
        }
        let interface_index = usize::from(self.interface.value());
        let request_channel = &self.materialization_requests[interface_index];
        let request = match request_channel.try_receive() {
            Ok(request) => request,
            Err(TryReceiveError::Empty) => {
                self.materialization_wakers.register(self.interface, cx);
                let Ok(request) = request_channel.try_receive() else {
                    return;
                };
                request
            }
        };
        assert_eq!(
            request.interface, self.interface,
            "materialization request crossed its logical interface"
        );
        let count = usize::from(request.count);
        assert!(
            count != 0 && count <= PINNED_TX_MATERIALIZATION_BATCH_CAPACITY,
            "materialization request must contain one bounded burst"
        );
        let interface_bit = 1_u32 << self.interface.value();
        let cancelled_before_copy =
            self.materialization_cancel.load(Ordering::Acquire) & interface_bit != 0;
        let mut completion = TxMaterializationCompletion::empty(self.interface);
        completion.count = request.count;

        for (completed, pair) in request.pairs[..count].iter().enumerate() {
            let source = self.staged_pool.claim_network(pair.source);
            if cancelled_before_copy {
                let source_index = source.release();
                if let Err(TrySendError::Full(_)) = self.free_staged_return.try_send(source_index) {
                    unreachable!("cancelled materialization returns its unique staged credit");
                }
                let destination = self.tx_pool.claim_network(pair.destination);
                let destination_index = destination.release();
                if let Err(TrySendError::Full(_)) = self.free_tx_return.try_send(destination_index)
                {
                    unreachable!("cancelled materialization returns its unique DMA credit");
                }
                continue;
            }

            let length = source.frame().len();
            let metadata = self.staged_metadata[usize::from(pair.source)].read(self.interface);
            let destination = self.tx_pool.claim_network(pair.destination);
            let (destination_index, ()) = destination.publish(length, |dma| {
                dma.copy_from_slice(source.frame());
            });
            self.tx_metadata[usize::from(destination_index)].publish(metadata);
            completion.destinations[completed] = destination_index;
            let source_index = source.release();
            if let Err(TrySendError::Full(_)) = self.free_staged_return.try_send(source_index) {
                unreachable!("materialization returns its unique staged credit");
            }
            if self.free_staged_return.len() == 1 {
                self.tx_credit_wakers.wake_waiter_after(
                    self.interface,
                    self.tx_active,
                    self.tx_credit_waiters,
                );
            }
        }

        let cancelled = cancelled_before_copy
            || self.materialization_cancel.load(Ordering::Acquire) & interface_bit != 0;
        if cancelled && !cancelled_before_copy {
            for destination in completion.destinations[..count].iter().copied() {
                let destination = self.tx_pool.claim_radio(destination);
                let index = destination.release();
                if let Err(TrySendError::Full(_)) = self.free_tx_return.try_send(index) {
                    unreachable!("cancelled ready materialization returns its unique DMA credit");
                }
            }
        }
        completion.cancelled = cancelled;
        if let Err(TrySendError::Full(_)) =
            self.materialization_completions[interface_index].try_send(completion)
        {
            unreachable!("one completion slot exists for the sole in-flight materialization");
        }
        // Cancellation can race the narrow interval between the final cancel
        // observation above and completion publication.  Re-check after the
        // publication: either Core0 has already claimed and reclaimed the
        // completion, or this worker still owns the sole receiver slot and
        // must do so here.  This makes terminal cancellation independent of
        // a later role/network poll.
        let cancelled_after_publication =
            !cancelled && self.materialization_cancel.load(Ordering::Acquire) & interface_bit != 0;
        if cancelled_after_publication {
            if let Ok(completion) = self.materialization_completions[interface_index].try_receive()
            {
                for index in completion.destinations[..count].iter().copied() {
                    let destination = self.tx_pool.claim_radio(index);
                    let index = destination.release();
                    if let Err(TrySendError::Full(_)) = self.free_tx_return.try_send(index) {
                        unreachable!("post-publication cancellation returns its unique DMA credit");
                    }
                }
                TX_CORE1_MATERIALIZER_COUNTERS
                    .cancelled_batches
                    .fetch_add(1, Ordering::Relaxed);
            }
            self.materialization_cancel
                .fetch_and(!interface_bit, Ordering::AcqRel);
            self.materialization_in_flight
                .fetch_and(!interface_bit, Ordering::AcqRel);
        }
        if cancelled {
            self.materialization_cancel
                .fetch_and(!interface_bit, Ordering::AcqRel);
            self.materialization_in_flight
                .fetch_and(!interface_bit, Ordering::AcqRel);
            TX_CORE1_MATERIALIZER_COUNTERS
                .cancelled_batches
                .fetch_add(1, Ordering::Relaxed);
        } else if !cancelled_after_publication {
            TX_CORE1_MATERIALIZER_COUNTERS
                .materialized_frames
                .fetch_add(u32::from(request.count), Ordering::Relaxed);
        }
        TX_CORE1_MATERIALIZER_COUNTERS
            .completed_batches
            .fetch_add(1, Ordering::Relaxed);
        self.tx_published.signal(());

        // Servicing the request consumes the wake that brought this driver
        // poll to Core1.  Re-arm the per-interface edge before returning: the
        // newly released staged credits can let this same network poll
        // publish the next packet, after which Core0 may submit a successor
        // request without any unrelated network timer/IRQ to poll us again.
        self.materialization_wakers.register(self.interface, cx);
        if !request_channel.is_empty() {
            // Close submit-between-completion-and-registration. There is only
            // one affine request slot, so one self-wake is sufficient.
            cx.waker().wake_by_ref();
        }
    }

    /// Add a second RX source whose storage remains owned by a lower staging
    /// pool. Ordinary frames can then cross into `embassy-net` by index while
    /// this device's original RX pool remains available for copying slow
    /// paths such as A-MSDU expansion.
    pub fn with_shared_rx<const SHARED_CAPACITY: usize, const SHARED_SLOTS: usize>(
        self,
        shared: SharedPinnedRxConsumer<'resources, M, SHARED_CAPACITY, SHARED_SLOTS>,
    ) -> SharedRxSplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
        SHARED_CAPACITY,
        SHARED_SLOTS,
    > {
        SharedRxSplitPinnedDevice {
            inner: self,
            shared,
        }
    }
}

/// Unique `embassy-net` lease for one permanently located received frame.
///
/// Consuming or dropping the token returns the slot to the radio publisher.
/// The frame bytes therefore stay at one stable address across the
/// radio-to-network ownership handoff.
pub struct PinnedReceiveToken<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    free_rx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    lease: Option<RxNetworkLease<'resources, FRAME_CAPACITY>>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> embassy_net_driver::RxToken
    for PinnedReceiveToken<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.lease
            .as_mut()
            .expect("live pinned RX token")
            .with_frame(f)
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for PinnedReceiveToken<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let index = lease.release();
            if let Err(TrySendError::Full(_)) = self.free_rx.try_send(index) {
                unreachable!("network RX token returns its unique pinned index");
            }
        }
    }
}

/// Network token backed by a lower staging pool rather than the adapter's
/// copying slow-path pool.
pub struct SharedPoolReceiveToken<'resources, const FRAME_CAPACITY: usize> {
    lease: Option<SharedPoolNetworkLease<'resources, FRAME_CAPACITY>>,
    on_release: fn(),
}

enum SharedPoolNetworkLease<'resources, const FRAME_CAPACITY: usize> {
    Copied(RxNetworkLease<'resources, FRAME_CAPACITY>),
    External(ExternalRxNetworkLease<'resources, FRAME_CAPACITY>),
}

impl<const FRAME_CAPACITY: usize> SharedPoolNetworkLease<'_, FRAME_CAPACITY> {
    fn with_frame<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        match self {
            Self::Copied(lease) => lease.with_frame(f),
            Self::External(lease) => lease.with_frame(f),
        }
    }
}

impl<const FRAME_CAPACITY: usize> embassy_net_driver::RxToken
    for SharedPoolReceiveToken<'_, FRAME_CAPACITY>
{
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.lease
            .as_mut()
            .expect("live shared RX token")
            .with_frame(f)
    }
}

impl<const FRAME_CAPACITY: usize> Drop for SharedPoolReceiveToken<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            drop(lease);
            (self.on_release)();
        }
    }
}

/// RX token for a device that accepts both copied slow-path frames and
/// in-place frames retained in the lower staging pool.
pub enum SharedPinnedReceiveToken<
    'resources,
    M: RawMutex,
    const OWNED_CAPACITY: usize,
    const OWNED_DEPTH: usize,
    const SHARED_CAPACITY: usize,
> {
    Owned(PinnedReceiveToken<'resources, M, OWNED_CAPACITY, OWNED_DEPTH>),
    Shared(SharedPoolReceiveToken<'resources, SHARED_CAPACITY>),
}

impl<
    M: RawMutex,
    const OWNED_CAPACITY: usize,
    const OWNED_DEPTH: usize,
    const SHARED_CAPACITY: usize,
> embassy_net_driver::RxToken
    for SharedPinnedReceiveToken<'_, M, OWNED_CAPACITY, OWNED_DEPTH, SHARED_CAPACITY>
{
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        match self {
            Self::Owned(token) => embassy_net_driver::RxToken::consume(token, f),
            Self::Shared(token) => embassy_net_driver::RxToken::consume(token, f),
        }
    }
}

/// `embassy-net` device that multiplexes an in-place staging pool and the
/// adapter-owned copying pool while retaining one common TX/link owner.
pub struct SharedRxSplitPinnedDevice<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    const SHARED_CAPACITY: usize,
    const SHARED_SLOTS: usize,
> {
    inner: SplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >,
    shared: SharedPinnedRxConsumer<'resources, M, SHARED_CAPACITY, SHARED_SLOTS>,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    const SHARED_CAPACITY: usize,
    const SHARED_SLOTS: usize,
>
    SharedRxSplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
        SHARED_CAPACITY,
        SHARED_SLOTS,
    >
{
    /// Override checksum capabilities before constructing the IP stack.
    pub fn with_checksum_capabilities(mut self, checksum: ChecksumCapabilities) -> Self {
        self.inner = self.inner.with_checksum_capabilities(checksum);
        self
    }

    /// Select software IPv4/UDP validation for received packets.
    ///
    /// Disabling it is intended for a controlled diagnostic or for a future
    /// lower layer that can prove both checksums were already validated. TX
    /// checksum generation and all other protocol policies remain unchanged.
    pub fn with_software_ipv4_udp_rx_checksum_validation(self, enabled: bool) -> Self {
        let mut checksum = ChecksumCapabilities::default();
        if !enabled {
            checksum.ipv4 = Checksum::Tx;
            checksum.udp = Checksum::Tx;
        }
        self.with_checksum_capabilities(checksum)
    }

    /// Select software generation of the IPv4 UDP checksum.
    ///
    /// Disabling generation emits the RFC 768 zero-checksum representation
    /// and is intended only for a controlled cost diagnostic. The mandatory
    /// IPv4 header checksum and the selected RX checksum policy are preserved.
    pub fn with_software_ipv4_udp_tx_checksum_generation(mut self, enabled: bool) -> Self {
        let validate_rx = matches!(self.inner.checksum.udp, Checksum::Both | Checksum::Rx);
        self.inner.checksum.udp = match (validate_rx, enabled) {
            (true, true) => Checksum::Both,
            (true, false) => Checksum::Rx,
            (false, true) => Checksum::Tx,
            (false, false) => Checksum::None,
        };
        self
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> Drop
    for SplitPinnedDevice<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if self.waiting_for_tx_credit {
            self.tx_credit_waiters
                .fetch_and(!(1_u32 << self.interface.value()), Ordering::AcqRel);
            self.waiting_for_tx_credit = false;
        }
        if self.waiting_for_control_tx {
            self.control_tx_waiters
                .fetch_and(!(1_u32 << self.interface.value()), Ordering::AcqRel);
            self.waiting_for_control_tx = false;
        }
        let ingress = self.ingress_tx.take();
        let application = self.application_tx.take();
        let control = self.control_tx.take();
        #[cfg(feature = "tx-phase-telemetry")]
        {
            self.tx_ingress_reserved
                .fetch_sub(u32::from(ingress.is_some()), Ordering::Relaxed);
            self.tx_application_reserved
                .fetch_sub(u32::from(application.is_some()), Ordering::Relaxed);
            self.tx_control_reserved
                .fetch_sub(u32::from(control.is_some()), Ordering::Relaxed);
        }
        for index in [ingress, application].into_iter().flatten() {
            #[cfg(feature = "tx-staging-copy-probe")]
            if self.staged_tx_selected {
                if let Err(TrySendError::Full(_)) = self.free_staged_return.try_send(index) {
                    unreachable!("reserved staged TX index was lost");
                }
            } else if let Err(TrySendError::Full(_)) = self.free_tx_return.try_send(index) {
                unreachable!("reserved pinned TX index was lost");
            }
            #[cfg(not(feature = "tx-staging-copy-probe"))]
            if let Err(TrySendError::Full(_)) = self.free_tx_return.try_send(index) {
                unreachable!("reserved pinned TX index was lost");
            }
            self.tx_credit_wakers.wake_waiter_after(
                self.interface,
                self.tx_active,
                self.tx_credit_waiters,
            );
        }
        if let Some(index) = control {
            PinnedTxReturn {
                free_tx: self.free_tx_return,
                interface: self.interface,
                tx_credit_wakers: self.tx_credit_wakers,
                tx_credit_waiters: self.tx_credit_waiters,
                control_tx_index: self.control_tx_index,
                control_tx_available: self.control_tx_available,
                control_tx_waiters: self.control_tx_waiters,
                tx_active: self.tx_active,
            }
            .return_network_index(index);
        }
    }
}

enum PinnedTransmitLease<
    'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
> {
    Direct(PinnedDmaTxNetworkLease<'resources, FRAME_CAPACITY, HEADROOM, TRAILER>),
    #[cfg(feature = "tx-staging-copy-probe")]
    Staged(RxRadioLease<'resources, FRAME_CAPACITY>),
}

pub struct PinnedTransmitToken<
    'device,
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    tx_return: PinnedTxReturn<'resources, M, QUEUE_DEPTH>,
    tx_metadata: &'resources [PinnedTxMetadataSlot; QUEUE_DEPTH],
    ready_tx: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-staging-copy-probe")]
    free_staged: Sender<'resources, M, u8, QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    ready_staged: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_pool: &'resources RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_metadata: &'resources [PinnedTxMetadataSlot; QUEUE_DEPTH],
    metadata: PinnedTxMetadata,
    tx_published: &'resources Signal<M, ()>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_tokens_in_flight: &'resources AtomicU32,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    lease: Option<PinnedTransmitLease<'resources, FRAME_CAPACITY, HEADROOM, TRAILER>>,
    _reservation: &'device mut (),
}

/// CPU-only packet owner published by the network task.
///
/// Its storage can live in PSRAM. The type deliberately does not implement
/// `StableDmaBacking`; a radio aggregate must first consume it through
/// [`PinnedTxInterfaceConsumer::try_promote`].
#[cfg(feature = "tx-staging-copy-probe")]
pub struct StagedTxFrame<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    tag: PinnedTxOwnerTag,
    lease: Option<RxNetworkLease<'resources, FRAME_CAPACITY>>,
    free_staged: Sender<'resources, M, u8, QUEUE_DEPTH>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
}

#[cfg(feature = "tx-staging-copy-probe")]
impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    StagedTxFrame<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub fn as_slice(&self) -> &[u8] {
        self.lease.as_ref().expect("live staged TX frame").frame()
    }

    pub fn ethernet(&self) -> &[u8] {
        self.as_slice()
    }

    pub const fn tag(&self) -> &NetworkInterfaceId {
        &self.tag.interface
    }

    const fn owner_tag(&self) -> PinnedTxOwnerTag {
        self.tag
    }

    fn release_index(mut self) -> u8 {
        let lease = self.lease.take().expect("live staged TX frame");
        lease.release()
    }

    /// Publish this source index to the Core1 materializer without returning
    /// its producer credit. The value-only request becomes the unique owner;
    /// Core1 must claim the READY slot and perform the terminal release.
    #[cfg(feature = "tx-core1-materializer-probe")]
    fn handoff_to_materializer(mut self) -> u8 {
        let lease = self.lease.take().expect("live staged TX frame");
        let length = lease.frame().len();
        lease.republish(0, length)
    }

    fn release(self) {
        let free_staged = self.free_staged;
        let tx_credit_wakers = self.tx_credit_wakers;
        let tx_credit_waiters = self.tx_credit_waiters;
        let tx_active = self.tx_active;
        let interface = self.tag.interface();
        let expected_index = self.tag.pool_index();
        let index = self.release_index();
        debug_assert_eq!(index, expected_index);
        if let Err(TrySendError::Full(_)) = free_staged.try_send(index) {
            unreachable!("staged TX frame returns its unique PSRAM index");
        }
        tx_credit_wakers.wake_waiter_after(interface, tx_active, tx_credit_waiters);
    }
}

#[cfg(feature = "tx-staging-copy-probe")]
impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for StagedTxFrame<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let index = lease.release();
            if let Err(TrySendError::Full(_)) = self.free_staged.try_send(index) {
                unreachable!("dropped staged TX frame returns its unique PSRAM index");
            }
            self.tx_credit_wakers.wake_waiter_after(
                self.tag.interface(),
                self.tx_active,
                self.tx_credit_waiters,
            );
        }
    }
}

/// Network-to-radio packet ownership before physical DMA admission.
pub enum PinnedNetworkTxFrame<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    Direct(PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>),
    #[cfg(feature = "tx-staging-copy-probe")]
    Staged(StagedTxFrame<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>),
}

impl<'resources, M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize>
    PinnedNetworkTxFrame<'resources, M, F, H, T, Q>
{
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Direct(frame) => frame.as_slice(),
            #[cfg(feature = "tx-staging-copy-probe")]
            Self::Staged(frame) => frame.as_slice(),
        }
    }

    pub fn ethernet(&self) -> &[u8] {
        self.as_slice()
    }

    pub fn tag(&self) -> &NetworkInterfaceId {
        match self {
            Self::Direct(frame) => &frame.tag().interface,
            #[cfg(feature = "tx-staging-copy-probe")]
            Self::Staged(frame) => frame.tag(),
        }
    }

    const fn owner_tag(&self) -> PinnedTxOwnerTag {
        match self {
            Self::Direct(frame) => *frame.tag(),
            #[cfg(feature = "tx-staging-copy-probe")]
            Self::Staged(frame) => frame.owner_tag(),
        }
    }

    pub fn into_direct(self) -> Result<PinnedTxFrame<'resources, M, F, H, T, Q>, Self> {
        match self {
            Self::Direct(frame) => Ok(frame),
            #[cfg(feature = "tx-staging-copy-probe")]
            Self::Staged(frame) => Err(Self::Staged(frame)),
        }
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> embassy_net_driver::TxToken
    for PinnedTransmitToken<'_, '_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn consume<R, F>(mut self, length: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let interface = self.metadata.interface();
        #[cfg(feature = "tx-phase-telemetry")]
        let consume_started = TxPerformanceSample::read();
        assert!(
            length <= FRAME_CAPACITY,
            "embassy-net requested a frame larger than pinned driver capabilities"
        );
        let lease = self.lease.take().expect("TX token consumed once");
        #[cfg(feature = "tx-phase-telemetry")]
        let mut emitted = TxPerformanceSample::default();
        let (index, staged, result) = match lease {
            PinnedTransmitLease::Direct(lease) => {
                let (index, result) = lease.publish(length, |buffer| {
                    #[cfg(feature = "tx-phase-telemetry")]
                    let started = TxPerformanceSample::read();
                    let result = f(buffer);
                    #[cfg(feature = "tx-phase-telemetry")]
                    {
                        emitted = TxPerformanceSample::read().wrapping_delta_since(started);
                    }
                    result
                });
                (index, false, result)
            }
            #[cfg(feature = "tx-staging-copy-probe")]
            PinnedTransmitLease::Staged(lease) => {
                let (index, result) = lease.publish(length, |buffer| {
                    #[cfg(feature = "tx-phase-telemetry")]
                    let started = TxPerformanceSample::read();
                    let result = f(buffer);
                    #[cfg(feature = "tx-phase-telemetry")]
                    {
                        emitted = TxPerformanceSample::read().wrapping_delta_since(started);
                    }
                    result
                });
                (index, true, result)
            }
        };
        #[cfg(feature = "tx-staging-copy-probe")]
        let (ready, index) = if staged {
            self.staged_metadata[usize::from(index)].publish(self.metadata);
            (&self.ready_staged[usize::from(interface.value())], index)
        } else {
            self.tx_metadata[usize::from(index)].publish(self.metadata);
            (&self.ready_tx[usize::from(interface.value())], index)
        };
        #[cfg(not(feature = "tx-staging-copy-probe"))]
        let _ = staged;
        #[cfg(not(feature = "tx-staging-copy-probe"))]
        self.tx_metadata[usize::from(index)].publish(self.metadata);
        #[cfg(not(feature = "tx-staging-copy-probe"))]
        let ready = &self.ready_tx[usize::from(interface.value())];
        if let Err(TrySendError::Full(_)) = ready.try_send(index) {
            unreachable!("one ready entry exists per non-free pinned TX slot");
        }
        // Link-down and publication run on different cores. Shutdown first
        // makes the VIF inactive and then drains its ready frontier, while a
        // synchronous stack emission may already own the final token. Cover
        // the opposite ordering here: if shutdown won the race before this
        // publication, return every inactive-VIF owner immediately. No other
        // token for this device can coexist because `_reservation` is affine.
        if self.tx_active.load(Ordering::Acquire) & (1_u32 << interface.value()) == 0 {
            #[cfg(feature = "tx-staging-copy-probe")]
            let mut staged_returned = false;
            while let Ok(stale_index) = ready.try_receive() {
                #[cfg(feature = "tx-staging-copy-probe")]
                if staged {
                    let stale_index = self.staged_pool.claim_network(stale_index).release();
                    if let Err(TrySendError::Full(_)) = self.free_staged.try_send(stale_index) {
                        unreachable!("late staged publication returns its unique index");
                    }
                    staged_returned = true;
                } else {
                    let stale_index = self.tx_pool.claim_radio(stale_index).release();
                    self.tx_return.return_network_index(stale_index);
                }
                #[cfg(not(feature = "tx-staging-copy-probe"))]
                {
                    let stale_index = self.tx_pool.claim_radio(stale_index).release();
                    self.tx_return.return_network_index(stale_index);
                }
            }
            #[cfg(feature = "tx-staging-copy-probe")]
            if staged_returned {
                self.tx_credit_wakers.wake_waiter_after(
                    interface,
                    self.tx_active,
                    self.tx_credit_waiters,
                );
            }
        }
        #[cfg(feature = "tx-phase-telemetry")]
        self.tx_tokens_in_flight.fetch_sub(1, Ordering::Relaxed);
        #[cfg(feature = "tx-phase-telemetry")]
        TX_PERFORMANCE.record_publication_geometry(self.tx_return.free_count(), ready.len());
        self.tx_published.signal(());
        #[cfg(feature = "tx-phase-telemetry")]
        TX_PERFORMANCE.record_consume(
            length,
            consume_started,
            emitted,
            TxPerformanceSample::read(),
        );
        result
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Drop for PinnedTransmitToken<'_, '_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            #[cfg(feature = "tx-phase-telemetry")]
            self.tx_tokens_in_flight.fetch_sub(1, Ordering::Relaxed);
            let staged_returned = match lease {
                PinnedTransmitLease::Direct(lease) => {
                    let index = lease.release();
                    self.tx_return.return_network_index(index);
                    false
                }
                #[cfg(feature = "tx-staging-copy-probe")]
                PinnedTransmitLease::Staged(lease) => {
                    let index = lease.release();
                    if let Err(TrySendError::Full(_)) = self.free_staged.try_send(index) {
                        unreachable!("dropped staged TX token returns its unique index");
                    }
                    true
                }
            };
            if staged_returned {
                self.tx_credit_wakers.wake_waiter_after(
                    self.metadata.interface(),
                    self.tx_active,
                    self.tx_credit_waiters,
                );
            }
        }
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> Driver
    for SplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    type RxToken<'device>
        = PinnedReceiveToken<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
    where
        Self: 'device;
    type TxToken<'device>
        = PinnedTransmitToken<
        'device,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >
    where
        Self: 'device;

    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        #[cfg(feature = "tx-core1-materializer-probe")]
        self.service_core1_materialization(cx);
        if !self.poll_reserve_ingress_tx(cx) {
            return None;
        }
        let index = match self.ready_rx.poll_receive(cx) {
            Poll::Ready(index) => index,
            Poll::Pending => return None,
        };
        let lease = self.rx_pool.claim_network(index);
        let tx_index = self
            .ingress_tx
            .take()
            .expect("ingress admission reserves one TX credit");
        #[cfg(feature = "tx-phase-telemetry")]
        self.tx_ingress_reserved.fetch_sub(1, Ordering::Relaxed);
        Some((
            PinnedReceiveToken {
                free_rx: self.free_rx,
                lease: Some(lease),
            },
            self.take_tx_token(
                tx_index,
                PinnedTxMetadata::unclassified(self.interface),
                PinnedTxAdmissionClass::Ordinary,
            ),
        ))
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        #[cfg(feature = "tx-core1-materializer-probe")]
        self.service_core1_materialization(cx);
        #[cfg(feature = "tx-phase-telemetry")]
        let started = TxPerformanceSample::read();
        let token = if !self.poll_reserve_application_tx(cx) {
            None
        } else {
            let index = self
                .application_tx
                .take()
                .expect("application admission reserves one TX credit");
            #[cfg(feature = "tx-phase-telemetry")]
            self.tx_application_reserved.fetch_sub(1, Ordering::Relaxed);
            Some(self.take_tx_token(
                index,
                PinnedTxMetadata::unclassified(self.interface),
                PinnedTxAdmissionClass::Ordinary,
            ))
        };
        #[cfg(feature = "tx-phase-telemetry")]
        TX_PERFORMANCE.record_admission(started, TxPerformanceSample::read(), token.is_some());
        token
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn transmit_control(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        #[cfg(feature = "tx-phase-telemetry")]
        let started = TxPerformanceSample::read();
        let token = if !self.poll_reserve_control_tx(cx) {
            None
        } else {
            let index = self
                .control_tx
                .take()
                .expect("control admission reserves the global control credit");
            #[cfg(feature = "tx-phase-telemetry")]
            self.tx_control_reserved.fetch_sub(1, Ordering::Relaxed);
            Some(self.take_tx_token(
                index,
                PinnedTxMetadata::unclassified(self.interface),
                PinnedTxAdmissionClass::Control,
            ))
        };
        #[cfg(feature = "tx-phase-telemetry")]
        TX_PERFORMANCE.record_admission(started, TxPerformanceSample::read(), token.is_some());
        token
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn egress_key(&mut self, route: EgressRoute) -> EgressKey {
        let epoch = self.refresh_scheduling_epoch();
        self.endpoint.classify(epoch, route)
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn transmit_for(
        &mut self,
        cx: &mut Context<'_>,
        egress: EgressKey,
    ) -> EgressAdmission<Self::TxToken<'_>> {
        #[cfg(feature = "tx-phase-telemetry")]
        let started = TxPerformanceSample::read();
        let epoch = self.refresh_scheduling_epoch();
        if !self.endpoint.key_is_current(egress, epoch) {
            #[cfg(feature = "tx-phase-telemetry")]
            TX_PERFORMANCE.record_admission(started, TxPerformanceSample::read(), false);
            return EgressAdmission::KeyDeferred;
        }
        if self.egress_demand_active
            && !self
                .egress_grant
                .as_ref()
                .is_some_and(|grant| grant.authorizes(egress))
        {
            #[cfg(feature = "tx-phase-telemetry")]
            TX_PERFORMANCE.record_admission(started, TxPerformanceSample::read(), false);
            return EgressAdmission::KeyDeferred;
        }
        let run_changed = self.keyed_egress != Some(egress);
        if run_changed {
            #[cfg(feature = "tx-phase-telemetry")]
            TX_PERFORMANCE.record_egress_run(self.keyed_run_length);
            self.keyed_egress = Some(egress);
            self.keyed_run_length = 0;
        }

        if !self.poll_reserve_application_tx(cx) {
            #[cfg(feature = "tx-phase-telemetry")]
            TX_PERFORMANCE.record_admission(started, TxPerformanceSample::read(), false);
            return EgressAdmission::GlobalExhausted;
        }
        let index = self
            .application_tx
            .take()
            .expect("keyed application admission reserves one TX credit");
        #[cfg(feature = "tx-phase-telemetry")]
        self.tx_application_reserved.fetch_sub(1, Ordering::Relaxed);
        if self.egress_demand_active {
            let grant = self
                .egress_grant
                .as_mut()
                .expect("authoritative admission retains its affine grant");
            grant.used_frames = grant
                .used_frames
                .checked_add(1)
                .expect("a bounded grant cannot overflow its frame count");
        }
        #[cfg(feature = "tx-phase-telemetry")]
        self.observe_successful_shadow_grant(egress);
        self.keyed_run_length = self.keyed_run_length.saturating_add(1);
        #[cfg(feature = "tx-phase-telemetry")]
        TX_PERFORMANCE.record_admission(started, TxPerformanceSample::read(), true);
        EgressAdmission::Granted(self.take_tx_token(
            index,
            PinnedTxMetadata::classified(self.interface, egress),
            PinnedTxAdmissionClass::Ordinary,
        ))
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> LinkState {
        self.link.get(cx)
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.max_transmission_unit = FRAME_CAPACITY;
        capabilities.max_burst_size = Some(RX_QUEUE_DEPTH.min(TX_QUEUE_DEPTH));
        capabilities.checksum = self.checksum.clone();
        capabilities
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn egress_schedule(&mut self) -> Option<EgressSchedule> {
        self.flush_egress_grant_progress();
        if self.egress_demand_active
            && self.egress_demand_flush_pending
            && let Some(control) = self.egress_control.as_mut()
        {
            self.egress_demand_flush_pending = control.flush_egress_demand();
        }
        if self.link.is_up() && crate::keyed_egress_scheduling_enabled() {
            let epoch = self.refresh_scheduling_epoch();
            Some(EgressSchedule::new(
                NonZeroU8::new(32).unwrap(),
                NonZeroU8::new(crate::keyed_egress_dispatch_quantum()).unwrap(),
                epoch,
                if self.egress_demand_active && self.egress_control.is_some() {
                    EgressGrantMode::Authoritative
                } else {
                    EgressGrantMode::StackSelected
                },
            ))
        } else {
            // A permanent network stack survives radio role epochs. Returning
            // FIFO while down resets stack-side burst cursors before the next
            // peer generation becomes active; no down-state publication can
            // reach the radio because inactive-VIF owners are reclaimed.
            None
        }
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn update_egress_demand(&mut self, cx: &mut Context<'_>, update: EgressDemandUpdate) {
        self.flush_egress_grant_progress();
        if self.egress_demand_active
            && let Some(control) = self.egress_control.as_mut()
        {
            // Malformed or over-capacity lifecycles are omitted from the
            // radio mirror. A valid Core0 grant and synchronous SRAM claim are
            // jointly required for final packet materialization.
            if let Ok(pending) = control.update_egress_demand(cx, update) {
                self.egress_demand_flush_pending = pending;
            }
        }
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn poll_egress_grant(&mut self, cx: &mut Context<'_>) -> Option<DriverEgressBurstGrant> {
        self.flush_egress_grant_progress();
        if !self.egress_demand_active || self.egress_standby_grant.is_some() {
            return None;
        }
        let grant = self.egress_control.as_deref_mut()?.try_receive_grant(cx)?;
        if self.egress_grant.is_none() {
            self.egress_grant = Some(PinnedEgressGrantState::new(grant));
        } else {
            self.egress_standby_grant = Some(PinnedEgressGrantState::new(grant));
        }
        Some(DriverEgressBurstGrant::new(
            grant.serial(),
            grant.demand(),
            grant.frame_credits(),
            grant.airtime_hundred_nanoseconds(),
        ))
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn finish_egress_grant(&mut self, _cx: &mut Context<'_>, completion: EgressGrantCompletion) {
        self.flush_egress_grant_progress();
        // A completion can only name one of the two affine grants returned by
        // `poll_egress_grant`. A mismatch is ignored as duplicate/foreign and
        // cannot close either live owner. A standby may close unused during an
        // epoch reset while current progress is waiting for transport space.
        for state in [&mut self.egress_grant, &mut self.egress_standby_grant] {
            if let Some(state) = state.as_mut()
                && state.grant.serial() == completion.serial()
                && state.completion.is_none()
                && state.used_frames == completion.used_frames()
                && completion.used_frames() <= state.grant.frame_credits().get()
            {
                state.completion = Some(completion);
                break;
            }
        }
        self.flush_egress_grant_progress();
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.endpoint.hardware_address())
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    const SHARED_CAPACITY: usize,
    const SHARED_SLOTS: usize,
> Driver
    for SharedRxSplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
        SHARED_CAPACITY,
        SHARED_SLOTS,
    >
{
    type RxToken<'device>
        = SharedPinnedReceiveToken<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH, SHARED_CAPACITY>
    where
        Self: 'device;
    type TxToken<'device>
        = PinnedTransmitToken<
        'device,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >
    where
        Self: 'device;

    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.inner.poll_reserve_ingress_tx(cx) {
            return None;
        }
        let ready = match self.shared.ready.poll_receive(cx) {
            Poll::Ready(ready) => ready,
            Poll::Pending => return None,
        };
        let rx = match ready.source() {
            OrderedRxSource::Owned(index) => {
                let lease = self.inner.rx_pool.claim_network(index);
                SharedPinnedReceiveToken::Owned(PinnedReceiveToken {
                    free_rx: self.inner.free_rx,
                    lease: Some(lease),
                })
            }
            OrderedRxSource::Shared(index) => {
                let lease = self.shared.pool.claim_network(index);
                SharedPinnedReceiveToken::Shared(SharedPoolReceiveToken {
                    lease: Some(lease),
                    on_release: self.shared.on_release,
                })
            }
        };
        let tx_index = self
            .inner
            .ingress_tx
            .take()
            .expect("ordered ingress admission reserves one TX credit");
        #[cfg(feature = "tx-phase-telemetry")]
        self.inner
            .tx_ingress_reserved
            .fetch_sub(1, Ordering::Relaxed);
        let interface = self.inner.interface;
        let tx = self.inner.take_tx_token(
            tx_index,
            PinnedTxMetadata::unclassified(interface),
            PinnedTxAdmissionClass::Ordinary,
        );
        Some((rx, tx))
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        self.inner.transmit(cx)
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn transmit_control(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        self.inner.transmit_control(cx)
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn egress_key(&mut self, route: EgressRoute) -> EgressKey {
        self.inner.egress_key(route)
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn transmit_for(
        &mut self,
        cx: &mut Context<'_>,
        egress: EgressKey,
    ) -> EgressAdmission<Self::TxToken<'_>> {
        self.inner.transmit_for(cx, egress)
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> LinkState {
        self.inner.link_state(cx)
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = self.inner.capabilities();
        capabilities.max_burst_size = Some(
            RX_QUEUE_DEPTH
                .saturating_add(SHARED_SLOTS)
                .min(TX_QUEUE_DEPTH),
        );
        capabilities
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn egress_schedule(&mut self) -> Option<EgressSchedule> {
        self.inner.egress_schedule()
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn update_egress_demand(&mut self, cx: &mut Context<'_>, update: EgressDemandUpdate) {
        self.inner.update_egress_demand(cx, update);
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn poll_egress_grant(&mut self, cx: &mut Context<'_>) -> Option<DriverEgressBurstGrant> {
        self.inner.poll_egress_grant(cx)
    }

    #[cfg(feature = "tx-egress-scheduling")]
    fn finish_egress_grant(&mut self, cx: &mut Context<'_>, completion: EgressGrantCompletion) {
        self.inner.finish_egress_grant(cx, completion);
    }

    fn hardware_address(&self) -> HardwareAddress {
        self.inner.hardware_address()
    }
}

/// Narrow radio-side capability that can only publish received Ethernet
/// frames to `embassy-net`.
///
/// This view deliberately contains no TX capability. It can therefore be
/// moved into an RX protocol sink while [`SplitPinnedRxRunner`] remains the
/// unique owner of TX leases.
pub struct PinnedRxPublisher<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    free_rx: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    free_rx_return: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ready_rx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ordered_rx: Option<Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>>,
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>,
    reserved_rx: Option<u8>,
}

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    PinnedRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn validate_length(length: usize) -> Result<(), FrameLengthError> {
        if length < ETHERNET_HEADER_LEN {
            Err(FrameLengthError::TooShort)
        } else if length > FRAME_CAPACITY {
            Err(FrameLengthError::TooLong)
        } else {
            Ok(())
        }
    }

    fn try_claim_slot(
        &mut self,
    ) -> Result<RxRadioLease<'resources, FRAME_CAPACITY>, RxEnqueueError> {
        let index = if let Some(index) = self.reserved_rx.take() {
            index
        } else {
            self.free_rx
                .try_receive()
                .map_err(|TryReceiveError::Empty| RxEnqueueError::QueueFull)?
        };
        Ok(self.rx_pool.claim_radio(index))
    }

    fn publish<R>(
        &self,
        lease: RxRadioLease<'resources, FRAME_CAPACITY>,
        length: usize,
        write: impl FnOnce(&mut [u8]) -> R,
    ) -> R {
        let (index, result) = lease.publish(length, write);
        self.publish_index(index);
        result
    }

    fn publish_index(&self, index: u8) {
        let published = if let Some(ordered) = self.ordered_rx {
            ordered.try_send(OrderedRxReady::owned(index)).is_ok()
        } else {
            self.ready_rx.try_send(index).is_ok()
        };
        if !published {
            unreachable!("one ordered or owned ready entry exists per non-free pinned RX slot");
        }
    }

    pub fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        Self::validate_length(frame.len()).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        self.publish(lease, frame.len(), |storage| storage.copy_from_slice(frame));
        Ok(())
    }

    /// Publish one contiguous Ethernet frame while exposing the exact edge
    /// before the claimed slot becomes visible to the network consumer.
    #[cfg(feature = "diagnostics")]
    pub fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: impl FnOnce(),
    ) -> Result<(), RxEnqueueError> {
        Self::validate_length(frame.len()).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        let (index, ()) = lease.publish(frame.len(), |storage| storage.copy_from_slice(frame));
        before_publish();
        self.publish_index(index);
        Ok(())
    }

    pub fn try_send_parts(
        &mut self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<(), RxEnqueueError> {
        let length = ETHERNET_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))?;
        Self::validate_length(length).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        self.publish(lease, length, |frame| {
            frame[..6].copy_from_slice(&destination);
            frame[6..12].copy_from_slice(&source);
            frame[12..14].copy_from_slice(&ether_type.to_be_bytes());
            frame[14..].copy_from_slice(payload);
        });
        Ok(())
    }

    /// Publish one Ethernet frame while exposing the exact ownership edge at
    /// which the claimed slot becomes visible to the network consumer.
    ///
    /// This method is absent from ordinary builds. `before_publish` runs after
    /// the frame copy but before insertion into `ready_rx`; failed admission
    /// never calls it.
    #[cfg(feature = "diagnostics")]
    pub fn try_send_parts_observed(
        &mut self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
        before_publish: impl FnOnce(),
    ) -> Result<(), RxEnqueueError> {
        let length = ETHERNET_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))?;
        Self::validate_length(length).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        let (index, ()) = lease.publish(length, |frame| {
            frame[..6].copy_from_slice(&destination);
            frame[6..12].copy_from_slice(&source);
            frame[12..14].copy_from_slice(&ether_type.to_be_bytes());
            frame[14..].copy_from_slice(payload);
        });
        before_publish();
        self.publish_index(index);
        Ok(())
    }

    pub async fn send(&mut self, frame: &[u8]) -> Result<(), FrameLengthError> {
        Self::validate_length(frame.len())?;
        self.wait_ready().await;
        self.try_send(frame)
            .expect("wait_ready reserved one pinned RX slot");
        Ok(())
    }

    /// Wait until at least one receive-queue owner is available.
    ///
    /// A protocol adapter can hold its independently staged radio frame while
    /// awaiting this edge, propagating bounded network backpressure instead of
    /// silently discarding a decoded Ethernet frame.
    pub async fn wait_ready(&mut self) {
        if self.reserved_rx.is_none() {
            self.reserved_rx = Some(self.free_rx.receive().await);
        }
    }

    pub fn free_capacity(&self) -> usize {
        self.free_rx.len() + usize::from(self.reserved_rx.is_some())
    }

    pub fn queue_len(&self) -> usize {
        self.ordered_rx
            .map_or_else(|| self.ready_rx.len(), |ready| ready.len())
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for PinnedRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(index) = self.reserved_rx.take()
            && let Err(TrySendError::Full(_)) = self.free_rx_return.try_send(index)
        {
            unreachable!("reserved pinned RX index was lost");
        }
    }
}

pub struct SplitPinnedRxRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const RX_QUEUE_DEPTH: usize,
> {
    free_rx: Receiver<'resources, M, u8, RX_QUEUE_DEPTH>,
    free_rx_return: Sender<'resources, M, u8, RX_QUEUE_DEPTH>,
    ready_rx: Sender<'resources, M, u8, RX_QUEUE_DEPTH>,
    ordered_rx: Option<Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>>,
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
    tx_active: &'resources AtomicU32,
    tx_interface: NetworkInterfaceId,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
}

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize>
    SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
{
    /// Bind copied RX publications to the same typed frontier as shared
    /// staging slots. The matching device must be wrapped with
    /// [`SplitPinnedDevice::with_shared_rx`] using this exact consumer.
    pub fn with_shared_rx_ordering<const SHARED_CAPACITY: usize, const SHARED_SLOTS: usize>(
        mut self,
        shared: &SharedPinnedRxConsumer<'resources, M, SHARED_CAPACITY, SHARED_SLOTS>,
    ) -> Self {
        assert!(
            RX_QUEUE_DEPTH.saturating_add(SHARED_SLOTS) <= ORDERED_RX_READY_CAPACITY,
            "ordered RX frontier must cover every owned and shared slot"
        );
        assert!(
            RX_QUEUE_DEPTH <= usize::from(ORDERED_RX_SHARED_BIT)
                && SHARED_SLOTS <= usize::from(ORDERED_RX_SHARED_BIT),
            "ordered RX pool indices must fit in seven bits"
        );
        self.ordered_rx = Some(shared.ready_sender);
        self
    }

    /// Derive the receive-only capability before moving this runner into the
    /// production Wi-Fi event loop. The returned handle cannot observe or
    /// claim any network-owned TX slot.
    pub fn rx_publisher(&self) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        PinnedRxPublisher {
            free_rx: self.free_rx,
            free_rx_return: self.free_rx_return,
            ready_rx: self.ready_rx,
            ordered_rx: self.ordered_rx,
            rx_pool: self.rx_pool,
            reserved_rx: None,
        }
    }

    pub fn set_link_state(&self, state: LinkState) {
        if state == LinkState::Up {
            // Make this endpoint eligible for returned-credit notification
            // before it becomes visible to its network stack.
            self.tx_active
                .fetch_or(1_u32 << self.tx_interface.value(), Ordering::AcqRel);
            self.tx_credit_wakers.wake_all();
            self.link.set(state);
        } else {
            // Stop network admission before removing the endpoint from the
            // returned-credit notification set.
            self.link.set(state);
            self.tx_active
                .fetch_and(!(1_u32 << self.tx_interface.value()), Ordering::AcqRel);
            self.tx_credit_wakers.wake_all();
        }
    }

    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        let mut publisher = self.rx_publisher();
        publisher.try_send(frame)
    }

    pub async fn send_rx(&self, frame: &[u8]) -> Result<(), FrameLengthError> {
        let mut publisher = self.rx_publisher();
        publisher.send(frame).await
    }

    pub fn rx_queue_len(&self) -> usize {
        self.ready_rx.len()
    }

    fn link_endpoint(&self) -> PinnedLinkEndpoint<'resources, M> {
        PinnedLinkEndpoint {
            interface: self.tx_interface,
            link: self.link,
            tx_active: self.tx_active,
            tx_credit_wakers: self.tx_credit_wakers,
        }
    }
}

struct PinnedLinkEndpoint<'resources, M: RawMutex> {
    interface: NetworkInterfaceId,
    link: &'resources SharedLinkState<M>,
    tx_active: &'resources AtomicU32,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
}

impl<M: RawMutex> Clone for PinnedLinkEndpoint<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for PinnedLinkEndpoint<'_, M> {}

impl<M: RawMutex> PinnedLinkEndpoint<'_, M> {
    fn set_link_state(self, state: LinkState) {
        if state == LinkState::Up {
            self.tx_active
                .fetch_or(1_u32 << self.interface.value(), Ordering::AcqRel);
            self.tx_credit_wakers.wake_all();
            self.link.set(state);
        } else {
            self.link.set(state);
            self.tx_active
                .fetch_and(!(1_u32 << self.interface.value()), Ordering::AcqRel);
            self.tx_credit_wakers.wake_all();
        }
    }
}

/// Copyable link-state capability independent of the unique Core0 scheduler.
///
/// AP control may retain this value while the datapath exclusively borrows
/// the egress policy. It can only publish link state and discard TX leases
/// which became ineligible; it cannot service radio policy or consume an
/// active frame.
pub struct PinnedNetworkLinkController<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    first: PinnedLinkEndpoint<'resources, M>,
    second: Option<PinnedLinkEndpoint<'resources, M>>,
    tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Clone for PinnedNetworkLinkController<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Copy for PinnedNetworkLinkController<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedNetworkLinkController<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub fn set_link_state(self, interface: NetworkInterfaceId, state: LinkState) {
        let endpoint = if self.first.interface == interface {
            self.first
        } else {
            self.second
                .filter(|endpoint| endpoint.interface == interface)
                .expect("network interface does not belong to this link controller")
        };
        endpoint.set_link_state(state);
        self.tx.discard_inactive_ready();
    }
}

/// Narrow radio-side capability for claiming ready network TX leases.
///
/// This value is the sole radio-side consumer created by [`PinnedTxResources`]
/// and is
/// independent of RX storage geometry. Aggregate construction may retain a
/// reference to it while claiming additional frames without gaining access to
/// link state or receive publication.
pub struct PinnedTxConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    tx_metadata: &'resources [PinnedTxMetadataSlot; QUEUE_DEPTH],
    #[cfg(feature = "tx-staging-copy-probe")]
    free_tx_claim: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    ready_tx: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-staging-copy-probe")]
    free_staged: Sender<'resources, M, u8, QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    ready_staged: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_pool: &'resources RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>,
    #[cfg(feature = "tx-staging-copy-probe")]
    staged_metadata: &'resources [PinnedTxMetadataSlot; QUEUE_DEPTH],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_requests:
        &'resources [Channel<M, TxMaterializationRequest, 1>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_completions:
        &'resources [Channel<M, TxMaterializationCompletion, 1>; PINNED_TX_CREDIT_WAKER_SLOTS],
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_in_flight: &'resources AtomicU32,
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_cancel: &'resources AtomicU32,
    #[cfg(feature = "tx-core1-materializer-probe")]
    materialization_wakers: &'resources PinnedTxCreditWakers<M>,
    next_interface: &'resources AtomicU32,
    tx_published: &'resources Signal<M, ()>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    control_tx_index: &'resources AtomicUsize,
    control_tx_available: &'resources AtomicBool,
    control_tx_waiters: &'resources AtomicU32,
    #[cfg(feature = "tx-staging-copy-probe")]
    tx_staged_interfaces: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_ingress_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_application_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_control_reserved: &'resources AtomicU32,
    #[cfg(feature = "tx-phase-telemetry")]
    tx_tokens_in_flight: &'resources AtomicU32,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

/// TX consumer narrowed to one logical interface.
///
/// Aggregate encoders receive this capability instead of the physical
/// consumer. They may extend a batch from their immutable per-VIF FIFO, but
/// can never claim a lease published by another VIF sharing the hardware.
pub struct PinnedTxInterfaceConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    physical: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    interface: NetworkInterfaceId,
}

/// Non-blocking result of reclaiming one Core1-materialized burst.
#[cfg(feature = "tx-core1-materializer-probe")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinnedTxCore1MaterializationPoll {
    Pending,
    Ready(usize),
    Cancelled,
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for PinnedTxInterfaceConsumer<'_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for PinnedTxInterfaceConsumer<'_, M, F, H, T, Q>
{
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for PinnedTxConsumer<'_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for PinnedTxConsumer<'_, M, F, H, T, Q>
{
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    #[cfg(feature = "tx-staging-copy-probe")]
    #[inline]
    fn staging_for(&self, interface: NetworkInterfaceId) -> bool {
        self.tx_staged_interfaces.load(Ordering::Acquire) & (1_u32 << interface.value()) != 0
    }

    fn claim(
        &self,
        interface: NetworkInterfaceId,
        index: u8,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        TaggedStableDmaBacking::new(
            PinnedTxOwnerTag::new(interface, index),
            ReturningStableDmaBacking::new(
                self.tx_pool.claim_radio(index),
                PinnedTxReturn {
                    free_tx: self.free_tx,
                    interface,
                    tx_credit_wakers: self.tx_credit_wakers,
                    tx_credit_waiters: self.tx_credit_waiters,
                    control_tx_index: self.control_tx_index,
                    control_tx_available: self.control_tx_available,
                    control_tx_waiters: self.control_tx_waiters,
                    tx_active: self.tx_active,
                },
            ),
        )
    }

    #[cfg(feature = "tx-staging-copy-probe")]
    fn claim_with_metadata(
        &self,
        metadata: PinnedTxMetadata,
        index: u8,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        let interface = metadata.interface();
        self.tx_metadata[usize::from(index)].publish(metadata);
        self.claim(interface, index)
    }

    #[cfg(feature = "tx-staging-copy-probe")]
    fn claim_staged(
        &self,
        interface: NetworkInterfaceId,
        index: u8,
    ) -> StagedTxFrame<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH> {
        StagedTxFrame {
            tag: PinnedTxOwnerTag::new(interface, index),
            lease: Some(self.staged_pool.claim_network(index)),
            free_staged: self.free_staged,
            tx_credit_wakers: self.tx_credit_wakers,
            tx_credit_waiters: self.tx_credit_waiters,
            tx_active: self.tx_active,
        }
    }

    pub const fn for_interface(
        self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    {
        PinnedTxInterfaceConsumer {
            physical: self,
            interface,
        }
    }

    pub fn try_receive(
        &self,
    ) -> Option<PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>
    {
        let start = self.next_interface.fetch_add(1, Ordering::Relaxed) as usize
            % PINNED_TX_CREDIT_WAKER_SLOTS;
        for offset in 0..PINNED_TX_CREDIT_WAKER_SLOTS {
            let interface = (start + offset) % PINNED_TX_CREDIT_WAKER_SLOTS;
            #[cfg(feature = "tx-staging-copy-probe")]
            let staged = self.staging_for(NetworkInterfaceId::new(interface as u8));
            #[cfg(feature = "tx-staging-copy-probe")]
            let index = if staged {
                self.ready_staged[interface].try_receive().ok()
            } else {
                self.ready_tx[interface].try_receive().ok()
            };
            #[cfg(not(feature = "tx-staging-copy-probe"))]
            let index = self.ready_tx[interface].try_receive().ok();
            if let Some(index) = index {
                self.next_interface.store(
                    ((interface + 1) % PINNED_TX_CREDIT_WAKER_SLOTS) as u32,
                    Ordering::Relaxed,
                );
                let interface = NetworkInterfaceId::new(interface as u8);
                #[cfg(feature = "tx-staging-copy-probe")]
                if staged {
                    return Some(PinnedNetworkTxFrame::Staged(
                        self.claim_staged(interface, index),
                    ));
                }
                return Some(PinnedNetworkTxFrame::Direct(self.claim(interface, index)));
            }
        }
        None
    }

    pub async fn receive(
        &self,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        loop {
            if let Some(frame) = self.try_receive() {
                return frame;
            }
            self.wait_publication().await;
        }
    }

    /// Claim a frame from an endpoint configured for direct DMA publication.
    ///
    /// This compatibility-free invariant is used by the STA fast path while
    /// the staged architecture experiment is enabled only for the AP endpoint.
    pub fn try_receive_direct(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>> {
        let frame = self.try_receive()?;
        match frame.into_direct() {
            Ok(frame) => Some(frame),
            Err(_) => panic!("staged TX frame reached a direct-only endpoint"),
        }
    }

    pub fn queue_len(&self) -> usize {
        #[cfg(feature = "tx-staging-copy-probe")]
        {
            (0..PINNED_TX_CREDIT_WAKER_SLOTS)
                .map(|interface| {
                    #[cfg(feature = "tx-core1-materializer-probe")]
                    let materialized = self.materialization_completions[interface]
                        .len()
                        .saturating_mul(PINNED_TX_MATERIALIZATION_BATCH_CAPACITY);
                    #[cfg(not(feature = "tx-core1-materializer-probe"))]
                    let materialized = 0;
                    if self.staging_for(NetworkInterfaceId::new(interface as u8)) {
                        self.ready_staged[interface]
                            .len()
                            .saturating_add(materialized)
                    } else {
                        self.ready_tx[interface].len().saturating_add(materialized)
                    }
                })
                .sum()
        }
        #[cfg(not(feature = "tx-staging-copy-probe"))]
        self.ready_tx.iter().map(Channel::len).sum()
    }

    /// Claim the oldest frame published by one logical interface.
    pub fn try_receive_for(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>
    {
        #[cfg(feature = "tx-staging-copy-probe")]
        if self.staging_for(interface) {
            let index = self.ready_staged[usize::from(interface.value())]
                .try_receive()
                .ok()?;
            return Some(PinnedNetworkTxFrame::Staged(
                self.claim_staged(interface, index),
            ));
        }
        let index = self.ready_tx[usize::from(interface.value())]
            .try_receive()
            .ok()?;
        Some(PinnedNetworkTxFrame::Direct(self.claim(interface, index)))
    }

    /// Count the immutable FIFO frontier for one logical interface.
    pub fn queue_len_for(&self, interface: NetworkInterfaceId) -> usize {
        #[cfg(feature = "tx-core1-materializer-probe")]
        let materialized = self.materialization_completions[usize::from(interface.value())]
            .len()
            .saturating_mul(PINNED_TX_MATERIALIZATION_BATCH_CAPACITY);
        #[cfg(not(feature = "tx-core1-materializer-probe"))]
        let materialized = 0;
        #[cfg(feature = "tx-staging-copy-probe")]
        if self.staging_for(interface) {
            return self.ready_staged[usize::from(interface.value())]
                .len()
                .saturating_add(materialized);
        }
        self.ready_tx[usize::from(interface.value())]
            .len()
            .saturating_add(materialized)
    }

    /// Return every not-yet-claimed frame for one logical interface.
    ///
    /// Role shutdown calls this only after its active/prepared radio owners
    /// have reached a terminal boundary. These entries are therefore pure
    /// network backlog: transmitting them in a later role epoch would be a
    /// stale-VIF correctness error and retaining them would permanently steal
    /// credits from the shared physical pool.
    pub fn discard_ready_for(&self, interface: NetworkInterfaceId) -> usize {
        let mut discarded = 0usize;
        while let Some(frame) = self.try_receive_for(interface) {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }

    /// Reclaim backlog published for every endpoint whose radio-side link is
    /// inactive. This closes a narrow cross-core edge where a final network
    /// poll can publish immediately around link-down.
    pub fn discard_inactive_ready(&self) -> usize {
        let active = self.tx_active.load(Ordering::Acquire);
        let mut discarded = 0usize;
        for interface in 0..PINNED_TX_CREDIT_WAKER_SLOTS {
            if active & (1_u32 << interface) == 0 {
                discarded = discarded.saturating_add(
                    self.discard_ready_for(NetworkInterfaceId::new(interface as u8)),
                );
            }
        }
        discarded
    }

    /// Snapshot the direct pinned pool without claiming or returning a credit.
    ///
    /// The one-copy staging experiment owns a distinct free pool and must not
    /// use this direct-pool invariant as evidence. Production direct TX and
    /// the indexed stack-selector diagnostic both use this exact geometry.
    #[cfg(feature = "tx-phase-telemetry")]
    pub fn ownership_snapshot_for(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxOwnershipSnapshot {
        let ready_for_interface = self.ready_tx[usize::from(interface.value())].len();
        let ready_total = self.ready_tx.iter().map(Channel::len).sum::<usize>();
        let control_free = usize::from(
            self.control_tx_index.load(Ordering::Acquire) < INITIALIZING_CONTROL_TX_INDEX
                && self.control_tx_available.load(Ordering::Acquire),
        );
        PinnedTxOwnershipSnapshot {
            free: self.free_tx.len().saturating_add(control_free),
            control_free,
            ready_for_interface,
            ready_for_other_interfaces: ready_total.saturating_sub(ready_for_interface),
            ingress_reserved: self.tx_ingress_reserved.load(Ordering::Relaxed) as usize,
            application_reserved: self.tx_application_reserved.load(Ordering::Relaxed) as usize,
            control_reserved: self.tx_control_reserved.load(Ordering::Relaxed) as usize,
            tokens_in_flight: self.tx_tokens_in_flight.load(Ordering::Relaxed) as usize,
        }
    }

    pub async fn receive_for(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        loop {
            if let Some(frame) = self.try_receive_for(interface) {
                return frame;
            }
            self.wait_publication().await;
        }
    }

    pub async fn wait_ready_for(&self, interface: NetworkInterfaceId) {
        loop {
            if self.queue_len_for(interface) != 0 {
                return;
            }
            self.wait_publication().await;
        }
    }

    pub async fn wait_publication(&self) {
        self.tx_published.wait().await;
    }

    pub async fn wait_ready(&self) {
        loop {
            if self.queue_len() != 0 {
                return;
            }
            self.wait_publication().await;
        }
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn interface(self) -> NetworkInterfaceId {
        self.interface
    }

    /// Read the immutable classification paired with a live packet owner.
    ///
    /// The affine pool index cannot be reused until `frame` is released, so
    /// this snapshot always belongs to this exact owner rather than a later
    /// packet occupying the same physical slot.
    pub fn metadata(
        &self,
        frame: &PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> PinnedTxMetadata {
        let tag = frame.owner_tag();
        assert_eq!(
            tag.interface(),
            self.interface,
            "TX metadata reader cannot cross an interface boundary"
        );
        match frame {
            PinnedNetworkTxFrame::Direct(_) => {
                self.physical.tx_metadata[usize::from(tag.pool_index())].read(self.interface)
            }
            #[cfg(feature = "tx-staging-copy-probe")]
            PinnedNetworkTxFrame::Staged(_) => {
                self.physical.staged_metadata[usize::from(tag.pool_index())].read(self.interface)
            }
        }
    }

    pub fn direct_metadata(
        &self,
        frame: &PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> PinnedTxMetadata {
        let tag = *frame.tag();
        assert_eq!(
            tag.interface(),
            self.interface,
            "TX metadata reader cannot cross an interface boundary"
        );
        self.physical.tx_metadata[usize::from(tag.pool_index())].read(self.interface)
    }

    pub fn try_receive(
        &self,
    ) -> Option<PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>
    {
        let frame = self.physical.try_receive_for(self.interface);
        frame.inspect(|frame| {
            assert_eq!(
                *frame.tag(),
                self.interface,
                "interface-narrowed TX endpoint received another VIF's lease"
            );
        })
    }

    pub async fn receive(
        &self,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        loop {
            if let Some(frame) = self.try_receive() {
                return frame;
            }
            self.physical.wait_publication().await;
        }
    }

    pub fn try_receive_direct(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>> {
        let frame = self.try_receive()?;
        match frame.into_direct() {
            Ok(frame) => Some(frame),
            Err(_) => panic!("staged TX frame reached a direct-only interface"),
        }
    }

    pub fn queue_len(&self) -> usize {
        self.physical.queue_len_for(self.interface)
    }

    #[cfg(feature = "tx-phase-telemetry")]
    pub fn ownership_snapshot(&self) -> PinnedTxOwnershipSnapshot {
        self.physical.ownership_snapshot_for(self.interface)
    }

    pub async fn wait_ready(&self) {
        loop {
            if self.queue_len() != 0 {
                return;
            }
            self.physical.wait_publication().await;
        }
    }

    /// Wait until this interface has published a complete queue prefix.
    ///
    /// This does not claim a lease. Aggregate owners use it to defer their
    /// expensive encode pass until the missing part of a negotiated batch is
    /// visible, while a concurrent TX completion may still consume a shorter
    /// prefix through the ordinary terminal path.
    pub async fn wait_queue_len_at_least(&self, minimum: usize) {
        loop {
            if self.queue_len() >= minimum {
                return;
            }
            self.physical.wait_publication().await;
        }
    }

    /// Number of staged packets which can be promoted without waiting for a
    /// physical DMA credit at this instant.
    ///
    /// Direct endpoints need no promotion and therefore expose no artificial
    /// limit. The value is a scheduling hint only: batch reservation remains
    /// all-or-nothing so a concurrent direct endpoint cannot cause partial
    /// ownership movement after this observation.
    pub fn promotion_capacity(&self) -> usize {
        #[cfg(feature = "tx-staging-copy-probe")]
        if self.physical.staging_for(self.interface) {
            return self.physical.free_tx_claim.len();
        }
        usize::MAX
    }

    /// Whether this same-image run moves selected staged bursts through the
    /// Core1 driver poll. This is a runtime discriminator, not a second queue
    /// topology: the AP scheduler and its selected peer/TID remain unchanged.
    #[cfg(feature = "tx-core1-materializer-probe")]
    pub fn core1_materializer_selected(&self) -> bool {
        self.physical.staging_for(self.interface) && tx_core1_materializer_enabled()
    }

    /// Transfer one already selected staged burst to the Core1 materializer.
    ///
    /// The method reserves every destination before changing a source owner.
    /// Once it returns `true`, every occupied input slot is empty and the
    /// value-only request owns the exact source/destination pairs. Only one
    /// request may be in flight per logical interface.
    #[cfg(feature = "tx-core1-materializer-probe")]
    pub fn try_submit_core1_materialization<const BATCH: usize>(
        &self,
        frames: &mut [Option<
            PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        >; BATCH],
    ) -> bool {
        assert!(
            BATCH <= PINNED_TX_MATERIALIZATION_BATCH_CAPACITY,
            "Core1 materialization batch exceeds one BA32 window"
        );
        if !self.core1_materializer_selected() {
            return false;
        }
        let count = frames.iter().flatten().count();
        if count == 0 {
            return false;
        }
        assert!(
            frames.iter().flatten().all(|frame| {
                *frame.tag() == self.interface && matches!(frame, PinnedNetworkTxFrame::Staged(_))
            }),
            "Core1 materialization accepts one staged logical-interface burst"
        );

        let interface_index = usize::from(self.interface.value());
        let interface_bit = 1_u32 << self.interface.value();
        if self
            .physical
            .materialization_in_flight
            .load(Ordering::Acquire)
            & interface_bit
            == 0
            && let Ok(stale) =
                self.physical.materialization_completions[interface_index].try_receive()
        {
            assert!(
                stale.cancelled,
                "an unclaimed successful materialization cannot cross a role epoch"
            );
        }
        if self
            .physical
            .materialization_in_flight
            .fetch_or(interface_bit, Ordering::AcqRel)
            & interface_bit
            != 0
        {
            return false;
        }
        self.physical
            .materialization_cancel
            .fetch_and(!interface_bit, Ordering::AcqRel);

        let mut destinations = [None; PINNED_TX_MATERIALIZATION_BATCH_CAPACITY];
        for destination in destinations.iter_mut().take(count) {
            let Ok(index) = self.physical.free_tx_claim.try_receive() else {
                for index in destinations.iter_mut().filter_map(Option::take) {
                    if let Err(TrySendError::Full(_)) = self.physical.free_tx.try_send(index) {
                        unreachable!("failed Core1 reservation returns its unique DMA credit");
                    }
                }
                self.physical
                    .materialization_in_flight
                    .fetch_and(!interface_bit, Ordering::AcqRel);
                TX_CORE1_MATERIALIZER_COUNTERS
                    .no_credit
                    .fetch_add(1, Ordering::Relaxed);
                return false;
            };
            *destination = Some(index);
        }

        let mut request = TxMaterializationRequest::empty(self.interface);
        request.count = u8::try_from(count).expect("BA32 materialization count fits u8");
        let mut next = 0;
        for slot in frames.iter_mut() {
            let Some(frame) = slot.take() else {
                continue;
            };
            let source = match frame {
                PinnedNetworkTxFrame::Staged(source) => source.handoff_to_materializer(),
                PinnedNetworkTxFrame::Direct(_) => unreachable!("validated staged batch"),
            };
            request.pairs[next] = TxMaterializationPair {
                source,
                destination: destinations[next]
                    .take()
                    .expect("one DMA destination was reserved per staged source"),
            };
            next += 1;
        }
        debug_assert_eq!(next, count);
        debug_assert!(destinations.iter().all(Option::is_none));
        if let Err(TrySendError::Full(_)) =
            self.physical.materialization_requests[interface_index].try_send(request)
        {
            unreachable!("the in-flight bit reserves the sole materialization request slot");
        }
        TX_CORE1_MATERIALIZER_COUNTERS
            .submitted_batches
            .fetch_add(1, Ordering::Relaxed);
        self.physical.materialization_wakers.wake(self.interface);
        true
    }

    /// Reclaim the unique ready owners produced by Core1 without waiting.
    #[cfg(feature = "tx-core1-materializer-probe")]
    pub fn poll_core1_materialization<const BATCH: usize>(
        &self,
        frames: &mut [Option<
            PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        >; BATCH],
    ) -> PinnedTxCore1MaterializationPoll {
        assert!(
            BATCH <= PINNED_TX_MATERIALIZATION_BATCH_CAPACITY,
            "Core1 completion batch exceeds one BA32 window"
        );
        assert!(
            frames.iter().all(Option::is_none),
            "Core1 completion requires an empty destination array"
        );
        let interface_index = usize::from(self.interface.value());
        let Ok(completion) =
            self.physical.materialization_completions[interface_index].try_receive()
        else {
            return PinnedTxCore1MaterializationPoll::Pending;
        };
        assert_eq!(
            completion.interface, self.interface,
            "materialization completion crossed its logical interface"
        );
        let interface_bit = 1_u32 << self.interface.value();
        self.physical
            .materialization_in_flight
            .fetch_and(!interface_bit, Ordering::AcqRel);
        self.physical
            .materialization_cancel
            .fetch_and(!interface_bit, Ordering::AcqRel);
        if completion.cancelled {
            return PinnedTxCore1MaterializationPoll::Cancelled;
        }
        let count = usize::from(completion.count);
        assert!(count <= BATCH, "completion fits caller-owned burst array");
        for (slot, index) in frames.iter_mut().zip(completion.destinations).take(count) {
            *slot = Some(PinnedNetworkTxFrame::Direct(
                self.physical.claim(self.interface, index),
            ));
        }
        PinnedTxCore1MaterializationPoll::Ready(count)
    }

    /// Request terminal cancellation without stealing a pending Core1 request.
    ///
    /// If Core1 already published success, this method consumes that sole
    /// completion and reclaims its ready DMA owners synchronously. Otherwise
    /// the worker observes the cancel bit before or immediately after
    /// publication and returns every source/destination credit itself.
    #[cfg(feature = "tx-core1-materializer-probe")]
    pub fn cancel_core1_materialization(&self) -> bool {
        let interface_bit = 1_u32 << self.interface.value();
        if self
            .physical
            .materialization_in_flight
            .load(Ordering::Acquire)
            & interface_bit
            == 0
        {
            return false;
        }
        self.physical
            .materialization_cancel
            .fetch_or(interface_bit, Ordering::AcqRel);
        let interface_index = usize::from(self.interface.value());
        if let Ok(completion) =
            self.physical.materialization_completions[interface_index].try_receive()
        {
            if !completion.cancelled {
                let count = usize::from(completion.count);
                for index in completion.destinations[..count].iter().copied() {
                    let destination = self.physical.claim(self.interface, index);
                    drop(destination);
                }
            }
            self.physical
                .materialization_cancel
                .fetch_and(!interface_bit, Ordering::AcqRel);
            self.physical
                .materialization_in_flight
                .fetch_and(!interface_bit, Ordering::AcqRel);
            if !completion.cancelled {
                TX_CORE1_MATERIALIZER_COUNTERS
                    .cancelled_batches
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.physical.materialization_wakers.wake(self.interface);
        true
    }

    /// Promote one scheduled CPU packet into its final DMA-visible slot.
    ///
    /// `Err(frame)` means no physical DMA credit is currently free. The
    /// caller retains the exact packet owner and may service completion before
    /// retrying; no copy has occurred in that case.
    pub fn try_promote(
        &self,
        frame: PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    > {
        match frame {
            PinnedNetworkTxFrame::Direct(frame) => Ok(frame),
            #[cfg(feature = "tx-staging-copy-probe")]
            PinnedNetworkTxFrame::Staged(frame) => {
                #[cfg(feature = "tx-phase-telemetry")]
                let promotion_started = TxPerformanceSample::read();
                let Ok(index) = self.physical.free_tx_claim.try_receive() else {
                    #[cfg(feature = "tx-phase-telemetry")]
                    TX_PERFORMANCE
                        .record_promotion_no_credit(promotion_started, TxPerformanceSample::read());
                    return Err(PinnedNetworkTxFrame::Staged(frame));
                };
                #[cfg(feature = "tx-phase-telemetry")]
                let credit_acquired = TxPerformanceSample::read();
                let tag = frame.owner_tag();
                let metadata = self.physical.staged_metadata[usize::from(tag.pool_index())]
                    .read(tag.interface());
                let length = frame.as_slice().len();
                let lease = self.physical.tx_pool.claim_network(index);
                #[cfg(feature = "tx-phase-telemetry")]
                let destination_claimed = TxPerformanceSample::read();
                #[cfg(feature = "tx-phase-telemetry")]
                let publication_started = TxPerformanceSample::read();
                #[cfg(feature = "tx-phase-telemetry")]
                let mut copy = TxPerformanceSample::default();
                let (index, ()) = lease.publish(length, |dma| {
                    #[cfg(feature = "tx-phase-telemetry")]
                    let copy_started = TxPerformanceSample::read();
                    dma.copy_from_slice(frame.as_slice());
                    #[cfg(feature = "tx-phase-telemetry")]
                    {
                        copy = TxPerformanceSample::read().wrapping_delta_since(copy_started);
                    }
                });
                #[cfg(feature = "tx-phase-telemetry")]
                let published = TxPerformanceSample::read();
                frame.release();
                #[cfg(feature = "tx-phase-telemetry")]
                let source_released = TxPerformanceSample::read();
                let promoted = self.physical.claim_with_metadata(metadata, index);
                #[cfg(feature = "tx-phase-telemetry")]
                TX_PERFORMANCE.record_promotion(
                    length,
                    promotion_started,
                    credit_acquired,
                    destination_claimed,
                    copy,
                    publication_started,
                    published,
                    source_released,
                    TxPerformanceSample::read(),
                );
                Ok(promoted)
            }
        }
    }

    /// Promote one bounded packet burst without partial ownership movement.
    ///
    /// Every occupied entry must belong to this interface. The method first
    /// reserves all physical DMA credits required by staged entries. If that
    /// reservation cannot be completed, it returns `false`, restores every
    /// reserved credit and leaves `frames` byte-for-byte and owner-for-owner
    /// unchanged. On success every occupied entry is direct DMA backing.
    /// Staged producer credits are returned together and their waiter is
    /// woken once for the complete burst rather than once per packet.
    #[cfg(feature = "tx-staging-copy-probe")]
    pub fn try_promote_batch<const BATCH: usize>(
        &self,
        frames: &mut [Option<
            PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        >; BATCH],
    ) -> bool {
        let staged_count = frames
            .iter()
            .flatten()
            .filter(|frame| matches!(frame, PinnedNetworkTxFrame::Staged(_)))
            .count();
        if staged_count == 0 {
            return true;
        }

        let mut reserved = [None; BATCH];
        for slot in reserved.iter_mut().take(staged_count) {
            let Ok(index) = self.physical.free_tx_claim.try_receive() else {
                for index in reserved.iter_mut().filter_map(Option::take) {
                    if let Err(TrySendError::Full(_)) = self.physical.free_tx.try_send(index) {
                        unreachable!("failed batch reservation returns its physical TX credit");
                    }
                }
                #[cfg(feature = "tx-phase-telemetry")]
                {
                    let now = TxPerformanceSample::read();
                    TX_PERFORMANCE.record_promotion_no_credit(now, now);
                }
                return false;
            };
            *slot = Some(index);
        }

        let mut next_reserved = 0;
        for slot in frames.iter_mut() {
            let Some(frame) = slot.as_ref() else {
                continue;
            };
            assert_eq!(
                *frame.tag(),
                self.interface,
                "batch promotion cannot cross an interface boundary"
            );
            if matches!(frame, PinnedNetworkTxFrame::Direct(_)) {
                continue;
            }

            let source = match slot.take().expect("checked occupied batch entry") {
                PinnedNetworkTxFrame::Staged(source) => source,
                PinnedNetworkTxFrame::Direct(_) => unreachable!("direct entry was skipped"),
            };
            let index = reserved[next_reserved]
                .take()
                .expect("one destination credit was reserved per staged source");
            next_reserved += 1;

            #[cfg(feature = "tx-phase-telemetry")]
            let promotion_started = TxPerformanceSample::read();
            #[cfg(feature = "tx-phase-telemetry")]
            let credit_acquired = promotion_started;
            let tag = source.owner_tag();
            let metadata =
                self.physical.staged_metadata[usize::from(tag.pool_index())].read(tag.interface());
            let length = source.as_slice().len();
            let lease = self.physical.tx_pool.claim_network(index);
            #[cfg(feature = "tx-phase-telemetry")]
            let destination_claimed = TxPerformanceSample::read();
            #[cfg(feature = "tx-phase-telemetry")]
            let publication_started = TxPerformanceSample::read();
            #[cfg(feature = "tx-phase-telemetry")]
            let mut copy = TxPerformanceSample::default();
            let (index, ()) = lease.publish(length, |dma| {
                #[cfg(feature = "tx-phase-telemetry")]
                let copy_started = TxPerformanceSample::read();
                dma.copy_from_slice(source.as_slice());
                #[cfg(feature = "tx-phase-telemetry")]
                {
                    copy = TxPerformanceSample::read().wrapping_delta_since(copy_started);
                }
            });
            #[cfg(feature = "tx-phase-telemetry")]
            let published = TxPerformanceSample::read();
            let source_index = source.release_index();
            if let Err(TrySendError::Full(_)) = self.physical.free_staged.try_send(source_index) {
                unreachable!("batch promotion returns every unique staged TX credit");
            }
            // Preserve producer/copy overlap without returning to one wake
            // per frame. The first returned index changes the staged pool
            // from empty to ready and lets Core1 refill software backlog
            // while Core0 copies the rest of this burst. If Core1 drains the
            // pool concurrently, a later return creates a new real edge and
            // legitimately wakes it again.
            if self.physical.free_staged.len() == 1 {
                self.physical.tx_credit_wakers.wake_waiter_after(
                    self.interface,
                    self.physical.tx_active,
                    self.physical.tx_credit_waiters,
                );
            }
            #[cfg(feature = "tx-phase-telemetry")]
            let source_released = TxPerformanceSample::read();
            let promoted = self.physical.claim_with_metadata(metadata, index);
            *slot = Some(PinnedNetworkTxFrame::Direct(promoted));
            #[cfg(feature = "tx-phase-telemetry")]
            TX_PERFORMANCE.record_promotion(
                length,
                promotion_started,
                credit_acquired,
                destination_claimed,
                copy,
                publication_started,
                published,
                source_released,
                TxPerformanceSample::read(),
            );
        }
        debug_assert!(reserved.iter().all(Option::is_none));
        true
    }

    /// Direct-only builds already satisfy the batch postcondition without an
    /// ownership transition.
    #[cfg(not(feature = "tx-staging-copy-probe"))]
    pub fn try_promote_batch<const BATCH: usize>(
        &self,
        frames: &mut [Option<
            PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        >; BATCH],
    ) -> bool {
        frames.iter().flatten().all(|frame| {
            *frame.tag() == self.interface && matches!(frame, PinnedNetworkTxFrame::Direct(_))
        })
    }
}

/// Explicit single-endpoint radio composition.
///
/// Resource ownership remains split: the RX runner belongs to one permanent
/// endpoint while the TX consumer belongs to the physical fabric. This
/// convenience owner is useful for single-VIF schedulers and can be replaced
/// by a multi-endpoint scheduler without recreating either resource.
pub struct PinnedNetworkRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    interface: NetworkInterfaceId,
    rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
}

/// One physical radio-side owner for two permanent logical network endpoints.
///
/// RX and link-state publication are addressed by logical interface. TX is
/// never duplicated or filtered: both network devices publish tagged leases
/// into the single consumer retained by this value, and the Wi-Fi scheduler
/// must dispatch every tag to its matching role encoder.
pub struct DualPinnedNetworkRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    first_interface: NetworkInterfaceId,
    first_rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    second_interface: NetworkInterfaceId,
    second_rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    DualPinnedNetworkRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    pub fn new(
        first_interface: NetworkInterfaceId,
        first_rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        second_interface: NetworkInterfaceId,
        second_rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) -> Self {
        assert_ne!(
            first_interface, second_interface,
            "dual network endpoints require distinct interface identities"
        );
        Self {
            first_interface,
            first_rx,
            second_interface,
            second_rx,
            tx,
        }
    }

    pub const fn first_interface(&self) -> NetworkInterfaceId {
        self.first_interface
    }

    pub const fn second_interface(&self) -> NetworkInterfaceId {
        self.second_interface
    }

    fn rx_for(
        &self,
        interface: NetworkInterfaceId,
    ) -> &SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        if interface == self.first_interface {
            &self.first_rx
        } else if interface == self.second_interface {
            &self.second_rx
        } else {
            panic!("network interface does not belong to this radio owner")
        }
    }

    pub fn with_shared_rx_ordering<
        const FIRST_SHARED_CAPACITY: usize,
        const FIRST_SHARED_SLOTS: usize,
        const SECOND_SHARED_CAPACITY: usize,
        const SECOND_SHARED_SLOTS: usize,
    >(
        self,
        first: &SharedPinnedRxConsumer<'resources, M, FIRST_SHARED_CAPACITY, FIRST_SHARED_SLOTS>,
        second: &SharedPinnedRxConsumer<'resources, M, SECOND_SHARED_CAPACITY, SECOND_SHARED_SLOTS>,
    ) -> Self {
        Self {
            first_rx: self.first_rx.with_shared_rx_ordering(first),
            second_rx: self.second_rx.with_shared_rx_ordering(second),
            ..self
        }
    }

    pub fn rx_publisher(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        self.rx_for(interface).rx_publisher()
    }

    pub fn link_controller(
        &self,
    ) -> PinnedNetworkLinkController<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        PinnedNetworkLinkController {
            first: self.first_rx.link_endpoint(),
            second: Some(self.second_rx.link_endpoint()),
            tx: self.tx,
        }
    }

    pub fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.rx_for(interface).set_link_state(state);
        self.tx.discard_inactive_ready();
    }

    pub fn try_receive_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        self.tx.try_receive()
    }

    pub async fn receive_tx(
        &self,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        self.tx.receive().await
    }

    pub fn tx_consumer(
        &self,
    ) -> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> {
        self.tx
    }

    pub async fn wait_tx_publication(&self) {
        self.tx.wait_publication().await;
    }

    pub async fn wait_tx_ready(&self) {
        self.tx.wait_ready().await;
    }

    pub fn tx_queue_len(&self) -> usize {
        self.tx.queue_len()
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    PinnedNetworkRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    pub const fn new(
        interface: NetworkInterfaceId,
        rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) -> Self {
        Self { interface, rx, tx }
    }

    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    pub fn into_parts(
        self,
    ) -> (
        SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) {
        (self.rx, self.tx)
    }

    pub fn with_shared_rx_ordering<const SHARED_CAPACITY: usize, const SHARED_SLOTS: usize>(
        self,
        shared: &SharedPinnedRxConsumer<'resources, M, SHARED_CAPACITY, SHARED_SLOTS>,
    ) -> Self {
        Self {
            interface: self.interface,
            rx: self.rx.with_shared_rx_ordering(shared),
            tx: self.tx,
        }
    }

    pub fn rx_publisher(&self) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        self.rx.rx_publisher()
    }

    pub fn link_controller(
        &self,
    ) -> PinnedNetworkLinkController<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        PinnedNetworkLinkController {
            first: self.rx.link_endpoint(),
            second: None,
            tx: self.tx,
        }
    }

    pub fn set_link_state(&self, state: LinkState) {
        self.rx.set_link_state(state);
        self.tx.discard_inactive_ready();
    }

    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.rx.try_send_rx(frame)
    }

    pub async fn send_rx(&self, frame: &[u8]) -> Result<(), FrameLengthError> {
        self.rx.send_rx(frame).await
    }

    pub fn try_receive_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        self.tx_consumer().try_receive()
    }

    pub fn try_receive_tx_direct(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        self.tx_consumer().try_receive_direct()
    }

    pub async fn receive_tx(
        &self,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        self.tx_consumer().receive().await
    }

    pub const fn tx_consumer(
        &self,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        self.tx.for_interface(self.interface)
    }

    pub async fn wait_tx_publication(&self) {
        self.tx.wait_publication().await;
    }

    pub async fn wait_tx_ready(&self) {
        self.tx_consumer().wait_ready().await;
    }

    pub fn rx_queue_len(&self) -> usize {
        self.rx.rx_queue_len()
    }

    pub fn tx_queue_len(&self) -> usize {
        self.tx_consumer().queue_len()
    }
}

/// Queue-return capability paired with a lower-level pinned DMA lease.
#[doc(hidden)]
pub struct PinnedTxReturn<'resources, M: RawMutex, const QUEUE_DEPTH: usize> {
    free_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    interface: NetworkInterfaceId,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    control_tx_index: &'resources AtomicUsize,
    control_tx_available: &'resources AtomicBool,
    control_tx_waiters: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
}

impl<M: RawMutex, const QUEUE_DEPTH: usize> PinnedTxReturn<'_, M, QUEUE_DEPTH> {
    fn is_control_index(&self, index: u8) -> bool {
        self.control_tx_index.load(Ordering::Acquire) == usize::from(index)
    }

    #[cfg(feature = "tx-phase-telemetry")]
    fn free_count(&self) -> usize {
        self.free_tx.len()
            + usize::from(
                self.control_tx_index.load(Ordering::Acquire) < usize::MAX - 1
                    && self.control_tx_available.load(Ordering::Acquire),
            )
    }

    fn return_network_index(&self, index: u8) -> bool {
        if self.is_control_index(index) {
            assert!(
                !self.control_tx_available.swap(true, Ordering::AcqRel),
                "control TX credit cannot be returned twice"
            );
            let waiting = self.control_tx_waiters.load(Ordering::Acquire)
                & self.tx_active.load(Ordering::Acquire);
            self.tx_credit_wakers.wake_mask(waiting);
            return waiting != 0;
        }
        if let Err(TrySendError::Full(_)) = self.free_tx.try_send(index) {
            unreachable!("network owner returns its unique pinned TX index");
        }
        let woke_network = self.free_tx.len() == 1;
        if woke_network {
            self.tx_credit_wakers.wake_waiter_after(
                self.interface,
                self.tx_active,
                self.tx_credit_waiters,
            );
        }
        woke_network
    }
}

impl<M: RawMutex, const QUEUE_DEPTH: usize> DmaIndexReturn for PinnedTxReturn<'_, M, QUEUE_DEPTH> {
    fn return_index(&self, index: u8) {
        // A terminal A-MPDU releases its retained leases synchronously. The
        // first returned index changes the physical pool from empty to ready;
        // the remaining indices are additional credits, not additional
        // readiness edges. If another core drains the pool concurrently, a
        // later return legitimately creates a new edge and wakes again.
        let woke_network = self.return_network_index(index);
        #[cfg(feature = "tx-phase-telemetry")]
        TX_PERFORMANCE.record_radio_return(woke_network);
        #[cfg(not(feature = "tx-phase-telemetry"))]
        let _ = woke_network;
    }
}

/// Unique radio-side lease for one permanently located TX allocation.
///
/// Dropping the lease first releases DMA ownership and then returns the index
/// to `embassy-net`. Chip-specific MAC code retains this value through final
/// completion, BlockAck processing and any retry.
type PinnedTxBacking<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = ReturningStableDmaBacking<
    PinnedDmaTxRadioLease<'resources, FRAME_CAPACITY, HEADROOM, TRAILER>,
    PinnedTxReturn<'resources, M, QUEUE_DEPTH>,
>;

/// One network-published DMA frame plus the logical endpoint that published
/// it. The tag remains outside the DMA allocation and must be consumed before
/// role-specific encoding begins.
pub type PinnedTxFrame<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = TaggedStableDmaBacking<
    PinnedTxOwnerTag,
    PinnedTxBacking<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
>;
