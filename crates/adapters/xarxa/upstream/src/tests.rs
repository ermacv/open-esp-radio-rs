extern crate std;
use super::*;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering as AtomicOrdering},
};
use std::task::{Wake, Waker};

struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, AtomicOrdering::Relaxed);
    }
}
fn waker() -> (Arc<Wakes>, Waker) {
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    (wakes.clone(), Waker::from(wakes))
}
fn packet(value: u8) -> PacketBuf {
    let mut packet = PacketBuf::try_new().expect("test has bounded packet ownership");
    packet.set_len(60);
    packet.fill(value);
    packet
}

#[test]
fn tx_backpressure_returns_exact_owner_and_credit_wakes_network() {
    let mut resources = Resources::<NoopRawMutex, 2, 1>::new();
    let (mut device, endpoint) = resources.split(NetworkInterfaceId::new(1), [2; 6]);
    endpoint.link_controller().set_link_state(LinkState::Up);
    let first = packet(7);
    let first_address = first.as_ptr();
    assert!(device.can_transmit());
    assert!(device.transmit(first).is_ok());
    let (wakes, waker) = waker();
    device.register_waker(&waker).unwrap();
    assert!(!device.can_transmit());
    let second = packet(8);
    let second_address = second.as_ptr();
    let rejected = device.transmit(second).err().unwrap();
    assert_eq!(rejected.as_ptr(), second_address);
    let frame = endpoint.try_receive_tx().unwrap();
    assert_eq!(frame.ethernet().as_ptr(), first_address);
    assert_eq!(frame.interface(), NetworkInterfaceId::new(1));
    assert!(wakes.0.load(AtomicOrdering::Relaxed) > 0);
    assert!(device.can_transmit());
    assert!(device.transmit(rejected).is_ok());
    assert_eq!(
        endpoint.try_receive_tx().unwrap().ethernet().as_ptr(),
        second_address
    );
}

#[test]
fn reconnect_cannot_retarget_admitted_or_queued_packets() {
    let mut resources = Resources::<NoopRawMutex, 2, 2>::new();
    let (mut device, endpoint) = resources.split(NetworkInterfaceId::new(1), [2; 6]);
    let link = endpoint.link_controller();
    link.set_link_state(LinkState::Up);
    assert!(device.transmit(packet(1)).is_ok());
    endpoint.rx_publisher().try_send(&[2; 60]).unwrap();
    assert!(device.can_transmit());
    let admitted = packet(3);
    link.set_link_state(LinkState::Down);
    assert_eq!(endpoint.tx_queue_len(), 0);
    assert_eq!(endpoint.rx_publisher().queue_len(), 0);
    link.set_link_state(LinkState::Up);
    assert!(
        device.transmit(admitted).is_ok(),
        "previous admission is honored terminally"
    );
    assert!(endpoint.try_receive_tx().is_none());
    assert!(device.receive().is_none());
    assert!(device.can_transmit());
    assert!(device.transmit(packet(4)).is_ok());
    assert_eq!(endpoint.try_receive_tx().unwrap().ethernet(), &[4; 60]);
}

#[test]
fn rx_parts_wake_stack_and_consumption_returns_queue_credit() {
    let mut resources = Resources::<NoopRawMutex, 1, 1>::new();
    let (mut device, endpoint) = resources.split(NetworkInterfaceId::new(1), [2; 6]);
    let rx = endpoint.rx_publisher();
    let (network_wakes, network_waker) = waker();
    device.register_waker(&network_waker).unwrap();
    endpoint.link_controller().set_link_state(LinkState::Up);
    assert!(network_wakes.0.load(AtomicOrdering::Relaxed) > 0);
    device.register_waker(&network_waker).unwrap();
    rx.try_send_parts([2; 6], [4; 6], 0x0800, &[9; 46]).unwrap();
    assert!(network_wakes.0.load(AtomicOrdering::Relaxed) > 1);
    let (radio_wakes, radio_waker) = waker();
    let mut cx = Context::from_waker(&radio_waker);
    assert!(rx.poll_ready(&mut cx).is_pending());
    assert_eq!(rx.try_send(&[0; 60]), Err(RxEnqueueError::QueueFull));
    let received = device.receive().unwrap();
    assert_eq!(&received[..6], &[2; 6]);
    assert_eq!(&received[6..12], &[4; 6]);
    assert_eq!(&received[12..14], &[8, 0]);
    assert_eq!(&received[14..], &[9; 46]);
    assert!(radio_wakes.0.load(AtomicOrdering::Relaxed) > 0);
    assert!(rx.poll_ready(&mut cx).is_ready());
}

#[test]
fn continuously_refilled_rx_yields_and_wakes_its_continuation() {
    let mut resources = Resources::<NoopRawMutex, 1, 1>::new();
    let (mut device, endpoint) = resources.split(NetworkInterfaceId::new(1), [2; 6]);
    endpoint.link_controller().set_link_state(LinkState::Up);
    let rx = endpoint.rx_publisher();
    rx.try_send(&[1; 60]).unwrap();
    drop(device.receive().unwrap());
    rx.try_send(&[2; 60]).unwrap();
    let (wakes, waker) = waker();
    device.register_waker(&waker).unwrap();
    assert!(
        device.receive().is_none(),
        "a single upstream drain must terminate"
    );
    assert!(wakes.0.load(AtomicOrdering::Relaxed) > 0);
    assert_eq!(&device.receive().unwrap()[..], &[2; 60]);
}
