use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Waker},
};

use embassy_net_driver::{Driver, HardwareAddress, LinkState, RxToken as _, TxToken as _};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_dma::RxHandoffPool;
use open_esp_radio_embassy_net::{
    DualPinnedNetworkRunner, ETHERNET_HEADER_LEN, FrameLengthError, NetworkInterfaceId,
    PinnedEndpointResources, PinnedNetworkRunner, PinnedTxPool, PinnedTxResources, Resources,
    RxEnqueueError, SharedPinnedRxQueue,
};

const FRAME_CAPACITY: usize = 64;
const TX_HEADROOM: usize = 28;
const TX_TRAILER: usize = 8;
const TEST_ETHERNET_LENGTH: usize = ETHERNET_HEADER_LEN + 8;
static SHARED_RX_RELEASES: AtomicUsize = AtomicUsize::new(0);

fn observe_shared_rx_release() {
    SHARED_RX_RELEASES.fetch_add(1, Ordering::Relaxed);
}

fn context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

macro_rules! split_pinned {
    ($resources:expr, $pool:expr, $interface:expr, $address:expr) => {{
        let tx_resources = Box::leak(Box::new(PinnedTxResources::new()));
        let (tx_provider, tx_consumer) = tx_resources.split($pool);
        let (device, rx_runner) = $resources.split(tx_provider, $interface, $address);
        (
            device,
            PinnedNetworkRunner::new($interface, rx_runner, tx_consumer),
        )
    }};
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
fn owned_device_ends_a_refilled_ingress_epoch_at_its_physical_depth() {
    let mut resources = Resources::<NoopRawMutex, FRAME_CAPACITY, 2>::new();
    let (mut device, radio) = resources.split([2, 0, 0, 0, 0, 1]);
    radio.try_send_rx(&[0x11; ETHERNET_HEADER_LEN]).unwrap();
    radio.try_send_rx(&[0x22; ETHERNET_HEADER_LEN]).unwrap();

    let (first, _reply) = device.receive(&mut context()).unwrap();
    first.consume(|_| ());
    radio.try_send_rx(&[0x33; ETHERNET_HEADER_LEN]).unwrap();
    let (second, _reply) = device.receive(&mut context()).unwrap();
    second.consume(|_| ());

    // The producer refilled the queue while smoltcp's current poll was
    // draining it. End that poll after the two-slot physical epoch; the
    // immediately self-woken next poll can consume the retained third frame.
    assert!(device.receive(&mut context()).is_none());
    let (third, _reply) = device.receive(&mut context()).unwrap();
    third.consume(|frame| assert_eq!(frame[0], 0x33));
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
fn pinned_rx_publisher_exposes_a_real_capacity_edge() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);
    let mut publisher = radio.rx_publisher();
    publisher.try_send(&[0; ETHERNET_HEADER_LEN]).unwrap();
    assert_eq!(publisher.free_capacity(), 0);

    {
        let mut ready = pin!(publisher.wait_ready());
        assert!(ready.as_mut().poll(&mut context()).is_pending());
        let received = device.receive(&mut context()).unwrap();
        drop(received);
        assert!(ready.as_mut().poll(&mut context()).is_ready());
    }
    assert_eq!(publisher.free_capacity(), 1);
    drop(publisher);

    let mut replacement = radio.rx_publisher();
    replacement
        .try_send(&[0; ETHERNET_HEADER_LEN])
        .expect("dropping a reservation returns its unique RX slot");
}

#[test]
fn pinned_device_has_no_artificial_frame_count_ingress_ceiling() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 2>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);
    let mut publisher = radio.rx_publisher();
    publisher.try_send(&[0x11; ETHERNET_HEADER_LEN]).unwrap();
    publisher.try_send(&[0x22; ETHERNET_HEADER_LEN]).unwrap();

    let first = device.receive(&mut context()).unwrap();
    drop(first);
    publisher.try_send(&[0x33; ETHERNET_HEADER_LEN]).unwrap();
    let second = device.receive(&mut context()).unwrap();
    drop(second);

    let (third, reply) = device.receive(&mut context()).unwrap();
    third.consume(|frame| assert_eq!(frame[0], 0x33));
    drop(reply);
}

#[test]
fn pinned_rx_slot_keeps_one_address_across_network_ownership() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);
    let mut publisher = radio.rx_publisher();

    publisher
        .try_send_parts([1; 6], [2; 6], 0x0800, &[3; 8])
        .unwrap();
    let (first, reply) = device.receive(&mut context()).unwrap();
    let first_address = first.consume(|frame| {
        assert_eq!(&frame[..6], &[1; 6]);
        assert_eq!(&frame[6..12], &[2; 6]);
        assert_eq!(&frame[12..14], &0x0800_u16.to_be_bytes());
        assert_eq!(&frame[14..], &[3; 8]);
        frame.as_mut_ptr()
    });
    drop(reply);

    publisher.try_send(&[0xa5; ETHERNET_HEADER_LEN]).unwrap();
    let (second, reply) = device.receive(&mut context()).unwrap();
    let second_address = second.consume(|frame| {
        assert_eq!(frame, &[0xa5; ETHERNET_HEADER_LEN]);
        frame.as_mut_ptr()
    });
    drop(reply);

    assert_eq!(first_address, second_address);
}

