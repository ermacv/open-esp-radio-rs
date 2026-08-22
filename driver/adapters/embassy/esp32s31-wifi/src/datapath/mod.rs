//! Role-neutral Embassy DATAPATH loop for one running ESP32-S31 Wi-Fi owner.
//!
//! DATAPATH owns IRQ/RX/TX/network arbitration. AP and STA semantics enter only
//! through [`DatapathServices`]; the scheduler never imports either role crate.

use core::future::{Future, pending, ready};

use crate::datapath::irq::EmbassyMacIrqRuntime;
use crate::datapath::network::{DatapathNetwork, DatapathNetworkRxSet};
use embassy_futures::{
    select::{Either, Either3, Either4, select, select3, select4},
    yield_now,
};
use embassy_time::{Duration, Instant, Timer};
use open_esp_radio_embassy_net::{
    LinkState, NetworkInterfaceId, PinnedRxPublisher, PinnedTxFrame, PinnedTxInterfaceConsumer,
    RawMutex,
};
pub use open_esp_radio_esp32s31_wifi::datapath::{
    DatapathControlContext, DatapathControlProgress, DatapathRxProgress, DatapathStopProgress,
};
pub use open_esp_radio_esp32s31_wifi::tx::{WifiTxProgress, WifiTxWake};

pub mod irq;
pub mod network;
pub mod rx;
pub mod services;
pub mod tx;

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
/// batch. `Some` carries the exact remaining frame credit before DATAPATH owes the
/// queued TX side another transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatapathRxServiceContext {
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
pub enum DatapathRunnerExit<E> {
    /// Role policy reached its terminal edge.
    Role(E),
    /// The outer lifecycle requested a finite stop. The runner waited
    /// for any active TX transaction to release hardware, published link-down
    /// and returned the same owners as a disconnect without claiming peer
    /// reachability had failed.
    Stopped,
}

/// Logical interfaces scheduled by one physical DATAPATH owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathInterfaceScope {
    Single(NetworkInterfaceId),
    Pair {
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    },
}

impl DatapathInterfaceScope {
    pub fn pair(first: NetworkInterfaceId, second: NetworkInterfaceId) -> Self {
        assert_ne!(first, second, "DATAPATH pair requires distinct interfaces");
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

/// Finite chip-specific and role-specific operations used by [`DatapathRunner`].
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
/// and the lifetime of a pinned network lease belong to [`DatapathRunner::run`].
pub trait DatapathServices<
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
        network_rx: &'a mut dyn DatapathNetworkRxSet,
        context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a;

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
        network_rx: &'a mut dyn DatapathNetworkRxSet,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        self.service_rx(
            network_rx,
            DatapathRxServiceContext {
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
    /// DATAPATH uses the delta across one service call for frame-based RX/TX
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
        _context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        ready(Ok(DatapathControlProgress::Idle))
    }

    /// VIF that owns a role-generated live TX transaction.
    ///
    /// Standalone runners infer their sole interface. A paired runner requires
    /// the combined services owner to report this identity before DATAPATH enters
    /// the shared IRQ/deadline completion loop.
    fn active_tx_interface(&self) -> Option<NetworkInterfaceId> {
        None
    }

    /// VIF retained by a software-owned standby aggregate.
    fn prepared_tx_interface(&self) -> Option<NetworkInterfaceId> {
        None
    }

    /// Advance role shutdown by one finite transition at an idle TX boundary.
    fn service_stop(&mut self) -> Result<DatapathStopProgress, Self::Error> {
        Ok(DatapathStopProgress::Stopped)
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

    /// Number of network leases claimed by the most recent successful
    /// [`Self::start_tx`] call.
    ///
    /// Aggregate encoders may synchronously drain more ready leases after the
    /// first frame is transferred by the runner. Fairness must charge every
    /// claimed lease, not merely the explicit first argument.
    fn last_started_tx_frame_count(&self) -> usize {
        1
    }

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
pub struct DatapathRunner<
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
    interfaces: DatapathInterfaceScope,
    network_rx: R,
    services: B,
    active_tx_interface: Option<NetworkInterfaceId>,
    prepared_tx_interface: Option<NetworkInterfaceId>,
    rx_progress: DatapathRxProgress,
    /// Signed RX-minus-TX frame balance. A negative value is retained across
    /// transactions so a large aggregate cannot erase the RX credit it
    /// consumed merely because the old unsigned counter saturated at zero.
    rx_frame_deficit: i64,
    /// Relative network frames admitted for each VIF while both members of a
    /// pair are backlogged. Selection follows the smaller total, so unequal
    /// aggregate sizes do not turn transaction round-robin into airtime-sized
    /// starvation. The counters reset whenever only one VIF is runnable.
    pair_tx_served_frames: [u64; 2],
}

mod owner;
pub mod paired;
mod scheduler;
mod service;

#[cfg(test)]
mod tests;
