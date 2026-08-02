//! Production ownership of the ESP32-S31 Wi-Fi RX descriptor ring.
//!
//! DMA storage, the live descriptor frontier and independent staging storage
//! are kept in one finite service. No DMA pointer escapes this owner: a
//! completed unit is copied and recycled before its staging lease is handed to
//! the separate protocol consumer.

use core::{
    cell::UnsafeCell,
    future::Future,
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_sync::channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError};
use open_esp_radio_embassy_net::{PinnedRxPublisher, RawMutex, RxEnqueueError};
use open_esp_radio_esp32s31_wifi_mac::{
    connected_rx::{ConnectedRxControlEvent, ConnectedRxEvent, ConnectedRxSink},
    descriptor::Descriptor,
    rx::{RxDma, RxRingError, RxRingLive, prepare_recycled_buffer},
    rx_pool::{
        RxStagePool, RxStageTransactionError, VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
        VENDOR_LARGE_RX_SLOT_COUNT,
    },
};

use crate::{
    backend::Esp32s31ConnectedRxService,
    embassy_rx::{RxReloadDelay, await_staged_rx_reload},
    runner::WifiRxProgress,
    rx_telemetry::RxPipelineCounters,
    staged_rx::{ConnectedRxProtocolSink, Esp32s31StagedRxFrame},
};

/// Descriptor count and allocation geometry qualified by the ordinary S31
/// large-RX profile.
pub const ESP32S31_RX_DESCRIPTOR_COUNT: usize = 32;
pub const ESP32S31_RX_BUFFER_SIZE: usize = 4_608;
pub const ESP32S31_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_SIZE + 4;

#[repr(C, align(4))]
pub struct Esp32s31RxDmaBuffer(UnsafeCell<[u8; ESP32S31_RX_BUFFER_STORAGE_SIZE]>);

impl Esp32s31RxDmaBuffer {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; ESP32S31_RX_BUFFER_STORAGE_SIZE]))
    }

    pub fn dma_address(&self) -> Result<u32, Esp32s31RxStorageError> {
        u32::try_from(self.0.get().addr()).map_err(|_| Esp32s31RxStorageError::AddressWidth)
    }

    /// The caller must own the matching completed descriptor. The returned
    /// view must not survive descriptor recycle.
    pub unsafe fn completed(&self) -> &[u8; ESP32S31_RX_BUFFER_SIZE] {
        // SAFETY: the type guarantees a prefix of exactly this size.
        unsafe { &*self.0.get().cast::<[u8; ESP32S31_RX_BUFFER_SIZE]>() }
    }

    /// The caller must own the matching completed descriptor and must invoke
    /// this only from the ring's rearm closure.
    pub unsafe fn prepare_for_recycle(&self) -> Result<(), RxRingError> {
        // SAFETY: ring ownership makes this the only CPU or DMA writer.
        unsafe { prepare_recycled_buffer(&mut *self.0.get(), ESP32S31_RX_BUFFER_SIZE) }
    }

    /// Volatile diagnostic word read after the matching descriptor completed.
    pub unsafe fn read_word(&self, offset: usize) -> u32 {
        assert!(offset + 4 <= ESP32S31_RX_BUFFER_SIZE);
        // SAFETY: the caller owns the completed descriptor and the assertion
        // bounds all four volatile byte reads.
        unsafe {
            let bytes = self.0.get().cast::<u8>().add(offset);
            u32::from_le_bytes([
                bytes.read_volatile(),
                bytes.add(1).read_volatile(),
                bytes.add(2).read_volatile(),
                bytes.add(3).read_volatile(),
            ])
        }
    }

    /// Volatile diagnostic byte read after the matching descriptor completed.
    pub unsafe fn read_byte(&self, offset: usize) -> u8 {
        assert!(offset < ESP32S31_RX_BUFFER_SIZE);
        // SAFETY: the caller owns the completed descriptor and offset is in
        // the advertised DMA prefix.
        unsafe { self.0.get().cast::<u8>().add(offset).read_volatile() }
    }
}

