//! Bounded connected-RX publication into the Embassy network adapter.

use core::future::{Future, ready};

use open_esp_radio_embassy_net::{
    PinnedRxPublisher, RawMutex, RxEnqueueError, SharedPinnedRxPublisher,
};
use open_esp_radio_esp32s31_wifi_mac::connected_rx::{ConnectedRxEvent, ConnectedRxSink};

use crate::{
    connected_rx_protocol::{
        ConnectedRxProtocolSink, Esp32s31StagedRxFrame, StagedEthernetPublication,
        StagedRxDisposition,
    },
    rx_pipeline_observer::{
        RxNetworkPublicationOutcome, RxPipelineObservation, RxPipelineObserver,
    },
};

/// Qualification-only observation of the exact network admission decision.
///
/// This interface and its publication hook are absent unless the explicit
/// `rx-delivery-observation` feature is enabled. Diagnostic implementations
/// must remain finite and non-blocking.
#[cfg(feature = "rx-delivery-observation")]
pub trait RxNetworkDeliveryObserver: Sync {
    fn admitted(&self, event: &ConnectedRxEvent<'_>);

    fn dropped(&self, event: &ConnectedRxEvent<'_>, error: RxEnqueueError);
}

/// Copies Ethernet events into the bounded network queue and forwards every
/// semantic event to a protocol observer.
pub struct EmbassyNetConnectedRxSink<
    'resources,
    M: RawMutex,
    O,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize = FRAME_CAPACITY,
    const STAGE_SLOTS: usize = QUEUE_DEPTH,
> {
    network: PinnedRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>,
    shared: Option<SharedPinnedRxPublisher<'resources, M, STAGE_SLOTS>>,
    observer: O,
    enqueued: u32,
    dropped: u32,
    last_enqueue_error: Option<RxEnqueueError>,
    pipeline_observer: Option<&'resources dyn RxPipelineObserver>,
    #[cfg(feature = "rx-delivery-observation")]
    delivery_observer: Option<&'resources dyn RxNetworkDeliveryObserver>,
}

impl<'resources, M: RawMutex, O, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    EmbassyNetConnectedRxSink<
        'resources,
        M,
        O,
        FRAME_CAPACITY,
        QUEUE_DEPTH,
        FRAME_CAPACITY,
        QUEUE_DEPTH,
    >
{
    pub const fn new(
        network: PinnedRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>,
        observer: O,
    ) -> Self {
        Self {
            network,
            shared: None,
            observer,
            enqueued: 0,
            dropped: 0,
            last_enqueue_error: None,
            pipeline_observer: None,
            #[cfg(feature = "rx-delivery-observation")]
            delivery_observer: None,
        }
    }
}

impl<
    'resources,
    M: RawMutex,
    O,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
>
    EmbassyNetConnectedRxSink<
        'resources,
        M,
        O,
        FRAME_CAPACITY,
        QUEUE_DEPTH,
        STAGE_CAPACITY,
        STAGE_SLOTS,
    >
{
    /// Create a zero-copy sink whose network and staging capacities may differ.
    pub const fn new_with_shared_rx(
        network: PinnedRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>,
        shared: SharedPinnedRxPublisher<'resources, M, STAGE_SLOTS>,
        observer: O,
    ) -> Self {
        Self {
            network,
            shared: Some(shared),
            observer,
            enqueued: 0,
            dropped: 0,
            last_enqueue_error: None,
            pipeline_observer: None,
            #[cfg(feature = "rx-delivery-observation")]
            delivery_observer: None,
        }
    }

    #[cfg(feature = "rx-delivery-observation")]
    pub fn with_delivery_observer(
        mut self,
        delivery_observer: Option<&'resources dyn RxNetworkDeliveryObserver>,
    ) -> Self {
        self.delivery_observer = delivery_observer;
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

impl<
    M: RawMutex,
    O: ConnectedRxSink,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> ConnectedRxSink
    for EmbassyNetConnectedRxSink<
        '_,
        M,
        O,
        FRAME_CAPACITY,
        QUEUE_DEPTH,
        STAGE_CAPACITY,
        STAGE_SLOTS,
    >
{
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { frame, .. } = event {
            // Connected-state EAPOL belongs to the WPA2 control owner. It is
            // still forwarded to `observer` below, but must never escape into
            // embassy-net as application traffic.
            if frame.ether_type == 0x888e {
                self.observer.publish(event);
                return;
            }
            let publish_started = self.pipeline_observer.map(|observer| observer.now_micros());
            #[cfg(not(feature = "rx-delivery-observation"))]
            let result = self.network.try_send_parts(
                frame.destination,
                frame.source,
                frame.ether_type,
                frame.payload,
            );
            #[cfg(feature = "rx-delivery-observation")]
            let result = {
                let delivery_observer = self.delivery_observer;
                self.network.try_send_parts_observed(
                    frame.destination,
                    frame.source,
                    frame.ether_type,
                    frame.payload,
                    || {
                        if let Some(observer) = delivery_observer {
                            observer.admitted(&event);
                        }
                    },
                )
            };
            let outcome = match result {
                Ok(()) => {
                    self.enqueued = self.enqueued.saturating_add(1);
                    RxNetworkPublicationOutcome::Enqueued
                }
                Err(error) => {
                    #[cfg(feature = "rx-delivery-observation")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(&event, error);
                    }
                    self.dropped = self.dropped.saturating_add(1);
                    self.last_enqueue_error = Some(error);
                    RxNetworkPublicationOutcome::Dropped
                }
            };
            if let (Some(observer), Some(started)) = (self.pipeline_observer, publish_started) {
                observer.observe(RxPipelineObservation::NetworkPublication {
                    bytes: frame.payload.len().saturating_add(14),
                    micros: observer.elapsed_micros_since(started),
                    outcome,
                });
            }
        }
        self.observer.publish(event);
    }
}

impl<
    M: RawMutex,
    O: ConnectedRxSink,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> ConnectedRxProtocolSink<STAGE_CAPACITY, STAGE_SLOTS>
    for EmbassyNetConnectedRxSink<
        '_,
        M,
        O,
        FRAME_CAPACITY,
        QUEUE_DEPTH,
        STAGE_CAPACITY,
        STAGE_SLOTS,
    >
{
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.network.wait_ready()
    }

    fn wait_staged_ready(&mut self) -> impl Future<Output = ()> + '_ {
        async move {
            if self.shared.is_none() {
                self.network.wait_ready().await;
            } else {
                ready(()).await;
            }
        }
    }

