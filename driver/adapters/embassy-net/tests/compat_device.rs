use core::task::{Context, Waker};

use embassy_net_driver::{Driver, RxToken as _, TxToken as _};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_embassy_net::{
    ETHERNET_HEADER_LEN, FrameLengthError, Resources, RxEnqueueError,
};

const FRAME_CAPACITY: usize = 64;

fn context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

#[test]
fn rx_owner_moves_into_and_out_of_the_unchanged_embassy_driver() {
    let mut resources = Resources::<NoopRawMutex, FRAME_CAPACITY, 2>::new();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1]);
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
    let mut resources = Resources::<NoopRawMutex, FRAME_CAPACITY, 1>::new();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1]);

    device
        .transmit(&mut context())
        .unwrap()
        .consume(ETHERNET_HEADER_LEN, |frame| frame[0] = 0x5a);
    assert!(device.transmit(&mut context()).is_none());

    let frame = radio.try_receive_tx().unwrap();
    assert_eq!(frame.as_slice()[0], 0x5a);
    assert!(device.transmit(&mut context()).is_some());
}

#[test]
fn unchanged_embassy_rx_rejects_invalid_and_full_submissions() {
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
