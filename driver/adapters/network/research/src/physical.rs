//! Executor-neutral fixed-SRAM batch composition for the research engine.

use core::{cell::Cell, pin::Pin};

use open_esp_radio_dma::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, DmaIndexReturn, PinnedDmaTxPool,
    PinnedDmaTxRadioLease, ReturningStableDmaBacking, TaggedStableDmaBacking,
};
use open_esp_radio_network::NetworkInterfaceId;
use open_esp_radio_wifi_datapath::{BatchWriteError, PhysicalTxSource, ReservedTxBatch};

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
            prepared: [const { Cell::new(None) }; BATCH_CAPACITY],
            prepared_cursor: Cell::new(0),
            prepared_count: Cell::new(0),
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
    prepared: [Cell<
        Option<
            PinnedResearchTxFrame<
                'allocator,
                'resources,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
        >,
    >; BATCH_CAPACITY],
    prepared_cursor: Cell<usize>,
    prepared_count: Cell<usize>,
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
        self.prepared_count.get()
    }

    pub fn take_prepared(
        &self,
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
        while self.prepared_cursor.get() < BATCH_CAPACITY {
            let index = self.prepared_cursor.get();
            self.prepared_cursor.set(index + 1);
            if let Some(frame) = self.prepared[index].take() {
                self.prepared_count.set(self.prepared_count.get() - 1);
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
    'allocator,
    'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const BATCH_CAPACITY: usize,
> PhysicalTxSource
    for PinnedReservedTxBatch<
        'allocator,
        'resources,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        BATCH_CAPACITY,
    >
{
    type Frame = PinnedResearchTxFrame<
        'allocator,
        'resources,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
    >;

    fn pending_frames(&self) -> usize {
        self.prepared_len()
    }

    fn try_take_physical(&self) -> Option<Self::Frame> {
        self.take_prepared()
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
        self.prepared[position].set(Some(TaggedStableDmaBacking::new(
            self.interface,
            ReturningStableDmaBacking::new(
                self.pool.claim_radio(index),
                PinnedBatchReturn {
                    free: self.free_return,
                },
            ),
        )));
        self.prepared_count.set(self.prepared_count.get() + 1);
        self.prepared_cursor
            .set(self.prepared_cursor.get().min(position));
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
mod tests;
