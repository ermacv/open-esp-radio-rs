#![expect(
    clippy::manual_async_fn,
    reason = "the station RX role keeps its borrowed Future service contract explicit"
)]

//! Connected-station RX role for a common STA+AP DATAPATH owner.

use core::future::{Future, ready};

use open_esp_radio_embassy_net::{FrameLengthError, RxEnqueueError};
use open_esp_radio_esp32s31_wifi_mac::rx_pool::VENDOR_LARGE_RX_PAYLOAD_CAPACITY;
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{
    ConnectedRxDispatch, ConnectedRxEvent, ConnectedRxSink,
};

use super::{Esp32s31StaApStationRxRole, Esp32s31StagedRxFrame};
use crate::{
    datapath::rx::ethernet::{PackedEthernetWriter, record_at},
    datapath::{DatapathRxProgress, network::DatapathNetworkRx},
    roles::station::rx_protocol::{ConnectedRxProtocolSink, Esp32s31ConnectedRxProcessor},
};

/// Finite station publication failure at the paired DATAPATH boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApStationRxError {
    BatchCapacity,
    Network(FrameLengthError),
}

/// Role-local decoded Ethernet batch plus the station control observer.
///
/// This sink owns no network publisher. It lets the protocol processor finish
/// one staging lease and retains only copied Ethernet facts until the common
/// DATAPATH lends the addressed station endpoint.
pub struct Esp32s31StaApStationRxSink<'storage, O> {
    storage: &'storage mut [u8],
    used: usize,
    offset: usize,
    failed: bool,
    observer: O,
}

impl<'storage, O> Esp32s31StaApStationRxSink<'storage, O> {
    pub fn new(storage: &'storage mut [u8], observer: O) -> Self {
        assert!(
            storage.len() >= VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
            "paired STA Ethernet batch must cover one complete staged RX unit"
        );
        Self {
            storage,
            used: 0,
            offset: 0,
            failed: false,
            observer,
        }
    }

    pub const fn observer(&self) -> &O {
        &self.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.observer
    }

    pub const fn has_pending(&self) -> bool {
        self.failed || self.offset != self.used
    }

    /// Return the role-local batch allocation and fact observer after the
    /// paired protocol processor has stopped.
    pub fn into_parts(self) -> (&'storage mut [u8], O) {
        (self.storage, self.observer)
    }

