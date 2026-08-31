//! AP-owned network TX transaction.
//!
//! DATAPATH schedules this owner but does not know peer admission, encoding,
//! aggregate publication, retry, or completion policy.

use super::*;
use crate::datapath::software_tx_queue::{IndexedLeaseArena, RoundRobinTxQueues};
use core::marker::PhantomData;

// Retention is deliberately bounded independently of the physical producer
// pool. Active and standby aggregates own their leases outside this arena;
// growing the DMA frontier must not multiply scheduler metadata per peer.
const AP_POWER_SAVE_FRAME_CAPACITY: usize = 66;

// Core1 publishes one immutable FIFO per VIF, but AP A-MPDU eligibility is a
// peer/TID property. Core0 therefore owns a bounded index arena which can
// regroup leases without moving their payload bytes. The current AP data path
// negotiates only TID 0, so fifteen unicast peers plus one group/invalid
// frontier are sufficient. Extending AP QoS to more TIDs must raise this
// explicit flow bound; it must not add per-peer payload rings.
const AP_ACTIVE_FLOW_CAPACITY: usize = AP_MAX_CLIENTS + 1;
const AP_ACTIVE_FRAME_CAPACITY: usize = AP_POWER_SAVE_FRAME_CAPACITY;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ApTxFlowKey {
    destination: [u8; 6],
    association_epoch: u32,
    association_id: u8,
    tid: u8,
}

impl ApTxFlowKey {
    const INVALID_PEER: [u8; 6] = [0; 6];

    const fn associated(identity: ApAssociationIdentity) -> Self {
        let association_id = identity.association_id();
        assert!(association_id <= u8::MAX as u16);
        Self {
            destination: identity.address(),
            association_epoch: identity.association_epoch(),
            association_id: association_id as u8,
            tid: open_esp_radio_esp32s31_wifi_ap::protocol::AP_TX_BLOCK_ACK_TID,
        }
    }

    fn unbound_from_ethernet(ethernet: &[u8]) -> Self {
        Self {
            destination: ethernet
                .get(..6)
                .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
                .unwrap_or(Self::INVALID_PEER),
            association_epoch: 0,
            association_id: 0,
            tid: open_esp_radio_esp32s31_wifi_ap::protocol::AP_TX_BLOCK_ACK_TID,
        }
    }

    fn association(self) -> Option<ApAssociationIdentity> {
        ApAssociationIdentity::new(
            self.destination,
            u16::from(self.association_id),
            self.association_epoch,
        )
    }

    fn is_current(self, engine: &Esp32s31ApEngine<'_>) -> bool {
        match self.association() {
            Some(identity) => engine.association_is_current(identity),
            None if self.destination[0] & 1 != 0 || self.destination == Self::INVALID_PEER => true,
            None => engine
                .peer_status(self.destination)
                .is_none_or(|status| status.phase != ApPeerPhase::Authorized),
        }
    }
}

/// Single physical owner of every AP-retained network lease which is not yet
/// encoded into an A-MPDU arena or published through ordinary TX.
///
/// Active scheduling and both power-save policies store only indices into
/// this arena. Their capacities therefore describe classifications of the
/// same 66 producer credits, not three independent arrays of `B` handles.
type ApFrameLeaseArena<B> = IndexedLeaseArena<B, AP_ACTIVE_FRAME_CAPACITY>;

/// Intrusive per-flow FIFO links over [`ApFrameLeaseArena`].
type ApActiveFrameQueues =
    RoundRobinTxQueues<ApTxFlowKey, AP_ACTIVE_FLOW_CAPACITY, AP_ACTIVE_FRAME_CAPACITY>;

const fn aggregate_adapter_available(ordinary_publication_pending: bool) -> bool {
    !ordinary_publication_pending
}

struct BufferedUnicast<B> {
    identity: ApAssociationIdentity,
    order: u64,
    frame: B,
}

struct BufferedUnicastRelease<B> {
    buffered: BufferedUnicast<B>,
    release: ApBufferedUnicastRelease,
}

#[derive(Clone, Copy)]
struct BufferedUnicastIndex {
    identity: ApAssociationIdentity,
    order: u64,
    frame_index: u8,
}

struct BufferedGroup<B> {
    order: u64,
    frame: B,
}

struct BufferedGroupRelease<B> {
    buffered: BufferedGroup<B>,
    release: ApBufferedGroupRelease,
}

#[derive(Clone, Copy)]
struct BufferedGroupIndex {
    order: u64,
    frame_index: u8,
}

struct ApPowerSaveFrameQueue {
    slots: [Option<BufferedUnicastIndex>; AP_POWER_SAVE_FRAME_CAPACITY],
    next_order: u64,
    len: usize,
}

impl ApPowerSaveFrameQueue {
    const fn new() -> Self {
        Self {
            slots: [const { None }; AP_POWER_SAVE_FRAME_CAPACITY],
            next_order: 0,
            len: 0,
        }
    }

    fn push<B>(
        &mut self,
        identity: ApAssociationIdentity,
        frame: B,
        arena: &mut ApFrameLeaseArena<B>,
    ) -> Result<usize, B> {
        let Some(index) = self.slots.iter().position(Option::is_none) else {
            return Err(frame);
        };
        let frame_index = arena.insert(frame)?;
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.slots[index] = Some(BufferedUnicastIndex {
            identity,
            order,
            frame_index,
        });
        self.len += 1;
        Ok(index)
    }

    fn take_at<B>(
        &mut self,
        index: usize,
        arena: &mut ApFrameLeaseArena<B>,
    ) -> Option<BufferedUnicast<B>> {
        let buffered = self.slots.get_mut(index)?.take()?;
        self.len -= 1;
        Some(BufferedUnicast {
            identity: buffered.identity,
            order: buffered.order,
            frame: arena.take(buffered.frame_index),
        })
    }

    fn restore<B>(&mut self, buffered: BufferedUnicast<B>, arena: &mut ApFrameLeaseArena<B>) {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a released AP power-save lease always leaves one queue slot");
        let frame_index = arena
            .insert(buffered.frame)
            .unwrap_or_else(|_| panic!("released AP power-save lease returns to its arena slot"));
        *slot = Some(BufferedUnicastIndex {
            identity: buffered.identity,
            order: buffered.order,
            frame_index,
        });
        self.len += 1;
    }

    fn oldest_index_for(&self, identity: ApAssociationIdentity) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let entry = entry.as_ref()?;
                (entry.identity == identity).then_some((index, entry.order))
            })
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
    }

    fn oldest_releasable_peer(
        &self,
        mut releasable: impl FnMut(ApAssociationIdentity) -> bool,
    ) -> Option<ApAssociationIdentity> {
        self.slots
            .iter()
            .flatten()
            .filter(|entry| releasable(entry.identity))
            .min_by_key(|entry| entry.order)
            .map(|entry| entry.identity)
    }

    fn retain<B>(
        &mut self,
        arena: &mut ApFrameLeaseArena<B>,
        mut keep: impl FnMut(ApAssociationIdentity) -> bool,
    ) {
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|entry| !keep(entry.identity)) {
                let removed = slot.take().expect("checked AP power-save entry");
                drop(arena.take(removed.frame_index));
                self.len -= 1;
            }
        }
    }
}

/// Bounded caller-owned group queue. Entries are pinned network leases, not
/// payload copies; the portable AP owns only the matching advertised count.
struct ApGroupFrameQueue {
    slots: [Option<BufferedGroupIndex>; AP_POWER_SAVE_FRAME_CAPACITY],
    next_order: u64,
    len: usize,
}

impl ApGroupFrameQueue {
    const fn new() -> Self {
        Self {
            slots: [const { None }; AP_POWER_SAVE_FRAME_CAPACITY],
            next_order: 0,
            len: 0,
        }
    }

    fn push<B>(&mut self, frame: B, arena: &mut ApFrameLeaseArena<B>) -> Result<usize, B> {
        let Some(index) = self.slots.iter().position(Option::is_none) else {
            return Err(frame);
        };
        let frame_index = arena.insert(frame)?;
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.slots[index] = Some(BufferedGroupIndex { order, frame_index });
        self.len += 1;
        Ok(index)
    }

    fn take_at<B>(
        &mut self,
        index: usize,
        arena: &mut ApFrameLeaseArena<B>,
    ) -> Option<BufferedGroup<B>> {
        let buffered = self.slots.get_mut(index)?.take()?;
        self.len -= 1;
        Some(BufferedGroup {
            order: buffered.order,
            frame: arena.take(buffered.frame_index),
        })
    }

