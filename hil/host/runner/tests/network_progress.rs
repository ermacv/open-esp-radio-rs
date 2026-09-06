//! Diagnostic wrappers exercise the production upstream queue owner.
use core::{
    future::Future,
    task::{Context, Poll, Waker},
};
use embassy_net::driver::{Driver, PacketBuf};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_xarxa_upstream::{LinkState, NetworkInterfaceId, Resources};

#[path = "../../../targets/esp32s31/runtime/src/product_hil/network/progress.rs"]
mod progress;

#[test]
fn observation_preserves_queue_backpressure_and_exact_packet_ownership() {
    let counters = Box::leak(Box::new(progress::Counters::new()));
    let mut resources = Resources::<NoopRawMutex, 2, 1>::new();
    let (device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6]);
    radio.link_controller().set_link_state(LinkState::Up);
    let mut device = progress::Device::new(device, counters);
    let before = counters.snapshot();
    assert!(device.can_transmit());
    let packet = PacketBuf::try_new().unwrap();
    let address = packet.as_ptr();
    assert!(device.transmit(packet).is_ok());
    for _ in 0..20 {
        assert!(!device.can_transmit());
    }
    let rejected = PacketBuf::try_new().unwrap();
    let rejected_address = rejected.as_ptr();
    let rejected = device.transmit(rejected).unwrap_err();
    assert_eq!(rejected.as_ptr(), rejected_address);
    assert_eq!(radio.try_receive_tx().unwrap().ethernet().as_ptr(), address);
    assert!(device.can_transmit());
    assert!(device.transmit(rejected).is_ok());
    assert_eq!(
        radio.try_receive_tx().unwrap().ethernet().as_ptr(),
        rejected_address
    );
    let sample = counters.snapshot().delta(before);
    assert_eq!(sample.get(progress::Event::TxReady), 2);
    assert_eq!(sample.get(progress::Event::TxUnavailable), 20);
    assert_eq!(sample.get(progress::Event::TxAccepted), 2);
    assert_eq!(sample.get(progress::Event::TxRejected), 1);
}

#[test]
fn idle_poll_and_packet_transfer_are_distinct_and_future_result_is_preserved() {
    let counters = Box::leak(Box::new(progress::Counters::new()));
    let mut resources = Resources::<NoopRawMutex, 2, 1>::new();
    let (device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6]);
    radio.link_controller().set_link_state(LinkState::Up);
    let mut device = progress::Device::new(device, counters);
    let work = core::future::poll_fn(|_| device.receive().map_or(Poll::Pending, Poll::Ready));
    let mut observed = core::pin::pin!(progress::observe(work, counters));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(observed.as_mut().poll(&mut cx).is_pending());
    radio.rx_publisher().try_send(&[42; 60]).unwrap();
    let Poll::Ready(packet) = observed.as_mut().poll(&mut cx) else {
        panic!("published packet must pass through observer")
    };
    assert_eq!(&packet[..], &[42; 60]);
    let sample = counters.snapshot();
    assert_eq!(sample.get(progress::Event::NetworkPoll), 2);
    assert_eq!(sample.get(progress::Event::PollWithoutTransfer), 1);
    assert_eq!(sample.get(progress::Event::RxEmpty), 1);
    assert_eq!(sample.get(progress::Event::RxDelivered), 1);
}
