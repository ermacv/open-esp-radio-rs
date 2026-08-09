#![no_std]
#![forbid(unsafe_code)]

//! Bounded ownership handoff from monitor RX to an async capture consumer.
//!
//! The radio path copies a borrowed monitor view into independent caller-owned
//! storage and performs only a non-blocking channel publication. It therefore
//! never retains a DMA descriptor and never waits for a slow sniffer. The
//! capture pool is ordinary CPU memory and may be placed in external RAM when
//! the selected chip can access that memory; the Embassy channel stores only
//! leases and metadata, not frame-sized arrays.

pub mod connected_tasks;
pub mod station_network;

#[cfg(test)]
extern crate std;

use core::future::Future;

use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TrySendError},
};
use open_esp_radio_dma::{RxHandoffPool, RxNetworkLease};
use open_esp_radio_wifi_softmac::{
    MacRxMetadata, MonitorDropReason, MonitorFrame, MonitorPublishOutcome, MonitorSink,
    interface::{ChannelContextId, MonitorTapPoint},
};

/// Metadata copied alongside one independently retained monitor frame.
#[derive(Clone, Copy, Debug)]
pub struct MonitorCaptureMetadata<Rate> {
    /// Supervisor generation which produced this frame.
    pub generation: u32,
    pub tap: MonitorTapPoint,
    pub channel_context: ChannelContextId,
    pub rx: MacRxMetadata<Rate>,
    /// Complete logical MPDU length reported by hardware.
    ///
    /// This can exceed `captured_length()` when hardware stripped an
    /// authenticated trailer before exposing the normalized DMA view.
    pub logical_length: usize,
}

/// Unique consumer ownership of one copied monitor frame.
///
/// Dropping this value returns its capture slot to the radio publisher.
pub struct MonitorCaptureFrame<'pool, Rate, const CAPACITY: usize> {
    lease: RxNetworkLease<'pool, CAPACITY>,
    metadata: MonitorCaptureMetadata<Rate>,
}

impl<Rate, const CAPACITY: usize> MonitorCaptureFrame<'_, Rate, CAPACITY> {
    pub fn bytes(&self) -> &[u8] {
        self.lease.frame()
    }

    pub const fn metadata(&self) -> &MonitorCaptureMetadata<Rate> {
        &self.metadata
    }

    pub fn captured_length(&self) -> usize {
        self.bytes().len()
    }

    pub fn is_complete(&self) -> bool {
        self.metadata.logical_length == self.captured_length()
    }
}

/// Caller-owned frame storage independent from the Wi-Fi DMA ring.
pub struct MonitorCapturePool<const CAPACITY: usize, const SLOTS: usize> {
    storage: RxHandoffPool<CAPACITY, SLOTS>,
}

impl<const CAPACITY: usize, const SLOTS: usize> MonitorCapturePool<CAPACITY, SLOTS> {
    pub const fn new() -> Self {
        assert!(CAPACITY != 0, "monitor capture slots must not be empty");
        assert!(SLOTS != 0, "monitor capture pool must contain a slot");
        assert!(
            SLOTS <= u8::MAX as usize + 1,
            "monitor capture pool exceeds the lease index domain"
        );
        Self {
            storage: RxHandoffPool::new(),
        }
    }

    /// Payload bytes reserved by this pool, excluding ownership bookkeeping.
    pub const fn payload_storage_bytes() -> usize {
        CAPACITY * SLOTS
    }

    pub const fn slot_capacity() -> usize {
        CAPACITY
    }

    pub const fn slot_count() -> usize {
        SLOTS
    }

    pub fn claimed_slots(&self) -> usize {
        self.storage.claimed_slots()
    }

    fn try_capture<'pool, Rate>(
        &'pool self,
        frame: MonitorFrame<'_, Rate>,
        generation: u32,
        snapshot_length: Option<usize>,
    ) -> Result<MonitorCaptureFrame<'pool, Rate, CAPACITY>, MonitorDropReason> {
        let captured_length = snapshot_length
            .unwrap_or(frame.bytes.len())
            .min(frame.bytes.len());
        if captured_length > CAPACITY {
            return Err(MonitorDropReason::TooLong);
        }
        let mut radio = self
            .storage
            .try_claim_radio()
            .ok_or(MonitorDropReason::Full)?;
        radio
            .frame_prefix_mut(captured_length)
            .copy_from_slice(&frame.bytes[..captured_length]);
        Ok(MonitorCaptureFrame {
            lease: radio.into_network(captured_length),
            metadata: MonitorCaptureMetadata {
                generation,
                tap: frame.tap,
                channel_context: frame.channel_context,
                rx: frame.metadata,
                logical_length: frame.logical_length,
            },
        })
    }
}

impl<const CAPACITY: usize, const SLOTS: usize> Default for MonitorCapturePool<CAPACITY, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

/// Embassy queue and capture-pool binding for one monitor epoch.
///
/// `DEPTH` is a retained-frame limit, never a processing batch size. It may
/// not exceed the number of independently owned capture slots.
pub struct MonitorCaptureResources<
    'pool,
    M: RawMutex,
    Rate,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
