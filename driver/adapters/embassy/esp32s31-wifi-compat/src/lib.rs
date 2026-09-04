#![no_std]
#![forbid(unsafe_code)]

//! Upstream-compatible copied-frame integration for the ESP32-S31 radio core.
//!
//! This crate is the only bridge between the unchanged released
//! `embassy-net-driver` adapter and open-radio's private selected-burst
//! materialization contract. Complete Ethernet frames wait in bounded general
//! adapter storage. The radio chooses a logical interface before the bridge
//! reserves final DMA-visible SRAM and performs one additional copy.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_embassy_net_compat::{
    EthernetFrame, RadioLinkController, RadioRunner, RadioRxPublisher, RadioTxConsumer,
};
#[cfg(feature = "tx-phase-telemetry")]
use open_esp_radio_esp32s31_wifi_embassy::datapath::MaterializationOwnershipSnapshot;
use open_esp_radio_esp32s31_wifi_embassy::datapath::{
    PinnedTxConsumer, PinnedTxFrame, PinnedTxInterfaceConsumer, SelectedBurstMaterializer,
    SoftwareTxFrame,
    network::{DatapathNetwork, DatapathNetworkLink, DatapathNetworkRx, DatapathNetworkRxSet},
};
use open_esp_radio_network::{LinkState, NetworkInterfaceId, RxEnqueueError};

/// Complete frame ownership received from an unchanged Embassy driver.
pub struct CompatibilityTxFrame<const FRAME_CAPACITY: usize> {
    interface: NetworkInterfaceId,
    frame: EthernetFrame<FRAME_CAPACITY>,
}

impl<const FRAME_CAPACITY: usize> CompatibilityTxFrame<FRAME_CAPACITY> {
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    pub fn ethernet(&self) -> &[u8] {
        self.frame.as_slice()
    }
}

impl<const FRAME_CAPACITY: usize> SoftwareTxFrame for CompatibilityTxFrame<FRAME_CAPACITY> {
    fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    fn ethernet(&self) -> &[u8] {
        self.frame.as_slice()
    }
}

/// RX-only compatibility publisher narrowed to the radio contract.
pub struct CompatibilityRxPublisher<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    inner: RadioRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Clone
    for CompatibilityRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Copy
    for CompatibilityRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> DatapathNetworkRx
    for CompatibilityRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn queue_len(&self) -> usize {
        self.inner.queue_len()
    }

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.inner.try_send(frame)
    }

    fn try_send_parts(
        &mut self,
        frame: open_esp_radio_esp32s31_wifi_embassy::datapath::network::EthernetFrameParts<'_>,
    ) -> Result<(), RxEnqueueError> {
        self.inner.try_send_parts(
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
        )
    }

    fn poll_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        self.inner.poll_ready(context)
    }

    #[cfg(feature = "diagnostics")]
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.inner.try_send(frame)
    }

    #[cfg(feature = "diagnostics")]
    fn try_send_parts_observed(
        &mut self,
        frame: open_esp_radio_esp32s31_wifi_embassy::datapath::network::EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.try_send_parts(frame)
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> DatapathNetworkRxSet
    for CompatibilityRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx {
        self
    }

    fn get_mut(&mut self, _interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx> {
        None
    }

    fn pair_mut(
        &mut self,
        _first: NetworkInterfaceId,
        _second: NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)> {
        None
    }
}

/// Link authority tagged with exactly one logical interface.
pub struct CompatibilityLinkController<'resources, M: RawMutex> {
    interface: NetworkInterfaceId,
    inner: RadioLinkController<'resources, M>,
}

impl<M: RawMutex> Clone for CompatibilityLinkController<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for CompatibilityLinkController<'_, M> {}

impl<M: RawMutex> DatapathNetworkLink for CompatibilityLinkController<'_, M> {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        assert_eq!(
            interface, self.interface,
            "compatibility link controller cannot change another interface"
        );
        self.inner.set_link_state(state);
    }
}