#[test]
fn shared_staging_slot_reaches_device_without_a_second_pool_copy() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 2>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let shared_pool = Box::leak(Box::new(RxHandoffPool::<FRAME_CAPACITY, 1>::new()));
    let shared_queue = Box::leak(Box::new(SharedPinnedRxQueue::<NoopRawMutex, 1>::new()));
    let (publisher, consumer) = shared_queue.split(shared_pool, observe_shared_rx_release);
    let (device, _radio) = split_pinned!(resources, tx_pool, NetworkInterfaceId::new(0), [0; 6]);
    let mut device = device.with_ingress_tx_reserve().with_shared_rx(consumer);
    SHARED_RX_RELEASES.store(0, Ordering::Relaxed);

    let (index, producer_address) =
        shared_pool
            .claim_radio(0)
            .publish(TEST_ETHERNET_LENGTH, |frame| {
                frame.fill(0x5a);
                frame.as_ptr() as usize
            });
    let index = shared_pool
        .claim_network(index)
        .republish(0, TEST_ETHERNET_LENGTH);
    publisher.publish(index);

    let (rx, reply) = device.receive(&mut context()).unwrap();
    let consumer_address = rx.consume(|frame| {
        assert_eq!(frame, &[0x5a; TEST_ETHERNET_LENGTH]);
        frame.as_ptr() as usize
    });
    drop(reply);
    assert_eq!(producer_address, consumer_address);
    assert_eq!(SHARED_RX_RELEASES.load(Ordering::Relaxed), 1);
    assert_eq!(shared_pool.claimed_slots(), 0);
}

#[test]
fn copied_and_shared_rx_follow_one_publication_order() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 3>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let shared_pool = Box::leak(Box::new(RxHandoffPool::<FRAME_CAPACITY, 2>::new()));
    let shared_queue = Box::leak(Box::new(SharedPinnedRxQueue::<NoopRawMutex, 2>::new()));
    let (shared, consumer) = shared_queue.split(shared_pool, observe_shared_rx_release);
    let (device, radio) = split_pinned!(resources, tx_pool, NetworkInterfaceId::new(0), [0; 6]);
    let radio = radio.with_shared_rx_ordering(&consumer);
    let mut copied = radio.rx_publisher();
    let mut device = device.with_ingress_tx_reserve().with_shared_rx(consumer);

    let publish_shared = |index: u8, byte: u8| {
        let (index, ()) = shared_pool
            .claim_radio(index)
            .publish(TEST_ETHERNET_LENGTH, |frame| frame.fill(byte));
        let index = shared_pool
            .claim_network(index)
            .republish(0, TEST_ETHERNET_LENGTH);
        shared.publish(index);
    };

    publish_shared(0, 0x11);
    copied.try_send(&[0x22; TEST_ETHERNET_LENGTH]).unwrap();
    publish_shared(1, 0x33);

    for expected in [0x11, 0x22, 0x33] {
        let (rx, reply) = device.receive(&mut context()).unwrap();
        rx.consume(|frame| assert_eq!(frame[0], expected));
        drop(reply);
    }
}

#[cfg(feature = "rx-delivery-observation")]
#[test]
fn observed_pinned_admission_precedes_network_visibility() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);
    let mut publisher = radio.rx_publisher();
    let mut callback_ran = false;

    publisher
        .try_send_parts_observed([1; 6], [2; 6], 0x0800, &[3; 8], || {
            callback_ran = true;
            assert!(device.receive(&mut context()).is_none());
        })
        .unwrap();

    assert!(callback_ran);
    assert!(device.receive(&mut context()).is_some());
}

#[cfg(feature = "rx-delivery-observation")]
#[test]
fn observed_contiguous_admission_precedes_network_visibility() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);
    let mut publisher = radio.rx_publisher();
    let mut callback_ran = false;

    publisher
        .try_send_observed(&[0xa5; ETHERNET_HEADER_LEN], || {
            callback_ran = true;
            assert!(device.receive(&mut context()).is_none());
        })
        .unwrap();

    assert!(callback_ran);
    assert!(device.receive(&mut context()).is_some());
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
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(
        resources,
        pool,
        NetworkInterfaceId::new(0),
        [2, 0, 0, 0, 0, 1]
    );
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
fn rejected_aggregate_candidate_requeues_the_same_pinned_frame() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let interface = NetworkInterfaceId::new(7);
    let (mut device, radio) = split_pinned!(resources, pool, interface, [0; 6]);

    for marker in [0x31, 0x72] {
        device
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    let consumer = radio.tx_consumer();
    let first = consumer.try_receive().unwrap();
    let mut second = consumer.try_receive().unwrap();
    assert_eq!(*first.tag(), interface);
    assert_eq!(*second.tag(), interface);
    let second_address = second.storage_mut().as_mut_ptr();
    assert_eq!(first.ethernet()[0], 0x31);
    assert_eq!(second.ethernet()[0], 0x72);

    consumer.requeue(second);
    let mut reclaimed = consumer.try_receive().unwrap();
    assert_eq!(*reclaimed.tag(), interface);
    assert_eq!(reclaimed.ethernet()[0], 0x72);
    assert_eq!(reclaimed.storage_mut().as_mut_ptr(), second_address);
    assert_eq!(consumer.queue_len(), 0);
}

