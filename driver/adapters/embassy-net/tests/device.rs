use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use std::{sync::Arc, task::Wake};

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
use embassy_net_driver::EgressGrantCompletion;
use embassy_net_driver::{
    Checksum, ChecksumCapabilities, Driver, HardwareAddress, LinkState, RxToken as _, TxToken as _,
};
#[cfg(feature = "tx-egress-scheduling")]
use embassy_net_driver::{
    EgressAdmission, EgressGrantMode, EgressKey, EgressRoute, EgressSchedule,
};
#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
use embassy_net_driver::{EgressDemand, EgressDemandId, EgressDemandLevel, EgressDemandUpdate};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_dma::RxHandoffPool;
#[cfg(feature = "tx-staging-copy-probe")]
use open_esp_radio_embassy_net::PinnedNetworkTxFrame;
#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
use open_esp_radio_embassy_net::{
    DefaultEgressControlPlane, DefaultEgressNetworkScheduler, DefaultEgressNetworkState,
    DefaultEgressRadioScheduler, EgressBurstGrant, EgressGrantKey, EgressGrantProgress,
};
use open_esp_radio_embassy_net::{
    DualPinnedNetworkRunner, ETHERNET_HEADER_LEN, FrameLengthError, NetworkEndpointConfig,
    NetworkInterfaceId, PinnedEndpointResources, PinnedNetworkRunner, PinnedTxPool,
    PinnedTxResources, Resources, RxEnqueueError, SharedPinnedRxQueue,
};
#[cfg(feature = "tx-egress-scheduling")]
use open_esp_radio_embassy_net::{EgressPeerDirectory, EgressPeerIdentity};
#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
use open_esp_radio_embassy_net::{EgressShadowGrant, TX_PERFORMANCE};
#[cfg(feature = "tx-core1-materializer-probe")]
use open_esp_radio_embassy_net::{
    PinnedTxCore1MaterializationPoll, configure_tx_core1_materializer_probe,
};

const FRAME_CAPACITY: usize = 64;
const TX_HEADROOM: usize = 28;
const TX_TRAILER: usize = 8;
const TEST_ETHERNET_LENGTH: usize = ETHERNET_HEADER_LEN + 8;
static SHARED_RX_RELEASES: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "tx-core1-materializer-probe")]
static CORE1_MATERIALIZER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct CountWake(Arc<AtomicUsize>);

impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

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
        let endpoint = NetworkEndpointConfig::per_link_destination($interface, $address);
        let (device, rx_runner) = $resources.split(tx_provider, endpoint);
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

    // The producer refilled the queue while the network stack's current poll was
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

#[cfg(feature = "tx-egress-scheduling")]
#[test]
fn shared_rx_wrapper_delegates_the_inner_egress_schedule() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let shared_pool = Box::leak(Box::new(RxHandoffPool::<FRAME_CAPACITY, 1>::new()));
    let shared_queue = Box::leak(Box::new(SharedPinnedRxQueue::<NoopRawMutex, 1>::new()));
    let (_publisher, consumer) = shared_queue.split(shared_pool, observe_shared_rx_release);
    let (device, radio) = split_pinned!(
        resources,
        tx_pool,
        NetworkInterfaceId::new(0),
        [2, 0, 0, 0, 0, 1]
    );
    let mut device = device.with_ingress_tx_reserve().with_shared_rx(consumer);

    let first_route = EgressRoute {
        destination: HardwareAddress::Ethernet([2, 0, 0, 0, 0, 3]),
        traffic_class: 0,
    };
    let second_route = EgressRoute {
        destination: HardwareAddress::Ethernet([2, 0, 0, 0, 0, 4]),
        traffic_class: 0,
    };
    assert_ne!(
        device.egress_key(first_route),
        device.egress_key(second_route),
        "shared RX middleware must preserve per-destination classification"
    );

    assert_eq!(device.egress_schedule(), None);
    radio.set_link_state(LinkState::Up);
    let schedule = device.egress_schedule().unwrap();
    assert_eq!(schedule.max_packets_per_key().get(), 32);
    assert_eq!(schedule.dispatch_quantum().get(), 4);
    assert_eq!(schedule.epoch(), 1);
    radio.set_link_state(LinkState::Down);
    assert_eq!(device.egress_schedule(), None);
    radio.set_link_state(LinkState::Up);
    let schedule = device.egress_schedule().unwrap();
    assert_eq!(
        schedule,
        EgressSchedule::new(
            core::num::NonZeroU8::new(32).unwrap(),
            core::num::NonZeroU8::new(4).unwrap(),
            core::num::NonZeroU16::new(16).unwrap(),
            2,
            EgressGrantMode::StackSelected,
        )
    );
}

#[cfg(feature = "tx-egress-scheduling")]
#[test]
fn endpoint_topology_classifies_sta_and_ap_routes_before_admission() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let station_resources = Box::leak(Box::new(TestResources::new()));
    let access_point_resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (provider, _consumer) = tx_resources.split(pool);
    let station_interface = NetworkInterfaceId::new(0);
    let access_point_interface = NetworkInterfaceId::new(1);
    let (mut station, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(station_interface, [2, 0, 0, 0, 0, 1]),
    );
    let (mut access_point, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(access_point_interface, [2, 0, 0, 0, 0, 2]),
    );
    station_rx.set_link_state(LinkState::Up);
    access_point_rx.set_link_state(LinkState::Up);

    let first_route = EgressRoute {
        destination: HardwareAddress::Ethernet([2, 0, 0, 0, 0, 3]),
        traffic_class: 0,
    };
    let second_route = EgressRoute {
        destination: HardwareAddress::Ethernet([2, 0, 0, 0, 0, 4]),
        traffic_class: 0,
    };
    assert_eq!(
        station.egress_key(first_route),
        station.egress_key(second_route),
        "infrastructure STA routes share one physical radio peer"
    );
    assert_ne!(
        access_point.egress_key(first_route),
        access_point.egress_key(second_route),
        "SoftAP destinations remain independent scheduling domains"
    );

    let before_reconnect = station.egress_key(first_route);
    station_rx.set_link_state(LinkState::Down);
    station_rx.set_link_state(LinkState::Up);
    assert_ne!(
        station.egress_key(first_route),
        before_reconnect,
        "a new link lifecycle must invalidate the previous device key"
    );
}

#[cfg(feature = "tx-egress-scheduling")]
#[test]
fn associated_peer_directory_advances_queue_identity_on_reassociation() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let peers = EgressPeerDirectory::<2>::new();
    let (provider, _consumer) = tx_resources.split(pool);
    let interface = NetworkInterfaceId::new(1);
    let (mut device, radio) = resources.split(
        provider,
        NetworkEndpointConfig::associated_peers(interface, [2, 0, 0, 0, 0, 2], &peers),
    );
    radio.set_link_state(LinkState::Up);

    let first_address = [2, 0, 0, 0, 0, 3];
    let second_address = [2, 0, 0, 0, 0, 4];
    let first_route = EgressRoute {
        destination: HardwareAddress::Ethernet(first_address),
        traffic_class: 0,
    };
    let second_route = EgressRoute {
        destination: HardwareAddress::Ethernet(second_address),
        traffic_class: 0,
    };

    let unresolved = device.egress_key(first_route);
    assert_eq!(device.egress_schedule().unwrap().epoch(), 1);

    let first_generation = [
        EgressPeerIdentity::try_new(first_address, 1, 7),
        EgressPeerIdentity::try_new(second_address, 2, 9),
    ];
    assert_eq!(peers.replace(&first_generation), Ok(true));
    assert_eq!(device.egress_schedule().unwrap().epoch(), 2);
    let associated = device.egress_key(first_route);
    assert_ne!(associated, unresolved);
    assert_ne!(associated, device.egress_key(second_route));

    assert_eq!(peers.replace(&first_generation), Ok(false));
    assert_eq!(device.egress_schedule().unwrap().epoch(), 2);

    let replacement = [EgressPeerIdentity::try_new(first_address, 1, 10), None];
    assert_eq!(peers.replace(&replacement), Ok(true));
    assert_eq!(device.egress_schedule().unwrap().epoch(), 3);
    assert_ne!(device.egress_key(first_route), associated);
    assert_ne!(
        device.egress_key(first_route),
        device.egress_key(second_route),
        "an unknown route keeps its link identity instead of inheriting a peer grant"
    );
}

