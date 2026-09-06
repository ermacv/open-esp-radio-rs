use core::task::{Context, Waker};

use embassy_net_driver::{Checksum, ChecksumCapabilities, Driver, RxToken as _, TxToken as _};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_embassy_net_compat::{
    ETHERNET_HEADER_LEN, FrameLengthError, FrameStorage, LinkState, Resources, RxEnqueueError,
};

const FRAME_CAPACITY: usize = 64;

#[test]
fn saturated_tx_blocks_rx_until_radio_releases_a_tx_owner() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<2>();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    let monitor = radio.resource_monitor();
    radio.set_link_state(LinkState::Up);
    for _ in 0..2 {
        device
            .transmit(&mut context())
            .unwrap()
            .consume(ETHERNET_HEADER_LEN, |frame| frame.fill(1));
        radio.try_send_rx(&[2; ETHERNET_HEADER_LEN]).unwrap();
    }
    let full = monitor.snapshot();
    assert_eq!(
        (full.rx_ready, full.rx_free, full.tx_ready, full.tx_free),
        (2, 0, 2, 0)
    );
    assert!(radio.rx_publisher().poll_ready(&mut context()).is_pending());
    assert!(device.receive(&mut context()).is_none());

    // Selecting TX still retains its payload owner. A scheduler awaiting RX
    // capacity before this terminal drop cannot break the circular wait.
    let selected = radio.try_receive_tx().unwrap();
    assert_eq!(monitor.snapshot().tx_ready, 1);
    assert_eq!(monitor.snapshot().tx_free, 0);
    assert!(device.receive(&mut context()).is_none());
    drop(selected);
    assert_eq!(monitor.snapshot().tx_free, 1);
    let (rx, reply) = device.receive(&mut context()).unwrap();
    assert_eq!(
        monitor.snapshot().rx_free,
        0,
        "RX token still owns its slot"
    );
    rx.consume(|frame| assert_eq!(frame, &[2; ETHERNET_HEADER_LEN]));
    drop(reply);
    assert!(radio.rx_publisher().poll_ready(&mut context()).is_ready());
    assert_eq!(monitor.snapshot().rx_free, 1);
}

fn context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

fn endpoint_resources<const QUEUE_DEPTH: usize>() -> (
    &'static mut Resources<NoopRawMutex, FRAME_CAPACITY, QUEUE_DEPTH>,
    &'static mut FrameStorage<FRAME_CAPACITY, QUEUE_DEPTH>,
    &'static mut FrameStorage<FRAME_CAPACITY, QUEUE_DEPTH>,
) {
    (
        Box::leak(Box::new(Resources::new())),
        Box::leak(Box::new(FrameStorage::new())),
        Box::leak(Box::new(FrameStorage::new())),
    )
}

#[test]
fn checksum_capabilities_are_forwarded_to_the_unchanged_stack() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<1>();
    let (device, _radio) = resources.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    let mut checksum = ChecksumCapabilities::default();
    checksum.ipv4 = Checksum::Tx;
    checksum.udp = Checksum::None;
    let device = device.with_checksum_capabilities(checksum);

    let observed = device.capabilities().checksum;
    assert!(matches!(observed.ipv4, Checksum::Tx));
    assert!(matches!(observed.udp, Checksum::None));
}

#[test]
fn rx_owner_moves_into_and_out_of_the_unchanged_embassy_driver() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<2>();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    radio.set_link_state(LinkState::Up);
    let mut source = [0_u8; ETHERNET_HEADER_LEN + 8];
    source[0] = 0xa1;
    source[6] = 0xb2;
    source[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());

    radio.try_send_rx(&source).unwrap();
    source.fill(0);

    let (rx, _) = device.receive(&mut context()).unwrap();
    rx.consume(|frame| {
        assert_eq!(frame[0], 0xa1);
        assert_eq!(frame[6], 0xb2);
    });
    assert_eq!(radio.rx_queue_len(), 0);
}

#[test]
fn unchanged_embassy_tx_token_reserves_bounded_capacity() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<1>();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    radio.set_link_state(LinkState::Up);

    device
        .transmit(&mut context())
        .unwrap()
        .consume(ETHERNET_HEADER_LEN, |frame| frame[0] = 0x5a);
    assert!(device.transmit(&mut context()).is_none());

    let frame = radio.try_receive_tx().unwrap();
    assert_eq!(frame.as_slice()[0], 0x5a);
    assert!(device.transmit(&mut context()).is_none());
    drop(frame);
    assert!(device.transmit(&mut context()).is_some());
}

#[test]
fn unchanged_embassy_rx_rejects_invalid_and_full_submissions() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<1>();
    let (_device, radio) = resources.split([0; 6], rx_storage, tx_storage);

    assert_eq!(
        radio.try_send_rx(&[0; ETHERNET_HEADER_LEN]),
        Err(RxEnqueueError::LinkDown)
    );
    radio.set_link_state(LinkState::Up);
    assert_eq!(
        radio.try_send_rx(&[0; ETHERNET_HEADER_LEN - 1]),
        Err(RxEnqueueError::InvalidLength(FrameLengthError::TooShort))
    );
    radio.try_send_rx(&[0; ETHERNET_HEADER_LEN]).unwrap();
    assert_eq!(
        radio.try_send_rx(&[0; ETHERNET_HEADER_LEN]),
        Err(RxEnqueueError::QueueFull)
    );
}

#[test]
fn link_epoch_drops_stale_tx_instead_of_retargeting_it() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<1>();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    radio.set_link_state(LinkState::Up);

    device
        .transmit(&mut context())
        .unwrap()
        .consume(ETHERNET_HEADER_LEN, |frame| frame[0] = 0x6a);
    radio.set_link_state(LinkState::Down);
    radio.set_link_state(LinkState::Up);

    assert!(radio.try_receive_tx().is_none());
    assert!(device.transmit(&mut context()).is_some());
}

#[test]
fn link_epoch_drops_stale_rx_before_the_network_stack_observes_it() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<1>();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    radio.set_link_state(LinkState::Up);
    radio.try_send_rx(&[0x73; ETHERNET_HEADER_LEN]).unwrap();
    radio.set_link_state(LinkState::Down);
    radio.set_link_state(LinkState::Up);

    assert!(device.receive(&mut context()).is_none());
    radio.try_send_rx(&[0x74; ETHERNET_HEADER_LEN]).unwrap();
    let (rx, _) = device.receive(&mut context()).unwrap();
    rx.consume(|frame| assert_eq!(frame, &[0x74; ETHERNET_HEADER_LEN]));
}

#[test]
fn panicking_rx_consumer_returns_the_exact_payload_lease() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<1>();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    radio.set_link_state(LinkState::Up);
    radio.try_send_rx(&[0x81; ETHERNET_HEADER_LEN]).unwrap();
    let (rx, _tx) = device.receive(&mut context()).unwrap();

    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rx.consume::<(), _>(|_| panic!("test unwind"));
    }));
    assert!(unwind.is_err());
    radio.try_send_rx(&[0x82; ETHERNET_HEADER_LEN]).unwrap();
}

#[test]
fn panicking_tx_builder_returns_the_exact_payload_lease() {
    let (resources, rx_storage, tx_storage) = endpoint_resources::<1>();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1], rx_storage, tx_storage);
    radio.set_link_state(LinkState::Up);
    let tx = device.transmit(&mut context()).unwrap();

    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tx.consume::<(), _>(ETHERNET_HEADER_LEN, |_| panic!("test unwind"));
    }));
    assert!(unwind.is_err());
    assert_eq!(radio.tx_queue_len(), 0);
    assert!(device.transmit(&mut context()).is_some());
}
