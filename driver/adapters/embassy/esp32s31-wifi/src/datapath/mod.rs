//! Role-neutral Embassy DATAPATH loop for one running ESP32-S31 Wi-Fi owner.
//!
//! DATAPATH owns IRQ/RX/TX/network arbitration. AP and STA semantics enter only
//! through [`DatapathServices`]; the scheduler never imports either role crate.

use core::future::{Future, pending, ready};
#[cfg(feature = "core0-rx-coarse-telemetry")]
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::datapath::irq::EmbassyMacIrqRuntime;
use crate::datapath::network::{DatapathNetwork, DatapathNetworkRxSet};
#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::PreparedTxSchedulerPhase;
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
    DatapathControlContext, DatapathControlProgress, DatapathRxProgress, DatapathRxWorkCounters,
    DatapathStopProgress,
};
pub use open_esp_radio_esp32s31_wifi::tx::{WifiTxProgress, WifiTxWake};

pub mod irq;
pub mod network;
pub mod rx;
pub mod services;
pub mod tx;

/// Maximum latency added while collecting one already-detected network burst.
// This is a burst-only deadline: the first frame after an observed quiet
// period bypasses it completely.
const TX_BATCH_MAX_WAIT: Duration = Duration::from_millis(2);
/// Maximum gap between single-frame admissions that still identifies one
/// continuous producer burst. Isolated traffic never enters batching mode.
const TX_BURST_ENTRY_GAP: Duration = Duration::from_millis(2);
/// Keep a proven burst warm across a short, explicitly observed empty
/// producer frontier. The next frame after a longer idle interval returns to
/// the immediate path; time spent inside an active hardware transaction does
/// not count as producer silence.
const TX_BURST_QUIET_TIMEOUT: Duration = Duration::from_millis(4);
const RX_TX_FAIRNESS_QUANTUM_FRAMES: u32 = 8;

#[cfg(feature = "core0-rx-coarse-telemetry")]
static RECYCLED_RX_PROBE_DELAY_MICROS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "core0-rx-coarse-telemetry")]
static ADAPTIVE_RECYCLED_RX_PROBE: AtomicBool = AtomicBool::new(false);

/// Configure a same-image delayed continuation for a masked recycled-only RX
/// frontier. Zero preserves the production immediate software repost.
///
/// The delay applies only after the first IRQ-originated service has finished;
/// it never delays an idle-to-active RX transition. Hardware terminal,
/// completion-frontier and budget continuations remain immediate.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub fn configure_recycled_rx_probe_delay_for_diagnostics(delay_micros: u32) {
    RECYCLED_RX_PROBE_DELAY_MICROS.store(delay_micros, Ordering::Relaxed);
}

/// Select the same-image adaptive continuation policy for coarse HIL.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub fn configure_adaptive_recycled_rx_probe_for_diagnostics(enabled: bool) {
    ADAPTIVE_RECYCLED_RX_PROBE.store(enabled, Ordering::Relaxed);
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
fn recycled_rx_probe_delay_for_diagnostics() -> Option<Duration> {
    let micros = RECYCLED_RX_PROBE_DELAY_MICROS.load(Ordering::Relaxed);
    (micros != 0).then(|| Duration::from_micros(u64::from(micros)))
}