#[cfg(feature = "tx-egress-scheduling")]
#[test]
fn final_keyed_admission_rejects_stale_and_foreign_peer_keys_before_sram_claim() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let peers = EgressPeerDirectory::<1>::new();
    let address = [2, 0, 0, 0, 0, 3];
    peers
        .replace(&[EgressPeerIdentity::try_new(address, 1, 7)])
        .unwrap();
    let (provider, consumer) = tx_resources.split(pool);
    let interface = NetworkInterfaceId::new(1);
    let (mut device, radio) = resources.split(
        provider,
        NetworkEndpointConfig::associated_peers(interface, [2, 0, 0, 0, 0, 2], &peers),
    );
    radio.set_link_state(LinkState::Up);
    let route = EgressRoute {
        destination: HardwareAddress::Ethernet(address),
        traffic_class: 0,
    };

    let generation_seven = device.egress_key(route);
    assert!(matches!(
        device.transmit_for(&mut context(), generation_seven),
        EgressAdmission::Granted(_)
    ));

    peers
        .replace(&[EgressPeerIdentity::try_new(address, 1, 8)])
        .unwrap();
    assert!(matches!(
        device.transmit_for(&mut context(), generation_seven),
        EgressAdmission::KeyDeferred
    ));

    let current = device.egress_key(route);
    let token = match device.transmit_for(&mut context(), current) {
        EgressAdmission::Granted(token) => token,
        _ => panic!("current generation receives a physical token"),
    };
    token.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0x5a));
    let radio = consumer.for_interface(interface);
    let frame = radio.try_receive_direct().unwrap();
    assert_eq!(frame.tag().interface(), interface);
    let metadata = radio.direct_metadata(&frame);
    assert_eq!(metadata.egress_key(), Some(current));
    let retained_identity = metadata.associated_peer_identity().unwrap();
    assert_eq!(retained_identity.interface(), interface.value());
    assert_ne!(retained_identity.schedule_epoch(), 0);
    assert_eq!(retained_identity.peer_slot().get(), 1);
    assert_eq!(retained_identity.peer_generation().get(), 8);
    assert_eq!(retained_identity.traffic_class(), 0);
    drop(frame);

    let mut foreign_words = current.words();
    foreign_words[0] ^= 1 << 8;
    assert!(matches!(
        device.transmit_for(&mut context(), EgressKey::from_words(foreign_words)),
        EgressAdmission::KeyDeferred
    ));
}

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
#[test]
fn authoritative_device_defers_sram_until_core0_grants_the_demand() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let control = Box::leak(Box::new(DefaultEgressControlPlane::<NoopRawMutex>::new()));
    let (network_control, radio_control) = control.split();
    let network_state = Box::leak(Box::new(DefaultEgressNetworkState::new()));
    let network_control = Box::leak(Box::new(DefaultEgressNetworkScheduler::new(
        network_control,
        network_state,
    )));
    let (provider, _consumer) = tx_resources.split(pool);
    let (device, radio) = resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1]),
    );
    let mut device = device.with_egress_control(network_control);
    radio.set_link_state(LinkState::Up);
    let schedule = device.egress_schedule().unwrap();
    let key = device.egress_key(EgressRoute {
        destination: HardwareAddress::Ethernet([2, 0, 0, 0, 0, 2]),
        traffic_class: 0,
    });
    let mut cx = context();
    device.update_egress_demand(
        &mut cx,
        EgressDemandUpdate::Reset {
            schedule_epoch: schedule.epoch(),
        },
    );
    device.update_egress_demand(
        &mut cx,
        EgressDemandUpdate::Active(EgressDemand::new(
            EgressDemandId::new(schedule.epoch(), core::num::NonZeroU32::new(1).unwrap()),
            key,
            EgressDemandLevel::new(core::num::NonZeroU16::new(32).unwrap(), true),
        )),
    );

    assert!(matches!(
        device.transmit_for(&mut cx, key),
        EgressAdmission::KeyDeferred
    ));
    let mut radio_control = DefaultEgressRadioScheduler::new(radio_control);
    assert!(radio_control.service_shadow());
    assert_eq!(control.snapshot().demand_publications, 2);
    assert_eq!(control.snapshot().radio_demand_updates, 2);
    assert_eq!(control.snapshot().radio_demand_rejected, 0);
}

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
#[test]
fn saturated_bulk_horizon_cannot_consume_the_control_credit() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;

    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let control = Box::leak(Box::new(DefaultEgressControlPlane::<NoopRawMutex>::new()));
    let (network_control, _radio_control) = control.split();
    let network_state = Box::leak(Box::new(DefaultEgressNetworkState::new()));
    let network_control = Box::leak(Box::new(DefaultEgressNetworkScheduler::new(
        network_control,
        network_state,
    )));
    let (provider, consumer) = tx_resources.split(pool);
    let interface = NetworkInterfaceId::new(0);
    let (device, radio) = resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(interface, [2, 0, 0, 0, 0, 1]),
    );
    let mut device = device.with_egress_control(network_control);
    radio.set_link_state(LinkState::Up);

    let initial = consumer.ownership_snapshot_for(interface);
    assert_eq!(initial.free, 3);
    assert_eq!(initial.control_free, 1);

    for fill in [0x5a, 0x5b] {
        device
            .transmit(&mut context())
            .expect("both ordinary credits remain available")
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(fill));
    }
    assert!(device.transmit(&mut context()).is_none());
    let bulk_full = consumer.ownership_snapshot_for(interface);
    assert_eq!(bulk_full.free, 1);
    assert_eq!(bulk_full.control_free, 1);
    assert_eq!(bulk_full.ready_for_interface, 2);

    device
        .transmit_control(&mut context())
        .expect("control admission owns its disjoint physical credit")
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0xc0));
    let fully_published = consumer.ownership_snapshot_for(interface);
    assert_eq!(fully_published.free, 0);
    assert_eq!(fully_published.control_free, 0);
    assert_eq!(fully_published.ready_for_interface, 3);

    while let Some(frame) = consumer.for_interface(interface).try_receive_direct() {
        drop(frame);
    }
    let returned = consumer.ownership_snapshot_for(interface);
    assert_eq!(returned.free, 3);
    assert_eq!(returned.control_free, 1);
    assert_eq!(returned.ready_for_interface, 0);
}

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
#[test]
fn affine_current_and_standby_grants_materialize_and_close_exactly() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    // Two ordinary data credits plus the global control reserve.
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;

    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let control = Box::leak(Box::new(DefaultEgressControlPlane::<NoopRawMutex>::new()));
    let (network_control, radio_control) = control.split();
    let network_state = Box::leak(Box::new(DefaultEgressNetworkState::new()));
    let network_control = Box::leak(Box::new(DefaultEgressNetworkScheduler::new(
        network_control,
        network_state,
    )));
    let (provider, consumer) = tx_resources.split(pool);
    let interface = NetworkInterfaceId::new(0);
    let (device, radio) = resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(interface, [2, 0, 0, 0, 0, 1]),
    );
    let mut device = device.with_egress_control(network_control);
    radio.set_link_state(LinkState::Up);
    let schedule = device.egress_schedule().unwrap();
    assert_eq!(schedule.grant_mode(), EgressGrantMode::Authoritative);
    let key = device.egress_key(EgressRoute {
        destination: HardwareAddress::Ethernet([2, 0, 0, 0, 0, 2]),
        traffic_class: 0,
    });
    let id = EgressDemandId::new(schedule.epoch(), core::num::NonZeroU32::new(1).unwrap());
    let demand = EgressDemand::new(
        id,
        key,
        EgressDemandLevel::new(core::num::NonZeroU16::new(4).unwrap(), false),
    );
    let mut cx = context();
    device.update_egress_demand(
        &mut cx,
        EgressDemandUpdate::Reset {
            schedule_epoch: schedule.epoch(),
        },
    );
    device.update_egress_demand(&mut cx, EgressDemandUpdate::Active(demand));

    let mut radio_control = DefaultEgressRadioScheduler::new(radio_control);
    assert!(radio_control.service_shadow());
    let grant = EgressBurstGrant::new(
        core::num::NonZeroU32::new(7).unwrap(),
        demand,
        core::num::NonZeroU8::new(2).unwrap(),
        core::num::NonZeroU32::new(1_000).unwrap(),
    );
    radio_control.try_issue_grant(grant).unwrap();
    let standby_demand = EgressDemand::new(
        id,
        key,
        EgressDemandLevel::new(core::num::NonZeroU16::new(2).unwrap(), false),
    );
    let standby = EgressBurstGrant::new(
        core::num::NonZeroU32::new(8).unwrap(),
        standby_demand,
        core::num::NonZeroU8::new(2).unwrap(),
        core::num::NonZeroU32::new(1_000).unwrap(),
    );
    radio_control.try_issue_grant(standby).unwrap();
    let observed = device.poll_egress_grant(&mut cx).unwrap();
    assert_eq!(observed.serial(), grant.serial());
    assert_eq!(observed.demand(), demand);
    let observed_standby = device.poll_egress_grant(&mut cx).unwrap();
    assert_eq!(observed_standby.serial(), standby.serial());
    assert_eq!(observed_standby.demand(), standby_demand);
    assert!(device.poll_egress_grant(&mut cx).is_none());
    assert!(matches!(
        device.transmit_granted(&mut cx, core::num::NonZeroU32::new(9).unwrap()),
        EgressAdmission::KeyDeferred
    ));

    let token = match device.transmit_granted(&mut cx, grant.serial()) {
        EgressAdmission::Granted(token) => token,
        _ => panic!("the authoritative grant must materialize one SRAM owner"),
    };
    token.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0x5a));
    let token = match device.transmit_granted(&mut cx, grant.serial()) {
        EgressAdmission::Granted(token) => token,
        _ => panic!("the authoritative grant must materialize the second SRAM owner"),
    };
    token.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0x5b));
    assert_eq!(consumer.queue_len_for(interface), 2);

    // Return one physical credit while retaining the exhausted affine grant.
    // A third packet must still be deferred before it can claim that SRAM.
    let interface_consumer = consumer.for_interface(interface);
    let first = interface_consumer.try_receive_direct().unwrap();
    assert_eq!(
        interface_consumer.direct_metadata(&first).egress_key(),
        Some(key)
    );
    drop(first);
    assert!(matches!(
        device.transmit_granted(&mut cx, grant.serial()),
        EgressAdmission::KeyDeferred
    ));
    assert_eq!(consumer.queue_len_for(interface), 1);

    // Neither a foreign serial nor a false spent-credit count may close the
    // live affine owner.
    device.finish_egress_grant(
        &mut cx,
        EgressGrantCompletion::new(core::num::NonZeroU32::new(9).unwrap(), 2, None),
    );
    device.finish_egress_grant(&mut cx, EgressGrantCompletion::new(grant.serial(), 1, None));
    assert!(!radio_control.service_shadow_control_observed(|_| {}, |_| {}));
    assert!(matches!(
        device.transmit_granted(&mut cx, grant.serial()),
        EgressAdmission::KeyDeferred
    ));

    device.finish_egress_grant(
        &mut cx,
        EgressGrantCompletion::new(
            grant.serial(),
            2,
            Some(EgressDemandLevel::new(
                core::num::NonZeroU16::new(2).unwrap(),
                false,
            )),
        ),
    );
    let mut progress = std::vec::Vec::new();
    assert!(radio_control.service_shadow_control_observed(|_| {}, |update| progress.push(update)));
    assert_eq!(
        progress,
        std::vec![EgressGrantProgress::Finished {
            serial: grant.serial(),
            used_frames: 2,
            remaining: Some(EgressDemandLevel::new(
                core::num::NonZeroU16::new(2).unwrap(),
                false,
            )),
        }]
    );
    assert_eq!(control.snapshot().network_grants, 2);
    assert_eq!(control.snapshot().grant_progress_publications, 1);
    assert_eq!(control.snapshot().radio_grant_updates, 1);

    drop(
        consumer
            .for_interface(interface)
            .try_receive_direct()
            .unwrap(),
    );
    for byte in [0x5c, 0x5d] {
        let token = match device.transmit_granted(&mut cx, standby.serial()) {
            EgressAdmission::Granted(token) => token,
            _ => panic!("the standby grant must authorize its disjoint prefix"),
        };
        token.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(byte));
    }
    device.finish_egress_grant(
        &mut cx,
        EgressGrantCompletion::new(standby.serial(), 2, None),
    );
    let mut standby_progress = std::vec::Vec::new();
    assert!(
        radio_control
            .service_shadow_control_observed(|_| {}, |update| standby_progress.push(update))
    );
    assert_eq!(
        standby_progress,
        std::vec![EgressGrantProgress::Finished {
            serial: standby.serial(),
            used_frames: 2,
            remaining: None,
        }]
    );
    assert_eq!(control.snapshot().network_grants, 2);
    assert_eq!(control.snapshot().grant_progress_publications, 2);
    assert_eq!(control.snapshot().radio_grant_updates, 2);
}

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
#[test]
fn terminal_current_does_not_block_standby_admission_when_progress_transport_is_full() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;

    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let control = Box::leak(Box::new(DefaultEgressControlPlane::<NoopRawMutex>::new()));
    let (network_control, radio_control) = control.split();
    let network_state = Box::leak(Box::new(DefaultEgressNetworkState::new()));
    let network_control = Box::leak(Box::new(DefaultEgressNetworkScheduler::new(
        network_control,
        network_state,
    )));
    let (provider, consumer) = tx_resources.split(pool);
    let interface = NetworkInterfaceId::new(0);
    let (device, radio) = resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(interface, [2, 0, 0, 0, 0, 1]),
    );
    let mut device = device.with_egress_control(network_control);
    radio.set_link_state(LinkState::Up);
    let schedule = device.egress_schedule().unwrap();
    let key = device.egress_key(EgressRoute {
        destination: HardwareAddress::Ethernet([2, 0, 0, 0, 0, 2]),
        traffic_class: 0,
    });
    let id = EgressDemandId::new(schedule.epoch(), core::num::NonZeroU32::new(1).unwrap());
    let demand = EgressDemand::new(
        id,
        key,
        EgressDemandLevel::new(core::num::NonZeroU16::new(1).unwrap(), false),
    );
    let mut cx = context();
    device.update_egress_demand(
        &mut cx,
        EgressDemandUpdate::Reset {
            schedule_epoch: schedule.epoch(),
        },
    );
    device.update_egress_demand(&mut cx, EgressDemandUpdate::Active(demand));

    let mut radio_control = DefaultEgressRadioScheduler::new(radio_control);
    assert!(radio_control.service_shadow());
    let current = EgressBurstGrant::new(
        core::num::NonZeroU32::new(7).unwrap(),
        demand,
        core::num::NonZeroU8::new(1).unwrap(),
        core::num::NonZeroU32::new(1_000).unwrap(),
    );
    let standby = EgressBurstGrant::new(
        core::num::NonZeroU32::new(8).unwrap(),
        demand,
        core::num::NonZeroU8::new(1).unwrap(),
        core::num::NonZeroU32::new(1_000).unwrap(),
    );
    radio_control.try_issue_grant(current).unwrap();
    radio_control.try_issue_grant(standby).unwrap();
    assert_eq!(
        device.poll_egress_grant(&mut cx).unwrap().serial(),
        current.serial()
    );
    assert_eq!(
        device.poll_egress_grant(&mut cx).unwrap().serial(),
        standby.serial()
    );

    let token = match device.transmit_granted(&mut cx, current.serial()) {
        EgressAdmission::Granted(token) => token,
        _ => panic!("the current grant must own its exact packet"),
    };
    token.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0x5a));
    drop(
        consumer
            .for_interface(interface)
            .try_receive_direct()
            .unwrap(),
    );

    // Fill the ordered Core1-to-Core0 stream with valid demand transitions.
    // The current completion must remain locally retained when it cannot be
    // published, without invalidating the already-issued standby quantum.
    for ready in 2..=17 {
        device.update_egress_demand(
            &mut cx,
            EgressDemandUpdate::Active(EgressDemand::new(
                id,
                key,
                EgressDemandLevel::new(core::num::NonZeroU16::new(ready).unwrap(), false),
            )),
        );
    }
    device.finish_egress_grant(
        &mut cx,
        EgressGrantCompletion::new(current.serial(), 1, None),
    );
    assert_eq!(control.snapshot().grant_progress_full, 1);

    let token = match device.transmit_granted(&mut cx, standby.serial()) {
        EgressAdmission::Granted(token) => token,
        _ => panic!("a terminal current grant must not block its affine standby"),
    };
    token.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0x5b));
    device.finish_egress_grant(
        &mut cx,
        EgressGrantCompletion::new(standby.serial(), 1, None),
    );

    // Free transport capacity, retry from the permanent device owner, and
    // prove that completion order remains current then standby.
    let mut progress = std::vec::Vec::new();
    assert!(radio_control.service_shadow_control_observed(|_| {}, |value| progress.push(value)));
    assert!(device.egress_schedule().is_some());
    for _ in 0..4 {
        radio_control.service_shadow_control_observed(|_| {}, |value| progress.push(value));
    }
    assert_eq!(
        progress,
        std::vec![
            EgressGrantProgress::Finished {
                serial: current.serial(),
                used_frames: 1,
                remaining: None,
            },
            EgressGrantProgress::Finished {
                serial: standby.serial(),
                used_frames: 1,
                remaining: None,
            },
        ]
    );
    assert_eq!(control.snapshot().grant_progress_publications, 2);
    assert_eq!(control.snapshot().radio_grant_updates, 2);
}

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
#[test]
fn software_demand_can_close_before_its_pinned_radio_owner_is_consumed() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let control = Box::leak(Box::new(DefaultEgressControlPlane::<NoopRawMutex>::new()));
    let (network_control, radio_control) = control.split();
    let network_state = Box::leak(Box::new(DefaultEgressNetworkState::new()));
    let network_control = Box::leak(Box::new(DefaultEgressNetworkScheduler::new(
        network_control,
        network_state,
    )));
    let (provider, consumer) = tx_resources.split(pool);
    let interface = NetworkInterfaceId::new(0);
    let (device, radio) = resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(interface, [2, 0, 0, 0, 0, 1]),
    );
    let mut device = device.with_egress_control(network_control);
    radio.set_link_state(LinkState::Up);
    let schedule = device.egress_schedule().unwrap();
    let key = device.egress_key(EgressRoute {
        destination: HardwareAddress::Ethernet([2, 0, 0, 0, 0, 2]),
        traffic_class: 0,
    });
    let id = EgressDemandId::new(schedule.epoch(), core::num::NonZeroU32::new(1).unwrap());
    let mut cx = context();
    device.update_egress_demand(
        &mut cx,
        EgressDemandUpdate::Reset {
            schedule_epoch: schedule.epoch(),
        },
    );
    let demand = EgressDemand::new(
        id,
        key,
        EgressDemandLevel::new(core::num::NonZeroU16::new(1).unwrap(), false),
    );
    device.update_egress_demand(&mut cx, EgressDemandUpdate::Active(demand));

    let mut radio_control = DefaultEgressRadioScheduler::new(radio_control);
    assert!(radio_control.service_shadow());
    let grant = EgressBurstGrant::new(
        core::num::NonZeroU32::new(9).unwrap(),
        demand,
        core::num::NonZeroU8::MIN,
        core::num::NonZeroU32::new(1_000).unwrap(),
    );
    radio_control.try_issue_grant(grant).unwrap();
    let observed = device.poll_egress_grant(&mut cx).unwrap();
    assert_eq!(observed.serial(), grant.serial());
    assert_eq!(observed.demand(), demand);

    match device.transmit_for(&mut cx, key) {
        EgressAdmission::Granted(token) => {
            token.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0x5a));
        }
        _ => panic!("active software demand must materialize one pinned frame"),
    }
    assert_eq!(consumer.queue_len_for(interface), 1);

    // Xarxa legitimately reports only its remaining software backlog. Once
    // the final packet has moved into the SRAM owner above, the next stack
    // observation may end the demand lifetime before Core0 consumes that
    // owner. Demand and admitted-radio-work are therefore different
    // frontiers; an authoritative design must bind the latter to a grant or
    // admission receipt instead of extending software demand artificially.
    device.update_egress_demand(&mut cx, EgressDemandUpdate::Inactive { id, key });
    device.finish_egress_grant(&mut cx, EgressGrantCompletion::new(grant.serial(), 1, None));
    let mut updates = std::vec::Vec::new();
    let mut progress = std::vec::Vec::new();
    assert!(radio_control.service_shadow_control_observed(
        |update| updates.push(update),
        |update| progress.push(update),
    ));
    assert_eq!(updates, std::vec![EgressDemandUpdate::Inactive { id, key }]);
    assert_eq!(
        progress,
        std::vec![EgressGrantProgress::Finished {
            serial: grant.serial(),
            used_frames: 1,
            remaining: None,
        }]
    );
    assert_eq!(consumer.queue_len_for(interface), 1);
    let frame = consumer
        .for_interface(interface)
        .try_receive_direct()
        .unwrap();
    assert_eq!(
        consumer
            .for_interface(interface)
            .direct_metadata(&frame)
            .egress_key(),
        Some(key)
    );
}

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
#[test]
fn shadow_grant_spends_local_credits_without_changing_real_admission() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let peers = EgressPeerDirectory::<1>::new();
    let shadow = EgressShadowGrant::new();
    let address = [2, 0, 0, 0, 0, 3];
    let identity = EgressPeerIdentity::try_new(address, 1, 7).unwrap();
    peers.replace(&[Some(identity)]).unwrap();
    let (provider, _consumer) = tx_resources.split(pool);
    let interface = NetworkInterfaceId::new(1);
    let endpoint = NetworkEndpointConfig::associated_peers(interface, [2, 0, 0, 0, 0, 2], &peers)
        .with_shadow_grant(&shadow);
    let (mut device, radio) = resources.split(provider, endpoint);
    radio.set_link_state(LinkState::Up);
    let key = device.egress_key(EgressRoute {
        destination: HardwareAddress::Ethernet(address),
        traffic_class: 0,
    });
    let before = TX_PERFORMANCE.snapshot();

    assert!(matches!(
        device.transmit_for(&mut context(), key),
        EgressAdmission::Granted(_)
    ));
    shadow
        .publish(
            EgressGrantKey::new(interface.value(), identity.slot(), identity.generation(), 0),
            core::num::NonZeroU8::new(2).unwrap(),
        )
        .unwrap();
    for _ in 0..3 {
        assert!(matches!(
            device.transmit_for(&mut context(), key),
            EgressAdmission::Granted(_)
        ));
    }
    shadow
        .publish(
            EgressGrantKey::new(
                interface.value(),
                identity.slot(),
                core::num::NonZeroU32::new(8).unwrap(),
                0,
            ),
            core::num::NonZeroU8::new(1).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        device.transmit_for(&mut context(), key),
        EgressAdmission::Granted(_)
    ));

    let delta = TX_PERFORMANCE.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.shadow_grant_checks, 5);
    assert_eq!(delta.shadow_grant_matches, 2);
    assert_eq!(delta.shadow_grant_no_window, 1);
    assert_eq!(delta.shadow_grant_key_mismatch, 1);
    assert_eq!(delta.shadow_grant_credit_exhausted, 1);
    assert_eq!(delta.shadow_grant_unclassified, 0);
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

