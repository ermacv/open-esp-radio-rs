//! Role-neutral network capabilities owned by the physical Wi-Fi datapath.

use core::future::Future;

use embassy_futures::select::select;
use open_esp_radio_embassy_net::{
    LinkState, NetworkInterfaceId, OwnedLinkController, OwnedNetworkRunner, OwnedNetworkTxFrame,
    OwnedRxPublisher, RawMutex, RxEnqueueError,
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;

use super::{
    DatapathTxConsumer, PinnedTxConsumer, PinnedTxFrame, SelectedBurstMaterializer, SoftwareTxFrame,
};

/// RX-only network publication capability exposed to one finite DATAPATH service.
/// It cannot observe or claim network-owned TX slots.
pub trait DatapathNetworkRx {
    /// Number of copied RX frames still waiting in the owned network queue.
    fn queue_len(&self) -> usize;

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError>;

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError>;

    /// Poll the next publication credit without allocating a boxed future.
    fn poll_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()>;

    #[cfg(feature = "diagnostics")]
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError>;

    #[cfg(feature = "diagnostics")]
    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError>;
}

/// RX publication authority presented to one DATAPATH services graph.
///
/// Standalone roles use only `primary_mut`. Same-channel compositions must
/// select a concrete endpoint by identity after fact-only VIF routing. The
/// trait has no fallback from an unknown identity to the primary endpoint.
pub trait DatapathNetworkRxSet {
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx;

    fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx>;

    fn pair_mut(
        &mut self,
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)>;

    fn poll_primary_ready(
        &mut self,
        context: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        self.primary_mut().poll_ready(context)
    }

    fn poll_any_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        self.poll_primary_ready(context)
    }
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> DatapathNetworkRxSet
    for OwnedRxPublisher<'_, M, RX_QUEUE_DEPTH>
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

/// Addressed RX publication endpoints owned by one physical DATAPATH.
///
/// A same-channel STA+AP scheduler must select the logical endpoint only
/// after the common RX dispatcher has classified the retained 802.11 owner.
/// Returning `None` for an unknown identity keeps that failure explicit; the
/// caller must account and release the exact frame instead of publishing it
/// through whichever role happens to be active.
pub struct DatapathNetworkRxEndpoints<A, B> {
    first_interface: NetworkInterfaceId,
    first: A,
    second_interface: NetworkInterfaceId,
    second: B,
}

impl<A, B> DatapathNetworkRxEndpoints<A, B> {
    pub fn new(
        first_interface: NetworkInterfaceId,
        first: A,
        second_interface: NetworkInterfaceId,
        second: B,
    ) -> Self {
        assert_ne!(
            first_interface, second_interface,
            "DATAPATH RX endpoints require distinct interface identities"
        );
        Self {
            first_interface,
            first,
            second_interface,
            second,
        }
    }

    pub const fn first_interface(&self) -> NetworkInterfaceId {
        self.first_interface
    }

    pub const fn second_interface(&self) -> NetworkInterfaceId {
        self.second_interface
    }

    pub fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx>
    where
        A: DatapathNetworkRx,
        B: DatapathNetworkRx,
    {
        if interface == self.first_interface {
            Some(&mut self.first)
        } else if interface == self.second_interface {
            Some(&mut self.second)
        } else {
            None
        }
    }

    pub fn into_parts(self) -> (A, B) {
        (self.first, self.second)
    }
}

impl<A: DatapathNetworkRx, B: DatapathNetworkRx> DatapathNetworkRxSet
    for DatapathNetworkRxEndpoints<A, B>
{
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx {
        &mut self.first
    }

    fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx> {
        DatapathNetworkRxEndpoints::get_mut(self, interface)
    }

    fn pair_mut(
        &mut self,
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)> {
        if first == self.first_interface && second == self.second_interface {
            Some((&mut self.first, &mut self.second))
        } else if first == self.second_interface && second == self.first_interface {
            Some((&mut self.second, &mut self.first))
        } else {
            None
        }
    }

    fn poll_any_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        if self.first.poll_ready(context).is_ready() {
            core::task::Poll::Ready(())
        } else {
            self.second.poll_ready(context)
        }
    }
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> DatapathNetworkRx
    for OwnedRxPublisher<'_, M, RX_QUEUE_DEPTH>
{
    fn queue_len(&self) -> usize {
        OwnedRxPublisher::queue_len(self)
    }

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        OwnedRxPublisher::try_send(self, frame)
    }

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        OwnedRxPublisher::try_send_parts(
            self,
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
        )
    }

    fn poll_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        OwnedRxPublisher::poll_ready(self, context)
    }

    #[cfg(feature = "diagnostics")]
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        OwnedRxPublisher::try_send(self, frame)
    }

    #[cfg(feature = "diagnostics")]
    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        OwnedRxPublisher::try_send_parts(
            self,
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
        )
    }
}