// SAFETY: access to the cell is admitted only through the unique live-ring
// transaction. The storage may be moved to a radio task before DMA starts.
unsafe impl Send for Esp32s31RxDmaBuffer {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxStorageError {
    AddressWidth,
}

/// Permanently located Wi-Fi descriptor and buffer storage.
///
/// The buffer address table remains caller-owned because [`RxRingLive`]
/// borrows it for its entire epoch. Keeping that table separate avoids a
/// self-referential owner and lets a platform place only DMA-visible storage
/// in its dedicated linker section.
pub struct Esp32s31RxDmaStorage<const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT> {
    descriptors: [Descriptor; COUNT],
    buffers: [Esp32s31RxDmaBuffer; COUNT],
}

impl<const COUNT: usize> Esp32s31RxDmaStorage<COUNT> {
    pub const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; COUNT],
            buffers: [const { Esp32s31RxDmaBuffer::new() }; COUNT],
        }
    }

    /// Initialize a large RX arena directly in its final allocation.
    pub fn init_in_place(storage: &mut MaybeUninit<Self>) -> &mut Self {
        let storage = storage.as_mut_ptr();
        // SAFETY: the allocation is exclusive and aligned. Each array element
        // is initialized exactly once before the reference is formed.
        unsafe {
            let descriptors = ptr::addr_of_mut!((*storage).descriptors).cast::<Descriptor>();
            let buffers = ptr::addr_of_mut!((*storage).buffers).cast::<Esp32s31RxDmaBuffer>();
            for index in 0..COUNT {
                descriptors.add(index).write(Descriptor::new());
                buffers.add(index).write(Esp32s31RxDmaBuffer::new());
            }
            &mut *storage
        }
    }

    pub const fn descriptors(&self) -> &[Descriptor; COUNT] {
        &self.descriptors
    }

    pub const fn buffers(&self) -> &[Esp32s31RxDmaBuffer; COUNT] {
        &self.buffers
    }

    pub fn dma_layout(
        &self,
        buffer_addresses: &mut [u32; COUNT],
    ) -> Result<u32, Esp32s31RxStorageError> {
        for (address, buffer) in buffer_addresses.iter_mut().zip(&self.buffers) {
            *address = buffer.dma_address()?;
        }
        u32::try_from(self.descriptors.as_ptr().addr())
            .map_err(|_| Esp32s31RxStorageError::AddressWidth)
    }
}

impl<const COUNT: usize> Default for Esp32s31RxDmaStorage<COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete production RX owner for one running descriptor-ring epoch.
pub struct Esp32s31ConnectedRx<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
> {
    ring: RxRingLive<'storage, COUNT>,
    buffers: &'storage [Esp32s31RxDmaBuffer; COUNT],
    pool: &'pool RxStagePool<VENDOR_LARGE_RX_SLOT_COUNT, VENDOR_LARGE_RX_PAYLOAD_CAPACITY>,
    frames: Sender<'queue, M, Esp32s31StagedRxFrame<'pool>, QUEUE_DEPTH>,
    delay: D,
    pipeline_counters: Option<&'pool RxPipelineCounters>,
}

impl<'storage, 'pool, 'queue, D, M: RawMutex, const QUEUE_DEPTH: usize, const COUNT: usize>
    Esp32s31ConnectedRx<'storage, 'pool, 'queue, D, M, QUEUE_DEPTH, COUNT>
{
    pub fn new(
        ring: RxRingLive<'storage, COUNT>,
        buffers: &'storage [Esp32s31RxDmaBuffer; COUNT],
        pool: &'pool RxStagePool<VENDOR_LARGE_RX_SLOT_COUNT, VENDOR_LARGE_RX_PAYLOAD_CAPACITY>,
        delay: D,
        frames: Sender<'queue, M, Esp32s31StagedRxFrame<'pool>, QUEUE_DEPTH>,
    ) -> Self {
        Self {
            ring,
            buffers,
            pool,
            frames,
            delay,
            pipeline_counters: None,
        }
    }

    pub fn with_pipeline_counters(mut self, counters: &'pool RxPipelineCounters) -> Self {
        self.pipeline_counters = Some(counters);
        self
    }

    pub const fn ring(&self) -> &RxRingLive<'storage, COUNT> {
        &self.ring
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }
}

