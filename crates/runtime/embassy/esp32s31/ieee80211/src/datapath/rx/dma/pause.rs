//! Same-epoch suspension of the physical RX producer.
//!
//! Queued and consumer-held leases retain their storage throughout suspension.
//! Only the live specialization can service RX or republish returned buffers.

use super::*;
use oer_esp32s31_wifi_dma::rx_ring::{RxResumeError, RxRingPaused, RxRingResumeFailure};

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
    R,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
        R,
    >
{
    fn split_ring(
        self,
    ) -> (
        R,
        StagedRxProducer<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            P,
            (),
        >,
    ) {
        let Self {
            ring,
            ring_lifetime: _,
            storage,
            pool,
            frames,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
            admission,
            serviced_descriptors,
            serviced_units,
            serviced_bytes,
        } = self;
        (
            ring,
            StagedRxProducer {
                ring: (),
                ring_lifetime: PhantomData,
                storage,
                pool,
                frames,
                delay,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer,
                admission,
                serviced_descriptors,
                serviced_units,
                serviced_bytes,
            },
        )
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
        (),
    >
{
    fn with_ring<R>(
        self,
        ring: R,
    ) -> StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
        R,
    > {
        let Self {
            ring: (),
            ring_lifetime,
            storage,
            pool,
            frames,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
            admission,
            serviced_descriptors,
            serviced_units,
            serviced_bytes,
        } = self;
        StagedRxProducer {
            ring,
            ring_lifetime,
            storage,
            pool,
            frames,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
            admission,
            serviced_descriptors,
            serviced_units,
            serviced_bytes,
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
        RxRingLive<'storage, COUNT>,
    >
{
    /// Suspend RX without draining leases or discarding admission and counters.
    /// MAC/IRQ quiescence and RF admission remain the caller's responsibility.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_pause<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        StagedRxProducer<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            P,
            RxRingPaused<'storage, COUNT>,
        >,
        (Self, RxRingError),
    > {
        let (ring, resources) = self.split_ring();
        match ring.try_pause(hardware) {
            Ok(ring) => Ok(resources.with_ring(ring)),
            Err((ring, error)) => Err((resources.with_ring(ring), error)),
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
        RxRingPaused<'storage, COUNT>,
    >
{
    /// Return the original producer with its queue, policy and progress intact.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_resume<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        StagedRxProducer<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            P,
            RxRingLive<'storage, COUNT>,
        >,
        StagedRxProducer<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            P,
            RxRingResumeFailure<'storage, COUNT>,
        >,
    > {
        let (ring, resources) = self.split_ring();
        match ring.try_resume(hardware) {
            Ok(ring) => Ok(resources.with_ring(ring)),
            Err(failure) => Err(resources.with_ring(failure)),
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
        RxRingResumeFailure<'storage, COUNT>,
    >
{
    pub const fn resume_error(&self) -> RxResumeError {
        self.ring.error()
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
        RxRingPaused<'storage, COUNT>,
    >
{
    /// Abandon the epoch only after all queued and consumer-held leases return.
    /// On hardware failure, retain the complete producer for a later stop.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_stop<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        StoppedReceive<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        (Self, RxRingError),
    > {
        if self.pool.claimed_slots() != 0 {
            return Err((self, RxRingError::Busy));
        }
        let (ring, resources) = self.split_ring();
        match ring.try_stop(hardware) {
            Ok(ring) => Ok(StoppedReceive {
                ring,
                storage: resources.storage,
                pool: resources.pool,
                frames: resources.frames,
                delay: resources.delay,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer: resources.pipeline_observer,
            }),
            Err((ring, error)) => Err((resources.with_ring(ring), error)),
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
        RxRingResumeFailure<'storage, COUNT>,
    >
{
    /// Abandon the epoch only after all queued and consumer-held leases return.
    /// On hardware failure, retain the complete producer for a later stop.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_stop<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        StoppedReceive<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        (Self, RxRingError),
    > {
        if self.pool.claimed_slots() != 0 {
            return Err((self, RxRingError::Busy));
        }
        let (ring, resources) = self.split_ring();
        match ring.try_stop(hardware) {
            Ok(ring) => Ok(StoppedReceive {
                ring,
                storage: resources.storage,
                pool: resources.pool,
                frames: resources.frames,
                delay: resources.delay,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer: resources.pipeline_observer,
            }),
            Err((ring, error)) => Err((resources.with_ring(ring), error)),
        }
    }
}
