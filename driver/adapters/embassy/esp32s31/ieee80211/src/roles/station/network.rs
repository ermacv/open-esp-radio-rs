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

#[cfg(all(test, feature = "owned-network"))]
mod tests;