#[cfg(feature = "diagnostics")]
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

#[cfg(feature = "diagnostics")]
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
    radio.set_link_state(LinkState::Up);
    device
        .transmit(&mut context())
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |frame| {
            frame.fill(0x5a);
            frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        });
    assert!(device.transmit(&mut context()).is_none());

    let mut frame = radio.try_receive_tx_direct().unwrap();
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
    let mut frame = radio.try_receive_tx_direct().unwrap();
    assert_eq!(frame.storage_mut().as_mut_ptr(), first_address);
    assert_eq!(&frame.storage_mut()[..TX_HEADROOM], &[0xc3; TX_HEADROOM]);
    assert_eq!(frame.ethernet(), &[0xa6; TEST_ETHERNET_LENGTH]);
}

#[cfg(feature = "tx-staging-copy-probe")]
#[test]
fn scheduled_staged_tx_keeps_ownership_until_one_dma_promotion_copy() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;

    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let station_interface = NetworkInterfaceId::new(0);
    let access_point_interface = NetworkInterfaceId::new(1);
    let (provider, consumer) = tx_resources.split(pool);
    let (mut station, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(station_interface, [2, 0, 0, 0, 0, 1]),
    );
    let (access_point, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(access_point_interface, [2, 0, 0, 0, 0, 2]),
    );
    let mut access_point = access_point.with_tx_staging_copy_selected(true);
    let station_tx = consumer.for_interface(station_interface);
    let access_point_tx = consumer.for_interface(access_point_interface);
    station_rx.set_link_state(LinkState::Up);
    access_point_rx.set_link_state(LinkState::Up);

    // Occupy the sole physical DMA credit through the direct STA endpoint.
    station
        .transmit(&mut context())
        .expect("the physical DMA credit starts free")
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0xd1));
    let direct = station_tx
        .try_receive_direct()
        .expect("STA publishes a direct DMA owner");

    // AP admission remains possible because it consumes a distinct CPU-packet
    // credit. Scheduling sees the immutable frame even while DMA is full.
    access_point
        .transmit(&mut context())
        .expect("the staged packet credit is independent of DMA")
        .consume(TEST_ETHERNET_LENGTH, |frame| {
            frame.fill(0xa5);
            frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        });
    let staged = access_point_tx
        .try_receive()
        .expect("AP publishes a CPU-owned packet");
    assert_eq!(staged.ethernet()[0], 0xa5);
    let staged = match access_point_tx.try_promote(staged) {
        Ok(_) => panic!("promotion must respect exhausted physical DMA credits"),
        Err(staged) => staged,
    };
    assert_eq!(&staged.ethernet()[12..14], &0x0800_u16.to_be_bytes());

    // Once completion returns the DMA credit, promotion makes the only packet
    // copy and releases the PSRAM/staging credit immediately.
    drop(direct);
    let promoted = match access_point_tx.try_promote(staged) {
        Ok(promoted) => promoted,
        Err(_) => panic!("returned DMA credit must allow promotion"),
    };
    assert_eq!(promoted.ethernet().len(), TEST_ETHERNET_LENGTH);
    assert_eq!(promoted.ethernet()[0], 0xa5);
    assert_eq!(&promoted.ethernet()[12..14], &0x0800_u16.to_be_bytes());
    drop(promoted);

    assert!(station.transmit(&mut context()).is_some());
    assert!(access_point.transmit(&mut context()).is_some());
}

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-staging-copy-probe"))]
#[test]
fn staged_promotion_retains_the_exact_egress_identity() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;

    let endpoint_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let interface = NetworkInterfaceId::new(1);
    let destination = [2, 0, 0, 0, 0, 9];
    let (provider, consumer) = tx_resources.split(pool);
    let (device, radio_state) = endpoint_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(interface, [2, 0, 0, 0, 0, 2]),
    );
    let mut device = device.with_tx_staging_copy_selected(true);
    let radio = consumer.for_interface(interface);
    radio_state.set_link_state(LinkState::Up);
    let key = device.egress_key(EgressRoute {
        destination: HardwareAddress::Ethernet(destination),
        traffic_class: 5,
    });

    let token = match device.transmit_for(&mut context(), key) {
        EgressAdmission::Granted(token) => token,
        _ => panic!("current egress identity receives one staged credit"),
    };
    token.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0xa5));
    let staged = radio.try_receive().expect("staged owner reaches the radio");
    assert_eq!(radio.metadata(&staged).egress_key(), Some(key));

    let promoted = radio
        .try_promote(staged)
        .unwrap_or_else(|_| panic!("one free DMA slot admits the staged owner"));
    assert_eq!(promoted.tag().interface(), interface);
    assert_eq!(radio.direct_metadata(&promoted).egress_key(), Some(key));
}