> {
    pool: &'pool MonitorCapturePool<CAPACITY, SLOTS>,
    frames: Channel<M, MonitorCaptureFrame<'pool, Rate, CAPACITY>, DEPTH>,
}

impl<'pool, M: RawMutex, Rate, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    MonitorCaptureResources<'pool, M, Rate, DEPTH, CAPACITY, SLOTS>
{
    pub const fn new(pool: &'pool MonitorCapturePool<CAPACITY, SLOTS>) -> Self {
        assert!(DEPTH != 0, "monitor capture queue must not be empty");
        assert!(
            DEPTH <= SLOTS,
            "monitor capture queue cannot outgrow its ownership pool"
        );
        Self {
            pool,
            frames: Channel::new(),
        }
    }

    /// Split one monitor epoch into non-blocking radio and async consumer
    /// capabilities. Endpoints can be reconstructed only after the previous
    /// epoch has stopped and its retained frames have been dropped.
    pub fn split(
        &self,
    ) -> (
        MonitorCaptureSink<'_, 'pool, M, Rate, DEPTH, CAPACITY, SLOTS>,
        MonitorCaptureReceiver<'_, 'pool, M, Rate, DEPTH, CAPACITY>,
    ) {
        (
            MonitorCaptureSink {
                pool: self.pool,
                sender: self.frames.sender(),
                generation: 0,
                snapshot_length: None,
            },
            MonitorCaptureReceiver {
                receiver: self.frames.receiver(),
            },
        )
    }

    /// Drop captures which were queued by a completed monitor generation.
    pub fn discard_queued(&self) -> usize {
        let mut discarded = 0_usize;
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }
}

/// Radio-side best-effort capture endpoint.
pub struct MonitorCaptureSink<
    'queue,
    'pool,
    M: RawMutex,
    Rate,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
> {
    pool: &'pool MonitorCapturePool<CAPACITY, SLOTS>,
    sender: Sender<'queue, M, MonitorCaptureFrame<'pool, Rate, CAPACITY>, DEPTH>,
    generation: u32,
    snapshot_length: Option<usize>,
}

impl<M: RawMutex, Rate, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    MonitorCaptureSink<'_, '_, M, Rate, DEPTH, CAPACITY, SLOTS>
{
    /// Bind a reusable sink to one supervisor generation and capture policy.
    pub fn configure(&mut self, generation: u32, snapshot_length: Option<usize>) {
        self.generation = generation;
        self.snapshot_length = snapshot_length;
    }
}

impl<M: RawMutex, Rate, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    MonitorSink<Rate> for MonitorCaptureSink<'_, '_, M, Rate, DEPTH, CAPACITY, SLOTS>
{
    fn try_publish(&mut self, frame: MonitorFrame<'_, Rate>) -> MonitorPublishOutcome {
        // Avoid an otherwise pointless frame-sized copy after a slow consumer
        // has already filled the queue. A concurrent receive may make this a
        // conservative best-effort drop, which is valid for a monitor tap.
        if self.sender.is_full() {
            return MonitorPublishOutcome::Dropped(MonitorDropReason::Full);
        }
        let captured = match self
            .pool
            .try_capture(frame, self.generation, self.snapshot_length)
        {
            Ok(captured) => captured,
            Err(reason) => return MonitorPublishOutcome::Dropped(reason),
        };
        match self.sender.try_send(captured) {
            Ok(()) => MonitorPublishOutcome::Published,
            Err(TrySendError::Full(_captured)) => {
                // Dropping the returned frame restores the pool credit.
                MonitorPublishOutcome::Dropped(MonitorDropReason::Full)
            }
        }
    }
}

/// Consumer-side async capture endpoint.
pub struct MonitorCaptureReceiver<
    'queue,
    'pool,
    M: RawMutex,
    Rate,
    const DEPTH: usize,
    const CAPACITY: usize,
> {
    receiver: Receiver<'queue, M, MonitorCaptureFrame<'pool, Rate, CAPACITY>, DEPTH>,
}

impl<'queue, 'pool, M: RawMutex, Rate, const DEPTH: usize, const CAPACITY: usize>
    MonitorCaptureReceiver<'queue, 'pool, M, Rate, DEPTH, CAPACITY>
{
    pub fn try_receive(&self) -> Option<MonitorCaptureFrame<'pool, Rate, CAPACITY>> {
        self.receiver.try_receive().ok()
    }

    pub fn receive(&self) -> impl Future<Output = MonitorCaptureFrame<'pool, Rate, CAPACITY>> + '_ {
        self.receiver.receive()
    }

    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// Drop every queued capture and return its pool credits.
    ///
    /// A monitor lifecycle calls this after stopping the RX owner and before
    /// reusing the same resources for another epoch.
    pub fn discard_queued(&self) -> usize {
        let mut discarded = 0_usize;
        while let Ok(frame) = self.receiver.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }
}

#[cfg(test)]
mod tests {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_wifi_softmac::{
        MacRxMetadata,
        interface::{ChannelContextId, MonitorTapPoint},
    };

    use super::*;

