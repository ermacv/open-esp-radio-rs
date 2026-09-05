//! Bounded lease storage and per-flow selection over one shared arena.

use super::*;
use crate::datapath::software_tx_queue::{IndexedLeaseArena, RoundRobinTxQueues};

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
pub(super) const AP_ACTIVE_FRAME_CAPACITY: usize = AP_POWER_SAVE_FRAME_CAPACITY;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ApTxFlowKey {
    pub(super) destination: [u8; 6],
    association_epoch: u32,
    association_id: u8,
    tid: u8,
}

impl ApTxFlowKey {
    const INVALID_PEER: [u8; 6] = [0; 6];

    pub(super) const fn associated(identity: ApAssociationIdentity) -> Self {
        let association_id = identity.association_id();
        assert!(association_id <= u8::MAX as u16);
        Self {
            destination: identity.address(),
            association_epoch: identity.association_epoch(),
            association_id: association_id as u8,
            tid: open_esp_radio_esp32s31_wifi_ap::protocol::AP_TX_BLOCK_ACK_TID,
        }
    }

    pub(super) fn unbound_from_ethernet(ethernet: &[u8]) -> Self {
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

    pub(super) fn association(self) -> Option<ApAssociationIdentity> {
        ApAssociationIdentity::new(
            self.destination,
            u16::from(self.association_id),
            self.association_epoch,
        )
    }

    pub(super) fn is_current(self, engine: &Esp32s31ApEngine<'_>) -> bool {
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
pub(super) type ApFrameLeaseArena<B> = IndexedLeaseArena<B, AP_ACTIVE_FRAME_CAPACITY>;

/// Intrusive per-flow FIFO links over [`ApFrameLeaseArena`].
pub(super) type ApActiveFrameQueues =
    RoundRobinTxQueues<ApTxFlowKey, AP_ACTIVE_FLOW_CAPACITY, AP_ACTIVE_FRAME_CAPACITY>;

pub(super) struct BufferedUnicast<B> {
    pub(super) identity: ApAssociationIdentity,
    order: u64,
    pub(super) frame: B,
}

#[derive(Clone, Copy)]
struct BufferedUnicastIndex {
    identity: ApAssociationIdentity,
    order: u64,
    frame_index: u8,
}

pub(super) struct BufferedGroup<B> {
    order: u64,
    pub(super) frame: B,
}

#[derive(Clone, Copy)]
struct BufferedGroupIndex {
    order: u64,
    frame_index: u8,
}

pub(super) struct ApPowerSaveFrameQueue {
    slots: [Option<BufferedUnicastIndex>; AP_POWER_SAVE_FRAME_CAPACITY],
    next_order: u64,
    pub(super) len: usize,
}

impl ApPowerSaveFrameQueue {
    pub(super) const fn new() -> Self {
        Self {
            slots: [const { None }; AP_POWER_SAVE_FRAME_CAPACITY],
            next_order: 0,
            len: 0,
        }
    }

    pub(super) fn push<B>(
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

    pub(super) fn take_at<B>(
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

    pub(super) fn restore<B>(
        &mut self,
        buffered: BufferedUnicast<B>,
        arena: &mut ApFrameLeaseArena<B>,
    ) {
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

    pub(super) fn oldest_index_for(&self, identity: ApAssociationIdentity) -> Option<usize> {
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

    pub(super) fn oldest_releasable_peer(
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

    pub(super) fn retain<B>(
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
pub(super) struct ApGroupFrameQueue {
    slots: [Option<BufferedGroupIndex>; AP_POWER_SAVE_FRAME_CAPACITY],
    next_order: u64,
    len: usize,
}

impl ApGroupFrameQueue {
    pub(super) const fn new() -> Self {
        Self {
            slots: [const { None }; AP_POWER_SAVE_FRAME_CAPACITY],
            next_order: 0,
            len: 0,
        }
    }

    pub(super) fn push<B>(
        &mut self,
        frame: B,
        arena: &mut ApFrameLeaseArena<B>,
    ) -> Result<usize, B> {
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

    pub(super) fn take_at<B>(
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

    pub(super) fn restore<B>(
        &mut self,
        buffered: BufferedGroup<B>,
        arena: &mut ApFrameLeaseArena<B>,
    ) {
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

    pub(super) fn oldest_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.as_ref().map(|entry| (index, entry.order)))
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
    }

    pub(super) fn clear<B>(&mut self, arena: &mut ApFrameLeaseArena<B>) -> usize {
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

impl<'observer, B, N> Esp32s31AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
{
    fn push_active_frame(&mut self, key: ApTxFlowKey, frame: N) -> Result<(), N> {
        let frame_index = self.frame_arena.insert(frame)?;
        if self.active_frames.push_back(key, frame_index).is_err() {
            return Err(self.frame_arena.take(frame_index));
        }
        Ok(())
    }

    fn push_active_frame_front(&mut self, key: ApTxFlowKey, frame: N) -> Result<(), N> {
        let frame_index = self.frame_arena.insert(frame)?;
        if self.active_frames.push_front(key, frame_index).is_err() {
            return Err(self.frame_arena.take(frame_index));
        }
        Ok(())
    }

    pub(super) fn restore_active_pair_front(&mut self, key: ApTxFlowKey, first: N, second: N) {
        // `push_front` reverses insertion order. Restore the younger frame
        // first so the next scheduler turn observes the original prefix.
        self.restore_active_frame_front(key, second);
        self.restore_active_frame_front(key, first);
    }

    pub(super) fn restore_active_frame_front(&mut self, key: ApTxFlowKey, frame: N) {
        self.push_active_frame_front(key, frame)
            .unwrap_or_else(|_| panic!("AP rollback lost its bounded arena credit"));
    }

    fn pop_active_key(&mut self, key: ApTxFlowKey) -> Option<N> {
        self.active_frames
            .pop_key(key)
            .map(|index| self.frame_arena.take(index))
    }

    pub(super) fn pop_scheduled_active(
        &mut self,
        engine: &Esp32s31ApEngine<'_>,
    ) -> Option<(ApTxFlowKey, N)> {
        loop {
            let (key, index) = self.active_frames.pop_scheduled()?;
            let frame = self.frame_arena.take(index);
            if key.is_current(engine) {
                return Some((key, frame));
            }
            drop(frame);
        }
    }

    fn observe_network_claim(&self, frame: &N) {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe_access_point_network_claim(frame.as_slice());
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        let _ = frame;
    }

    pub(super) fn retain_active_frame(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        frame: N,
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

    pub(super) fn take_matching_active_or_network(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        key: ApTxFlowKey,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<Option<N>, Esp32s31AccessPointDatapathError> {
        if !key.is_current(engine) {
            while let Some(frame) = self.pop_active_key(key) {
                drop(frame);
            }
            return Ok(None);
        }
        if let Some(frame) = self.pop_active_key(key) {
            return Ok(Some(frame));
        }
        while let Some(frame) = network.try_take() {
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

    pub(super) fn take_scheduled_active_or_network(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<Option<(ApTxFlowKey, N)>, Esp32s31AccessPointDatapathError> {
        if let Some((key, frame)) = self.pop_scheduled_active(engine) {
            return Ok(Some((key, frame)));
        }
        let Some(frame) = network.try_take() else {
            return Ok(None);
        };
        self.observe_network_claim(&frame);
        self.retain_power_save(engine, frame)
    }
}