#[cfg(feature = "tx-staging-copy-probe")]
#[test]
fn scheduled_staged_tx_promotes_a_complete_batch_in_order() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;

    let endpoint_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let interface = NetworkInterfaceId::new(1);
    let (provider, consumer) = tx_resources.split(pool);
    let (device, radio_state) = endpoint_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(interface, [2, 0, 0, 0, 0, 2]),
    );
    let mut device = device.with_tx_staging_copy_selected(true);
    let radio = consumer.for_interface(interface);
    radio_state.set_link_state(LinkState::Up);

    for marker in 1..=3_u8 {
        device
            .transmit(&mut context())
            .expect("the staged packet pool has one credit per frame")
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    let mut batch = [
        radio.try_receive(),
        radio.try_receive(),
        radio.try_receive(),
    ];
    assert!(radio.try_promote_batch(&mut batch));
    for (index, frame) in batch.into_iter().enumerate() {
        let frame = frame
            .expect("every batch entry remains occupied")
            .into_direct()
            .unwrap_or_else(|_| panic!("successful batch promotion leaves only DMA owners"));
        assert_eq!(frame.ethernet()[0], index as u8 + 1);
        drop(frame);
    }
    assert!(device.transmit(&mut context()).is_some());
}

#[cfg(feature = "tx-core1-materializer-probe")]
#[test]
fn selected_batch_materializes_in_the_network_driver_poll() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;

    let _guard = CORE1_MATERIALIZER_TEST_LOCK.lock().unwrap();
    configure_tx_core1_materializer_probe(true);
    let endpoint_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let interface = NetworkInterfaceId::new(1);
    let (provider, consumer) = tx_resources.split(pool);
    let (device, radio_state) = endpoint_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(interface, [2, 0, 0, 0, 0, 2]),
    );
    let mut device = device.with_tx_staging_copy_selected(true);
    let radio = consumer.for_interface(interface);
    radio_state.set_link_state(LinkState::Up);

    for marker in 1..=3_u8 {
        device
            .transmit(&mut context())
            .expect("the staged packet pool has one credit per frame")
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    let mut batch = [
        radio.try_receive(),
        radio.try_receive(),
        radio.try_receive(),
    ];
    assert!(radio.try_submit_core1_materialization(&mut batch));
    assert!(batch.iter().all(Option::is_none));
    assert_eq!(
        radio.poll_core1_materialization(&mut batch),
        PinnedTxCore1MaterializationPoll::Pending
    );

    // Driver::transmit is executed by the network runner on Core1 in the
    // production split topology. Its bounded service turn performs the copy
    // and publishes only one completion record for the complete batch.
    drop(device.transmit(&mut context()));
    assert_eq!(
        radio.poll_core1_materialization(&mut batch),
        PinnedTxCore1MaterializationPoll::Ready(3)
    );
    for (index, frame) in batch.into_iter().enumerate() {
        let frame = frame
            .expect("completion restores every selected owner")
            .into_direct()
            .unwrap_or_else(|_| panic!("Core1 completion contains only DMA owners"));
        assert_eq!(frame.ethernet()[0], index as u8 + 1);
        drop(frame);
    }
    configure_tx_core1_materializer_probe(false);
}

