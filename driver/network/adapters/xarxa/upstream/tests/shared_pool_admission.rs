//! Isolate the upstream singleton pool from other tests in a separate process.
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_xarxa_upstream::{
    LinkState, NetworkInterfaceId, Resources, RxEnqueueError,
    driver::{Driver, PacketBuf},
};

#[test]
fn tx_queue_can_consume_the_rx_pool_and_smaller_queue_preserves_headroom() {
    {
        let mut resources = Resources::<NoopRawMutex, 16, 16>::new();
        let (mut device, endpoint) = resources.split(NetworkInterfaceId::new(1), [2; 6]);
        endpoint.link_controller().set_link_state(LinkState::Up);
        for _ in 0..16 {
            assert!(device.can_transmit());
            let mut packet = PacketBuf::try_new().unwrap();
            packet.set_len(60);
            packet.fill(2);
            device.transmit(packet).unwrap();
        }
        assert_eq!(
            endpoint.rx_publisher().try_send(&[2; 60]),
            Err(RxEnqueueError::PoolExhausted)
        );
        assert_eq!(endpoint.rx_pool_drops(), 1);
        let selected = endpoint.try_receive_tx().unwrap();
        assert!(
            device.can_transmit(),
            "queue credit is distinct from pool credit"
        );
        assert_eq!(
            endpoint.rx_publisher().try_send(&[2; 60]),
            Err(RxEnqueueError::PoolExhausted)
        );
        drop(selected);
        endpoint.rx_publisher().try_send(&[2; 60]).unwrap();
    }
    {
        let mut resources = Resources::<NoopRawMutex, 16, 8>::new();
        let (mut device, endpoint) = resources.split(NetworkInterfaceId::new(1), [2; 6]);
        endpoint.link_controller().set_link_state(LinkState::Up);
        for _ in 0..8 {
            let mut packet = PacketBuf::try_new().unwrap();
            packet.set_len(60);
            packet.fill(2);
            device.transmit(packet).unwrap();
        }
        assert!(!device.can_transmit());
        endpoint.rx_publisher().try_send(&[2; 60]).unwrap();
        assert_eq!(endpoint.rx_pool_drops(), 0);
        assert_eq!(device.receive().unwrap().len(), 60);
    }
}
