//! Original upstream send recovery with a deliberately stopped radio consumer.
//!
//! Poll counts characterize the pinned baseline without requiring its excessive
//! wakeups to persist after an upstream fix. No hardware or packet loss is
//! needed to reach backpressure; downstream disposal then tests recovery.

use embassy_net::{
    Stack, StackStorage,
    udp::UdpSocket,
    wire::{IpAddress, IpCidr},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
// Link the host time driver shared with the original Git Embassy package.
use embassy_time as _;
use open_esp_radio_xarxa_upstream::{LinkState, NetworkInterfaceId, Resources};
use std::{
    future::Future,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

struct Scheduled(AtomicBool);

impl Scheduled {
    fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

impl Wake for Scheduled {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn exercise_backpressure() -> (usize, usize) {
    let mut resources = Resources::<NoopRawMutex, 2, 1>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(1), [2, 0, 0, 0, 0, 1]);
    let mut storage = StackStorage::new();
    let (stack, mut runner) = Stack::new(&mut storage, 1);
    let iface = stack.add_iface(&mut device).unwrap();
    iface
        .set_ip_addrs([IpCidr::new(IpAddress::v4(192, 0, 2, 1), 24)])
        .unwrap();
    let mut socket = UdpSocket::new(stack).unwrap();
    socket.bind(1234).unwrap();
    radio.link_controller().set_link_state(LinkState::Up);
    let network = Arc::new(Scheduled(AtomicBool::new(true)));
    let application = Arc::new(Scheduled(AtomicBool::new(true)));
    let network_waker = Waker::from(network.clone());
    let application_waker = Waker::from(application.clone());
    let mut network_context = Context::from_waker(&network_waker);
    let mut application_context = Context::from_waker(&application_waker);
    let mut run = pin!(runner.run());
    assert!(run.as_mut().poll(&mut network_context).is_pending());
    let peer = (IpAddress::v4(192, 0, 2, 255), 1235);
    socket.try_send_to(&[1; 32], peer).unwrap();
    assert_eq!(radio.tx_queue_len(), 1);
    let payload = [2; 32];
    let mut send = pin!(socket.send_to(&payload, peer));
    let (mut network_polls, mut application_polls) = (0, 0);
    for _ in 0..1000 {
        if application.take() {
            assert!(send.as_mut().poll(&mut application_context).is_pending());
            application_polls += 1;
        }
        if network.take() {
            assert!(run.as_mut().poll(&mut network_context).is_pending());
            network_polls += 1;
        }
    }
    println!(
        "without radio progress: application_polls={application_polls} network_polls={network_polls}"
    );
    assert_eq!(
        radio.tx_queue_len(),
        1,
        "blocked attempts publish no extra packet"
    );

    // Disposal represents downstream loss after stack admission. It must return
    // capacity without requiring an acknowledgement or an unrelated RX packet.
    drop(radio.try_receive_tx().unwrap());
    let mut completed = false;
    for _ in 0..8 {
        if network.take() {
            assert!(run.as_mut().poll(&mut network_context).is_pending());
        }
        if !completed
            && application.take()
            && let Poll::Ready(result) = send.as_mut().poll(&mut application_context)
        {
            result.unwrap();
            completed = true;
        }
    }
    assert!(
        completed,
        "returned capacity wakes and completes the blocked send"
    );
    let frame = radio.try_receive_tx().unwrap();
    assert!(frame.ethernet().ends_with(&payload));
    drop(frame);
    let mut after_disposal_polls = 0;
    for _ in 0..1000 {
        if network.take() {
            assert!(run.as_mut().poll(&mut network_context).is_pending());
            after_disposal_polls += 1;
        }
    }
    println!("after disposal: network_polls={after_disposal_polls}");
    assert!(
        after_disposal_polls < 8,
        "UDP disposal must not sustain a wake loop"
    );
    assert_eq!(
        radio.tx_queue_len(),
        0,
        "disposed UDP is not retransmitted by the stack"
    );
    // A buffer may be returned by application code, outside the driver queue.
    // Device readiness alone must not turn pool exhaustion into a lost wake.
    let mut pool_socket = UdpSocket::new(stack).unwrap();
    pool_socket.bind(1236).unwrap();
    let mut held = Vec::new();
    while let Some(packet) = xarxa::driver::PacketBuf::try_new() {
        held.push(packet);
    }
    assert!(!held.is_empty());
    let mut pool_send = pin!(pool_socket.send_to(b"pool released", peer));
    assert!(
        pool_send
            .as_mut()
            .poll(&mut application_context)
            .is_pending()
    );
    for _ in 0..16 {
        if application.take() {
            assert!(
                pool_send
                    .as_mut()
                    .poll(&mut application_context)
                    .is_pending()
            );
        }
        if network.take() {
            assert!(run.as_mut().poll(&mut network_context).is_pending());
        }
    }
    assert_eq!(radio.tx_queue_len(), 0);
    drop(held.pop());
    let mut pool_completed = false;
    for _ in 0..8 {
        if network.take() {
            assert!(run.as_mut().poll(&mut network_context).is_pending());
        }
        if !pool_completed
            && application.take()
            && let Poll::Ready(result) = pool_send.as_mut().poll(&mut application_context)
        {
            result.unwrap();
            pool_completed = true;
        }
    }
    assert!(
        pool_completed,
        "a buffer returned outside the driver must release the send"
    );
    let frame = radio.try_receive_tx().unwrap();
    assert!(frame.ethernet().ends_with(b"pool released"));
    drop(frame);
    drop(held);

    (application_polls, network_polls)
}

#[test]
fn blocked_send_recovers_after_queue_release_and_packet_disposal() {
    exercise_backpressure();
}

#[test]
#[ignore = "requires the pinned patch: cargo xtask check network-backpressure"]
fn blocked_send_quiesces_until_capacity_returns() {
    let (application_polls, network_polls) = exercise_backpressure();
    assert!(
        application_polls <= 2,
        "blocked application must stop polling"
    );
    assert!(
        network_polls <= 2,
        "blocked network runner must stop polling"
    );
}