#[cfg(feature = "tx-core1-materializer-probe")]
#[test]
fn pending_core1_materialization_cancellation_returns_both_credit_classes() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;

    let _guard = CORE1_MATERIALIZER_TEST_LOCK.lock().unwrap();
    configure_tx_core1_materializer_probe(true);
    let endpoint_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let interface = NetworkInterfaceId::new(1);
    let (provider, consumer) = tx_resources.split(pool);
    let (device, radio_state) = endpoint_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(interface, [2, 0, 0, 0, 0, 2]),
    );
    let mut device = device.with_tx_staging_copy_selected(true);
    let radio = consumer.for_interface(interface);
    radio_state.set_link_state(LinkState::Up);

    for marker in 1..=3_u8 {
        device
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    let mut batch = [
        radio.try_receive(),
        radio.try_receive(),
        radio.try_receive(),
    ];
    assert!(radio.try_submit_core1_materialization(&mut batch));
    assert!(radio.cancel_core1_materialization());
    drop(device.transmit(&mut context()));
    assert_eq!(
        radio.poll_core1_materialization(&mut batch),
        PinnedTxCore1MaterializationPoll::Cancelled
    );

    // The cancelled request returned all three staged sources and all three
    // reserved physical destinations; neither pool depends on a later epoch.
    for marker in 4..=6_u8 {
        device
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    let mut replacement = [
        radio.try_receive(),
        radio.try_receive(),
        radio.try_receive(),
    ];
    assert!(radio.try_promote_batch(&mut replacement));
    drop(replacement);
    configure_tx_core1_materializer_probe(false);
}

#[cfg(feature = "tx-core1-materializer-probe")]
#[test]
fn published_core1_materialization_cancellation_reclaims_ready_dma_owners() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;

    let _guard = CORE1_MATERIALIZER_TEST_LOCK.lock().unwrap();
    configure_tx_core1_materializer_probe(true);
    let endpoint_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let interface = NetworkInterfaceId::new(1);
    let (provider, consumer) = tx_resources.split(pool);
    let (device, radio_state) = endpoint_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(interface, [2, 0, 0, 0, 0, 2]),
    );
    let mut device = device.with_tx_staging_copy_selected(true);
    let radio = consumer.for_interface(interface);
    radio_state.set_link_state(LinkState::Up);

    for marker in 1..=3_u8 {
        device
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    let mut batch = [
        radio.try_receive(),
        radio.try_receive(),
        radio.try_receive(),
    ];
    assert!(radio.try_submit_core1_materialization(&mut batch));
    drop(device.transmit(&mut context()));
    assert!(radio.cancel_core1_materialization());
    assert_eq!(
        radio.poll_core1_materialization(&mut batch),
        PinnedTxCore1MaterializationPoll::Pending
    );

    // Cancellation consumed the already-published completion and returned
    // its READY destinations, so another full physical batch can be promoted.
    for marker in 4..=6_u8 {
        device
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    let mut replacement = [
        radio.try_receive(),
        radio.try_receive(),
        radio.try_receive(),
    ];
    assert!(radio.try_promote_batch(&mut replacement));
    drop(replacement);
    configure_tx_core1_materializer_probe(false);
}

#[cfg(feature = "tx-core1-materializer-probe")]
#[test]
fn materializer_rearms_core1_wake_before_returning_a_staged_credit() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;

    let _guard = CORE1_MATERIALIZER_TEST_LOCK.lock().unwrap();
    configure_tx_core1_materializer_probe(true);
    let endpoint_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let interface = NetworkInterfaceId::new(1);
    let (provider, consumer) = tx_resources.split(pool);
    let (device, radio_state) = endpoint_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(interface, [2, 0, 0, 0, 0, 2]),
    );
    let mut device = device.with_tx_staging_copy_selected(true);
    let radio = consumer.for_interface(interface);
    radio_state.set_link_state(LinkState::Up);

    device
        .transmit(&mut context())
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(1));
    let mut first = [radio.try_receive()];
    assert!(radio.try_submit_core1_materialization(&mut first));

    let wake_count = Arc::new(AtomicUsize::new(0));
    let waker = Waker::from(Arc::new(CountWake(Arc::clone(&wake_count))));
    let mut counted_context = Context::from_waker(&waker);
    // This one poll both services the first copy and uses its returned staged
    // credit to publish the successor. No empty driver poll exists between
    // the two Core0 submissions.
    device
        .transmit(&mut counted_context)
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(2));
    assert_eq!(
        radio.poll_core1_materialization(&mut first),
        PinnedTxCore1MaterializationPoll::Ready(1)
    );
    drop(first);

    let mut second = [radio.try_receive()];
    let wakes_before = wake_count.load(Ordering::Relaxed);
    assert!(radio.try_submit_core1_materialization(&mut second));
    assert!(
        wake_count.load(Ordering::Relaxed) > wakes_before,
        "the successor request must wake the Core1 driver without a timer assist"
    );
    assert!(radio.cancel_core1_materialization());
    drop(device.transmit(&mut counted_context));
    configure_tx_core1_materializer_probe(false);
}

