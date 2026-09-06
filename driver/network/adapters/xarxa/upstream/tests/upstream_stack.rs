//! Exercise the real, unmodified Embassy/Xarxa stack through two radio endpoints.
use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use embassy_net_upstream::{
    Stack, StackStorage, TryError,
    udp::UdpSocket,
    wire::{IpAddress, IpCidr},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time as _; // Link the official host timer backend used by Git Embassy.
use open_esp_radio_xarxa_upstream::{LinkState, NetworkInterfaceId, Resources};

#[test]
fn original_stack_resolves_arp_and_exchanges_udp_across_reconnect() {
    let mut left = Resources::<NoopRawMutex, 4, 4>::new();
    let mut right = Resources::<NoopRawMutex, 4, 4>::new();
    let (mut left_device, left_radio) = left.split(NetworkInterfaceId::new(1), [2, 0, 0, 0, 0, 1]);
    let (mut right_device, right_radio) =
        right.split(NetworkInterfaceId::new(2), [2, 0, 0, 0, 0, 2]);
    let mut left_storage = StackStorage::new();
    let mut right_storage = StackStorage::new();
    let (left_stack, mut left_runner) = Stack::new(&mut left_storage, 1);
    let (right_stack, mut right_runner) = Stack::new(&mut right_storage, 2);
    let left_iface = left_stack.add_iface(&mut left_device).unwrap();
    let right_iface = right_stack.add_iface(&mut right_device).unwrap();
    let left_ip = IpAddress::v4(192, 0, 2, 1);
    let right_ip = IpAddress::v4(192, 0, 2, 2);
    left_iface.set_ip_addrs([IpCidr::new(left_ip, 24)]).unwrap();
    right_iface
        .set_ip_addrs([IpCidr::new(right_ip, 24)])
        .unwrap();
    let mut sender = UdpSocket::new(left_stack).unwrap();
    let mut receiver = UdpSocket::new(right_stack).unwrap();
    sender.bind(4321).unwrap();
    receiver.bind(4322).unwrap();
    let mut left_poll = pin!(left_runner.run());
    let mut right_poll = pin!(right_runner.run());
    let mut cx = Context::from_waker(Waker::noop());
    let mut arp_frames = 0;
    for round in 0..2 {
        left_radio.link_controller().set_link_state(LinkState::Up);
        right_radio.link_controller().set_link_state(LinkState::Up);
        assert!(left_poll.as_mut().poll(&mut cx).is_pending());
        assert!(right_poll.as_mut().poll(&mut cx).is_pending());
        assert!(left_iface.is_link_up() && right_iface.is_link_up());
        let payload = [round + 1; 80];
        let mut send = pin!(sender.send_to(&payload, (right_ip, 4322)));
        let mut sent = false;
        let mut received = false;
        for _ in 0..16 {
            if !sent && let Poll::Ready(result) = send.as_mut().poll(&mut cx) {
                result.unwrap();
                sent = true;
            }
            assert!(left_poll.as_mut().poll(&mut cx).is_pending());
            assert!(right_poll.as_mut().poll(&mut cx).is_pending());
            while let Some(frame) = left_radio.try_receive_tx() {
                if frame.ethernet()[12..14] == [8, 6] {
                    arp_frames += 1;
                }
                right_radio
                    .rx_publisher()
                    .try_send(frame.ethernet())
                    .unwrap();
            }
            while let Some(frame) = right_radio.try_receive_tx() {
                if frame.ethernet()[12..14] == [8, 6] {
                    arp_frames += 1;
                }
                left_radio
                    .rx_publisher()
                    .try_send(frame.ethernet())
                    .unwrap();
            }
            let mut bytes = [0; 80];
            match receiver.try_recv_from(&mut bytes) {
                Ok((length, metadata)) => {
                    assert_eq!(length, payload.len());
                    assert_eq!(bytes, payload);
                    assert_eq!(metadata.endpoint.addr, left_ip);
                    assert_eq!(metadata.endpoint.port, 4321);
                    received = true;
                    break;
                }
                Err(TryError::WouldBlock) => {}
                Err(error) => panic!("unexpected UDP receive failure: {error:?}"),
            }
        }
        assert!(
            sent && received,
            "the original stacks must complete ARP and UDP"
        );
        left_radio.link_controller().set_link_state(LinkState::Down);
        right_radio
            .link_controller()
            .set_link_state(LinkState::Down);
        assert!(left_poll.as_mut().poll(&mut cx).is_pending());
        assert!(right_poll.as_mut().poll(&mut cx).is_pending());
        assert!(!left_iface.is_link_up() && !right_iface.is_link_up());
    }
    assert!(
        arp_frames >= 2,
        "neighbor resolution must traverse the actual driver"
    );
}