    fn restore<B>(&mut self, buffered: BufferedGroup<B>, arena: &mut ApFrameLeaseArena<B>) {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a released AP group lease always leaves one queue slot");
        let frame_index = arena
            .insert(buffered.frame)
            .unwrap_or_else(|_| panic!("released AP group lease returns to its arena slot"));
        *slot = Some(BufferedGroupIndex {
            order: buffered.order,
            frame_index,
        });
        self.len += 1;
    }

    fn oldest_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.as_ref().map(|entry| (index, entry.order)))
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
    }

    fn clear<B>(&mut self, arena: &mut ApFrameLeaseArena<B>) -> usize {
        let discarded = self.len;
        for slot in &mut self.slots {
            if let Some(removed) = slot.take() {
                drop(arena.take(removed.frame_index));
            }
        }
        self.len = 0;
        discarded
    }
}

struct PreparedStandby {
    admission: Esp32s31ApAggregateAdmission,
    policy: HtAmpduTxRolePolicy,
    admitted: usize,
    #[cfg(feature = "tx-phase-telemetry")]
    mismatch_claims: usize,
    #[cfg(any(feature = "diagnostics", test))]
    preparation_micros: u64,
}

pub struct Esp32s31AccessPointNetworkTx<'observer, B, N = B> {
    dma_backing: PhantomData<B>,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'observer dyn AggregateTxObserver>,
    #[cfg(not(any(feature = "diagnostics", test)))]
    observer_lifetime: PhantomData<&'observer ()>,
    deadline_micros: Option<u64>,
    #[cfg(any(feature = "diagnostics", test))]
    exchange_started_micros: Option<u64>,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_acknowledged: Option<u8>,
    frame_arena: ApFrameLeaseArena<N>,
    active_frames: ApActiveFrameQueues,
    prepared_first: Option<N>,
    prepared_first_key: Option<ApTxFlowKey>,
    prepared_second: Option<N>,
    prepared_second_key: Option<ApTxFlowKey>,
    prepared_standby: Option<PreparedStandby>,
    #[cfg(feature = "tx-core1-materializer-probe")]
    core1_materialization_in_flight: bool,
    buffered_unicast: ApPowerSaveFrameQueue,
    buffered_group: ApGroupFrameQueue,
    prepared_buffered_release: Option<BufferedUnicastRelease<N>>,
    active_buffered_release: Option<BufferedUnicastRelease<N>>,
    prepared_group_release: Option<BufferedGroupRelease<N>>,
    active_group_release: Option<BufferedGroupRelease<N>>,
    /// Remaining prefix authorized by one successful DTIM beacon. Frames
    /// retained after that beacon can never join this release window.
    dtim_group_release_remaining: u16,
    last_started_frames: usize,
}

impl<'observer, B, N> Esp32s31AccessPointNetworkTx<'observer, B, N>
where
    B: StableDmaBacking,
{
    pub const fn new(
        #[cfg(any(feature = "diagnostics", test))] observer: Option<
            &'observer dyn AggregateTxObserver,
        >,
    ) -> Self {
        Self {
            dma_backing: PhantomData,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(not(any(feature = "diagnostics", test)))]
            observer_lifetime: PhantomData,
            deadline_micros: None,
            #[cfg(any(feature = "diagnostics", test))]
            exchange_started_micros: None,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_acknowledged: None,
            frame_arena: ApFrameLeaseArena::new(),
            active_frames: ApActiveFrameQueues::new(),
            prepared_first: None,
            prepared_first_key: None,
            prepared_second: None,
            prepared_second_key: None,
            prepared_standby: None,
            #[cfg(feature = "tx-core1-materializer-probe")]
            core1_materialization_in_flight: false,
            buffered_unicast: ApPowerSaveFrameQueue::new(),
            buffered_group: ApGroupFrameQueue::new(),
            prepared_buffered_release: None,
            active_buffered_release: None,
            prepared_group_release: None,
            active_group_release: None,
            dtim_group_release_remaining: 0,
            last_started_frames: 1,
        }
    }

    pub(super) const fn aggregate_pending(&self) -> bool {
        self.deadline_micros.is_some()
    }

    #[cfg(feature = "tx-phase-telemetry")]
    #[inline(never)]
    fn publish_shadow_grant(&self, identity: ApAssociationIdentity, frame_credits: u8) {
        let grant = super::access_point_egress_shadow_grant();
        let slot = u8::try_from(identity.association_id())
            .ok()
            .and_then(core::num::NonZeroU8::new)
            .expect("an AP association slot is non-zero and byte-sized");
        let generation = core::num::NonZeroU32::new(identity.association_epoch())
            .expect("an AP association epoch is non-zero");
        let frame_credits = core::num::NonZeroU8::new(frame_credits)
            .expect("an AP aggregate policy has a non-zero frame limit");
        grant
            .publish(
                EgressShadowGrantKey::new(
                    crate::roles::concurrent::AP_NETWORK_INTERFACE_ID.value(),
                    slot,
                    generation,
                    open_esp_radio_esp32s31_wifi_ap::protocol::AP_TX_BLOCK_ACK_TID,
                ),
                frame_credits,
            )
            .expect("the Core0 shadow-grant publication is single-owner and non-reusable");
    }

    #[cfg(feature = "tx-phase-telemetry")]
    #[inline(never)]
    fn clear_shadow_grant(&self) {
        super::access_point_egress_shadow_grant()
            .clear()
            .expect("the Core0 shadow-grant publication is single-owner and non-reusable");
    }

    pub(super) fn has_prepared(&self) -> bool {
        self.active_frames.len() != 0
            || self.prepared_first.is_some()
            || self.prepared_second.is_some()
            || self.prepared_standby.is_some()
            || self.prepared_buffered_release.is_some()
            || self.prepared_group_release.is_some()
    }

    pub(super) fn prepared_start_ready(&self) -> bool {
        #[cfg(feature = "tx-core1-materializer-probe")]
        if self.core1_materialization_in_flight {
            return false;
        }
        self.has_prepared()
    }

    pub(super) fn prepared_frame_count(&self) -> usize {
        if self.prepared_group_release.is_some() || self.prepared_buffered_release.is_some() {
            return 1;
        }
        if let Some(batch) = self.prepared_standby.as_ref() {
            return batch.admitted;
        }
        if let Some(first) = self.prepared_first.as_ref() {
            let _ = first;
            return 1
                + self.active_frames.len_for(
                    self.prepared_first_key
                        .expect("prepared AP frame retains its flow key"),
                )
                + usize::from(self.prepared_second.is_some());
        }
        self.active_frames.scheduled_len()
    }

    pub(super) const fn last_started_frame_count(&self) -> usize {
        self.last_started_frames
    }

    /// Publish the terminal aggregate observation at the outer role-service
    /// boundary, after role diagnostics have consumed the completed state.
    /// This keeps completion-to-publication focused on DATAPATH scheduling
    /// instead of charging unrelated observer bookkeeping to the scheduler.
    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn observe_service_boundary(&mut self) {
        let Some(acknowledged) = self.terminal_acknowledged.take() else {
            return;
        };
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::Completed {
                acknowledged,
                individual_retry: false,
            });
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn mark_prepared_scheduler_phase(
        &mut self,
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    ) {
        if !self.has_prepared() {
            return;
        }
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::PreparedSchedulerPhase { phase, at_micros });
        }
    }

    pub(super) fn can_prepare<const SLOTS: usize, const BUFFER_SIZE: usize>(
        &self,
        aggregate: &Esp32s31AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        ordinary_publication_pending: bool,
    ) -> bool {
        // A retained peer-boundary frame may coexist with an ordinary MPDU
        // publication, but it cannot authorize claiming the next frame yet.
        // Encoding that pair borrows the ordinary descriptor policy adapter;
        // the adapter remains owned by the in-flight MPDU until its terminal
        // service edge. Aggregate-active preparation is still allowed because
        // an A-MPDU does not set the ordinary MAC publication owner.
        if !aggregate_adapter_available(ordinary_publication_pending) {
            return false;
        }
        if !aggregate.has_standby() {
            return false;
        }
        if self.prepared_buffered_release.is_some() {
            return false;
        }
        if self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
            || self.dtim_group_release_remaining != 0
        {
            return false;
        }
        match self.prepared_standby.as_ref() {
            Some(batch) => {
                self.prepared_first.is_none()
                    && batch.admitted < usize::from(batch.policy.frame_limit())
            }
            None => {
                (self.deadline_micros.is_some()
                    || self.active_frames.len() != 0
                    || self.prepared_first.is_some())
                    && self.prepared_second.is_none()
            }
        }
    }
}

