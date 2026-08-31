use core::{
    future::{Future, poll_fn},
    pin::pin,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};

use embassy_net::{
    Config, Ipv4Address, Ipv4Cidr, StackResources, StaticConfigV4,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_embassy_net::{
    NetworkEndpointConfig, NetworkInterfaceId, PinnedEndpointResources, PinnedNetworkRunner,
    PinnedTxPool, PinnedTxResources,
};

const FRAME_CAPACITY: usize = 1_536;
const TX_HEADROOM: usize = 28;
const TX_TRAILER: usize = 8;

#[test]
fn permanent_access_point_endpoint_delivers_arp() {
    let done = Box::leak(Box::new(AtomicBool::new(false)));
    let executor = Box::leak(Box::new(embassy_executor::Executor::new()));
    executor.run_until(
        |spawner| {
            spawner.spawn(run_access_point_arp_test(done).expect("test task allocates once"));
        },
        || done.load(Ordering::Acquire),
    );
}

#[embassy_executor::task]
async fn run_access_point_arp_test(done: &'static AtomicBool) {
    type Resources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 2>;
    type TxPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let resources = Box::leak(Box::new(Resources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let tx_pool = TxPool::pin_static(Box::leak(Box::new(TxPool::new())));
    let access_point = [0x32, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let client = [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0];
    let (provider, consumer) = tx_resources.split(tx_pool);
    let endpoint =
        NetworkEndpointConfig::per_link_destination(NetworkInterfaceId::new(1), access_point);
    let (device, rx) = resources.split(provider, endpoint);
    let radio = PinnedNetworkRunner::new(NetworkInterfaceId::new(1), rx, consumer);
    let stack_resources = Box::leak(Box::new(StackResources::<1>::new()));
    let (stack, mut runner) = embassy_net::new(
        device,
        Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(10, 43, 0, 1), 24),
            gateway: None,
            dns_servers: Default::default(),
        }),
        stack_resources,
        1,
    );

    radio.set_link_state(embassy_net_driver::LinkState::Up);
    radio
        .try_send_rx(&arp_request(client, [10, 43, 0, 2]))
        .unwrap();

    let mut run = pin!(runner.run());
    let reply = poll_fn(|context| {
        assert!(run.as_mut().poll(context).is_pending());
        match radio.try_receive_tx() {
            Some(reply) => Poll::Ready(reply),
            None => {
                context.waker().wake_by_ref();
                Poll::Pending
            }
        }
    })
    .await;
    let bytes = reply.ethernet();
    assert_eq!(&bytes[..6], &client);
    assert_eq!(&bytes[6..12], &access_point);
    assert_eq!(&bytes[12..14], &0x0806_u16.to_be_bytes());
    assert_eq!(&bytes[20..22], &2_u16.to_be_bytes());
    assert_eq!(&bytes[22..28], &access_point);
    assert_eq!(&bytes[28..32], &[10, 43, 0, 1]);
    assert_eq!(
        stack.config_v4().unwrap().address.address(),
        Ipv4Address::new(10, 43, 0, 1)
    );
    done.store(true, Ordering::Release);
}

#[test]
fn permanent_access_point_retains_the_full_softap_neighbor_set() {
    let done = Box::leak(Box::new(AtomicBool::new(false)));
    let executor = Box::leak(Box::new(embassy_executor::Executor::new()));
    executor.run_until(
        |spawner| {
            spawner
                .spawn(run_full_softap_neighbor_set_test(done).expect("test task allocates once"));
        },
        || done.load(Ordering::Acquire),
    );
}

#[cfg(feature = "tx-egress-scheduling")]
#[test]
fn keyed_egress_schedule_is_contiguous_across_udp_sockets() {
    let done = Box::leak(Box::new(AtomicBool::new(false)));
    let executor = Box::leak(Box::new(embassy_executor::Executor::new()));
    executor.run_until(
        |spawner| {
            spawner.spawn(
                run_cross_socket_resolved_burst_test(done).expect("test task allocates once"),
            );
        },
        || done.load(Ordering::Acquire),
    );
}

#[cfg(feature = "tx-egress-scheduling")]
#[embassy_executor::task]
async fn run_cross_socket_resolved_burst_test(done: &'static AtomicBool) {
    const PACKETS_PER_PEER: usize = 4;

    type Resources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 2>;
    type TxPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 4>;

    let resources = Box::leak(Box::new(Resources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let tx_pool = TxPool::pin_static(Box::leak(Box::new(TxPool::new())));
    let access_point = [0x32, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let clients = [
        ([0x02, 0, 0, 0, 0, 2], [10, 43, 0, 2]),
        ([0x02, 0, 0, 0, 0, 3], [10, 43, 0, 3]),
    ];
    let (provider, consumer) = tx_resources.split(tx_pool);
    let endpoint =
        NetworkEndpointConfig::per_link_destination(NetworkInterfaceId::new(1), access_point);
    let (device, rx) = resources.split(provider, endpoint);
    let radio = PinnedNetworkRunner::new(NetworkInterfaceId::new(1), rx, consumer);
    let stack_resources = Box::leak(Box::new(StackResources::<2>::new()));
    let (stack, mut runner) = embassy_net::new(
        device,
        Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(10, 43, 0, 1), 24),
            gateway: None,
            dns_servers: Default::default(),
        }),
        stack_resources,
        3,
    );
    radio.set_link_state(embassy_net_driver::LinkState::Up);
    let mut run = pin!(runner.run());

    for (hardware, address) in clients {
        radio.try_send_rx(&arp_request(hardware, address)).unwrap();
        let reply = poll_fn(|context| {
            assert!(run.as_mut().poll(context).is_pending());
            match radio.try_receive_tx() {
                Some(reply) => Poll::Ready(reply),
                None => {
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        })
        .await;
        assert_eq!(&reply.ethernet()[..6], &hardware);
    }

    let a_rx_metadata = Box::leak(Box::new([PacketMetadata::EMPTY; 1]));
    let a_rx_payload = Box::leak(Box::new([0_u8; 1]));
    let a_tx_metadata = Box::leak(Box::new([PacketMetadata::EMPTY; PACKETS_PER_PEER]));
    let a_tx_payload = Box::leak(Box::new([0_u8; PACKETS_PER_PEER]));
    let b_rx_metadata = Box::leak(Box::new([PacketMetadata::EMPTY; 1]));
    let b_rx_payload = Box::leak(Box::new([0_u8; 1]));
    let b_tx_metadata = Box::leak(Box::new([PacketMetadata::EMPTY; PACKETS_PER_PEER]));
    let b_tx_payload = Box::leak(Box::new([0_u8; PACKETS_PER_PEER]));
    let mut socket_a = UdpSocket::new(
        stack,
        a_rx_metadata,
        a_rx_payload,
        a_tx_metadata,
        a_tx_payload,
    );
    let mut socket_b = UdpSocket::new(
        stack,
        b_rx_metadata,
        b_rx_payload,
        b_tx_metadata,
        b_tx_payload,
    );
    socket_a.bind(8_000).unwrap();
    socket_b.bind(8_001).unwrap();
    for sequence in 0..PACKETS_PER_PEER as u8 {
        socket_a
            .try_send_to(&[0xa0 | sequence], (Ipv4Address::new(10, 43, 0, 2), 9_000))
            .unwrap();
        socket_b
            .try_send_to(&[0xb0 | sequence], (Ipv4Address::new(10, 43, 0, 3), 9_001))
            .unwrap();
    }

    let mut observed = Vec::new();
    while observed.len() < PACKETS_PER_PEER * clients.len() {
        let frame = poll_fn(|context| {
            assert!(run.as_mut().poll(context).is_pending());
            match radio.try_receive_tx() {
                Some(frame) => Poll::Ready(frame),
                None => {
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        })
        .await;
        let ethernet = frame.ethernet();
        assert_eq!(&ethernet[12..14], &0x0800_u16.to_be_bytes());
        observed.push((ethernet[..6].to_vec(), ethernet[42]));
    }

    for (sequence, (destination, payload)) in observed[..PACKETS_PER_PEER].iter().enumerate() {
        assert_eq!(destination.as_slice(), &clients[0].0);
        assert_eq!(*payload, 0xa0 | sequence as u8);
    }
    for (sequence, (destination, payload)) in observed[PACKETS_PER_PEER..].iter().enumerate() {
        assert_eq!(destination.as_slice(), &clients[1].0);
        assert_eq!(*payload, 0xb0 | sequence as u8);
    }

    // No stack poll is allowed to observe Down here. The next Up carries a
    // new driver epoch and must still discard both the interface-wide and
    // socket-local phase left at peer B by the preceding lifecycle.
    radio.set_link_state(embassy_net_driver::LinkState::Down);
    radio.set_link_state(embassy_net_driver::LinkState::Up);
    for sequence in 0..PACKETS_PER_PEER as u8 {
        socket_a
            .try_send_to(&[0xc0 | sequence], (Ipv4Address::new(10, 43, 0, 2), 9_000))
            .unwrap();
        socket_b
            .try_send_to(&[0xd0 | sequence], (Ipv4Address::new(10, 43, 0, 3), 9_001))
            .unwrap();
    }

    let mut restarted = Vec::new();
    while restarted.len() < PACKETS_PER_PEER * clients.len() {
        let frame = poll_fn(|context| {
            assert!(run.as_mut().poll(context).is_pending());
            match radio.try_receive_tx() {
                Some(frame) => Poll::Ready(frame),
                None => {
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        })
        .await;
        let ethernet = frame.ethernet();
        restarted.push((ethernet[..6].to_vec(), ethernet[42]));
    }
    for (sequence, (destination, payload)) in restarted[..PACKETS_PER_PEER].iter().enumerate() {
        assert_eq!(destination.as_slice(), &clients[0].0);
        assert_eq!(*payload, 0xc0 | sequence as u8);
    }
    for (sequence, (destination, payload)) in restarted[PACKETS_PER_PEER..].iter().enumerate() {
        assert_eq!(destination.as_slice(), &clients[1].0);
        assert_eq!(*payload, 0xd0 | sequence as u8);
    }

    done.store(true, Ordering::Release);
}

#[embassy_executor::task]
async fn run_full_softap_neighbor_set_test(done: &'static AtomicBool) {
    const CLIENTS: usize = 15;

    type Resources = PinnedEndpointResources<NoopRawMutex, FRAME_CAPACITY, 2>;
    type TxPool = PinnedTxPool<FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;
    type TxResources = PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, TX_HEADROOM, TX_TRAILER, 2>;

    let resources = Box::leak(Box::new(Resources::new()));
    let tx_resources = Box::leak(Box::new(TxResources::new()));
    let tx_pool = TxPool::pin_static(Box::leak(Box::new(TxPool::new())));
    let access_point = [0x32, 0xed, 0xa0, 0xf3, 0xf6, 0xd0];
    let (provider, consumer) = tx_resources.split(tx_pool);
    let endpoint =
        NetworkEndpointConfig::per_link_destination(NetworkInterfaceId::new(1), access_point);
    let (device, rx) = resources.split(provider, endpoint);
    let radio = PinnedNetworkRunner::new(NetworkInterfaceId::new(1), rx, consumer);
    let stack_resources = Box::leak(Box::new(StackResources::<1>::new()));
    let (stack, mut runner) = embassy_net::new(
        device,
        Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(10, 43, 0, 1), 24),
            gateway: None,
            dns_servers: Default::default(),
        }),
        stack_resources,
        2,
    );
    radio.set_link_state(embassy_net_driver::LinkState::Up);
    let mut run = pin!(runner.run());

    for peer in 0..CLIENTS as u8 {
        let address = [10, 43, 0, peer + 2];
        let hardware = [0x02, 0, 0, 0, 0, peer + 2];
        radio.try_send_rx(&arp_request(hardware, address)).unwrap();
        let reply = poll_fn(|context| {
            assert!(run.as_mut().poll(context).is_pending());
            match radio.try_receive_tx() {
                Some(reply) => Poll::Ready(reply),
                None => {
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        })
        .await;
        assert_eq!(&reply.ethernet()[..6], &hardware);
        assert_eq!(&reply.ethernet()[12..14], &0x0806_u16.to_be_bytes());
    }

    let rx_metadata = Box::leak(Box::new([PacketMetadata::EMPTY; 1]));
    let rx_payload = Box::leak(Box::new([0_u8; 16]));
    let tx_metadata = Box::leak(Box::new([PacketMetadata::EMPTY; 1]));
    let tx_payload = Box::leak(Box::new([0_u8; 16]));
    let mut socket = UdpSocket::new(stack, rx_metadata, rx_payload, tx_metadata, tx_payload);
    socket.bind(8_000).unwrap();

    // Revisit every learned client after the cache contains the complete set.
    // An eight-entry cache would emit ARP for the early peers here instead of
    // the expected IPv4 datagram.
    for peer in 0..CLIENTS as u8 {
        let address = Ipv4Address::new(10, 43, 0, peer + 2);
        let hardware = [0x02, 0, 0, 0, 0, peer + 2];
        socket.try_send_to(&[peer], (address, 9_000)).unwrap();
        let frame = poll_fn(|context| {
            assert!(run.as_mut().poll(context).is_pending());
            match radio.try_receive_tx() {
                Some(frame) => Poll::Ready(frame),
                None => {
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        })
        .await;
        assert_eq!(&frame.ethernet()[..6], &hardware);
        assert_eq!(&frame.ethernet()[12..14], &0x0800_u16.to_be_bytes());
    }

    done.store(true, Ordering::Release);
}

fn arp_request(client: [u8; 6], client_address: [u8; 4]) -> [u8; 42] {
    let mut frame = [0_u8; 42];
    frame[..6].fill(0xff);
    frame[6..12].copy_from_slice(&client);
    frame[12..14].copy_from_slice(&0x0806_u16.to_be_bytes());
    frame[14..16].copy_from_slice(&1_u16.to_be_bytes());
    frame[16..18].copy_from_slice(&0x0800_u16.to_be_bytes());
    frame[18] = 6;
    frame[19] = 4;
    frame[20..22].copy_from_slice(&1_u16.to_be_bytes());
    frame[22..28].copy_from_slice(&client);
    frame[28..32].copy_from_slice(&client_address);
    frame[38..42].copy_from_slice(&[10, 43, 0, 1]);
    frame
}