#[cfg(feature = "tx-staging-copy-probe")]
#[test]
fn scheduled_staged_tx_batch_reservation_is_all_or_nothing() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let station_interface = NetworkInterfaceId::new(0);
    let access_point_interface = NetworkInterfaceId::new(1);
    let (provider, consumer) = tx_resources.split(pool);
    let (mut station, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(station_interface, [2, 0, 0, 0, 0, 1]),
    );
    let (access_point, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(access_point_interface, [2, 0, 0, 0, 0, 2]),
    );
    let mut access_point = access_point.with_tx_staging_copy_selected(true);
    let station_tx = consumer.for_interface(station_interface);
    let access_point_tx = consumer.for_interface(access_point_interface);
    station_rx.set_link_state(LinkState::Up);
    access_point_rx.set_link_state(LinkState::Up);

    station
        .transmit(&mut context())
        .expect("the first physical credit is free")
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0xd1));
    let occupied = station_tx
        .try_receive_direct()
        .expect("STA retains one of two physical credits");
    assert_eq!(access_point_tx.promotion_capacity(), 1);
    for marker in 1..=2_u8 {
        access_point
            .transmit(&mut context())
            .expect("AP staging is independent of physical credits")
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    let mut batch = [access_point_tx.try_receive(), access_point_tx.try_receive()];

    assert!(!access_point_tx.try_promote_batch(&mut batch));
    for (index, frame) in batch.iter().enumerate() {
        assert_eq!(frame.as_ref().unwrap().ethernet()[0], index as u8 + 1);
        assert!(matches!(
            frame.as_ref().unwrap(),
            PinnedNetworkTxFrame::Staged(_)
        ));
    }

    drop(occupied);
    assert_eq!(access_point_tx.promotion_capacity(), 2);
    assert!(access_point_tx.try_promote_batch(&mut batch));
    for frame in batch.into_iter().flatten() {
        drop(
            frame
                .into_direct()
                .unwrap_or_else(|_| panic!("retry promotes every retained source")),
        );
    }
}

#[test]
fn pinned_tx_batch_wait_does_not_claim_a_partial_prefix() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 3>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 3>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(
        resources,
        pool,
        NetworkInterfaceId::new(0),
        [2, 0, 0, 0, 0, 1]
    );
    radio.set_link_state(LinkState::Up);
    let tx = radio.tx_consumer();
    let mut batch = pin!(tx.wait_queue_len_at_least(3));

    assert!(batch.as_mut().poll(&mut context()).is_pending());
    for marker in 1..=2_u8 {
        device
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
        assert!(batch.as_mut().poll(&mut context()).is_pending());
        assert_eq!(tx.queue_len(), usize::from(marker));
    }
    device
        .transmit(&mut context())
        .unwrap()
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(3));
    assert!(batch.as_mut().poll(&mut context()).is_ready());
    assert_eq!(tx.queue_len(), 3);

    for marker in 1..=3_u8 {
        let frame = tx.try_receive().unwrap();
        assert_eq!(frame.ethernet()[0], marker);
    }
}

