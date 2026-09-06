//! Saturate the real upstream stack before delivering its ARP reply.
//! A separate executable owns Xarxa's process-global packet pool.
use core::{
    future::Future,
    pin::pin,
    task::{Context, Waker},
};
use embassy_net_upstream::{
    Stack, StackStorage, TryError,
    udp::UdpSocket,
    wire::{IpAddress, IpCidr},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time as _;
use open_esp_radio_xarxa_upstream::{LinkState, NetworkInterfaceId, Resources, RxEnqueueError};

#[test]
fn unresolved_udp_can_starve_the_arp_reply_of_a_packet_buffer() {
    let mut resources = Resources::<NoopRawMutex, 16, 16>::new();
    let local_mac = [2, 0, 0, 0, 0, 1];
    let peer_mac = [2, 0, 0, 0, 0, 2];
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(1), local_mac);
    let mut storage = StackStorage::new();
    let (stack, mut runner) = Stack::new(&mut storage, 1);
    let iface = stack.add_iface(&mut device).unwrap();
    iface
        .set_ip_addrs([IpCidr::new(IpAddress::v4(192, 0, 2, 1), 24)])
        .unwrap();
    let mut socket = UdpSocket::new(stack).unwrap();
    socket.bind(4321).unwrap();
    radio.link_controller().set_link_state(LinkState::Up);
    let mut run = pin!(runner.run());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(run.as_mut().poll(&mut cx).is_pending());
    let destination = (IpAddress::v4(192, 0, 2, 2), 4322);
    socket.try_send_to(&[1; 80], destination).unwrap();
    let request = radio
        .try_receive_tx()
        .expect("first UDP starts ARP resolution");
    let mut reply = request.ethernet().to_vec();
    assert_eq!(&reply[12..14], &[8, 6]);
    // Turn the emitted Ethernet/IPv4 ARP request into its peer's response.
    reply[..6].copy_from_slice(&local_mac);
    reply[6..12].copy_from_slice(&peer_mac);
    reply[20..22].copy_from_slice(&2_u16.to_be_bytes());
    reply[22..28].copy_from_slice(&peer_mac);
    reply[28..32].copy_from_slice(&[192, 0, 2, 2]);
    reply[32..38].copy_from_slice(&local_mac);
    reply[38..42].copy_from_slice(&[192, 0, 2, 1]);
    drop(request);
    let mut accepted = 1;
    loop {
        match socket.try_send_to(&[1; 80], destination) {
            Ok(()) => accepted += 1,
            Err(TryError::WouldBlock) => break,
            Err(error) => panic!("unexpected send error: {error:?}"),
        }
        assert!(accepted < 256, "bounded unresolved-UDP reproduction");
    }
    assert_eq!(radio.tx_queue_len(), 0, "UDP is held inside the stack");
    assert_eq!(radio.rx_publisher().queue_len(), 0);
    assert_eq!(
        radio.rx_publisher().try_send(&reply),
        Err(RxEnqueueError::PoolExhausted)
    );
    assert_eq!(radio.rx_pool_drops(), 1);
    eprintln!("accepted={accepted}; ARP reply rejected: global packet pool exhausted");
}
