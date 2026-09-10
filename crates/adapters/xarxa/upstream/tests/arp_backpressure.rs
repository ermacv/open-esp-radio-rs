//! Characterize the original stack's immediate ARP response under TX pressure.
//! This executable owns a separate process-global packet pool.
use core::{
    future::Future,
    pin::pin,
    task::{Context, Waker},
};
use embassy_net_upstream::{
    Stack, StackStorage,
    wire::{IpAddress, IpCidr},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time as _;
use oer_xarxa_upstream::{
    LinkState, NetworkInterfaceId, Resources,
    driver::{Driver, PacketBuf},
};

#[test]
fn original_arp_response_is_not_retried_after_device_rejection() {
    let mut resources = Resources::<NoopRawMutex, 2, 1>::new();
    let local_mac = [2, 0, 0, 0, 0, 1];
    let peer_mac = [2, 0, 0, 0, 0, 2];
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(1), local_mac);
    radio.link_controller().set_link_state(LinkState::Up);
    let mut filler = PacketBuf::try_new().unwrap();
    filler.set_len(60);
    filler.fill(0x55);
    assert!(device.transmit(filler).is_ok());
    let mut storage = StackStorage::new();
    let (stack, mut runner) = Stack::new(&mut storage, 1);
    let iface = stack.add_iface(&mut device).unwrap();
    iface
        .set_ip_addrs([IpCidr::new(IpAddress::v4(192, 0, 2, 1), 24)])
        .unwrap();
    let mut run = pin!(runner.run());
    let mut cx = Context::from_waker(Waker::noop());
    let mut request = [0u8; 42];
    request[..6].copy_from_slice(&local_mac);
    request[6..12].copy_from_slice(&peer_mac);
    request[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
    request[22..28].copy_from_slice(&peer_mac);
    request[28..32].copy_from_slice(&[192, 0, 2, 2]);
    request[38..42].copy_from_slice(&[192, 0, 2, 1]);
    radio.rx_publisher().try_send(&request).unwrap();
    assert!(run.as_mut().poll(&mut cx).is_pending());
    assert_eq!(radio.rx_publisher().queue_len(), 0, "stack consumed ARP");
    assert_eq!(radio.rx_pool_drops(), 0);
    assert_eq!(radio.try_receive_tx().unwrap().ethernet(), &[0x55; 60]);
    // Returning the occupied TX credit does not recover the discarded reply.
    assert!(run.as_mut().poll(&mut cx).is_pending());
    assert!(radio.try_receive_tx().is_none());
    // The same request succeeds when it arrives with a free TX slot.
    radio.rx_publisher().try_send(&request).unwrap();
    assert!(run.as_mut().poll(&mut cx).is_pending());
    let reply = radio
        .try_receive_tx()
        .expect("ARP reply with free device credit");
    let reply = reply.ethernet();
    assert_eq!(&reply[..6], &peer_mac);
    assert_eq!(&reply[12..14], &[8, 6]);
    assert_eq!(&reply[20..22], &[0, 2]);
    assert_eq!(&reply[28..32], &[192, 0, 2, 1]);
    assert_eq!(&reply[38..42], &[192, 0, 2, 2]);
}
