//! Executor-neutral fixed-SRAM batch composition for the research engine.

use core::pin::Pin;

use open_esp_radio_dma::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, DmaIndexReturn, PinnedDmaTxPool,
    PinnedDmaTxRadioLease, ReturningStableDmaBacking, TaggedStableDmaBacking,
};
use open_esp_radio_network::NetworkInterfaceId;
use open_esp_radio_wifi_datapath::{BatchWriteError, ReservedTxBatch};

/// Static free-credit storage for one fused pinned TX allocator.
pub struct PinnedBatchResources<const QUEUE_DEPTH: usize> {
    free: AffineSpscQueue<u8, QUEUE_DEPTH>,
}

impl<const QUEUE_DEPTH: usize> PinnedBatchResources<QUEUE_DEPTH> {
    pub const fn new() -> Self {
        Self {
            free: AffineSpscQueue::new(),
        }
    }

    /// Binds one ownership epoch to a permanently located DMA pool.
    pub fn bind<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>(
        &self,
        pool: Pin<&'static mut PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>,
    ) -> PinnedBatchAllocator<'_, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        assert!(QUEUE_DEPTH <= usize::from(u8::MAX) + 1);
        let (free_return, free_claim) = self.free.split();
        for index in 0..QUEUE_DEPTH {
            free_return
                .try_send(index as u8)
                .expect("empty free-credit queue accepts every SRAM index");
        }
        PinnedBatchAllocator {
            free_return,
            free_claim,
            pool: Pin::into_ref(pool).get_ref(),
        }
    }
}

impl<const QUEUE_DEPTH: usize> Default for PinnedBatchResources<QUEUE_DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

/// Sole fused owner of one fixed physical TX horizon.
pub struct PinnedBatchAllocator<
    'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free_return: AffineSpscSender<'resources, u8, QUEUE_DEPTH>,
    free_claim: AffineSpscReceiver<'resources, u8, QUEUE_DEPTH>,
    pool: &'static PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedBatchAllocator<'_, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub fn free_credits(&self) -> usize {
        self.free_claim.len()
    }

    /// Reserves a complete physical prefix before network work is consumed.
    pub fn try_reserve<const BATCH_CAPACITY: usize>(
        &self,
        interface: NetworkInterfaceId,
        count: usize,
    ) -> Option<
        PinnedReservedTxBatch<
            '_,
            '_,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            BATCH_CAPACITY,
        >,
    > {
        assert!(
            count != 0,
            "a physical batch must reserve at least one slot"
        );
        assert!(
            count <= BATCH_CAPACITY,
            "batch request exceeds its typed storage"
        );
        let mut reserved = [None; BATCH_CAPACITY];
        for slot in reserved.iter_mut().take(count) {
            let Ok(index) = self.free_claim.try_receive() else {
                for index in reserved.iter_mut().filter_map(Option::take) {
                    self.return_index(index);
                }
                return None;
            };
            *slot = Some(index);
        }
        Some(PinnedReservedTxBatch {
            interface,
            free_return: &self.free_return,
            pool: self.pool,
            reserved,
            prepared: [const { None }; BATCH_CAPACITY],
            prepared_cursor: 0,
        })
    }

    fn return_index(&self, index: u8) {
        if self.free_return.try_send(index).is_err() {
            unreachable!("a unique unused SRAM reservation returns exactly once");
        }
    }
}

/// Queue-return capability retained by a prepared radio owner.
pub struct PinnedBatchReturn<'allocator, 'resources, const QUEUE_DEPTH: usize> {
    free: &'allocator AffineSpscSender<'resources, u8, QUEUE_DEPTH>,
}

impl<const QUEUE_DEPTH: usize> DmaIndexReturn for PinnedBatchReturn<'_, '_, QUEUE_DEPTH> {
    fn return_index(&self, index: u8) {
        if self.free.try_send(index).is_err() {
            unreachable!("terminal radio ownership returns its unique SRAM credit");
        }
    }
}

/// Final pinned owner produced by the research direct-construction path.
pub type PinnedResearchTxFrame<
    'allocator,
    'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = TaggedStableDmaBacking<
    NetworkInterfaceId,
    ReturningStableDmaBacking<
        PinnedDmaTxRadioLease<'static, FRAME_CAPACITY, HEADROOM, TRAILER>,
        PinnedBatchReturn<'allocator, 'resources, QUEUE_DEPTH>,
    >,
>;

/// One fully reserved batch which implements direct final-frame construction.
pub struct PinnedReservedTxBatch<
    'allocator,
    'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const BATCH_CAPACITY: usize,