/// Bound a masked post-append probe by the packet-rate implied by the last
/// physical batch. Small-frame streams use 512 us against the 96-entry
/// physical ring. Long-frame streams ramp through 128/256/512 us; their byte
/// rate bounds packet pressure well below the same reserve and may extend a
/// proven long-frame burst to 1024 us. A recycle-only confirmation retains the
/// class established earlier in the same masked drain epoch instead of
/// collapsing a sustained stream back to the 64-us bootstrap interval.
fn adaptive_recycled_rx_probe_delay(
    work: DatapathRxWorkCounters,
    current_level: u8,
) -> (Duration, u8) {
    let average_bytes = work
        .staged_bytes
        .checked_div(work.completed_units)
        .unwrap_or(0);
    if work.completed_units == 0 {
        let micros = match current_level {
            1 => 128,
            2 => 256,
            3 => 512,
            4 => 1_024,
            _ => 64,
        };
        return (Duration::from_micros(micros), current_level.min(4));
    }
    if average_bytes < 1_024 {
        return (Duration::from_micros(512), 3);
    }
    let next_level = if work.completed_units >= 4 {
        4
    } else {
        current_level.saturating_add(1).min(4)
    };
    let micros = match next_level {
        0 => 64,
        1 => 128,
        2 => 256,
        3 => 512,
        _ => 1_024,
    };
    (Duration::from_micros(micros), next_level)
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
fn adaptive_recycled_rx_probe_enabled() -> bool {
    ADAPTIVE_RECYCLED_RX_PROBE.load(Ordering::Relaxed)
}

#[cfg(not(feature = "core0-rx-coarse-telemetry"))]
fn adaptive_recycled_rx_probe_enabled() -> bool {
    true
}

const fn tx_lookahead_allowed(
    requested: bool,
    interfaces: DatapathInterfaceScope,
    origin: DatapathTxOrigin,
) -> bool {
    let _ = (requested, interfaces, origin);
    false
}

#[derive(Clone, Copy, Debug)]
struct TxBatchState {
    /// A started transaction may be followed directly by a prepared standby
    /// without the scheduler ever observing an empty publication frontier.
    continuation_armed: bool,
    burst: bool,
    idle_since: Option<Instant>,
    collection_deadline: Option<Instant>,
}

impl TxBatchState {
    const fn new() -> Self {
        Self {
            continuation_armed: false,
            burst: false,
            idle_since: None,
            collection_deadline: None,
        }
    }

    /// Return a bounded collection deadline only after queue history proves a
    /// burst. The first frame after a quiet period always remains immediate.
    fn collection_deadline(
        &mut self,
        preferred: usize,
        ready_frames: usize,
        now: Instant,
    ) -> Option<Instant> {
        if preferred <= 1 || ready_frames == 0 || ready_frames >= preferred {
            self.collection_deadline = None;
            if ready_frames >= preferred && preferred > 1 {
                self.burst = true;
            }
            return None;
        }

        if let Some(idle_since) = self.idle_since.take() {
            let quiet_for = now.duration_since(idle_since);
            if self.burst {
                if quiet_for >= TX_BURST_QUIET_TIMEOUT {
                    self.burst = false;
                    self.continuation_armed = false;
                }
            } else if self.continuation_armed && quiet_for <= TX_BURST_ENTRY_GAP {
                self.burst = true;
            } else {
                self.continuation_armed = false;
            }
        } else if self.continuation_armed {
            // No empty scheduler boundary separated this publication from the
            // preceding active transaction. This is a proven continuation
            // even when the hardware transaction itself exceeded the entry
            // time used for an observed empty gap.
            self.burst = true;
        }

        if ready_frames >= 2 {
            self.burst = true;
        }

        if !self.burst {
            self.collection_deadline = None;
            return None;
        }

        let deadline = *self
            .collection_deadline
            .get_or_insert(now + TX_BATCH_MAX_WAIT);
        if now < deadline {
            Some(deadline)
        } else {
            self.collection_deadline = None;
            None
        }
    }

    fn note_started(&mut self, frames: usize) {
        self.continuation_armed = true;
        self.idle_since = None;
        self.collection_deadline = None;
        if frames >= 2 {
            self.burst = true;
        }
    }

    fn note_idle(&mut self, now: Instant) {
        self.idle_since.get_or_insert(now);
        self.collection_deadline = None;
    }
}

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

/// Semantic owner of the currently active physical TX transaction.
///
/// Network completion returns directly to data arbitration. Control completion
/// re-arms the control machine because one terminal management edge may expose
/// another finite protocol transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DatapathTxOrigin {
    Network,
    Control,
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
/// durable RX frontier into bounded descriptor-backed upper ownership. A separate
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

    /// Monotonic physical units and bytes completed by the RX DMA owner.
    fn rx_work_counters(&self) -> DatapathRxWorkCounters {
        DatapathRxWorkCounters::default()
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

    /// Whether control work is ready at this transaction boundary.
    ///
    /// Implementations must make this an O(1) observation. Timed roles retain
    /// one absolute deadline; mailbox-driven roles inspect their published
    /// readiness state. The conservative default preserves custom services
    /// which have not specialised their control scheduler.
    fn control_ready(&self, _now_micros: u64) -> bool {
        true
    }

    fn control_required_before_network_tx(&self) -> bool {
        false
    }

    fn control_required_before_stop(&self) -> bool {
        false
    }

    /// VIF that owns a role-generated live TX transaction.
    ///
    /// Standalone runners infer their sole interface. A paired runner requires
    /// the combined services owner to report this identity before DATAPATH enters
    /// the shared IRQ/deadline completion loop.
    fn active_tx_interface(&self) -> Option<NetworkInterfaceId> {
        None
    }

    /// Whether role work published a live TX transaction inside the current
    /// service call.
    ///
    /// A paired owner reports this through [`Self::active_tx_interface`]. A
    /// single-interface role may leave the identity implicit, but must still
    /// expose the ownership edge so the runner adopts the transaction before
    /// observing stop or any other idle-boundary work.
    fn has_active_tx(&self) -> bool {
        self.active_tx_interface().is_some()
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

    /// Preferred batch for a concrete logical interface before its first
    /// frame is claimed. Standalone services use one policy; paired services
    /// override this to preserve independent STA/AP aggregation contracts.
    fn preferred_tx_batch_size_for(&self, interface: NetworkInterfaceId) -> usize {
        let _ = interface;
        self.preferred_tx_batch_size()
    }

    /// MPDUs already retained in the software-owned standby arena.
    fn prepared_tx_frame_count(&self) -> usize {
        0
    }

    /// Retain one diagnostic phase boundary for a later typed publication
    /// observation. This method and all its call sites are absent from an
    /// ordinary build.
    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_tx_scheduler_phase(
        &mut self,
        _phase: PreparedTxSchedulerPhase,
        _at_micros: u64,
    ) {
    }

    fn start_prepared_tx(
        &mut self,
        _network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Self::Error> {
        Ok(WifiTxProgress::Complete)
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
    active_tx_origin: Option<DatapathTxOrigin>,
    prepared_tx_interface: Option<NetworkInterfaceId>,
    control_ready_latched: bool,
    rx_progress: DatapathRxProgress,
    recycled_rx_probe_deadline: Option<Instant>,
    recycled_rx_probe_coalescing_level: u8,
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