impl<'storage, 'pool, 'queue, H, D, M: RawMutex, const QUEUE_DEPTH: usize, const COUNT: usize>
    Esp32s31ConnectedRxService<H>
    for Esp32s31ConnectedRx<'storage, 'pool, 'queue, D, M, QUEUE_DEPTH, COUNT>
where
    H: RxDma,
    D: RxReloadDelay,
{
    type Error = RxStageTransactionError;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + 'a {
        async move {
            let service_started = self.pipeline_counters.map(RxPipelineCounters::now_micros);
            // Freeze the completion frontier before any descriptor is rearmed.
            // A saturated producer can therefore only create a later epoch; it
            // cannot make this service call unbounded by refilling the ring.
            let frontier = self.ring.completed_frontier_len();
            let pool_credits = self.pool.available_slots();
            let queue_credits = self.frames.free_capacity();
            let credits = pool_credits.min(queue_credits);
            let admitted = frontier.min(credits);
            let mut staged_bytes = 0_usize;

            for _ in 0..admitted {
                let index = self.ring.recycle_start();
                let completed = self
                    .ring
                    .take_completed(index)
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                // SAFETY: `take_completed` uniquely transferred the matching
                // descriptor. `stage_recycle` copies this view before its
                // rearm closure publishes the buffer to DMA again.
                let source = unsafe { self.buffers[index].completed() };
                let pending = self.pool.stage_recycle(
                    completed,
                    source,
                    hardware,
                    &mut self.ring,
                    |recycled| {
                        // SAFETY: the ring invokes this only for its owned
                        // completed prefix immediately before publication.
                        unsafe { self.buffers[recycled].prepare_for_recycle() }
                    },
                )?;
                let frame =
                    await_staged_rx_reload(pending, hardware, &mut self.ring, &mut self.delay)
                        .await?;
                staged_bytes = staged_bytes.saturating_add(frame.length());
                self.frames.try_send(frame).map_err(|error| match error {
                    TrySendError::Full(_) => RxStageTransactionError::Ring(RxRingError::Corrupt),
                })?;
            }

            if let (Some(counters), Some(started)) = (self.pipeline_counters, service_started) {
                counters.record_service(
                    frontier,
                    pool_credits,
                    queue_credits,
                    admitted,
                    staged_bytes,
                    counters.elapsed_micros_since(started),
                );
            }

            Ok(if admitted < frontier {
                WifiRxProgress::Backpressured
            } else {
                WifiRxProgress::Drained
            })
        }
    }
}

/// Connected RX sink that copies Ethernet events into the bounded network
/// queue and forwards every semantic event to a protocol observer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxEnqueueCounterSnapshot {
    pub enqueued: u32,
    pub dropped: u32,
}

/// Optional shared receive-queue telemetry for integration and HIL policy.
///
/// The counters do not participate in admission. They only make the sink's
/// existing local accounting observable while its production owner is inside
/// a long-running [`crate::runner::WifiRunner`].
pub struct RxEnqueueCounters {
    enqueued: AtomicU32,
    dropped: AtomicU32,
}

impl RxEnqueueCounters {
    pub const fn new() -> Self {
        Self {
            enqueued: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
        }
    }

