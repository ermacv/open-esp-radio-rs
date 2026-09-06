use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_embassy::datapath::{
    PinnedTxPool, PinnedTxResources, SelectedBurstMaterializer,
    network::{DatapathNetwork, DatapathNetworkRx},
};
use open_esp_radio_esp32s31_wifi_xarxa_upstream::Network;
use open_esp_radio_xarxa_upstream::{
    LinkState, NetworkInterfaceId, Resources,
    driver::{Driver, PacketBuf},
};

#[test]
fn two_interfaces_share_physical_credit_but_keep_packet_and_link_ownership() {
    let mut first = Resources::<NoopRawMutex, 2, 2>::new();
    let mut second = Resources::<NoopRawMutex, 2, 2>::new();
    let first_id = NetworkInterfaceId::new(1);
    let second_id = NetworkInterfaceId::new(2);
    let (mut first_device, first_endpoint) = first.split(first_id, [2; 6]);
    let (mut second_device, second_endpoint) = second.split(second_id, [4; 6]);
    let physical = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 1514, 16, 8, 1>::new(),
    ));
    let pool = PinnedTxPool::<1514, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let network = Network::new(first_endpoint, second_endpoint, physical.split(pool));
    network.set_link_state(first_id, LinkState::Up);
    network.set_link_state(second_id, LinkState::Up);
    for (device, value) in [(&mut first_device, 7), (&mut second_device, 8)] {
        let mut packet = PacketBuf::try_new().unwrap();
        packet.set_len(60);
        packet.fill(value);
        assert!(device.can_transmit());
        assert!(device.transmit(packet).is_ok());
    }
    let first_consumer = network.tx_consumer(first_id);
    let second_consumer = network.tx_consumer(second_id);
    let held = first_consumer.try_materialize_next().unwrap();
    assert_eq!(held.as_slice(), &[7; 60]);
    assert!(second_consumer.try_materialize_next().is_none());
    assert_eq!(
        second_consumer.queue_len(),
        1,
        "unselected upstream owner stays queued"
    );
    network.set_link_state(first_id, LinkState::Down);
    assert_eq!(
        second_device.link_state(),
        open_esp_radio_xarxa_upstream::driver::LinkState::Up
    );
    drop(held);
    let next = second_consumer.try_materialize_next().unwrap();
    assert_eq!(next.as_slice(), &[8; 60]);
    network.rx_publisher(second_id).try_send(&[9; 60]).unwrap();
    assert!(first_device.receive().is_none());
    assert_eq!(&second_device.receive().unwrap()[..], &[9; 60]);
}