    fn frame(bytes: &[u8]) -> MonitorFrame<'_, ()> {
        MonitorFrame {
            tap: MonitorTapPoint::Normalized,
            channel_context: ChannelContextId::PRIMARY,
            bytes,
            metadata: MacRxMetadata::unavailable(),
            logical_length: bytes.len(),
        }
    }

    #[test]
    fn capture_is_independent_from_the_borrowed_source() {
        let pool = MonitorCapturePool::<16, 1>::new();
        let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 16, 1>::new(&pool);
        let (mut sink, receiver) = resources.split();
        let mut source = [1, 2, 3, 4];

        assert_eq!(
            sink.try_publish(frame(&source)),
            MonitorPublishOutcome::Published
        );
        source.fill(9);
        let captured = receiver.try_receive().expect("one retained capture");
        assert_eq!(captured.bytes(), &[1, 2, 3, 4]);
        assert!(captured.is_complete());
        assert_eq!(pool.claimed_slots(), 1);
        drop(captured);
        assert_eq!(pool.claimed_slots(), 0);
    }

    #[test]
    fn full_queue_drops_the_new_capture_and_restores_its_pool_slot() {
        let pool = MonitorCapturePool::<16, 2>::new();
        let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 16, 2>::new(&pool);
        let (mut sink, receiver) = resources.split();

        assert_eq!(
            sink.try_publish(frame(&[1])),
            MonitorPublishOutcome::Published
        );
        assert_eq!(
            sink.try_publish(frame(&[2])),
            MonitorPublishOutcome::Dropped(MonitorDropReason::Full)
        );
        assert_eq!(pool.claimed_slots(), 1);
        drop(
            receiver
                .try_receive()
                .expect("first capture remains queued"),
        );
        assert_eq!(pool.claimed_slots(), 0);
    }

    #[test]
    fn epoch_cleanup_discards_queued_frames_and_restores_all_credits() {
        let pool = MonitorCapturePool::<16, 2>::new();
        let resources = MonitorCaptureResources::<NoopRawMutex, (), 2, 16, 2>::new(&pool);
        let (mut sink, receiver) = resources.split();
        assert_eq!(
            sink.try_publish(frame(&[1])),
            MonitorPublishOutcome::Published
        );
        assert_eq!(
            sink.try_publish(frame(&[2])),
            MonitorPublishOutcome::Published
        );
        assert_eq!(pool.claimed_slots(), 2);

        assert_eq!(receiver.discard_queued(), 2);
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(receiver.discard_queued(), 0);
    }

    #[test]
    fn exhausted_pool_and_oversized_frames_have_distinct_reasons() {
        let pool = MonitorCapturePool::<4, 1>::new();
        let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 4, 1>::new(&pool);
        let (mut sink, receiver) = resources.split();

        assert_eq!(
            sink.try_publish(frame(&[1, 2, 3, 4, 5])),
            MonitorPublishOutcome::Dropped(MonitorDropReason::TooLong)
        );
        assert_eq!(
            sink.try_publish(frame(&[1])),
            MonitorPublishOutcome::Published
        );
        let retained = receiver.try_receive().expect("pool owner");
        assert_eq!(
            sink.try_publish(frame(&[2])),
            MonitorPublishOutcome::Dropped(MonitorDropReason::Full)
        );
        drop(retained);
    }

    #[test]
    fn incomplete_normalized_capture_preserves_logical_length() {
        let pool = MonitorCapturePool::<8, 1>::new();
        let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 8, 1>::new(&pool);
        let (mut sink, receiver) = resources.split();
        let bytes = [1, 2, 3, 4];
        let mut observed = frame(&bytes);
        observed.logical_length = 12;

        assert_eq!(sink.try_publish(observed), MonitorPublishOutcome::Published);
        let captured = receiver.try_receive().expect("one capture");
        assert!(!captured.is_complete());
        assert_eq!(captured.metadata().logical_length, 12);
        assert_eq!(captured.captured_length(), 4);
    }

    #[test]
    fn epoch_policy_tags_and_truncates_without_changing_logical_length() {
        let pool = MonitorCapturePool::<8, 1>::new();
        let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 8, 1>::new(&pool);
        let (mut sink, receiver) = resources.split();
        sink.configure(17, Some(3));

        assert_eq!(
            sink.try_publish(frame(&[1, 2, 3, 4, 5])),
            MonitorPublishOutcome::Published
        );
        let captured = receiver.try_receive().expect("one truncated capture");
        assert_eq!(captured.bytes(), &[1, 2, 3]);
        assert_eq!(captured.metadata().generation, 17);
        assert_eq!(captured.metadata().logical_length, 5);
        assert!(!captured.is_complete());
    }

    #[test]
    fn reported_payload_storage_excludes_queue_metadata() {
        assert_eq!(
            MonitorCapturePool::<2_048, 12>::payload_storage_bytes(),
            24_576
        );
        assert_eq!(MonitorCapturePool::<2_048, 12>::slot_capacity(), 2_048);
        assert_eq!(MonitorCapturePool::<2_048, 12>::slot_count(), 12);
    }
}