#[test]
fn permanent_endpoints_keep_distinct_addresses_and_share_one_tx_fabric() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let station = [2, 0, 0, 0, 0, 1];
    let access_point = [2, 0, 0, 0, 0, 2];
    let station_interface = NetworkInterfaceId::new(3);
    let access_point_interface = NetworkInterfaceId::new(7);
    let (provider, consumer) = tx_resources.split(pool);
    let (mut station_device, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(station_interface, station),
    );
    let (mut access_point_device, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(access_point_interface, access_point),
    );
    let radio = DualPinnedNetworkRunner::new(
        station_interface,
        station_rx,
        access_point_interface,
        access_point_rx,
        consumer,
    );
    radio.set_link_state(station_interface, LinkState::Up);

    // A permanent but inactive peer must not halve the sole running role's
    // batching frontier. The station may use the complete physical pool.
    for marker in 0..4_u8 {
        station_device
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    assert!(station_device.transmit(&mut context()).is_none());
    {
        let mut publication = pin!(radio.wait_tx_publication());
        assert!(matches!(
            publication.as_mut().poll(&mut context()),
            Poll::Ready(())
        ));
    }
    let station_tx = radio.tx_consumer().for_interface(station_interface);
    for marker in 0..4_u8 {
        let frame = station_tx.try_receive_direct().unwrap();
        assert_eq!(frame.ethernet()[0], marker);
        drop(frame);
    }
    {
        let mut false_publication = pin!(radio.wait_tx_publication());
        assert!(matches!(
            false_publication.as_mut().poll(&mut context()),
            Poll::Pending
        ));
    }

    // Once both roles are active, a saturated endpoint may still use the
    // complete physical pool. Fairness is applied at the real contention
    // edge rather than by permanently halving standalone capacity.
    radio.set_link_state(access_point_interface, LinkState::Up);

    assert_eq!(
        station_device.hardware_address(),
        HardwareAddress::Ethernet(station)
    );
    assert_eq!(
        access_point_device.hardware_address(),
        HardwareAddress::Ethernet(access_point)
    );

    for marker in 0x51..=0x54_u8 {
        station_device
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }

    let station_wakes = Arc::new(AtomicUsize::new(0));
    let access_point_wakes = Arc::new(AtomicUsize::new(0));
    let station_waker = Waker::from(Arc::new(CountWake(station_wakes.clone())));
    let access_point_waker = Waker::from(Arc::new(CountWake(access_point_wakes.clone())));
    assert!(
        station_device
            .transmit(&mut Context::from_waker(&station_waker))
            .is_none()
    );
    assert!(
        access_point_device
            .transmit(&mut Context::from_waker(&access_point_waker))
            .is_none()
    );

    let consumer = radio.tx_consumer();
    let station_tx = consumer.for_interface(station_interface);
    let access_point_tx = consumer.for_interface(access_point_interface);
    let station_first = station_tx.try_receive_direct().unwrap();
    assert_eq!(station_first.tag().interface(), station_interface);
    #[cfg(feature = "tx-egress-scheduling")]
    assert_eq!(
        station_tx.direct_metadata(&station_first).egress_key(),
        None
    );
    assert_eq!(station_first.ethernet()[0], 0x51);
    drop(station_first);
    assert_eq!(station_wakes.load(Ordering::Relaxed), 0);
    assert_eq!(access_point_wakes.load(Ordering::Relaxed), 1);

    access_point_device
        .transmit(&mut Context::from_waker(&access_point_waker))
        .expect("the waiting peer claims the returned credit")
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0xa1));
    let access_point_first = access_point_tx.try_receive_direct().unwrap();
    assert_eq!(access_point_first.tag().interface(), access_point_interface);
    #[cfg(feature = "tx-egress-scheduling")]
    assert_eq!(
        access_point_tx
            .direct_metadata(&access_point_first)
            .egress_key(),
        None
    );
    assert_eq!(access_point_first.ethernet()[0], 0xa1);
    drop(access_point_first);

    // Each VIF owns a FIFO. Granting the peer a returned credit must not
    // rotate the already-published station frames.
    for marker in 0x52..=0x54_u8 {
        let frame = station_tx.try_receive_direct().unwrap();
        assert_eq!(frame.ethernet()[0], marker);
        drop(frame);
    }
    assert_eq!(station_tx.queue_len(), 0);
    assert_eq!(access_point_tx.queue_len(), 0);
}

#[test]
fn link_down_reclaims_unclaimed_vif_backlog_from_the_shared_pool() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let station_interface = NetworkInterfaceId::new(0);
    let access_point_interface = NetworkInterfaceId::new(1);
    let (provider, consumer) = tx_resources.split(pool);
    let (mut station, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(station_interface, [2, 0, 0, 0, 0, 1]),
    );
    let (mut access_point, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(access_point_interface, [2, 0, 0, 0, 0, 2]),
    );
    let radio = DualPinnedNetworkRunner::new(
        station_interface,
        station_rx,
        access_point_interface,
        access_point_rx,
        consumer,
    );
    radio.set_link_state(station_interface, LinkState::Up);
    radio.set_link_state(access_point_interface, LinkState::Up);

    for marker in 0..2_u8 {
        station
            .transmit(&mut context())
            .unwrap()
            .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(marker));
    }
    assert_eq!(radio.tx_consumer().queue_len_for(station_interface), 2);

    radio.set_link_state(station_interface, LinkState::Down);
    assert_eq!(radio.tx_consumer().queue_len_for(station_interface), 0);

    // Cover the opposite cross-core ordering: link-down drains first, then
    // the already-issued synchronous token completes its publication.
    radio.set_link_state(station_interface, LinkState::Up);
    let late = station
        .transmit(&mut context())
        .expect("one station token is issued before link-down");
    radio.set_link_state(station_interface, LinkState::Down);
    late.consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0xee));
    assert_eq!(radio.tx_consumer().queue_len_for(station_interface), 0);

    // A stale network poll may also acquire its token after the link already
    // went down. Publication-time state is authoritative: such a frame must
    // return its physical credit instead of becoming hidden VIF backlog.
    station
        .transmit(&mut context())
        .expect("a stale poll can still own one globally free credit")
        .consume(TEST_ETHERNET_LENGTH, |frame| frame.fill(0xef));
    assert_eq!(radio.tx_consumer().queue_len_for(station_interface), 0);

    for _ in 0..4 {
        access_point
            .transmit(&mut context())
            .expect("stopped STA backlog no longer owns a physical credit")
            .consume(TEST_ETHERNET_LENGTH, |_| ());
    }
    assert!(access_point.transmit(&mut context()).is_none());
}

#[test]
fn returned_shared_tx_credit_wakes_one_waiting_peer() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (provider, consumer) = tx_resources.split(pool);
    let (mut station, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1]),
    );
    let (mut access_point, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(NetworkInterfaceId::new(1), [2, 0, 0, 0, 0, 2]),
    );
    station_rx.set_link_state(LinkState::Up);
    access_point_rx.set_link_state(LinkState::Up);
    station
        .transmit(&mut context())
        .expect("the sole physical credit starts free")
        .consume(TEST_ETHERNET_LENGTH, |_| ());

    let station_wakes = Arc::new(AtomicUsize::new(0));
    let access_point_wakes = Arc::new(AtomicUsize::new(0));
    let station_waker = Waker::from(Arc::new(CountWake(station_wakes.clone())));
    let access_point_waker = Waker::from(Arc::new(CountWake(access_point_wakes.clone())));
    let mut station_context = Context::from_waker(&station_waker);
    let mut access_point_context = Context::from_waker(&access_point_waker);

    assert!(station.transmit(&mut station_context).is_none());
    assert!(access_point.transmit(&mut access_point_context).is_none());
    assert_eq!(station_wakes.load(Ordering::Relaxed), 0);
    assert_eq!(access_point_wakes.load(Ordering::Relaxed), 0);

    drop(
        consumer
            .try_receive()
            .expect("radio owns the published slot"),
    );
    assert_eq!(station_wakes.load(Ordering::Relaxed), 0);
    assert_eq!(access_point_wakes.load(Ordering::Relaxed), 1);
}

