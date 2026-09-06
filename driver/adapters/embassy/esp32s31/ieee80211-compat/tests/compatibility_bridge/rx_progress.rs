use core::future::Future;
use open_esp_radio_esp32s31_wifi_embassy::roles::station::{
    network::EmbassyNetConnectedRxSink, rx_protocol::ConnectedRxProtocolSink,
};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};
use open_esp_radio_wifi_softmac::MacRxMetadata;

use super::*;

struct Observer;
impl ConnectedRxSink for Observer {
    fn publish(&mut self, _: ConnectedRxEvent<'_>) {}
}

#[test]
fn full_rx_and_tx_return_radio_control_and_recover_without_losing_queued_frames() {
    let (endpoint, rx_storage, tx_storage) = endpoint();
    let interface = NetworkInterfaceId::new(0);
    let (mut device, radio) = endpoint.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    let monitor = radio.resource_monitor();
    let network = CompatibilityDatapathNetwork::new(interface, radio, physical());
    network.set_link_state(interface, LinkState::Up);
    let mut publisher = network.rx_publisher(interface);
    let mut sink = EmbassyNetConnectedRxSink::new(publisher, Observer);
    let consumer = network.tx_consumer(interface);

    // Repeat after recovery: a one-time escape or a leaked credit is insufficient.
    for epoch in 0..2 {
        for value in 0..NETWORK_QUEUE_DEPTH {
            publisher
                .try_send(&[value as u8; ETHERNET_HEADER_LEN])
                .unwrap();
            device
                .transmit(&mut context())
                .unwrap()
                .consume(ETHERNET_HEADER_LEN, |frame| frame.fill(value as u8));
        }
        assert!(device.receive(&mut context()).is_none());
        assert!(publisher.poll_ready(&mut context()).is_pending());
        let full = monitor.snapshot();
        assert_eq!((full.rx_free, full.tx_free), (0, 0));

        // These are the actual admission edges used by staged ordinary RX
        // and by copied A-MSDU/reorder publication inside service_bounded().
        // Neither may suspend the owner needed to materialize queued TX.
        {
            let wait = <_ as ConnectedRxProtocolSink<64, 1>>::wait_staged_ready(&mut sink);
            assert!(
                core::pin::pin!(wait)
                    .as_mut()
                    .poll(&mut context())
                    .is_ready()
            );
        }
        {
            let wait = <_ as ConnectedRxProtocolSink<64, 1>>::wait_ready(&mut sink);
            assert!(
                core::pin::pin!(wait)
                    .as_mut()
                    .poll(&mut context())
                    .is_ready()
            );
        }
        sink.publish(ConnectedRxEvent::Ethernet {
            frame: open_esp_radio_esp32s31_wifi_embassy::datapath::network::EthernetFrameParts {
                destination: [2; 6],
                source: [4; 6],
                ether_type: 0x0800,
                payload: &[0xff],
            },
            raw: &[],
            amsdu: false,
            metadata: MacRxMetadata::unavailable(),
        });
        assert_eq!(monitor.snapshot().rx_queue_full, epoch + 1);
        assert_eq!(publisher.queue_len(), NETWORK_QUEUE_DEPTH);

        // TX materialization returns the real software token, making RX
        // admissible without resetting the device or discarding queued data.
        for value in 0..NETWORK_QUEUE_DEPTH {
            let tx = consumer.try_materialize_next().unwrap();
            assert_eq!(tx.as_slice(), &[value as u8; ETHERNET_HEADER_LEN]);
            drop(tx);
            let (rx, reply) = device.receive(&mut context()).unwrap();
            rx.consume(|frame| assert_eq!(frame, &[value as u8; ETHERNET_HEADER_LEN]));
            drop(reply);
        }
        let empty = monitor.snapshot();
        assert_eq!((empty.rx_ready, empty.tx_ready), (0, 0));
        assert_eq!(
            (empty.rx_free, empty.tx_free),
            (NETWORK_QUEUE_DEPTH, NETWORK_QUEUE_DEPTH)
        );
    }
}
