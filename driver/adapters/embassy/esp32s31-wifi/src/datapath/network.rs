//! Role-neutral network capabilities owned by the physical Wi-Fi datapath.

use core::future::Future;

use embassy_futures::select::{Either, select};
#[cfg(feature = "tx-egress-scheduling")]
use open_esp_radio_embassy_net::DefaultEgressControlledNetwork;
use open_esp_radio_embassy_net::{
    DualPinnedNetworkRunner, LinkState, NetworkInterfaceId, OwnedLinkController,
    OwnedNetworkRunner, OwnedRxPublisher, PinnedNetworkLinkController, PinnedNetworkRunner,
    PinnedNetworkTxFrame, PinnedRxPublisher, PinnedTxConsumer, PinnedTxInterfaceConsumer, RawMutex,
    RxEnqueueError,
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;

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

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize> DatapathNetworkRxSet
    for PinnedRxPublisher<'_, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
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

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize> DatapathNetworkRx
    for PinnedRxPublisher<'_, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
{
    fn queue_len(&self) -> usize {
        PinnedRxPublisher::queue_len(self)
    }

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        PinnedRxPublisher::try_send(self, frame)
    }

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        PinnedRxPublisher::try_send_parts(
            self,
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
        )
    }

    fn poll_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        let future = PinnedRxPublisher::wait_ready(self);
        let mut future = core::pin::pin!(future);
        Future::poll(future.as_mut(), context)
    }

    #[cfg(feature = "diagnostics")]
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        PinnedRxPublisher::try_send_observed(self, frame, before_publish)
    }

    #[cfg(feature = "diagnostics")]
    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        PinnedRxPublisher::try_send_parts_observed(
            self,
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
            before_publish,
        )
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

    fn link_controller(&self) -> Self::LinkController;

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher;
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState);
    /// Service one bounded radio-owned egress-control turn.
    ///
    /// The datapath calls this at hardware-transaction boundaries, never from
    /// per-frame queue accessors. Implementations without an egress control
    /// plane return `false`.
    fn service_egress_control(&mut self) -> bool;
    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize;
    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    >;
    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_;
    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_;
    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_;
    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_;

    /// Number of leases in the one physical tagged TX frontier.
    ///
    /// Unlike [`Self::tx_queue_len`], this does not filter by VIF. Combined
    /// scheduling uses it to preserve publication order before dispatching a
    /// lease to a role-specific encoder.
    fn physical_tx_queue_len(&self) -> usize;

    /// Claim the next physical tagged TX lease without filtering or requeue.
    fn try_receive_physical_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    >;

    /// Wait for and claim the next physical tagged TX lease.
    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_;
}

