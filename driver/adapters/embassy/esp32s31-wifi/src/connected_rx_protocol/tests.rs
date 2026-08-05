use open_esp_radio_embassy_net::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_mac::{connected_rx::ConnectedRxConfig, rx::RxIngressConfig};

use super::*;

struct Sink;

impl ConnectedRxSink for Sink {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
}

impl ConnectedRxProtocolSink for Sink {
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

#[test]
fn deferred_ethernet_frames_pack_complete_ordered_records() {
    let first_payload = [1, 2, 3];
    let second_payload = [4, 5];
    let first = EthernetFrameParts {
        destination: [0x10; 6],
        source: [0x20; 6],
        ether_type: 0x0800,
        payload: &first_payload,
    };
    let second = EthernetFrameParts {
        destination: [0x30; 6],
        source: [0x40; 6],
        ether_type: 0x0806,
        payload: &second_payload,
    };
    let mut storage = [0_u8; 64];
    let mut deferred = DeferredEthernetFrames::new(&mut storage);

    deferred.publish(ConnectedRxEvent::Ethernet {
        frame: first,
        raw: &[],
        amsdu: true,
        metadata: MacRxMetadata::unavailable(),
    });
    deferred.publish(ConnectedRxEvent::Ethernet {
        frame: second,
        raw: &[],
        amsdu: true,
        metadata: MacRxMetadata::unavailable(),
    });

    let first_end = 2 + first.length();
    assert_eq!(
        &deferred.storage[..2],
        &(first.length() as u16).to_be_bytes()
    );
    assert_eq!(&deferred.storage[2..8], &first.destination);
    assert_eq!(&deferred.storage[8..14], &first.source);
    assert_eq!(&deferred.storage[14..16], &first.ether_type.to_be_bytes());
    assert_eq!(&deferred.storage[16..first_end], &first_payload);
    assert_eq!(
        &deferred.storage[first_end..first_end + 2],
        &(second.length() as u16).to_be_bytes()
    );
    assert_eq!(deferred.used, first_end + 2 + second.length());
}

#[test]
fn stop_edge_returns_an_empty_reusable_protocol_epoch() {
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let dispatcher = ConnectedRxDispatcher::new(ConnectedRxConfig {
        station_address: [2, 3, 4, 5, 6, 7],
        bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        association_id: 1,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    });
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let mpdu_ptr = mpdu.as_mut_ptr();
    let ethernet_ptr = ethernet.as_mut_ptr();
    let protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        dispatcher,
        Sink,
        &mut mpdu,
        &mut ethernet,
    );

    let stopped = embassy_futures::block_on(protocol.run_until_stopped(ready(())));
    assert_eq!(stopped.shutdown(), ConnectedRxProtocolShutdown::default());
    let (returned_mpdu, returned_ethernet) = stopped.into_scratch();
    assert_eq!(returned_mpdu.as_mut_ptr(), mpdu_ptr);
    assert_eq!(returned_mpdu.len(), 64);
    assert_eq!(returned_ethernet.as_mut_ptr(), ethernet_ptr);
    assert_eq!(returned_ethernet.len(), 64);
}
