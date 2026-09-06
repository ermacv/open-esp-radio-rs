//! One radio burst must fit the HIL UDP socket before its task gets a turn.
use embassy_net::{
    Stack, StackStorage,
    udp::UdpSocket,
    wire::{IpAddress, IpCidr},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time as _;
use open_esp_radio_xarxa_upstream::{LinkState, NetworkInterfaceId, Resources};
use std::{
    future::Future,
    pin::pin,
    task::{Context, Waker},
};

#[test]
fn udp_retains_a_radio_burst_until_the_receiver_task_runs() {
    let mut sender_resources = Resources::<NoopRawMutex, 16, 16>::new();
    let mut receiver_resources = Resources::<NoopRawMutex, 16, 16>::new();
    let (mut sender_device, sender_radio) =
        sender_resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1]);
    let (mut receiver_device, receiver_radio) =
        receiver_resources.split(NetworkInterfaceId::new(1), [2, 0, 0, 0, 0, 2]);
    let mut sender_storage = StackStorage::new();
    let mut receiver_storage = StackStorage::new();
    let (sender_stack, mut sender_runner) = Stack::new(&mut sender_storage, 1);
    let (receiver_stack, mut receiver_runner) = Stack::new(&mut receiver_storage, 2);
    sender_stack
        .add_iface(&mut sender_device)
        .unwrap()
        .set_ip_addrs([IpCidr::new(IpAddress::v4(192, 0, 2, 1), 24)])
        .unwrap();
    receiver_stack
        .add_iface(&mut receiver_device)
        .unwrap()
        .set_ip_addrs([IpCidr::new(IpAddress::v4(192, 0, 2, 2), 24)])
        .unwrap();
    let mut sender = UdpSocket::new(sender_stack).unwrap();
    let mut receiver = UdpSocket::new(receiver_stack).unwrap();
    sender.bind(4321).unwrap();
    receiver.bind(4322).unwrap();
    sender_radio.link_controller().set_link_state(LinkState::Up);
    receiver_radio
        .link_controller()
        .set_link_state(LinkState::Up);
    let mut cx = Context::from_waker(Waker::noop());
    let mut send_run = pin!(sender_runner.run());
    let mut receive_run = pin!(receiver_runner.run());
    assert!(send_run.as_mut().poll(&mut cx).is_pending());
    assert!(receive_run.as_mut().poll(&mut cx).is_pending());
    // Deliver one burst before polling the stack, as a separate radio core can.
    for sequence in 0..16_u8 {
        sender
            .try_send_to(&[sequence; 64], (IpAddress::v4(192, 0, 2, 255), 4322))
            .unwrap();
        let frame = sender_radio.try_receive_tx().unwrap();
        // Release the sender owner before emulating delivery into the receiver pool.
        let wire = frame.ethernet().to_vec();
        drop(frame);
        receiver_radio.rx_publisher().try_send(&wire).unwrap();
    }
    assert!(receive_run.as_mut().poll(&mut cx).is_pending());
    let mut received = Vec::new();
    let mut bytes = [0; 64];
    while let Ok((length, _)) = receiver.try_recv_from(&mut bytes) {
        assert_eq!(length, 64);
        assert!(bytes.iter().all(|&byte| byte == bytes[0]));
        received.push(bytes[0]);
    }
    assert_eq!(received, (0..16).collect::<Vec<_>>());
}
