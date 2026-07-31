use core::{
    mem::MaybeUninit,
    task::{Context, Waker},
};

use embassy_net_driver::{Driver, HardwareAddress, LinkState, RxToken as _, TxToken as _};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_embassy_net::{
    FrameLengthError, PinnedResources, PinnedTxPool, Resources, RxEnqueueError, ETHERNET_HEADER_LEN,
};

const FRAME_CAPACITY: usize = 64;
const TX_HEADROOM: usize = 28;
const TX_TRAILER: usize = 8;
const TEST_ETHERNET_LENGTH: usize = ETHERNET_HEADER_LEN + 8;

fn context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

#[test]
fn rx_frame_ownership_moves_into_and_out_of_driver() {
    let mut resources = Resources::<NoopRawMutex, FRAME_CAPACITY, 2>::new();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1]);
    let mut source = [0u8; ETHERNET_HEADER_LEN + 8];
    source[0] = 0xa1;
    source[6] = 0xb2;
    source[12..14].copy_from_slice(&0x0800u16.to_be_bytes());

    radio.try_send_rx(&source).unwrap();
    source.fill(0);

    let (rx, _reply) = device.receive(&mut context()).unwrap();
    let copied = rx.consume(|frame| {
        assert_eq!(frame[0], 0xa1);
        assert_eq!(frame[6], 0xb2);
        frame.len()
    });
    assert_eq!(copied, ETHERNET_HEADER_LEN + 8);
    assert_eq!(radio.rx_queue_len(), 0);
}

#[test]
fn tx_token_reserves_capacity_and_applies_backpressure() {
    let mut resources = Resources::<NoopRawMutex, FRAME_CAPACITY, 1>::new();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1]);

    device
        .transmit(&mut context())
        .unwrap()
        .consume(ETHERNET_HEADER_LEN, |frame| frame[0] = 0x5a);
    assert_eq!(radio.tx_queue_len(), 1);
    assert!(device.transmit(&mut context()).is_none());

    let frame = radio.try_receive_tx().unwrap();
    assert_eq!(frame.as_slice()[0], 0x5a);
    assert!(device.transmit(&mut context()).is_some());
}

#[test]
fn invalid_and_full_rx_frames_are_reported() {
    let mut resources = Resources::<NoopRawMutex, FRAME_CAPACITY, 1>::new();
    let (_device, radio) = resources.split([0; 6]);

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
fn link_and_device_metadata_match_radio_state() {
    let mut resources = Resources::<NoopRawMutex, FRAME_CAPACITY, 2>::new();
    let address = [2, 1, 2, 3, 4, 5];
    let (mut device, radio) = resources.split(address);

    assert!(device.link_state(&mut context()) == LinkState::Down);
    radio.set_link_state(LinkState::Up);
    assert!(device.link_state(&mut context()) == LinkState::Up);
    assert_eq!(
        device.hardware_address(),
        HardwareAddress::Ethernet(address)
    );
    assert_eq!(device.capabilities().max_transmission_unit, FRAME_CAPACITY);
}

#[test]
fn pinned_tx_slot_moves_between_network_and_radio_without_copying() {
    type TestResources = PinnedResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let storage = Box::leak(Box::new(MaybeUninit::<TestResources>::uninit()));
    let resources = TestResources::init_in_place(storage);
    let pool = Box::leak(Box::new(MaybeUninit::<TestPool>::uninit()));
    let pool = TestPool::pin_static(TestPool::init_in_place(pool));
    let (mut device, radio) = resources.split(pool, [2, 0, 0, 0, 0, 1]);
    device
        .transmit(&mut context())
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |frame| {
            frame.fill(0x5a);
            frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        });
    assert!(device.transmit(&mut context()).is_none());

    let mut frame = radio.try_receive_tx().unwrap();
    assert_eq!(frame.ethernet_offset(), TX_HEADROOM);
    assert_eq!(frame.ethernet_length(), TEST_ETHERNET_LENGTH);
    assert_eq!(frame.ethernet()[0], 0x5a);
    assert_eq!(&frame.ethernet()[12..14], &0x0800_u16.to_be_bytes());
    let first_address = frame.storage_mut().as_mut_ptr();
    frame.storage_mut()[..TX_HEADROOM].fill(0xc3);
    drop(frame);

    device
        .transmit(&mut context())
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0xa6));
    let mut frame = radio.try_receive_tx().unwrap();
    assert_eq!(frame.storage_mut().as_mut_ptr(), first_address);
    assert_eq!(&frame.storage_mut()[..TX_HEADROOM], &[0xc3; TX_HEADROOM]);
    assert_eq!(frame.ethernet(), &[0xa6; TEST_ETHERNET_LENGTH]);
}

#[test]
fn dropped_pinned_tx_token_returns_its_reserved_slot() {
    type TestResources = PinnedResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, _radio) = resources.split(pool, [0; 6]);

    let token = device.transmit(&mut context()).unwrap();
    drop(token);
    assert!(device.transmit(&mut context()).is_some());
}
