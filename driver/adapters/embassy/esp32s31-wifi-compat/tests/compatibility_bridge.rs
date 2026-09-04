use core::task::{Context, Waker};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_embassy_net_compat::{
    Driver as _, ETHERNET_HEADER_LEN, Resources, RxToken as _, TxToken as _,
};
use open_esp_radio_esp32s31_wifi_embassy::datapath::{
    PinnedTxPool, PinnedTxResources, SelectedBurstMaterializer,
    network::{DatapathNetwork, DatapathNetworkRx},
};
use open_esp_radio_esp32s31_wifi_embassy_compat::{
    CompatibilityDatapathNetwork, DualCompatibilityDatapathNetwork,
};
use open_esp_radio_network::{LinkState, NetworkInterfaceId};

const FRAME_CAPACITY: usize = 64;
const HEADROOM: usize = 16;
const TRAILER: usize = 8;
const NETWORK_QUEUE_DEPTH: usize = 2;
const PHYSICAL_QUEUE_DEPTH: usize = 1;

type Endpoint = Resources<NoopRawMutex, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>;
type PhysicalPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, PHYSICAL_QUEUE_DEPTH>;
type PhysicalResources =
    PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, PHYSICAL_QUEUE_DEPTH>;

fn context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

fn physical() -> open_esp_radio_esp32s31_wifi_embassy::datapath::PinnedTxConsumer<
    'static,
    NoopRawMutex,
    FRAME_CAPACITY,
    HEADROOM,
    TRAILER,
    PHYSICAL_QUEUE_DEPTH,
> {
    let resources = Box::leak(Box::new(PhysicalResources::new()));
    let pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
    resources.split(pool)
}

#[test]
fn unchanged_driver_reaches_the_radio_materializer_without_policy_duplication() {
    let endpoint = Box::leak(Box::new(Endpoint::new()));
    let interface = NetworkInterfaceId::new(3);
    let (mut device, radio) = endpoint.split([2, 0, 0, 0, 0, 3]);
    let network = CompatibilityDatapathNetwork::new(interface, radio, physical());
    network.set_link_state(interface, LinkState::Up);

    device
        .transmit(&mut context())
        .expect("official driver has one software credit")
        .consume(ETHERNET_HEADER_LEN, |frame| frame.fill(0x51));

    let consumer = network.tx_consumer(interface);
    let first = consumer
        .try_materialize_next()
        .expect("one physical SRAM credit is free");
    assert_eq!(first.as_slice(), &[0x51; ETHERNET_HEADER_LEN]);

    device
        .transmit(&mut context())
        .expect("software queue is independent from SRAM")
        .consume(ETHERNET_HEADER_LEN, |frame| frame.fill(0x52));
    assert!(consumer.try_materialize_next().is_none());
    assert_eq!(consumer.queue_len(), 1);

    drop(first);
    let second = consumer
        .try_materialize_next()
        .expect("terminal return releases the physical credit");
    assert_eq!(second.as_slice(), &[0x52; ETHERNET_HEADER_LEN]);
}

#[test]
fn compatibility_rx_parts_use_the_same_radio_publication_contract() {
    let endpoint = Box::leak(Box::new(Endpoint::new()));
    let interface = NetworkInterfaceId::new(4);
    let (mut device, radio) = endpoint.split([2, 0, 0, 0, 0, 4]);
    let network = CompatibilityDatapathNetwork::new(interface, radio, physical());
    network.set_link_state(interface, LinkState::Up);

    network
        .rx_publisher(interface)
        .try_send_parts(
            open_esp_radio_esp32s31_wifi_embassy::datapath::network::EthernetFrameParts {
                destination: [0x11; 6],
                source: [0x22; 6],
                ether_type: 0x0800,
                payload: &[0x33; 5],
            },
        )
        .unwrap();

    let (rx, _) = device.receive(&mut context()).expect("current RX lifetime");
    rx.consume(|frame| {
        assert_eq!(&frame[..6], &[0x11; 6]);
        assert_eq!(&frame[6..12], &[0x22; 6]);
        assert_eq!(&frame[12..14], &[0x08, 0x00]);
        assert_eq!(&frame[14..], &[0x33; 5]);
    });
}

#[test]
fn dual_compatibility_endpoints_share_only_the_physical_horizon() {
    let first_endpoint = Box::leak(Box::new(Endpoint::new()));
    let second_endpoint = Box::leak(Box::new(Endpoint::new()));
    let first_interface = NetworkInterfaceId::new(0);
    let second_interface = NetworkInterfaceId::new(1);
    let (mut first_device, first_radio) = first_endpoint.split([2, 0, 0, 0, 0, 1]);
    let (mut second_device, second_radio) = second_endpoint.split([2, 0, 0, 0, 0, 2]);
    let network = DualCompatibilityDatapathNetwork::new(
        first_interface,
        first_radio,
        second_interface,
        second_radio,
        physical(),
    );
    network.set_link_state(first_interface, LinkState::Up);
    network.set_link_state(second_interface, LinkState::Up);

    first_device
        .transmit(&mut context())
        .unwrap()
        .consume(ETHERNET_HEADER_LEN, |frame| frame.fill(0xa1));
    second_device
        .transmit(&mut context())
        .unwrap()
        .consume(ETHERNET_HEADER_LEN, |frame| frame.fill(0xb2));

    let first = network
        .tx_consumer(first_interface)
        .try_materialize_next()
        .expect("first interface claims the shared SRAM credit");
    assert_eq!(first.as_slice(), &[0xa1; ETHERNET_HEADER_LEN]);
    assert!(
        network
            .tx_consumer(second_interface)
            .try_materialize_next()
            .is_none()
    );
    assert_eq!(network.tx_queue_len(second_interface), 1);

    drop(first);
    let second = network
        .tx_consumer(second_interface)
        .try_materialize_next()
        .expect("the other interface receives the returned global credit");
    assert_eq!(second.as_slice(), &[0xb2; ETHERNET_HEADER_LEN]);
}
