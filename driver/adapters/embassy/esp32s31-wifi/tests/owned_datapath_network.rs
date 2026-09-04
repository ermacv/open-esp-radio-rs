use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_embassy_net::{
    LinkState, NetworkInterfaceId, OwnedEndpointResources, PinnedTxPool, PinnedTxResources,
};
use open_esp_radio_esp32s31_wifi_embassy::datapath::network::{
    DatapathNetwork, OwnedDatapathNetwork,
};
use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPool, PacketPoolStorage};

fn allocator<const N: usize>() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

fn packet(allocator: PacketBufAllocator, marker: u8) -> PacketBuf {
    let mut packet = allocator.try_alloc().expect("test packet credit");
    packet.set_len(14);
    packet.fill(marker);
    packet
}

#[test]
fn owned_backlog_and_physical_dma_credits_remain_independent() {
    const FRAME_CAPACITY: usize = 64;
    const HEADROOM: usize = 16;
    const TRAILER: usize = 8;
    const RX_QUEUE_DEPTH: usize = 1;
    const NETWORK_TX_DEPTH: usize = 2;
    const TX_QUEUE_DEPTH: usize = 1;
    type PhysicalPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type PhysicalResources =
        PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;

    let general = allocator::<NETWORK_TX_DEPTH>();
    let rx = allocator::<RX_QUEUE_DEPTH>();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<
        NoopRawMutex,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
    >::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], rx);

    let physical_resources = Box::leak(Box::new(PhysicalResources::new()));
    let physical_pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
    let (_unused_direct_provider, physical) = physical_resources.split(physical_pool);
    let network = OwnedDatapathNetwork::new(owned, physical);
    network.set_link_state(interface, LinkState::Up);

    device.transmit(packet(general, 0x41)).unwrap();
    device.transmit(packet(general, 0x42)).unwrap();
    assert_eq!(network.tx_queue_len(interface), NETWORK_TX_DEPTH);

    let first = network.try_receive_tx(interface).expect("first owned TX");
    let physical = network.tx_consumer(interface);
    let first = physical
        .try_promote(first)
        .unwrap_or_else(|_| panic!("one physical SRAM credit is initially free"));

    let second = network.try_receive_tx(interface).expect("second owned TX");
    let second = match physical.try_promote(second) {
        Ok(_) => panic!("the retained first frame owns the only SRAM credit"),
        Err(second) => second,
    };
    assert_eq!(second.as_slice(), &[0x42; 14]);
    let returned_first = general
        .try_alloc()
        .expect("successful promotion released only its source owner");
    assert!(general.try_alloc().is_none());

    drop(first);
    let second = physical
        .try_promote(second)
        .unwrap_or_else(|_| panic!("completion returns the physical credit"));
    assert_eq!(second.as_slice(), &[0x42; 14]);
    assert!(general.try_alloc().is_some());
    drop(returned_first);
}