#[cfg(not(any(feature = "diagnostics", test)))]
impl<'observer, B> Default for Esp32s31AccessPointNetworkTx<'observer, B, B>
where
    B: StableDmaBacking,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
    'observer,
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
>
    Esp32s31AccessPointNetworkTx<
        'observer,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    >
where
    M: RawMutex,
{
    pub(super) fn advance_prepared<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        self.prepare_ready_standby(aggregate, control, network)?;
        #[cfg(feature = "tx-phase-telemetry")]
        self.record_partial_frontier(network);
        Ok(())
    }

    #[cfg(feature = "tx-phase-telemetry")]
    fn record_partial_frontier(
        &self,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) {
        let Some(batch) = self.prepared_standby.as_ref() else {
            return;
        };
        if batch.admitted >= usize::from(batch.policy.frame_limit()) {
            return;
        }
        let key = ApTxFlowKey::associated(batch.admission.association());
        let matching_retained = self.active_frames.len_for(key)
            + usize::from(self.prepared_first_key == Some(key))
            + usize::from(self.prepared_second_key == Some(key));
        let retained = self.active_frames.len()
            + usize::from(self.prepared_first.is_some())
            + usize::from(self.prepared_second.is_some());
        CORE0_PERFORMANCE.record_ap_partial_frontier(
            matching_retained,
            retained.saturating_sub(matching_retained),
            network.queue_len(),
            batch.mismatch_claims,
        );
        let ownership = network.ownership_snapshot();
        let radio_owned = ownership.radio_owned(TX_QUEUE_DEPTH);
        CORE0_PERFORMANCE.record_ap_partial_publication(
            batch.admitted,
            ownership.free,
            ownership.ready_for_interface,
            ownership.ready_for_other_interfaces,
            ownership.ingress_reserved,
            ownership.application_reserved,
            ownership.tokens_in_flight,
            radio_owned,
            radio_owned.saturating_sub(batch.admitted.saturating_add(retained)),
        );
    }

    fn push_active_frame(
        &mut self,
        key: ApTxFlowKey,
        frame: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<
        (),
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        let frame_index = self.frame_arena.insert(frame)?;
        if self.active_frames.push_back(key, frame_index).is_err() {
            return Err(self.frame_arena.take(frame_index));
        }
        Ok(())
    }

    fn push_active_frame_front(
        &mut self,
        key: ApTxFlowKey,
        frame: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<
        (),
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        let frame_index = self.frame_arena.insert(frame)?;
        if self.active_frames.push_front(key, frame_index).is_err() {
            return Err(self.frame_arena.take(frame_index));
        }
        Ok(())
    }

    fn restore_active_pair_front(
        &mut self,
        key: ApTxFlowKey,
        first: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
        second: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) {
        // `push_front` reverses insertion order. Restore the younger frame
        // first so the next scheduler turn observes the original prefix.
        self.restore_active_frame_front(key, second);
        self.restore_active_frame_front(key, first);
    }

    fn restore_active_frame_front(
        &mut self,
        key: ApTxFlowKey,
        frame: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) {
        self.push_active_frame_front(key, frame)
            .unwrap_or_else(|_| panic!("AP rollback lost its bounded arena credit"));
    }

    fn pop_active_key(
        &mut self,
        key: ApTxFlowKey,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        self.active_frames
            .pop_key(key)
            .map(|index| self.frame_arena.take(index))
    }

    fn pop_scheduled_active(
        &mut self,
        engine: &Esp32s31ApEngine<'_>,
    ) -> Option<(
        ApTxFlowKey,
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    )> {
        loop {
            let (key, index) = self.active_frames.pop_scheduled()?;
            let frame = self.frame_arena.take(index);
            if key.is_current(engine) {
                return Some((key, frame));
            }
            drop(frame);
        }
    }

    fn observe_network_claim(
        &self,
        frame: &PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe_access_point_network_claim(frame.as_slice());
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        let _ = frame;
    }

    fn retain_active_frame(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        frame: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError> {
        self.observe_network_claim(&frame);
        let Some((key, frame)) = self.retain_power_save(engine, frame)? else {
            return Ok(());
        };
        // The arena owns every default producer credit which is not already
        // held by active/standby DMA or power-save storage. A custom larger
        // producer cannot turn this bounded Core0 classifier into unbounded
        // retention; excess leases return to that producer immediately.
        let _ = self.push_active_frame(key, frame);
        Ok(())
    }

    fn take_matching_active_or_network(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        key: ApTxFlowKey,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<
        Option<
            PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        >,
        Esp32s31AccessPointDatapathError,
    > {
        if !key.is_current(engine) {
            while let Some(frame) = self.pop_active_key(key) {
                drop(frame);
            }
            return Ok(None);
        }
        if let Some(frame) = self.pop_active_key(key) {
            return Ok(Some(frame));
        }
        while let Some(frame) = network.try_receive() {
            self.observe_network_claim(&frame);
            let Some((frame_key, frame)) = self.retain_power_save(engine, frame)? else {
                continue;
            };
            if frame_key == key {
                return Ok(Some(frame));
            }
            #[cfg(feature = "tx-phase-telemetry")]
            if self
                .prepared_standby
                .as_ref()
                .is_some_and(|batch| batch.admission.peer() == key.destination)
            {
                let batch = self
                    .prepared_standby
                    .as_mut()
                    .expect("the checked AP standby remains owned");
                batch.mismatch_claims = batch.mismatch_claims.saturating_add(1);
            }
            let _ = self.push_active_frame(frame_key, frame);
        }
        Ok(None)
    }

    fn take_scheduled_active_or_network(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<
        Option<(
            ApTxFlowKey,
            PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        )>,
        Esp32s31AccessPointDatapathError,
    > {
        if let Some((key, frame)) = self.pop_scheduled_active(engine) {
            return Ok(Some((key, frame)));
        }
        let Some(frame) = network.try_receive() else {
            return Ok(None);
        };
        self.observe_network_claim(&frame);
        self.retain_power_save(engine, frame)
    }

    fn retain_power_save(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        frame: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<
        Option<(
            ApTxFlowKey,
            PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        )>,
        Esp32s31AccessPointDatapathError,
    > {
        let unbound_key = ApTxFlowKey::unbound_from_ethernet(frame.as_slice());
        let Some(peer) = frame
            .as_slice()
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            return Ok(Some((unbound_key, frame)));
        };
        if peer[0] & 1 != 0 {
            if engine.group_downlink_disposition() == ApDownlinkDisposition::TransmitNow {
                return Ok(Some((unbound_key, frame)));
            }
            let Ok(index) = self.buffered_group.push(frame, &mut self.frame_arena) else {
                // The caller-owned queue is deliberately bounded. Releasing
                // this excess lease applies backpressure at the producer pool
                // without claiming a TIM entry for payload we did not retain.
                return Ok(None);
            };
            if let Err(error) = engine.commit_buffered_group() {
                let _ = self
                    .buffered_group
                    .take_at(index, &mut self.frame_arena)
                    .expect("the just-inserted AP group lease is still owned");
                return Err(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::from(error),
                ));
            }
            return Ok(None);
        }
        let admission = match engine.admit_downlink(peer) {
            Ok(admission) => admission,
            // Preserve the ordinary admission path for an unknown or
            // unauthorized destination so its existing rejection accounting
            // remains authoritative.
            Err(_) => return Ok(Some((unbound_key, frame))),
        };
        let identity = admission.identity();
        let key = ApTxFlowKey::associated(identity);
        if admission.disposition() == ApDownlinkDisposition::TransmitNow {
            return Ok(Some((key, frame)));
        }

        let Ok(index) = self
            .buffered_unicast
            .push(identity, frame, &mut self.frame_arena)
        else {
            // The bounded queue owns the complete default TX lease frontier.
            // A custom larger producer cannot force an allocation or an
            // unbounded retention path; its excess lease is released here.
            return Ok(None);
        };
        if let Err(error) = engine.commit_buffered_unicast(identity) {
            let _ = self
                .buffered_unicast
                .take_at(index, &mut self.frame_arena)
                .expect("the just-inserted AP power-save lease is still owned");
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::from(error),
            ));
        }
        Ok(None)
    }

    /// Reserve the oldest retained frame whose peer has returned to Active.
    /// This mutates no frame bytes and leaves the TIM count unchanged until
    /// terminal TX resolves the affine release token.
    pub(super) fn stage_awake_buffered_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if self.prepared_buffered_release.is_some() || self.active_buffered_release.is_some() {
            return Ok(false);
        }

        if let Some(release) = control.take_pending_buffered_release() {
            let identity = release.identity();
            if let Some(index) = self.buffered_unicast.oldest_index_for(identity) {
                let buffered = self
                    .buffered_unicast
                    .take_at(index, &mut self.frame_arena)
                    .expect("the PS-Poll release names one retained lease");
                self.prepared_buffered_release = Some(BufferedUnicastRelease { buffered, release });
                return Ok(true);
            }
            control
                .mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false)
                .map_err(Esp32s31AccessPointControlError::from)
                .map_err(Esp32s31AccessPointDatapathError::Control)?;
        }

        // Peer teardown clears the portable counters. Release matching caller
        // leases at the same observation boundary instead of retaining stale
        // addresses into a later association generation.
        self.buffered_unicast
            .retain(&mut self.frame_arena, |identity| {
                control.mac.engine().association_is_current(identity)
            });
        let Some(identity) = self.buffered_unicast.oldest_releasable_peer(|identity| {
            control
                .mac
                .engine()
                .association_status(identity)
                .is_some_and(|status| {
                    status.power_state == ApPeerPowerState::Active
                        && !status.buffered_release_in_flight
                })
        }) else {
            return Ok(false);
        };
        let Some(release) = control
            .mac
            .engine_mut()
            .begin_buffered_unicast_release(identity)
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?
        else {
            return Ok(false);
        };
        let Some(index) = self.buffered_unicast.oldest_index_for(identity) else {
            let _ = control
                .mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false);
            return Ok(false);
        };
        let buffered = self
            .buffered_unicast
            .take_at(index, &mut self.frame_arena)
            .expect("the selected AP power-save lease remains retained");
        self.prepared_buffered_release = Some(BufferedUnicastRelease { buffered, release });
        Ok(true)
    }

    fn rollback_prepared_buffered_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(prepared) = self.prepared_buffered_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_unicast_release(prepared.release, false);
        self.buffered_unicast
            .restore(prepared.buffered, &mut self.frame_arena);
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)
    }

    fn complete_active_buffered_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        delivered: bool,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(active) = self.active_buffered_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_unicast_release(active.release, delivered);
        if !delivered || result.is_err() {
            self.buffered_unicast
                .restore(active.buffered, &mut self.frame_arena);
        }
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        let _ = self.stage_awake_buffered_release(control)?;
        Ok(())
    }

    fn start_prepared_buffered_release<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware,
    {
        let prepared = self
            .prepared_buffered_release
            .take()
            .expect("checked prepared AP power-save release");
        let result = control.start_network_tx_with_more_data(
            hardware,
            prepared.buffered.frame.as_slice(),
            prepared.release.more_data(),
        );
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active_buffered_release = Some(prepared);
                Ok(WifiTxProgress::Pending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.prepared_buffered_release = Some(prepared);
                self.rollback_prepared_buffered_release(control)?;
                Ok(WifiTxProgress::Complete)
            }
            Err(error) => {
                self.prepared_buffered_release = Some(prepared);
                self.rollback_prepared_buffered_release(control)?;
                Err(Esp32s31AccessPointDatapathError::Control(error))
            }
        }
    }

    /// Bind the exact queue prefix announced by a successfully transmitted
    /// DTIM beacon to the oldest caller-owned group lease.
    fn stage_dtim_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if let Some(advertised_frames) = control.take_pending_dtim_group_frames() {
            if self.dtim_group_release_remaining != 0
                || self.prepared_group_release.is_some()
                || self.active_group_release.is_some()
            {
                return Err(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::DtimGroupReleaseAlreadyPending,
                ));
            }
            self.dtim_group_release_remaining = advertised_frames;
        }
        if self.dtim_group_release_remaining == 0
            || self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
        {
            return Ok(false);
        }

        let Some(release) = control
            .mac
            .engine_mut()
            .begin_buffered_group_release()
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?
        else {
            self.dtim_group_release_remaining = 0;
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        };
        let Some(index) = self.buffered_group.oldest_index() else {
            let rollback = control
                .mac
                .engine_mut()
                .complete_buffered_group_release(release, false)
                .map_err(Esp32s31AccessPointControlError::from)
                .map_err(Esp32s31AccessPointDatapathError::Control);
            self.dtim_group_release_remaining = 0;
            rollback?;
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        };
        let buffered = self
            .buffered_group
            .take_at(index, &mut self.frame_arena)
            .expect("the selected AP group lease remains retained");
        self.prepared_group_release = Some(BufferedGroupRelease { buffered, release });
        Ok(true)
    }

    fn rollback_prepared_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(prepared) = self.prepared_group_release.take() else {
            self.dtim_group_release_remaining = 0;
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_group_release(prepared.release, false);
        self.buffered_group
            .restore(prepared.buffered, &mut self.frame_arena);
        self.dtim_group_release_remaining = 0;
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)
    }

    fn complete_active_group_release<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        published: bool,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(active) = self.active_group_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_group_release(active.release, published);
        if !published || result.is_err() {
            self.buffered_group
                .restore(active.buffered, &mut self.frame_arena);
            self.dtim_group_release_remaining = 0;
        } else {
            self.dtim_group_release_remaining = self
                .dtim_group_release_remaining
                .checked_sub(1)
                .ok_or(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
                ))?;
        }
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        if self.dtim_group_release_remaining != 0 {
            let _ = self.stage_dtim_group_release(control)?;
        }
        Ok(())
    }

    fn start_prepared_group_release<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware,
    {
        let prepared = self
            .prepared_group_release
            .take()
            .expect("checked prepared AP DTIM group release");
        let result = control.start_network_tx_with_more_data(
            hardware,
            prepared.buffered.frame.as_slice(),
            prepared.release.more_data(),
        );
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active_group_release = Some(prepared);
                Ok(WifiTxProgress::Pending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.prepared_group_release = Some(prepared);
                self.rollback_prepared_group_release(control)?;
                // The control owner returns Complete without publication when
                // no authorized receiver remains. Drop both the retained
                // leases and their TIM accounting instead of advertising an
                // undeliverable queue forever.
                self.discard_group_buffer(control)?;
                Ok(WifiTxProgress::Complete)
            }
            Err(error) => {
                self.prepared_group_release = Some(prepared);
                self.rollback_prepared_group_release(control)?;
                Err(Esp32s31AccessPointDatapathError::Control(error))
            }
        }
    }

    pub(super) fn discard_group_buffer<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if self.active_group_release.is_some() {
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        }
        self.rollback_prepared_group_release(control)?;
        let _ = control.take_pending_dtim_group_frames();
        let portable = control
            .mac
            .engine_mut()
            .discard_buffered_groups()
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        let retained = self.buffered_group.clear(&mut self.frame_arena);
        self.dtim_group_release_remaining = 0;
        if usize::from(portable) != retained {
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        frame: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        self.last_started_frames = 1;
        let _ = self.stage_dtim_group_release(control)?;
        self.retain_active_frame(control.mac.engine_mut(), frame)?;
        if self.prepared_group_release.is_some() {
            return self.start_prepared_group_release(control, hardware);
        }
        let _ = self.stage_awake_buffered_release(control)?;
        if self.prepared_buffered_release.is_some() {
            return self.start_prepared_buffered_release(control, hardware);
        }
        let Some((flow_key, frame)) = self.pop_scheduled_active(control.mac.engine()) else {
            return Ok(WifiTxProgress::Complete);
        };
        let admission = control.mac.aggregate_admission(frame.as_slice());
        let mut retained_aggregate_second = None;

        // Open APs have no BlockAck owner, so they use bounded ordinary
        // A-MSDUs whenever an ordered partner is available. For WPA2+BA keep
        // saturated bursts on A-MPDU; coalesce the exact two-frame tail only
        // when the negotiated agreement echoed A-MSDU support.
        let same_flow_ready = self.active_frames.len_for(flow_key);
        let producer_ready = network.queue_len();
        if same_flow_ready.saturating_add(producer_ready) != 0
            && (admission.is_none()
                || (same_flow_ready.saturating_add(producer_ready) == 1
                    && admission.is_some_and(Esp32s31ApAggregateAdmission::amsdu)))
            && let Some(second) =
                self.take_matching_active_or_network(control.mac.engine_mut(), flow_key, network)?
        {
            match control.start_network_amsdu_pair(hardware, frame.as_slice(), second.as_slice()) {
                Ok(Some(progress)) => {
                    self.last_started_frames = 2;
                    return Ok(progress);
                }
                Ok(None) => {
                    if admission.is_some() {
                        // The pair may be too large for the ordinary AP
                        // scratch while both individual MPDUs still fit the
                        // retained A-MPDU arena. Preserve the already claimed
                        // second lease for that exact fallback.
                        retained_aggregate_second = Some(second);
                    } else {
                        self.restore_active_frame_front(flow_key, second);
                    }
                }
                Err(error) => {
                    self.restore_active_pair_front(flow_key, frame, second);
                    return Err(Esp32s31AccessPointDatapathError::Control(error));
                }
            }
        }

        if let Some(admission) = admission
            && (retained_aggregate_second.is_some()
                || self.active_frames.len_for(flow_key) != 0
                || network.queue_len() != 0)
        {
            let second = if let Some(second) = retained_aggregate_second.take() {
                second
            } else {
                let Some(second) = self.take_matching_active_or_network(
                    control.mac.engine_mut(),
                    flow_key,
                    network,
                )?
                else {
                    return control
                        .start_network_tx(hardware, frame.as_slice())
                        .map_err(Esp32s31AccessPointDatapathError::Control);
                };
                second
            };
            #[cfg(any(feature = "diagnostics", test))]
            let preparation_started = self.observer.map(AggregateTxObserver::now_micros);
            debug_assert!(admission.accepts_ethernet(second.as_slice()));

            let peer = admission.peer();
            let (engine, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            ordinary
                .require_unprotected_ht_aggregate(admission.rate())
                .map_err(Esp32s31ApAmpduError::Protection)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            let mut frame = match network.try_promote(frame) {
                Ok(frame) => frame,
                Err(frame) => {
                    self.restore_active_pair_front(flow_key, frame, second);
                    return Ok(WifiTxProgress::Complete);
                }
            };
            let mut second = match network.try_promote(second) {
                Ok(second) => second,
                Err(second) => {
                    self.restore_active_pair_front(
                        flow_key,
                        PinnedNetworkTxFrame::Direct(frame),
                        second,
                    );
                    return Ok(WifiTxProgress::Complete);
                }
            };
            let first_offset = frame.ethernet_offset();
            let first_length = frame.ethernet_length();
            let first_encoded = engine
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    frame.storage_mut(),
                    first_offset,
                    first_length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(
                        Esp32s31AccessPointControlError::from(error),
                    )
                })?;
            let policy = admission
                .bind_policy(first_encoded.hardware_key_selector, SLOTS)
                .map_err(Esp32s31ApAmpduError::from)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            let active = aggregate.active_mut();
            active
                .begin(
                    peer,
                    policy.rate(),
                    first_encoded.sequence_number,
                    policy.role().hardware_key_selector,
                )
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            active
                .push(peer, frame, first_encoded)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;

            let second_offset = second.ethernet_offset();
            let second_length = second.ethernet_length();
            let second_encoded = engine
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    second.storage_mut(),
                    second_offset,
                    second_length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(
                        Esp32s31AccessPointControlError::from(error),
                    )
                })?;
            active
                .push(peer, second, second_encoded)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;

            let target = usize::from(policy.frame_limit());
            let mut admitted = 2_usize;
            while admitted < target {
                let Some(next) = self.take_matching_active_or_network(engine, flow_key, network)?
                else {
                    break;
                };
                debug_assert!(admission.accepts_ethernet(next.as_slice()));
                let mut next = match network.try_promote(next) {
                    Ok(next) => next,
                    Err(next) => {
                        self.restore_active_frame_front(flow_key, next);
                        break;
                    }
                };
                let offset = next.ethernet_offset();
                let length = next.ethernet_length();
                let encoded = engine
                    .encode_aggregate_ethernet_in_place(
                        admission.binding(),
                        next.storage_mut(),
                        offset,
                        length,
                    )
                    .map_err(|error| {
                        Esp32s31AccessPointDatapathError::Control(
                            Esp32s31AccessPointControlError::from(error),
                        )
                    })?;
                active
                    .push(peer, next, encoded)
                    .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
                admitted += 1;
            }
            #[cfg(any(feature = "diagnostics", test))]
            let publication_started = self.observer.map(AggregateTxObserver::now_micros);
            active
                .publish(ordinary, hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                let finished = observer.now_micros();
                let started = publication_started.unwrap_or(finished);
                observe_aggregate_rate(observer, policy.rate());
                observer.observe(AggregateTxObservation::Prepared {
                    subframes: u8::try_from(admitted).unwrap_or(u8::MAX),
                    stop: if admitted == target {
                        AggregateBuildStop::FrameLimit
                    } else {
                        AggregateBuildStop::QueueEmpty
                    },
                });
                observer.observe(AggregateTxObservation::PreparationCompleted {
                    micros: started.saturating_sub(preparation_started.unwrap_or(started)),
                });
                observer.observe(AggregateTxObservation::Published {
                    at_micros: started,
                    program_micros: finished.saturating_sub(started),
                });
                self.exchange_started_micros = Some(started);
            }
            let deadline_micros = ordinary
                .now_micros()
                .saturating_add(ordinary.publication_timeout_micros());
            self.deadline_micros = Some(deadline_micros);
            #[cfg(any(feature = "diagnostics", test))]
            control.observe_ht_aggregate(policy.rate());
            self.last_started_frames = admitted;
            self.prepare_ready_standby(aggregate, control, network)?;
            return Ok(WifiTxProgress::Pending);
        }

        control
            .start_network_tx(hardware, frame.as_slice())
            .map_err(Esp32s31AccessPointDatapathError::Control)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        frame: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        assert!(
            (self.aggregate_pending() || self.has_prepared()) && aggregate.has_standby(),
            "DATAPATH must check AP standby ownership before claiming another ordered lease"
        );
        self.retain_active_frame(control.mac.engine_mut(), frame)?;

        // The AP-specific peer, power-save and key checks remain per frame.
        // The Core0 arena, rather than the immutable cross-core FIFO order,
        // now defines the same-peer aggregate frontier.
        self.prepare_ready_standby(aggregate, control, network)?;
        Ok(())
    }

    fn prepare_ready_standby<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if self.prepared_standby.as_ref().is_some_and(|batch| {
            !control
                .mac
                .engine()
                .association_is_current(batch.admission.association())
        }) {
            aggregate
                .standby_mut()
                .expect("prepared batch owns standby arena")
                .cancel_build()
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            let _ = self
                .prepared_standby
                .take()
                .expect("checked stale AP standby batch remains owned");
            #[cfg(feature = "tx-phase-telemetry")]
            self.clear_shadow_grant();
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::StandbyCancelled);
            }
        }
        while self.can_prepare(aggregate, control.tx_pending()) {
            if self.prepared_standby.is_some() {
                if !self.prepare_existing_standby_batch(aggregate, control, network)? {
                    break;
                }
                continue;
            }
            let selected = if self.prepared_first.is_some() {
                let key = self
                    .prepared_first_key
                    .expect("prepared AP frame retains its flow key");
                self.take_matching_active_or_network(control.mac.engine_mut(), key, network)?
                    .map(|frame| (key, frame))
            } else {
                self.take_scheduled_active_or_network(control.mac.engine_mut(), network)?
            };
            let Some((key, frame)) = selected else {
                break;
            };
            if !self.prepare_retained_one(aggregate, control, key, frame, network)? {
                break;
            }
        }
        Ok(())
    }

    fn prepare_existing_standby_batch<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let (admission, remaining) = {
            let batch = self
                .prepared_standby
                .as_ref()
                .expect("caller checked the prepared AP standby batch");
            (
                batch.admission,
                usize::from(batch.policy.frame_limit()).saturating_sub(batch.admitted),
            )
        };
        if remaining == 0 {
            return Ok(false);
        }
        let key = ApTxFlowKey::associated(admission.association());
        let mut frames = [const { None }; SLOTS];
        #[cfg(feature = "tx-core1-materializer-probe")]
        let completed = if self.core1_materialization_in_flight {
            match network.poll_core1_materialization(&mut frames) {
                open_esp_radio_embassy_net::PinnedTxCore1MaterializationPoll::Pending => {
                    return Ok(false);
                }
                open_esp_radio_embassy_net::PinnedTxCore1MaterializationPoll::Cancelled => {
                    self.core1_materialization_in_flight = false;
                    return Ok(false);
                }
                open_esp_radio_embassy_net::PinnedTxCore1MaterializationPoll::Ready(count) => {
                    self.core1_materialization_in_flight = false;
                    Some(count)
                }
            }
        } else {
            None
        };
        #[cfg(not(feature = "tx-core1-materializer-probe"))]
        let completed: Option<usize> = None;

        let count = if let Some(count) = completed {
            count
        } else {
            let burst_limit = remaining.min(SLOTS).min(network.promotion_capacity());
            if burst_limit == 0 {
                return Ok(false);
            }
            let mut count = 0;
            while count < burst_limit {
                let Some(frame) =
                    self.take_matching_active_or_network(control.mac.engine_mut(), key, network)?
                else {
                    break;
                };
                debug_assert!(admission.accepts_ethernet(frame.as_slice()));
                frames[count] = Some(frame);
                count += 1;
            }
            if count == 0 {
                return Ok(false);
            }
            #[cfg(feature = "tx-core1-materializer-probe")]
            if network.core1_materializer_selected() {
                if network.try_submit_core1_materialization(&mut frames) {
                    self.core1_materialization_in_flight = true;
                    return Ok(true);
                }
                for frame in frames[..count].iter_mut().rev().filter_map(Option::take) {
                    self.restore_active_frame_front(key, frame);
                }
                return Ok(false);
            }
            count
        };

        #[cfg(any(feature = "diagnostics", test))]
        let started = self.observer.map(AggregateTxObserver::now_micros);
        if !network.try_promote_batch(&mut frames) {
            for frame in frames[..count].iter_mut().rev().filter_map(Option::take) {
                self.restore_active_frame_front(key, frame);
            }
            return Ok(false);
        }

        let peer = admission.peer();
        for slot in frames[..count].iter_mut() {
            let mut frame = slot
                .take()
                .expect("the selected AP burst retains every packet owner")
                .into_direct()
                .unwrap_or_else(|_| panic!("successful promotion leaves only DMA owners"));
            let offset = frame.ethernet_offset();
            let length = frame.ethernet_length();
            let encoded = control
                .mac
                .engine_mut()
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    frame.storage_mut(),
                    offset,
                    length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(
                        Esp32s31AccessPointControlError::from(error),
                    )
                })?;
            aggregate
                .standby_mut()
                .expect("checked standby arena")
                .push(peer, frame, encoded)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        }
        let frame_limit;
        {
            let batch = self
                .prepared_standby
                .as_mut()
                .expect("checked AP standby batch");
            batch.admitted += count;
            frame_limit = usize::from(batch.policy.frame_limit());
            #[cfg(any(feature = "diagnostics", test))]
            {
                batch.preparation_micros = batch.preparation_micros.saturating_add(
                    self.observer
                        .map(|observer| observer.now_micros().saturating_sub(started.unwrap_or(0)))
                        .unwrap_or(0),
                );
            }
        }
        let _ = frame_limit;
        Ok(true)
    }

    fn prepare_retained_one<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        key: ApTxFlowKey,
        frame: PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        #[cfg(any(feature = "diagnostics", test))]
        let started = self.observer.map(AggregateTxObserver::now_micros);
        if let Some(batch) = self.prepared_standby.as_ref() {
            let admission = batch.admission;
            if key != ApTxFlowKey::associated(admission.association())
                || !admission.accepts_ethernet(frame.as_slice())
            {
                debug_assert!(self.prepared_first.is_none());
                self.prepared_first_key = Some(key);
                self.prepared_first = Some(frame);
                return Ok(true);
            }
            {
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                        error,
                    ))
                })?;
                ordinary
                    .require_unprotected_ht_aggregate(admission.rate())
                    .map_err(Esp32s31ApAmpduError::Protection)
                    .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            }
            let mut frame = match network.try_promote(frame) {
                Ok(frame) => frame,
                Err(frame) => {
                    self.restore_active_frame_front(key, frame);
                    return Ok(false);
                }
            };
            let peer = admission.peer();
            let offset = frame.ethernet_offset();
            let length = frame.ethernet_length();
            let encoded = control
                .mac
                .engine_mut()
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    frame.storage_mut(),
                    offset,
                    length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(
                        Esp32s31AccessPointControlError::from(error),
                    )
                })?;
            aggregate
                .standby_mut()
                .expect("checked standby arena")
                .push(peer, frame, encoded)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            let batch = self
                .prepared_standby
                .as_mut()
                .expect("checked AP standby batch");
            batch.admitted += 1;
            #[cfg(any(feature = "diagnostics", test))]
            {
                batch.preparation_micros = batch.preparation_micros.saturating_add(
                    self.observer
                        .map(|observer| observer.now_micros().saturating_sub(started.unwrap_or(0)))
                        .unwrap_or(0),
                );
            }
            return Ok(true);
        }

        let Some(first) = self.prepared_first.take() else {
            self.prepared_first_key = Some(key);
            self.prepared_first = Some(frame);
            return Ok(true);
        };
        let first_key = self
            .prepared_first_key
            .take()
            .expect("prepared AP frame retains its flow key");
        let admission = control.mac.aggregate_admission(first.as_slice());
        let Some(admission) = admission.filter(|admission| {
            first_key == key
                && first_key == ApTxFlowKey::associated(admission.association())
                && admission.accepts_ethernet(frame.as_slice())
        }) else {
            debug_assert!(self.prepared_second.is_none());
            self.prepared_first_key = Some(first_key);
            self.prepared_first = Some(first);
            self.prepared_second_key = Some(key);
            self.prepared_second = Some(frame);
            return Ok(true);
        };
        let peer = admission.peer();
        {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            ordinary
                .require_unprotected_ht_aggregate(admission.rate())
                .map_err(Esp32s31ApAmpduError::Protection)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        }
        let mut first = match network.try_promote(first) {
            Ok(first) => first,
            Err(first) => {
                self.restore_active_pair_front(first_key, first, frame);
                return Ok(false);
            }
        };
        let mut frame = match network.try_promote(frame) {
            Ok(frame) => frame,
            Err(frame) => {
                self.restore_active_pair_front(
                    first_key,
                    PinnedNetworkTxFrame::Direct(first),
                    frame,
                );
                return Ok(false);
            }
        };
        let first_offset = first.ethernet_offset();
        let first_length = first.ethernet_length();
        let first_encoded = control
            .mac
            .engine_mut()
            .encode_aggregate_ethernet_in_place(
                admission.binding(),
                first.storage_mut(),
                first_offset,
                first_length,
            )
            .map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::from(
                    error,
                ))
            })?;
        let policy = admission
            .bind_policy(first_encoded.hardware_key_selector, SLOTS)
            .map_err(Esp32s31ApAmpduError::from)
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        let standby = aggregate.standby_mut().expect("checked standby arena");
        standby
            .begin(
                peer,
                policy.rate(),
                first_encoded.sequence_number,
                policy.role().hardware_key_selector,
            )
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        standby
            .push(peer, first, first_encoded)
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        let offset = frame.ethernet_offset();
        let length = frame.ethernet_length();
        let encoded = control
            .mac
            .engine_mut()
            .encode_aggregate_ethernet_in_place(
                admission.binding(),
                frame.storage_mut(),
                offset,
                length,
            )
            .map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::from(
                    error,
                ))
            })?;
        aggregate
            .standby_mut()
            .expect("checked standby arena")
            .push(peer, frame, encoded)
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        #[cfg(feature = "tx-phase-telemetry")]
        self.publish_shadow_grant(admission.association(), policy.frame_limit());
        self.prepared_standby = Some(PreparedStandby {
            admission,
            policy,
            admitted: 2,
            #[cfg(feature = "tx-phase-telemetry")]
            mismatch_claims: 0,
            #[cfg(any(feature = "diagnostics", test))]
            preparation_micros: self
                .observer
                .map(|observer| observer.now_micros().saturating_sub(started.unwrap_or(0)))
                .unwrap_or(0),
        });
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::StandbyPrepared);
        }
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_prepared<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        let _ = self.stage_dtim_group_release(control)?;
        if self.prepared_group_release.is_some() {
            return self.start_prepared_group_release(control, hardware);
        }
        if self.prepared_buffered_release.is_some() {
            return self.start_prepared_buffered_release(control, hardware);
        }
        self.prepare_ready_standby(aggregate, control, network)?;
        #[cfg(feature = "tx-phase-telemetry")]
        self.record_partial_frontier(network);
        let Some(_batch) = self.prepared_standby.take() else {
            loop {
                let Some(frame) = self.prepared_first.take() else {
                    return Ok(WifiTxProgress::Complete);
                };
                let key = self
                    .prepared_first_key
                    .take()
                    .expect("prepared AP frame retains its flow key");
                self.prepared_first = self.prepared_second.take();
                self.prepared_first_key = self.prepared_second_key.take();
                if !key.is_current(control.mac.engine()) {
                    drop(frame);
                    continue;
                }
                let Some((readmitted_key, frame)) =
                    self.retain_power_save(control.mac.engine_mut(), frame)?
                else {
                    continue;
                };
                if readmitted_key != key {
                    drop(frame);
                    continue;
                }
                return control
                    .start_network_tx(hardware, frame.as_slice())
                    .map_err(Esp32s31AccessPointDatapathError::Control);
            }
        };
        #[cfg(any(feature = "diagnostics", test))]
        let publication_started = self.observer.map(AggregateTxObserver::now_micros);
        let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
            Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(error))
        })?;
        aggregate
            .publish_standby(ordinary, hardware)
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        let now = ordinary.now_micros();
        self.deadline_micros = Some(now.saturating_add(ordinary.publication_timeout_micros()));
        #[cfg(any(feature = "diagnostics", test))]
        {
            self.exchange_started_micros = publication_started;
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            let finished = observer.now_micros();
            let started = publication_started.unwrap_or(finished);
            observe_aggregate_rate(observer, _batch.policy.rate());
            observer.observe(AggregateTxObservation::Prepared {
                subframes: u8::try_from(_batch.admitted).unwrap_or(u8::MAX),
                stop: if _batch.admitted == usize::from(_batch.policy.frame_limit()) {
                    AggregateBuildStop::FrameLimit
                } else {
                    AggregateBuildStop::QueueEmpty
                },
            });
            observer.observe(AggregateTxObservation::PreparationCompleted {
                micros: _batch.preparation_micros,
            });
            observer.observe(AggregateTxObservation::Published {
                at_micros: started,
                program_micros: finished.saturating_sub(started),
            });
            observer.observe(AggregateTxObservation::StandbyPublished);
            control.observe_ht_aggregate(_batch.policy.rate());
        }
        self.prepare_ready_standby(aggregate, control, network)?;
        #[cfg(feature = "tx-phase-telemetry")]
        if self.prepared_standby.is_none() {
            self.clear_shadow_grant();
        }
        Ok(WifiTxProgress::Pending)
    }

    pub(super) fn cancel_prepared<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        network: Option<
            &PinnedTxInterfaceConsumer<
                'resources,
                M,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                TX_QUEUE_DEPTH,
            >,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        #[cfg(feature = "tx-core1-materializer-probe")]
        if self.core1_materialization_in_flight {
            network
                .expect("an in-flight Core1 batch retains its interface capability")
                .cancel_core1_materialization();
            self.core1_materialization_in_flight = false;
        }
        #[cfg(not(feature = "tx-core1-materializer-probe"))]
        let _ = network;
        self.rollback_prepared_buffered_release(control)?;
        self.discard_group_buffer(control)?;
        control
            .rollback_pending_buffered_releases()
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        self.prepared_first = None;
        self.prepared_first_key = None;
        self.prepared_second = None;
        self.prepared_second_key = None;
        while let Some((_, index)) = self.active_frames.pop_scheduled() {
            drop(self.frame_arena.take(index));
        }
        if self.prepared_standby.take().is_some() {
            aggregate
                .standby_mut()
                .expect("prepared batch owns standby arena")
                .cancel_build()
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::StandbyCancelled);
            }
        }
        #[cfg(feature = "tx-phase-telemetry")]
        self.clear_shadow_grant();
        Ok(())
    }

    pub(super) async fn wait_deadline<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if let Some(deadline) = self.deadline_micros {
            let (_, ordinary) = control
                .mac
                .try_aggregate_adapter()
                .expect("aggregate publication leaves ordinary AP TX idle");
            ordinary.wait_until(deadline).await;
        } else {
            control.wait_tx_deadline().await;
        }
    }

    pub(super) async fn service<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        if self.deadline_micros.is_none() {
            let progress = match control.service_tx(hardware, wake).await {
                Ok(progress) => progress,
                Err(error) => {
                    if self.active_group_release.is_some() {
                        self.complete_active_group_release(control, false)?;
                    }
                    if self.active_buffered_release.is_some() {
                        self.complete_active_buffered_release(control, false)?;
                    }
                    return Err(Esp32s31AccessPointDatapathError::Control(error));
                }
            };
            if progress == WifiTxProgress::Complete {
                let succeeded = control.take_last_terminal_tx_succeeded().unwrap_or(false);
                if self.active_group_release.is_some() {
                    // A group MPDU has no ACK. `succeeded` is only terminal
                    // hardware publication success for the one-attempt basic-
                    // rate transaction.
                    self.complete_active_group_release(control, succeeded)?;
                }
                if self.active_buffered_release.is_some() {
                    self.complete_active_buffered_release(control, succeeded)?;
                }
                let _ = self.stage_dtim_group_release(control)?;
                if self.prepared_group_release.is_none() {
                    let _ = self.stage_awake_buffered_release(control)?;
                }
            }
            return Ok(progress);
        }

        let service_event = AggregateTxServiceEvent::classify(wake).map_err(|error| {
            Esp32s31AccessPointDatapathError::Aggregate(
                Esp32s31ApAmpduError::ConflictingInterruptEvents(error.events),
            )
        })?;
        if service_event == AggregateTxServiceEvent::Collision {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            if !aggregate
                .active_mut()
                .abort_collision(hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?
            {
                return Err(Esp32s31AccessPointDatapathError::Aggregate(
                    Esp32s31ApAmpduError::HardwareDidNotDetach,
                ));
            }
            ordinary.reset_aggregate_contention();
            self.deadline_micros = None;
            #[cfg(any(feature = "diagnostics", test))]
            {
                self.exchange_started_micros = None;
            }
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Collision);
            }
            return Ok(WifiTxProgress::Complete);
        }
        if matches!(
            service_event,
            AggregateTxServiceEvent::HardwareTimeout | AggregateTxServiceEvent::ExecutorDeadline
        ) {
            if !aggregate
                .active_mut()
                .begin_timeout_abort(hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?
            {
                return Err(Esp32s31AccessPointDatapathError::Aggregate(
                    Esp32s31ApAmpduError::HardwareDidNotDetach,
                ));
            }
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            ordinary.after_micros(16).await;
            aggregate
                .active_mut()
                .finish_timeout_abort(hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            ordinary.reset_aggregate_contention();
            self.deadline_micros = None;
            #[cfg(any(feature = "diagnostics", test))]
            {
                self.exchange_started_micros = None;
            }
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::HardwareTimeout);
            }
            return Ok(WifiTxProgress::Complete);
        }

        let aggregate_progress = {
            #[cfg(any(feature = "diagnostics", test))]
            let completion_started = self.observer.map(AggregateTxObserver::now_micros);
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            let progress = aggregate
                .active_mut()
                .service_completion(ordinary, hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                let finished = observer.now_micros();
                let started = completion_started.unwrap_or(finished);
                match progress {
                    Esp32s31ApAmpduProgress::Republished(_) => {
                        observer.observe(AggregateTxObservation::Published {
                            at_micros: started,
                            program_micros: finished.saturating_sub(started),
                        });
                    }
                    Esp32s31ApAmpduProgress::CompletionReady(_) => {
                        observer.observe(AggregateTxObservation::CompletionCoreCompleted {
                            micros: finished.saturating_sub(started),
                        });
                    }
                    Esp32s31ApAmpduProgress::Pending => {}
                }
            }
            progress
        };
        match aggregate_progress {
            Esp32s31ApAmpduProgress::CompletionReady(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_completion_details(completion, false);
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                #[cfg(any(feature = "diagnostics", test))]
                let release_started = self.observer.map(AggregateTxObserver::now_micros);
                aggregate
                    .active_mut()
                    .release_completed()
                    .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    let finished = observer.now_micros();
                    observer.observe(AggregateTxObservation::BackingReleaseCompleted {
                        micros: finished.saturating_sub(release_started.unwrap_or(finished)),
                    });
                }
                #[cfg(any(feature = "diagnostics", test))]
                {
                    debug_assert!(self.terminal_acknowledged.is_none());
                    self.terminal_acknowledged = Some(completion.acknowledged);
                }
                self.deadline_micros = None;
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.exchange_started_micros = None;
                }
                Ok(WifiTxProgress::Complete)
            }
            Esp32s31ApAmpduProgress::Republished(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_completion_details(completion, true);
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                        error,
                    ))
                })?;
                self.deadline_micros = Some(
                    ordinary
                        .now_micros()
                        .saturating_add(ordinary.publication_timeout_micros()),
                );
                Ok(WifiTxProgress::Pending)
            }
            Esp32s31ApAmpduProgress::Pending => {
                if service_event == AggregateTxServiceEvent::Completion {
                    return Err(Esp32s31AccessPointDatapathError::Aggregate(
                        Esp32s31ApAmpduError::CompletionInterruptWithoutState,
                    ));
                }
                Ok(WifiTxProgress::Pending)
            }
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_completion_details(&self, completion: Esp32s31ApAmpduCompletion, republished: bool) {
        let Some(observer) = self.observer else {
            return;
        };
        observer.observe(AggregateTxObservation::BlockAckProcessed {
            tx_status: completion.tx_status,
            block_ack_received: completion.block_ack_received,
            control: completion.block_ack_control,
            first_sequence: completion.first_sequence,
            starting_sequence: completion.starting_sequence,
            subframes: completion.subframes,
            missing: completion.missing,
        });
        if !republished && let Some(started) = self.exchange_started_micros {
            observer.observe(AggregateTxObservation::ExchangeCompleted {
                micros: observer.now_micros().saturating_sub(started),
                publications: completion.aggregate_attempts,
            });
        }
    }
}