/// Radio-side network ownership consumed by [`DatapathRunner`].
///
/// Single-VIF owners expose one RX endpoint. A dual owner selects between
/// permanent STA/AP RX endpoints while retaining the sole tagged TX consumer.
/// Role-specific semantics remain outside this scheduler contract.
pub trait DatapathNetwork<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
{
    type LinkController: DatapathNetworkLink + Copy;
    type RxPublisher: DatapathNetworkRxSet;
    type TxFrame: SoftwareTxFrame;
    type PhysicalTxFrame: crate::datapath::MaterializedTxFrame;
    type TxConsumer<'network>: SelectedBurstMaterializer<
            SoftwareFrame = Self::TxFrame,
            PhysicalFrame = Self::PhysicalTxFrame,
        >
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController;

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher;
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState);
    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize;
    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<Self::TxFrame>;
    fn receive_tx(&self, interface: NetworkInterfaceId)
    -> impl Future<Output = Self::TxFrame> + '_;
    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_>;
    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_;
    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_;
    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_;
}

/// Link-only capability which may coexist with the unique Core0 scheduler.
pub trait DatapathNetworkLink {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState);
}

impl<M: RawMutex> DatapathNetworkLink for OwnedLinkController<'_, M> {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        assert_eq!(
            interface,
            self.interface(),
            "owned link controller cannot change another interface"
        );
        self.set_link_up(matches!(state, LinkState::Up));
    }
}

/// Link-only authority for two permanent owned network endpoints.
///
/// The physical DATAPATH retains this pair across STA, AP and same-channel
/// STA+AP epochs. A role transition can therefore change only its addressed
/// logical interface and cannot accidentally publish link state through the
/// other endpoint.
pub struct OwnedNetworkLinkControllers<'resources, M: RawMutex> {
    first: OwnedLinkController<'resources, M>,
    second: OwnedLinkController<'resources, M>,
}

impl<M: RawMutex> Clone for OwnedNetworkLinkControllers<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for OwnedNetworkLinkControllers<'_, M> {}

impl<'resources, M: RawMutex> OwnedNetworkLinkControllers<'resources, M> {
    fn new(
        first: OwnedLinkController<'resources, M>,
        second: OwnedLinkController<'resources, M>,
    ) -> Self {
        assert_ne!(
            first.interface(),
            second.interface(),
            "dual owned endpoints require distinct interface identities"
        );
        Self { first, second }
    }
}

impl<M: RawMutex> DatapathNetworkLink for OwnedNetworkLinkControllers<'_, M> {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        let controller = if interface == self.first.interface() {
            self.first
        } else {
            assert_eq!(
                interface,
                self.second.interface(),
                "link interface does not belong to this dual owned owner"
            );
            self.second
        };
        controller.set_link_up(matches!(state, LinkState::Up));
    }
}

/// One owned Xarxa endpoint backed by a separate physical SRAM TX horizon.
///
/// `NETWORK_TX_DEPTH` bounds complete packet owners waiting for radio
/// selection. `TX_QUEUE_DEPTH` bounds DMA-capable execution storage. Keeping
/// the dimensions independent prevents software backlog policy from silently
/// consuming additional physical radio credits as peer count grows.
pub struct OwnedDatapathNetwork<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    network: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
    physical: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
}

