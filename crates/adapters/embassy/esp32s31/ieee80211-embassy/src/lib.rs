#![no_std]
#![forbid(unsafe_code)]

//! Upstream-compatible copied-frame integration for the ESP32-S31 radio core.
//!
//! This crate is the only bridge between the unchanged released
//! `embassy-net-driver` adapter and open-radio's private selected-burst
//! materialization contract. Complete Ethernet frames wait in bounded general
//! adapter storage. The radio chooses a logical interface before the bridge
//! reserves final DMA-visible SRAM and performs one additional copy.
//!
//! Standalone RX publication drops a new frame when upstream storage is
//! full. It cannot await RX space while holding the radio owner: upstream RX
//! needs a reply TX token whose return depends on that owner's TX progress.
//! Existing queued frames retain their ownership and order.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_embassy_net_upstream::{
    RadioLinkController, RadioRunner, RadioRxPublisher, RadioTxConsumer, RadioTxFrame,
};
#[cfg(feature = "tx-phase-telemetry")]
use oer_esp32s31_wifi_runtime::datapath::MaterializationOwnershipSnapshot;
use oer_esp32s31_wifi_runtime::datapath::{
    PinnedTxConsumer, PinnedTxFrame, PinnedTxInterfaceConsumer, SelectedBurstMaterializer,
    SoftwareTxFrame,
    network::{
        DatapathNetwork, DatapathNetworkLink, DatapathNetworkRx, DatapathNetworkRxSet,
        RxBackpressure,
    },
};
use oer_network::{LinkState, NetworkInterfaceId, RxEnqueueError};

/// Complete frame ownership received from an unchanged Embassy driver.
pub struct EmbassyTxFrame<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const NETWORK_QUEUE_DEPTH: usize,
> {
    interface: NetworkInterfaceId,
    frame: RadioTxFrame<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const NETWORK_QUEUE_DEPTH: usize>
    EmbassyTxFrame<'_, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>
{
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    pub fn ethernet(&self) -> &[u8] {
        self.frame.as_slice()
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const NETWORK_QUEUE_DEPTH: usize> SoftwareTxFrame
    for EmbassyTxFrame<'_, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>
{
    fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    fn ethernet(&self) -> &[u8] {
        self.frame.as_slice()
    }
}

/// RX-only upstream publisher narrowed to the radio contract.
pub struct EmbassyRxPublisher<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    inner: RadioRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Clone
    for EmbassyRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Copy
    for EmbassyRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> DatapathNetworkRx
    for EmbassyRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn backpressure(&self) -> RxBackpressure {
        // Upstream receive() needs a TX token even for an RX-only packet.
        // Waiting on a full RX queue here would stop the radio owner that
        // must materialize TX frames to return those tokens to the stack.
        RxBackpressure::DropFrame
    }

    fn queue_len(&self) -> usize {
        self.inner.queue_len()
    }

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.inner.try_send(frame)
    }

    fn try_send_parts(
        &mut self,
        frame: oer_esp32s31_wifi_runtime::datapath::network::EthernetFrameParts<'_>,
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
        frame: oer_esp32s31_wifi_runtime::datapath::network::EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.try_send_parts(frame)
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> DatapathNetworkRxSet
    for EmbassyRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
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
pub struct EmbassyLinkController<'resources, M: RawMutex> {
    interface: NetworkInterfaceId,
    inner: RadioLinkController<'resources, M>,
}

impl<M: RawMutex> Clone for EmbassyLinkController<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for EmbassyLinkController<'_, M> {}

impl<M: RawMutex> DatapathNetworkLink for EmbassyLinkController<'_, M> {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        assert_eq!(
            interface, self.interface,
            "upstream link controller cannot change another interface"
        );
        self.inner.set_link_state(state);
    }
}

/// Radio-side source plus fixed physical SRAM allocator for one interface.
pub struct EmbassyTxConsumer<
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
    for EmbassyTxConsumer<
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
    for EmbassyTxConsumer<
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
    for EmbassyTxConsumer<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    type SoftwareFrame = EmbassyTxFrame<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>;
    type PhysicalFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, PHYSICAL_QUEUE_DEPTH>;

    fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    fn queue_len(&self) -> usize {
        self.source.queue_len()
    }

    fn try_take(&self) -> Option<Self::SoftwareFrame> {
        self.source.try_receive().map(|frame| EmbassyTxFrame {
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
pub struct EmbassyDatapathNetwork<
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
    EmbassyDatapathNetwork<
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
            "single upstream network cannot access another interface"
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
    for EmbassyDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        NETWORK_QUEUE_DEPTH,
        PHYSICAL_QUEUE_DEPTH,
    >
{
    type LinkController = EmbassyLinkController<'resources, M>;
    type RxPublisher = EmbassyRxPublisher<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>;
    type TxFrame = EmbassyTxFrame<'resources, M, FRAME_CAPACITY, NETWORK_QUEUE_DEPTH>;
    type PhysicalTxFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, PHYSICAL_QUEUE_DEPTH>;
    type TxConsumer<'network>
        = EmbassyTxConsumer<
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
        EmbassyLinkController {
            interface: self.interface,
            inner: self.network.link_controller(),
        }
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        self.assert_interface(interface);
        EmbassyRxPublisher {
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
            .map(|frame| EmbassyTxFrame { interface, frame })
    }

    async fn receive_tx(&self, interface: NetworkInterfaceId) -> Self::TxFrame {
        self.assert_interface(interface);
        EmbassyTxFrame {
            interface,
            frame: self.network.receive_tx().await,
        }
    }

    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_> {
        self.assert_interface(interface);
        EmbassyTxConsumer {
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
> oer_wifi_embassy::station_network::StationNetworkLink
    for EmbassyDatapathNetwork<
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

// The dual-VIF bridge follows after the single endpoint so upstream
// integration can reuse exactly the same endpoint capabilities.
mod dual;

pub use dual::{DualEmbassyDatapathNetwork, EmbassyLinkControllers};
