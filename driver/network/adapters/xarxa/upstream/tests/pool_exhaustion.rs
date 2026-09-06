//! A separate process owns the upstream singleton pool, so deliberate
//! exhaustion cannot steal packet capacity from unrelated parallel tests.
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_xarxa_upstream::{
    LinkState, NetworkInterfaceId, Resources, RxEnqueueError,
    driver::{Driver, PacketBuf},
};
use std::task::{Context, Waker};

#[test]
fn exhausted_global_pool_drops_rx_without_waiting_and_recovers_after_release() {
    let mut resources = Resources::<NoopRawMutex, 2, 2>::new();
    let (mut device, endpoint) = resources.split(NetworkInterfaceId::new(1), [2; 6]);
    endpoint.link_controller().set_link_state(LinkState::Up);
    let rx = endpoint.rx_publisher();
    let mut held = Vec::new();
    while let Some(packet) = PacketBuf::try_new() {
        held.push(packet);
    }
    assert!(!held.is_empty());
    let count = held.len();
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        rx.poll_ready(&mut cx).is_ready(),
        "no upstream pool-release waker exists"
    );
    let mut admitted = 0;
    assert_eq!(
        rx.try_send_observed(&[2; 60], &mut || admitted += 1),
        Err(RxEnqueueError::PoolExhausted)
    );
    assert_eq!(
        admitted, 0,
        "allocation refusal is not successful admission"
    );
    assert_eq!(rx.queue_len(), 0);
    assert_eq!(endpoint.rx_pool_drops(), 1);
    drop(held.pop());
    rx.try_send(&[3; 60]).unwrap();
    let packet = device.receive().unwrap();
    assert_eq!(&packet[..], &[3; 60]);
    assert!(
        PacketBuf::try_new().is_none(),
        "socket owner still occupies the slot"
    );
    drop(packet);
    drop(held);
    let mut reclaimed = Vec::new();
    while let Some(packet) = PacketBuf::try_new() {
        reclaimed.push(packet);
    }
    assert_eq!(
        reclaimed.len(),
        count,
        "every packet owner returns exactly once"
    );
}
