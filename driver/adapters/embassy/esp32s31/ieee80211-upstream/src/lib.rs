#![no_std]
#![forbid(unsafe_code)]

//! Upstream packet ownership at the common radio scheduling boundary.
//!
//! Both logical interfaces share the same physical SRAM allocator. A TX
//! owner stays in the original Xarxa pool until selected for materialization.

use core::task::{Context, Poll};
use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_embassy::datapath::{
    PinnedTxConsumer, PinnedTxFrame, PinnedTxInterfaceConsumer, SelectedBurstMaterializer,
    network::{
        DatapathNetwork, DatapathNetworkLink, DatapathNetworkRx, DatapathNetworkRxSet,
        EthernetFrameParts,
    },
};
use open_esp_radio_network::{LinkState, NetworkInterfaceId, RxEnqueueError};
use open_esp_radio_xarxa_upstream::{Endpoint, LinkController, RxPublisher, TxFrame};

pub struct Rx<'a, M: RawMutex, const RX: usize, const TX: usize>(RxPublisher<'a, M, RX, TX>);

impl<M: RawMutex, const RX: usize, const TX: usize> DatapathNetworkRx for Rx<'_, M, RX, TX> {
    fn pool_exhaustion(
        &self,
    ) -> open_esp_radio_esp32s31_wifi_embassy::datapath::network::RxPoolExhaustion {
        open_esp_radio_esp32s31_wifi_embassy::datapath::network::RxPoolExhaustion::DropFrame
    }
    fn queue_len(&self) -> usize {
        self.0.queue_len()
    }
    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.0.try_send(frame)
    }
    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        self.0.try_send_parts(
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
        )
    }
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.0.poll_ready(cx)
    }
    #[cfg(feature = "diagnostics")]
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        self.0.try_send_observed(frame, before_publish)
    }
    #[cfg(feature = "diagnostics")]
    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        self.0.try_send_parts_observed(
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
            before_publish,
        )
    }
}
impl<M: RawMutex, const RX: usize, const TX: usize> DatapathNetworkRxSet for Rx<'_, M, RX, TX> {
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx {
        self
    }
    fn get_mut(&mut self, _: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx> {
        None
    }
    fn pair_mut(
        &mut self,
        _: NetworkInterfaceId,
        _: NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)> {
        None
    }
}

pub struct Links<'a, M: RawMutex, const RX: usize, const TX: usize>(
    [LinkController<'a, M, RX, TX>; 2],
);
impl<M: RawMutex, const RX: usize, const TX: usize> Copy for Links<'_, M, RX, TX> {}
impl<M: RawMutex, const RX: usize, const TX: usize> Clone for Links<'_, M, RX, TX> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<M: RawMutex, const RX: usize, const TX: usize> DatapathNetworkLink for Links<'_, M, RX, TX> {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.0
            .iter()
            .find(|link| link.interface() == interface)
            .expect("network interface belongs to this radio")
            .set_link_state(state);
    }
}

pub struct Consumer<
    'a,
    M: RawMutex,
    const FRAME: usize,
    const HEAD: usize,
    const TAIL: usize,
    const RX: usize,
    const TX: usize,
    const PHYSICAL: usize,
> {
    endpoint: Endpoint<'a, M, RX, TX>,
    physical: PinnedTxInterfaceConsumer<'a, M, FRAME, HEAD, TAIL, PHYSICAL>,
}

impl<
    'a,
    M: RawMutex + 'a,
    const FRAME: usize,
    const HEAD: usize,
    const TAIL: usize,
    const RX: usize,
    const TX: usize,
    const PHYSICAL: usize,
> SelectedBurstMaterializer for Consumer<'a, M, FRAME, HEAD, TAIL, RX, TX, PHYSICAL>
{
    type SoftwareFrame = TxFrame<'a, M>;
    type PhysicalFrame = PinnedTxFrame<'a, M, FRAME, HEAD, TAIL, PHYSICAL>;
    fn interface(&self) -> NetworkInterfaceId {
        self.endpoint.interface()
    }
    fn queue_len(&self) -> usize {
        self.endpoint.tx_queue_len()
    }
    fn try_take(&self) -> Option<TxFrame<'a, M>> {
        self.endpoint.try_receive_tx()
    }
    fn try_materialize(
        &self,
        frame: TxFrame<'a, M>,
    ) -> Result<Self::PhysicalFrame, TxFrame<'a, M>> {
        self.physical.try_materialize(frame)
    }
    fn try_materialize_next(&self) -> Option<Self::PhysicalFrame> {
        self.physical.try_materialize_from(|| self.try_take())
    }
    fn materialization_capacity(&self) -> usize {
        self.physical.promotion_capacity()
    }
    fn try_materialize_batch<const BATCH: usize>(
        &self,
        sources: &mut [Option<TxFrame<'a, M>>; BATCH],
        destinations: &mut [Option<Self::PhysicalFrame>; BATCH],
    ) -> bool {
        self.physical.try_materialize_batch(sources, destinations)
    }
    #[cfg(feature = "tx-phase-telemetry")]
    fn ownership_snapshot(
        &self,
    ) -> open_esp_radio_esp32s31_wifi_embassy::datapath::MaterializationOwnershipSnapshot {
        self.physical.ownership_snapshot()
    }
}