/// Narrow bridge used by the same-channel RX owner to turn a peer's PM=0
/// edge into prepared network work without exposing frame storage to the
/// protocol processor.
pub(super) trait AccessPointPowerSaveNetworkTx<
    P,
    E,
    T,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
{
    fn stage_awake_release(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError>;

    fn has_power_save_release(&self) -> bool;

    fn discard_group_power_save(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError>;
}

impl<
    'observer,
    'resources,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> AccessPointPowerSaveNetworkTx<P, E, T, DMA_BUFFER_SIZE, TX_BUFFER_SIZE>
    for Esp32s31AccessPointNetworkTx<
        'observer,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn stage_awake_release(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, Esp32s31AccessPointDatapathError> {
        self.stage_awake_buffered_release(control)
    }

    fn has_power_save_release(&self) -> bool {
        self.prepared_buffered_release.is_some()
            || self.active_buffered_release.is_some()
            || self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
            || self.dtim_group_release_remaining != 0
    }

    fn discard_group_power_save(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointDatapathError> {
        self.discard_group_buffer(control)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApActiveFrameQueues, ApAssociationIdentity, ApFrameLeaseArena, ApPowerSaveFrameQueue,
        ApTxFlowKey, aggregate_adapter_available,
    };

    struct TestActiveArena<B> {
        leases: ApFrameLeaseArena<B>,
        queues: ApActiveFrameQueues,
    }

    impl<B> TestActiveArena<B> {
        const fn new() -> Self {
            Self {
                leases: ApFrameLeaseArena::new(),
                queues: ApActiveFrameQueues::new(),
            }
        }

        fn push(&mut self, key: ApTxFlowKey, frame: B) -> Result<(), B> {
            let index = self.leases.insert(frame)?;
            if self.queues.push_back(key, index).is_err() {
                return Err(self.leases.take(index));
            }
            Ok(())
        }

        fn pop_key(&mut self, key: ApTxFlowKey) -> Option<B> {
            self.queues
                .pop_key(key)
                .map(|index| self.leases.take(index))
        }

        fn pop_scheduled(&mut self) -> Option<(ApTxFlowKey, B)> {
            self.queues
                .pop_scheduled()
                .map(|(key, index)| (key, self.leases.take(index)))
        }

        fn len_for(&self, key: ApTxFlowKey) -> usize {
            self.queues.len_for(key)
        }

        fn len(&self) -> usize {
            self.queues.len()
        }
    }

    const FLOW_A: ApTxFlowKey =
        ApTxFlowKey::associated(ApAssociationIdentity::new([0x02, 0, 0, 0, 0, 1], 1, 10).unwrap());
    const FLOW_B: ApTxFlowKey =
        ApTxFlowKey::associated(ApAssociationIdentity::new([0x02, 0, 0, 0, 0, 2], 2, 11).unwrap());
    const FLOW_A_REASSOCIATED: ApTxFlowKey =
        ApTxFlowKey::associated(ApAssociationIdentity::new([0x02, 0, 0, 0, 0, 1], 1, 12).unwrap());

    #[test]
    fn ordinary_publication_blocks_aggregate_adapter_borrow() {
        assert!(aggregate_adapter_available(false));
        assert!(!aggregate_adapter_available(true));
    }

    #[test]
    fn generation_bound_flow_key_stays_compact() {
        assert_eq!(core::mem::size_of::<ApTxFlowKey>(), 12);
    }

    #[test]
    fn active_arena_regroups_interleaved_frames_without_reordering_a_flow() {
        let mut arena = TestActiveArena::new();
        for sequence in 0..16_u8 {
            arena.push(FLOW_A, sequence * 2).unwrap();
            arena.push(FLOW_B, sequence * 2 + 1).unwrap();
        }

        assert_eq!(arena.len_for(FLOW_A), 16);
        assert_eq!(arena.len_for(FLOW_B), 16);
        for sequence in 0..16_u8 {
            assert_eq!(arena.pop_key(FLOW_A), Some(sequence * 2));
        }
        for sequence in 0..16_u8 {
            assert_eq!(arena.pop_key(FLOW_B), Some(sequence * 2 + 1));
        }
        assert_eq!(arena.len(), 0);
    }

    #[test]
    fn active_arena_schedules_nonempty_flows_round_robin() {
        let mut arena = TestActiveArena::new();
        arena.push(FLOW_A, 10).unwrap();
        arena.push(FLOW_A, 11).unwrap();
        arena.push(FLOW_B, 20).unwrap();
        arena.push(FLOW_B, 21).unwrap();

        assert_eq!(arena.pop_scheduled(), Some((FLOW_A, 10)));
        assert_eq!(arena.pop_scheduled(), Some((FLOW_B, 20)));
        assert_eq!(arena.pop_scheduled(), Some((FLOW_A, 11)));
        assert_eq!(arena.pop_scheduled(), Some((FLOW_B, 21)));
    }

    #[test]
    fn active_arena_never_merges_reused_peer_generations() {
        let mut arena = TestActiveArena::new();
        arena.push(FLOW_A, 10).unwrap();
        arena.push(FLOW_A_REASSOCIATED, 20).unwrap();

        assert_eq!(arena.len_for(FLOW_A), 1);
        assert_eq!(arena.len_for(FLOW_A_REASSOCIATED), 1);
        assert_eq!(arena.pop_key(FLOW_A), Some(10));
        assert_eq!(arena.pop_key(FLOW_A_REASSOCIATED), Some(20));
    }

    #[test]
    fn power_save_queue_drops_only_the_stale_association_generation() {
        let first = FLOW_A.association().unwrap();
        let replacement = FLOW_A_REASSOCIATED.association().unwrap();
        let mut leases = ApFrameLeaseArena::new();
        let mut queue = ApPowerSaveFrameQueue::new();
        queue.push(first, 10, &mut leases).unwrap();
        queue.push(replacement, 20, &mut leases).unwrap();

        queue.retain(&mut leases, |identity| identity == replacement);

        assert_eq!(queue.len, 1);
        assert_eq!(queue.oldest_index_for(first), None);
        let index = queue.oldest_index_for(replacement).unwrap();
        assert_eq!(queue.take_at(index, &mut leases).unwrap().frame, 20);
    }

    #[test]
    fn active_arena_reuses_every_bounded_frame_slot() {
        let mut arena = TestActiveArena::new();
        for value in 0..super::AP_ACTIVE_FRAME_CAPACITY {
            arena.push(FLOW_A, value).unwrap();
        }
        assert_eq!(arena.push(FLOW_A, usize::MAX), Err(usize::MAX));
        for value in 0..super::AP_ACTIVE_FRAME_CAPACITY {
            assert_eq!(arena.pop_key(FLOW_A), Some(value));
        }
        arena.push(FLOW_B, 42).unwrap();
        assert_eq!(arena.pop_key(FLOW_B), Some(42));
    }
}