/// Link-only capability which may coexist with the unique Core0 scheduler.
pub trait DatapathNetworkLink {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState);
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetworkLink
    for PinnedNetworkLinkController<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
{
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        PinnedNetworkLinkController::set_link_state(*self, interface, state);
    }
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

    fn service_egress_control(&mut self) -> bool {
        false
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.assert_interface(interface);
        self.network.tx_queue_len()
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        self.assert_interface(interface);
        self.network
            .try_receive_tx()
            .map(PinnedNetworkTxFrame::Owned)
    }

    async fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        self.assert_interface(interface);
        PinnedNetworkTxFrame::Owned(self.network.receive_tx().await)
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        self.assert_interface(interface);
        self.physical.for_interface(interface)
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

    fn physical_tx_queue_len(&self) -> usize {
        self.network.tx_queue_len()
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        self.network
            .try_receive_tx()
            .map(PinnedNetworkTxFrame::Owned)
    }

    async fn receive_physical_tx(
        &self,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        PinnedNetworkTxFrame::Owned(self.network.receive_tx().await)
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

    fn service_egress_control(&mut self) -> bool {
        false
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.endpoint(interface).tx_queue_len()
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        self.endpoint(interface)
            .try_receive_tx()
            .map(PinnedNetworkTxFrame::Owned)
    }

    async fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        PinnedNetworkTxFrame::Owned(self.endpoint(interface).receive_tx().await)
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        let _ = self.endpoint(interface);
        self.physical.for_interface(interface)
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

    fn physical_tx_queue_len(&self) -> usize {
        self.first
            .tx_queue_len()
            .saturating_add(self.second.tx_queue_len())
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        self.first
            .try_receive_tx()
            .or_else(|| self.second.try_receive_tx())
            .map(PinnedNetworkTxFrame::Owned)
    }

    async fn receive_physical_tx(
        &self,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        loop {
            if let Some(frame) = self.try_receive_physical_tx() {
                return frame;
            }
            match select(
                self.first.wait_tx_publication(),
                self.second.wait_tx_publication(),
            )
            .await
            {
                Either::First(()) | Either::Second(()) => {}
            }
        }
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
    for PinnedNetworkRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    type LinkController = PinnedNetworkLinkController<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >;
    type RxPublisher = PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>;

    fn link_controller(&self) -> Self::LinkController {
        PinnedNetworkRunner::link_controller(self)
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        assert_eq!(
            interface,
            self.interface(),
            "single network owner cannot publish to another interface"
        );
        PinnedNetworkRunner::rx_publisher(self)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        assert_eq!(
            interface,
            self.interface(),
            "single network owner cannot change another interface"
        );
        PinnedNetworkRunner::set_link_state(self, state);
    }

    fn service_egress_control(&mut self) -> bool {
        false
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        assert_eq!(interface, self.interface());
        PinnedNetworkRunner::tx_queue_len(self)
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        assert_eq!(interface, self.interface());
        PinnedNetworkRunner::try_receive_tx(self)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_ {
        assert_eq!(interface, self.interface());
        PinnedNetworkRunner::receive_tx(self)
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        assert_eq!(interface, self.interface());
        PinnedNetworkRunner::tx_consumer(self)
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        assert_eq!(interface, self.interface());
        PinnedNetworkRunner::wait_tx_ready(self)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        assert_eq!(interface, self.interface());
        let tx = PinnedNetworkRunner::tx_consumer(self);
        async move { tx.wait_queue_len_at_least(minimum).await }
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        PinnedNetworkRunner::wait_tx_publication(self)
    }

    fn physical_tx_queue_len(&self) -> usize {
        PinnedNetworkRunner::tx_queue_len(self)
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        PinnedNetworkRunner::try_receive_tx(self)
    }

    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_ {
        PinnedNetworkRunner::receive_tx(self)
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
    for DualPinnedNetworkRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    type LinkController = PinnedNetworkLinkController<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >;
    type RxPublisher = PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>;

    fn link_controller(&self) -> Self::LinkController {
        DualPinnedNetworkRunner::link_controller(self)
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        DualPinnedNetworkRunner::rx_publisher(self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        DualPinnedNetworkRunner::set_link_state(self, interface, state);
    }

    fn service_egress_control(&mut self) -> bool {
        false
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        DualPinnedNetworkRunner::tx_consumer(self).queue_len_for(interface)
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        DualPinnedNetworkRunner::tx_consumer(self).try_receive_for(interface)
    }

    async fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        loop {
            if let Some(frame) =
                DualPinnedNetworkRunner::tx_consumer(self).try_receive_for(interface)
            {
                return frame;
            }
            DualPinnedNetworkRunner::wait_tx_publication(self).await;
        }
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        assert!(
            interface == self.first_interface() || interface == self.second_interface(),
            "TX interface does not belong to this radio owner"
        );
        DualPinnedNetworkRunner::tx_consumer(self).for_interface(interface)
    }

    async fn wait_tx_ready(&self, interface: NetworkInterfaceId) {
        loop {
            if DualPinnedNetworkRunner::tx_consumer(self).queue_len_for(interface) != 0 {
                return;
            }
            DualPinnedNetworkRunner::wait_tx_publication(self).await;
        }
    }

    async fn wait_tx_queue_len_at_least(&self, interface: NetworkInterfaceId, minimum: usize) {
        loop {
            if DualPinnedNetworkRunner::tx_consumer(self).queue_len_for(interface) >= minimum {
                return;
            }
            DualPinnedNetworkRunner::wait_tx_publication(self).await;
        }
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        DualPinnedNetworkRunner::wait_tx_publication(self)
    }

    fn physical_tx_queue_len(&self) -> usize {
        DualPinnedNetworkRunner::tx_queue_len(self)
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        DualPinnedNetworkRunner::try_receive_tx(self)
    }

    async fn receive_physical_tx(
        &self,
    ) -> PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        loop {
            if let Some(frame) = DualPinnedNetworkRunner::try_receive_tx(self) {
                return frame;
            }
            DualPinnedNetworkRunner::wait_tx_publication(self).await;
        }
    }
}

#[cfg(feature = "tx-egress-scheduling")]
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
    for DefaultEgressControlledNetwork<'resources, M, N>
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
        >,
{
    type LinkController = N::LinkController;
    type RxPublisher = N::RxPublisher;

    fn link_controller(&self) -> Self::LinkController {
        N::link_controller(self.inner())
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        N::rx_publisher(self.inner(), interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        N::set_link_state(self.inner(), interface, state);
    }

    fn service_egress_control(&mut self) -> bool {
        DefaultEgressControlledNetwork::service_egress_control(self)
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        N::tx_queue_len(self.inner(), interface)
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        N::try_receive_tx(self.inner(), interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_ {
        N::receive_tx(self.inner(), interface)
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        N::tx_consumer(self.inner(), interface)
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        N::wait_tx_ready(self.inner(), interface)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        N::wait_tx_queue_len_at_least(self.inner(), interface, minimum)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        self.wait_egress_or(N::wait_tx_publication(self.inner()))
    }

    fn physical_tx_queue_len(&self) -> usize {
        N::physical_tx_queue_len(self.inner())
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        N::try_receive_physical_tx(self.inner())
    }

    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_ {
        N::receive_physical_tx(self.inner())
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

    fn link_controller(&self) -> Self::LinkController {
        N::link_controller(*self)
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        N::rx_publisher(*self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        N::set_link_state(*self, interface, state);
    }

    fn service_egress_control(&mut self) -> bool {
        // A shared reference cannot own mutable radio policy. Production
        // egress control is carried by value or through `&mut N` below.
        false
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        N::tx_queue_len(*self, interface)
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        N::try_receive_tx(*self, interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_ {
        N::receive_tx(*self, interface)
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
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

    fn physical_tx_queue_len(&self) -> usize {
        N::physical_tx_queue_len(*self)
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        N::try_receive_physical_tx(*self)
    }

    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_ {
        N::receive_physical_tx(*self)
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

    fn link_controller(&self) -> Self::LinkController {
        N::link_controller(*self)
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        N::rx_publisher(*self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        N::set_link_state(*self, interface, state);
    }

    fn service_egress_control(&mut self) -> bool {
        N::service_egress_control(*self)
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        N::tx_queue_len(*self, interface)
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        N::try_receive_tx(*self, interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_ {
        N::receive_tx(*self, interface)
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
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

    fn physical_tx_queue_len(&self) -> usize {
        N::physical_tx_queue_len(*self)
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<
        PinnedNetworkTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > {
        N::try_receive_physical_tx(*self)
    }

    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedNetworkTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    > + '_ {
        N::receive_physical_tx(*self)
    }
}
