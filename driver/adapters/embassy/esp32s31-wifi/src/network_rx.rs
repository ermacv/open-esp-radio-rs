//! Bounded connected-RX publication into the Embassy network adapter.

use core::{
    future::Future,
    sync::atomic::{AtomicU32, Ordering},
};

use open_esp_radio_embassy_net::{PinnedRxPublisher, RawMutex, RxEnqueueError};
use open_esp_radio_esp32s31_wifi_mac::connected_rx::{ConnectedRxEvent, ConnectedRxSink};

use crate::{
    connected_rx_protocol::ConnectedRxProtocolSink,
    rx_observer::{RxPipelineObservation, RxPipelineObserver},
};

/// One observation of the bounded network RX publication counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxEnqueueCounterSnapshot {
    pub enqueued: u32,
    pub dropped: u32,
}

/// Optional shared receive-queue telemetry for integration and HIL policy.
///
/// The counters do not participate in admission. They only make the sink's
/// existing local accounting observable while its production owner is inside
/// a long-running [`crate::connected_runner::ConnectedRunner`].
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

/// Copies Ethernet events into the bounded network queue and forwards every
/// semantic event to a protocol observer.
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
    pipeline_observer: Option<&'resources dyn RxPipelineObserver>,
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
            pipeline_observer: None,
        }
    }

    pub fn with_counters(mut self, counters: &'resources RxEnqueueCounters) -> Self {
        self.counters = Some(counters);
        self
    }

    pub fn with_pipeline_observer(mut self, observer: &'resources dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
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
            let publish_started = self.pipeline_observer.map(|observer| observer.now_micros());
            let result = self.network.try_send_parts(
                frame.destination,
                frame.source,
                frame.ether_type,
                frame.payload,
            );
            if let (Some(observer), Some(started)) = (self.pipeline_observer, publish_started) {
                observer.observe(RxPipelineObservation::NetworkPublished {
                    bytes: frame.payload.len().saturating_add(14),
                    micros: observer.elapsed_micros_since(started),
                });
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

#[cfg(test)]
mod tests {
    use core::task::{Context, Waker};

    use open_esp_radio_embassy_net::{
        Driver as _, NoopRawMutex, PinnedResources, PinnedTxPool, RxEnqueueError,
    };
    use open_esp_radio_esp32s31_wifi_mac::connected_rx::{ConnectedRxEvent, ConnectedRxSink};
    use open_esp_radio_ieee80211::data::EthernetFrameParts;

    use super::*;

    #[derive(Default)]
    struct Observer(u32);

    impl ConnectedRxSink for Observer {
        fn publish(&mut self, _event: ConnectedRxEvent<'_>) {
            self.0 += 1;
        }
    }

    #[test]
    fn sink_has_rx_only_capability_and_reports_bounded_backpressure() {
        const FRAME_CAPACITY: usize = 64;
        const HEADROOM: usize = 32;
        const TRAILER: usize = 8;
        const QUEUE_DEPTH: usize = 1;
        type Resources =
            PinnedResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;
        type Pool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
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
            metadata: open_esp_radio_wifi_softmac::MacRxMetadata::unavailable(),
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
}