/// Radio-side source plus fixed physical SRAM allocator for one interface.
pub struct CompatibilityTxConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
> {
    interface: NetworkInterfaceId,
    source: RadioTxConsumer<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>,
    physical: PinnedTxInterfaceConsumer<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        PHYSICAL_QUEUE_DEPTH,
    >,
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
> Clone
    for CompatibilityTxConsumer<
        '_,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
> Copy
    for CompatibilityTxConsumer<
        '_,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
> SelectedBurstMaterializer
    for CompatibilityTxConsumer<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    type SoftwareFrame = CompatibilityTxFrame<FRAME_CAPACITY>;
    type PhysicalFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, PHYSICAL_QUEUE_DEPTH>;

    fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    fn queue_len(&self) -> usize {
        self.source.queue_len()
    }

    fn try_take(&self) -> Option<Self::SoftwareFrame> {
        self.source.try_receive().map(|frame| CompatibilityTxFrame {
            interface: self.interface,
            frame,
        })
    }

    fn try_materialize(
        &self,
        frame: Self::SoftwareFrame,
    ) -> Result<Self::PhysicalFrame, Self::SoftwareFrame> {
        self.physical.try_materialize(frame)
    }

    fn try_materialize_next(&self) -> Option<Self::PhysicalFrame> {
        self.physical.try_materialize_from(|| self.try_take())
    }

    fn materialization_capacity(&self) -> usize {
        self.physical.promotion_capacity()
    }

    #[cfg(feature = "tx-phase-telemetry")]
    fn ownership_snapshot(&self) -> MaterializationOwnershipSnapshot {
        self.physical.ownership_snapshot()
    }

    fn try_materialize_batch<const BATCH: usize>(
        &self,
        sources: &mut [Option<Self::SoftwareFrame>; BATCH],
        destinations: &mut [Option<Self::PhysicalFrame>; BATCH],
    ) -> bool {
        self.physical.try_materialize_batch(sources, destinations)
    }
}

/// One unchanged Embassy endpoint composed with the shared physical TX pool.
pub struct CompatibilityDatapathNetwork<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const NETWORK_QUEUE_DEPTH: usize,
    const PHYSICAL_QUEUE_DEPTH: usize,
> {
    interface: NetworkInterfaceId,
    network: RadioRunner<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>,
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
    CompatibilityDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    pub const fn new(
        interface: NetworkInterfaceId,
        network: RadioRunner<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>,
        physical: PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            PHYSICAL_QUEUE_DEPTH,
        >,
    ) -> Self {
        Self {
            interface,
            network,
            physical,
        }
    }

    fn assert_interface(&self, interface: NetworkInterfaceId) {
        assert_eq!(
            interface, self.interface,
            "single compatibility network cannot access another interface"
        );
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
    for CompatibilityDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    type LinkController = CompatibilityLinkController<'resources, M>;
    type RxPublisher = CompatibilityRxPublisher<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>;
    type TxFrame = CompatibilityTxFrame<FRAME_CAPACITY>;
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
        CompatibilityLinkController {
            interface: self.interface,
            inner: self.network.link_controller(),
        }
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        self.assert_interface(interface);
        CompatibilityRxPublisher {
            inner: self.network.rx_publisher(),
        }
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.link_controller().set_link_state(interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.assert_interface(interface);
        self.network.tx_queue_len()
    }

    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<Self::TxFrame> {
        self.assert_interface(interface);
        self.network
            .try_receive_tx()
            .map(|frame| CompatibilityTxFrame { interface, frame })
    }

    async fn receive_tx(&self, interface: NetworkInterfaceId) -> Self::TxFrame {
        self.assert_interface(interface);
        CompatibilityTxFrame {
            interface,
            frame: self.network.receive_tx().await,
        }
    }

    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_> {
        self.assert_interface(interface);
        CompatibilityTxConsumer {
            interface,
            source: self.network.tx_consumer(),
            physical: self.physical.for_interface(interface),
        }
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        self.assert_interface(interface);
        self.network.tx_consumer().wait_for_queue_len_at_least(1)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        self.assert_interface(interface);
        self.network
            .tx_consumer()
            .wait_for_queue_len_at_least(minimum)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        self.network.tx_consumer().wait_for_publication()
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
    for CompatibilityDatapathNetwork<
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
        self.set_link_state(self.interface, LinkState::Up);
    }
}

// The dual-VIF bridge follows after the single endpoint so compatibility
// integration can reuse exactly the same endpoint capabilities.
mod dual;

pub use dual::{CompatibilityLinkControllers, DualCompatibilityDatapathNetwork};