#[test]
fn aggregate_credit_return_publishes_one_readiness_edge() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (provider, consumer) = tx_resources.split(pool);
    let station_interface = NetworkInterfaceId::new(0);
    let access_point_interface = NetworkInterfaceId::new(1);
    let (mut station, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(station_interface, [2, 0, 0, 0, 0, 1]),
    );
    let (mut access_point, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(access_point_interface, [2, 0, 0, 0, 0, 2]),
    );
    station_rx.set_link_state(LinkState::Up);
    access_point_rx.set_link_state(LinkState::Up);
    for _ in 0..4 {
        station
            .transmit(&mut context())
            .expect("each physical credit is initially free")
            .consume(TEST_ETHERNET_LENGTH, |_| ());
    }
    let held = core::array::from_fn::<_, 4, _>(|_| {
        consumer
            .try_receive()
            .expect("radio claims the complete aggregate batch")
    });

    let station_wakes = Arc::new(AtomicUsize::new(0));
    let access_point_wakes = Arc::new(AtomicUsize::new(0));
    let station_waker = Waker::from(Arc::new(CountWake(station_wakes.clone())));
    let access_point_waker = Waker::from(Arc::new(CountWake(access_point_wakes.clone())));
    assert!(
        station
            .transmit(&mut Context::from_waker(&station_waker))
            .is_none()
    );
    assert!(
        access_point
            .transmit(&mut Context::from_waker(&access_point_waker))
            .is_none()
    );

    drop(held);
    assert_eq!(station_wakes.load(Ordering::Relaxed), 0);
    assert_eq!(access_point_wakes.load(Ordering::Relaxed), 1);
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
    radio.set_link_state(LinkState::Up);
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
fn every_permanent_endpoint_keeps_its_own_ingress_credit() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    // Two application credits plus one ingress credit for each endpoint.
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (provider, _consumer) = tx_resources.split(pool);
    let (station, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1]),
    );
    let (access_point, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(NetworkInterfaceId::new(1), [2, 0, 0, 0, 0, 2]),
    );
    let mut station = station.with_ingress_tx_reserve();
    let mut access_point = access_point.with_ingress_tx_reserve();
    station_rx.set_link_state(LinkState::Up);
    access_point_rx.set_link_state(LinkState::Up);

    station
        .transmit(&mut context())
        .expect("first application credit")
        .consume(ETHERNET_HEADER_LEN, |_| ());
    access_point
        .transmit(&mut context())
        .expect("second application credit")
        .consume(ETHERNET_HEADER_LEN, |_| ());
    assert!(station.transmit(&mut context()).is_none());
    assert!(access_point.transmit(&mut context()).is_none());

    station_rx
        .try_send_rx(&[0x51; ETHERNET_HEADER_LEN])
        .unwrap();
    access_point_rx
        .try_send_rx(&[0xa7; ETHERNET_HEADER_LEN])
        .unwrap();
    let (station_frame, station_reply) = station
        .receive(&mut context())
        .expect("STA keeps a paired ingress TX credit");
    let (access_point_frame, access_point_reply) = access_point
        .receive(&mut context())
        .expect("AP keeps a distinct paired ingress TX credit");
    station_frame.consume(|frame| assert_eq!(frame[0], 0x51));
    access_point_frame.consume(|frame| assert_eq!(frame[0], 0xa7));
    drop(station_reply);
    drop(access_point_reply);
}

#[cfg(feature = "tx-phase-telemetry")]
#[test]
fn direct_pool_snapshot_accounts_reserves_tokens_ready_and_radio_owners() {
    type EndpointResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    let station_resources = Box::leak(Box::new(EndpointResources::new()));
    let access_point_resources = Box::leak(Box::new(EndpointResources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (provider, consumer) = tx_resources.split(pool);
    let (station, station_rx) = station_resources.split(
        provider,
        NetworkEndpointConfig::single_radio_peer(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1]),
    );
    let (access_point, access_point_rx) = access_point_resources.split(
        provider,
        NetworkEndpointConfig::per_link_destination(NetworkInterfaceId::new(1), [2, 0, 0, 0, 0, 2]),
    );
    station_rx.set_link_state(LinkState::Up);
    access_point_rx.set_link_state(LinkState::Up);
    let mut station = station.with_ingress_tx_reserve();
    let _access_point = access_point.with_ingress_tx_reserve();

    let token = station
        .transmit(&mut context())
        .expect("one application token remains available");
    assert_eq!(
        consumer.ownership_snapshot_for(NetworkInterfaceId::new(0)),
        open_esp_radio_embassy_net::PinnedTxOwnershipSnapshot {
            free: 1,
            control_free: 0,
            ready_for_interface: 0,
            ready_for_other_interfaces: 0,
            ingress_reserved: 2,
            application_reserved: 0,
            control_reserved: 0,
            tokens_in_flight: 1,
        }
    );

    token.consume(ETHERNET_HEADER_LEN, |_| ());
    let published = consumer.ownership_snapshot_for(NetworkInterfaceId::new(0));
    assert_eq!(published.free, 1);
    assert_eq!(published.ready_for_interface, 1);
    assert_eq!(published.ingress_reserved, 2);
    assert_eq!(published.tokens_in_flight, 0);

    let radio_owned = consumer
        .try_receive_for(NetworkInterfaceId::new(0))
        .expect("radio claims the published frame");
    let claimed = consumer.ownership_snapshot_for(NetworkInterfaceId::new(0));
    assert_eq!(claimed.ready_for_interface, 0);
    assert_eq!(claimed.radio_owned(4), 1);
    drop(radio_owned);
    assert_eq!(
        consumer
            .ownership_snapshot_for(NetworkInterfaceId::new(0))
            .free,
        2
    );
}

#[test]
fn pinned_rx_and_tx_depths_are_independent() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 3>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (mut device, radio) = split_pinned!(resources, pool, NetworkInterfaceId::new(0), [0; 6]);
    radio.set_link_state(LinkState::Up);
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
    assert_eq!(consumer.try_receive_direct().unwrap().ethernet()[0], 0xa5);

    for marker in 0..3 {
        let (received, reply) = device.receive(&mut context()).unwrap();
        received.consume(|frame| assert_eq!(frame[0], marker));
        drop(reply);
    }
    assert_eq!(publisher.free_capacity(), 3);
}

#[test]
fn pinned_device_reports_explicit_checksum_capabilities() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 1>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let (device, _radio) = split_pinned!(
        resources,
        pool,
        NetworkInterfaceId::new(0),
        [2, 0, 0, 0, 0, 1]
    );
    let mut requested = ChecksumCapabilities::default();
    requested.ipv4 = Checksum::Tx;
    requested.udp = Checksum::Tx;
    let device = device.with_checksum_capabilities(requested);

    let checksum = device.capabilities().checksum;
    assert!(matches!(checksum.ipv4, Checksum::Tx));
    assert!(matches!(checksum.udp, Checksum::Tx));
    assert!(matches!(checksum.tcp, Checksum::Both));
}

#[test]
fn shared_device_can_omit_only_the_ipv4_udp_tx_checksum() {
    type TestResources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 1>;
    type TestPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    let resources = Box::leak(Box::new(TestResources::new()));
    let tx_pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let shared_pool = Box::leak(Box::new(RxHandoffPool::<FRAME_CAPACITY, 1>::new()));
    let shared_queue = Box::leak(Box::new(SharedPinnedRxQueue::<NoopRawMutex, 1>::new()));
    let (_publisher, consumer) = shared_queue.split(shared_pool, observe_shared_rx_release);
    let (device, _radio) = split_pinned!(
        resources,
        tx_pool,
        NetworkInterfaceId::new(0),
        [2, 0, 0, 0, 0, 1]
    );
    let device = device
        .with_ingress_tx_reserve()
        .with_shared_rx(consumer)
        .with_software_ipv4_udp_tx_checksum_generation(false);

    let checksum = device.capabilities().checksum;
    assert!(matches!(checksum.ipv4, Checksum::Both));
    assert!(matches!(checksum.udp, Checksum::Rx));
    assert!(matches!(checksum.tcp, Checksum::Both));
}
