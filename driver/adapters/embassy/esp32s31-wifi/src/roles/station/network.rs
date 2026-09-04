#![expect(
    clippy::manual_async_fn,
    reason = "network publication keeps the adapter trait's explicit borrowed Future contract"
)]

//! Bounded connected-RX publication into the Embassy network adapter.

use core::{
    future::{Future, poll_fn},
    marker::PhantomData,
};

use open_esp_radio_esp32s31_wifi_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};

#[cfg(feature = "diagnostics")]
use crate::diagnostics::network::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::rx_pipeline::{
    RxNetworkPublicationOutcome, RxPipelineObservation, RxPipelineObserver,
};
use crate::{
    datapath::network::DatapathNetworkRx,
    datapath::rx::staging::{
        Esp32s31StagedRxFrame, StagedEthernetPublication, StagedRxDisposition,
    },
    roles::station::rx_protocol::ConnectedRxProtocolSink,
    roles::station::rx_protocol::StagedRxAdmission,
};

/// Copies Ethernet events into the bounded network queue and forwards every
/// semantic event to a protocol observer.
pub struct EmbassyNetConnectedRxSink<'resources, N, O> {
    network: N,
    observer: O,
    _resources: PhantomData<&'resources ()>,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'resources dyn RxPipelineObserver>,
    #[cfg(feature = "diagnostics")]
    delivery_observer: Option<&'resources dyn RxNetworkDeliveryObserver>,
}

impl<'resources, N, O> EmbassyNetConnectedRxSink<'resources, N, O> {
    pub const fn new(network: N, observer: O) -> Self {
        Self {
            network,
            observer,
            _resources: PhantomData,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: None,
            #[cfg(feature = "diagnostics")]
            delivery_observer: None,
        }
    }
}

impl<'resources, N, O> EmbassyNetConnectedRxSink<'resources, N, O> {
    #[cfg(feature = "diagnostics")]
    pub fn with_delivery_observer(
        mut self,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<
            &'resources dyn RxNetworkDeliveryObserver,
        >,
    ) -> Self {
        self.delivery_observer = delivery_observer;
        self
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_pipeline_observer(mut self, observer: &'resources dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    pub const fn observer(&self) -> &O {
        &self.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.observer
    }
}

impl<N: DatapathNetworkRx, O: ConnectedRxSink> ConnectedRxSink
    for EmbassyNetConnectedRxSink<'_, N, O>
{
    fn wants_power_save_delivery(&self) -> bool {
        self.observer.wants_power_save_delivery()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet {
            frame,
            #[cfg(feature = "diagnostics")]
            raw,
            ..
        } = event
        {
            // Connected-state EAPOL belongs to the WPA2 control owner. It is
            // still forwarded to `observer` below, but must never escape into
            // embassy-net as application traffic.
            if frame.ether_type == 0x888e {
                self.observer.publish(event);
                return;
            }
            #[cfg(any(feature = "diagnostics", test))]
            let publish_started = self.pipeline_observer.map(|observer| observer.now_micros());
            #[cfg(not(feature = "diagnostics"))]
            let result = self.network.try_send_parts(frame);
            #[cfg(feature = "diagnostics")]
            let result = {
                let delivery_observer = self.delivery_observer;
                self.network.try_send_parts_observed(frame, &mut || {
                    if let Some(observer) = delivery_observer {
                        observer.admitted(RxNetworkDeliveryEvent::decoded(frame, Some(raw)));
                    }
                })
            };
            #[cfg(any(feature = "diagnostics", test))]
            let outcome = match result {
                Ok(()) => RxNetworkPublicationOutcome::Enqueued,
                Err(_error) => {
                    #[cfg(feature = "diagnostics")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(RxNetworkDeliveryEvent::decoded(frame, Some(raw)), _error);
                    }
                    RxNetworkPublicationOutcome::Dropped
                }
            };
            #[cfg(not(any(feature = "diagnostics", test)))]
            let _ = result;
            #[cfg(any(feature = "diagnostics", test))]
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

    fn supports_esp_now_v2(&self) -> bool {
        self.observer.supports_esp_now_v2()
    }

    fn publish_esp_now_v2(
        &mut self,
        received: open_esp_radio_wifi_softmac::EspNowReceivedV2<'_>,
        metadata: open_esp_radio_wifi_softmac::MacRxMetadata<
            open_esp_radio_esp32s31_wifi_mac::rx::RxPhyInfo,
        >,
    ) {
        self.observer.publish_esp_now_v2(received, metadata);
    }
}

impl<
    N: DatapathNetworkRx,
    O: ConnectedRxSink,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> ConnectedRxProtocolSink<STAGE_CAPACITY, STAGE_SLOTS> for EmbassyNetConnectedRxSink<'_, N, O>
{
    fn staged_rx_admission(&self) -> StagedRxAdmission {
        StagedRxAdmission::AwaitCapacity
    }

    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        poll_fn(|context| self.network.poll_ready(context))
    }

    fn wait_staged_ready(&mut self) -> impl Future<Output = ()> + '_ {
        poll_fn(|context| self.network.poll_ready(context))
    }

    fn publish_staged(
        &mut self,
        frame: Esp32s31StagedRxFrame<'_, STAGE_CAPACITY, STAGE_SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
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
            self.publish(event);
        }
        drop(frame);
        StagedRxDisposition::Released
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU64, Ordering};
    use open_esp_radio_embassy_net::{NetworkInterfaceId, NoopRawMutex, OwnedEndpointResources};
    use open_esp_radio_esp32s31_wifi_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};
    use open_esp_radio_ieee80211::data::EthernetFrameParts;
    use xarxa_driver::{PacketPool, PacketPoolStorage};

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
        const QUEUE_DEPTH: usize = 1;
        type Resources = OwnedEndpointResources<NoopRawMutex, QUEUE_DEPTH, QUEUE_DEPTH>;

        let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
        let rx_storage =
            std::boxed::Box::leak(std::boxed::Box::new(PacketPoolStorage::<QUEUE_DEPTH>::new()));
        let rx_pool = std::boxed::Box::leak(std::boxed::Box::new(PacketPool::new(rx_storage)));
        let (mut device, runner) = resources.split(
            NetworkInterfaceId::new(0),
            [2, 3, 4, 5, 6, 7],
            rx_pool.allocator(),
        );
        runner.link_controller().set_link_up(true);
        let pipeline_observer = PipelineObserver::default();
        let mut sink = EmbassyNetConnectedRxSink::new(runner.rx_publisher(), Observer::default())
            .with_pipeline_observer(&pipeline_observer);
        assert_eq!(
            <EmbassyNetConnectedRxSink<'_, _, _> as ConnectedRxProtocolSink<
                FRAME_CAPACITY,
                QUEUE_DEPTH,
            >>::staged_rx_admission(&sink),
            StagedRxAdmission::AwaitCapacity
        );
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
        assert!(device.receive().is_some());
        assert!(device.receive().is_none());

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
        assert_eq!(sink.observer().0, 3);
        assert!(device.receive().is_none());
    }
}
