//! Role-neutral ownership handoff from physical RX DMA to protocol roles.

use core::marker::PhantomData;

use embassy_sync::{blocking_mutex::raw::RawMutex, channel::TryReceiveError};

use oer_memory::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, AffineSpscTryReceiveError,
    AffineSpscTrySendError,
};

use oer_esp32s31_ieee80211_mac::rx::{
    RxPhyInfo,
    pool::{NetworkRxFrame, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};

use oer_ieee80211_softmac::MacRxMetadata;

/// Unique owner of one staged RX unit.
pub type StagedRxFrame<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> = NetworkRxFrame<'pool, SLOTS, CAPACITY>;

/// Full result from a non-blocking staged-RX publication.
pub struct StagedRxTrySendError<T>(pub T);

/// Single physical-DMA producer endpoint.
///
/// The producer and consumer cursors occupy different cache lines. Payload
/// ownership is published by the producer cursor's Release store and returned
/// by the consumer cursor's Release store; no mutex, interrupt masking, scan,
/// or per-slot state transition is required for this same-stream handoff.
pub struct StagedRxSender<
    'queue,
    'pool,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
> {
    inner: AffineSpscSender<'queue, StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    mutex: PhantomData<M>,
}

impl<'queue, 'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    StagedRxSender<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS>
{
    pub fn try_send(
        &self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Result<(), StagedRxTrySendError<StagedRxFrame<'pool, CAPACITY, SLOTS>>> {
        let started = crate::diagnostics::core0_rx_cycles::cycle_count();
        let result = self
            .inner
            .try_send(frame)
            .map_err(|AffineSpscTrySendError(frame)| StagedRxTrySendError(frame));
        crate::diagnostics::core0_rx_service_histogram::CORE0_RX_SERVICE_HISTOGRAM
            .record_spsc_push(
                crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(started),
                result.is_err(),
            );
        result
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn free_capacity(&self) -> usize {
        DEPTH.saturating_sub(self.len())
    }
}

/// Single protocol consumer endpoint paired with [`StagedRxSender`].
pub struct StagedRxReceiver<
    'queue,
    'pool,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
> {
    inner: AffineSpscReceiver<'queue, StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    mutex: PhantomData<M>,
}

impl<'queue, 'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    StagedRxReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS>
{
    pub fn try_receive(&self) -> Result<StagedRxFrame<'pool, CAPACITY, SLOTS>, TryReceiveError> {
        let started = crate::diagnostics::core0_rx_cycles::cycle_count();
        let result = self
            .inner
            .try_receive()
            .map_err(|AffineSpscTryReceiveError::Empty| TryReceiveError::Empty);
        crate::diagnostics::core0_rx_service_histogram::CORE0_RX_SERVICE_HISTOGRAM.record_spsc_pop(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(started),
            result.is_err(),
        );
        result
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Static bounded storage for the physical-RX to protocol-role handoff.
pub struct StagedRxQueue<
    'pool,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    inner: AffineSpscQueue<StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    mutex: PhantomData<M>,
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    StagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    pub const fn new() -> Self {
        const {
            assert!(
                DEPTH <= SLOTS,
                "staged RX queue cannot outgrow its ownership pool"
            );
        }
        Self {
            inner: AffineSpscQueue::new(),
            mutex: PhantomData,
        }
    }

    pub fn split(
        &mut self,
    ) -> (
        StagedRxSender<'_, 'pool, M, DEPTH, CAPACITY, SLOTS>,
        StagedRxReceiver<'_, 'pool, M, DEPTH, CAPACITY, SLOTS>,
    ) {
        let (sender, receiver) = self.inner.split();
        (
            StagedRxSender {
                inner: sender,
                mutex: PhantomData,
            },
            StagedRxReceiver {
                inner: receiver,
                mutex: PhantomData,
            },
        )
    }
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize> Default
    for StagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Borrow-free Ethernet publication captured while a role validates a frame.
#[derive(Clone, Copy, Debug)]
pub struct StagedEthernetPublication {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload_offset: usize,
    pub payload_length: usize,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedRxDisposition {
    Released,
    RetainedByNetwork,
}
