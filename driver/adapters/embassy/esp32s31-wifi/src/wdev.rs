//! Role-neutral Embassy WDEV loop for one running ESP32-S31 Wi-Fi owner.
//!
//! WDEV owns IRQ/RX/TX/network arbitration. AP and STA semantics enter only
//! through [`WdevServices`]; the scheduler never imports either role crate.

use core::future::{Future, pending, ready};

use embassy_futures::{
    select::{Either, Either3, Either4, select, select3, select4},
    yield_now,
};
use embassy_time::{Duration, Instant, Timer};
use open_esp_radio_embassy_net::{
    DualPinnedNetworkRunner, LinkState, NetworkInterfaceId, PinnedNetworkRunner, PinnedRxPublisher,
    PinnedTxFrame, PinnedTxInterfaceConsumer, RawMutex, RxEnqueueError,
};
pub use open_esp_radio_esp32s31_wifi::tx::{WifiTxProgress, WifiTxWake};
pub use open_esp_radio_esp32s31_wifi::wdev::{
    WdevControlContext, WdevControlProgress, WdevRxProgress, WdevStopProgress,
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;

use crate::embassy_irq::EmbassyMacIrqRuntime;

pub mod services;

/// RX-only network publication capability exposed to one finite WDEV service.
/// It cannot observe or claim network-owned TX slots.
pub trait WdevNetworkRx {
    /// Number of copied RX frames still waiting in the owned network queue.
    fn queue_len(&self) -> usize;

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError>;

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError>;

    /// Poll the next publication credit without allocating a boxed future.
    fn poll_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()>;

    #[cfg(feature = "rx-delivery-observation")]
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError>;

    #[cfg(feature = "rx-delivery-observation")]
    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError>;
}

/// RX publication authority presented to one WDEV services graph.
///
/// Standalone roles use only `primary_mut`. Same-channel compositions must
/// select a concrete endpoint by identity after fact-only VIF routing. The
/// trait has no fallback from an unknown identity to the primary endpoint.
pub trait WdevNetworkRxSet {
    fn primary_mut(&mut self) -> &mut dyn WdevNetworkRx;

    fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn WdevNetworkRx>;

    fn pair_mut(
        &mut self,
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    ) -> Option<(&mut dyn WdevNetworkRx, &mut dyn WdevNetworkRx)>;

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

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize> WdevNetworkRxSet
    for PinnedRxPublisher<'_, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
{
    fn primary_mut(&mut self) -> &mut dyn WdevNetworkRx {
        self
    }

    fn get_mut(&mut self, _interface: NetworkInterfaceId) -> Option<&mut dyn WdevNetworkRx> {
        None
    }

    fn pair_mut(
        &mut self,
        _first: NetworkInterfaceId,
        _second: NetworkInterfaceId,
    ) -> Option<(&mut dyn WdevNetworkRx, &mut dyn WdevNetworkRx)> {
        None
    }
}

/// Addressed RX publication endpoints owned by one physical WDEV.
///
/// A same-channel STA+AP scheduler must select the logical endpoint only
/// after the common RX dispatcher has classified the retained 802.11 owner.
/// Returning `None` for an unknown identity keeps that failure explicit; the
/// caller must account and release the exact frame instead of publishing it
/// through whichever role happens to be active.
pub struct WdevNetworkRxEndpoints<A, B> {
    first_interface: NetworkInterfaceId,
    first: A,
    second_interface: NetworkInterfaceId,
    second: B,
}

impl<A, B> WdevNetworkRxEndpoints<A, B> {
    pub fn new(
        first_interface: NetworkInterfaceId,
        first: A,
        second_interface: NetworkInterfaceId,
        second: B,
    ) -> Self {
        assert_ne!(
            first_interface, second_interface,
            "WDEV RX endpoints require distinct interface identities"
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

    pub fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn WdevNetworkRx>
    where
        A: WdevNetworkRx,
        B: WdevNetworkRx,
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

impl<A: WdevNetworkRx, B: WdevNetworkRx> WdevNetworkRxSet for WdevNetworkRxEndpoints<A, B> {
    fn primary_mut(&mut self) -> &mut dyn WdevNetworkRx {
        &mut self.first
    }

    fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn WdevNetworkRx> {
        WdevNetworkRxEndpoints::get_mut(self, interface)
    }

    fn pair_mut(
        &mut self,
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    ) -> Option<(&mut dyn WdevNetworkRx, &mut dyn WdevNetworkRx)> {
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

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize> WdevNetworkRx
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

    #[cfg(feature = "rx-delivery-observation")]
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        PinnedRxPublisher::try_send_observed(self, frame, before_publish)
    }

    #[cfg(feature = "rx-delivery-observation")]
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

/// Radio-side network ownership consumed by [`WdevRunner`].
///
/// Single-VIF owners expose one RX endpoint. A dual owner selects between
/// permanent STA/AP RX endpoints while retaining the sole tagged TX consumer.
/// Role-specific semantics remain outside this scheduler contract.
pub trait WdevNetwork<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
{
    fn rx_publisher(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>;
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState);
    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize;
    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>;
    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > + '_;
    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_;
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
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>;

    /// Wait for and claim the next physical tagged TX lease.
    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > + '_;
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> WdevNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
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
    fn rx_publisher(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
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

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        assert_eq!(interface, self.interface());
        PinnedNetworkRunner::tx_queue_len(self)
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        assert_eq!(interface, self.interface());
        PinnedNetworkRunner::try_receive_tx(self)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        PinnedNetworkRunner::wait_tx_publication(self)
    }

    fn physical_tx_queue_len(&self) -> usize {
        PinnedNetworkRunner::tx_queue_len(self)
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        PinnedNetworkRunner::try_receive_tx(self)
    }

    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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
> WdevNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
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
    fn rx_publisher(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        DualPinnedNetworkRunner::rx_publisher(self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        DualPinnedNetworkRunner::set_link_state(self, interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        DualPinnedNetworkRunner::tx_consumer(self).queue_len_for(interface)
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        DualPinnedNetworkRunner::tx_consumer(self).try_receive_for(interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > + '_ {
        let tx = DualPinnedNetworkRunner::tx_consumer(self);
        async move { tx.receive_for(interface).await }
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

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        let tx = DualPinnedNetworkRunner::tx_consumer(self);
        async move { tx.wait_ready_for(interface).await }
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        DualPinnedNetworkRunner::wait_tx_publication(self)
    }

    fn physical_tx_queue_len(&self) -> usize {
        DualPinnedNetworkRunner::tx_queue_len(self)
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        DualPinnedNetworkRunner::try_receive_tx(self)
    }

    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > + '_ {
        DualPinnedNetworkRunner::receive_tx(self)
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
> WdevNetwork<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
    for &N
where
    M: RawMutex + 'resources,
    N: WdevNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        > + ?Sized,
{
    fn rx_publisher(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        N::rx_publisher(*self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        N::set_link_state(*self, interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        N::tx_queue_len(*self, interface)
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        N::try_receive_tx(*self, interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        N::wait_tx_publication(*self)
    }

    fn physical_tx_queue_len(&self) -> usize {
        N::physical_tx_queue_len(*self)
    }

    fn try_receive_physical_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        N::try_receive_physical_tx(*self)
    }

    fn receive_physical_tx(
        &self,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > + '_ {
        N::receive_physical_tx(*self)
    }
}

/// Maximum latency added while collecting an aggregate-sized network burst.
/// The target itself comes from the negotiated BlockAck window; this deadline
/// prevents sparse/control traffic from waiting indefinitely for that target.
// At the qualified 100+ Mbit/s offer rate, filling a negotiated 32-MPDU BA
// window from an empty network queue takes roughly 2 ms. Keep that latency
// explicit and bounded: a full window starts immediately, while sparse
// traffic cannot be held beyond this deadline.
const TX_BATCH_MAX_WAIT: Duration = Duration::from_millis(2);
const RX_TX_FAIRNESS_QUANTUM_FRAMES: u32 = 8;

/// Scheduler-owned limit for one role-specific protocol RX turn.
///
/// `None` means no network TX is waiting and the role may use its own bounded
/// batch. `Some` carries the exact remaining frame credit before WDEV owes the
/// queued TX side another transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WdevRxServiceContext {
    pub maximum_protocol_frames: Option<usize>,
}

fn rx_protocol_frame_budget(rx_frame_deficit: i64, network_tx_pending: bool) -> Option<usize> {
    if !network_tx_pending {
        return None;
    }
    let remaining = i64::from(RX_TX_FAIRNESS_QUANTUM_FRAMES)
        .saturating_sub(rx_frame_deficit)
        .max(1);
    Some(usize::try_from(remaining).unwrap_or(usize::MAX))
}

const fn should_collect_network_batch(preferred: usize, available: usize) -> bool {
    preferred > 1 && available != 0 && available < preferred
}

/// Terminal, non-error outcome of one role-neutral radio event loop.
///
/// Keeping this distinct from `()` prevents an outer station owner from
/// confusing a proved link loss with a runner that completed without a
/// lifecycle transition. The caller may use this edge to tear down the
/// network stack, release the connected epoch and start reassociation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevRunnerExit<E> {
    /// Role policy reached its terminal edge.
    Role(E),
    /// The outer lifecycle requested a finite stop. The runner waited
    /// for any active TX transaction to release hardware, published link-down
    /// and returned the same owners as a disconnect without claiming peer
    /// reachability had failed.
    Stopped,
}

/// Logical interfaces scheduled by one physical WDEV owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WdevInterfaceScope {
    Single(NetworkInterfaceId),
    Pair {
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    },
}

impl WdevInterfaceScope {
    pub fn pair(first: NetworkInterfaceId, second: NetworkInterfaceId) -> Self {
        assert_ne!(first, second, "WDEV pair requires distinct interfaces");
        Self::Pair { first, second }
    }

    pub const fn primary(self) -> NetworkInterfaceId {
        match self {
            Self::Single(interface)
            | Self::Pair {
                first: interface, ..
            } => interface,
        }
    }

    pub const fn contains(self, interface: NetworkInterfaceId) -> bool {
        match self {
            Self::Single(owned) => owned.value() == interface.value(),
            Self::Pair { first, second } => {
                first.value() == interface.value() || second.value() == interface.value()
            }
        }
    }

    pub const fn is_pair(self) -> bool {
        matches!(self, Self::Pair { .. })
    }
}

/// Finite chip-specific and role-specific operations used by [`WdevRunner`].
///
/// An implementation normally owns the live RX descriptor ring, staging
/// storage, staging publisher, TX descriptor state and a short-lived PAC
/// facade such as `CooperativeRadioHardware`. `service_rx` must snapshot and drain one
/// durable RX frontier into independent staging ownership. A separate
/// protocol consumer retains duplicate/protocol history.
///
/// Every method must finish after a bounded number of hardware observations.
/// A method may await a timer edge needed by a typed transaction, but it must
/// release every mutable PAC borrow before that edge. RX-before-TX arbitration
/// and the lifetime of a pinned network lease belong to [`WdevRunner::run`].
pub trait WdevServices<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
>
{
    type Error;
    type Exit;

    /// Drain one snapshotted RX-success frontier into independent ownership.
    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn WdevNetworkRxSet,
        context: WdevRxServiceContext,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a;

    /// Whether the role exposes a DMA-only RX producer that is safe while a
    /// TX transaction owns its protocol/control domain.
    fn can_service_rx_during_tx(&self) -> bool {
        true
    }

    /// Service RX work proven safe while an active TX owns its transaction.
    ///
    /// The default preserves roles whose ordinary RX service is already safe.
    /// A combined role must override this method and admit only its DMA
    /// producer plus protocol work that cannot execute control or hardware
    /// actions until the TX terminal edge.
    fn service_rx_during_tx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn WdevNetworkRxSet,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        self.service_rx(
            network_rx,
            WdevRxServiceContext {
                maximum_protocol_frames: None,
            },
        )
    }

    /// Whether role-owned software RX work is ready without a fresh hardware
    /// interrupt. This covers finite reorder deadlines and retained batches;
    /// it must not report speculative hardware work.
    fn has_rx_work(&self) -> bool {
        false
    }

    /// Monotonic number of protocol frames completed by this role.
    ///
    /// WDEV uses the delta across one service call for frame-based RX/TX
    /// fairness. DMA-only staging must not advance this counter: it releases
    /// hardware ownership but has not consumed a protocol scheduling turn.
    fn serviced_rx_frames(&self) -> u64 {
        0
    }

    /// Apply at most one owned control event or publish one control frame.
    ///
    /// The runner invokes this only while no network TX transaction owns the
    /// shared descriptor. `More` returns through the scheduler before a new
    /// network lease can be claimed; `TxPending` enters the same IRQ/deadline
    /// completion loop as ordinary data.
    fn service_control<'a>(
        &'a mut self,
        _context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        ready(Ok(WdevControlProgress::Idle))
    }

    /// VIF that owns a role-generated live TX transaction.
    ///
    /// Standalone runners infer their sole interface. A paired runner requires
    /// the combined services owner to report this identity before WDEV enters
    /// the shared IRQ/deadline completion loop.
    fn active_tx_interface(&self) -> Option<NetworkInterfaceId> {
        None
    }

    /// VIF retained by a software-owned standby aggregate.
    fn prepared_tx_interface(&self) -> Option<NetworkInterfaceId> {
        None
    }

    /// Advance role shutdown by one finite transition at an idle TX boundary.
    fn service_stop(&mut self) -> Result<WdevStopProgress, Self::Error> {
        Ok(WdevStopProgress::Stopped)
    }

    /// Wake the outer scheduler for a control timer or independently
    /// published control event. Backends without such a source never wake it.
    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        pending()
    }

    /// Transfer one network-owned frame into the MAC/DMA transaction.
    ///
    /// The services owner receives ownership rather than a temporary borrow. An
    /// ordinary copy-based transmitter may release the lease immediately;
    /// a referenced A-MPDU owner may retain it, claim further ready leases
    /// from `network`, and return all of them only after BlockAck/detach.
    fn start_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    /// Wait for the executor deadline of the active TX transaction.
    ///
    /// This future is created only after [`Self::start_tx`] returned
    /// [`WifiTxProgress::Pending`]. A retry may replace the active deadline;
    /// the runner creates a fresh future after every service operation.
    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_;

    /// Inspect, complete, abort or restart the active TX transaction.
    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    fn has_prepared_tx(&self) -> bool {
        false
    }

    /// Preferred number of MPDUs visible before starting a network batch.
    /// Non-aggregate services keep the default immediate single-frame path.
    fn preferred_tx_batch_size(&self) -> usize {
        1
    }

    /// MPDUs already retained in the software-owned standby arena.
    fn prepared_tx_frame_count(&self) -> usize {
        0
    }

    fn start_prepared_tx<'a>(
        &'a mut self,
        _network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        ready(Ok(WifiTxProgress::Complete))
    }

    fn cancel_prepared_tx(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn can_prepare_tx(&self) -> bool {
        false
    }

    fn prepare_tx<'a>(
        &'a mut self,
        _frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        _network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        ready(Ok(()))
    }
}

/// Single Embassy owner for RX DMA, control, network TX and MAC IRQ order.
pub struct WdevRunner<
    'resources,
    'irq,
    M: RawMutex,
    N,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    R = PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
> {
    resources: core::marker::PhantomData<&'resources ()>,
    irq: &'irq EmbassyMacIrqRuntime<M>,
    network: N,
    interfaces: WdevInterfaceScope,
    network_rx: R,
    services: B,
    active_tx_interface: Option<NetworkInterfaceId>,
    prepared_tx_interface: Option<NetworkInterfaceId>,
    rx_progress: WdevRxProgress,
    /// Signed RX-minus-TX frame balance. A negative value is retained across
    /// transactions so a large aggregate cannot erase the RX credit it
    /// consumed merely because the old unsigned counter saturated at zero.
    rx_frame_deficit: i64,
}

mod arbitration;
mod owner;
pub mod paired;
mod service;

#[cfg(test)]
mod tests;