/// Two upstream devices composed with the existing STA/AP scheduler and one
/// physical TX arena. This type owns no IP stack or additional radio policy.
pub struct Network<
    'a,
    M: RawMutex,
    const FRAME: usize,
    const HEAD: usize,
    const TAIL: usize,
    const RX: usize,
    const TX: usize,
    const PHYSICAL: usize,
> {
    endpoints: [Endpoint<'a, M, RX, TX>; 2],
    physical: PinnedTxConsumer<'a, M, FRAME, HEAD, TAIL, PHYSICAL>,
}
impl<
    'a,
    M: RawMutex,
    const FRAME: usize,
    const HEAD: usize,
    const TAIL: usize,
    const RX: usize,
    const TX: usize,
    const PHYSICAL: usize,
> Network<'a, M, FRAME, HEAD, TAIL, RX, TX, PHYSICAL>
{
    pub fn new(
        first: Endpoint<'a, M, RX, TX>,
        second: Endpoint<'a, M, RX, TX>,
        physical: PinnedTxConsumer<'a, M, FRAME, HEAD, TAIL, PHYSICAL>,
    ) -> Self {
        assert_ne!(
            first.interface(),
            second.interface(),
            "logical interfaces must be distinct"
        );
        Self {
            endpoints: [first, second],
            physical,
        }
    }
    fn endpoint(&self, interface: NetworkInterfaceId) -> &Endpoint<'a, M, RX, TX> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.interface() == interface)
            .expect("network interface belongs to this radio")
    }
}
impl<
    'a,
    M: RawMutex + 'a,
    const FRAME: usize,
    const HEAD: usize,
    const TAIL: usize,
    const RX: usize,
    const TX: usize,
    const PHYSICAL: usize,
> DatapathNetwork for Network<'a, M, FRAME, HEAD, TAIL, RX, TX, PHYSICAL>
{
    type LinkController = Links<'a, M, RX, TX>;
    type RxPublisher = Rx<'a, M, RX, TX>;
    type TxFrame = TxFrame<'a, M>;
    type PhysicalTxFrame = PinnedTxFrame<'a, M, FRAME, HEAD, TAIL, PHYSICAL>;
    type TxConsumer<'network>
        = Consumer<'a, M, FRAME, HEAD, TAIL, RX, TX, PHYSICAL>
    where
        Self: 'network;
    fn link_controller(&self) -> Self::LinkController {
        Links(self.endpoints.map(|endpoint| endpoint.link_controller()))
    }
    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        Rx(self.endpoint(interface).rx_publisher())
    }
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.endpoint(interface)
            .link_controller()
            .set_link_state(state);
    }
    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.endpoint(interface).tx_queue_len()
    }
    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<TxFrame<'a, M>> {
        self.endpoint(interface).try_receive_tx()
    }
    async fn receive_tx(&self, interface: NetworkInterfaceId) -> TxFrame<'a, M> {
        self.endpoint(interface).receive_tx().await
    }
    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_> {
        Consumer {
            endpoint: *self.endpoint(interface),
            physical: self.physical.for_interface(interface),
        }
    }
    async fn wait_tx_ready(&self, interface: NetworkInterfaceId) {
        self.endpoint(interface).wait_tx_queue_len_at_least(1).await;
    }
    async fn wait_tx_queue_len_at_least(&self, interface: NetworkInterfaceId, minimum: usize) {
        self.endpoint(interface)
            .wait_tx_queue_len_at_least(minimum)
            .await;
    }
    async fn wait_tx_publication(&self) {
        select(
            self.endpoints[0].wait_tx_publication(),
            self.endpoints[1].wait_tx_publication(),
        )
        .await;
    }
}

impl<
    M: RawMutex,
    const FRAME: usize,
    const HEAD: usize,
    const TAIL: usize,
    const RX: usize,
    const TX: usize,
    const PHYSICAL: usize,
> open_esp_radio_wifi_embassy::station_network::StationNetworkLink
    for Network<'_, M, FRAME, HEAD, TAIL, RX, TX, PHYSICAL>
{
    fn publish_link_up(&self) {
        self.endpoints[0]
            .link_controller()
            .set_link_state(LinkState::Up);
    }
}
