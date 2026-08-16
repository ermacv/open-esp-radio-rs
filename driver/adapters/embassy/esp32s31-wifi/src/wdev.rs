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
    LinkState, PinnedRxPublisher, PinnedTxConsumer, PinnedTxFrame, RawMutex, RxEnqueueError,
    SplitPinnedRadioRunner,
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
    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError>;

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError>;

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

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize> WdevNetworkRx
    for PinnedRxPublisher<'_, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
{
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
/// An owned endpoint is returned by `into_parts` for station reassociation. A
/// temporary AP epoch may instead pass a borrow of the same persistent
/// endpoint; both expose exactly the same RX/TX queue operations.
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
    fn rx_publisher(&self) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>;
    fn set_link_state(&self, state: LinkState);
    fn tx_queue_len(&self) -> usize;
    fn try_receive_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>;
    fn receive_tx(
        &self,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > + '_;
    fn tx_consumer(
        &self,
    ) -> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    fn wait_tx_ready(&self) -> impl Future<Output = ()> + '_;
    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_;
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
    for SplitPinnedRadioRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    fn rx_publisher(&self) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        SplitPinnedRadioRunner::rx_publisher(self)
    }

    fn set_link_state(&self, state: LinkState) {
        SplitPinnedRadioRunner::set_link_state(self, state);
    }

    fn tx_queue_len(&self) -> usize {
        SplitPinnedRadioRunner::tx_queue_len(self)
    }

    fn try_receive_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        SplitPinnedRadioRunner::try_receive_tx(self)
    }

    fn receive_tx(
        &self,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > + '_ {
        SplitPinnedRadioRunner::receive_tx(self)
    }

    fn tx_consumer(
        &self,
    ) -> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> {
        SplitPinnedRadioRunner::tx_consumer(self)
    }

    fn wait_tx_ready(&self) -> impl Future<Output = ()> + '_ {
        SplitPinnedRadioRunner::wait_tx_ready(self)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        SplitPinnedRadioRunner::wait_tx_publication(self)
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
    fn rx_publisher(&self) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        N::rx_publisher(*self)
    }

    fn set_link_state(&self, state: LinkState) {
        N::set_link_state(*self, state);
    }

    fn tx_queue_len(&self) -> usize {
        N::tx_queue_len(*self)
    }

    fn try_receive_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        N::try_receive_tx(*self)
    }

    fn receive_tx(
        &self,
    ) -> impl Future<
        Output = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    > + '_ {
        N::receive_tx(*self)
    }

    fn tx_consumer(
        &self,
    ) -> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> {
        N::tx_consumer(*self)
    }

    fn wait_tx_ready(&self) -> impl Future<Output = ()> + '_ {
        N::wait_tx_ready(*self)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        N::wait_tx_publication(*self)
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
        network_rx: &'a mut dyn WdevNetworkRx,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a;

    /// Whether RX hardware may be inspected while a TX transaction owns the
    /// shared descriptor domain.
    fn service_rx_during_tx(&self) -> bool {
        true
    }

    /// Whether role-owned software RX work is ready without a fresh hardware
    /// interrupt. This covers finite reorder deadlines and retained batches;
    /// it must not report speculative hardware work.
    fn has_rx_work(&self) -> bool {
        false
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
        network: &'a PinnedTxConsumer<
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
        _network: &'a PinnedTxConsumer<
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
        _network: &'a PinnedTxConsumer<
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
> {
    irq: &'irq EmbassyMacIrqRuntime<M>,
    network: N,
    network_rx: PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    services: B,
    rx_progress: WdevRxProgress,
    network_turn_owed: bool,
}

mod arbitration;
mod owner;
mod service;

#[cfg(test)]
mod tests;