    pub fn snapshot(&self) -> RxEnqueueCounterSnapshot {
        RxEnqueueCounterSnapshot {
            enqueued: self.enqueued.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

impl Default for RxEnqueueCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub struct EmbassyNetConnectedRxSink<
    'resources,
    M: RawMutex,
    O,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    network: PinnedRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>,
    observer: O,
    enqueued: u32,
    dropped: u32,
    last_enqueue_error: Option<RxEnqueueError>,
    counters: Option<&'resources RxEnqueueCounters>,
    pipeline_counters: Option<&'resources RxPipelineCounters>,
}

impl<'resources, M: RawMutex, O, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    EmbassyNetConnectedRxSink<'resources, M, O, FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub const fn new(
        network: PinnedRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>,
        observer: O,
    ) -> Self {
        Self {
            network,
            observer,
            enqueued: 0,
            dropped: 0,
            last_enqueue_error: None,
            counters: None,
            pipeline_counters: None,
        }
    }

    pub fn with_counters(mut self, counters: &'resources RxEnqueueCounters) -> Self {
        self.counters = Some(counters);
        self
    }

    pub fn with_pipeline_counters(mut self, counters: &'resources RxPipelineCounters) -> Self {
        self.pipeline_counters = Some(counters);
        self
    }

    pub const fn enqueued(&self) -> u32 {
        self.enqueued
    }

    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    pub const fn last_enqueue_error(&self) -> Option<RxEnqueueError> {
        self.last_enqueue_error
    }

    pub const fn observer(&self) -> &O {
        &self.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.observer
    }
}

impl<M: RawMutex, O: ConnectedRxSink, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    ConnectedRxSink for EmbassyNetConnectedRxSink<'_, M, O, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { frame, .. } = event {
            let publish_started = self.pipeline_counters.map(RxPipelineCounters::now_micros);
            let result = self.network.try_send_parts(
                frame.destination,
                frame.source,
                frame.ether_type,
                frame.payload,
            );
            if let (Some(counters), Some(started)) = (self.pipeline_counters, publish_started) {
                counters.record_network_publish(
                    frame.payload.len().saturating_add(14),
                    counters.elapsed_micros_since(started),
                );
            }
            match result {
                Ok(()) => {
                    self.enqueued = self.enqueued.saturating_add(1);
                    if let Some(counters) = self.counters {
                        counters.enqueued.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(error) => {
                    self.dropped = self.dropped.saturating_add(1);
                    self.last_enqueue_error = Some(error);
                    if let Some(counters) = self.counters {
                        counters.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        self.observer.publish(event);
    }
}

impl<M: RawMutex, O: ConnectedRxSink, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    ConnectedRxProtocolSink for EmbassyNetConnectedRxSink<'_, M, O, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.network.wait_ready()
    }
}

/// Explicit observer for profiles that intentionally ignore control-plane
/// events. Production association/BlockAck state should supply a real sink.
pub struct IgnoreConnectedControl;

impl ConnectedRxSink for IgnoreConnectedControl {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
}

/// Fixed mailbox between borrowed frame dispatch and the async/PAC control
/// owner. Overflow is explicit evidence; it never allocates or overwrites an
/// older action silently.
pub struct ConnectedControlQueue<const CAPACITY: usize> {
    events: [Option<ConnectedRxControlEvent>; CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
    dropped: u32,
}

fn scheduled_connected_control(event: ConnectedRxEvent<'_>) -> Option<ConnectedRxControlEvent> {
    match event.control()? {
        // These are the only event classes consumed by
        // `Esp32s31ConnectedControl` today. High-rate protected-data
        // observations and diagnostic Trigger/NDPA events must not starve a
        // beacon or ADDBA/DELBA transition in the bounded mailbox.
        event @ (ConnectedRxControlEvent::Beacon(_) | ConnectedRxControlEvent::BlockAck(_)) => {
            Some(event)
        }
        ConnectedRxControlEvent::ProtectedFrame(_)
        | ConnectedRxControlEvent::Trigger { .. }
        | ConnectedRxControlEvent::Ndpa { .. } => None,
    }
}

impl<const CAPACITY: usize> ConnectedControlQueue<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            events: [None; CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
            dropped: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    pub fn pop(&mut self) -> Option<ConnectedRxControlEvent> {
        if self.len == 0 || CAPACITY == 0 {
            return None;
        }
        let event = self.events[self.head].take()?;
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        Some(event)
    }
}

impl<const CAPACITY: usize> ConnectedRxSink for ConnectedControlQueue<CAPACITY> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let Some(event) = scheduled_connected_control(event) else {
            return;
        };
        if CAPACITY == 0 || self.len == CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.events[self.tail] = Some(event);
        self.tail = (self.tail + 1) % CAPACITY;
        self.len += 1;
    }
}

impl<const CAPACITY: usize> Default for ConnectedControlQueue<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Static Embassy mailbox for owned connected control events.
pub struct ConnectedControlResources<M: RawMutex, const CAPACITY: usize> {
    channel: Channel<M, ConnectedRxControlEvent, CAPACITY>,
    dropped: AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedControlResources<M, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            channel: Channel::new(),
            dropped: AtomicU32::new(0),
        }
    }

    /// Split into a receive-dispatch publisher and the unique scheduler-side
    /// consumer. The mutable borrow prevents a second split while either
    /// capability remains live.
    pub fn split(
        &mut self,
    ) -> (
        ConnectedControlPublisher<'_, M, CAPACITY>,
        ConnectedControlReceiver<'_, M, CAPACITY>,
    ) {
        let resources: &Self = self;
        (
            ConnectedControlPublisher {
                sender: resources.channel.sender(),
                dropped: &resources.dropped,
            },
            ConnectedControlReceiver {
                receiver: resources.channel.receiver(),
                dropped: &resources.dropped,
            },
        )
    }
}

