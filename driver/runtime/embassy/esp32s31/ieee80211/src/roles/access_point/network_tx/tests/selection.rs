//! Exercise AP selection with the production owned queues and materializer.

use std::boxed::Box;

use super::super::{
    AP_SOFTWARE_TX_CAPACITY, ApTxFlowKey, Esp32s31AccessPointNetworkTx, Esp32s31ApEngine,
};
use super::support::{Hardware, allocator, packet};
use crate::datapath::{DatapathTxConsumer, PinnedTxPool, PinnedTxResources};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_embassy_net::{NetworkInterfaceId, OwnedEndpointResources};
use open_esp_radio_esp32s31_wifi_ap::{protocol::*, security::Esp32s31ApPairwiseKeyStorage};
use open_esp_radio_ieee80211::{
    beacon::WPA2_BEACON_CAPACITY, channel::WifiChannel, ssid::WifiSsid,
};

mod admission;
mod awake;

#[test]
fn ap_selection_leaves_more_than_an_arena_of_other_peer_owners_at_the_source() {
    const OTHER_PEER_FRAMES: usize = AP_SOFTWARE_TX_CAPACITY + 1;
    const SOURCE_CAPACITY: usize = OTHER_PEER_FRAMES + 1;
    let mut peers = AccessPointPeerStorage::new();
    let service = AccessPointService::new_open(
        [2; 6],
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::default(),
        &mut peers,
    );
    let mut beacon = [0; WPA2_BEACON_CAPACITY];
    let mut keys = Esp32s31ApPairwiseKeyStorage::new();
    let mut engine = Esp32s31ApEngine::start(
        &mut Hardware,
        service,
        &mut beacon,
        &mut keys,
        &WifiSsid::new(b"test").unwrap(),
        WifiChannel::mhz20(13).unwrap(),
        100,
        2,
    )
    .unwrap_or_else(|_| panic!("open AP startup"));
    let pool = allocator::<SOURCE_CAPACITY>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<
        NoopRawMutex,
        1,
        SOURCE_CAPACITY,
    >::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, radio) = resources.split(interface, [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    let dma = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let dma_resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let source = DatapathTxConsumer::new(&radio, dma_resources.split(dma).for_interface(interface));
    let mut tx_storage = super::super::AccessPointTxStorage::new();
    let mut ap = Esp32s31AccessPointNetworkTx::new(&mut tx_storage, None);
    for n in 0..OTHER_PEER_FRAMES {
        device.transmit(packet(pool, 4, n as u8)).unwrap();
    }
    let selected_packet = packet(pool, 6, 200);
    let key = ApTxFlowKey::unbound_from_ethernet(&selected_packet);
    device.transmit(selected_packet).unwrap();
    let selected = ap
        .take_matching_active_or_network(&mut engine, key, &source)
        .unwrap()
        .unwrap();
    assert_eq!(selected.ethernet()[14], 200);
    assert_eq!(radio.tx_queue_len(), OTHER_PEER_FRAMES);
    assert_eq!(
        ap.active_frames.len(),
        0,
        "AP never rehomes another destination"
    );
    assert!(
        ap.take_matching_active_or_network(&mut engine, key, &source)
            .unwrap()
            .is_none()
    );
    assert_eq!(radio.tx_queue_len(), OTHER_PEER_FRAMES);
    assert!(
        pool.try_alloc().is_none(),
        "no admitted owner was silently dropped"
    );
    drop(selected);
    for n in 0..OTHER_PEER_FRAMES {
        assert_eq!(radio.try_receive_tx().unwrap().ethernet()[14], n as u8);
    }
}