    fn publish_pending(
        &mut self,
        network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, Esp32s31StaApStationRxError> {
        if self.failed {
            return Err(Esp32s31StaApStationRxError::BatchCapacity);
        }
        while let Some(record) = record_at(self.storage, self.used, self.offset)
            .map_err(|_| Esp32s31StaApStationRxError::BatchCapacity)?
        {
            match network.try_send_parts(record.frame) {
                Ok(()) => self.offset = record.next_offset,
                Err(RxEnqueueError::QueueFull) => {
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }
                Err(RxEnqueueError::InvalidLength(error)) => {
                    return Err(Esp32s31StaApStationRxError::Network(error));
                }
            }
        }
        self.used = 0;
        self.offset = 0;
        Ok(DatapathRxProgress::Drained)
    }
}

impl<O: ConnectedRxSink> ConnectedRxSink for Esp32s31StaApStationRxSink<'_, O> {
    fn wants_power_save_delivery(&self) -> bool {
        self.observer.wants_power_save_delivery()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { frame, .. } = event
            && frame.ether_type != 0x888e
        {
            let result =
                PackedEthernetWriter::resume(self.storage, self.used).and_then(|mut writer| {
                    writer.push(frame)?;
                    self.used = writer.used();
                    Ok(())
                });
            if result.is_err() {
                self.failed = true;
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

impl<O: ConnectedRxSink, const CAPACITY: usize, const SLOTS: usize>
    ConnectedRxProtocolSink<CAPACITY, SLOTS> for Esp32s31StaApStationRxSink<'_, O>
{
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

impl<'pool, M, O, const CAPACITY: usize, const SLOTS: usize, const REORDER_SLOTS: usize>
    Esp32s31StaApStationRxRole<'pool, CAPACITY, SLOTS>
    for Esp32s31ConnectedRxProcessor<
        '_,
        'pool,
        '_,
        '_,
        M,
        Esp32s31StaApStationRxSink<'_, O>,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
where
    M: open_esp_radio_embassy_net::RawMutex,
    O: ConnectedRxSink,
{
    type Dispatch = Option<ConnectedRxDispatch>;
    type Error = Esp32s31StaApStationRxError;

    fn publish_pending_rx(
        &mut self,
        network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, Self::Error> {
        self.sink_mut().publish_pending(network)
    }

    fn service_station_rx<'a>(
        &'a mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        _network: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<Self::Dispatch, Self::Error>> + 'a
    where
        'pool: 'a,
    {
        async move {
            debug_assert!(!self.sink().has_pending());
            Ok(self.dispatch_frame(frame).await)
        }
    }

    fn has_pending_rx(&self) -> bool {
        self.sink().has_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Observer(usize);

    impl ConnectedRxSink for Observer {
        fn publish(&mut self, _event: ConnectedRxEvent<'_>) {
            self.0 += 1;
        }
    }

    struct Network {
        capacity: usize,
        ether_types: std::vec::Vec<u16>,
    }

    impl DatapathNetworkRx for Network {
        fn queue_len(&self) -> usize {
            self.ether_types.len()
        }

        fn try_send(&mut self, _frame: &[u8]) -> Result<(), RxEnqueueError> {
            unreachable!("station batch publishes structured Ethernet records")
        }

        fn try_send_parts(
            &mut self,
            frame: open_esp_radio_ieee80211::data::EthernetFrameParts<'_>,
        ) -> Result<(), RxEnqueueError> {
            if self.capacity == 0 {
                return Err(RxEnqueueError::QueueFull);
            }
            self.capacity -= 1;
            self.ether_types.push(frame.ether_type);
            Ok(())
        }

        fn poll_ready(&mut self, _context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
            if self.capacity == 0 {
                core::task::Poll::Pending
            } else {
                core::task::Poll::Ready(())
            }
        }

        #[cfg(feature = "diagnostics")]
        fn try_send_observed(
            &mut self,
            frame: &[u8],
            before_publish: &mut dyn FnMut(),
        ) -> Result<(), RxEnqueueError> {
            let result = self.try_send(frame);
            if result.is_ok() {
                before_publish();
            }
            result
        }

        #[cfg(feature = "diagnostics")]
        fn try_send_parts_observed(
            &mut self,
            frame: open_esp_radio_ieee80211::data::EthernetFrameParts<'_>,
            before_publish: &mut dyn FnMut(),
        ) -> Result<(), RxEnqueueError> {
            let result = self.try_send_parts(frame);
            if result.is_ok() {
                before_publish();
            }
            result
        }
    }

    fn event(ether_type: u16) -> ConnectedRxEvent<'static> {
        ConnectedRxEvent::Ethernet {
            frame: open_esp_radio_ieee80211::data::EthernetFrameParts {
                destination: [1; 6],
                source: [2; 6],
                ether_type,
                payload: &[3, 4],
            },
            raw: &[0; 32],
            amsdu: false,
            metadata: open_esp_radio_wifi_softmac::MacRxMetadata::unavailable(),
        }
    }

    #[test]
    fn network_backpressure_retains_exact_station_batch_cursor() {
        let mut storage = [0_u8; VENDOR_LARGE_RX_PAYLOAD_CAPACITY];
        let mut sink = Esp32s31StaApStationRxSink::new(&mut storage, Observer::default());
        sink.publish(event(0x0800));
        sink.publish(event(0x0806));
        let mut network = Network {
            capacity: 1,
            ether_types: std::vec::Vec::new(),
        };

        assert_eq!(
            sink.publish_pending(&mut network),
            Ok(DatapathRxProgress::NetworkBackpressured)
        );
        assert!(sink.has_pending());
        assert_eq!(network.ether_types, [0x0800]);

        network.capacity = 1;
        assert_eq!(
            sink.publish_pending(&mut network),
            Ok(DatapathRxProgress::Drained)
        );
        assert!(!sink.has_pending());
        assert_eq!(network.ether_types, [0x0800, 0x0806]);
        assert_eq!(sink.observer().0, 2);
    }
}