impl<M: RawMutex, const CAPACITY: usize> Default for ConnectedControlResources<M, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// RX-dispatch capability; it can publish but cannot consume or execute a
/// control action.
#[derive(Clone, Copy)]
pub struct ConnectedControlPublisher<'resources, M: RawMutex, const CAPACITY: usize> {
    sender: Sender<'resources, M, ConnectedRxControlEvent, CAPACITY>,
    dropped: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedRxSink
    for ConnectedControlPublisher<'_, M, CAPACITY>
{
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let Some(event) = scheduled_connected_control(event) else {
            return;
        };
        if let Err(TrySendError::Full(_)) = self.sender.try_send(event) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Scheduler-side control capability; it cannot publish borrowed RX data.
pub struct ConnectedControlReceiver<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: Receiver<'resources, M, ConnectedRxControlEvent, CAPACITY>,
    dropped: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedControlReceiver<'_, M, CAPACITY> {
    pub fn try_receive(&self) -> Option<ConnectedRxControlEvent> {
        match self.receiver.try_receive() {
            Ok(event) => Some(event),
            Err(TryReceiveError::Empty) => None,
        }
    }

    pub fn ready(&self) -> impl Future<Output = ()> + '_ {
        self.receiver.ready_to_receive()
    }

    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::ready,
        mem::MaybeUninit,
        task::{Context, Waker},
    };

    use open_esp_radio_embassy_net::{Driver as _, NoopRawMutex, PinnedResources, PinnedTxPool};
    use open_esp_radio_esp32s31_wifi_mac::{
        connected_rx::{ConnectedRxConfig, ConnectedRxDispatcher, ConnectedRxEvent},
        descriptor::{BIT_30, BIT_31, DESCRIPTOR_BYTES, LENGTH_SHIFT},
        rx::{RxIngressConfig, RxRingStopped},
        tx_ampdu::BlockAckAction,
    };
    use open_esp_radio_ieee80211::data::EthernetFrameParts;

    use super::*;
    use crate::{
        embassy_irq::EmbassyMacIrqRuntime,
        staged_rx::{Esp32s31ConnectedRxProtocol, Esp32s31StagedRxQueue},
    };

    const BASE: u32 = 0x2f00_1000;

    #[derive(Default)]
    struct MockRxDma {
        walker: bool,
        descriptor_base: u32,
    }

    impl RxDma for MockRxDma {
        fn last_descriptor_low(&mut self) -> u32 {
            0
        }
        fn next_descriptor_low(&mut self) -> u32 {
            BASE + DESCRIPTOR_BYTES
        }
        fn walker_enabled(&mut self) -> bool {
            self.walker
        }
        fn reload_pending(&mut self) -> bool {
            false
        }
        fn set_descriptor_high_window(&mut self, _address_high: u16) {}
        fn write_descriptor_base(&mut self, address: u32) {
            self.descriptor_base = address;
        }
        fn publish_walker_enable(&mut self) {
            self.walker = true;
        }
        fn request_reload(&mut self) {}
        fn try_enable_walker(&mut self) -> bool {
            self.walker = true;
            true
        }
        fn try_disable_walker(&mut self) -> bool {
            self.walker = false;
            true
        }
        fn fence(&mut self) {}
    }

    struct NoDelay;

    impl RxReloadDelay for NoDelay {
        fn after_micros(&mut self, _micros: u32) -> impl Future<Output = ()> + '_ {
            ready(())
        }
    }

    #[derive(Default)]
    struct Observer(u32);

    impl ConnectedRxSink for Observer {
        fn publish(&mut self, _event: ConnectedRxEvent<'_>) {
            self.0 += 1;
        }
    }

    fn dispatcher() -> ConnectedRxDispatcher {
        ConnectedRxDispatcher::new(ConnectedRxConfig {
            station_address: [2, 3, 4, 5, 6, 7],
            bssid: [8, 9, 10, 11, 12, 13],
            association_id: 1,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        })
    }

    #[test]
    fn finite_service_uses_queue_credits_and_protocol_dispatch_returns_ownership() {
        const COUNT: usize = 2;
        const STAGED_DEPTH: usize = 1;
        let storage = Esp32s31RxDmaStorage::<COUNT>::new();
        let addresses = [0x2f00_2000, 0x2f00_3200];
        let mut hardware = MockRxDma::default();
        let stopped = RxRingStopped::prepare(
            &mut hardware,
            storage.descriptors(),
            BASE,
            &addresses,
            ESP32S31_RX_BUFFER_SIZE as u32,
            |_| Ok(()),
        )
        .unwrap();
        let ring = stopped.start(&mut hardware).unwrap();
        storage.descriptors()[0]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
        storage.descriptors()[1]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
        let pool = RxStagePool::new();
        let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
        let (sender, receiver) = queue.split();
        let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let mut mpdu = [0; ESP32S31_RX_BUFFER_SIZE];
        let mut ethernet = [0; ESP32S31_RX_BUFFER_SIZE];
        let mut service = Esp32s31ConnectedRx::new(ring, storage.buffers(), &pool, NoDelay, sender);
        let mut protocol = Esp32s31ConnectedRxProtocol::new(
            receiver,
            &irq,
            dispatcher(),
            crate::staged_rx::AlwaysReadyConnectedRxSink(Observer::default()),
            &mut mpdu,
            &mut ethernet,
        );

        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Backpressured),
        );
        assert_eq!(pool.claimed_slots(), 1);
        assert_eq!(pool.network_slots(), 1);
        assert_eq!(protocol.queue_len(), 1);
        embassy_futures::block_on(protocol.dispatch_next());
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(pool.network_slots(), 0);
        assert_eq!(service.ring().recycle_start(), 1);
        assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
        assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);

        assert_eq!(
            embassy_futures::block_on(service.service(&mut hardware)),
            Ok(WifiRxProgress::Drained),
        );
        assert_eq!(service.ring().recycle_start(), 0);
        assert_eq!(protocol.queue_len(), 1);
        embassy_futures::block_on(protocol.dispatch_next());
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(pool.network_slots(), 0);
    }

    #[test]
    fn network_sink_has_rx_only_capability_and_reports_bounded_backpressure() {
        const FRAME_CAPACITY: usize = 64;
        const HEADROOM: usize = 32;
        const TRAILER: usize = 8;
        const QUEUE_DEPTH: usize = 1;
        type Resources =
            PinnedResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
        type Pool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

        let resources =
            std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Resources>::uninit()));
        let resources = Resources::init_in_place(resources);
        let pool = std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::<Pool>::uninit()));
        let pool = Pool::pin_static(Pool::init_in_place(pool));
        let (mut device, runner) = resources.split(pool, [2, 3, 4, 5, 6, 7]);
        let counters = RxEnqueueCounters::new();
        let mut sink = EmbassyNetConnectedRxSink::new(runner.rx_publisher(), Observer::default())
            .with_counters(&counters);
        let ethernet = [0_u8; 14];
        let event = ConnectedRxEvent::Ethernet {
            frame: EthernetFrameParts {
                destination: [0; 6],
                source: [0; 6],
                ether_type: 0,
                payload: &[],
            },
            raw: &ethernet,
            amsdu: false,
        };

        sink.publish(event);
        sink.publish(event);

        assert_eq!(sink.enqueued(), 1);
        assert_eq!(sink.dropped(), 1);
        assert_eq!(sink.last_enqueue_error(), Some(RxEnqueueError::QueueFull));
        assert_eq!(
            counters.snapshot(),
            RxEnqueueCounterSnapshot {
                enqueued: 1,
                dropped: 1,
            }
        );
        assert_eq!(sink.observer().0, 2);
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(device.receive(&mut context), Some(_)));
        assert!(matches!(device.receive(&mut context), None));
    }

    #[test]
    fn control_queue_copies_actions_but_never_borrowed_ethernet() {
        let body = [3, 2, 0, 0, 0, 0];
        let action = BlockAckAction::Delba {
            tid: 0,
            initiator: true,
            reason: 37,
        };
        let mut queue = ConnectedControlQueue::<1>::new();

        queue.publish(ConnectedRxEvent::Ethernet {
            frame: EthernetFrameParts {
                destination: [0; 6],
                source: [0; 6],
                ether_type: 0,
                payload: &[],
            },
            raw: &[0; 14],
            amsdu: false,
        });
        queue.publish(ConnectedRxEvent::BlockAck {
            action,
            body: &body,
        });
        queue.publish(ConnectedRxEvent::ProtectedFrame(
            open_esp_radio_esp32s31_wifi_mac::connected_rx::ConnectedRxProtection::Pairwise,
        ));

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.dropped(), 0);
        assert_eq!(queue.pop(), Some(ConnectedRxControlEvent::BlockAck(action)));
        assert!(queue.is_empty());
    }

    #[test]
    fn embassy_control_capabilities_preserve_fifo_and_report_overflow() {
        let mut resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
        let (mut publisher, receiver) = resources.split();
        let first = BlockAckAction::Delba {
            tid: 1,
            initiator: true,
            reason: 2,
        };
        let second = BlockAckAction::Delba {
            tid: 3,
            initiator: false,
            reason: 4,
        };

        publisher.publish(ConnectedRxEvent::BlockAck {
            action: first,
            body: &[3, 2, 0, 0, 2, 0],
        });
        publisher.publish(ConnectedRxEvent::BlockAck {
            action: second,
            body: &[3, 2, 0, 0, 4, 0],
        });

        assert_eq!(receiver.len(), 1);
        assert_eq!(receiver.dropped(), 1);
        assert_eq!(
            receiver.try_receive(),
            Some(ConnectedRxControlEvent::BlockAck(first))
        );
        assert_eq!(receiver.try_receive(), None);
    }
}