> {
    interface: NetworkInterfaceId,
    free_return: &'allocator AffineSpscSender<'resources, u8, QUEUE_DEPTH>,
    pool: &'static PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    reserved: [Option<u8>; BATCH_CAPACITY],
    prepared: [Option<
        PinnedResearchTxFrame<
            'allocator,
            'resources,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    >; BATCH_CAPACITY],
    prepared_cursor: usize,
}

impl<
    'allocator,
    'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const BATCH_CAPACITY: usize,
>
    PinnedReservedTxBatch<
        'allocator,
        'resources,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        BATCH_CAPACITY,
    >
{
    pub fn prepared_len(&self) -> usize {
        self.prepared.iter().filter(|frame| frame.is_some()).count()
    }

    pub fn take_prepared(
        &mut self,
    ) -> Option<
        PinnedResearchTxFrame<
            'allocator,
            'resources,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    > {
        while self.prepared_cursor < BATCH_CAPACITY {
            let index = self.prepared_cursor;
            self.prepared_cursor += 1;
            if let Some(frame) = self.prepared[index].take() {
                return Some(frame);
            }
        }
        None
    }

    fn return_index(&self, index: u8) {
        if self.free_return.try_send(index).is_err() {
            unreachable!("a reserved research SRAM credit returns exactly once");
        }
    }
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const BATCH_CAPACITY: usize,
> ReservedTxBatch
    for PinnedReservedTxBatch<
        '_,
        '_,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        BATCH_CAPACITY,
    >
{
    fn remaining(&self) -> usize {
        self.reserved.iter().filter(|slot| slot.is_some()).count()
    }

    fn try_write<WriteError>(
        &mut self,
        length: usize,
        write: impl FnOnce(&mut [u8]) -> Result<(), WriteError>,
    ) -> Result<(), BatchWriteError<WriteError>> {
        let Some(position) = self.reserved.iter().position(Option::is_some) else {
            return Err(BatchWriteError::Exhausted);
        };
        let index = self.reserved[position]
            .take()
            .expect("selected reservation owns one SRAM index");
        let lease = self.pool.claim_network(index);
        let index = match lease.try_publish(length, write) {
            Ok((index, ())) => index,
            Err((lease, error)) => {
                let index = lease.release();
                self.return_index(index);
                return Err(BatchWriteError::Write(error));
            }
        };
        self.prepared[position] = Some(TaggedStableDmaBacking::new(
            self.interface,
            ReturningStableDmaBacking::new(
                self.pool.claim_radio(index),
                PinnedBatchReturn {
                    free: self.free_return,
                },
            ),
        ));
        Ok(())
    }
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const BATCH_CAPACITY: usize,
> Drop
    for PinnedReservedTxBatch<
        '_,
        '_,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        BATCH_CAPACITY,
    >
{
    fn drop(&mut self) {
        for index in self.reserved.iter_mut().filter_map(Option::take) {
            if self.free_return.try_send(index).is_err() {
                unreachable!("dropping a batch restores each unused SRAM credit");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::num::{NonZeroU16, NonZeroU32};
    use std::{boxed::Box, vec::Vec};

    use open_esp_radio_dma::PinnedDmaTxPool;
    use open_esp_radio_wifi_datapath::{
        AdmissionClass, EgressDemand, EgressFlowKey, EgressSelection, MaterializedTxFrame,
        RadioEgressKey, RadioPeer, TrafficIdentifier,
    };

    use super::*;
    use crate::{
        Ipv4Address, MacAddress, ResearchNetworkConfig, ResearchNetworkEngine, ResolvedIpv4Route,
    };

    type TestPool = PinnedDmaTxPool<1600, 64, 32, 3>;

    fn radio_key() -> RadioEgressKey {
        RadioEgressKey::new(
            NetworkInterfaceId::new(1),
            2,
            RadioPeer::Unicast {
                slot: 3,
                generation: 4,
            },
            TrafficIdentifier::new(0).unwrap(),
        )
    }

    #[test]
    fn research_engine_constructs_directly_in_the_pinned_sram_batch() {
        let storage = Box::leak(Box::new(TestPool::new()));
        let pool = TestPool::pin_static(storage);
        let resources = PinnedBatchResources::<3>::new();
        let allocator = resources.bind(pool);
        let config = ResearchNetworkConfig {
            interface: NetworkInterfaceId::new(1),
            mac: MacAddress::new([2, 0, 0, 0, 0, 1]),
            ipv4: Ipv4Address::new([192, 168, 1, 1]),
        };
        let mut engine = ResearchNetworkEngine::<2, 4, 1472>::new(config);
        engine
            .enqueue_udp(
                1,
                ResolvedIpv4Route {
                    destination_mac: MacAddress::new([2, 0, 0, 0, 0, 3]),
                    destination_ip: Ipv4Address::new([192, 168, 1, 3]),
                    radio: radio_key(),
                },
                1000,
                2000,
                AdmissionClass::Bulk,
                b"final-sram",
            )
            .unwrap();
        let mut demands = Vec::<EgressDemand>::new();
        engine.visit_demands(|demand| demands.push(demand));
        let mut batch = allocator.try_reserve::<2>(config.interface, 2).unwrap();
        let outcome = engine
            .fill_selected(
                EgressSelection {
                    key: EgressFlowKey {
                        radio: radio_key(),
                        admission: AdmissionClass::Bulk,
                    },
                    max_frames: NonZeroU16::new(2).unwrap(),
                    max_bytes: NonZeroU32::new(3200).unwrap(),
                },
                &mut batch,
            )
            .unwrap();
        assert_eq!(demands[0].ready_frames, 1);
        assert_eq!(outcome.frames, 1);
        assert_eq!(batch.prepared_len(), 1);
        assert_eq!(allocator.free_credits(), 1);

        let frame = batch.take_prepared().unwrap();
        assert_eq!(&frame.ethernet()[42..], b"final-sram");
        drop(frame);
        drop(batch);
        assert_eq!(allocator.free_credits(), 3);
    }

    #[test]
    fn failed_whole_batch_reservation_restores_every_credit() {
        let storage = Box::leak(Box::new(TestPool::new()));
        let pool = TestPool::pin_static(storage);
        let resources = PinnedBatchResources::<3>::new();
        let allocator = resources.bind(pool);
        let first = allocator
            .try_reserve::<2>(NetworkInterfaceId::new(1), 2)
            .unwrap();
        assert!(
            allocator
                .try_reserve::<2>(NetworkInterfaceId::new(1), 2)
                .is_none()
        );
        assert_eq!(allocator.free_credits(), 1);
        drop(first);
        assert_eq!(allocator.free_credits(), 3);
    }
}