/// Two owned Xarxa endpoints sharing one fixed physical SRAM TX horizon.
///
/// Each logical interface has an independent bounded software-owner queue.
/// The sole Core0 DATAPATH chooses an interface before claiming its next
/// owner, then promotes only the selected frame or burst through `physical`.
/// Associated-peer count therefore changes software queue metadata/backlog,
/// never the number of DMA-capable SRAM slots.
pub struct DualOwnedDatapathNetwork<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    first: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
    second: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
    physical: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    DualOwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    pub fn new(
        first: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
        second: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
        physical: PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Self {
        assert_ne!(
            first.interface(),
            second.interface(),
            "dual owned endpoints require distinct interface identities"
        );
        Self {
            first,
            second,
            physical,
        }
    }

    fn endpoint(
        &self,
        interface: NetworkInterfaceId,
    ) -> &OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH> {
        if interface == self.first.interface() {
            &self.first
        } else {
            assert_eq!(
                interface,
                self.second.interface(),
                "network interface does not belong to this dual owned owner"
            );
            &self.second
        }
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    OwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    pub const fn new(
        network: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
        physical: PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Self {
        Self { network, physical }
    }

    pub fn into_parts(
        self,
    ) -> (
        OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
        PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) {
        (self.network, self.physical)
    }

    pub const fn interface(&self) -> NetworkInterfaceId {
        self.network.interface()
    }

    fn assert_interface(&self, interface: NetworkInterfaceId) {
        assert_eq!(
            interface,
            self.interface(),
            "single owned network cannot access another interface"
        );
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
    for OwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    type LinkController = OwnedLinkController<'resources, M>;
    type RxPublisher = OwnedRxPublisher<'resources, M, RX_QUEUE_DEPTH>;
    type TxFrame = OwnedNetworkTxFrame;
    type PhysicalTxFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type TxConsumer<'network>
        = DatapathTxConsumer<
        'network,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        self.network.link_controller()
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        self.assert_interface(interface);
        self.network.rx_publisher()
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.link_controller().set_link_state(interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.assert_interface(interface);
        self.network.tx_queue_len()
    }

    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<OwnedNetworkTxFrame> {
        self.assert_interface(interface);
        self.network.try_receive_tx()
    }

    async fn receive_tx(&self, interface: NetworkInterfaceId) -> OwnedNetworkTxFrame {
        self.assert_interface(interface);
        self.network.receive_tx().await
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> DatapathTxConsumer<'_, 'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        self.assert_interface(interface);
        DatapathTxConsumer::new(&self.network, self.physical.for_interface(interface))
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        self.assert_interface(interface);
        self.network.wait_tx_queue_len_at_least(1)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        self.assert_interface(interface);
        self.network.wait_tx_queue_len_at_least(minimum)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        self.network.wait_tx_publication()
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> open_esp_radio_wifi_embassy::station_network::StationNetworkLink
    for DualOwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    fn publish_link_up(&self) {
        self.set_link_state(
            crate::roles::concurrent::STA_NETWORK_INTERFACE_ID,
            LinkState::Up,
        );
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
    for DualOwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    type LinkController = OwnedNetworkLinkControllers<'resources, M>;
    type RxPublisher = OwnedRxPublisher<'resources, M, RX_QUEUE_DEPTH>;
    type TxFrame = OwnedNetworkTxFrame;
    type PhysicalTxFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type TxConsumer<'network>
        = DatapathTxConsumer<
        'network,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        OwnedNetworkLinkControllers::new(
            self.first.link_controller(),
            self.second.link_controller(),
        )
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        self.endpoint(interface).rx_publisher()
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.link_controller().set_link_state(interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.endpoint(interface).tx_queue_len()
    }

    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<OwnedNetworkTxFrame> {
        self.endpoint(interface).try_receive_tx()
    }

    async fn receive_tx(&self, interface: NetworkInterfaceId) -> OwnedNetworkTxFrame {
        self.endpoint(interface).receive_tx().await
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> DatapathTxConsumer<'_, 'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        DatapathTxConsumer::new(
            self.endpoint(interface),
            self.physical.for_interface(interface),
        )
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        self.endpoint(interface).wait_tx_queue_len_at_least(1)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        self.endpoint(interface).wait_tx_queue_len_at_least(minimum)
    }

    async fn wait_tx_publication(&self) {
        let _ = select(
            self.first.wait_tx_publication(),
            self.second.wait_tx_publication(),
        )
        .await;
    }
}

impl<
    'resources,
    M,
    N,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
    for &N
where
    M: RawMutex + 'resources,
    N: DatapathNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        > + ?Sized,
{
    type LinkController = N::LinkController;
    type RxPublisher = N::RxPublisher;
    type TxFrame = N::TxFrame;
    type PhysicalTxFrame = N::PhysicalTxFrame;
    type TxConsumer<'network>
        = N::TxConsumer<'network>
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        N::link_controller(*self)
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        N::rx_publisher(*self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        N::set_link_state(*self, interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        N::tx_queue_len(*self, interface)
    }

    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<Self::TxFrame> {
        N::try_receive_tx(*self, interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<Output = Self::TxFrame> + '_ {
        N::receive_tx(*self, interface)
    }

    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_> {
        N::tx_consumer(*self, interface)
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        N::wait_tx_ready(*self, interface)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        N::wait_tx_queue_len_at_least(*self, interface, minimum)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        N::wait_tx_publication(*self)
    }
}

impl<
    'resources,
    M,
    N,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
    for &mut N
where
    M: RawMutex + 'resources,
    N: DatapathNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        > + ?Sized,
{
    type LinkController = N::LinkController;
    type RxPublisher = N::RxPublisher;
    type TxFrame = N::TxFrame;
    type PhysicalTxFrame = N::PhysicalTxFrame;
    type TxConsumer<'network>
        = N::TxConsumer<'network>
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        N::link_controller(*self)
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        N::rx_publisher(*self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        N::set_link_state(*self, interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        N::tx_queue_len(*self, interface)
    }

    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<Self::TxFrame> {
        N::try_receive_tx(*self, interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<Output = Self::TxFrame> + '_ {
        N::receive_tx(*self, interface)
    }

    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_> {
        N::tx_consumer(*self, interface)
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        N::wait_tx_ready(*self, interface)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        N::wait_tx_queue_len_at_least(*self, interface, minimum)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        N::wait_tx_publication(*self)
    }
}