    fn publish_staged(
        &mut self,
        frame: Esp32s31StagedRxFrame<'_, STAGE_CAPACITY, STAGE_SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
        let Some(shared) = self.shared else {
            {
                let raw = frame.segment().buffer;
                let payload = &raw
                    [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
                self.publish(ConnectedRxEvent::Ethernet {
                    frame: open_esp_radio_ieee80211::data::EthernetFrameParts {
                        destination: ethernet.destination,
                        source: ethernet.source,
                        ether_type: ethernet.ether_type,
                        payload,
                    },
                    raw,
                    amsdu: false,
                    metadata: ethernet.metadata,
                });
            }
            drop(frame);
            return StagedRxDisposition::Released;
        };

        let publish_started = self.pipeline_observer.map(|observer| observer.now_micros());
        {
            let raw = frame.segment().buffer;
            let payload =
                &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
            let event = ConnectedRxEvent::Ethernet {
                frame: open_esp_radio_ieee80211::data::EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                },
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            };
            if ethernet.ether_type == 0x888e {
                self.observer.publish(event);
                drop(frame);
                return StagedRxDisposition::Released;
            }
            #[cfg(feature = "rx-delivery-observation")]
            if let Some(observer) = self.delivery_observer {
                observer.admitted(&event);
            }
            self.observer.publish(event);
        }

        let ethernet_length = ethernet.payload_length.saturating_add(14);
        let index = match frame.publish_ethernet_in_place(
            ethernet.destination,
            ethernet.source,
            ethernet.ether_type,
            ethernet.payload_offset,
            ethernet.payload_length,
        ) {
            Ok(index) => index,
            Err((_frame, error)) => {
                unreachable!("validated staged Ethernet publication failed: {error:?}")
            }
        };
        shared.publish(index);
        self.enqueued = self.enqueued.saturating_add(1);
        if let (Some(observer), Some(started)) = (self.pipeline_observer, publish_started) {
            observer.observe(RxPipelineObservation::NetworkPublication {
                bytes: ethernet_length,
                micros: observer.elapsed_micros_since(started),
                outcome: RxNetworkPublicationOutcome::Enqueued,
            });
        }
        StagedRxDisposition::RetainedByNetwork
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU64, Ordering};
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

    #[derive(Default)]
    struct PipelineObserver {
        clock: AtomicU64,
        observations: std::sync::Mutex<std::vec::Vec<RxPipelineObservation>>,
    }

    impl RxPipelineObserver for PipelineObserver {
        fn now_micros(&self) -> u64 {
            self.clock.fetch_add(1, Ordering::Relaxed)
        }

        fn observe(&self, observation: RxPipelineObservation) {
            self.observations.lock().unwrap().push(observation);
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
        let pipeline_observer = PipelineObserver::default();
        let mut sink = EmbassyNetConnectedRxSink::<
            _,
            _,
            FRAME_CAPACITY,
            QUEUE_DEPTH,
            FRAME_CAPACITY,
            QUEUE_DEPTH,
        >::new(runner.rx_publisher(), Observer::default())
        .with_pipeline_observer(&pipeline_observer);
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
            *pipeline_observer.observations.lock().unwrap(),
            [
                RxPipelineObservation::NetworkPublication {
                    bytes: 14,
                    micros: 1,
                    outcome: RxNetworkPublicationOutcome::Enqueued,
                },
                RxPipelineObservation::NetworkPublication {
                    bytes: 14,
                    micros: 1,
                    outcome: RxNetworkPublicationOutcome::Dropped,
                },
            ]
        );
        assert_eq!(sink.observer().0, 2);
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(device.receive(&mut context), Some(_)));
        assert!(matches!(device.receive(&mut context), None));

        let eapol = ConnectedRxEvent::Ethernet {
            frame: EthernetFrameParts {
                destination: [0; 6],
                source: [1; 6],
                ether_type: 0x888e,
                payload: &[1, 2, 3],
            },
            raw: &ethernet,
            amsdu: false,
            metadata: open_esp_radio_wifi_softmac::MacRxMetadata::unavailable(),
        };
        sink.publish(eapol);
        assert_eq!(sink.enqueued(), 1);
        assert_eq!(sink.dropped(), 1);
        assert_eq!(sink.observer().0, 3);
        assert!(matches!(device.receive(&mut context), None));
    }
}