#[test]
fn permanent_endpoints_keep_distinct_addresses_and_share_one_tx_fabric() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let station = [2, 0, 0, 0, 0, 1];
    let access_point = [2, 0, 0, 0, 0, 2];
    let station_interface = NetworkInterfaceId::new(3);
    let access_point_interface = NetworkInterfaceId::new(7);
    let (provider, consumer) = tx_resources.split(pool);
    let (mut station_device, station_rx) =
        station_resources.split(provider, station_interface, station);
    let (mut access_point_device, access_point_rx) =
        access_point_resources.split(provider, access_point_interface, access_point);
    let radio = DualPinnedNetworkRunner::new(
        station_interface,
        station_rx,
        access_point_interface,
        access_point_rx,
        consumer,
    );

    assert_eq!(
        station_device.hardware_address(),
        HardwareAddress::Ethernet(station)
    );
    assert_eq!(
        access_point_device.hardware_address(),
        HardwareAddress::Ethernet(access_point)
    );

    access_point_device
        .transmit(&mut context())
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |_| ());
    station_device
        .transmit(&mut context())
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |_| ());
    let consumer = radio.tx_consumer();
    assert_eq!(consumer.queue_len_for(station_interface), 1);
    assert_eq!(consumer.queue_len_for(access_point_interface), 1);
    let station_tx = consumer.for_interface(station_interface);
    let access_point_tx = consumer.for_interface(access_point_interface);
    assert_eq!(*station_tx.try_receive().unwrap().tag(), station_interface);
    assert_eq!(station_tx.queue_len(), 0);
    assert_eq!(access_point_tx.queue_len(), 1);
    assert_eq!(
        *access_point_tx.try_receive().unwrap().tag(),
        access_point_interface
    );
    access_point_device
        .transmit(&mut context())
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |_| ());
    assert_eq!(
        *access_point_tx.try_receive().unwrap().tag(),
        access_point_interface
    );
}

#[test]
fn dropped_pinned_tx_token_returns_its_reserved_slot() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, _radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);

    let token = device.transmit(&mut context()).unwrap();
    drop(token);
    assert!(device.transmit(&mut context()).is_some());
}

#[test]
fn pinned_ingress_credit_survives_saturated_application_egress() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 2>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (device, radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);
    let mut device = device.with_ingress_tx_reserve();
    let mut publisher = radio.rx_publisher();

    device
        .transmit(&mut context())
        .expect("one application credit")
        .consume(ETHERNET_HEADER_LEN, |frame| frame.fill(0x5a));
    assert!(device.transmit(&mut context()).is_none());

    publisher.try_send(&[0xa5; ETHERNET_HEADER_LEN]).unwrap();
    let (received, response) = device
        .receive(&mut context())
        .expect("reserved ingress response credit");
    received.consume(|frame| assert_eq!(frame[0], 0xa5));
    drop(response);
}

#[test]
fn pinned_rx_and_tx_depths_are_independent() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 3>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);
    let mut publisher = radio.rx_publisher();
    assert_eq!(device.capabilities().max_burst_size, Some(1));

    for marker in 0..3 {
        let mut frame = [0; ETHERNET_HEADER_LEN];
        frame[0] = marker;
        publisher.try_send(&frame).unwrap();
    }
    assert_eq!(publisher.queue_len(), 3);
    assert_eq!(publisher.free_capacity(), 0);

    device
        .transmit(&mut context())
        .unwrap()
        .consume(ETHERNET_HEADER_LEN, |frame| frame[0] = 0xa5);
    assert!(device.transmit(&mut context()).is_none());
    let consumer = radio.tx_consumer();
    assert_eq!(consumer.queue_len(), 1);
    assert_eq!(consumer.try_receive().unwrap().ethernet()[0], 0xa5);

    for marker in 0..3 {
        let (received, reply) = device.receive(&mut context()).unwrap();
        received.consume(|frame| assert_eq!(frame[0], marker));
        drop(reply);
    }
    assert_eq!(publisher.free_capacity(), 3);
}
