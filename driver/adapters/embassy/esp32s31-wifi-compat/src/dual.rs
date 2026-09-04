use core::future::Future;

use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_embassy_net_compat::RadioRunner;
use open_esp_radio_esp32s31_wifi_embassy::datapath::{
    PinnedTxConsumer, PinnedTxFrame,
    network::{DatapathNetwork, DatapathNetworkLink},
};
use open_esp_radio_network::{LinkState, NetworkInterfaceId};

use crate::{
    CompatibilityLinkController, CompatibilityRxPublisher, CompatibilityTxConsumer,
    CompatibilityTxFrame,
};

/// Link-only authority for two permanent compatibility endpoints.
pub struct CompatibilityLinkControllers<'resources, M: RawMutex> {
    first: CompatibilityLinkController<'resources, M>,
    second: CompatibilityLinkController<'resources, M>,
}

impl<M: RawMutex> Clone for CompatibilityLinkControllers<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for CompatibilityLinkControllers<'_, M> {}

impl<M: RawMutex> DatapathNetworkLink for CompatibilityLinkControllers<'_, M> {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        if interface == self.first.interface {
            self.first.set_link_state(interface, state);
        } else {
            assert_eq!(
                interface, self.second.interface,
                "link interface does not belong to this dual compatibility owner"
            );
            self.second.set_link_state(interface, state);
        }
    }
}

/// Two unchanged Embassy endpoints sharing one fixed physical SRAM horizon.
pub struct DualCompatibilityDatapathNetwork<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
> {
    first_interface: NetworkInterfaceId,
    first: RadioRunner<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>,
    second_interface: NetworkInterfaceId,
    second: RadioRunner<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>,
    physical:
        PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, PHYSICAL_QUEUE_DEPTH>,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
>
    DualCompatibilityDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    pub fn new(
        first_interface: NetworkInterfaceId,
        first: RadioRunner<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>,
        second_interface: NetworkInterfaceId,
        second: RadioRunner<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>,
        physical: PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            PHYSICAL_QUEUE_DEPTH,
        >,
    ) -> Self {
        assert_ne!(
            first_interface, second_interface,
            "dual compatibility endpoints require distinct interface identities"
        );
        Self {
            first_interface,
            first,
            second_interface,
            second,
            physical,
        }
    }

    fn endpoint(
        &self,
        interface: NetworkInterfaceId,
    ) -> &RadioRunner<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH> {
        if interface == self.first_interface {
            &self.first
        } else {
            assert_eq!(
                interface, self.second_interface,
                "network interface does not belong to this dual compatibility owner"
            );
            &self.second
        }
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
> DatapathNetwork
    for DualCompatibilityDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    type LinkController = CompatibilityLinkControllers<'resources, M>;
    type RxPublisher = CompatibilityRxPublisher<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>;
    type TxFrame = CompatibilityTxFrame<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>;
    type PhysicalTxFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, PHYSICAL_QUEUE_DEPTH>;
    type TxConsumer<'network>
        = CompatibilityTxConsumer<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        CompatibilityLinkControllers {
            first: CompatibilityLinkController {
                interface: self.first_interface,
                inner: self.first.link_controller(),
            },
            second: CompatibilityLinkController {
                interface: self.second_interface,
                inner: self.second.link_controller(),
            },
        }
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        CompatibilityRxPublisher {
            inner: self.endpoint(interface).rx_publisher(),
        }
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.link_controller().set_link_state(interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.endpoint(interface).tx_queue_len()
    }

    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<Self::TxFrame> {
        self.endpoint(interface)
            .try_receive_tx()
            .map(|frame| CompatibilityTxFrame { interface, frame })
    }

    async fn receive_tx(&self, interface: NetworkInterfaceId) -> Self::TxFrame {
        CompatibilityTxFrame {
            interface,
            frame: self.endpoint(interface).receive_tx().await,
        }
    }

    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_> {
        CompatibilityTxConsumer {
            interface,
            source: self.endpoint(interface).tx_consumer(),
            physical: self.physical.for_interface(interface),
        }
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        self.endpoint(interface)
            .tx_consumer()
            .wait_for_queue_len_at_least(1)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        self.endpoint(interface)
            .tx_consumer()
            .wait_for_queue_len_at_least(minimum)
    }

    async fn wait_tx_publication(&self) {
        let _ = select(
            self.first.tx_consumer().wait_for_publication(),
            self.second.tx_consumer().wait_for_publication(),
        )
        .await;
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
> open_esp_radio_wifi_embassy::station_network::StationNetworkLink
    for DualCompatibilityDatapathNetwork<
        '_,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    fn publish_link_up(&self) {
        self.set_link_state(
            open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::STA_NETWORK_INTERFACE_ID,
            LinkState::Up,
        );
    }
}
